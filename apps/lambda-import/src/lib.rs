//! Import Lambda (`docs/aws/architecture.md` section 9, issue #13).
//!
//! An S3 `ObjectCreated` event on `site/imports/` invokes it. [`run`] polls the Lambda Runtime
//! API, [`uploads`] takes the export keys out of the event, and the job imports each one with
//! `cms_import::import`, then writes its `rebuild-all` outbox event. A run cut off by the
//! 15-minute limit fails its invocation; S3's async retry starts it again, and the checkpoint
//! item lets it resume after the last committed batch.
//!
//! [`start`] composes the `infra-aws` adapters ([`aws`]) with one `cms_db::Store` connection to
//! the site database; the `bootstrap` binary passes the `infra-dsql` store.

pub mod aws;
mod config;
mod runtime;

pub use config::{Config, StartError, import_one, start};
pub use runtime::{RuntimeError, TriggerError, Upload, run, uploads};
