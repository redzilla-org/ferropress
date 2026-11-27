//! Escaping for HTML text and attributes, XML, CDATA and JSON strings.

/// Appends `text` with `& < > " '` escaped, safe in HTML text, attributes and XML.
pub fn push_escaped(out: &mut String, text: &str) {
    // Copy unescaped runs in one go; only the five specials are rewritten.
    let mut start = 0;
    for (at, byte) in text.bytes().enumerate() {
        let entity = match byte {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\'' => "&#039;",
            _ => continue,
        };
        out.push_str(&text[start..at]);
        out.push_str(entity);
        start = at + 1;
    }
    out.push_str(&text[start..]);
}

/// `text` escaped for HTML or XML.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    push_escaped(&mut out, text);
    out
}

/// Appends `text` as one or more CDATA sections; a `]]>` inside is split across two sections.
pub fn push_cdata(out: &mut String, text: &str) {
    out.push_str("<![CDATA[");
    out.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
    out.push_str("]]>");
}

/// Appends `text` as a quoted JSON string. `<`, `>` and `&` are written as `\u` escapes so the
/// value can never close a `<script>` element it is embedded in.
pub fn push_json(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' | '>' | '&' | '\u{0}'..='\u{1f}' | '\u{2028}' | '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            _ => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::{escape, push_cdata, push_json};

    #[test]
    fn html_specials_are_escaped() {
        assert_eq!(
            escape(r#"a & <b> "c" 'd'"#),
            "a &amp; &lt;b&gt; &quot;c&quot; &#039;d&#039;"
        );
        assert_eq!(escape("plain é"), "plain é");
        assert_eq!(escape(""), "");
    }

    #[test]
    fn cdata_splits_its_terminator() {
        let mut out = String::new();
        push_cdata(&mut out, "a]]>b");
        assert_eq!(out, "<![CDATA[a]]]]><![CDATA[>b]]>");
    }

    #[test]
    fn json_strings_cannot_close_a_script() {
        let mut out = String::new();
        push_json(&mut out, "a\"\\\n\r\t</script>&\u{1}\u{2028}é");
        assert_eq!(
            out,
            "\"a\\\"\\\\\\n\\r\\t\\u003c/script\\u003e\\u0026\\u0001\\u2028é\""
        );
    }
}
