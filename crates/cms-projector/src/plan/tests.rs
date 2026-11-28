//! Affected-route computation for each kind of post change.

use cms_readmodel::BlobKey;

use super::{Affected, affected, site_targets};
use crate::{Kind, Listing, PostDoc, PostFacts, Target, TermRef};

fn term(slug: &str) -> TermRef {
    TermRef {
        taxonomy: "category".into(),
        slug: slug.into(),
    }
}

fn facts(permalink: &str, slug: &str, date: (u16, u8, u8)) -> PostFacts {
    PostFacts {
        id: 7,
        kind: Kind::Post,
        permalink: permalink.into(),
        terms: vec![term(slug)],
        author: "ana".into(),
        date,
        date_gmt: format!("{:04}-{:02}-{:02}T10:00:00+00:00", date.0, date.1, date.2).into(),
        modified_gmt: "2026-11-27T10:00:00+00:00".into(),
    }
}

fn doc(facts: PostFacts) -> PostDoc {
    PostDoc {
        facts,
        title: "T".into(),
        excerpt: "E".into(),
        author_name: "Ana".into(),
        body: BlobKey::from("rendered/old.html"),
    }
}

fn listing(slug: &str, date: &str) -> Listing {
    Listing {
        term: term(slug),
        sort: format!("{date}T10:00:00+00:00#7").into(),
        post_id: 7,
    }
}

fn date(year: u16, month: Option<u8>, day: Option<u8>) -> Target {
    Target::Date {
        year,
        month,
        day,
        page: 1,
    }
}

#[test]
fn a_new_post_touches_its_permalink_listings_feeds_and_sitemaps() {
    let now = facts("/blog/hello/", "news", (2026, 11, 27));
    let out = affected(None, Some(&now));
    let expected: Vec<Target> = vec![
        Target::Post {
            id: 7,
            path: "/blog/hello/".into(),
        },
        Target::Home { page: 1 },
        Target::Term {
            term: term("news"),
            page: 1,
        },
        date(2026, None, None),
        date(2026, Some(11), None),
        date(2026, Some(11), Some(27)),
        Target::Author {
            slug: "ana".into(),
            page: 1,
        },
        Target::Feed,
        Target::TermFeed(term("news")),
        Target::Sitemaps,
        Target::Search,
    ];
    assert_eq!(out.targets, expected.into_iter().collect());
    assert_eq!(out.docs, [7].into());
    assert_eq!(out.listed, [listing("news", "2026-11-27")].into());
    assert!(out.redirects.is_empty() && out.removed.is_empty() && out.unlisted.is_empty());
}

#[test]
fn a_moved_post_redirects_and_rerenders_its_old_archives() {
    let old = doc(facts("/blog/old/", "news", (2025, 1, 2)));
    let now = facts("/blog/new/", "rust", (2026, 11, 27));
    let out = affected(Some(&old), Some(&now));
    assert_eq!(
        out.redirects.get("/blog/old/").map(|to| &**to),
        Some("/blog/new/")
    );
    for target in [
        Target::Term {
            term: term("news"),
            page: 1,
        },
        Target::Term {
            term: term("rust"),
            page: 1,
        },
        date(2025, None, None),
        Target::TermFeed(term("news")),
    ] {
        assert!(out.targets.contains(&target), "{target:?}");
    }
    assert_eq!(out.unlisted, [listing("news", "2025-01-02")].into());
    assert_eq!(out.listed, [listing("rust", "2026-11-27")].into());
}

#[test]
fn an_unchanged_permalink_keeps_its_rows_and_adds_no_redirect() {
    let old = doc(facts("/blog/same/", "news", (2026, 11, 27)));
    let now = facts("/blog/same/", "news", (2026, 11, 27));
    let out = affected(Some(&old), Some(&now));
    assert!(out.redirects.is_empty());
    assert!(out.unlisted.is_empty());
}

#[test]
fn an_unpublished_post_is_removed_with_its_doc_and_rows() {
    let old = doc(facts("/blog/gone/", "news", (2026, 11, 27)));
    let out = affected(Some(&old), None);
    assert_eq!(out.removed, ["/blog/gone/".into()].into());
    assert_eq!(out.dropped, [7].into());
    assert!(out.docs.is_empty() && out.listed.is_empty());
    assert_eq!(out.unlisted, [listing("news", "2026-11-27")].into());
    assert!(out.targets.contains(&Target::Home { page: 1 }));
    assert!(!out.targets.iter().any(|t| matches!(t, Target::Post { .. })));
}

#[test]
fn a_post_never_projected_and_not_public_touches_nothing() {
    assert_eq!(affected(None, None), Affected::default());
}

#[test]
fn a_page_touches_only_its_permalink_sitemaps_and_search() {
    let mut page = facts("/blog/about/", "news", (2026, 11, 27));
    page.kind = Kind::Page;
    let out = affected(None, Some(&page));
    let expected: Vec<Target> = vec![
        Target::Post {
            id: 7,
            path: "/blog/about/".into(),
        },
        Target::Sitemaps,
        Target::Search,
    ];
    assert_eq!(out.targets, expected.into_iter().collect());
    assert!(out.listed.is_empty());
}

#[test]
fn merging_unions_every_part() {
    let mut first = affected(None, Some(&facts("/blog/a/", "news", (2026, 11, 27))));
    let second = affected(Some(&doc(facts("/blog/b/", "x", (2026, 11, 27)))), None);
    first.merge(second);
    assert!(first.removed.contains("/blog/b/"));
    assert!(first.docs.contains(&7));
    assert!(first.dropped.contains(&7));
    assert!(site_targets().contains(&Target::NotFound));
}
