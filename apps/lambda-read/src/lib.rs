//! Tiny read-path Lambda (`docs/aws/architecture.md` section 3, issue #3).
//!
//! [`run`] polls the Lambda Runtime API, decodes each Function URL event, serves it through
//! [`cms_readmodel::serve`] and posts the Function URL response. The storage adapters are
//! passed in; the `DynamoDB` and `S3` adapters and the binary entry point land with `infra-aws`
//! (issue #4).

mod event;
mod runtime;

pub use event::{EventError, decode, encode};
pub use runtime::{RuntimeError, run};
