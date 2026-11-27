//! HMAC (RFC 2104) over the digests the reference uses, plus lowercase hex.

use md5::Md5;
use sha2::{Digest, Sha256, Sha384};

// RFC 2104 block sizes: MD5 and SHA-256 use 64-byte blocks, SHA-384 uses 128.
const MD5_BLOCK: usize = 64;
const SHA384_BLOCK: usize = 128;

/// HMAC-MD5, as the reference's keyed hash for nonces.
pub(crate) fn hmac_md5(key: &[u8], message: &[u8]) -> Vec<u8> {
    hmac::<Md5>(MD5_BLOCK, key, message)
}

/// HMAC-SHA384, as the reference's password pre-hash.
pub(crate) fn hmac_sha384(key: &[u8], message: &[u8]) -> Vec<u8> {
    hmac::<Sha384>(SHA384_BLOCK, key, message)
}

/// Lowercase hex sha256 of `bytes`, the stored form of a session token.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Constant-time byte comparison, so verification time does not leak a matching prefix.
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// Keys longer than a block are hashed first, then zero-padded (RFC 2104 section 2).
fn hmac<D: Digest>(block: usize, key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut padded = if key.len() > block {
        D::digest(key).to_vec()
    } else {
        key.to_vec()
    };
    padded.resize(block, 0);
    let inner_pad: Vec<u8> = padded.iter().map(|byte| byte ^ 0x36).collect();
    let outer_pad: Vec<u8> = padded.iter().map(|byte| byte ^ 0x5c).collect();
    let mut inner = D::new();
    inner.update(&inner_pad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = D::new();
    outer.update(&outer_pad);
    outer.update(&inner);
    outer.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::{hex, hmac_md5, hmac_sha384, same, sha256_hex};

    // RFC 2202 section 2 test cases 1, 2 and 6 (HMAC-MD5).
    #[test]
    fn hmac_md5_matches_rfc_2202() {
        assert_eq!(
            hex(&hmac_md5(&[0x0b; 16], b"Hi There")),
            "9294727a3638bb1c13f48ef8158bfc9d"
        );
        assert_eq!(
            hex(&hmac_md5(b"Jefe", b"what do ya want for nothing?")),
            "750c783e6ab0b503eaa86e310a5db738"
        );
        assert_eq!(
            hex(&hmac_md5(
                &[0xaa; 80],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "6b1ab7fe4bd7bf8f0b62e6ce61b9d0cd"
        );
    }

    // RFC 4231 section 4 test cases 1, 2 and 6 (HMAC-SHA-384).
    #[test]
    fn hmac_sha384_matches_rfc_4231() {
        assert_eq!(
            hex(&hmac_sha384(&[0x0b; 20], b"Hi There")),
            "afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6"
        );
        assert_eq!(
            hex(&hmac_sha384(b"Jefe", b"what do ya want for nothing?")),
            "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649"
        );
        assert_eq!(
            hex(&hmac_sha384(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "4ece084485813e9088d2c63a041bc5b44f9ef1012a2b588f3cd11f05033ac4c60c2ef6ab4030fe8296248df163f44952"
        );
    }

    #[test]
    fn sha256_hex_and_comparison() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hex(&[0x00, 0xff, 0x1a]), "00ff1a");
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
    }
}
