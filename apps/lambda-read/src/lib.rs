//! Tiny read-path Lambda (`docs/aws/architecture.md` section 3, issues #3 and #4).
//!
//! [`Config::from_lookup`] reads the environment, [`start`] wires the `DynamoDB` read table and
//! the S3 blob store (behind a warm [`ShellCache`]) into [`run`], which polls the Lambda Runtime
//! API, decodes each Function URL event, serves it through [`cms_readmodel::serve`] and posts
//! the Function URL response.

mod cache;
mod config;
mod event;
mod runtime;

pub use cache::ShellCache;
pub use config::{Config, StartError, start};
pub use event::{EventError, decode, encode};
pub use runtime::{RuntimeError, run};
