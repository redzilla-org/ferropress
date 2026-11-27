//! Errors of the upload and processing paths.

/// An adapter behind a port failed (network, permissions, missing bucket).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct PortError(pub Box<str>);

impl PortError {
    /// A port error with this message.
    #[must_use]
    pub fn new(message: impl Into<Box<str>>) -> Self {
        Self(message.into())
    }
}

/// Why an upload was refused or could not be processed.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// No valid session, or the user lacks `upload_files`.
    #[error("Sorry, you are not allowed to upload files.")]
    Forbidden,
    /// The file extension or declared type is not an accepted image type.
    #[error("Sorry, you are not allowed to upload this file type.")]
    FileType,
    /// The declared size is zero or above the configured maximum.
    #[error("the file size {size} is outside 1..={max} bytes")]
    Size {
        /// Declared size.
        size: u64,
        /// Configured maximum.
        max: u64,
    },
    /// The file name sanitizes to nothing usable.
    #[error("the file name is empty after sanitizing")]
    Name,
    /// The key is not `site/incoming/<uuid>/<name>`.
    #[error("not an incoming upload key: {0}")]
    Key(Box<str>),
    /// The bytes do not decode as the image type the extension names.
    #[error("the file is not a valid {0} image")]
    Decode(&'static str),
    /// Encoding a sub-size failed.
    #[error("encoding a sub-size failed: {0}")]
    Encode(Box<str>),
    /// The clock string is not `YYYY-MM-DD hh:mm:ss`.
    #[error("not a date-time: {0}")]
    Clock(Box<str>),
    /// A value cannot be written as a literal in the store's dialect.
    #[error("a value cannot be written as a SQL literal")]
    Literal,
    /// The database rejected a statement; the transaction was rolled back.
    #[error("the database rejected a statement: {0}")]
    Rejected(Box<str>),
    /// The store could not reach the database.
    #[error("the database is unavailable")]
    Unavailable,
    /// A port adapter failed.
    #[error(transparent)]
    Port(#[from] PortError),
}

#[cfg(test)]
mod tests {
    use super::{Error, PortError};

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(
            Error::Forbidden.to_string(),
            "Sorry, you are not allowed to upload files."
        );
        assert_eq!(
            Error::FileType.to_string(),
            "Sorry, you are not allowed to upload this file type."
        );
        assert_eq!(
            Error::Size { size: 0, max: 5 }.to_string(),
            "the file size 0 is outside 1..=5 bytes"
        );
        assert_eq!(
            Error::Name.to_string(),
            "the file name is empty after sanitizing"
        );
        assert_eq!(
            Error::Key("x".into()).to_string(),
            "not an incoming upload key: x"
        );
        assert_eq!(
            Error::Decode("png").to_string(),
            "the file is not a valid png image"
        );
        assert_eq!(
            Error::Encode("e".into()).to_string(),
            "encoding a sub-size failed: e"
        );
        assert_eq!(Error::Clock("c".into()).to_string(), "not a date-time: c");
        assert_eq!(
            Error::Literal.to_string(),
            "a value cannot be written as a SQL literal"
        );
        assert_eq!(
            Error::Rejected("r".into()).to_string(),
            "the database rejected a statement: r"
        );
        assert_eq!(
            Error::Unavailable.to_string(),
            "the database is unavailable"
        );
        assert_eq!(Error::from(PortError::new("down")).to_string(), "down");
    }
}
