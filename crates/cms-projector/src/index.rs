//! The published set in memory: facts per post, and rendering of each target with `cms-render`.

use std::collections::{BTreeMap, HashMap};

use cms_readmodel::RouteKey;
use cms_render::{
    Archive, ArchiveKind, CardSource, Channel, FEED_ITEMS, Post, PostKind, Rendered, Site,
    SitemapUrls, Term,
};
use serde_json::json;

use crate::{Kind, Paths, PostFacts, ProjectError, Renderer, Target, TermRef};

const CATEGORY: &str = "category";
const TAG: &str = "post_tag";
const JSON: &str = "application/json; charset=UTF-8";

/// What rendering one target produced.
#[derive(Debug)]
pub(crate) enum Rendering {
    /// An HTML fragment for the shell.
    Page {
        fragment: Rendered,
        /// Card markers the source had no card for.
        unresolved: usize,
    },
    /// A complete object.
    Object { body: Rendered, unresolved: usize },
    /// Several objects at their own keys (the sitemap index and shards).
    Files(Vec<(RouteKey, Rendered)>),
    /// The route has nothing to show and its item is deleted.
    Missing,
}

/// Every published post, newest first, with the facts the planner reads.
pub(crate) struct Index<'a> {
    site: &'a Site,
    paths: &'a Paths,
    theme: &'a dyn Renderer,
    posts: Vec<Post>,
    facts: HashMap<u64, PostFacts>,
}

