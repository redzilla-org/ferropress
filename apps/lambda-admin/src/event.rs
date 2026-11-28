//! Function URL (payload format 2.0) event in, Function URL response out.

use cms_authoring::{Method, Request, Response};
use serde_json::{Map, Value, json};

/// The event or the response could not be translated.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventError {
    /// The invocation payload is not a Function URL event.
    #[error("invocation is not a function URL event: {0}")]
    Shape(&'static str),
    /// The admin only answers text (HTML, JSON, JavaScript); other bytes are a defect.
    #[error("response body is not UTF-8")]
    NotUtf8,
}

/// Decodes a Function URL event, including a base64 body (Function URLs encode form posts).
///
/// # Errors
///
/// [`EventError::Shape`] when a required field is missing or the body is not valid base64.
pub fn decode(payload: &[u8]) -> Result<Request, EventError> {
    let event: Value =
        serde_json::from_slice(payload).map_err(|_not_json| EventError::Shape("not JSON"))?;
    let text = |pointer: &str| event.pointer(pointer).and_then(Value::as_str);
    let path = text("/rawPath").ok_or(EventError::Shape("rawPath"))?;
    let method = text("/requestContext/http/method")
        .ok_or(EventError::Shape("requestContext.http.method"))?;
    // Function URLs deliver header names lower-cased, and cookies as their own array.
    let headers = event
        .get("headers")
        .and_then(Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let cookies = event
        .get("cookies")
        .and_then(Value::as_array)
        .map(|cookies| {
            cookies
                .iter()
                .filter_map(|cookie| cookie.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let raw = text("/body").unwrap_or_default();
    let body = if event.get("isBase64Encoded").and_then(Value::as_bool) == Some(true) {
        base64(raw).ok_or(EventError::Shape("body is not base64"))?
    } else {
        raw.as_bytes().to_vec()
    };
    Ok(Request {
        method: Method::parse(method),
        path: path.to_owned(),
        query: text("/rawQueryString").unwrap_or_default().to_owned(),
        headers,
        cookies,
        body,
    })
}

/// Encodes a response as the Function URL response JSON.
///
/// # Errors
///
/// [`EventError::NotUtf8`] when the body is not UTF-8.
pub fn encode(response: Response) -> Result<Vec<u8>, EventError> {
    let body = String::from_utf8(response.body).map_err(|_not_utf8| EventError::NotUtf8)?;
    let headers: Map<String, Value> = response
        .headers
        .into_iter()
        .map(|(name, value)| (name.to_owned(), Value::String(value)))
        .collect();
    let reply = json!({
        "statusCode": response.status,
        "headers": headers,
        "body": body,
        "isBase64Encoded": false,
    });
    Ok(reply.to_string().into_bytes())
}

// Standard-alphabet base64 with optional padding; anything else is malformed. Small enough not
// to justify a new dependency for one decode per request.
fn base64(text: &str) -> Option<Vec<u8>> {
    let digits = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in digits {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = buffer << 6 | u32::from(value);
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
    use super::{EventError, base64, decode, encode};
    use cms_authoring::{Method, Response};

    #[test]
    fn function_url_events_decode_with_cookies_headers_and_bodies() {
        let event = br#"{"version":"2.0","rawPath":"/blog/api/posts","rawQueryString":"page=2",
            "cookies":["fp_session=tok","x=1"],
            "headers":{"x-fp-nonce":"n1","content-type":"application/json"},
            "body":"{\"title\":\"T\"}","isBase64Encoded":false,
            "requestContext":{"http":{"method":"POST"}}}"#;
        let request = decode(event);
        assert!(request.is_ok(), "{request:?}");
        if let Ok(request) = request {
            assert_eq!(request.method, Method::Post);
            assert_eq!(request.path, "/blog/api/posts");
            assert_eq!(request.query, "page=2");
            assert_eq!(request.cookie("fp_session"), Some("tok"));
            assert_eq!(request.header("x-fp-nonce"), Some("n1"));
            assert_eq!(request.body, br#"{"title":"T"}"#);
        }
        let form = br#"{"rawPath":"/blog/admin/posts","body":"ZG89ZHJhZnQmdGl0bGU9SGk=",
            "isBase64Encoded":true,"requestContext":{"http":{"method":"POST"}}}"#;
        assert!(decode(form).is_ok_and(|r| r.body == b"do=draft&title=Hi" && r.cookies.is_empty()));
        let get = br#"{"rawPath":"/blog/admin/","requestContext":{"http":{"method":"GET"}}}"#;
        assert!(decode(get).is_ok_and(|r| r.body.is_empty() && r.query.is_empty()));
    }

    #[test]
    fn malformed_events_name_what_is_wrong() {
        assert_eq!(decode(b"nope"), Err(EventError::Shape("not JSON")));
        assert_eq!(decode(b"{}"), Err(EventError::Shape("rawPath")));
        assert_eq!(
            decode(br#"{"rawPath":"/"}"#),
            Err(EventError::Shape("requestContext.http.method"))
        );
        let bad = br#"{"rawPath":"/","body":"!!","isBase64Encoded":true,
            "requestContext":{"http":{"method":"POST"}}}"#;
        assert_eq!(decode(bad), Err(EventError::Shape("body is not base64")));
    }

    #[test]
    fn base64_decodes_padded_and_unpadded() {
        assert_eq!(base64("aGk="), Some(b"hi".to_vec()));
        assert_eq!(base64("aGk"), Some(b"hi".to_vec()));
        assert_eq!(base64("YWJj"), Some(b"abc".to_vec()));
        assert_eq!(base64("+/8="), Some(vec![0xfb, 0xff]));
        assert_eq!(base64(""), Some(Vec::new()));
    }

    #[test]
    fn responses_encode_status_headers_and_text_body() {
        let response = Response::redirect(303, "/blog/admin/");
        let encoded = encode(response).map(String::from_utf8);
        assert_eq!(
            encoded,
            Ok(Ok(
                r#"{"body":"","headers":{"cache-control":"no-store","content-type":"text/plain; charset=UTF-8","location":"/blog/admin/"},"isBase64Encoded":false,"statusCode":303}"#
                    .to_owned()
            ))
        );
        let binary = Response::new(200, "image/png", vec![0xff]);
        assert_eq!(encode(binary), Err(EventError::NotUtf8));
    }
}
