//! The legacy portable hash: iterated MD5 over salt and password, `$P$` or `$H$`.
//!
//! Layout (34 bytes): 3-byte id, one count character (log2 of the iteration count, 7..=30),
//! 8 salt characters, then 22 characters encoding the 16-byte digest.

use md5::{Digest, Md5};

use crate::mac::same;

/// The 64-character alphabet of the portable form.
const ALPHABET: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// Longer passwords are rejected before hashing, bounding the CPU cost of a guess.
const MAX_PASSWORD: usize = 4096;
const SETTING: usize = 12;
const LENGTH: usize = 34;

/// Whether `hash` names the portable scheme.
pub(crate) fn is_portable(hash: &[u8]) -> bool {
    hash.starts_with(b"$P$") || hash.starts_with(b"$H$")
}

/// Whether `password` hashes to `stored` under the portable scheme.
pub(crate) fn verify(password: &[u8], stored: &[u8]) -> bool {
    if password.len() > MAX_PASSWORD || stored.len() != LENGTH {
        return false;
    }
    compute(password, &stored[..SETTING]).is_some_and(|hash| same(&hash, stored))
}

// None for a malformed setting; the reference's failure marker never matches a stored hash.
fn compute(password: &[u8], setting: &[u8]) -> Option<Vec<u8>> {
    if !is_portable(setting) {
        return None;
    }
    let log2 = ALPHABET.iter().position(|&c| Some(&c) == setting.get(3))?;
    if !(7..=30).contains(&log2) {
        return None;
    }
    let salt = &setting[4..SETTING];
    let mut digest = Md5::new().chain_update(salt).chain_update(password).finalize();
    for _ in 0..(1_u64 << log2) {
        digest = Md5::new()
            .chain_update(digest)
            .chain_update(password)
            .finalize();
    }
    let mut out = setting.to_vec();
    encode(&digest, &mut out);
    Some(out)
}

// Little-endian 6-bit groups over 3-byte chunks; a short tail emits only the groups it fills.
fn encode(input: &[u8], out: &mut Vec<u8>) {
    for chunk in input.chunks(3) {
        let mut value = 0_u32;
        for (shift, &byte) in chunk.iter().enumerate() {
            value |= u32::from(byte) << (8 * shift);
        }
        for group in 0..=chunk.len() {
            out.push(ALPHABET[((value >> (6 * group)) & 0x3f) as usize]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{compute, encode, is_portable, verify};

    // The classic phpass known answer: test.php in the phpass 0.3 distribution
    // (openwall.com/phpass) checks "test12345" against this portable hash and expects
    // "test12346" to fail.
    const VECTOR: &[u8] = b"$P$9IQRaTwmfeRo7ud9Fh4E2PdI0S3r.L0";

    #[test]
    fn the_published_vector_verifies_and_a_near_miss_fails() {
        assert!(verify(b"test12345", VECTOR));
        assert!(!verify(b"test12346", VECTOR));
        // The id does not enter the digest, so the `$H$` spelling of the same hash verifies.
        assert!(verify(b"test12345", b"$H$9IQRaTwmfeRo7ud9Fh4E2PdI0S3r.L0"));
    }

    #[test]
    fn malformed_settings_and_oversized_passwords_fail() {
        assert!(is_portable(b"$P$x") && is_portable(b"$H$x") && !is_portable(b"$2y$"));
        assert!(!verify(b"test12345", &VECTOR[..33]));
        assert!(!verify(&[b'a'; 4097], VECTOR));
        // Count characters outside 7..=30 ('5' is 7, so '4' is 6; 'T' is 30, so 'U' is 31).
        assert!(compute(b"x", b"$P$4IQRaTwmf").is_none());
        assert!(compute(b"x", b"$P$UIQRaTwmf").is_none());
        assert!(compute(b"x", b"$P$*IQRaTwmf").is_none());
        assert!(compute(b"x", b"$X$9IQRaTwmf").is_none());
        assert!(compute(b"x", b"$P$5IQRaTwmf").is_some());
    }

    #[test]
    fn encoding_emits_one_more_character_than_bytes_per_chunk() {
        let mut out = Vec::new();
        encode(&[0xff], &mut out);
        assert_eq!(out, b"z1");
        out.clear();
        encode(&[0; 16], &mut out);
        assert_eq!(out.len(), 22);
    }
}
