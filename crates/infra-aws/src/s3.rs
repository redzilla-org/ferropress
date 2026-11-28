//! S3 objects: get, put and delete.

use cms_readmodel::{BlobKey, Blobs, PortError};

use crate::http::Call;
use crate::sigv4::{PathStyle, uri_encode};
use crate::{AwsClient, AwsError, Endpoint};

/// How the bucket is addressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Addressing {
    /// `https://<bucket>.s3.<region>.amazonaws.com/<key>` (AWS).
    VirtualHosted,
    /// `<endpoint>/<bucket>/<key>` (local emulators and fakes).
    Path,
}

/// An S3 client for one bucket; also the read path's [`Blobs`] port.
#[derive(Clone, Debug)]
pub struct S3 {
    aws: AwsClient,
    endpoint: Endpoint,
    bucket: Box<str>,
    addressing: Addressing,
}

impl S3 {
    /// A client for `bucket` reached at `endpoint`. For [`Addressing::VirtualHosted`] the
    /// endpoint already names the bucket host.
    #[must_use]
    pub fn new(aws: AwsClient, endpoint: Endpoint, bucket: &str, addressing: Addressing) -> Self {
        Self {
            aws,
            endpoint,
            bucket: bucket.into(),
            addressing,
        }
    }

    /// A virtual-hosted client for `bucket` in the client's region.
    #[must_use]
    pub fn regional(aws: AwsClient, bucket: &str) -> Self {
        let endpoint = Endpoint::https(&format!("{bucket}.s3.{}.amazonaws.com", aws.region()));
        Self::new(aws, endpoint, bucket, Addressing::VirtualHosted)
    }

    /// `GetObject`. A missing key is `None` (S3 answers 404 only when the caller may list the
    /// bucket; without that permission it is a 403 error).
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>, AwsError> {
        const OP: &str = "S3.GetObject";
        let reply = self.send(OP, "GET", key, &[], &[])?;
        if reply.status == 404 {
            return Ok(None);
        }
        Ok(Some(reply.success(OP)?.body))
    }

    /// `PutObject` with the given `Content-Type` and `Cache-Control`.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn put_object(
        &self,
        key: &str,
        body: &[u8],
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), AwsError> {
        const OP: &str = "S3.PutObject";
        let headers = [
            ("content-type", content_type),
            ("cache-control", cache_control),
        ];
        self.send(OP, "PUT", key, &headers, body)?
            .success(OP)
            .map(drop)
    }

    /// `DeleteObject`; deleting a missing key succeeds.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn delete_object(&self, key: &str) -> Result<(), AwsError> {
        const OP: &str = "S3.DeleteObject";
        self.send(OP, "DELETE", key, &[], &[])?
            .success(OP)
            .map(drop)
    }

    fn send(
        &self,
        operation: &'static str,
        method: &'static str,
        key: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<crate::http::Reply, AwsError> {
        if key.is_empty() {
            return Err(AwsError::Input {
                operation,
                what: "empty object key",
            });
        }
        // S3 signs the once-encoded path as sent.
        let path = match self.addressing {
            Addressing::VirtualHosted => format!("/{}", uri_encode(key, true)),
            Addressing::Path => format!(
                "/{}/{}",
                uri_encode(&self.bucket, false),
                uri_encode(key, true)
            ),
        };
        self.aws.send(&Call {
            operation,
            service: "s3",
            region: None,
            path_style: PathStyle::S3,
            endpoint: &self.endpoint,
            method,
            path: &path,
            query: &[],
            headers,
            body,
        })
    }
}

impl Blobs for S3 {
    fn get(&mut self, key: &BlobKey) -> Result<Option<Vec<u8>>, PortError> {
        self.get_object(key.as_str())
            .map_err(|error| PortError::Store(error.to_string().into()))
    }
}
