//! One searchable document: a published post or page as the index stores it.

use serde::{Deserialize, Serialize};

/// A searchable post. Field names are one letter in the JSON index to keep it compact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// Post id.
    #[serde(rename = "i")]
    pub id: u64,
    /// Post type (`post`, `page`, ...).
    #[serde(rename = "k")]
    pub kind: Box<str>,
    /// Local post date, `YYYY-MM-DD hh:mm:ss`; fixed width, so it sorts chronologically as text.
    #[serde(rename = "d")]
    pub date: Box<str>,
    /// Public permalink (absolute path or URL).
    #[serde(rename = "u")]
    pub permalink: Box<str>,
    /// Post title as stored.
    #[serde(rename = "t")]
    pub title: Box<str>,
    /// The excerpt column as stored (often empty); the reference matches this, not a generated one.
    #[serde(rename = "x")]
    pub excerpt: Box<str>,
    /// Post content with markup removed.
    #[serde(rename = "c")]
    pub content: Box<str>,
}

impl Document {
    /// A document from a post's stored fields; `content_html` is reduced with [`text_of`].
    #[must_use]
    pub fn new(
        id: u64,
        kind: &str,
        date: &str,
        permalink: &str,
        title: &str,
        excerpt: &str,
        content_html: &str,
    ) -> Self {
        Self {
            id,
            kind: kind.into(),
            date: date.into(),
            permalink: permalink.into(),
            title: title.into(),
            excerpt: excerpt.into(),
            content: text_of(content_html).into(),
        }
    }
}

/// `html` without its comments (block delimiters included) and tags; text and entities are kept.
///
/// Removing markup keeps the index small and stops a search for a tag or block name from matching
/// every post. A `<` with no closing `>` is text.
#[must_use]
pub fn text_of(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let markup = &rest[at..];
        // A comment may contain `>`, so it ends only at `-->`.
        let end = if markup.starts_with("<!--") {
            markup.find("-->").map(|close| close + 3)
        } else {
            markup.find('>').map(|close| close + 1)
        };
        let Some(end) = end else {
            out.push_str(markup);
            return out;
        };
        rest = &markup[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{Document, text_of};

    #[test]
    fn markup_is_removed_and_text_kept() {
        assert_eq!(
            text_of("<!-- wp:paragraph --><p>Fish &amp; <b>chips</b></p><!-- /wp:paragraph -->"),
            "Fish &amp; chips"
        );
        assert_eq!(text_of("a <!-- x > y --> b"), "a  b");
        assert_eq!(text_of("1 < 2"), "1 < 2");
        assert_eq!(text_of("a <!-- open"), "a <!-- open");
        assert_eq!(text_of("plain"), "plain");
    }

    #[test]
    fn documents_strip_content_only() {
        let doc = Document::new(
            7,
            "page",
            "2026-01-02 03:04:05",
            "/p/",
            "<i>T</i>",
            "x",
            "<p>c</p>",
        );
        assert_eq!(doc.id, 7);
        assert_eq!(&*doc.kind, "page");
        assert_eq!(&*doc.date, "2026-01-02 03:04:05");
        assert_eq!(&*doc.permalink, "/p/");
        assert_eq!(&*doc.title, "<i>T</i>");
        assert_eq!(&*doc.excerpt, "x");
        assert_eq!(&*doc.content, "c");
    }
}
