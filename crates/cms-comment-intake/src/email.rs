//! The reference's deliberately loose email syntax check.

/// Whether `email` passes the reference's syntax check: at least 6 bytes, a local part of
/// RFC 5322 atom characters and dots, and a domain of two or more dot-separated labels of
/// letters, digits and inner hyphens.
#[must_use]
pub(crate) fn is_email(email: &str) -> bool {
    if email.len() < 6 {
        return false;
    }
    // The `@` must not be the first byte; the split is at the first `@` after it.
    let Some(at) = email.as_bytes()[1..]
        .iter()
        .position(|&byte| byte == b'@')
        .map(|index| index + 1)
    else {
        return false;
    };
    let (local, domain) = (&email[..at], &email[at + 1..]);
    if !local.bytes().all(is_local_byte) {
        return false;
    }
    if domain.contains("..") || domain.trim_matches(is_edge_space_or_dot) != domain {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

const fn is_local_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'!' | b'#'..=b'\'' | b'*' | b'+' | b'/' | b'=' | b'?' | b'^'..=b'`' | b'{'..=b'~' | b'.' | b'-')
}

fn is_edge_space_or_dot(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\0' | '\x0b' | '.')
}

#[cfg(test)]
mod tests {
    use super::is_email;

    #[test]
    fn the_reference_accepts_and_refuses_these() {
        let accepted = [
            "ada@example.com",
            "a.b+c@sub.example.org",
            "o'neil@example.co",
            "x{y}@a-b.example",
            "ab@c.de",
        ];
        for email in accepted {
            assert!(is_email(email), "{email}");
        }
        let refused = [
            "a@b.c",
            "@example.com",
            "adaexample.com",
            "ada(x)@example.com",
            "ada@example..com",
            "ada@.example.com",
            "ada@example.com.",
            "ada@localhost",
            "ada@-example.com",
            "ada@example-.com",
            "ada@exa_mple.com",
            "ada@ex ample.com",
            "ada@example.com@x",
            "\u{e9}da@example.com",
        ];
        for email in refused {
            assert!(!is_email(email), "{email}");
        }
    }
}
