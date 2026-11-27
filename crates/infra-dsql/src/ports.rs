//! What the adapter needs from its host: endpoint, auth tokens, streams and time.

use std::fmt;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::Error;

/// Where to connect and as whom. The password is never here: it is the IAM token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    host: Box<str>,
    port: u16,
    user: Box<str>,
    database: Box<str>,
}

impl Endpoint {
    /// A cluster endpoint, e.g. `("abc.dsql.us-east-1.on.aws", 5432, "app", "postgres")`.
    #[must_use]
    pub fn new(host: &str, port: u16, user: &str, database: &str) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            database: database.into(),
        }
    }

    /// The host name, also the TLS server name.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The TCP port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// The database role.
    #[must_use]
    pub fn user(&self) -> &str {
        &self.user
    }

    /// The database name.
    #[must_use]
    pub fn database(&self) -> &str {
        &self.database
    }
}

/// A database auth token and how long it stays valid from the moment it was issued.
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    secret: Box<str>,
    lifetime: Duration,
}

impl Token {
    /// A token valid for `lifetime` (DSQL IAM tokens: 15 minutes by default).
    #[must_use]
    pub fn new(secret: impl Into<Box<str>>, lifetime: Duration) -> Self {
        Self {
            secret: secret.into(),
            lifetime,
        }
    }

    /// The password to send.
    #[must_use]
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// How long the token is valid after issue.
    #[must_use]
    pub const fn lifetime(&self) -> Duration {
        self.lifetime
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The token is a bearer credential; it must never reach logs.
        out.debug_struct("Token")
            .field("secret", &"<redacted>")
            .field("lifetime", &self.lifetime)
            .finish()
    }
}

/// Issues DSQL auth tokens (`dsql:DbConnect`), e.g. a SigV4 presigner in the binary.
pub trait TokenSource {
    /// A fresh token for `endpoint`.
    ///
    /// # Errors
    ///
    /// [`Error::Token`] when no credentials are available.
    fn token(&mut self, endpoint: &Endpoint) -> Result<Token, Error>;
}

/// Opens byte streams to the server, with TLS already negotiated where required.
pub trait Connector {
    /// The stream type.
    type Stream: Read + Write;

    /// A connected stream to `endpoint`, ready for the startup message.
    ///
    /// # Errors
    ///
    /// I/O or TLS failures.
    fn connect(&self, endpoint: &Endpoint) -> Result<Self::Stream, Error>;
}

/// Time as the adapter sees it, injected so tests control ages and backoff.
pub trait Clock {
    /// The current instant.
    fn now(&self) -> Instant;
    /// Blocks for `delay` (retry backoff only).
    fn sleep(&self, delay: Duration);
}

/// The operating system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, Endpoint, SystemClock, Token};
    use std::time::Duration;

    #[test]
    fn endpoint_keeps_its_parts() {
        let endpoint = Endpoint::new("db.example.com", 5432, "app", "postgres");
        assert_eq!(endpoint.host(), "db.example.com");
        assert_eq!(endpoint.port(), 5432);
        assert_eq!(endpoint.user(), "app");
        assert_eq!(endpoint.database(), "postgres");
    }

    #[test]
    fn token_debug_redacts_the_secret() {
        let token = Token::new("signed-secret", Duration::from_secs(900));
        assert_eq!(token.secret(), "signed-secret");
        assert_eq!(token.lifetime(), Duration::from_secs(900));
        let shown = format!("{token:?}");
        assert!(shown.contains("<redacted>"));
        assert!(!shown.contains("signed-secret"));
    }

    #[test]
    fn system_clock_moves_forward() {
        let clock = SystemClock;
        let before = clock.now();
        clock.sleep(Duration::from_millis(1));
        assert!(clock.now() > before);
    }
}
