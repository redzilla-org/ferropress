//! XML entity decoding for text and attribute values.

/// The character an entity reference names: the five predefined XML entities and numeric
/// references. Anything else is `None` so the caller can keep it verbatim.
pub(crate) fn entity(name: &str) -> Option<char> {
    match name {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        _ => {
            // Numeric references: decimal `#65` or hexadecimal `#x41`.
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse::<u32>().ok()?,
            };
            char::from_u32(code).filter(|&ch| ch != '\0')
        }
    }
}

/// Appends the decoded form of the reference `name` to `out`, or `&name;` when it is unknown,
/// so HTML entity names in exported content survive unchanged.
pub(crate) fn push_entity(out: &mut String, name: &str) {
    if let Some(ch) = entity(name) {
        out.push(ch);
    } else {
        out.push('&');
        out.push_str(name);
        out.push(';');
    }
}

/// `raw` (an attribute value) with every entity reference decoded.
pub(crate) fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        // A bare ampersand without a terminator is kept as text.
        let Some(end) = after.find(';') else {
            out.push('&');
            rest = after;
            continue;
        };
        push_entity(&mut out, &after[..end]);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{entity, push_entity, unescape};

    #[test]
    fn predefined_and_numeric_references_decode() {
        assert_eq!(entity("lt"), Some('<'));
        assert_eq!(entity("gt"), Some('>'));
        assert_eq!(entity("amp"), Some('&'));
        assert_eq!(entity("apos"), Some('\''));
        assert_eq!(entity("quot"), Some('"'));
        assert_eq!(entity("#65"), Some('A'));
        assert_eq!(entity("#x41"), Some('A'));
        assert_eq!(entity("#X41"), Some('A'));
        assert_eq!(entity("#0"), None);
        assert_eq!(entity("#xZZ"), None);
        assert_eq!(entity("#1114112"), None);
        assert_eq!(entity("nbsp"), None);
    }

    #[test]
    fn unknown_references_are_kept_verbatim() {
        let mut out = String::new();
        push_entity(&mut out, "nbsp");
        push_entity(&mut out, "amp");
        assert_eq!(out, "&nbsp;&");
        assert_eq!(unescape("a &amp; b"), "a & b");
        assert_eq!(unescape("x&lt;y&gt;&#33;"), "x<y>!");
        assert_eq!(unescape("tom & jerry"), "tom & jerry");
        assert_eq!(unescape("&copy;"), "&copy;");
        assert_eq!(unescape("plain"), "plain");
    }

    proptest::proptest! {
        #[test]
        fn text_without_ampersands_is_unchanged(text in "[^&]{0,40}") {
            proptest::prop_assert_eq!(unescape(&text), text);
        }
    }
}
