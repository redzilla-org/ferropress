//! Field sanitizing the reference applies before a comment is stored.

/// Elements a comment may keep, each with the attributes it may keep (the reference's
/// comment allowlist).
const ALLOWED: &[(&str, &[&str])] = &[
    ("a", &["href", "title"]),
    ("abbr", &["title"]),
    ("acronym", &["title"]),
    ("b", &[]),
    ("blockquote", &["cite"]),
    ("cite", &[]),
    ("code", &[]),
    ("del", &["datetime"]),
    ("em", &[]),
    ("i", &[]),
    ("q", &["cite"]),
    ("s", &[]),
    ("strike", &[]),
    ("strong", &[]),
];

/// URL schemes kept in `href`, `cite` and the author URL.
const PROTOCOLS: &[&str] = &[
    "http", "https", "ftp", "ftps", "mailto", "news", "irc", "irc6", "ircs", "gopher", "nntp",
    "feed", "telnet", "mms", "rtsp", "sms", "svn", "tel", "fax", "xmpp", "webcal", "urn",
];

/// Comment text reduced to the allowlist: other elements and HTML comments are removed with
/// their markup only (their text stays), kept elements keep only allowed attributes, a stray
/// `>` becomes `&gt;`, and a bare `&` becomes `&amp;`.
#[must_use]
pub(crate) fn filter_content(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(index) = rest.find(['<', '>']) {
        push_text(&mut out, &rest[..index]);
        let tail = &rest[index..];
        if tail.starts_with('>') {
            out.push_str("&gt;");
            rest = &tail[1..];
            continue;
        }
        if let Some(after) = tail.strip_prefix("<!--") {
            // An HTML comment runs to `-->` or the end, and is dropped.
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        // A tag runs to the next `>` or the end of the text.
        let end = tail.find('>').map_or(tail.len(), |end| end + 1);
        out.push_str(&element(&tail[..end]));
        rest = &tail[end..];
    }
    push_text(&mut out, rest);
    out
}

/// Every tag removed, `script` and `style` elements with their content.
#[must_use]
pub(crate) fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('<') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let lower = tail.get(..7).map(str::to_ascii_lowercase);
        let closer = match lower.as_deref() {
            Some(head) if head.starts_with("<script") => Some("</script>"),
            Some(head) if head.starts_with("<style") => Some("</style>"),
            _ => None,
        };
        rest = match closer {
            Some(closer) => {
                find_ignore_case(tail, closer).map_or("", |at| &tail[at + closer.len()..])
            }
            None => tail.find('>').map_or("", |end| &tail[end + 1..]),
        };
    }
    out.push_str(rest);
    out
}

/// The author name as plain text: tags stripped, whitespace runs collapsed, trimmed, then
/// `&`, `<` and `>` escaped (existing entities are kept).
#[must_use]
pub(crate) fn sanitize_author(author: &str) -> String {
    let plain = strip_tags(author);
    let collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    push_text(&mut out, &collapsed);
    out
}

/// The author URL limited to the allowed schemes. A URL without a scheme gets `http://`;
/// characters a URL cannot hold are removed; anything else is the empty string.
#[must_use]
pub(crate) fn sanitize_url(url: &str) -> String {
    let plain = strip_tags(url);
    let kept: String = plain.trim().chars().filter(|&c| is_url_char(c)).collect();
    if kept.is_empty() {
        return kept;
    }
    match scheme(&kept) {
        Some(scheme) if is_allowed_scheme(scheme) => kept,
        Some(_) => String::new(),
        // Relative references stay; a bare host gets the reference's default scheme.
        None if kept.starts_with(['/', '#', '?']) => kept,
        None => format!("http://{kept}"),
    }
}

/// The client address with every byte outside hex digits, `:`, `.`, `,` and space removed.
#[must_use]
pub(crate) fn sanitize_ip(ip: &str) -> String {
    ip.chars()
        .filter(|&c| c.is_ascii_hexdigit() || matches!(c, ':' | '.' | ',' | ' '))
        .collect()
}

