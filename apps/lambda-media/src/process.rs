//! `media-process`: S3 `ObjectCreated` event to attachment rows.

use cms_db::Store;
use cms_media::{Clock, Objects, Site, process};
use serde_json::{Value, json};

/// The event is not an S3 notification this function can read.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventError {
    /// A required field is missing.
    #[error("invocation is not an S3 event: {0}")]
    Shape(&'static str),
    /// An object key is not valid URL-encoded UTF-8.
    #[error("object key is not URL-encoded UTF-8: {0}")]
    Key(Box<str>),
}

/// The decoded object keys of every record, in event order.
///
/// # Errors
///
/// [`EventError`] when the payload is not an S3 event or a key does not decode.
pub fn incoming_keys(payload: &[u8]) -> Result<Vec<String>, EventError> {
    let event: Value =
        serde_json::from_slice(payload).map_err(|_not_json| EventError::Shape("not JSON"))?;
    let records = event
        .get("Records")
        .and_then(Value::as_array)
        .ok_or(EventError::Shape("Records"))?;
    records
        .iter()
        .map(|record| {
            let raw = record
                .pointer("/s3/object/key")
                .and_then(Value::as_str)
                .ok_or(EventError::Shape("s3.object.key"))?;
            url_decode(raw).ok_or_else(|| EventError::Key(raw.into()))
        })
        .collect()
}

/// Processes every record. The reply lists `{"key", "id"}` per record, `id` null when the
/// object was already processed.
///
/// # Errors
///
/// A message for the Runtime API error endpoint when the event is malformed or any record
/// fails; Lambda's async retry then redelivers the event, and records already processed are
/// skipped because their incoming object is gone.
pub fn process_invocation(
    payload: &[u8],
    site: &Site,
    clock: &Clock,
    objects: &mut impl Objects,
    store: &mut impl Store,
) -> Result<Vec<u8>, String> {
    let keys = incoming_keys(payload).map_err(|error| error.to_string())?;
    let mut done = Vec::with_capacity(keys.len());
    for key in keys {
        let processed = process(&key, site, clock, objects, store)
            .map_err(|error| format!("{key}: {error}"))?;
        done.push(json!({ "key": key, "id": processed.map(|p| p.id) }));
    }
    Ok(json!({ "processed": done }).to_string().into_bytes())
}

// S3 notifications form-encode keys: `+` is a space, `%XX` a byte.
fn url_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 2;
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::{EventError, incoming_keys, url_decode};

    #[test]
    fn keys_are_form_decoded() {
        assert_eq!(url_decode("a+b%2Fc%C3%A9"), Some("a b/c\u{e9}".to_owned()));
        assert_eq!(url_decode("%"), None);
        assert_eq!(url_decode("%zz"), None);
        assert_eq!(url_decode("%ff"), None);
        let event = br#"{"Records":[{"s3":{"object":{"key":"site/incoming/u/a+b.png"}}}]}"#;
        assert_eq!(
            incoming_keys(event),
            Ok(vec!["site/incoming/u/a b.png".to_owned()])
        );
    }

    #[test]
    fn malformed_events_name_what_is_missing() {
        assert_eq!(incoming_keys(b"x"), Err(EventError::Shape("not JSON")));
        assert_eq!(incoming_keys(b"{}"), Err(EventError::Shape("Records")));
        assert_eq!(
            incoming_keys(br#"{"Records":[{}]}"#),
            Err(EventError::Shape("s3.object.key"))
        );
        assert_eq!(
            incoming_keys(br#"{"Records":[{"s3":{"object":{"key":"%"}}}]}"#)
                .map_err(|error| error.to_string()),
            Err("object key is not URL-encoded UTF-8: %".to_owned())
        );
    }
}
