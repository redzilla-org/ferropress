//! Projector job for the serverless profile (`docs/aws/architecture.md` section 5).
//!
//! [`Projector::run_once`] drains up to [`BATCH`] outbox rows. It computes the routes those
//! events affect, renders them with the native theme (`cms-render`), and then writes in the
//! order that keeps readers consistent: content-addressed objects ([`BlobWriter`]), then route,
//! doc and term-listing items conditional on a version ([`RouteWriter`]), then one batched
//! invalidation ([`Invalidator`]). Only after that does it delete the rows. Every step is
//! idempotent, so at-least-once delivery is safe.
//!
//! # Examples
//!
//! ```
//! use cms_projector::{Paths, Target, TermRef};
//! use cms_readmodel::BasePath;
//!
//! let paths = Paths::new(BasePath::new("/blog/")?);
//! let term = TermRef { taxonomy: "category".into(), slug: "news".into() };
//! assert_eq!(paths.key(&Target::Term { term, page: 2 }).as_str(), "/blog/category/news/page/2/");
//! # Ok::<(), cms_readmodel::PathError>(())
//! ```

mod index;
mod metric;
mod paths;
mod plan;
mod run;
mod theme;
mod time;

pub use metric::emf_line;
pub use paths::Paths;
pub use plan::{Affected, affected};
pub use run::{Projector, Report};
pub use theme::{Native, Renderer};
pub use time::{imf_fixdate, version};

use cms_readmodel::{BlobKey, Route, RouteKey};

/// Most outbox rows one run takes.
pub const BATCH: u32 = 100;

/// A storage or source port failed. The message names the operation, never secrets.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct PortError(pub Box<str>);

/// A run could not finish. The batch's rows stay in the outbox for the next run.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectError {
    /// Reading or deleting outbox rows failed.
    #[error(transparent)]
    Outbox(#[from] cms_outbox::OutboxError),
    /// A port failed.
    #[error("projector port failed: {0}")]
    Port(#[from] PortError),
    /// The card source failed, so a page could not be rendered completely.
    #[error("card source failed: {0}")]
    Cards(#[from] cms_render::CardError),
    /// The shell cannot hold a fragment.
    #[error("page assembly failed: {0}")]
    Assemble(#[from] cms_readmodel::MissingMark),
    /// A loaded post does not fit the site (its permalink is outside the home URL).
    #[error("post {id} permalink {permalink} is outside the site")]
    Outside {
        /// Post id.
        id: u64,
        /// Its permalink.
        permalink: Box<str>,
    },
}

/// A term a post belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TermRef {
    /// Taxonomy name as stored (`category` or `post_tag`).
    pub taxonomy: Box<str>,
    /// Term slug.
    pub slug: Box<str>,
}

/// Whether a post is a dated post or a standalone page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// `post`: listed on home, archives and feeds.
    Post,
    /// `page`: only its permalink and the pages sitemap.
    Page,
}

/// What the projector needs to know about a published post to find the routes it touches.
/// The post doc keeps a copy, so the next change can find the routes it used to touch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostFacts {
    /// Post id.
    pub id: u64,
    /// Post or page.
    pub kind: Kind,
    /// Normalized permalink path, such as `/blog/hello/`.
    pub permalink: Box<str>,
    /// Categories then tags.
    pub terms: Vec<TermRef>,
    /// Author's nicename (archive slug).
    pub author: Box<str>,
    /// Local `post_date` as `(year, month, day)`, which names the date archives.
    pub date: (u16, u8, u8),
    /// `post_date_gmt` in ISO 8601, the term-listing sort key.
    pub date_gmt: Box<str>,
    /// `post_modified_gmt` in ISO 8601.
    pub modified_gmt: Box<str>,
}

/// The published post doc item (`POST#<id>`): the facts plus the body object it was rendered to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostDoc {
    /// The facts the doc was written from.
    pub facts: PostFacts,
    /// Title, plain text.
    pub title: Box<str>,
    /// Excerpt, plain text.
    pub excerpt: Box<str>,
    /// Author display name.
    pub author_name: Box<str>,
    /// The rendered body fragment.
    pub body: BlobKey,
}

/// One row of a term listing (`TERM#<taxonomy>#<slug>` / `<post_date_gmt>#<id>`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Listing {
    /// The term.
    pub term: TermRef,
    /// Sort key `<post_date_gmt>#<id>`, newest first when queried descending.
    pub sort: Box<str>,
    /// The listed post.
    pub post_id: u64,
}

/// One renderable route. Paged targets are planned at page 1; the run renders every page.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    /// A post or page at its permalink.
    Post {
        /// Post id.
        id: u64,
        /// Normalized permalink path.
        path: Box<str>,
    },
    /// Home listing page.
    Home {
        /// 1-based page.
        page: u32,
    },
    /// Term archive page.
    Term {
        /// The term.
        term: TermRef,
        /// 1-based page.
        page: u32,
    },
    /// Year, month or day archive page.
    Date {
        /// Four-digit year.
        year: u16,
        /// Month 1-12, if a month or day archive.
        month: Option<u8>,
        /// Day 1-31, if a day archive.
        day: Option<u8>,
        /// 1-based page.
        page: u32,
    },
    /// Author archive page.
    Author {
        /// Author nicename.
        slug: Box<str>,
        /// 1-based page.
        page: u32,
    },
    /// The site RSS feed.
    Feed,
    /// A term's RSS feed.
    TermFeed(TermRef),
    /// The sitemap index and every shard (one render produces them all).
    Sitemaps,
    /// `robots.txt`.
    Robots,
    /// The not-found page served for unknown paths.
    NotFound,
    /// The prebuilt search index.
    Search,
}

