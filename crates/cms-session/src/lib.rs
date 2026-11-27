//! Password hashes, login sessions, rate limits and nonces for the serverless auth path.
//!
//! Pure domain code: storage is reached through [`SessionStore`], [`RateLimiter`] and
//! [`cms_db::Store`]; adapters live in infrastructure crates. Design: `docs/aws/architecture.md`
//! section 7. Secret-bearing types ([`Password`], [`SessionToken`], [`NonceKey`]) redact
//! themselves in [`Debug`](std::fmt::Debug).
//!
//! # Examples
//!
//! ```
//! use cms_session::{Password, Scheme, needs_rehash, verify};
//!
//! let stored = b"$P$9IQRaTwmfeRo7ud9Fh4E2PdI0S3r.L0";
//! assert_eq!(Scheme::of(stored), Scheme::Portable);
//! assert!(verify(&Password::new("test12345"), stored));
//! assert!(needs_rehash(stored));
//! ```

mod cookie;
mod error;
mod mac;
mod nonce;
mod password;
mod portable;
mod ports;
mod token;

pub use cookie::{CookiePath, NAME as COOKIE_NAME, read as read_cookie};
pub use error::Error;
pub use nonce::{
    Age, LIFETIME as NONCE_LIFETIME, NonceKey, create as create_nonce, tick, verify as verify_nonce,
};
pub use password::{COST, Password, Scheme, hash_password, needs_rehash, verify};
#[cfg(feature = "test-util")]
pub use ports::memory;
pub use ports::{RateKey, RateLimiter, Session, SessionStore};
pub use token::{Entropy, OsEntropy, SessionToken, TokenDigest};
