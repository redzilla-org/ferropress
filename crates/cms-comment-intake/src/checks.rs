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

#[cfg(test)]
mod tests {
    use super::{link_count, matches_keys};

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
}
