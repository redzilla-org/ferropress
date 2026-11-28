//! Content formatting: block delimiters, paragraph auto-formatting, plain text and excerpts.
//!
//! Studied behavior, implemented independently: block-serialized content has its delimiter
//! comments removed and is not auto-paragraphed; classic content is split into paragraphs on
//! blank lines. Dynamic blocks (self-closing delimiters) render nothing in this theme.

use crate::Post;
use crate::cards::strip_markers;

/// Words in a generated excerpt, as the reference's default `excerpt_length`.
pub const EXCERPT_WORDS: usize = 55;

// Chunks starting with one of these tags are already block markup and are not wrapped in `<p>`.
const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
    "style",
    "script",
    "iframe",
];

/// `post_content` as served HTML: block delimiters stripped, or classic content auto-paragraphed.
#[must_use]
pub fn format_content(raw: &str) -> String {
    let raw = raw.replace("\r\n", "\n");
    if raw.contains("<!-- wp:") {
        strip_block_delimiters(&raw)
    } else {
        auto_paragraphs(&raw)
    }
}

fn strip_block_delimiters(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find("<!--") {
        let after = rest[at + 4..].trim_start_matches(' ');
        let delimiter = after.starts_with("wp:") || after.starts_with("/wp:");
        let Some(end) = rest[at..].find("-->") else {
            break;
        };
        let end = at + end + 3;
        if delimiter {
            out.push_str(&rest[..at]);
            // The newline that ends a delimiter line goes with it.
            rest = rest[end..].strip_prefix('\n').unwrap_or(&rest[end..]);
        } else {
            out.push_str(&rest[..end]);
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

fn auto_paragraphs(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 64);
    for chunk in raw.split("\n\n").map(str::trim).filter(|c| !c.is_empty()) {
        if !out.is_empty() {
            out.push('\n');
        }
        if starts_with_block(chunk) {
            out.push_str(chunk);
        } else {
            out.push_str("<p>");
            out.push_str(&chunk.replace('\n', "<br />\n"));
            out.push_str("</p>");
        }
    }
    out
}

fn starts_with_block(chunk: &str) -> bool {
    if chunk.starts_with("<!--") {
        return true;
    }
    chunk
        .strip_prefix('<')
        .is_some_and(|tag| BLOCK_TAGS.contains(&tag_name(tag).as_str()))
}

// Whether the tag at the start of `markup` separates words in its visible text.
fn breaks_words(markup: &str) -> bool {
    let tag = markup.trim_start_matches('<').trim_start_matches('/');
    let name = tag_name(tag);
    name.is_empty() || name == "br" || BLOCK_TAGS.contains(&name.as_str())
}

fn tag_name(tag: &str) -> String {
    tag.chars()
        .take_while(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Visible text of an HTML fragment: tags, comments, script and style removed, common entities
/// decoded, whitespace collapsed.
#[must_use]
pub fn plain_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        text.push_str(&rest[..at]);
        rest = &rest[at..];
        let lower = rest.get(..7).unwrap_or(rest).to_ascii_lowercase();
        let close = if rest.starts_with("<!--") {
            "-->"
        } else if lower.starts_with("<script") {
            "</script>"
        } else if lower.starts_with("<style") {
            "</style>"
        } else {
            ">"
        };
        // Block tags and line breaks separate words; inline tags join their text.
        if breaks_words(rest) {
            text.push(' ');
        }
        // Script and style close tags are matched case-insensitively.
        let end = rest.to_ascii_lowercase().find(close);
        rest = end.map_or("", |end| &rest[end + close.len()..]);
    }
    text.push_str(rest);
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Decodes `&amp; &lt; &gt; &quot; &apos; &nbsp;`, the typographic names the reference emits, and
/// numeric references; anything else is kept as written.
#[must_use]
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest
            .find(';')
            .filter(|&end| end <= 10)
            .and_then(|end| entity(&rest[1..end]).map(|c| (c, end)));
        if let Some((c, end)) = decoded {
            out.push(c);
            rest = &rest[end + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    let named = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "hellip" => '\u{2026}',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            return char::from_u32(code);
        }
    };
    Some(named)
}

/// The post's excerpt as plain text: the hand-written one, or the first [`EXCERPT_WORDS`] words
/// of the content followed by ` […]` when it was cut. Card markers never reach an excerpt.
#[must_use]
pub fn excerpt(post: &Post) -> String {
    if !post.excerpt.trim().is_empty() {
        return post.excerpt.trim().to_owned();
    }
    let text = plain_text(&strip_markers(&format_content(&post.content)));
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    if words.len() <= EXCERPT_WORDS {
        return words.join(" ");
    }
    let mut out = words[..EXCERPT_WORDS].join(" ");
    out.push_str(" [\u{2026}]");
    out
}

#[cfg(test)]
mod tests {
    use super::{decode_entities, format_content, plain_text};

    #[test]
    fn block_content_loses_its_delimiters_only() {
        let raw = "<!-- wp:paragraph -->\n<p>One</p>\n<!-- /wp:paragraph -->\n\n<!-- note -->\
                   <!-- wp:latest-posts {\"n\":3} /-->\n<!--wp:heading-->\n<h2>Two</h2>\n<!-- /wp:heading -->";
        assert_eq!(
            format_content(raw),
            "<p>One</p>\n\n<!-- note --><h2>Two</h2>"
        );
        assert_eq!(
            format_content("<!-- wp:x --><p>a</p><!-- broken"),
            "<p>a</p><!-- broken"
        );
    }

    #[test]
    fn classic_content_is_auto_paragraphed() {
        let raw = "First line\r\nsecond line\r\n\r\n<h2>Head</h2>\n\n\n<!-- c -->\n\nLast <em>bit</em>\n\n  ";
        assert_eq!(
            format_content(raw),
            "<p>First line<br />\nsecond line</p>\n<h2>Head</h2>\n<!-- c -->\n<p>Last <em>bit</em></p>"
        );
        assert_eq!(format_content("<UL><li>x</li></UL>"), "<UL><li>x</li></UL>");
        assert_eq!(format_content("<span>x</span>"), "<p><span>x</span></p>");
        assert_eq!(format_content(""), "");
    }

    #[test]
    fn plain_text_drops_markup_scripts_and_decodes() {
        assert_eq!(
            plain_text(
                "<p>Tom &amp; <b>Jerry</b></p><SCRIPT>x()</script><style>p{}</STYLE><!-- c -->end"
            ),
            "Tom & Jerry end"
        );
        assert_eq!(plain_text("a <b"), "a");
        assert_eq!(plain_text("a<br>b"), "a b");
    }

    #[test]
    fn entities_decode_or_stay() {
        assert_eq!(
            decode_entities(
                "&lt;&gt;&quot;&apos;&nbsp;&hellip;&ndash;&mdash;&lsquo;&rsquo;&ldquo;&rdquo;&#65;&#x42;&#X43;"
            ),
            "<>\"' \u{2026}\u{2013}\u{2014}\u{2018}\u{2019}\u{201c}\u{201d}ABC"
        );
        assert_eq!(
            decode_entities("&bogus; & &#xZZ; &verylongentityname;"),
            "&bogus; & &#xZZ; &verylongentityname;"
        );
        assert_eq!(decode_entities("&#1114112;"), "&#1114112;");
    }
}
