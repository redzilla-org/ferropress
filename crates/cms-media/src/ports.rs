//! Ports the Lambda binaries fill with AWS adapters (`infra-aws`) and sessions (`cms-auth`).

use crate::PortError;

/// Issues presigned S3 PUT URLs (`SigV4` query signing, `infra-aws`).
pub trait Presigner {
    /// A URL that lets its holder PUT `key` for `expires_secs`, with `headers` (lower-case
    /// names) signed so the upload must send exactly those values.
    ///
    /// # Errors
    ///
    /// [`PortError`] when credentials are unavailable.
    fn presign_put(
        &self,
        key: &str,
        headers: &[(&str, &str)],
        expires_secs: u32,
    ) -> Result<String, PortError>;
}

/// Resolves a session token to a user who may upload.
pub trait Uploaders {
    /// The user id when `session` is a live session whose user has `upload_files`, else `None`.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the session or user store cannot be read.
    fn uploader(&mut self, session: &[u8]) -> Result<Option<u64>, PortError>;
}

/// One stored object with its user metadata (`x-amz-meta-*` names without the prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    /// The object's bytes.
    pub bytes: Vec<u8>,
    /// User metadata pairs, names lower-case.
    pub metadata: Vec<(String, String)>,
}

/// The site bucket (`S3` in production).
pub trait Objects {
    /// The object at `key`, or `None` when it does not exist (already processed).
    ///
    /// # Errors
    ///
    /// [`PortError`] on any other failure.
    fn get(&mut self, key: &str) -> Result<Option<Object>, PortError>;

    /// Stores `bytes` at `key` with this content type and the uploads cache policy.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the write fails.
    fn put(&mut self, key: &str, content_type: &str, bytes: &[u8]) -> Result<(), PortError>;

    /// Removes `key`; removing a missing key succeeds.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the delete fails.
    fn delete(&mut self, key: &str) -> Result<(), PortError>;

    /// Every key that starts with `prefix`.
    ///
    /// # Errors
    ///
    /// [`PortError`] when listing fails.
    fn list(&mut self, prefix: &str) -> Result<Vec<String>, PortError>;
}
