//! The native built-in theme of the serverless profile (ADR-0011): pure renderers that turn
//! view models into page fragments, the shared shell, RSS feeds, sitemaps and robots.txt.
//!
//! No I/O happens here. The projector loads view models, renders, hashes the [`Rendered`] bytes
//! and stores them; card data for listing markers comes through the [`CardSource`] port.
//! Pages are split as `cms_readmodel::assemble` composes them: [`shell`] carries the head and
//! body marks, and each fragment is a per-page head part, the head-end mark, then the body.
//!
//! # Examples
//!
//! ```
//! use cms_render::{Site, robots};
//!
//! let site = Site {
//!     title: "Example".into(),
//!     description: "".into(),
//!     home: "https://example.com/blog/".into(),
//!     base: "/blog/".into(),
//!     language: "en-US".into(),
//!     posts_per_page: cms_render::DEFAULT_POSTS_PER_PAGE,
//!     nav: Vec::new(),
//! };
//! let robots = robots(&site);
//! assert!(robots.bytes.ends_with(b"Sitemap: https://example.com/blog/wp-sitemap.xml\n"));
//! ```

mod cards;
mod escape;
mod feed;
mod format;
mod model;
mod sitemap;
mod stamp;
mod theme;

pub use cards::{
    Card, CardError, CardSource, Facts, ListingCard, Marker, NoCards, SoldFigures, Tile, WithCards,
    ZipCard, push_card, render_markers, strip_markers,
};
pub use escape::{escape, push_cdata, push_escaped, push_json};
pub use feed::{Channel, FEED_ITEMS, RSS, feed};
pub use format::{EXCERPT_WORDS, decode_entities, excerpt, format_content, plain_text};
pub use model::{
    Archive, ArchiveKind, Author, DEFAULT_POSTS_PER_PAGE, Link, Post, PostKind, Site, Term,
    page_count, paged_url,
};
pub use sitemap::{
    SITEMAP_INDEX, SitemapFile, SitemapUrls, TEXT, URLS_PER_SITEMAP, XML, robots, sitemaps,
};
pub use stamp::{Stamp, StampError, month_name};
pub use theme::{HTML, Output, Rendered, archive, not_found, shell, single};
