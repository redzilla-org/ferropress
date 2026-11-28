//! The native built-in theme: the shared shell and the per-route page fragments.
//!
//! A fragment is `<head part>` + [`HEAD_END`] + `<body markup>`; `cms_readmodel::assemble` puts
//! the head part at the shell's head mark and the body at its body mark (ADR-0011).

use std::fmt::Write as _;

use cms_readmodel::{BODY_MARK, HEAD_END, HEAD_MARK};

use crate::cards::{CardError, CardSource, Marker, render_markers};
use crate::escape::{push_escaped, push_json};
use crate::format::{excerpt, format_content};
use crate::model::{Archive, ArchiveKind, Post, PostKind, Site, Term, paged_url};
use crate::stamp::month_name;

/// `Content-Type` of pages and fragments.
pub const HTML: &str = "text/html; charset=UTF-8";

// The reference's document title separator.
const SEP: &str = " \u{2013} ";

// Minimal readable styles; the shell is one object, so these cost one fetch per warm Lambda.
const STYLE: &str = "body{margin:0 auto;max-width:44rem;padding:0 1rem;font:1.05rem/1.6 system-ui,sans-serif;color:#1a1a1a}\
header.site,footer.site{padding:1rem 0;border-bottom:1px solid #ddd}footer.site{border:0;border-top:1px solid #ddd}\
header.site nav a{margin-right:1rem}.meta{color:#595959;font-size:.9rem}img{max-width:100%;height:auto}\
.fp-card{border:1px solid #d9d9d9;border-radius:6px;padding:1rem;margin:1.5rem 0}\
.fp-card table{border-collapse:collapse}.fp-card th,.fp-card td{border:1px solid #d9d9d9;padding:.3rem .7rem;text-align:left}\
.fp-card-tiles{display:grid;grid-template-columns:repeat(auto-fill,minmax(12rem,1fr));gap:.75rem}\
.fp-card-tile{display:block;border:1px solid #d9d9d9;border-radius:6px;text-decoration:none;color:inherit}\
.fp-card-tile span{display:block;padding:0 .6rem}.fp-card-flag{font-weight:700;letter-spacing:.08em}\
.fp-card-price{font-weight:700}.fp-card-credit{font-size:.75rem;color:#595959}";

/// Rendered bytes and their content type; the projector hashes and stores them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    /// The bytes.
    pub bytes: Vec<u8>,
    /// `Content-Type` value.
    pub content_type: &'static str,
}

impl Rendered {
    pub(crate) fn new(text: String, content_type: &'static str) -> Self {
        Self {
            bytes: text.into_bytes(),
            content_type,
        }
    }
}

/// A render whose content may hold card markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The bytes.
    pub rendered: Rendered,
    /// Markers the card source had no card for; they render as nothing.
    pub unresolved: Vec<Marker>,
}

/// The site's shared shell: doctype, head (with the head mark), header, nav, main (with the body
/// mark) and footer.
#[must_use]
pub fn shell(site: &Site) -> Rendered {
    let mut out = String::with_capacity(2048);
    out.push_str("<!DOCTYPE html>\n<html lang=\"");
    push_escaped(&mut out, &site.language);
    out.push_str("\">\n<head>\n<meta charset=\"UTF-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    feed_link(
        &mut out,
        &format!("{} \u{bb} Feed", site.title),
        &site.url("feed/"),
    );
    out.push_str("<style>");
    out.push_str(STYLE);
    out.push_str("</style>\n");
    out.push_str(ascii(HEAD_MARK));
    out.push_str("\n</head>\n<body>\n<header class=\"site\"><p class=\"site-title\"><a href=\"");
    push_escaped(&mut out, &site.home);
    out.push_str("\">");
    push_escaped(&mut out, &site.title);
    out.push_str("</a></p>");
    if !site.description.is_empty() {
        out.push_str("<p class=\"site-description\">");
        push_escaped(&mut out, &site.description);
        out.push_str("</p>");
    }
    if !site.nav.is_empty() {
        out.push_str("<nav>");
        for link in &site.nav {
            anchor(&mut out, &link.url, &link.label);
        }
        out.push_str("</nav>");
    }
    out.push_str("</header>\n<main>\n");
    out.push_str(ascii(BODY_MARK));
    out.push_str("\n</main>\n<footer class=\"site\"><p><a href=\"");
    push_escaped(&mut out, &site.home);
    out.push_str("\">");
    push_escaped(&mut out, &site.title);
    out.push_str("</a></p></footer>\n</body>\n</html>\n");
    Rendered::new(out, HTML)
}

