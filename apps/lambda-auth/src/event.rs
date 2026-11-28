//! Function URL (payload format 2.0) event in, Function URL response out.

use serde_json::{Map, Value, json};

/// The invocation payload is not a Function URL event.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("invocation is not a function URL event: {0}")]
pub struct EventError(pub &'static str);

/// The parts of a request the auth routes read. [`Debug`] omits the body and the cookie,
/// which carry the password and the session token.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthRequest {
    /// HTTP method, upper case.
    pub method: String,
    /// Raw path, such as `/blog/login`.
    pub path: String,
    /// The Cookie header (Function URLs deliver cookies as an array; they are rejoined).
    pub cookie: String,
    /// Decoded request body.
    pub body: Vec<u8>,
    /// Client address.
    pub ip: String,
    /// User-Agent header.
    pub user_agent: String,
}

impl std::fmt::Debug for AuthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRequest")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("ip", &self.ip)
            .finish_non_exhaustive()
    }
}

/// A response. [`Debug`] omits the cookies, which carry the session token.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthReply {
    /// HTTP status.
    pub status: u16,
    /// Response headers other than Set-Cookie.
    pub headers: Vec<(&'static str, String)>,
    /// Set-Cookie values.
    pub cookies: Vec<String>,
    /// HTML body (empty for redirects).
    pub body: String,
}

impl std::fmt::Debug for AuthReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthReply")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("cookies", &self.cookies.len())
            .finish_non_exhaustive()
    }
}

/// Decodes a Function URL event.
///
/// # Errors
///
/// [`EventError`] naming the missing or malformed field.
pub fn decode(payload: &[u8]) -> Result<AuthRequest, EventError> {
    let event: Value = serde_json::from_slice(payload).map_err(|_not_json| EventError("not JSON"))?;
    let text = |pointer: &str| event.pointer(pointer).and_then(Value::as_str);
    let path = text("/rawPath").ok_or(EventError("rawPath"))?;
    let method = text("/requestContext/http/method").ok_or(EventError("requestContext.http.method"))?;
    let cookie = match event.get("cookies") {
        None => String::new(),
        Some(Value::Array(cookies)) => cookies
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("; "),
        Some(_) => return Err(EventError("cookies")),
    };
    let raw = text("/body").unwrap_or_default();
    let body = if event.get("isBase64Encoded").and_then(Value::as_bool) == Some(true) {
        base64_decode(raw).ok_or(EventError("body is not base64"))?
    } else {
        raw.as_bytes().to_vec()
    };
    // Behind CloudFront the peer is the edge; CloudFront appends the viewer address as the
    // last X-Forwarded-For entry, and a client cannot write past it.
    let ip = text("/headers/x-forwarded-for")
        .and_then(|list| list.rsplit(',').next())
        .map(str::trim)
        .filter(|ip| !ip.is_empty())
        .or_else(|| text("/requestContext/http/sourceIp"))
        .ok_or(EventError("source address"))?;
    Ok(AuthRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        cookie,
        body,
        ip: ip.to_owned(),
        user_agent: text("/headers/user-agent").unwrap_or_default().to_owned(),
    })
}

/// Encodes a reply as the Function URL response JSON; cookies go in the `cookies` array.
#[must_use]
pub fn encode(reply: AuthReply) -> Vec<u8> {
    let headers: Map<String, Value> = reply
        .headers
        .into_iter()
        .map(|(name, value)| (name.to_owned(), Value::String(value)))
        .collect();
    json!({
        "statusCode": reply.status,
        "headers": headers,
        "cookies": reply.cookies,
        "body": reply.body,
        "isBase64Encoded": false,
    })
    .to_string()
    .into_bytes()
}

// Standard-alphabet base64 with optional padding, as Function URLs send binary bodies.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(u32::from(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    };
    let digits = text.trim_end_matches('=').as_bytes();
    if digits.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let mut acc = 0_u32;
        for &c in chunk {
            acc = (acc << 6) | value(c)?;
        }
        // Left-align the bits, then emit one byte per full 8 bits present.
        acc <<= 6 * (4 - u32::try_from(chunk.len()).ok()?);
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{AuthReply, EventError, base64_decode, decode, encode};

    #[test]
    fn events_decode_with_cookies_base64_bodies_and_the_viewer_address() {
        let event = br#"{"rawPath":"/blog/login","cookies":["a=1","fp_session=x"],
            "headers":{"x-forwarded-for":"10.0.0.9, 198.51.100.7","user-agent":"UA"},
            "body":"bG9nPWFkYSZwd2Q9cHc=","isBase64Encoded":true,
            "requestContext":{"http":{"method":"POST","sourceIp":"203.0.113.1"}}}"#;
        let request = decode(event).unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/blog/login");
        assert_eq!(request.cookie, "a=1; fp_session=x");
        assert_eq!(request.body, b"log=ada&pwd=pw");
        assert_eq!(request.ip, "198.51.100.7");
        assert_eq!(request.user_agent, "UA");
        // Debug carries neither the body (password) nor the cookie (token).
        let shown = format!("{request:?}");
        assert!(!shown.contains("pwd") && !shown.contains("fp_session"));
        let plain = br#"{"rawPath":"/blog/login","body":"x=1",
            "requestContext":{"http":{"method":"GET","sourceIp":"203.0.113.1"}}}"#;
        let request = decode(plain).unwrap();
        assert_eq!((request.ip.as_str(), &request.body[..]), ("203.0.113.1", &b"x=1"[..]));
        assert!(request.cookie.is_empty());
    }

    #[test]
    fn malformed_events_name_what_is_wrong() {
        assert_eq!(decode(b"nope"), Err(EventError("not JSON")));
        assert_eq!(decode(b"{}"), Err(EventError("rawPath")));
        assert_eq!(
            decode(br#"{"rawPath":"/"}"#),
            Err(EventError("requestContext.http.method"))
        );
        let base = r#""rawPath":"/","requestContext":{"http":{"method":"GET"}}"#;
        assert_eq!(
            decode(format!("{{{base}}}").as_bytes()),
            Err(EventError("source address"))
        );
        assert_eq!(
            decode(format!(r#"{{{base},"cookies":"x"}}"#).as_bytes()),
            Err(EventError("cookies"))
        );
        assert_eq!(
            decode(format!(r#"{{{base},"body":"*","isBase64Encoded":true}}"#).as_bytes()),
            Err(EventError("body is not base64"))
        );
        assert_eq!(
            EventError("x").to_string(),
            "invocation is not a function URL event: x"
        );
    }

    #[test]
    fn replies_encode_cookies_separately_and_hide_them_from_debug() {
        let reply = AuthReply {
            status: 302,
            headers: vec![("location", "/blog/admin/".to_owned())],
            cookies: vec!["fp_session=secret".to_owned()],
            body: String::new(),
        };
        assert!(!format!("{reply:?}").contains("secret"));
        assert_eq!(
            String::from_utf8(encode(reply)).unwrap(),
            r#"{"body":"","cookies":["fp_session=secret"],"headers":{"location":"/blog/admin/"},"isBase64Encoded":false,"statusCode":302}"#
        );
    }

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        // RFC 4648 section 10 test vectors.
        for (encoded, plain) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ] {
            assert_eq!(base64_decode(encoded), Some(plain.as_bytes().to_vec()), "{encoded}");
        }
        assert_eq!(base64_decode("Z"), None);
        assert_eq!(base64_decode("Z-=="), None);
    }
}
