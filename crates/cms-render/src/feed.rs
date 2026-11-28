//! RSS 2.0 feeds for the site and for one term or author.

use crate::cards::{CardError, CardSource, render_markers};
use crate::escape::{push_cdata, push_escaped};
use crate::format::{excerpt, format_content};
use crate::model::{Post, Site};
use crate::theme::{Output, Rendered};

/// Items per feed, as the reference's default `posts_per_rss`.
pub const FEED_ITEMS: usize = 10;

/// `Content-Type` of feeds.
pub const RSS: &str = "application/rss+xml; charset=UTF-8";

/// One feed to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel<'a> {
    /// Term or author name for a narrowed feed; `None` for the site feed.
    pub name: Option<&'a str>,
    /// The page the feed belongs to (home or archive URL).
    pub link: &'a str,
    /// The feed's own URL.
    pub self_url: &'a str,
    /// Items, newest first; the caller passes at most [`FEED_ITEMS`].
    pub items: &'a [Post],
}

/// The RSS 2.0 document for `channel`, with each item's markers replaced by cards.
///
/// # Errors
///
/// [`CardError`] from the card source.
pub fn feed<C: CardSource + ?Sized>(
    site: &Site,
    channel: &Channel<'_>,
    cards: &mut C,
) -> Result<Output, CardError> {
    let mut out = String::with_capacity(1024 + channel.items.len() * 4096);
    let mut unresolved = Vec::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:content=\"http://purl.org/rss/1.0/modules/content/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:atom=\"http://www.w3.org/2005/Atom\">\n<channel>\n<title>");
    match channel.name {
        Some(name) => push_escaped(&mut out, &format!("{} \u{bb} {name}", site.title)),
        None => push_escaped(&mut out, &site.title),
    }
    out.push_str("</title>\n<atom:link href=\"");
    push_escaped(&mut out, channel.self_url);
    out.push_str("\" rel=\"self\" type=\"application/rss+xml\" />\n");
    element(&mut out, "link", channel.link);
    element(&mut out, "description", &site.description);
    // The newest modification among the items; a pure renderer has no clock for an empty feed.
    if let Some(newest) = channel.items.iter().map(|post| post.modified_gmt).max() {
        element(&mut out, "lastBuildDate", &newest.rfc822());
    }
    element(&mut out, "language", &site.language);
    for post in channel.items {
        let content = render_markers(&format_content(&post.content), cards)?;
        unresolved.extend(content.unresolved);
        out.push_str("<item>\n");
        element(&mut out, "title", &post.title);
        element(&mut out, "link", &post.permalink);
        out.push_str("<dc:creator>");
        push_cdata(&mut out, &post.author.name);
        out.push_str("</dc:creator>\n");
        element(&mut out, "pubDate", &post.date_gmt.rfc822());
        for term in post.categories.iter().chain(&post.tags) {
            out.push_str("<category>");
            push_cdata(&mut out, &term.name);
            out.push_str("</category>\n");
        }
        out.push_str("<guid isPermaLink=\"false\">");
        push_escaped(&mut out, &post.guid);
        out.push_str("</guid>\n<description>");
        push_cdata(&mut out, &excerpt(post));
        out.push_str("</description>\n<content:encoded>");
        push_cdata(&mut out, &content.html);
        out.push_str("</content:encoded>\n</item>\n");
    }
    out.push_str("</channel>\n</rss>\n");
    Ok(Output {
        rendered: Rendered::new(out, RSS),
        unresolved,
    })
}

fn element(out: &mut String, name: &str, text: &str) {
    out.push('<');
    out.push_str(name);
    out.push('>');
    push_escaped(out, text);
    out.push_str("</");
    out.push_str(name);
    out.push_str(">\n");
}
