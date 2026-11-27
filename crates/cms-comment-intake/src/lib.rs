//! Accept, hold or refuse one public comment submission, then store it with an outbox row.
//!
//! Serverless profile, `docs/aws/architecture.md` sections 3 and 5 (issue #11). The rules follow
//! the reference's documented comment-submission behavior; they are reimplemented here, not
//! translated. Hooks and plugin filters are not run.
//!
//! # Examples
//!
//! ```
//! use cms_comment_intake::Settings;
//!
//! let settings = Settings::from_values(|_| None)?;
//! assert!(settings.require_name_email);
//! assert_eq!(settings.max_links, 2);
//! # Ok::<(), cms_comment_intake::Error>(())
//! ```

mod checks;
mod email;
mod error;
mod settings;
mod time;

pub use error::Error;
pub use settings::{FLOOD_SECONDS, RATE_PER_MINUTE, Settings};