impl Target {
    /// The page number of a paged target.
    #[must_use]
    pub const fn page(&self) -> Option<u32> {
        match self {
            Self::Home { page }
            | Self::Term { page, .. }
            | Self::Date { page, .. }
            | Self::Author { page, .. } => Some(*page),
            _ => None,
        }
    }

    /// The same target at `page`; unpaged targets are returned unchanged.
    #[must_use]
    pub fn at(&self, page: u32) -> Self {
        let mut target = self.clone();
        match &mut target {
            Self::Home { page: at }
            | Self::Term { page: at, .. }
            | Self::Date { page: at, .. }
            | Self::Author { page: at, .. } => *at = page,
            _ => {}
        }
        target
    }
}

/// Loads current state from the source of truth (DSQL through the domain crates).
///
/// The whole published set is loaded once per run. At blog scale (hundreds of posts) one query
/// is cheaper and simpler than per-archive queries, and it gives exact page counts.
pub trait Source {
    /// Site settings from the options table.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the database cannot be read.
    fn site(&mut self) -> Result<cms_render::Site, PortError>;

    /// Every published post and page, in any order. Rows with the zero date are not
    /// published and are left out by the adapter, because `cms_render::Stamp` rejects them.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the database cannot be read.
    fn published(&mut self) -> Result<Vec<cms_render::Post>, PortError>;
}

/// Writes content-addressed objects (`S3` `PutObject` in production).
pub trait BlobWriter {
    /// Stores `bytes` under `key`. The key is the content hash, so a repeat is harmless.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the store fails.
    fn put(&mut self, key: &BlobKey, bytes: &[u8], content_type: &str) -> Result<(), PortError>;
}

/// One read-model item to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A route item.
    Route(RouteKey, Route),
    /// A published post doc.
    Doc(PostDoc),
    /// A term-listing row.
    Listing(Listing),
}

/// The key of one read-model item.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemKey {
    /// `ROUTE#<path>`.
    Route(RouteKey),
    /// `POST#<id>`.
    Doc(u64),
    /// `TERM#<taxonomy>#<slug>` / sort key.
    Listing(Listing),
}

impl Item {
    /// The key this item is stored under.
    #[must_use]
    pub fn key(&self) -> ItemKey {
        match self {
            Self::Route(key, _) => ItemKey::Route(key.clone()),
            Self::Doc(doc) => ItemKey::Doc(doc.facts.id),
            Self::Listing(listing) => ItemKey::Listing(listing.clone()),
        }
    }
}

/// Whether a conditional write took effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The write was applied.
    Written,
    /// The stored version is the same or newer, so the write was skipped.
    Stale,
}

/// Reads and conditionally writes read-model items (`DynamoDB` in production).
///
/// Writes are conditional on `attribute_not_exists(version) OR version < :new`.
pub trait RouteWriter {
    /// The current post doc for `id`, if any.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the table cannot be read or the item is malformed.
    fn post_doc(&mut self, id: u64) -> Result<Option<PostDoc>, PortError>;

    /// Puts `item` at `version` unless a same-or-newer version is stored.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the table fails for any reason other than the condition.
    fn put(&mut self, item: &Item, version: &str) -> Result<Applied, PortError>;

    /// Deletes `key` unless a same-or-newer version is stored. A missing item is
    /// [`Applied::Stale`]: nothing changed, so a replay applies nothing.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the table fails for any reason other than the condition.
    fn delete(&mut self, key: &ItemKey, version: &str) -> Result<Applied, PortError>;
}

/// Invalidates edge-cached paths (`CloudFront` `CreateInvalidation` in production).
pub trait Invalidator {
    /// Invalidates `paths` in one request. Paths may end in `*`.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the request fails.
    fn invalidate(&mut self, paths: &[Box<str>]) -> Result<(), PortError>;
}

/// The projector's time source.
pub trait Clock {
    /// Microseconds since the Unix epoch.
    fn now_micros(&self) -> i64;
}

/// Asks for another projector run (an async self-invoke in production). A run that fills its
/// batch nudges instead of looping, so each invocation stays bounded in time.
pub trait Nudger {
    /// Requests one more run.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the request fails.
    fn nudge(&mut self) -> Result<(), PortError>;
}

#[cfg(test)]
mod tests {
    use super::{Item, ItemKey, PortError, ProjectError, Target, TermRef};

    #[test]
    fn targets_know_their_page() {
        let term = TermRef {
            taxonomy: "category".into(),
            slug: "a".into(),
        };
        let paged = [
            Target::Home { page: 1 },
            Target::Term { term, page: 1 },
            Target::Date {
                year: 2026,
                month: None,
                day: None,
                page: 1,
            },
            Target::Author {
                slug: "a".into(),
                page: 1,
            },
        ];
        for target in paged {
            assert_eq!(target.page(), Some(1));
            assert_eq!(target.at(3).page(), Some(3));
        }
        assert_eq!(Target::Feed.page(), None);
        assert_eq!(Target::Feed.at(2), Target::Feed);
    }

    #[test]
    fn items_and_errors_spell_themselves() {
        let route = cms_readmodel::Route {
            status: 200,
            content_type: "text/html".into(),
            etag: "\"e\"".into(),
            last_modified: "x".into(),
            content: cms_readmodel::Content::Gone,
        };
        let key = cms_readmodel::RouteKey::from("/blog/");
        assert_eq!(Item::Route(key.clone(), route).key(), ItemKey::Route(key));
        assert_eq!(
            ProjectError::from(PortError("s3 put failed".into())).to_string(),
            "projector port failed: s3 put failed"
        );
        let outside = ProjectError::Outside {
            id: 3,
            permalink: "https://other.example/x/".into(),
        };
        assert_eq!(
            outside.to_string(),
            "post 3 permalink https://other.example/x/ is outside the site"
        );
    }
}
