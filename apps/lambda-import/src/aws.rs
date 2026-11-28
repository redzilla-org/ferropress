//! The import job's ports over `infra-aws` and ureq: the checkpoint item in the read-model
//! table, the uploads prefix in the site bucket, and attachment downloads over HTTP(S).

use std::time::Duration;

use cms_import::{Checkpoint, Fetcher, PortError, UploadSink};
use infra_aws::{AwsError, DynamoDb, Item, S3};
use serde_json::{Value, json};

/// Media keeps the reference's upload URL layout under this bucket prefix.
const UPLOADS: &str = "site/uploads/";
/// Uploads are served for a year at the edge (`blog/wp-content/uploads/*`).
const UPLOAD_CACHE: &str = "public, max-age=31536000";
/// The checkpoint item's sort key.
const CHECKPOINT_SK: &str = "C";
/// Largest attachment downloaded; Lambda memory is 2 GiB and one file is held at a time.
const MAX_ATTACHMENT: u64 = 512 * 1024 * 1024;

fn port(error: &AwsError) -> PortError {
    PortError::new(error.to_string())
}

/// [`Checkpoint`] in the read-model table at `IMPORT#<key>#<eTag>` / `C`.
#[derive(Debug)]
pub struct ReadCheckpoint {
    db: DynamoDb,
    table: Box<str>,
    pk: String,
}

impl ReadCheckpoint {
    /// The checkpoint of upload `key` with `etag` in table `table`.
    #[must_use]
    pub fn new(db: DynamoDb, table: &str, key: &str, etag: &str) -> Self {
        Self {
            db,
            table: table.into(),
            pk: format!("IMPORT#{key}#{etag}"),
        }
    }

    fn key(&self) -> Item {
        let mut key = Item::new();
        key.insert("pk".to_owned(), json!({ "S": self.pk }));
        key.insert("sk".to_owned(), json!({ "S": CHECKPOINT_SK }));
        key
    }
}

impl Checkpoint for ReadCheckpoint {
    fn load(&mut self) -> Result<Option<u64>, PortError> {
        let item = self
            .db
            .get_item(&self.table, &self.key())
            .map_err(|error| port(&error))?;
        item.map(|item| {
            item.get("records")
                .and_then(|value| value.get("N"))
                .and_then(Value::as_str)
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| PortError::new("checkpoint item has no valid records"))
        })
        .transpose()
    }

    fn save(&mut self, records: u64) -> Result<(), PortError> {
        let mut item = self.key();
        item.insert("records".to_owned(), json!({ "N": records.to_string() }));
        // Zero-padded so versions sort as numbers: a stale write (a slower duplicate run) is
        // refused instead of moving the checkpoint back.
        self.db
            .put_item_versioned(&self.table, item, &format!("{records:020}"))
            .map(drop)
            .map_err(|error| port(&error))
    }
}

/// [`UploadSink`] over S3 `PutObject` under `site/uploads/`.
#[derive(Debug)]
pub struct Uploads(pub S3);

impl UploadSink for Uploads {
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), PortError> {
        self.0
            .put_object(
                &format!("{UPLOADS}{path}"),
                bytes,
                content_type(path),
                UPLOAD_CACHE,
            )
            .map_err(|error| port(&error))
    }
}

/// The `Content-Type` the edge serves an upload with, by extension.
#[must_use]
pub fn content_type(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

/// [`Fetcher`] over plain HTTP(S) GET; a non-2xx status is an error.
#[derive(Debug)]
pub struct Http(ureq::Agent);

impl Http {
    /// A fetcher whose every download is bounded by `timeout`.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self(
            ureq::Agent::config_builder()
                .timeout_global(Some(timeout))
                .build()
                .new_agent(),
        )
    }
}

impl Fetcher for Http {
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, PortError> {
        let failed = |error: ureq::Error| PortError::new(error.to_string());
        self.0
            .get(url)
            .call()
            .map_err(failed)?
            .into_body()
            .with_config()
            .limit(MAX_ATTACHMENT)
            .read_to_vec()
            .map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::content_type;

    #[test]
    fn uploads_get_their_media_type() {
        assert_eq!(content_type("2026/01/a.JPG"), "image/jpeg");
        assert_eq!(content_type("2026/01/a.pdf"), "application/pdf");
        assert_eq!(content_type("2026/01/noext"), "application/octet-stream");
    }
}
