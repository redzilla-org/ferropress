//! Failures of an editor workflow.

use cms_db::LiteralError;

/// Why a workflow step could not complete. Each maps to one HTTP status in the router.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The store or another port could not be reached.
    #[error("a backing service is unavailable")]
    Unavailable,
    /// The server rejected a statement inside a write; the transaction was rolled back.
    #[error("the database rejected a statement: {0}")]
    Rejected(Box<str>),
    /// The commit failed (an optimistic-concurrency conflict on DSQL); nothing was written.
    #[error("the change conflicted with a concurrent write")]
    Conflict,
    /// A value cannot be written in the store's dialect.
    #[error(transparent)]
    Literal(#[from] LiteralError),
    /// The request named a post or term that does not exist.
    #[error("not found: {0}")]
    NotFound(&'static str),
    /// The request body or a field was malformed.
    #[error("invalid request: {0}")]
    Invalid(Box<str>),
    /// The current user lacks a required capability.
    #[error("missing capability {0}")]
    Forbidden(&'static str),
}

impl Error {
    /// The HTTP status the router answers for this failure.
    #[must_use]
    pub const fn status(&self) -> u16 {
        match self {
            Self::Unavailable => 503,
            Self::Rejected(_) | Self::Literal(_) => 500,
            Self::Conflict => 409,
            Self::NotFound(_) => 404,
            Self::Invalid(_) => 400,
            Self::Forbidden(_) => 403,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use cms_db::LiteralError;

    #[test]
    fn every_failure_has_a_status_and_message() {
        let cases = [
            (Error::Unavailable, 503, "a backing service is unavailable"),
            (
                Error::Rejected("bad".into()),
                500,
                "the database rejected a statement: bad",
            ),
            (
                Error::Conflict,
                409,
                "the change conflicted with a concurrent write",
            ),
            (
                Error::from(LiteralError::Nul),
                500,
                "a PostgreSQL text literal must not contain a nul byte",
            ),
            (Error::NotFound("post"), 404, "not found: post"),
            (Error::Invalid("x".into()), 400, "invalid request: x"),
            (
                Error::Forbidden("publish_posts"),
                403,
                "missing capability publish_posts",
            ),
        ];
        for (error, status, message) in cases {
            assert_eq!(error.status(), status);
            assert_eq!(error.to_string(), message);
        }
    }
}
