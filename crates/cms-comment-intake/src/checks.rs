//! Text predicates behind the moderation and disallowed-list rules.

/// Links the reference counts in comment text: every `//`, optionally preceded by `http:` /
/// `https:` and an `href=` attribute, counted without overlap.
#[must_use]
pub(crate) fn link_count(content: &str) -> u64 {
    content.matches("//").count() as u64
}

/// Whether any non-empty trimmed line of `keys` occurs, ignoring case, in one of `fields`.
#[must_use]
pub(crate) fn matches_keys(keys: &str, fields: &[&str]) -> bool {
    let lowered: Vec<String> = fields.iter().map(|field| field.to_lowercase()).collect();
    keys.split('\n')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .any(|word| {
            let word = word.to_lowercase();
            lowered.iter().any(|field| field.contains(&word))
        })
}

/// Whether `text` hides a plain-ASCII character (other than `&`) behind a decimal entity,
/// a trick the reference treats as disallowed.
#[must_use]
pub(crate) fn has_encoded_ascii(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find("&#") {
        rest = &rest[start + 2..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && rest.as_bytes().get(digits) == Some(&b';') {
            // Long digit runs exceed every ASCII code point.
            let code = rest[..digits].parse::<u32>().unwrap_or(u32::MAX);
            if code < 128 && code != 38 {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{has_encoded_ascii, link_count, matches_keys};

    #[test]
    fn links_are_counted_per_double_slash() {
        let cases = [
            ("no links", 0),
            ("see https://a.example and http://b.example", 2),
            ("<a href=\"//c.example\">c</a>", 1),
            ("///", 1),
            ("////", 2),
        ];
        for (text, count) in cases {
            assert_eq!(link_count(text), count, "{text}");
        }
    }

    #[test]
    fn keys_match_case_insensitively_on_trimmed_lines() {
        assert!(matches_keys("casino\n", &["", "Best CASINO here"]));
        assert!(matches_keys("  \n 203.0.113 \n", &["203.0.113.9"]));
        assert!(matches_keys("\u{c9}T\u{c9}", &["un \u{e9}t\u{e9}"]));
        assert!(!matches_keys("", &["anything"]));
        assert!(!matches_keys("\n  \n", &["anything"]));
        assert!(!matches_keys("loan", &["hello", "world"]));
    }

    #[test]
    fn encoded_ascii_is_found_but_ampersands_and_high_code_points_are_not() {
        assert!(has_encoded_ascii("v&#105;agra"));
        assert!(has_encoded_ascii("x &#0; y"));
        assert!(!has_encoded_ascii("fish &#38; chips"));
        assert!(!has_encoded_ascii("caf&#233;"));
        assert!(!has_encoded_ascii("&#99999999999999999999;"));
        assert!(!has_encoded_ascii("&#; &#12 &#x41;"));
        assert!(!has_encoded_ascii("plain"));
    }
}
