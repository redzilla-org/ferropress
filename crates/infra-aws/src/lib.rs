//! AWS adapters for the serverless profile (`docs/aws/architecture.md`, issue #4).
//!
//! Everything is blocking and SDK-free so the read Lambda stays tiny:
//!
//! - [`sigv4`]: Signature Version 4 header signing and query presigning, plus
//!   [`dsql::auth_token`] for Aurora DSQL IAM authentication.
//! - [`Environment`]: credentials and region read from an explicit variable lookup.
//! - [`AwsClient`]: one connection pool and signing context shared by the service clients
//!   [`DynamoDb`], [`S3`] and [`CloudFront`]. Every client takes an [`Endpoint`], so tests and
//!   local emulators point it at `127.0.0.1`.
//! - [`ReadTable`]: the read-model table as a [`cms_readmodel::RouteTable`] plus the projector's
//!   version-gated writers; [`S3`] is the [`cms_readmodel::Blobs`] adapter.
//!
//! # Examples
//!
//! ```
//! use infra_aws::{AmzDate, Credentials, dsql};
//!
//! let credentials = Credentials::new("AKIDEXAMPLE", "secret", None);
//! let token = dsql::auth_token(
//!     &credentials,
//!     "us-east-1",
//!     "cluster.dsql.us-east-1.on.aws",
//!     dsql::Role::Admin,
//!     AmzDate::from_unix(1_440_938_160),
//!     dsql::DEFAULT_TOKEN_TTL_SECS,
//! );
//! assert!(token.starts_with("cluster.dsql.us-east-1.on.aws/?Action=DbConnectAdmin&"));
//! ```

mod cloudfront;
mod credentials;
mod digest;
pub mod dsql;
mod dynamodb;
mod http;
mod readmodel;
mod s3;
pub mod sigv4;
mod time;

pub use cloudfront::CloudFront;
pub use credentials::{ConfigError, Credentials, Environment};
pub use digest::sha256_hex;
pub use dynamodb::{DynamoDb, Item, Page, Write};
pub use http::{AwsClient, AwsError, Endpoint};
pub use readmodel::{ReadTable, decode_route, encode_route, route_pk, term_pk};
pub use s3::{Addressing, S3};
pub use time::AmzDate;
