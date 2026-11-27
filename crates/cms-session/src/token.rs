//! Random session tokens and the digest the server stores in their place.

use std::fmt;

use crate::error::Error;
use crate::mac::sha256_hex;

/// Token length in bytes (256 bits).
const BYTES: usize = 32;

/// A source of cryptographically secure random bytes.
pub trait Entropy {
    /// Fill `buf` completely.
    ///
    /// # Errors
    ///
    /// [`Error::Entropy`] when the source cannot produce bytes.
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), Error>;
}

/// The operating system CSPRNG.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        getrandom::fill(buf).map_err(|_source| Error::Entropy)
    }
}

/// The cookie value: 64 lowercase hex characters. Redacted in [`Debug`].
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(String);

impl SessionToken {
    /// A fresh token from `entropy`.
    ///
    /// # Errors
    ///
    /// [`Error::Entropy`] when the source fails.
    pub fn generate(entropy: &mut impl Entropy) -> Result<Self, Error> {
        let mut raw = [0_u8; BYTES];
        entropy.fill(&mut raw)?;
        Ok(Self(crate::mac::hex(&raw)))
    }

    /// A token read back from a cookie, or `None` when it is not 64 lowercase hex characters.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let well_formed = value.len() == BYTES * 2
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        well_formed.then(|| Self(value.to_owned()))
    }

    /// The secret cookie value. Never log it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// What the session store keys on.
    #[must_use]
    pub fn digest(&self) -> TokenDigest {
        TokenDigest(sha256_hex(self.0.as_bytes()))
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionToken(..)")
    }
}

/// Lowercase hex sha256 of a token. Not secret: it cannot be turned back into a cookie.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenDigest(String);

impl TokenDigest {
    /// A digest read back from a store, or `None` when it is not 64 lowercase hex characters.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        SessionToken::parse(value).map(|token| Self(token.0))
    }

    /// The hex text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Entropy, OsEntropy, SessionToken, TokenDigest};
    use crate::error::Error;

    struct Fixed(u8);

    impl Entropy for Fixed {
        fn fill(&mut self, buf: &mut [u8]) -> Result<(), Error> {
            buf.fill(self.0);
            Ok(())
        }
    }

    struct Broken;

    impl Entropy for Broken {
        fn fill(&mut self, _: &mut [u8]) -> Result<(), Error> {
            Err(Error::Entropy)
        }
    }

    #[test]
    fn tokens_are_hex_with_a_sha256_digest_and_redacted_debug() {
        let token = SessionToken::generate(&mut Fixed(0xab));
        assert!(token.is_ok());
        if let Ok(token) = token {
            assert_eq!(token.expose(), "ab".repeat(32));
            // Independent value: python3 hashlib.sha256(b"ab" * 32).hexdigest().
            assert_eq!(
                token.digest().as_str(),
                "271a413bd339c5709fdceaec41f14f11e9fbfb5042d72d331c65f32b284cd09a"
            );
            assert_eq!(format!("{token:?}"), "SessionToken(..)");
            assert_eq!(SessionToken::parse(token.expose()), Some(token));
        }
        assert_eq!(
            SessionToken::generate(&mut Broken).err(),
            Some(Error::Entropy)
        );
    }

    #[test]
    fn os_tokens_differ_and_parse_rejects_malformed_values() {
        let a = SessionToken::generate(&mut OsEntropy);
        let b = SessionToken::generate(&mut OsEntropy);
        assert!(a.is_ok() && b.is_ok() && a != b);
        assert!(SessionToken::parse("").is_none());
        assert!(SessionToken::parse(&"AB".repeat(32)).is_none());
        assert!(SessionToken::parse(&"ab".repeat(31)).is_none());
        assert!(SessionToken::parse(&"zz".repeat(32)).is_none());
        assert!(TokenDigest::parse(&"0f".repeat(32)).is_some());
        assert!(TokenDigest::parse("x").is_none());
    }
}