/// The fragment for one post or page, its markers replaced by cards from `cards`.
///
/// # Errors
///
/// [`CardError`] from the card source.
pub fn single<C: CardSource + ?Sized>(
    site: &Site,
    post: &Post,
    cards: &mut C,
) -> Result<Output, CardError> {
    let body = render_markers(&format_content(&post.content), cards)?;
    let description = excerpt(post);
    let mut out = String::with_capacity(body.html.len() + 2048);
    head(
        &mut out,
        &[&post.title, &site.title],
        Some(&post.permalink),
        &description,
    );
    if post.kind == PostKind::Post {
        json_ld(&mut out, site, post, &description);
    }
    out.push_str(ascii(HEAD_END));
    let class = if post.kind == PostKind::Post {
        "post"
    } else {
        "page"
    };
    out.push_str("<article class=\"");
    out.push_str(class);
    out.push_str("\">\n<header><h1>");
    push_escaped(&mut out, &post.title);
    out.push_str("</h1>");
    if post.kind == PostKind::Post {
        out.push_str("<p class=\"meta\">");
        time(&mut out, post);
        out.push_str(" by ");
        anchor(&mut out, &post.author.link, &post.author.name);
        out.push_str("</p>");
    }
    out.push_str("</header>\n<div class=\"content\">\n");
    out.push_str(&body.html);
    out.push_str("\n</div>\n");
    if post.kind == PostKind::Post && (!post.categories.is_empty() || !post.tags.is_empty()) {
        out.push_str("<footer>");
        terms(&mut out, "Categories", &post.categories);
        terms(&mut out, "Tags", &post.tags);
        out.push_str("</footer>\n");
    }
    out.push_str("</article>\n");
    Ok(Output {
        rendered: Rendered::new(out, HTML),
        unresolved: body.unresolved,
    })
}

/// The fragment for one page of an archive (home, term, author or date). Items show their
/// excerpts, so no card source is needed.
#[must_use]
pub fn archive(site: &Site, page: &Archive<'_>) -> Rendered {
    let heading = archive_heading(page.kind);
    let numbered = (page.page > 1).then(|| format!("Page {}", page.page));
    let mut parts: Vec<&str> = Vec::with_capacity(3);
    let name = archive_name(page.kind);
    match page.kind {
        ArchiveKind::Home => parts.push(&site.title),
        _ => parts.push(&name),
    }
    if let Some(numbered) = &numbered {
        parts.push(numbered);
    }
    match page.kind {
        ArchiveKind::Home if !site.description.is_empty() => parts.push(&site.description),
        ArchiveKind::Home => {}
        _ => parts.push(&site.title),
    }
    let canonical = paged_url(page.url, page.page);
    let description = if matches!(page.kind, ArchiveKind::Home) && page.page == 1 {
        &site.description
    } else {
        ""
    };
    let mut out = String::with_capacity(1024 + page.items.len() * 512);
    head(&mut out, &parts, Some(&canonical), description);
    if let ArchiveKind::Category(term) | ArchiveKind::Tag(term) = page.kind {
        let label = match page.kind {
            ArchiveKind::Category(_) => "Category",
            _ => "Tag",
        };
        feed_link(
            &mut out,
            &format!("{} \u{bb} {} {label} Feed", site.title, term.name),
            &format!("{}feed/", term.link),
        );
    }
    out.push_str(ascii(HEAD_END));
    if let Some(heading) = heading {
        out.push_str("<header class=\"archive\"><h1>");
        push_escaped(&mut out, &heading);
        out.push_str("</h1></header>\n");
    }
    if page.items.is_empty() {
        out.push_str("<p>Nothing has been published here yet.</p>\n");
    }
    for post in page.items {
        out.push_str("<article class=\"post\"><h2>");
        anchor(&mut out, &post.permalink, &post.title);
        out.push_str("</h2><p class=\"meta\">");
        time(&mut out, post);
        out.push_str("</p><p>");
        push_escaped(&mut out, &excerpt(post));
        out.push_str("</p><p><a href=\"");
        push_escaped(&mut out, &post.permalink);
        out.push_str("\">Continue reading</a></p></article>\n");
    }
    if page.pages > 1 {
        out.push_str("<nav class=\"pagination\">");
        if page.page > 1 {
            out.push_str("<a rel=\"prev\" href=\"");
            push_escaped(&mut out, &paged_url(page.url, page.page - 1));
            out.push_str("\">Newer posts</a> ");
        }
        let _ = write!(out, "<span>Page {} of {}</span>", page.page, page.pages);
        if page.page < page.pages {
            out.push_str(" <a rel=\"next\" href=\"");
            push_escaped(&mut out, &paged_url(page.url, page.page + 1));
            out.push_str("\">Older posts</a>");
        }
        out.push_str("</nav>\n");
    }
    Rendered::new(out, HTML)
}

