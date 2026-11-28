//! Lambda Runtime API loop and the S3 event shape it accepts.

use serde_json::{Value, json};

/// Only exports under this prefix are imported; the bucket notification filters on it too.
const IMPORTS: &str = "site/imports/";

/// The Runtime API itself failed; the process exits and Lambda restarts the environment.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeError {
    /// HTTP to the Runtime API failed.
    #[error("runtime API request failed: {0}")]
    Http(Box<str>),
    /// `next` returned no request id header.
    #[error("runtime API sent no request id")]
    NoRequestId,
}

/// An invocation payload that is not an S3 event naming exports under `site/imports/`.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("import payload is not an S3 event for site/imports/: {0}")]
pub struct TriggerError(pub Box<str>);

/// One uploaded export: its object key and the upload's `eTag`, which tells a re-upload under
/// the same key apart from a retry of the same upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    /// The decoded object key, such as `site/imports/export.xml`.
    pub key: String,
    /// The object's `eTag` from the event (empty when the event has none).
    pub etag: String,
}

/// The exports an S3 `ObjectCreated` event names, in event order.
///
/// # Errors
///
/// [`TriggerError`] when the payload is not such an event or names a key outside
/// `site/imports/`, so a misrouted event fails loudly.
pub fn uploads(payload: &[u8]) -> Result<Vec<Upload>, TriggerError> {
    let bad = |what: &str| TriggerError(what.into());
    let event: Value = serde_json::from_slice(payload).map_err(|error| bad(&error.to_string()))?;
    let records = event
        .get("Records")
        .and_then(Value::as_array)
        .filter(|records| !records.is_empty())
        .ok_or_else(|| bad("no Records"))?;
    records
        .iter()
        .map(|record| {
            let object = record.pointer("/s3/object").ok_or_else(|| bad("no s3.object"))?;
            let raw = object
                .get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("no s3.object.key"))?;
            let key = decode_key(raw).ok_or_else(|| bad(raw))?;
            if !key.starts_with(IMPORTS) {
                return Err(bad(&key));
            }
            let etag = object.get("eTag").and_then(Value::as_str).unwrap_or_default();
            Ok(Upload {
                key,
                etag: etag.to_owned(),
            })
        })
        .collect()
}

/// S3 event keys are form-encoded: `+` is a space and `%XX` a byte.
fn decode_key(raw: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(raw.len());
    let mut input = raw.bytes();
    while let Some(byte) = input.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = char::from(input.next()?).to_digit(16)?;
                let low = char::from(input.next()?).to_digit(16)?;
                bytes.push(u8::try_from(high * 16 + low).ok()?);
            }
            other => bytes.push(other),
        }
    }
    String::from_utf8(bytes).ok()
}

/// Serves invocations from the Runtime API at `api` forever, running `job` once per upload the
/// event names and answering with the summaries `job` returns.
///
/// A failed upload fails the invocation, so S3's async retry runs it again from its checkpoint.
///
/// # Errors
///
/// [`RuntimeError`] only when the Runtime API fails.
pub fn run<J>(api: &str, mut job: J) -> Result<(), RuntimeError>
where
    J: FnMut(&Upload) -> Result<Value, String>,
{
    // One agent reuses the localhost connection; `next` long-polls, so no timeout is set.
    let agent = ureq::Agent::new_with_defaults();
    let root = format!("http://{api}/2018-06-01/runtime/invocation");
    loop {
        let mut next = agent
            .get(format!("{root}/next"))
            .call()
            .map_err(|error| http(&error))?;
        let id = next
            .headers()
            .get("lambda-runtime-aws-request-id")
            .and_then(|value| value.to_str().ok())
            .ok_or(RuntimeError::NoRequestId)?
            .to_owned();
        let payload = next
            .body_mut()
            .read_to_vec()
            .map_err(|error| http(&error))?;
        let outcome = uploads(&payload)
            .map_err(|error| error.to_string())
            .and_then(|uploads| uploads.iter().map(&mut job).collect::<Result<Vec<_>, _>>());
        let (path, body) = match outcome {
            Ok(summaries) => ("response", json!({ "imports": summaries })),
            Err(message) => (
                "error",
                json!({ "errorMessage": message, "errorType": "ImportError" }),
            ),
        };
        agent
            .post(format!("{root}/{id}/{path}"))
            .header("content-type", "application/json")
            .send(body.to_string().as_bytes())
            .map_err(|error| http(&error))?;
    }
}

fn http(error: &ureq::Error) -> RuntimeError {
    RuntimeError::Http(error.to_string().into())
}

#[cfg(test)]
mod tests {
    use super::{TriggerError, Upload, decode_key, uploads};

    #[test]
    fn s3_events_name_decoded_import_keys() {
        let event = br#"{"Records":[{"eventName":"ObjectCreated:Put","s3":{"object":
            {"key":"site/imports/my+export%C3%A9.xml","eTag":"abc"}}}]}"#;
        assert_eq!(
            uploads(event),
            Ok(vec![Upload {
                key: "site/imports/my export\u{e9}.xml".to_owned(),
                etag: "abc".to_owned(),
            }])
        );
        assert_eq!(decode_key("a%2"), None);
        assert_eq!(decode_key("%ff"), None);
    }

    #[test]
    fn anything_else_is_rejected() {
        let outside = br#"{"Records":[{"s3":{"object":{"key":"site/uploads/x.xml"}}}]}"#;
        assert_eq!(
            uploads(outside),
            Err(TriggerError("site/uploads/x.xml".into()))
        );
        for payload in [&b""[..], b"{}", br#"{"Records":[]}"#, br#"{"Records":[{}]}"#] {
            assert!(uploads(payload).is_err());
        }
        assert!(uploads(br#"{"Records":[{"s3":{"object":{}}}]}"#).is_err());
    }
}
