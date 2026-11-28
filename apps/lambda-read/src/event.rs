//! Function URL (payload format 2.0) event in, Function URL response out.

use cms_readmodel::{Method, Request, Response};
use serde_json::{Map, Value, json};

/// The event or the response could not be translated.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventError {
    /// The invocation payload is not a Function URL event.
    #[error("invocation is not a function URL event: {0}")]
    Shape(&'static str),
    /// The read path only serves text; the projector writes UTF-8, so other bytes are a defect.
    #[error("response body is not UTF-8")]
    NotUtf8,
}

/// Decodes a Function URL event.
///
/// # Errors
///
/// [`EventError::Shape`] when `rawPath` or the HTTP method is missing.
pub fn decode(payload: &[u8]) -> Result<Request, EventError> {
    let event: Value =
        serde_json::from_slice(payload).map_err(|_not_json| EventError::Shape("not JSON"))?;
    let path = event
        .get("rawPath")
        .and_then(Value::as_str)
        .ok_or(EventError::Shape("rawPath"))?;
    let method = event
        .pointer("/requestContext/http/method")
        .and_then(Value::as_str)
        .ok_or(EventError::Shape("requestContext.http.method"))?;
    // Function URLs deliver header names lower-cased.
    let if_none_match = event
        .pointer("/headers/if-none-match")
        .and_then(Value::as_str)
        .map(Into::into);
    Ok(Request {
        method: match method {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            _ => Method::Other,
        },
        path: path.into(),
        if_none_match,
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
        .map(|(name, value)| (name.to_owned(), Value::String(value.into())))
        .collect();
    let reply = json!({
        "statusCode": response.status,
        "headers": headers,
        "body": body,
        "isBase64Encoded": false,
    });
    Ok(reply.to_string().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::{EventError, decode, encode};
    use cms_readmodel::{Method, Response};

    #[test]
    fn function_url_events_decode_to_requests() {
        let event = br#"{"version":"2.0","rawPath":"/blog/hello/","rawQueryString":"",
            "headers":{"if-none-match":"\"e1\"","host":"x.lambda-url.us-east-1.on.aws"},
            "requestContext":{"http":{"method":"GET","path":"/blog/hello/"}}}"#;
        let request = decode(event);
        assert!(request.is_ok());
        if let Ok(request) = request {
            assert_eq!(request.method, Method::Get);
            assert_eq!(&*request.path, "/blog/hello/");
            assert_eq!(request.if_none_match.as_deref(), Some("\"e1\""));
        }
        let head = br#"{"rawPath":"/","requestContext":{"http":{"method":"HEAD"}}}"#;
        assert!(decode(head).is_ok_and(|r| r.method == Method::Head && r.if_none_match.is_none()));
        let post = br#"{"rawPath":"/","requestContext":{"http":{"method":"POST"}}}"#;
        assert!(decode(post).is_ok_and(|r| r.method == Method::Other));
    }

    #[test]
    fn malformed_events_name_what_is_missing() {
        assert_eq!(decode(b"nope"), Err(EventError::Shape("not JSON")));
        assert_eq!(decode(b"{}"), Err(EventError::Shape("rawPath")));
        assert_eq!(
            decode(br#"{"rawPath":"/"}"#).map_err(|e| e.to_string()),
            Err("invocation is not a function URL event: requestContext.http.method".to_owned())
        );
    }

    #[test]
    fn responses_encode_status_headers_and_text_body() {
        let response = Response {
            status: 200,
            headers: vec![("etag", "\"e1\"".into())],
            body: b"<h1>Hi</h1>".to_vec(),
        };
        let encoded = encode(response).map(String::from_utf8);
        assert_eq!(
            encoded,
            Ok(Ok(
                r#"{"body":"<h1>Hi</h1>","headers":{"etag":"\"e1\""},"isBase64Encoded":false,"statusCode":200}"#
                    .to_owned()
            ))
        );
        let binary = Response {
            status: 200,
            headers: Vec::new(),
            body: vec![0xff],
        };
        assert_eq!(encode(binary), Err(EventError::NotUtf8));
        assert_eq!(
            EventError::NotUtf8.to_string(),
            "response body is not UTF-8"
        );
    }
}
