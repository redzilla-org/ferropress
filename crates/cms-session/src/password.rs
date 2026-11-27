//! Stored password hash formats: check, rehash decision and new hashes.
//!
//! Formats, in the order the reference tries them:
//! - 32 bytes or fewer: plain MD5 hex (oldest installs);
//! - `$P$` / `$H$`: the portable iterated-MD5 form;
//! - `$wp` + bcrypt: bcrypt over base64(HMAC-SHA384(password, `wp-sha384`)), the current form;
//! - `$2a$` / `$2b$` / `$2y$`: plain bcrypt.
//!
//! Anything else fails verification.

use std::fmt;

use bcrypt::Version;
use md5::{Digest, Md5};

use crate::error::Error;
use crate::mac::{hex, hmac_sha384, same};
use crate::portable;
use crate::token::Entropy;

/// bcrypt cost for new hashes: the default of the reference runtime (PHP 8.4 and later).
pub const COST: u32 = 12;
/// Marker the reference puts before a bcrypt hash of the pre-hashed password.
const PREFIX: &[u8] = b"$wp";
/// HMAC key of the pre-hash; a fixed reference constant, not a secret.
const PREHASH_KEY: &[u8] = b"wp-sha384";
const MD5_HEX: usize = 32;

/// A plaintext password from a login form. Redacted in [`Debug`]; never log it.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(Box<[u8]>);

impl Password {
    /// Wrap the submitted bytes unchanged.
    #[must_use]
    pub fn new(bytes: impl AsRef<[u8]>) -> Self {
        Self(Box::from(bytes.as_ref()))
    }

    /// Whether nothing was submitted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A copy without leading or trailing space, tab, newline, carriage return, NUL or vertical
    /// tab, the characters the reference strips from a submitted password.
    #[must_use]
    pub fn trimmed(&self) -> Self {
        let strip = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0 | 0x0b);
        let start = self
            .0
            .iter()
            .position(|b| !strip(b))
            .unwrap_or(self.0.len());
        let end = self
            .0
            .iter()
            .rposition(|b| !strip(b))
            .map_or(start, |i| i + 1);
        Self::new(&self.0[start..end])
    }

    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(..)")
    }
}

/// Which stored format a hash is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// `$wp` + bcrypt over the pre-hash.
    Prehashed,
    /// Plain bcrypt.
    Bcrypt,
    /// `$P$` / `$H$` iterated MD5.
    Portable,
    /// 32 bytes or fewer, compared as MD5 hex.
    Md5,
    /// Nothing this crate can check.
    Unknown,
}

impl Scheme {
    /// Classify a stored `user_pass` value.
    #[must_use]
    pub fn of(hash: &[u8]) -> Self {
        if hash.len() <= MD5_HEX {
            Self::Md5
        } else if portable::is_portable(hash) {
            Self::Portable
        } else if hash.strip_prefix(PREFIX).is_some_and(is_bcrypt) {
            Self::Prehashed
        } else if is_bcrypt(hash) {
            Self::Bcrypt
        } else {
            Self::Unknown
        }
    }
}

/// Whether `password` matches the stored `hash`.
#[must_use]
pub fn verify(password: &Password, hash: &[u8]) -> bool {
    let input = password.bytes();
    match Scheme::of(hash) {
        Scheme::Md5 => same(hex(&Md5::digest(input)).as_bytes(), hash),
        Scheme::Portable => portable::verify(input, hash),
        Scheme::Prehashed => bcrypt_verify(prehash(input).as_bytes(), &hash[PREFIX.len()..]),
        Scheme::Bcrypt => bcrypt_verify(input, hash),
        Scheme::Unknown => false,
    }
}

/// Whether a hash that just verified should be replaced by [`hash_password`] output.
///
/// Only the current form at the current cost is kept: `$wp$2y$` with cost [`COST`].
#[must_use]
pub fn needs_rehash(hash: &[u8]) -> bool {
    let Some(rest) = hash.strip_prefix(PREFIX) else {
        return true;
    };
    !(is_bcrypt(rest) && rest.starts_with(b"$2y$") && cost(rest) == Some(COST))
}

