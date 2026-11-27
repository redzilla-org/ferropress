//! The native admin's HTML: list, editor and error pages, in Ferropress's own markup and styles.

use cms_render::push_escaped;

use crate::editor::{Page, Site};
use crate::http::encode;
use crate::model::{PostRow, Status, StatusFilter, TermRow};

/// The admin client script, served at `admin/editor.js`.
pub(crate) const SCRIPT: &str = include_str!("editor.js");

/// HTML content type.
pub(crate) const HTML: &str = "text/html; charset=UTF-8";

const STYLE: &str = "body{font:16px/1.5 system-ui,sans-serif;margin:0;color:#1d1d1f;background:#f6f6f4}\
header.admin{background:#20303a;color:#fff;padding:.6rem 1rem}header.admin a{color:#fff;margin-right:1rem}\
main{max-width:60rem;margin:1rem auto;padding:0 1rem}table{width:100%;border-collapse:collapse;background:#fff}\
th,td{text-align:left;padding:.4rem .6rem;border-bottom:1px solid #ddd}label{display:block;margin:.8rem 0 .2rem;font-weight:600}\
input[type=text],textarea{width:100%;box-sizing:border-box;font:inherit;padding:.4rem}textarea.body{min-height:24rem;font-family:ui-monospace,monospace}\
.notice{background:#e7f4e4;padding:.5rem 1rem}.error{background:#fbe9e7;padding:.5rem 1rem}\
.actions button{margin-right:.5rem;padding:.4rem 1rem}.filters a{margin-right:.8rem}fieldset{border:0;padding:0}";

/// The admin frame around `body`.
pub(crate) fn frame(site: &Site, title: &str, body: &str) -> Vec<u8> {
    let base = &site.theme.base;
    let mut out = String::with_capacity(body.len() + 1024);
    out.push_str("<!DOCTYPE html>\n<html lang=\"");
    push_escaped(&mut out, &site.theme.language);
    out.push_str("\">\n<head>\n<meta charset=\"UTF-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta name=\"robots\" content=\"noindex\">\n<title>");
    push_escaped(&mut out, title);
    out.push_str(" \u{2039} ");
    push_escaped(&mut out, &site.theme.title);
    out.push_str("</title>\n<style>");
    out.push_str(STYLE);
    out.push_str("</style>\n<script src=\"");
    push_escaped(&mut out, base);
    out.push_str("admin/editor.js\" defer></script>\n</head>\n<body>\n<header class=\"admin\"><a href=\"");
    push_escaped(&mut out, base);
    out.push_str("admin/\">Posts</a><a href=\"");
    push_escaped(&mut out, base);
    out.push_str("admin/posts/new\">New post</a><a href=\"");
    push_escaped(&mut out, &site.theme.home);
    out.push_str("\">View site</a></header>\n<main>\n");
    out.push_str(body);
    out.push_str("\n</main>\n</body>\n</html>\n");
    out.into_bytes()
}

/// The post list for `filter`, page `number`.
pub(crate) fn list(site: &Site, filter: StatusFilter, number: u64, page: &Page) -> Vec<u8> {
    let base = &site.theme.base;
    let mut out = String::from("<h1>Posts</h1>\n<p class=\"filters\">");
    let filters = [
        ("All", None),
        ("Drafts", Some(Status::Draft)),
        ("Pending", Some(Status::Pending)),
        ("Published", Some(Status::Publish)),
        ("Trash", Some(Status::Trash)),
    ];
    for (label, status) in filters {
        out.push_str("<a href=\"");
        push_escaped(&mut out, base);
        out.push_str("admin/");
        if let Some(status) = status {
            out.push_str("?post_status=");
            out.push_str(status.as_str());
        }
        out.push('"');
        let current = match status {
            Some(status) => filter == StatusFilter::Only(status),
            None => filter == StatusFilter::All,
        };
        if current {
            out.push_str(" aria-current=\"page\"");
        }
        out.push('>');
        out.push_str(label);
        out.push_str("</a>");
    }
    out.push_str("</p>\n<table>\n<thead><tr><th>Title</th><th>Status</th><th>Date</th><th></th></tr></thead>\n<tbody>\n");
    for post in &page.posts {
        row(&mut out, base, post);
    }
    if page.posts.is_empty() {
        out.push_str("<tr><td colspan=\"4\">No posts found.</td></tr>\n");
    }
    out.push_str("</tbody>\n</table>\n<p class=\"pages\">");
    let status = match filter {
        StatusFilter::All => String::new(),
        StatusFilter::Only(status) => format!("post_status={}&amp;", status.as_str()),
    };
    if number > 1 {
        out.push_str(&format!(
            "<a href=\"{}admin/?{status}paged={}\">Newer</a> ",
            encode(base),
            number - 1
        ));
    }
    if page.has_next {
        out.push_str(&format!(
            "<a href=\"{}admin/?{status}paged={}\">Older</a>",
            encode(base),
            number + 1
        ));
    }
    out.push_str("</p>");
    frame(site, "Posts", &out)
}

