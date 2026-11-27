//! Aurora DSQL adapter for the [`cms_db::Store`] port (serverless profile).
//!
//! Speaks the `PostgreSQL` v3 wire protocol directly over any blocking byte stream: startup,
//! cleartext-password auth (DSQL takes the IAM token as the password, over TLS), and the simple
//! Query protocol, since statements arrive fully rendered by [`cms_db::Dialect::Postgres`].
//! Replies keep the port's shape: text cells, affected rows from the command tag, the insert id
//! from `RETURNING "ID"`, and SQLSTATEs mapped into [`cms_db::ServerError`].
//!
//! [`Dsql`] reuses one connection across warm invocations, reconnects before DSQL's 60-minute
//! cap, refreshes the token on expiry, reruns [`Dsql::with_transaction`] closures on optimistic
//! concurrency conflicts, and applies `deploy/aws/dsql/*.sql` with [`Dsql::migrate`].
//! Contract: `contracts/db/store.md`; design: `docs/aws/architecture.md` sections 4 and 8.
//!
//! # Examples
//!
//! ```
//! use infra_dsql::{Endpoint, SqlState, Token};
//! use std::time::Duration;
//!
//! let endpoint = Endpoint::new("cluster.example.com", 5432, "app", "postgres");
//! assert_eq!(endpoint.user(), "app");
//! let token = Token::new("presigned", Duration::from_secs(900));
//! assert!(!format!("{token:?}").contains("presigned"));
//! assert!(SqlState::new(*b"OC000").is_conflict());
//! ```

mod conn;
mod error;
mod migrate;
mod ports;
mod session;
mod sqlstate;
mod tls;
mod transaction;
mod wire;

#[cfg(feature = "test-util")]
pub mod fake;

pub use error::Error;
pub use migrate::{Applied, MigrateError, Migration};
pub use ports::{Clock, Connector, Endpoint, SystemClock, Token, TokenSource};
pub use session::{DEFAULT_MAX_AGE, Dsql, Retry};
pub use sqlstate::{PgError, SqlState};
pub use tls::Tls;
pub use transaction::{Transaction, TransactionError};
