//! Time-tick nonces with the reference's semantics.
//!
//! A nonce is ten hex characters of HMAC-MD5 over `tick|action|uid|token`, keyed by the site's
//! nonce key followed by its nonce salt. A tick is half the lifetime, so a nonce is accepted
//! during the tick it was made in and the next one.

use std::fmt;

use crate::mac::{hex, hmac_md5, same};
use crate::token::SessionToken;

/// Nonce lifetime in seconds (one day), the reference default.
pub const LIFETIME: u64 = 86_400;

/// The secret HMAC key: nonce key then nonce salt. Redacted in [`Debug`]; never log it.
#[derive(Clone, PartialEq, Eq)]
pub struct NonceKey(Box<[u8]>);

impl NonceKey {
    /// Join the site's nonce key and nonce salt, as the reference's salt lookup does.
    #[must_use]
    pub fn new(key: &[u8], salt: &[u8]) -> Self {
        Self([key, salt].concat().into_boxed_slice())
    }
}

impl fmt::Debug for NonceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NonceKey(..)")
    }
}

/// Which tick a valid nonce came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    /// Made in the current tick (the reference returns 1).
    Current,
    /// Made in the previous tick (the reference returns 2).
    Previous,
}

/// The tick for unix time `now`: `ceil(now / (LIFETIME / 2))`.
#[must_use]
pub const fn tick(now: u64) -> u64 {
    now.div_ceil(LIFETIME / 2)
}

/// The nonce for `action`, for user `uid` (0 when logged out) and their session `token`.
#[must_use]
pub fn create(
    key: &NonceKey,
    action: &str,
    uid: u64,
    token: Option<&SessionToken>,
    now: u64,
) -> String {
    at_tick(key, tick(now), action, uid, token)
}

/// Whether `nonce` is valid now, and from which tick. An empty nonce is never valid.
#[must_use]
pub fn verify(
    key: &NonceKey,
    nonce: &str,
    action: &str,
    uid: u64,
    token: Option<&SessionToken>,
    now: u64,
) -> Option<Age> {
    if nonce.is_empty() {
        return None;
    }
    let current = tick(now);
    if same(
        at_tick(key, current, action, uid, token).as_bytes(),
        nonce.as_bytes(),
    ) {
        return Some(Age::Current);
    }
    let previous = current.checked_sub(1)?;
    same(
        at_tick(key, previous, action, uid, token).as_bytes(),
        nonce.as_bytes(),
    )
    .then_some(Age::Previous)
}

// Characters 20..30 of the 32-hex-character MAC: the reference takes 10 starting 12 from the end.
fn at_tick(
    key: &NonceKey,
    tick: u64,
    action: &str,
    uid: u64,
    token: Option<&SessionToken>,
) -> String {
    let token = token.map_or("", SessionToken::expose);
    let message = format!("{tick}|{action}|{uid}|{token}");
    let mac = hex(&hmac_md5(&key.0, message.as_bytes()));
    mac[20..30].to_owned()
}

#[cfg(test)]
mod tests {
    use super::{Age, LIFETIME, NonceKey, create, tick, verify};
    use crate::token::SessionToken;

    // Tick 41225 ends exactly at this second.
    const NOW: u64 = 41_225 * (LIFETIME / 2);

    fn key() -> NonceKey {
        NonceKey::new(b"nonce-key-example", b"nonce-salt-example")
    }

    #[test]
    fn a_nonce_matches_an_independent_hmac_md5() {
        // python3: hmac.new(b"nonce-key-examplenonce-salt-example",
        //   b"41225|edit-post_7|3|" + b"ab" * 32, md5).hexdigest()[-12:-2]
        let token = SessionToken::parse(&"ab".repeat(32));
        assert!(token.is_some());
        assert_eq!(tick(NOW), 41_225);
        assert_eq!(tick(NOW + 1), 41_226);
        assert_eq!(
            create(&key(), "edit-post_7", 3, token.as_ref(), NOW),
            "ce4eaf09b0"
        );
    }

    #[test]
    fn nonces_live_for_the_current_and_the_next_tick() {
        let token = SessionToken::parse(&"ab".repeat(32));
        let nonce = create(&key(), "a", 3, token.as_ref(), NOW);
        let check = |nonce: &str, action: &str, uid: u64, now: u64| {
            verify(&key(), nonce, action, uid, token.as_ref(), now)
        };
        assert_eq!(check(&nonce, "a", 3, NOW), Some(Age::Current));
        assert_eq!(check(&nonce, "a", 3, NOW + 1), Some(Age::Previous));
        // Made at the last second of its tick, it lives one more tick: half a lifetime.
        assert_eq!(check(&nonce, "a", 3, NOW + LIFETIME / 2), Some(Age::Previous));
        assert_eq!(check(&nonce, "a", 3, NOW + LIFETIME / 2 + 1), None);
        assert_eq!(check(&nonce, "b", 3, NOW), None);
        assert_eq!(check(&nonce, "a", 4, NOW), None);
        assert_eq!(check("", "a", 3, NOW), None);
        assert_eq!(verify(&key(), &nonce, "a", 3, None, NOW), None);
        // Tick zero has no previous tick.
        assert_eq!(verify(&key(), "0000000000", "a", 0, None, 0), None);
        let other = NonceKey::new(b"other", b"");
        assert_eq!(verify(&other, &nonce, "a", 3, token.as_ref(), NOW), None);
        assert_eq!(format!("{:?}", key()), "NonceKey(..)");
    }
}
