//! The HTML results page.

use std::fmt::Write as _;

use cms_readmodel::BODY_MARK;
use cms_search::{Document, Page};

use crate::query::encode;

// The reference's generated excerpt length, in words, and its "more" marker.
const SUMMARY_WORDS: usize = 55;
const MORE: &str = " [&hellip;]";

/// The results fragment for `search` on page `page` of `found`, with links under `base`.
pub(crate) fn results(base: &str, search: &str, page: usize, found: &Page<'_>) -> String {
    let mut out = String::with_capacity(512 + found.hits.len() * 512);
    // Writing to a String cannot fail; the results are ignored throughout.
    let _ = write!(
        out,
        "<section class=\"search-results\"><header class=\"page-header\">\
         <h1 class=\"page-title\">Search results for: <span>{}</span></h1></header>",
        escape(search)
    );
    if found.hits.is_empty() {
        out.push_str(
            "<p class=\"no-results\">Nothing matched your search. Try different keywords.</p>",
        );
    }
    for doc in &found.hits {
        // Titles and excerpts are stored markup written by the site's editors, so they pass as-is.
        let _ = write!(
            out,
            "<article id=\"post-{id}\" class=\"post-{id} type-{kind}\"><h2 class=\"entry-title\">\
             <a href=\"{href}\" rel=\"bookmark\">{title}</a></h2>\
             <div class=\"entry-summary\"><p>{summary}</p></div></article>",
            id = doc.id,
            kind = escape(&doc.kind),
            href = escape(&doc.permalink),
            title = doc.title,
            summary = summary(doc),
        );
    }
    if found.pages > 1 {
        out.push_str("<nav class=\"navigation pagination\" aria-label=\"Search results\">");
        if page > 1 {
            let _ = write!(
                out,
                "<a class=\"prev page-numbers\" href=\"{}\">Previous</a>",
                link(base, search, page - 1)
            );
        }
        let _ = write!(
            out,
            "<span class=\"page-numbers current\">{page} / {}</span>",
            found.pages
        );
        if page < found.pages {
            let _ = write!(
                out,
                "<a class=\"next page-numbers\" href=\"{}\">Next</a>",
                link(base, search, page + 1)
            );
        }
        out.push_str("</nav>");
    }
    out.push_str("</section>");
    out
}

/// The full page: `fragment` in the theme shell at the body mark, or in a bare document while the
/// site has no rendered shell. `None` when the shell lacks the mark.
pub(crate) fn document(shell: Option<&[u8]>, fragment: &str) -> Option<Vec<u8>> {
    let Some(shell) = shell else {
        return Some(
            format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><title>Search results</title>\
                 </head><body>{fragment}</body></html>"
            )
            .into_bytes(),
        );
    };
    let at = shell
        .windows(BODY_MARK.len())
        .position(|window| window == BODY_MARK)?;
    let mut out = Vec::with_capacity(shell.len() + fragment.len());
    out.extend_from_slice(&shell[..at]);
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(&shell[at + BODY_MARK.len()..]);
    Some(out)
}

// The reference's pretty search paging links: `<base>?s=` then `<base>page/N/?s=`.
fn link(base: &str, search: &str, page: usize) -> String {
    let search = escape(&encode(search));
    if page == 1 {
        format!("{base}?s={search}")
    } else {
        format!("{base}page/{page}/?s={search}")
    }
}

// The stored excerpt, else the first words of the content.
fn summary(doc: &Document) -> String {
    if !doc.excerpt.is_empty() {
        return doc.excerpt.to_string();
    }
    let mut words = doc.content.split_whitespace();
    let mut out = words
        .by_ref()
        .take(SUMMARY_WORDS)
        .collect::<Vec<_>>()
        .join(" ");
    if words.next().is_some() {
        out.push_str(MORE);
    }
    out
}

/// `text` safe inside HTML text and double-quoted attributes.
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
    use super::{document, escape, link, summary};
    use cms_search::Document;

    #[test]
    fn text_is_escaped() {
        assert_eq!(
            escape(r#"<a href="x">'&'"#),
            "&lt;a href=&quot;x&quot;&gt;&#039;&amp;&#039;"
        );
    }

    #[test]
    fn links_follow_the_pretty_paging_form() {
        assert_eq!(link("/blog/", "red boat", 1), "/blog/?s=red+boat");
        assert_eq!(link("/blog/", "a&b", 3), "/blog/page/3/?s=a%26b");
    }

    #[test]
    fn summaries_prefer_the_excerpt_then_trim_content() {
        let long = vec!["w"; 56].join(" ");
        let doc = Document::new(1, "post", "", "/", "T", "", &long);
        let trimmed = summary(&doc);
        assert!(trimmed.ends_with("w [&hellip;]"));
        assert_eq!(trimmed.matches('w').count(), 55);
        let short = Document::new(1, "post", "", "/", "T", "", "<p>two words</p>");
        assert_eq!(summary(&short), "two words");
        let excerpt = Document::new(1, "post", "", "/", "T", "Hand <em>made</em>", "x");
        assert_eq!(summary(&excerpt), "Hand <em>made</em>");
    }

    #[test]
    fn documents_use_the_shell_mark_or_a_bare_page() {
        assert_eq!(
            document(Some(b"<main><!--fp:body--></main>"), "<p>r</p>"),
            Some(b"<main><p>r</p></main>".to_vec())
        );
        assert_eq!(document(Some(b"<main></main>"), "x"), None);
        let bare = document(None, "<p>r</p>").map(String::from_utf8);
        assert!(bare.is_some_and(|page| page.is_ok_and(|p| p.contains("<body><p>r</p></body>"))));
    }
}
