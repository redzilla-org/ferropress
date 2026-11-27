//! Ports the login service needs from storage, implemented by infrastructure adapters.

use cms_users::UserId;

use crate::error::Error;
use crate::token::TokenDigest;

/// One stored session. Holds no secret: the token itself is never stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// sha256 hex of the cookie token; the store's key.
    pub digest: TokenDigest,
    /// Owner, also the key of the "log out everywhere" index.
    pub user: UserId,
    /// Unix seconds at login.
    pub created: u64,
    /// Unix seconds after which the session is dead; also the store's TTL.
    pub expires: u64,
    /// sha256 hex of the login request's User-Agent.
    pub agent: String,
}

/// Session storage (the sessions table).
pub trait SessionStore {
    /// Store `session`, replacing any session with the same digest.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the store cannot be reached.
    fn put(&mut self, session: &Session) -> Result<(), Error>;

    /// The session for `digest`, expired or not, or `None`.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the store cannot be reached.
    fn get(&mut self, digest: &TokenDigest) -> Result<Option<Session>, Error>;

    /// Remove the session for `digest`; removing a missing one succeeds.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the store cannot be reached.
    fn delete(&mut self, digest: &TokenDigest) -> Result<(), Error>;

    /// Remove every session of `user`, returning how many were removed.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the store cannot be reached.
    fn delete_all_for_user(&mut self, user: UserId) -> Result<u64, Error>;
}

/// One fixed-window counter. The window start is part of `name`, so a new window is a new key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateKey {
    /// Store key, for example `RL#login-ip#192.0.2.1#29475000`.
    pub name: String,
    /// Unix seconds after which the counter may be dropped (the store's TTL).
    pub expires: u64,
}

/// Atomic counters (the limits table).
pub trait RateLimiter {
    /// Add one to `key`'s counter, creating it at one, and return the new count.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the store cannot be reached.
    fn increment(&mut self, key: &RateKey) -> Result<u64, Error>;
}

/// In-memory port fakes for tests (`test-util`).
#[cfg(any(test, feature = "test-util"))]
pub mod memory {
    use std::collections::{BTreeMap, HashMap};

    use cms_users::UserId;

    use super::{RateKey, RateLimiter, Session, SessionStore};
    use crate::error::Error;
    use crate::token::TokenDigest;

    /// A session map. `down` makes every call fail with [`Error::Unavailable`].
    #[derive(Debug, Default)]
    pub struct MemorySessions {
        /// Stored sessions by digest.
        pub sessions: BTreeMap<TokenDigest, Session>,
        /// Simulate an outage.
        pub down: bool,
    }

    impl MemorySessions {
        fn up(&self) -> Result<(), Error> {
            if self.down {
                Err(Error::Unavailable)
            } else {
                Ok(())
            }
        }
    }

    impl SessionStore for MemorySessions {
        fn put(&mut self, session: &Session) -> Result<(), Error> {
            self.up()?;
            self.sessions
                .insert(session.digest.clone(), session.clone());
            Ok(())
        }

        fn get(&mut self, digest: &TokenDigest) -> Result<Option<Session>, Error> {
            self.up()?;
            Ok(self.sessions.get(digest).cloned())
        }

        fn delete(&mut self, digest: &TokenDigest) -> Result<(), Error> {
            self.up()?;
            self.sessions.remove(digest);
            Ok(())
        }

        fn delete_all_for_user(&mut self, user: UserId) -> Result<u64, Error> {
            self.up()?;
            let before = self.sessions.len();
            self.sessions.retain(|_, session| session.user != user);
            Ok(u64::try_from(before - self.sessions.len()).unwrap_or(u64::MAX))
        }
    }

    /// A counter map. `down` makes every call fail with [`Error::Unavailable`].
    #[derive(Debug, Default)]
    pub struct MemoryLimits {
        /// Counts by key name.
        pub counts: HashMap<String, u64>,
        /// Simulate an outage.
        pub down: bool,
    }

    impl RateLimiter for MemoryLimits {
        fn increment(&mut self, key: &RateKey) -> Result<u64, Error> {
            if self.down {
                return Err(Error::Unavailable);
            }
            let count = self.counts.entry(key.name.clone()).or_insert(0);
            *count += 1;
            Ok(*count)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::memory::{MemoryLimits, MemorySessions};
    use super::{RateKey, RateLimiter, Session, SessionStore};
    use crate::error::Error;
    use crate::token::TokenDigest;
    use cms_users::UserId;

    fn session(fill: &str, user: u64) -> Option<Session> {
        Some(Session {
            digest: TokenDigest::parse(&fill.repeat(32))?,
            user: UserId::new(user).ok()?,
            created: 1,
            expires: 2,
            agent: String::new(),
        })
    }

    #[test]
    fn memory_sessions_put_get_delete_and_delete_by_user() {
        let (Some(a), Some(b), Some(c)) = (session("aa", 1), session("bb", 1), session("cc", 2))
        else {
            panic!("fixture sessions");
        };
        let mut store = MemorySessions::default();
        for s in [&a, &b, &c] {
            assert!(store.put(s).is_ok());
        }
        assert_eq!(store.get(&a.digest), Ok(Some(a.clone())));
        assert!(store.delete(&a.digest).is_ok());
        assert_eq!(store.get(&a.digest), Ok(None));
        assert_eq!(store.delete_all_for_user(a.user), Ok(1));
        assert_eq!(store.get(&c.digest), Ok(Some(c.clone())));
        store.down = true;
        assert_eq!(store.put(&a), Err(Error::Unavailable));
        assert_eq!(store.get(&a.digest), Err(Error::Unavailable));
        assert_eq!(store.delete(&a.digest), Err(Error::Unavailable));
        assert_eq!(store.delete_all_for_user(a.user), Err(Error::Unavailable));
    }

    #[test]
    fn memory_limits_count_per_key() {
        let key = RateKey {
            name: "k".to_owned(),
            expires: 9,
        };
        let mut limits = MemoryLimits::default();
        assert_eq!(limits.increment(&key), Ok(1));
        assert_eq!(limits.increment(&key), Ok(2));
        limits.down = true;
        assert_eq!(limits.increment(&key), Err(Error::Unavailable));
    }
}
