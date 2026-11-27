//! View models: everything a renderer reads, already resolved by the projector.
//!
//! Text fields are plain text and are escaped on output; only [`Post::content`] is HTML.
//! URLs are absolute, as the reference's permalinks are.

use crate::Stamp;

/// The reference's default `posts_per_page`.
pub const DEFAULT_POSTS_PER_PAGE: u32 = 10;

/// A labelled link (navigation entry, term, author).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Visible text.
    pub label: Box<str>,
    /// Absolute URL.
    pub url: Box<str>,
}

/// Site-wide settings, from the options table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// Site title (`blogname`).
    pub title: Box<str>,
    /// Tagline (`blogdescription`).
    pub description: Box<str>,
    /// Home URL with its trailing slash, base path included, such as `https://example.com/blog/`.
    pub home: Box<str>,
    /// The path the site is mounted at, such as `/blog/`.
    pub base: Box<str>,
    /// Document language, such as `en-US`.
    pub language: Box<str>,
    /// Posts per archive page (`posts_per_page`); see [`DEFAULT_POSTS_PER_PAGE`].
    pub posts_per_page: u32,
    /// Header navigation, in order.
    pub nav: Vec<Link>,
}

impl Site {
    /// An absolute URL for `path` relative to the home URL.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        let mut out = String::with_capacity(self.home.len() + path.len());
        out.push_str(&self.home);
        out.push_str(path.trim_start_matches('/'));
        out
    }
}

/// Whether a post row is a blog post or a static page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostKind {
    /// `post`: dated, authored, in terms and feeds.
    Post,
    /// `page`: a standalone page.
    Page,
}

/// A taxonomy term as linked from a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    /// Display name.
    pub name: Box<str>,
    /// Slug.
    pub slug: Box<str>,
    /// Archive URL.
    pub link: Box<str>,
}

/// A post's author as shown publicly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Author {
    /// `display_name`.
    pub name: Box<str>,
    /// `user_nicename`.
    pub slug: Box<str>,
    /// Author archive URL.
    pub link: Box<str>,
}

/// One published post or page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Post {
    /// Row id.
    pub id: u64,
    /// Post or page.
    pub kind: PostKind,
    /// Title, plain text.
    pub title: Box<str>,
    /// `post_name`.
    pub slug: Box<str>,
    /// Absolute permalink.
    pub permalink: Box<str>,
    /// `guid`, written to feeds as a non-permalink id.
    pub guid: Box<str>,
    /// `post_date` (site-local).
    pub date: Stamp,
    /// `post_date_gmt`.
    pub date_gmt: Stamp,
    /// `post_modified` (site-local).
    pub modified: Stamp,
    /// `post_modified_gmt`.
    pub modified_gmt: Stamp,
    /// Author.
    pub author: Author,
    /// Categories, in display order.
    pub categories: Vec<Term>,
    /// Tags, in display order.
    pub tags: Vec<Term>,
    /// Hand-written excerpt, plain text; empty means generate one from the content.
    pub excerpt: Box<str>,
    /// `post_content`, HTML.
    pub content: Box<str>,
}

/// Which listing an archive page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    /// The posts page at the home URL.
    Home,
    /// One category.
    Category(Term),
    /// One tag.
    Tag(Term),
    /// One author.
    Author(Author),
    /// A year, a month of it, or a day of that month.
    Date {
        /// Year.
        year: u16,
        /// Month 1 to 12, if a month or day archive.
        month: Option<u8>,
        /// Day, if a day archive.
        day: Option<u8>,
    },
}

/// One page of an archive listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Archive<'a> {
    /// What is listed.
    pub kind: &'a ArchiveKind,
    /// Absolute URL of page 1.
    pub url: &'a str,
    /// This page, from 1.
    pub page: u32,
    /// Number of pages, at least 1.
    pub pages: u32,
    /// The posts on this page, newest first.
    pub items: &'a [Post],
}

/// Pages needed for `total` posts at `per_page` each; at least 1, so an empty home still renders.
#[must_use]
pub fn page_count(total: usize, per_page: u32) -> u32 {
    let per_page = usize::try_from(per_page.max(1)).unwrap_or(usize::MAX);
    u32::try_from(total.div_ceil(per_page)).unwrap_or(u32::MAX).max(1)
}

/// URL of page `page` of the archive whose page 1 is `url`: `<url>page/<n>/`, as the reference
/// paginates.
#[must_use]
pub fn paged_url(url: &str, page: u32) -> String {
    if page <= 1 {
        return url.to_owned();
    }
    format!("{url}page/{page}/")
}

#[cfg(test)]
mod tests {
    use super::{Site, page_count, paged_url};

    #[test]
    fn pages_round_up_and_never_drop_below_one() {
        assert_eq!(page_count(0, 10), 1);
        assert_eq!(page_count(10, 10), 1);
        assert_eq!(page_count(11, 10), 2);
        assert_eq!(page_count(189, 10), 19);
        assert_eq!(page_count(3, 0), 3);
    }

    #[test]
    fn paged_urls_follow_the_reference() {
        assert_eq!(paged_url("https://example.com/blog/", 1), "https://example.com/blog/");
        assert_eq!(
            paged_url("https://example.com/blog/category/news/", 3),
            "https://example.com/blog/category/news/page/3/"
        );
    }

    #[test]
    fn site_urls_join_under_home() {
        let site = Site {
            title: "T".into(),
            description: String::new().into(),
            home: "https://example.com/blog/".into(),
            base: "/blog/".into(),
            language: "en-US".into(),
            posts_per_page: 10,
            nav: Vec::new(),
        };
        assert_eq!(site.url("/feed/"), "https://example.com/blog/feed/");
        assert_eq!(site.url("feed/"), "https://example.com/blog/feed/");
    }
}
