//! Projector Lambda (`docs/aws/architecture.md` section 5, issue #6).
//!
//! [`run`] polls the Lambda Runtime API. Every invocation, whether the empty async nudge sent
//! after a commit or the 5-minute `EventBridge` Scheduler event, runs one projection: the
//! payload only says "look at the outbox", so [`trigger`] accepts both and nothing else differs.
//! Each run writes one EMF line (`cms_projector::emf_line`) to the metrics sink.
//!
//! [`start`] composes the `infra-aws` adapters ([`aws`]) with two `cms_db::Store` connections
//! to the site database; the `bootstrap` binary passes the `infra-dsql` store.

pub mod aws;
mod config;
mod runtime;

pub use config::{Config, StartError, start};
pub use runtime::{RuntimeError, TriggerError, run, summary, trigger};
