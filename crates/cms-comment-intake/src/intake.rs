//! The whole submission: rate limit, decide, store with the outbox row, nudge the projector.

use cms_db::Store;

use crate::decide::{Candidate, Commenter, Rejection, Submission, decide};
use crate::error::Error;
use crate::settings::{RATE_PER_MINUTE, Settings};
use crate::store::SqlIntake;

/// Counts requests per key in the limits table (a conditional `UpdateItem` with a TTL).
pub trait RateLimiter {
    /// Adds one hit to `key` and returns the count including it.
    ///
    /// # Errors
    ///
    /// [`Error::Port`] when the table cannot be reached.
    fn hit(&mut self, key: &str) -> Result<u64, Error>;
}

/// Wakes the projector after a commit (an async invoke with an empty payload).
pub trait Nudge {
    /// Sends one nudge.
    ///
    /// # Errors
    ///
    /// [`Error::Port`] when the invoke is refused.
    fn nudge(&mut self) -> Result<(), Error>;
}

/// A stored comment.
#[derive(Debug, PartialEq, Eq)]
pub struct Accepted {
    /// The new `comment_ID`.
    pub id: u64,
    /// What was stored.
    pub candidate: Candidate,
    /// A failed nudge, for the caller to log. The comment is committed either way and the
    /// projector's schedule picks it up.
    pub nudge_error: Option<Error>,
}

/// The limits-table key for one client address in one minute.
#[must_use]
pub fn rate_key(ip: &str, now: i64) -> String {
    format!("RL#comments#{ip}#{}", now.div_euclid(60))
}

/// Handles one submission at `now` (Unix seconds).
///
/// # Errors
///
/// [`Error`] when the limiter, a lookup or the write transaction fails.
pub fn accept<S: Store>(
    settings: &Settings,
    sql: &mut SqlIntake<S>,
    limiter: &mut impl RateLimiter,
    nudge: &mut impl Nudge,
    submission: &Submission,
    commenter: Option<&Commenter>,
    now: i64,
) -> Result<Result<Accepted, Rejection>, Error> {
    // The limiter runs first so a flood of posts costs no database work.
    if limiter.hit(&rate_key(&submission.ip, now))? > RATE_PER_MINUTE {
        return Ok(Err(Rejection::RateLimited));
    }
    let candidate = match decide(settings, sql, submission, commenter, now)? {
        Ok(candidate) => candidate,
        Err(rejection) => return Ok(Err(rejection)),
    };
    let id = sql.commit(&candidate)?;
    let nudge_error = nudge.nudge().err();
    Ok(Ok(Accepted {
        id,
        candidate,
        nudge_error,
    }))
}
