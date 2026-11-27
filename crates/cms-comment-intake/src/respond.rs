//! Where the browser goes after an accepted comment, and the commenter cookies it gets.

use sha2::{Digest as _, Sha256};

use crate::decide::{Approval, Candidate};

/// Seconds the commenter cookies live (the reference's lifetime).
pub const COOKIE_SECONDS: u64 = 30_000_000;

/// Inputs for the post-submit redirect.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// The form's `redirect_to`, when non-empty.
    pub redirect_to: Option<&'a str>,
    /// The post's permalink.
    pub permalink: &'a str,
    /// The site's `home` URL, also the fallback for off-site targets.
    pub home: &'a str,
    /// The site's nonce salt, which keys the moderation hash.
    pub salt: &'a [u8],
}

/// The `Location` for comment `id`: `redirect_to` or the permalink, plus `#comment-<id>`; a
/// held comment with an email also gets `unapproved` and `moderation-hash` query arguments
/// so its author can see it. A target on another host falls back to the home URL.
#[must_use]
pub fn location(id: u64, candidate: &Candidate, target: &Target<'_>) -> String {
    let base = target
        .redirect_to
        .filter(|to| !to.is_empty())
        .unwrap_or(target.permalink);
    let mut url = format!("{base}#comment-{id}");
    if candidate.approval == Approval::Hold && !candidate.email.is_empty() {
        let hash = moderation_hash(target.salt, &candidate.date_gmt);
        url = add_query(&url, &format!("unapproved={id}&moderation-hash={hash}"));
    }
    // Header safety first: control bytes could split the response.
    let url: String = url.chars().filter(|c| !c.is_control()).collect();
    if same_site(&url, target.home) {
        url
    } else {
        target.home.to_owned()
    }
}

/// `Set-Cookie` values for an anonymous commenter: the name, email and URL cookies when
/// `consent` was given, or expiring them when it was not. Logged-in users get none.
#[must_use]
pub fn cookies(candidate: &Candidate, consent: bool, site_url: &str, path: &str) -> Vec<String> {
    if candidate.user_id != 0 {
        return Vec::new();
    }
    let hash = cookie_hash(site_url);
    let secure = if site_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let pairs = [
        ("comment_author_", &*candidate.author),
        ("comment_author_email_", &*candidate.email),
        ("comment_author_url_", &*candidate.url),
    ];
    pairs
        .iter()
        .map(|(stem, value)| {
            let (value, age) = if consent {
                (percent_encode(value), COOKIE_SECONDS)
            } else {
                (String::new(), 0)
            };
            format!("{stem}{hash}={value}; Max-Age={age}; Path={path}; SameSite=Lax{secure}")
        })
        .collect()
}

/// Suffix of the commenter cookie names: the first 32 hex digits of SHA-256 of the site URL.
#[must_use]
pub fn cookie_hash(site_url: &str) -> String {
    hex(&Sha256::digest(site_url.as_bytes()))[..32].to_owned()
}

/// HMAC-SHA-256 of `date_gmt` under `salt`, as 32 hex digits.
#[must_use]
pub fn moderation_hash(salt: &[u8], date_gmt: &str) -> String {
    // RFC 2104 over SHA-256 (64-byte blocks); long keys are hashed first.
    let mut key = [0_u8; 64];
    if salt.len() > 64 {
        key[..32].copy_from_slice(&Sha256::digest(salt));
    } else {
        key[..salt.len()].copy_from_slice(salt);
    }
    let pad = |byte: u8| key.map(|k| k ^ byte);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(date_gmt.as_bytes())
        .finalize();
    let outer = Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize();
    hex(&outer)[..32].to_owned()
}

// The query goes before the fragment, joined with `&` when a query already exists.
fn add_query(url: &str, query: &str) -> String {
    let (head, fragment) = url
        .split_once('#')
        .map_or((url, None), |(h, f)| (h, Some(f)));
    let joiner = if head.contains('?') { '&' } else { '?' };
    match fragment {
        Some(fragment) => format!("{head}{joiner}{query}#{fragment}"),
        None => format!("{head}{joiner}{query}"),
    }
}

// Relative targets stay on the site; absolute ones must be http(s) on the home host.
fn same_site(url: &str, home: &str) -> bool {
    if url.starts_with("//") {
        return host(&url[2..]) == host_of(home);
    }
    match url.split_once("://") {
        // A colon before any path, query or fragment would be a scheme.
        None => match (url.find(':'), url.find(['/', '?', '#'])) {
            (None, _) => true,
            (Some(colon), Some(separator)) => colon > separator,
            (Some(_), None) => false,
        },
        Some((scheme, rest)) => {
            matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
                && host(rest) == host_of(home)
        }
    }
}

fn host_of(url: &str) -> String {
    url.split_once("://")
        .map_or_else(String::new, |(_, rest)| host(rest))
}

// Host without user info or port, lower-cased.
fn host(rest: &str) -> String {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = authority.split(':').next().unwrap_or(authority);
    host.to_ascii_lowercase()
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::{add_query, cookie_hash, moderation_hash, same_site};

    #[test]
    fn hmac_matches_the_rfc_4231_vector() {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?".
        assert_eq!(
            moderation_hash(b"Jefe", "what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c7"
        );
        // Test case 6: a 131-byte key is hashed first.
        assert_eq!(
            moderation_hash(
                &[0xaa; 131],
                "Test Using Larger Than Block-Size Key - Hash Key First"
            ),
            "60e431591ee0b67f0d8a26aacbf5b77f"
        );
        assert_eq!(cookie_hash("https://example.com").len(), 32);
    }

    #[test]
    fn queries_go_before_the_fragment() {
        assert_eq!(add_query("/p/#c", "a=1"), "/p/?a=1#c");
        assert_eq!(add_query("/p/?x=2#c", "a=1"), "/p/?x=2&a=1#c");
        assert_eq!(add_query("/p/", "a=1"), "/p/?a=1");
    }

    #[test]
    fn only_home_host_targets_are_same_site() {
        let home = "https://example.com/blog";
        assert!(same_site("https://example.com/blog/x/", home));
        assert!(same_site("http://EXAMPLE.com:8080/x", home));
        assert!(same_site("/blog/x/", home));
        assert!(same_site("x/y#comment-1", home));
        assert!(same_site("//example.com/x", home));
        assert!(!same_site("//evil.example/x", home));
        assert!(!same_site("https://evil.example/x", home));
        assert!(!same_site("https://example.com@evil.example/", home));
        assert!(!same_site("javascript:alert(1)", home));
        assert!(!same_site("ftp://example.com/", home));
    }
}
