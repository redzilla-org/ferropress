//! Function URL (payload format 2.0) event in, Function URL response out.

use std::collections::HashMap;

use serde_json::{Value, json};

/// The event could not be translated.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventError {
    /// The invocation payload is not a Function URL event.
    #[error("invocation is not a function URL event: {0}")]
    Shape(&'static str),
}

/// The parts of a Function URL request the comment endpoint reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// HTTP method, upper case.
    pub method: Box<str>,
    /// Headers with lower-case names (Function URLs deliver them lower-cased).
    pub headers: HashMap<String, String>,
    /// The body, base64-decoded when the event says so.
    pub body: Vec<u8>,
    /// The client address: `CloudFront`'s viewer address when present, else the source IP.
    pub ip: Box<str>,
}

impl Request {
    /// A header value by lower-case name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// A response for the Function URL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// Headers other than `Set-Cookie`.
    pub headers: Vec<(&'static str, String)>,
    /// `Set-Cookie` values.
    pub cookies: Vec<String>,
    /// UTF-8 body.
    pub body: String,
}

/// Decodes a Function URL event.
///
/// # Errors
///
/// [`EventError::Shape`] when the method is missing or a base64 body is malformed.
pub fn decode(payload: &[u8]) -> Result<Request, EventError> {
    let event: Value =
        serde_json::from_slice(payload).map_err(|_not_json| EventError::Shape("not JSON"))?;
    let method = event
        .pointer("/requestContext/http/method")
        .and_then(Value::as_str)
        .ok_or(EventError::Shape("requestContext.http.method"))?;
    let headers: HashMap<String, String> = event
        .get("headers")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| Some((name.to_ascii_lowercase(), value.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let text = event.get("body").and_then(Value::as_str).unwrap_or("");
    let body = if event.get("isBase64Encoded").and_then(Value::as_bool) == Some(true) {
        base64(text).ok_or(EventError::Shape("body is not base64"))?
    } else {
        text.as_bytes().to_vec()
    };
    // CloudFront sends `ip:port`; the function URL's own source IP is CloudFront's edge.
    let ip = headers
        .get("cloudfront-viewer-address")
        .and_then(|address| address.rsplit_once(':').map(|(ip, _port)| ip.to_owned()))
        .or_else(|| {
            event
                .pointer("/requestContext/http/sourceIp")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    Ok(Request {
        method: method.into(),
        headers,
        body,
        ip: ip.into(),
    })
}

/// Encodes a response as the Function URL response JSON.
#[must_use]
pub fn encode(response: &Response) -> Vec<u8> {
    let headers: serde_json::Map<String, Value> = response
        .headers
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::String(value.clone())))
        .collect();
    json!({
        "statusCode": response.status,
        "headers": headers,
        "cookies": response.cookies,
        "body": response.body,
        "isBase64Encoded": false,
    })
    .to_string()
    .into_bytes()
}

/// `application/x-www-form-urlencoded` pairs; the first occurrence of a name wins.
#[must_use]
pub fn form(body: &[u8]) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for pair in body.split(|&byte| byte == b'&').filter(|pair| !pair.is_empty()) {
        let (name, value) = match pair.iter().position(|&byte| byte == b'=') {
            Some(at) => (&pair[..at], &pair[at + 1..]),
            None => (pair, &[][..]),
        };
        fields
            .entry(unescape(name))
            .or_insert_with(|| unescape(value));
    }
    fields
}

// `+` is a space and `%XX` a byte; a malformed escape stays literal.
fn unescape(text: &[u8]) -> String {
    let mut out = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        match text[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < text.len() + 0 && hex(text[index + 1]).is_some() && hex(text[index + 2]).is_some() => {
                out.push(hex(text[index + 1]).unwrap_or(0) * 16 + hex(text[index + 2]).unwrap_or(0));
                index += 2;
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

// Standard alphabet with optional padding; whitespace is not allowed.
fn base64(text: &str) -> Option<Vec<u8>> {
    let trimmed = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut buffer = 0_u32;
    let mut bits = 0;
    for byte in trimmed.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{EventError, Response, base64, decode, encode, form, unescape};

    #[test]
    fn function_url_posts_decode_with_the_viewer_address() {
        let event = br#"{"version":"2.0","rawPath":"/blog/comments",
            "headers":{"content-type":"application/x-www-form-urlencoded",
                "cloudfront-viewer-address":"2001:db8::1:443","user-agent":"UA"},
            "body":"Y29tbWVudD1oaQ==","isBase64Encoded":true,
            "requestContext":{"http":{"method":"POST","sourceIp":"198.51.100.1"}}}"#;
        let request = decode(event);
        assert!(request.is_ok());
        if let Ok(request) = request {
            assert_eq!(&*request.method, "POST");
            assert_eq!(&*request.ip, "2001:db8::1");
            assert_eq!(request.body, b"comment=hi");
            assert_eq!(request.header("user-agent"), Some("UA"));
            assert_eq!(request.header("cookie"), None);
        }
        let plain = br#"{"body":"a=b","requestContext":{"http":{"method":"GET","sourceIp":"198.51.100.1"}}}"#;
        assert!(decode(plain).is_ok_and(|r| &*r.ip == "198.51.100.1" && r.body == b"a=b"));
        let bare = br#"{"requestContext":{"http":{"method":"POST"}}}"#;
        assert!(decode(bare).is_ok_and(|r| r.ip.is_empty() && r.body.is_empty()));
    }

    #[test]
    fn malformed_events_name_what_is_wrong() {
        assert_eq!(decode(b"nope"), Err(EventError::Shape("not JSON")));
        assert_eq!(decode(b"{}"), Err(EventError::Shape("requestContext.http.method")));
        let bad = br#"{"body":"!!","isBase64Encoded":true,"requestContext":{"http":{"method":"POST"}}}"#;
        assert_eq!(
            decode(bad).map_err(|e| e.to_string()),
            Err("invocation is not a function URL event: body is not base64".to_owned())
        );
    }

    #[test]
    fn responses_carry_cookies_separately() {
        let response = Response {
            status: 302,
            headers: vec![("location", "/blog/".to_owned())],
            cookies: vec!["a=b".to_owned()],
            body: String::new(),
        };
        let text = String::from_utf8(encode(&response));
        assert_eq!(
            text.as_deref(),
            Ok(r#"{"body":"","cookies":["a=b"],"headers":{"location":"/blog/"},"isBase64Encoded":false,"statusCode":302}"#)
        );
    }

    #[test]
    fn forms_and_base64_decode() {
        let fields = form(b"author=Ada+L&email=ada%40example.com&flag&author=dup&&bad=%zz%4");
        assert_eq!(fields.get("author").map(String::as_str), Some("Ada L"));
        assert_eq!(fields.get("email").map(String::as_str), Some("ada@example.com"));
        assert_eq!(fields.get("flag").map(String::as_str), Some(""));
        assert_eq!(fields.get("bad").map(String::as_str), Some("%zz%4"));
        assert_eq!(unescape(b"%C3%A9"), "\u{e9}");
        assert_eq!(base64("aGk="), Some(b"hi".to_vec()));
        assert_eq!(base64("aGkh"), Some(b"hi!".to_vec()));
        assert_eq!(base64("+/8="), Some(vec![0xfb, 0xff]));
        assert_eq!(base64(""), Some(Vec::new()));
        assert_eq!(base64("a b"), None);
    }
}
