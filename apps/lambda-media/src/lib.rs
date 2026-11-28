//! Media Lambdas (`docs/aws/architecture.md` section 3, issue #10).
//!
//! - [`sign_invocation`]: a Function URL `POST /blog/api/media/sign` event in, a Function URL
//!   response with the presigned PUT out (`media-sign`).
//! - [`process_invocation`]: an S3 `ObjectCreated` event for `site/incoming/` in, one
//!   [`cms_media::process`] run per record (`media-process`).
//!
//! The ports are passed in. The Runtime API loop, the `S3`/presign adapters, the `DSQL` store
//! and the session reader that make these binaries arrive with `infra-aws` (#4), `infra-dsql`
//! (#5) and the auth phase (#8), as for `lambda-read`.

mod process;
mod sign;

pub use process::{EventError, incoming_keys, process_invocation};
pub use sign::sign_invocation;
