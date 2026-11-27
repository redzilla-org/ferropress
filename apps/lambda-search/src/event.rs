//! Function URL (payload format 2.0) event in, Function URL response out.

use cms_readmodel::{Method, Response};
use serde_json::{Map, Value, json};

/// The event or the response could not be translated.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventError {
    /// The invocation payload is not a Function URL event.
    #[error("invocation is not a function URL event: {0}")]
    Shape(&'static str),
    /// Search output is text built from UTF-8 pieces, so other bytes are a defect.
    #[error("response body is not UTF-8")]
    NotUtf8,
}

/// One search request, decoded from the platform event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRequest {
    /// The method.
    pub method: Method,
    /// The path without the query string.
    pub path: Box<str>,
    /// The raw query string without `?` (still percent-encoded).
    pub query: Box<str>,
}

/// Decodes a Function URL event.
///
/// # Errors
///
/// [`EventError::Shape`] when `rawPath` or the HTTP method is missing.
pub fn decode(payload: &[u8]) -> Result<SearchRequest, EventError> {
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
    // Function URLs send an empty string, but tolerate the key missing for a query-less request.
    let query = event
        .get("rawQueryString")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(SearchRequest {
        method: match method {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            _ => Method::Other,
        },
        path: path.into(),
        query: query.into(),
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
    fn events_decode_with_their_query() {
        let event = br#"{"rawPath":"/blog/","rawQueryString":"s=red",
            "requestContext":{"http":{"method":"GET"}}}"#;
        let request = decode(event);
        assert!(request.as_ref().is_ok_and(|r| r.method == Method::Get
            && &*r.path == "/blog/"
            && &*r.query == "s=red"));
        let head = br#"{"rawPath":"/","requestContext":{"http":{"method":"HEAD"}}}"#;
        assert!(decode(head).is_ok_and(|r| r.method == Method::Head && r.query.is_empty()));
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
    fn responses_encode_and_reject_binary() {
        let response = Response {
            status: 200,
            headers: vec![("x-wp-total", "3".into())],
            body: b"[]".to_vec(),
        };
        assert_eq!(
            encode(response).map(String::from_utf8),
            Ok(Ok(
                r#"{"body":"[]","headers":{"x-wp-total":"3"},"isBase64Encoded":false,"statusCode":200}"#
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
