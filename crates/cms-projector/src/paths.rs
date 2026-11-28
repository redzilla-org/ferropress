//! Route keys for targets, at the reference's default rewrite layout under the base path.

use std::fmt::Write as _;

use cms_readmodel::{BasePath, RouteKey};

use crate::{Target, TermRef};

/// Builds route keys under one base path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    base: BasePath,
}

impl Paths {
    /// Paths under `base`.
    #[must_use]
    pub const fn new(base: BasePath) -> Self {
        Self { base }
    }

    /// The base path.
    #[must_use]
    pub const fn base(&self) -> &BasePath {
        &self.base
    }

    /// The route key `target` is served under. For [`Target::Sitemaps`] that is the index; the
    /// shards are keyed with [`Self::file`].
    #[must_use]
    pub fn key(&self, target: &Target) -> RouteKey {
        let base = self.base.as_str();
        let path = match target {
            Target::Post { path, .. } => path.to_string(),
            Target::Home { page } => paged(base.to_owned(), *page),
            Target::Term { term, page } => paged(term_root(base, term), *page),
            Target::Date {
                year,
                month,
                day,
                page,
            } => {
                let mut path = format!("{base}{year:04}/");
                // Writing into a String cannot fail.
                if let Some(month) = month {
                    let _ = write!(path, "{month:02}/");
                }
                if let Some(day) = day {
                    let _ = write!(path, "{day:02}/");
                }
                paged(path, *page)
            }
            Target::Author { slug, page } => paged(format!("{base}author/{slug}/"), *page),
            Target::Feed => format!("{base}feed/"),
            Target::TermFeed(term) => format!("{}feed/", term_root(base, term)),
            Target::Sitemaps => format!("{base}{}", cms_render::SITEMAP_INDEX),
            Target::Robots => format!("{base}robots.txt"),
            Target::NotFound => format!("{base}404"),
            // Not a public path: the search Lambda reads this item to find the current index.
            Target::Search => "SEARCH".to_owned(),
        };
        RouteKey::from(path.as_str())
    }

    /// The route key of a file named relative to the base path, such as a sitemap shard.
    #[must_use]
    pub fn file(&self, relative: &str) -> RouteKey {
        RouteKey::from(format!("{}{relative}", self.base.as_str()).as_str())
    }

    /// The edge path that invalidates everything under the base path.
    #[must_use]
    pub fn wildcard(&self) -> Box<str> {
        format!("{}*", self.base.as_str()).into()
    }
}

// The reference's default bases: `category` and `tag`; other taxonomies use their own name.
fn term_root(base: &str, term: &TermRef) -> String {
    let segment = match &*term.taxonomy {
        "post_tag" => "tag",
        other => other,
    };
    format!("{base}{segment}/{}/", term.slug)
}

// Page 1 is the listing's own path; later pages add `page/<n>/`.
fn paged(mut path: String, page: u32) -> String {
    if page > 1 {
        let _ = write!(path, "page/{page}/");
    }
    path
}

#[cfg(test)]
mod tests {
    use cms_readmodel::{BasePath, PathError};

    use super::Paths;
    use crate::{Target, TermRef};

    fn term(taxonomy: &str, slug: &str) -> TermRef {
        TermRef {
            taxonomy: taxonomy.into(),
            slug: slug.into(),
        }
    }

    #[test]
    fn every_target_has_its_reference_path() -> Result<(), PathError> {
        let paths = Paths::new(BasePath::new("/blog/")?);
        let cases = [
            (
                Target::Post {
                    id: 1,
                    path: "/blog/hello/".into(),
                },
                "/blog/hello/",
            ),
            (Target::Home { page: 1 }, "/blog/"),
            (Target::Home { page: 3 }, "/blog/page/3/"),
            (
                Target::Term {
                    term: term("category", "news"),
                    page: 1,
                },
                "/blog/category/news/",
            ),
            (
                Target::Term {
                    term: term("post_tag", "rust"),
                    page: 2,
                },
                "/blog/tag/rust/page/2/",
            ),
            (
                Target::Term {
                    term: term("genre", "jazz"),
                    page: 1,
                },
                "/blog/genre/jazz/",
            ),
            (
                Target::Date {
                    year: 2026,
                    month: None,
                    day: None,
                    page: 1,
                },
                "/blog/2026/",
            ),
            (
                Target::Date {
                    year: 2026,
                    month: Some(3),
                    day: Some(7),
                    page: 2,
                },
                "/blog/2026/03/07/page/2/",
            ),
            (
                Target::Author {
                    slug: "ana".into(),
                    page: 1,
                },
                "/blog/author/ana/",
            ),
            (Target::Feed, "/blog/feed/"),
            (Target::TermFeed(term("post_tag", "rust")), "/blog/tag/rust/feed/"),
            (Target::Sitemaps, "/blog/wp-sitemap.xml"),
            (Target::Robots, "/blog/robots.txt"),
            (Target::NotFound, "/blog/404"),
            (Target::Search, "SEARCH"),
        ];
        for (target, expected) in cases {
            assert_eq!(paths.key(&target).as_str(), expected, "{target:?}");
        }
        assert_eq!(
            paths.file("wp-sitemap-users-1.xml").as_str(),
            "/blog/wp-sitemap-users-1.xml"
        );
        assert_eq!(&*paths.wildcard(), "/blog/*");
        assert_eq!(paths.base().as_str(), "/blog/");
        Ok(())
    }
}