/// A new hash in the current form: `$wp$2y$12$…`.
///
/// # Errors
///
/// [`Error::Entropy`] when no salt can be drawn, [`Error::Hash`] when bcrypt rejects the cost.
pub fn hash_password(password: &Password, entropy: &mut impl Entropy) -> Result<String, Error> {
    hash_with_cost(password, COST, entropy)
}

fn hash_with_cost(
    password: &Password,
    cost: u32,
    entropy: &mut impl Entropy,
) -> Result<String, Error> {
    let mut salt = [0_u8; 16];
    entropy.fill(&mut salt)?;
    let parts = bcrypt::hash_with_salt(prehash(password.bytes()), cost, salt)
        .map_err(|_rejected| Error::Hash)?;
    let mut out = String::from_utf8_lossy(PREFIX).into_owned();
    out.push_str(&parts.format_for_version(Version::TwoY));
    Ok(out)
}

// base64(HMAC-SHA384) keeps the bcrypt input at 64 bytes, under bcrypt's 72-byte limit.
fn prehash(password: &[u8]) -> String {
    base64(&hmac_sha384(PREHASH_KEY, password))
}

// The reference's bcrypt reads the password as a C string, so input stops at the first NUL.
fn bcrypt_verify(password: &[u8], hash: &[u8]) -> bool {
    let end = password
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(password.len());
    std::str::from_utf8(hash)
        .ok()
        .and_then(|hash| bcrypt::verify(&password[..end], hash).ok())
        .unwrap_or(false)
}

// `$2a$`, `$2b$` or `$2y$`, a two-digit cost, `$`, then 53 characters: 60 bytes in all.
fn is_bcrypt(hash: &[u8]) -> bool {
    hash.len() == 60
        && hash
            .get(..4)
            .is_some_and(|id| matches!(id, b"$2a$" | b"$2b$" | b"$2y$"))
        && hash.get(6) == Some(&b'$')
        && cost(hash).is_some()
}