fn row(out: &mut String, base: &str, post: &PostRow) {
    let id = post.id.as_u64();
    out.push_str("<tr><td><a href=\"");
    push_escaped(out, base);
    out.push_str(&format!("admin/posts/{id}/edit\">"));
    if post.title.is_empty() {
        out.push_str("(no title)");
    } else {
        push_escaped(out, &post.title);
    }
    out.push_str("</a></td><td>");
    push_escaped(out, &post.status);
    out.push_str("</td><td>");
    push_escaped(out, &post.date);
    out.push_str("</td><td><a href=\"");
    push_escaped(out, base);
    out.push_str(&format!("admin/preview/{id}\">Preview</a></td></tr>\n"));
}

/// The editor for a new post (`post` is `None`) or an existing one.
pub(crate) fn editor(
    site: &Site,
    post: Option<(&PostRow, &[TermRow])>,
    categories: &[TermRow],
    nonce: &str,
    notice: Option<&str>,
) -> Vec<u8> {
    let base = &site.theme.base;
    let mut out = String::with_capacity(4096);
    out.push_str(if post.is_some() {
        "<h1>Edit post</h1>\n"
    } else {
        "<h1>New post</h1>\n"
    });
    if let Some(notice) = notice {
        out.push_str("<p class=\"notice\">");
        push_escaped(&mut out, notice);
        out.push_str("</p>\n");
    }
    out.push_str("<form method=\"post\" data-fp-signed action=\"");
    push_escaped(&mut out, base);
    match post {
        Some((row, _)) => out.push_str(&format!("admin/posts/{}", row.id.as_u64())),
        None => out.push_str("admin/posts"),
    }
    out.push_str("\">\n<input type=\"hidden\" name=\"_fpnonce\" value=\"");
    push_escaped(&mut out, nonce);
    out.push_str("\">\n");
    let row = post.map(|(row, _)| row);
    let assigned = post.map_or(&[][..], |(_, terms)| terms);
    text_field(&mut out, "title", "Title", row.map_or("", |row| &row.title));
    text_field(&mut out, "slug", "Slug", row.map_or("", |row| &row.name));
    out.push_str("<label for=\"content\">Body (HTML)</label><textarea class=\"body\" id=\"content\" name=\"content\">");
    push_escaped(&mut out, row.map_or("", |row| &row.content));
    out.push_str("</textarea>\n<label for=\"excerpt\">Excerpt</label><textarea id=\"excerpt\" name=\"excerpt\" rows=\"3\">");
    push_escaped(&mut out, row.map_or("", |row| &row.excerpt));
    out.push_str("</textarea>\n<fieldset><legend><strong>Categories</strong></legend>\n<input type=\"hidden\" name=\"categories_sent\" value=\"1\">\n");
    for category in categories {
        let checked = assigned
            .iter()
            .any(|term| term.taxonomy_id == category.taxonomy_id);
        out.push_str(&format!(
            "<label><input type=\"checkbox\" name=\"category\" value=\"{}\"{}> ",
            category.term_id,
            if checked { " checked" } else { "" }
        ));
        push_escaped(&mut out, &category.name);
        out.push_str("</label>\n");
    }
    out.push_str("</fieldset>\n");
    let tags: Vec<&str> = assigned
        .iter()
        .filter(|term| term.taxonomy == "post_tag")
        .map(|term| term.name.as_str())
        .collect();
    text_field(&mut out, "tags", "Tags (comma separated)", &tags.join(", "));
    out.push_str("<p>Status: <strong>");
    push_escaped(&mut out, row.map_or("new", |row| &row.status));
    out.push_str("</strong></p>\n<p class=\"actions\">");
    let published = row.is_some_and(PostRow::is_published);
    let mut buttons = vec![("draft", "Save draft"), ("pending", "Submit for review")];
    if published {
        buttons = vec![("publish", "Update"), ("unpublish", "Unpublish")];
    } else {
        buttons.push(("publish", "Publish"));
    }
    if row.is_some() {
        buttons.push(("trash", "Move to trash"));
    }
    for (value, label) in buttons {
        out.push_str(&format!(
            "<button type=\"submit\" name=\"do\" value=\"{value}\">{label}</button>"
        ));
    }
    if let Some(row) = row {
        out.push_str("<a href=\"");
        push_escaped(&mut out, base);
        out.push_str(&format!(
            "admin/preview/{}\" target=\"_blank\">Preview</a>",
            row.id.as_u64()
        ));
    }
    out.push_str("</p>\n</form>");
    frame(
        site,
        if post.is_some() { "Edit post" } else { "New post" },
        &out,
    )
}

fn text_field(out: &mut String, name: &str, label: &str, value: &str) {
    out.push_str(&format!(
        "<label for=\"{name}\">{label}</label><input type=\"text\" id=\"{name}\" name=\"{name}\" value=\""
    ));
    push_escaped(out, value);
    out.push_str("\">\n");
}

/// An error page.
pub(crate) fn error(site: &Site, status: u16, message: &str) -> Vec<u8> {
    let mut out = format!("<h1>Error {status}</h1>\n<p class=\"error\">");
    push_escaped(&mut out, message);
    out.push_str("</p>");
    frame(site, "Error", &out)
}