/// `text` with `&`, `<`, `>`, `"` and `'` escaped for an HTML attribute or text node.
#[must_use]
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#039;"),
            other => out.push(other),
        }
    }
    out
}

// Text between tags: `&` is escaped unless it starts a well-formed entity; `<` is escaped.
fn push_text(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(index) = rest.find(['&', '<', '>']) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        match tail.as_bytes()[0] {
            b'<' => out.push_str("&lt;"),
            b'>' => out.push_str("&gt;"),
            _ if entity_len(tail).is_some() => out.push('&'),
            _ => out.push_str("&amp;"),
        }
        rest = &tail[1..];
    }
    out.push_str(rest);
}

// Length of `&name;`, `&#123;` or `&#x1f;` at the start of `text`.
fn entity_len(text: &str) -> Option<usize> {
    let body = text.strip_prefix('&')?;
    let end = body.find(';')?;
    let name = &body[..end];
    let valid = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        !hex.is_empty() && hex.len() <= 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())
    } else if let Some(digits) = name.strip_prefix('#') {
        !digits.is_empty() && digits.len() <= 7 && digits.bytes().all(|b| b.is_ascii_digit())
    } else {
        !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric())
    };
    valid.then_some(end + 2)
}

// One `<…>` chunk: rebuilt when its element is allowed, otherwise removed.
fn element(tag: &str) -> String {
    let inner = tag.trim_start_matches('<').trim_end_matches('>');
    let inner = inner.trim_start();
    let (closing, inner) = match inner.strip_prefix('/') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, inner),
    };
    let name_len = inner
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-')
        .count();
    if name_len == 0 {
        return String::new();
    }
    let (name, attributes) = inner.split_at(name_len);
    let lower = name.to_ascii_lowercase();
    let Some((_, allowed)) = ALLOWED.iter().find(|(element, _)| *element == lower) else {
        return String::new();
    };
    if closing {
        return format!("</{name}>");
    }
    let mut out = format!("<{name}");
    for (attribute, value) in parse_attributes(attributes) {
        if !allowed.contains(&attribute.as_str()) {
            continue;
        }
        match value {
            None => {
                out.push(' ');
                out.push_str(&attribute);
            }
            Some(value) => {
                let is_link = attribute == "href" || attribute == "cite";
                if is_link && scheme(&value).is_some_and(|s| !is_allowed_scheme(s)) {
                    continue;
                }
                out.push(' ');
                out.push_str(&attribute);
                out.push_str("=\"");
                out.push_str(&value.replace('"', "&quot;"));
                out.push('"');
            }
        }
    }
    if attributes.trim_end().ends_with('/') {
        out.push_str(" /");
    }
    out.push('>');
    out
}

// `name="v"`, `name='v'`, `name=v` and bare `name`, with names lower-cased.
fn parse_attributes(text: &str) -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let name_len = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_'))
            .count();
        if name_len == 0 {
            return found;
        }
        let name = rest[..name_len].to_ascii_lowercase();
        rest = rest[name_len..].trim_start();
        let Some(after) = rest.strip_prefix('=') else {
            found.push((name, None));
            continue;
        };
        let after = after.trim_start();
        let (value, remaining) = match after.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let body = &after[1..];
                let end = body.find(quote).unwrap_or(body.len());
                (&body[..end], body.get(end + 1..).unwrap_or(""))
            }
            _ => {
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        found.push((name, Some(value.to_owned())));
        rest = remaining;
    }
}

// The scheme of `url` when a `:` comes before any `/`, `?` or `#`.
fn scheme(url: &str) -> Option<&str> {
    let colon = url.find(':')?;
    let candidate = &url[..colon];
    let plain = !candidate.is_empty()
        && candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    plain.then_some(candidate)
}

fn is_allowed_scheme(scheme: &str) -> bool {
    let lower = scheme.to_ascii_lowercase();
    PROTOCOLS.contains(&lower.as_str())
}