fn cost(hash: &[u8]) -> Option<u32> {
    let digits = hash.get(4..6)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(u32::from(digits[0] - b'0') * 10 + u32::from(digits[1] - b'0'))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |acc, (i, &b)| acc | (u32::from(b) << (16 - 8 * i)));
        for group in 0..4 {
            if group <= chunk.len() {
                out.push(char::from(
                    ALPHABET[((value >> (18 - 6 * group)) & 0x3f) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        COST, Password, Scheme, base64, cost, hash_password, hash_with_cost, needs_rehash, prehash,
        verify,
    };
    use crate::error::Error;
    use crate::token::Entropy;

    // Fixture sources (no reference lab or PHP on the build host):
    // - PREHASHED: python3 `base64(hmac.new(b"wp-sha384", b"password", sha384))` gives
    //   PREHASH_OF_PASSWORD; Apache `htpasswd -nbBC 5` (APR's independent bcrypt) over that
    //   string gives the `$2y$05$…` part.
    // - BCRYPT: `htpasswd -nbBC 5 u correct-horse`.
    // - OPENWALL: crypt_blowfish 1.3 wrapper.c test vector for "U*U" (openwall.com/crypt).
    // - PORTABLE: phpass 0.3 test.php vector for "test12345".
    // - MD5: md5("password"), RFC 1321 implementation in python3 hashlib.
    const PREHASH_OF_PASSWORD: &str =
        "ZEIBxmCIjkyF8MNbMmsrJOPmp2/ioyAvd90BaIOY1lr6z3gfcWZusfjOv0kWasvS";
    const PREHASHED: &[u8] = b"$wp$2y$05$kHbrVlDOfaQPB.OpcfgVpeX/MDYHYsIZybpMWqNi6GhEq6LvQndRy";
    const BCRYPT: &[u8] = b"$2y$05$jMESjQ0wL8gqyWN4KgyDguu.fC.fUPZWLEVd63sya8Po2BoJkCFLy";
    const OPENWALL: &[u8] = b"$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW";
    const PORTABLE: &[u8] = b"$P$9IQRaTwmfeRo7ud9Fh4E2PdI0S3r.L0";
    const MD5: &[u8] = b"5f4dcc3b5aa765d61d8327deb882cf99";

    struct Fixed;

    impl Entropy for Fixed {
        fn fill(&mut self, buf: &mut [u8]) -> Result<(), Error> {
            buf.fill(7);
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
    fn every_stored_format_verifies_its_fixture_and_rejects_a_wrong_password() {
        let cases: [(&[u8], &str, Scheme); 5] = [
            (PREHASHED, "password", Scheme::Prehashed),
            (BCRYPT, "correct-horse", Scheme::Bcrypt),
            (OPENWALL, "U*U", Scheme::Bcrypt),
            (PORTABLE, "test12345", Scheme::Portable),
            (MD5, "password", Scheme::Md5),
        ];
        for (hash, word, scheme) in cases {
            assert_eq!(Scheme::of(hash), scheme);
            assert!(verify(&Password::new(word), hash), "{scheme:?}");
            assert!(!verify(&Password::new("wrong"), hash), "{scheme:?}");
        }
        // The `$2b$` spelling of the openwall hash is the same computation.
        let mut two_b = OPENWALL.to_vec();
        two_b[2] = b'b';
        assert!(verify(&Password::new("U*U"), &two_b));
    }

    #[test]
    fn the_prehash_matches_an_independent_hmac_and_stops_bcrypt_at_nul() {
        assert_eq!(prehash(b"password"), PREHASH_OF_PASSWORD);
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // Bytes after a NUL are ignored, like the reference's C-string bcrypt input.
        assert!(verify(&Password::new(b"correct-horse\0tail"), BCRYPT));
    }

    #[test]
    fn unknown_or_malformed_hashes_fail() {
        let unknown: [&[u8]; 5] = [
            b"$1$abcdefgh$0123456789012345678901",
            b"$6$rounds=5000$salt$0123456789012345678901234567890",
            b"$2x$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW",
            b"$2y$xx$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW",
            b"$wp$2y$05$short-and-wrong-length-but-over-32-bytes",
        ];
        for hash in unknown {
            assert_eq!(Scheme::of(hash), Scheme::Unknown);
            assert!(!verify(&Password::new("U*U"), hash));
        }
        // Right shape, bad base64 in the salt: bcrypt refuses, so the check fails.
        assert!(!verify(
            &Password::new("U*U"),
            b"$2y$05$!!!!!!!!!!!!!!!!!!!!!.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW"
        ));
        assert!(!verify(&Password::new("U*U"), &[0xff; 60]));
        assert_eq!(cost(b"$2y$1"), None);
    }

    #[test]
    fn only_the_current_form_at_the_current_cost_is_kept() {
        assert!(needs_rehash(PREHASHED));
        assert!(needs_rehash(BCRYPT));
        assert!(needs_rehash(PORTABLE));
        assert!(needs_rehash(MD5));
        assert!(needs_rehash(
            b"$wp$2a$12$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW"
        ));
        assert!(!needs_rehash(
            b"$wp$2y$12$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW"
        ));
    }

    #[test]
    fn new_hashes_are_prehashed_2y_and_verify() {
        let low = hash_with_cost(&Password::new("password"), 5, &mut Fixed);
        assert!(low.is_ok());
        if let Ok(low) = low {
            assert!(low.starts_with("$wp$2y$05$"));
            assert_eq!(low.len(), 63);
            assert!(verify(&Password::new("password"), low.as_bytes()));
            assert!(!verify(&Password::new("Password"), low.as_bytes()));
        }
        let current = hash_password(&Password::new("password"), &mut Fixed);
        assert!(current.is_ok());
        if let Ok(current) = current {
            assert!(current.starts_with("$wp$2y$12$"));
            assert!(!needs_rehash(current.as_bytes()));
        }
        assert_eq!(COST, 12);
        assert_eq!(
            hash_password(&Password::new("x"), &mut Broken).err(),
            Some(Error::Entropy)
        );
        assert_eq!(
            hash_with_cost(&Password::new("x"), 99, &mut Fixed).err(),
            Some(Error::Hash)
        );
    }

    #[test]
    fn passwords_are_redacted() {
        let password = Password::new("hunter2");
        assert_eq!(format!("{password:?}"), "Password(..)");
        assert!(!password.is_empty());
        assert!(Password::new("").is_empty());
        assert_eq!(
            Password::new(" \t\0a b\x0b\r\n").trimmed(),
            Password::new("a b")
        );
        assert!(Password::new(" \n ").trimmed().is_empty());
        assert_eq!(Password::new("x").trimmed(), Password::new("x"));
    }
}
