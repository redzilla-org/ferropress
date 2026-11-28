//! `application/x-www-form-urlencoded` bodies and HTML escaping for the login page.

/// The value of the first `name` field, percent- and plus-decoded; `None` when absent.
#[must_use]
pub(crate) fn field(body: &[u8], name: &str) -> Option<Vec<u8>> {
    body.split(|&b| b == b'&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, |&b| b == b'=');
            Some((parts.next()?, parts.next().unwrap_or_default()))
        })
        .find(|(key, _)| decode(key) == name.as_bytes())
        .map(|(_, value)| decode(value))
}

// `+` is a space; `%XX` is a byte; a malformed escape is kept literally, as PHP does.
fn decode(bytes: &[u8]) -> Vec<u8> {
    let hex = |b: u8| {
        char::from(b)
            .to_digit(16)
            .and_then(|d| u8::try_from(d).ok())
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match (
            b,
            bytes.get(i + 1).copied().and_then(hex),
            bytes.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(high), Some(low)) => {
                out.push(high << 4 | low);
                i += 3;
                continue;
            }
            (b'+', ..) => out.push(b' '),
            _ => out.push(b),
        }
        i += 1;
    }
    out
}

/// `text` safe inside HTML text and double-quoted attributes.
#[must_use]
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#039;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{escape, field};

    #[test]
    fn fields_decode_plus_percent_and_keep_bad_escapes() {
        let body = b"log=a+b%40c.test&pwd=p%26w%3D&x&bad=%zz%4&log=second";
        assert_eq!(field(body, "log"), Some(b"a b@c.test".to_vec()));
        assert_eq!(field(body, "pwd"), Some(b"p&w=".to_vec()));
        assert_eq!(field(body, "x"), Some(Vec::new()));
        assert_eq!(field(body, "bad"), Some(b"%zz%4".to_vec()));
        assert_eq!(field(body, "missing"), None);
        assert_eq!(field(b"", "log"), None);
    }

    #[test]
    fn escape_covers_markup_and_quotes() {
        assert_eq!(
            escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#039;&amp;&#039;&lt;/a&gt;"
        );
    }
}
