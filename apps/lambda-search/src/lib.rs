//! Search Lambda (`docs/aws/architecture.md` sections 3 and 6, issue #12).
//!
//! `CloudFront` sends requests carrying the `s` query var, and REST search requests, to this
//! function's URL. [`handle`] loads the prebuilt index named by the `SEARCH` route item (once per
//! warm container, again only when the item names a new object) and answers:
//!
//! - `<base>?s=…` and `<base>page/N/?s=…`: an HTML results page placed into the home page's theme
//!   shell;
//! - `<base>wp-json/wp/v2/search?search=…`: the REST search JSON.
//!
//! [`run`] drives [`invoke`] from the Lambda Runtime API. The `DynamoDB` and `S3` adapters and the
//! binary entry point land with `infra-aws` (issue #4), as for the read Lambda.

mod cache;
mod event;
mod handle;
mod html;
mod query;
mod rest;
mod runtime;

pub use cache::Cache;
pub use event::{EventError, SearchRequest, decode, encode};
pub use handle::{SearchError, handle, invoke};
pub use runtime::{RuntimeError, run};