/// The not-found fragment, stored at the `<base>404` route.
#[must_use]
pub fn not_found(site: &Site) -> Rendered {
    let mut out = String::with_capacity(512);
    head(&mut out, &["Page not found", &site.title], None, "");
    out.push_str("<meta name=\"robots\" content=\"noindex\">\n");
    out.push_str(ascii(HEAD_END));
    out.push_str("<header class=\"archive\"><h1>Page not found</h1></header>\n<p>Nothing was found at this address. Try the <a href=\"");
    push_escaped(&mut out, &site.home);
    out.push_str("\">home page</a>.</p>\n");
    Rendered::new(out, HTML)
}

// The visible heading of an archive (none on the home page).
fn archive_heading(kind: &ArchiveKind) -> Option<String> {
    let prefix = match kind {
        ArchiveKind::Home => return None,
        ArchiveKind::Category(_) => "Category",
        ArchiveKind::Tag(_) => "Tag",
        ArchiveKind::Author(_) => "Author",
        ArchiveKind::Date { month: None, .. } => "Year",
        ArchiveKind::Date { day: None, .. } => "Month",
        ArchiveKind::Date { .. } => "Day",
    };
    Some(format!("{prefix}: {}", archive_name(kind)))
}

// The archive's own name, used in its document title.
fn archive_name(kind: &ArchiveKind) -> String {
    match kind {
        ArchiveKind::Home => String::new(),
        ArchiveKind::Category(term) | ArchiveKind::Tag(term) => term.name.to_string(),
        ArchiveKind::Author(author) => author.name.to_string(),
        ArchiveKind::Date { year, month, day } => match (month, day) {
            (Some(month), Some(day)) => format!("{} {day}, {year}", month_name(*month)),
            (Some(month), None) => format!("{} {year}", month_name(*month)),
            _ => year.to_string(),
        },
    }
}

fn head(out: &mut String, title: &[&str], canonical: Option<&str>, description: &str) {
    out.push_str("<title>");
    push_escaped(out, &title.join(SEP));
    out.push_str("</title>\n");
    if let Some(canonical) = canonical {
        out.push_str("<link rel=\"canonical\" href=\"");
        push_escaped(out, canonical);
        out.push_str("\">\n");
    }
    if !description.is_empty() {
        out.push_str("<meta name=\"description\" content=\"");
        push_escaped(out, description);
        out.push_str("\">\n");
    }
}

fn json_ld(out: &mut String, site: &Site, post: &Post, description: &str) {
    out.push_str("<script type=\"application/ld+json\">{\"@context\":\"https://schema.org\",\"@type\":\"BlogPosting\",\"headline\":");
    push_json(out, &post.title);
    out.push_str(",\"url\":");
    push_json(out, &post.permalink);
    out.push_str(",\"mainEntityOfPage\":");
    push_json(out, &post.permalink);
    out.push_str(",\"datePublished\":");
    push_json(out, &post.date.iso8601(post.date_gmt));
    out.push_str(",\"dateModified\":");
    push_json(out, &post.modified.iso8601(post.modified_gmt));
    out.push_str(",\"author\":{\"@type\":\"Person\",\"name\":");
    push_json(out, &post.author.name);
    out.push_str(",\"url\":");
    push_json(out, &post.author.link);
    out.push_str("},\"publisher\":{\"@type\":\"Organization\",\"name\":");
    push_json(out, &site.title);
    out.push_str(",\"url\":");
    push_json(out, &site.home);
    out.push('}');
    if !description.is_empty() {
        out.push_str(",\"description\":");
        push_json(out, description);
    }
    out.push_str("}</script>\n");
}

fn feed_link(out: &mut String, title: &str, href: &str) {
    out.push_str("<link rel=\"alternate\" type=\"application/rss+xml\" title=\"");
    push_escaped(out, title);
    out.push_str("\" href=\"");
    push_escaped(out, href);
    out.push_str("\">\n");
}

fn anchor(out: &mut String, href: &str, text: &str) {
    out.push_str("<a href=\"");
    push_escaped(out, href);
    out.push_str("\">");
    push_escaped(out, text);
    out.push_str("</a>");
}

fn time(out: &mut String, post: &Post) {
    out.push_str("<time datetime=\"");
    out.push_str(&post.date.iso8601(post.date_gmt));
    out.push_str("\">");
    out.push_str(&post.date.display_date());
    out.push_str("</time>");
}

fn terms(out: &mut String, label: &str, terms: &[Term]) {
    if terms.is_empty() {
        return;
    }
    out.push_str("<p class=\"meta\">");
    out.push_str(label);
    out.push_str(": ");
    for (index, term) in terms.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        anchor(out, &term.link, &term.name);
    }
    out.push_str("</p>");
}

// The marks are ASCII constants owned by the read model.
fn ascii(mark: &[u8]) -> &str {
    std::str::from_utf8(mark).unwrap_or_default()
}