impl<'a> Index<'a> {
    /// Indexes `posts` for `site`, to be rendered with `theme`.
    pub(crate) fn new(
        site: &'a Site,
        paths: &'a Paths,
        theme: &'a dyn Renderer,
        mut posts: Vec<Post>,
    ) -> Result<Self, ProjectError> {
        // Newest first, ties by higher id, as the reference orders listings.
        posts.sort_by(|a, b| (b.date_gmt, b.id).cmp(&(a.date_gmt, a.id)));
        let facts = posts
            .iter()
            .map(|post| facts(site, paths, post).map(|facts| (post.id, facts)))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            site,
            paths,
            theme,
            posts,
            facts,
        })
    }

    pub(crate) fn facts(&self, id: u64) -> Option<&PostFacts> {
        self.facts.get(&id)
    }

    pub(crate) fn post(&self, id: u64) -> Option<&Post> {
        self.posts.iter().find(|post| post.id == id)
    }

    pub(crate) fn ids(&self) -> Vec<u64> {
        self.posts.iter().map(|post| post.id).collect()
    }

    /// Pages a paged target has now; 0 when it shows nothing (and is removed).
    pub(crate) fn pages(&self, target: &Target) -> u32 {
        self.listing(target).map_or(0, |(_, items)| {
            cms_render::page_count(items.len(), self.site.posts_per_page)
        })
    }

    pub(crate) fn render(&self, target: &Target, cards: &mut dyn CardSource) -> Result<Rendering, ProjectError> {
        let site = self.site;
        let rendering = match target {
            Target::Post { id, .. } => match self.post(*id) {
                Some(post) => {
                    let out = self.theme.single(site, post, cards)?;
                    Rendering::Page {
                        fragment: out.rendered,
                        unresolved: out.unresolved.len(),
                    }
                }
                None => Rendering::Missing,
            },
            Target::Home { page }
            | Target::Term { page, .. }
            | Target::Date { page, .. }
            | Target::Author { page, .. } => self.archive(target, *page),
            Target::Feed => {
                let items = self.newest(self.of_kind(Kind::Post));
                let url = self.url(&Target::Feed);
                self.feed(None, &site.home, &url, &items, cards)?
            }
            Target::TermFeed(term) => {
                let page_one = Target::Term {
                    term: term.clone(),
                    page: 1,
                };
                match self.listing(&page_one) {
                    Some((ArchiveKind::Category(found) | ArchiveKind::Tag(found), items)) => {
                        let items = self.newest(items);
                        let (link, url) = (self.url(&page_one), self.url(target));
                        self.feed(Some(&found.name), &link, &url, &items, cards)?
                    }
                    _ => Rendering::Missing,
                }
            }
            Target::Sitemaps => Rendering::Files(
                self.theme.sitemaps(site, &self.sitemap_urls())
                    .into_iter()
                    .map(|file| (self.paths.file(&file.path), file.rendered))
                    .collect(),
            ),
            Target::Robots => Rendering::Object {
                body: self.theme.robots(site),
                unresolved: 0,
            },
            Target::NotFound => Rendering::Page {
                fragment: self.theme.not_found(site),
                unresolved: 0,
            },
            Target::Search => Rendering::Object {
                body: self.search(),
                unresolved: 0,
            },
        };
        Ok(rendering)
    }

    fn archive(&self, target: &Target, page: u32) -> Rendering {
        let Some((kind, items)) = self.listing(target) else {
            return Rendering::Missing;
        };
        let per_page = self.site.posts_per_page.max(1);
        let pages = cms_render::page_count(items.len(), per_page);
        if page == 0 || page > pages {
            return Rendering::Missing;
        }
        let skip = usize::try_from((page - 1) * per_page).unwrap_or(usize::MAX);
        let take = usize::try_from(per_page).unwrap_or(usize::MAX);
        let shown: Vec<Post> = items.into_iter().skip(skip).take(take).cloned().collect();
        let url = self.url(&target.at(1));
        let fragment = self.theme.archive(
            self.site,
            &Archive {
                kind: &kind,
                url: &url,
                page,
                pages,
                items: &shown,
            },
        );
        Rendering::Page {
            fragment,
            unresolved: 0,
        }
    }

    fn feed(
        &self,
        name: Option<&str>,
        link: &str,
        self_url: &str,
        items: &[Post],
        cards: &mut dyn CardSource,
    ) -> Result<Rendering, ProjectError> {
        let channel = Channel {
            name,
            link,
            self_url,
            items,
        };
        let out = self.theme.feed(self.site, &channel, cards)?;
        Ok(Rendering::Object {
            body: out.rendered,
            unresolved: out.unresolved.len(),
        })
    }

    // The posts a listing target shows, newest first, with the archive kind naming it. A term,
    // date or author with no published posts shows nothing; the home page always renders.
    fn listing(&self, target: &Target) -> Option<(ArchiveKind, Vec<&Post>)> {
        let posts = self.of_kind(Kind::Post);
        let (kind, items): (Option<ArchiveKind>, Vec<&Post>) = match target {
            Target::Home { .. } => (Some(ArchiveKind::Home), posts),
            Target::Term { term, .. } => {
                let items: Vec<&Post> = posts
                    .into_iter()
                    .filter(|post| terms_of(post, &term.taxonomy).iter().any(|t| t.slug == term.slug))
                    .collect();
                let found = items.first().and_then(|post| {
                    terms_of(post, &term.taxonomy)
                        .iter()
                        .find(|t| t.slug == term.slug)
                        .cloned()
                });
                let kind = found.map(|found| {
                    if &*term.taxonomy == TAG {
                        ArchiveKind::Tag(found)
                    } else {
                        ArchiveKind::Category(found)
                    }
                });
                (kind, items)
            }
            Target::Date {
                year, month, day, ..
            } => {
                let items: Vec<&Post> = posts
                    .into_iter()
                    .filter(|post| {
                        post.date.year() == *year
                            && month.is_none_or(|m| post.date.month() == m)
                            && day.is_none_or(|d| post.date.day() == d)
                    })
                    .collect();
                let kind = ArchiveKind::Date {
                    year: *year,
                    month: *month,
                    day: *day,
                };
                (Some(kind).filter(|_| !items.is_empty()), items)
            }
            Target::Author { slug, .. } => {
                let items: Vec<&Post> = posts
                    .into_iter()
                    .filter(|post| post.author.slug == *slug)
                    .collect();
                let kind = items
                    .first()
                    .map(|post| ArchiveKind::Author(post.author.clone()));
                (kind, items)
            }
            _ => (None, Vec::new()),
        };
        kind.map(|kind| (kind, items))
    }

    fn of_kind(&self, kind: Kind) -> Vec<&Post> {
        self.posts
            .iter()
            .filter(|post| kind_of(post) == kind)
            .collect()
    }

    fn newest(&self, items: Vec<&Post>) -> Vec<Post> {
        items.into_iter().take(FEED_ITEMS).cloned().collect()
    }

    // Absolute URL of a target: the home URL plus the key below the base path.
    fn url(&self, target: &Target) -> String {
        let key = self.paths.key(target);
        let relative = key
            .as_str()
            .strip_prefix(self.paths.base().as_str())
            .unwrap_or_default();
        self.site.url(relative)
    }

    fn sitemap_urls(&self) -> SitemapUrls {
        // Maps keyed by slug keep each archive once and the order stable across runs.
        let mut categories = BTreeMap::new();
        let mut tags = BTreeMap::new();
        let mut users = BTreeMap::new();
        let mut urls = SitemapUrls {
            posts: Vec::new(),
            pages: Vec::new(),
            categories: Vec::new(),
            tags: Vec::new(),
            users: Vec::new(),
        };
        for post in &self.posts {
            match post.kind {
                PostKind::Page => urls.pages.push(post.permalink.clone()),
                PostKind::Post => {
                    urls.posts.push(post.permalink.clone());
                    for term in &post.categories {
                        categories.insert(term.slug.clone(), term.link.clone());
                    }
                    for term in &post.tags {
                        tags.insert(term.slug.clone(), term.link.clone());
                    }
                    users.insert(post.author.slug.clone(), post.author.link.clone());
                }
            }
        }
        urls.categories = categories.into_values().collect();
        urls.tags = tags.into_values().collect();
        urls.users = users.into_values().collect();
        urls
    }

    // The prebuilt search index: one entry per published post and page, newest first.
    fn search(&self) -> Rendered {
        let entries: Vec<serde_json::Value> = self
            .posts
            .iter()
            .map(|post| {
                json!({
                    "id": post.id,
                    "title": &*post.title,
                    "url": &*post.permalink,
                    "date": post.date_gmt.iso8601(post.date_gmt),
                    "excerpt": cms_render::excerpt(post),
                })
            })
            .collect();
        Rendered {
            bytes: serde_json::Value::Array(entries).to_string().into_bytes(),
            content_type: JSON,
        }
    }
}

