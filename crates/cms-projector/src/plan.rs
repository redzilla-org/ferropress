//! The affected route set for one post change.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Kind, Listing, PostDoc, PostFacts, Target};

/// Everything one or more post changes touch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Affected {
    /// Targets to render (paged targets at page 1).
    pub targets: BTreeSet<Target>,
    /// Old permalinks that now redirect, mapped to the new permalink.
    pub redirects: BTreeMap<Box<str>, Box<str>>,
    /// Permalinks of posts that are no longer public; their route items are deleted.
    pub removed: BTreeSet<Box<str>>,
    /// Post docs to write once their bodies are rendered.
    pub docs: BTreeSet<u64>,
    /// Post docs to delete.
    pub dropped: BTreeSet<u64>,
    /// Term-listing rows to put.
    pub listed: BTreeSet<Listing>,
    /// Term-listing rows to delete.
    pub unlisted: BTreeSet<Listing>,
}

impl Affected {
    /// Adds everything in `other`.
    pub fn merge(&mut self, other: Self) {
        self.targets.extend(other.targets);
        self.redirects.extend(other.redirects);
        self.removed.extend(other.removed);
        self.docs.extend(other.docs);
        self.dropped.extend(other.dropped);
        self.listed.extend(other.listed);
        self.unlisted.extend(other.unlisted);
    }
}

/// The routes a post change affects, given the doc written by the previous projection
/// (`previous`) and the post's current published state (`current`, `None` when not public).
///
/// Both sides contribute: the old permalink, terms, dates and author must be re-rendered too,
/// or archives would keep listing a post that moved away.
#[must_use]
pub fn affected(previous: Option<&PostDoc>, current: Option<&PostFacts>) -> Affected {
    let mut out = Affected::default();
    let old = previous.map(|doc| &doc.facts);
    for facts in old.into_iter().chain(current) {
        listing_targets(&mut out.targets, facts);
    }
    if let Some(now) = current {
        out.targets.insert(Target::Post {
            id: now.id,
            path: now.permalink.clone(),
        });
        out.docs.insert(now.id);
        out.listed.extend(listings(now));
        // A changed slug or date keeps the old URL working (the reference's old-slug redirect).
        if let Some(old) = old.filter(|old| old.permalink != now.permalink) {
            out.redirects
                .insert(old.permalink.clone(), now.permalink.clone());
        }
    } else if let Some(old) = old {
        out.removed.insert(old.permalink.clone());
        out.dropped.insert(old.id);
    }
    // Rows the post no longer has: removed terms, or every row when the sort key moved.
    if let Some(old) = old {
        let kept: BTreeSet<Listing> = current.map(listings).unwrap_or_default().into_iter().collect();
        out.unlisted
            .extend(listings(old).into_iter().filter(|row| !kept.contains(row)));
    }
    out
}

/// The site-wide targets every rebuild renders besides the posts themselves.
#[must_use]
pub(crate) fn site_targets() -> BTreeSet<Target> {
    [
        Target::Home { page: 1 },
        Target::Feed,
        Target::Sitemaps,
        Target::Robots,
        Target::NotFound,
        Target::Search,
    ]
    .into_iter()
    .collect()
}

// Listings, feeds, sitemaps and search that show `facts`.
fn listing_targets(targets: &mut BTreeSet<Target>, facts: &PostFacts) {
    targets.extend([Target::Sitemaps, Target::Search]);
    if facts.kind == Kind::Page {
        return;
    }
    let (year, month, day) = facts.date;
    targets.extend([
        Target::Home { page: 1 },
        Target::Feed,
        Target::Author {
            slug: facts.author.clone(),
            page: 1,
        },
        Target::Date {
            year,
            month: None,
            day: None,
            page: 1,
        },
        Target::Date {
            year,
            month: Some(month),
            day: None,
            page: 1,
        },
        Target::Date {
            year,
            month: Some(month),
            day: Some(day),
            page: 1,
        },
    ]);
    for term in &facts.terms {
        targets.extend([
            Target::Term {
                term: term.clone(),
                page: 1,
            },
            Target::TermFeed(term.clone()),
        ]);
    }
}

fn listings(facts: &PostFacts) -> Vec<Listing> {
    if facts.kind == Kind::Page {
        return Vec::new();
    }
    let sort: Box<str> = format!("{}#{}", facts.date_gmt, facts.id).into();
    facts
        .terms
        .iter()
        .map(|term| Listing {
            term: term.clone(),
            sort: sort.clone(),
            post_id: facts.id,
        })
        .collect()
}

#[cfg(test)]
mod tests;
