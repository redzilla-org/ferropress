//! Projector Lambda (`docs/aws/architecture.md` section 5, issue #6).
//!
//! [`run`] polls the Lambda Runtime API. Every invocation, whether the empty async nudge sent
//! after a commit or the 5-minute `EventBridge` Scheduler event, runs one projection: the
//! payload only says "look at the outbox", so [`trigger`] accepts both and nothing else differs.
//! Each run writes one EMF line (`cms_projector::emf_line`) to the metrics sink.

mod runtime;

pub use runtime::{RuntimeError, TriggerError, run, summary, trigger};
