//! Errors from reading an export file, from a port, and from the import run.

/// The export file is not a readable WXR document.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// The XML layer rejected the input.
    #[error("malformed xml: {0}")]
    Xml(#[from] quick_xml::Error),
    /// An attribute could not be read.
    #[error("malformed attribute: {0}")]
    Attribute(#[from] quick_xml::events::attributes::AttrError),
    /// The input ended inside an element.
    #[error("the export ends inside <{0}>")]
    Truncated(Box<str>),
    /// Elements nest deeper than any WXR record does.
    #[error("elements nest deeper than {0} levels")]
    TooDeep(usize),
    /// A numeric field holds something other than a non-negative integer.
    #[error("<{field}> is not a number: {value:?}")]
    Number {
        /// The element name.
        field: &'static str,
        /// The text found.
        value: Box<str>,
    },
}

/// A port (checkpoint, fetcher or upload sink) failed. The adapter's message is kept as text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PortError {
    message: Box<str>,
}

impl PortError {
    /// An error with this message.
    #[must_use]
    pub fn new(message: impl Into<Box<str>>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The adapter's message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Why an import run stopped. Committed batches stay committed and the checkpoint names them.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ImportError {
    /// The export file could not be read.
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// The store could not talk to a server.
    #[error("store unavailable: {0}")]
    Store(#[from] cms_db::StoreError),
    /// The server rejected a statement.
    #[error("server error {code}: {message}")]
    Server {
        /// The server's error code.
        code: u16,
        /// The server's message.
        message: Box<str>,
    },
    /// A value cannot be written as a literal in the chosen dialect.
    #[error(transparent)]
    Literal(#[from] cms_db::LiteralError),
    /// The table prefix is unusable.
    #[error(transparent)]
    Table(#[from] cms_db::NameError),
    /// The checkpoint store failed.
    #[error("checkpoint: {0}")]
    Checkpoint(PortError),
    /// An attachment could not be fetched.
    #[error("fetch {url}: {source}")]
    Fetch {
        /// The attachment's source URL.
        url: Box<str>,
        /// The fetcher's error.
        source: PortError,
    },
    /// An attachment could not be stored.
    #[error("upload {path}: {source}")]
    Upload {
        /// The uploads-relative path.
        path: Box<str>,
        /// The sink's error.
        source: PortError,
    },
    /// An item has no guid, so re-runs could not recognize it.
    #[error("item {0} has no guid")]
    MissingGuid(u64),
    /// An attachment's URL is outside the source uploads directory and it has no stored path.
    #[error("attachment {0} has no uploads-relative path")]
    AttachmentPath(u64),
    /// One record alone exceeds the batch limits, so no transaction can hold it.
    #[error("record {index} needs {rows} rows / {bytes} bytes, over one batch")]
    RecordTooLarge {
        /// Zero-based record index in the file.
        index: u64,
        /// Estimated rows.
        rows: u64,
        /// Estimated bytes.
        bytes: u64,
    },
    /// The checkpoint says a record was committed but its row is missing.
    #[error("record {0} is before the checkpoint but its row is missing")]
    CheckpointMismatch(u64),
    /// A lookup returned something other than a row of ids.
    #[error("unexpected reply to: {0}")]
    Reply(Box<str>),
}

#[cfg(test)]
mod tests {
    use super::{ImportError, ParseError, PortError};

    #[test]
    fn messages_name_the_cause() {
        let port = PortError::new("timeout");
        assert_eq!(port.message(), "timeout");
        assert_eq!(port.to_string(), "timeout");
        let fetch = ImportError::Fetch {
            url: "https://example.com/a.jpg".into(),
            source: port.clone(),
        };
        assert_eq!(fetch.to_string(), "fetch https://example.com/a.jpg: timeout");
        let upload = ImportError::Upload {
            path: "2026/01/a.jpg".into(),
            source: port.clone(),
        };
        assert_eq!(upload.to_string(), "upload 2026/01/a.jpg: timeout");
        assert_eq!(
            ImportError::Checkpoint(port).to_string(),
            "checkpoint: timeout"
        );
        assert_eq!(ImportError::MissingGuid(3).to_string(), "item 3 has no guid");
        assert!(ImportError::AttachmentPath(4).to_string().contains('4'));
        assert!(
            ImportError::RecordTooLarge {
                index: 1,
                rows: 2,
                bytes: 3
            }
            .to_string()
            .contains("2 rows")
        );
        assert!(ImportError::CheckpointMismatch(5).to_string().contains('5'));
        assert!(ImportError::Reply("SELECT 1".into()).to_string().contains("SELECT 1"));
        assert!(
            ImportError::Server {
                code: 1,
                message: "no".into()
            }
            .to_string()
            .contains("server error 1")
        );
        let number = ParseError::Number {
            field: "wp:post_id",
            value: "x".into(),
        };
        assert!(ImportError::from(number).to_string().contains("wp:post_id"));
        assert!(ParseError::Truncated("item".into()).to_string().contains("item"));
        assert!(ParseError::TooDeep(8).to_string().contains('8'));
        assert!(
            ImportError::from(cms_db::StoreError::Unavailable)
                .to_string()
                .contains("store")
        );
    }
}