fn kind_of(post: &Post) -> Kind {
    match post.kind {
        PostKind::Post => Kind::Post,
        PostKind::Page => Kind::Page,
    }
}

fn terms_of<'p>(post: &'p Post, taxonomy: &str) -> &'p [Term] {
    match taxonomy {
        CATEGORY => &post.categories,
        TAG => &post.tags,
        _ => &[],
    }
}

// The facts of one loaded post; its permalink must sit under the site's home URL.
fn facts(site: &Site, paths: &Paths, post: &Post) -> Result<PostFacts, ProjectError> {
    let relative = post
        .permalink
        .strip_prefix(&*site.home)
        .ok_or_else(|| ProjectError::Outside {
            id: post.id,
            permalink: post.permalink.clone(),
        })?;
    let tagged = |taxonomy: &str, terms: &[Term]| -> Vec<TermRef> {
        terms
            .iter()
            .map(|term| TermRef {
                taxonomy: taxonomy.into(),
                slug: term.slug.clone(),
            })
            .collect()
    };
    let mut terms = tagged(CATEGORY, &post.categories);
    terms.extend(tagged(TAG, &post.tags));
    Ok(PostFacts {
        id: post.id,
        kind: kind_of(post),
        permalink: paths.file(relative).as_str().into(),
        terms,
        author: post.author.slug.clone(),
        date: (post.date.year(), post.date.month(), post.date.day()),
        date_gmt: post.date_gmt.iso8601(post.date_gmt).into(),
        modified_gmt: post.modified_gmt.iso8601(post.modified_gmt).into(),
    })
}
