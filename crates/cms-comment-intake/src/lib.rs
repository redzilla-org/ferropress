//! Accept, hold or refuse one public comment submission, then store it with an outbox row.
//!
//! Serverless profile, `docs/aws/architecture.md` sections 3 and 5 (issue #11). The rules follow
//! the reference's documented comment-submission behavior; they are reimplemented here, not
//! translated. Hooks and plugin filters are not run.
//!
//! - [`decide`] is the pure decision over a [`History`] port.
//! - [`SqlIntake`] implements [`History`] and the write transaction over [`cms_db::Store`].
//! - [`accept`] adds the per-IP [`RateLimiter`] and the projector [`Nudge`].
//! - [`location`] and [`cookies`] shape the reply; [`PREFILL_SCRIPT`] fills the form.
//!
//! # Examples
//!
//! ```
//! use cms_comment_intake::{Rejection, Settings};
//!
//! let settings = Settings::from_values(|_| None)?;
//! assert!(settings.require_name_email);
//! assert_eq!(Rejection::Flood.status(), 429);
//! # Ok::<(), cms_comment_intake::Error>(())
//! ```

mod checks;
mod decide;
mod email;
mod error;
mod intake;
mod markup;
mod prefill;
mod respond;
mod settings;
mod store;
mod time;

pub use decide::{Approval, Candidate, Commenter, History, Post, Rejection, Submission, decide};
pub use error::Error;
pub use intake::{Accepted, Nudge, RateLimiter, accept, rate_key};
pub use markup::escape_html;
pub use prefill::{FETCH_HEADER, PREFILL_SCRIPT};
pub use respond::{COOKIE_SECONDS, Target, cookie_hash, cookies, location, moderation_hash};
pub use settings::{FLOOD_SECONDS, RATE_PER_MINUTE, Settings};
pub use store::SqlIntake;
