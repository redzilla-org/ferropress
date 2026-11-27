//! `media-sign`: Function URL event to presigned PUT response.

use cms_media::{Error, Presigner, SignPolicy, SignRequest, Uploaders, sign};
use serde_json::{Map, Value, json};

const SESSION_COOKIE: &str = "fp_session=";

/// Answers one Function URL sign request; every outcome is an HTTP response.
///
/// The body is JSON `{"filename", "type", "size"}`; the session comes from the `fp_session`
/// cookie. 200 carries `{"key", "url", "headers"}`; 400, 403, 405 and 500 carry `{"message"}`.
pub fn sign_invocation(
    payload: &[u8],
    policy: SignPolicy,
    random: [u8; 16],
    uploaders: &mut impl Uploaders,
    presigner: &impl Presigner,
) -> Vec<u8> {
    let Ok(event) = serde_json::from_slice::<Value>(payload) else {
        return reply(400, &message("invalid request"));
    };
    let method = event.pointer("/requestContext/http/method").and_then(Value::as_str);
    if method != Some("POST") {
        return reply(405, &message("method not allowed"));
    }
    // Function URLs move cookies out of the headers into a `cookies` array.
    let session = event
        .get("cookies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(|cookie| cookie.strip_prefix(SESSION_COOKIE))
        .unwrap_or_default();
    // The editor posts JSON text; a base64 body is a client this endpoint does not serve.
    let base64 = event.get("isBase64Encoded").and_then(Value::as_bool) == Some(true);
    let body = event.get("body").and_then(Value::as_str);
    let fields = match body {
        Some(text) if !base64 => serde_json::from_str::<Value>(text).ok(),
        _ => None,
    };
    let Some(fields) = fields else {
        return reply(400, &message("invalid request"));
    };
    let (Some(filename), Some(mime), Some(size)) = (
        fields.get("filename").and_then(Value::as_str),
        fields.get("type").and_then(Value::as_str),
        fields.get("size").and_then(Value::as_u64),
    ) else {
        return reply(400, &message("filename, type and size are required"));
    };
    let request = SignRequest {
        session: session.as_bytes(),
        filename,
        mime,
        size,
    };
    match sign(&request, policy, random, uploaders, presigner) {
        Ok(signed) => {
            let headers: Map<String, Value> = signed
                .headers
                .into_iter()
                .map(|(name, value)| (name, Value::String(value)))
                .collect();
            reply(
                200,
                &json!({ "key": signed.key, "url": signed.url, "headers": headers }),
            )
        }
        Err(Error::Forbidden) => reply(403, &message(&Error::Forbidden.to_string())),
        // Adapter failures are logged by the platform from the error type, never echoed.
        Err(Error::Port(_)) => reply(500, &message("upload signing failed")),
        Err(refused) => reply(400, &message(&refused.to_string())),
    }
}

fn message(text: &str) -> Value {
    json!({ "message": text })
}

fn reply(status: u16, body: &Value) -> Vec<u8> {
    json!({
        "statusCode": status,
        "headers": { "content-type": "application/json", "cache-control": "no-store" },
        "body": body.to_string(),
        "isBase64Encoded": false,
    })
    .to_string()
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::sign_invocation;
    use cms_media::{PortError, Presigner, SignPolicy, Uploaders};
    use serde_json::{Value, json};

    struct Sessions;

    impl Uploaders for Sessions {
        fn uploader(&mut self, session: &[u8]) -> Result<Option<u64>, PortError> {
            match session {
                b"good" => Ok(Some(5)),
                b"broken" => Err(PortError::new("table missing")),
                _ => Ok(None),
            }
        }
    }

    struct Signer;

    impl Presigner for Signer {
        fn presign_put(
            &self,
            key: &str,
            _headers: &[(&str, &str)],
            _expires_secs: u32,
        ) -> Result<String, PortError> {
            Ok(format!("https://bucket.example.com/{key}?sig"))
        }
    }

    const POLICY: SignPolicy = SignPolicy {
        max_bytes: 100,
        expires_secs: 60,
    };

    fn call(event: &Value) -> (u64, Value) {
        let out = sign_invocation(
            event.to_string().as_bytes(),
            POLICY,
            [0; 16],
            &mut Sessions,
            &Signer,
        );
        let response: Value = serde_json::from_slice(&out).unwrap_or_default();
        let status = response["statusCode"].as_u64().unwrap_or_default();
        let body = response["body"]
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default();
        (status, body)
    }

    fn event(cookie: &str, body: &Value) -> Value {
        json!({
            "requestContext": { "http": { "method": "POST" } },
            "cookies": ["other=1", format!("fp_session={cookie}")],
            "body": body.to_string(),
            "isBase64Encoded": false,
        })
    }

    #[test]
    fn signs_for_a_logged_in_uploader() {
        let body = json!({ "filename": "a b.png", "type": "image/png", "size": 10 });
        let (status, reply) = call(&event("good", &body));
        assert_eq!(status, 200);
        let key = "site/incoming/00000000-0000-4000-8000-000000000000/a-b.png";
        assert_eq!(reply["key"], key);
        assert_eq!(reply["url"], format!("https://bucket.example.com/{key}?sig"));
        assert_eq!(
            reply["headers"],
            json!({ "content-length": "10", "content-type": "image/png", "x-amz-meta-fp-author": "5" })
        );
    }

    #[test]
    fn maps_refusals_to_statuses() {
        let png = json!({ "filename": "a.png", "type": "image/png", "size": 10 });
        assert_eq!(call(&event("nobody", &png)).0, 403);
        assert_eq!(call(&event("broken", &png)), (500, json!({ "message": "upload signing failed" })));
        let pdf = json!({ "filename": "a.pdf", "type": "application/pdf", "size": 10 });
        assert_eq!(
            call(&event("good", &pdf)),
            (400, json!({ "message": "Sorry, you are not allowed to upload this file type." }))
        );
        assert_eq!(call(&event("good", &json!({ "filename": "a.png" }))).0, 400);
        let mut get = event("good", &png);
        get["requestContext"]["http"]["method"] = json!("GET");
        assert_eq!(call(&get).0, 405);
        let mut base64 = event("good", &png);
        base64["isBase64Encoded"] = json!(true);
        assert_eq!(call(&base64).0, 400);
        let mut no_cookie = event("good", &png);
        no_cookie["cookies"] = json!(null);
        assert_eq!(call(&no_cookie).0, 403);
        let mut not_json = event("good", &png);
        not_json["body"] = json!("{");
        assert_eq!(call(&not_json).0, 400);
        let raw = sign_invocation(b"nope", POLICY, [0; 16], &mut Sessions, &Signer);
        assert!(String::from_utf8_lossy(&raw).contains(r#""statusCode":400"#));
    }
}