fn is_url_char(c: char) -> bool {
    !c.is_ascii() || c.is_ascii_alphanumeric() || "-~+_.?#=!&;,/:%@$|*'()[]".contains(c)
}

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{
        escape_html, filter_content, sanitize_author, sanitize_ip, sanitize_url, strip_tags,
    };

    #[test]
    fn comment_markup_keeps_only_the_allowlist() {
        let cases = [
            ("plain text", "plain text"),
            ("<b>bold</b> <i>it</i>", "<b>bold</b> <i>it</i>"),
            ("<script>alert(1)</script>", "alert(1)"),
            ("<p onclick=\"x\">para</p>", "para"),
            (
                "<a href=\"https://a.example\" onclick=\"x\" title='T'>a</a>",
                "<a href=\"https://a.example\" title=\"T\">a</a>",
            ),
            ("<a href=\"javascript:alert(1)\">x</a>", "<a>x</a>"),
            ("<a href=/relative>x</a>", "<a href=\"/relative\">x</a>"),
            (
                "<A HREF=\"http://a.example\">x</A>",
                "<A href=\"http://a.example\">x</A>",
            ),
            ("<abbr title>t</abbr>", "<abbr title>t</abbr>"),
            ("<b />", "<b />"),
            (
                "<blockquote cite='x\"y'>q</blockquote>",
                "<blockquote cite=\"x&quot;y\">q</blockquote>",
            ),
            ("a <!-- hidden --> b", "a  b"),
            ("a <!-- open", "a "),
            ("1 > 0", "1 &gt; 0"),
            ("1 <3 you", "1 "),
            ("< b>x", "<b>x"),
            ("<>", ""),
            (
                "fish & chips &amp; &#38; &#x26; &bogus",
                "fish &amp; chips &amp; &#38; &#x26; &amp;bogus",
            ),
            ("unclosed <a href=\"x", "unclosed <a href=\"x\">"),
        ];
        for (input, expected) in cases {
            assert_eq!(filter_content(input), expected, "{input}");
        }
    }

    #[test]
    fn tags_are_stripped_with_script_and_style_bodies() {
        assert_eq!(strip_tags("a<b>b</b>c"), "abc");
        assert_eq!(strip_tags("x<SCRIPT>evil()</script>y"), "xy");
        assert_eq!(strip_tags("x<style>p{}</style>y"), "xy");
        assert_eq!(strip_tags("x<script>never closed"), "x");
        assert_eq!(strip_tags("x<b"), "x");
        assert_eq!(strip_tags("no tags"), "no tags");
    }

    #[test]
    fn authors_become_escaped_single_line_text() {
        assert_eq!(sanitize_author("  <b>Ada</b>\n\tLovelace "), "Ada Lovelace");
        assert_eq!(
            sanitize_author("Tom & Jerry &amp; co"),
            "Tom &amp; Jerry &amp; co"
        );
        assert_eq!(sanitize_author("a > b"), "a &gt; b");
    }

    #[test]
    fn urls_keep_allowed_schemes_and_gain_http() {
        let cases = [
            ("https://ada.example/x?y=1", "https://ada.example/x?y=1"),
            ("ada.example", "http://ada.example"),
            ("  ada.example/a b ", "http://ada.example/ab"),
            ("javascript:alert(1)", ""),
            ("/local", "/local"),
            ("", ""),
            ("<b></b>", ""),
            ("MAILTO:a@example.com", "MAILTO:a@example.com"),
            ("caf\u{e9}.example", "http://caf\u{e9}.example"),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_url(input), expected, "{input}");
        }
    }

    #[test]
    fn addresses_and_html_are_cleaned() {
        assert_eq!(sanitize_ip("203.0.113.9"), "203.0.113.9");
        assert_eq!(sanitize_ip("2001:db8::1<x>"), "2001:db8::1");
        assert_eq!(
            escape_html("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#039;&amp;&#039;&lt;/a&gt;"
        );
    }
}
