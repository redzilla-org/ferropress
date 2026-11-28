//! The reusable DSQL store: lazy connect, age-capped reuse and token refresh.

use std::fmt;
use std::time::{Duration, Instant};

use cms_db::{Dialect, Reply, Sql, Store, StoreError};

use crate::Error;
use crate::conn::{Connection, Outcome, TxStatus};
use crate::ports::{Clock, Connector, Endpoint, SystemClock, Token, TokenSource};

/// DSQL closes connections after 60 minutes; reconnecting at 55 leaves room for a long statement.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(55 * 60);
/// A token this close to expiry is replaced before connecting, so it cannot lapse mid-handshake.
const TOKEN_MARGIN: Duration = Duration::from_secs(60);

/// How conflicting transactions are rerun: full-jitter exponential backoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retry {
    /// Total runs of the closure, the first included.
    pub attempts: u32,
    /// Upper bound of the first delay.
    pub base: Duration,
    /// Upper bound of any delay.
    pub cap: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            attempts: 8,
            base: Duration::from_millis(20),
            cap: Duration::from_secs(2),
        }
    }
}

struct Live<S: std::io::Read + std::io::Write> {
    connection: Connection<S>,
    opened: Instant,
}

/// A [`Store`] over one DSQL connection, opened on first use and reused across warm invocations.
pub struct Dsql<C: Connector, K: TokenSource> {
    endpoint: Endpoint,
    connector: C,
    tokens: K,
    clock: Box<dyn Clock>,
    max_age: Duration,
    pub(crate) retry: Retry,
    rng: u64,
    token: Option<(Token, Instant)>,
    live: Option<Live<C::Stream>>,
}

impl<C: Connector, K: TokenSource> fmt::Debug for Dsql<C, K> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Dsql")
            .field("endpoint", &self.endpoint)
            .field("max_age", &self.max_age)
            .field("retry", &self.retry)
            .field("connected", &self.live.is_some())
            .finish_non_exhaustive()
    }
}

impl<C: Connector, K: TokenSource> Dsql<C, K> {
    /// A store for `endpoint`. Nothing connects until the first statement, so cold starts that
    /// never touch the database pay nothing.
    pub fn new(endpoint: Endpoint, connector: C, tokens: K) -> Self {
        // Jitter only needs to differ between concurrent Lambdas, not be unpredictable.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9_7f4a_7c15, |since| {
                u64::from(since.subsec_nanos()) ^ since.as_secs().rotate_left(32)
            });
        Self {
            endpoint,
            connector,
            tokens,
            clock: Box::new(SystemClock),
            max_age: DEFAULT_MAX_AGE,
            retry: Retry::default(),
            rng: seed | 1,
            token: None,
            live: None,
        }
    }

    /// Replaces the clock (tests use a manual one).
    #[must_use]
    pub fn with_clock(mut self, clock: impl Clock + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Replaces the jitter seed, for reproducible backoff.
    #[must_use]
    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.rng = seed | 1;
        self
    }

    /// Replaces the reconnect age.
    #[must_use]
    pub const fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }

    /// Replaces the conflict retry policy.
    #[must_use]
    pub const fn with_retry(mut self, retry: Retry) -> Self {
        self.retry = retry;
        self
    }

    /// Whether a connection is open.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.live.is_some()
    }

    /// Closes the connection, if any; the next statement reconnects.
    pub fn disconnect(&mut self) {
        self.live = None;
    }

    /// Runs `sql` and keeps the SQLSTATE, reconnecting first when the connection is missing or
    /// too old. Callers inside a transaction use [`Self::run_in_block`] instead.
    pub(crate) fn run(&mut self, sql: &[u8]) -> Result<Outcome, Error> {
        self.ensure_connected()?;
        self.run_in_block(sql)
    }

    /// Runs `sql` on the open connection without reconnecting, since a new connection would
    /// silently leave the transaction.
    pub(crate) fn run_in_block(&mut self, sql: &[u8]) -> Result<Outcome, Error> {
        let live = self
            .live
            .as_mut()
            .ok_or(Error::Protocol("no open connection"))?;
        let outcome = live.connection.query(sql);
        if outcome.is_err() {
            // After an I/O or protocol failure the stream position is unknown.
            self.live = None;
        }
        outcome
    }

    /// The server's transaction state, `None` when disconnected.
    pub(crate) fn status(&self) -> Option<TxStatus> {
        self.live.as_ref().map(|live| live.connection.status())
    }

    /// Sleeps a full-jitter delay before rerun number `attempt` (1-based count of failures).
    pub(crate) fn backoff(&mut self, attempt: u32) {
        let ceiling = self
            .retry
            .base
            .saturating_mul(
                1_u32
                    .checked_shl(attempt.saturating_sub(1))
                    .unwrap_or(u32::MAX),
            )
            .min(self.retry.cap);
        // xorshift64: cheap, dependency-free spread across concurrent writers.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let nanos = u64::try_from(ceiling.as_nanos()).unwrap_or(u64::MAX);
        let delay = Duration::from_nanos(self.rng % nanos.saturating_add(1));
        self.clock.sleep(delay);
    }

    fn ensure_connected(&mut self) -> Result<(), Error> {
        let now = self.clock.now();
        if let Some(live) = &self.live
            && now.saturating_duration_since(live.opened) < self.max_age
        {
            return Ok(());
        }
        self.live = None;
        let connection = match self.open() {
            // A rejected token (expired early, rotated role) gets one retry with a fresh one.
            Err(Error::Refused { state, .. }) if state.is_auth_failure() => {
                self.token = None;
                self.open()?
            }
            other => other?,
        };
        self.live = Some(Live {
            connection,
            opened: self.clock.now(),
        });
        Ok(())
    }

    fn open(&mut self) -> Result<Connection<C::Stream>, Error> {
        let now = self.clock.now();
        let secret = match &self.token {
            Some((token, issued))
                if now.saturating_duration_since(*issued) + TOKEN_MARGIN < token.lifetime() =>
            {
                token.secret().to_owned()
            }
            _ => {
                let token = self.tokens.token(&self.endpoint)?;
                let secret = token.secret().to_owned();
                self.token = Some((token, now));
                secret
            }
        };
        let stream = self.connector.connect(&self.endpoint)?;
        Connection::open(
            stream,
            self.endpoint.user(),
            self.endpoint.database(),
            &secret,
        )
    }
}

impl<C: Connector, K: TokenSource> Store for Dsql<C, K> {
    // DSQL speaks PostgreSQL, so repositories render statements in that dialect.
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn query(&mut self, sql: &Sql) -> Result<Reply, StoreError> {
        Ok(self.run(sql.as_bytes())?.into_reply())
    }
}
