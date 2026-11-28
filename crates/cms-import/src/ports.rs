//! Ports the import job needs from its host: resume state, attachment source and upload target.
//!
//! The Lambda wires these to `DynamoDB` (`IMPORT#<upload key>` / `C`), HTTP and S3
//! (`site/uploads/`); tests use in-memory fakes.

use crate::error::PortError;

/// Remembers how many records of one export file are committed, so a timed-out run resumes.
pub trait Checkpoint {
    /// Records committed by earlier runs, or `None` when this file was never started.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the checkpoint store cannot be read.
    fn load(&mut self) -> Result<Option<u64>, PortError>;

    /// Records the first `records` records as committed.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the checkpoint store cannot be written.
    fn save(&mut self, records: u64) -> Result<(), PortError>;
}

/// Downloads an attachment from its source URL.
pub trait Fetcher {
    /// The bytes at `url`.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the URL cannot be fetched.
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, PortError>;
}

/// Stores an attachment at an uploads-relative path such as `2026/01/photo.jpg`.
pub trait UploadSink {
    /// Writes `bytes` at `path`, replacing any earlier object there.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the object cannot be written.
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), PortError>;
}
