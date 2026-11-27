//! Failures from hashing, ports and the login service.

use cms_auth::Error as AuthError;
use cms_db::NameError as TableNameError;
use cms_users::Error as UsersError;

/// A dependency failed. Rejected credentials and expired sessions are outcomes, not errors.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The operating system random source failed.
    #[error("the random source is unavailable")]
    Entropy,
    /// The bcrypt hasher rejected its input.
    #[error("the password hasher rejected its input")]
    Hash,
    /// The user store, session store or rate limiter could not be reached.
    #[error("an auth dependency is unavailable")]
    Unavailable,
    /// The prefix could not form a table name.
    #[error(transparent)]
    Table(#[from] TableNameError),
}

impl Error {
    /// Whether this is [`Self::Unavailable`].
    #[must_use]
    pub const fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

impl From<UsersError> for Error {
    fn from(error: UsersError) -> Self {
        if let UsersError::Table(name) = error {
            Self::Table(name)
        } else {
            Self::Unavailable
        }
    }
}

impl From<AuthError> for Error {
    fn from(error: AuthError) -> Self {
        if let AuthError::Table(name) = error {
            Self::Table(name)
        } else {
            Self::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use cms_auth::Error as AuthError;
    use cms_db::NameError as TableNameError;
    use cms_users::Error as UsersError;

    #[test]
    fn dependency_failures_map_to_unavailable_or_table() {
        assert!(Error::Unavailable.is_unavailable());
        assert!(!Error::Entropy.is_unavailable());
        assert_eq!(
            Error::Entropy.to_string(),
            "the random source is unavailable"
        );
        assert_eq!(
            Error::Hash.to_string(),
            "the password hasher rejected its input"
        );
        assert!(Error::from(UsersError::Unavailable).is_unavailable());
        assert!(Error::from(AuthError::Unavailable).is_unavailable());
        let table = Error::from(UsersError::from(TableNameError::Nul));
        assert_eq!(table, Error::Table(TableNameError::Nul));
        let table = Error::from(AuthError::from(TableNameError::Nul));
        assert_eq!(table, Error::Table(TableNameError::Nul));
    }
}
