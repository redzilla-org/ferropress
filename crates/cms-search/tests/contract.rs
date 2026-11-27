//! Contract cases for search matching and ordering, one per rule of the reference's search query
//! (term splitting, stopwords, exclusions, AND across title/excerpt/content, relevance ranks, then
//! newest first, ten per page). Ids are listed in expected result order.

use cms_search::{Document, Index, PER_PAGE, Terms, build};

// Day `n` of November 2026: a higher n is newer.
fn doc(id: u64, day: u32, title: &str, excerpt: &str, content: &str) -> Document {
    Document::new(
        id,
        "post",
        &format!("2026-11-{day:02} 09:00:00"),
        &format!("/blog/p{id}/"),
        title,
        excerpt,
        content,
    )
}

// A build or decode failure yields an empty index, which every case below would detect.
fn index(docs: Vec<Document>) -> Index {
    build(docs)
        .and_then(|built| Index::from_slice(&built.bytes))
        .unwrap_or_else(|_failed| Index::empty())
}

fn ids(index: &Index, search: &str) -> Vec<u64> {
    index
        .search(&Terms::parse(search), 1, PER_PAGE)
        .hits
        .iter()
        .map(|doc| doc.id)
        .collect()
}

// One document per relevance rank for "red boat"; the better the rank, the older the post, so
// any rank mistake shows up as date order instead.
fn ranked() -> Index {
    index(vec![
        doc(1, 1, "A red boat trip", "", "x"),
        doc(2, 2, "Boat painted red", "", "x"),
        doc(3, 3, "Red sky", "", "a boat"),
        doc(4, 4, "Harbor", "one red boat", "x"),
        doc(5, 5, "News", "", "<p>the red boat sank</p>"),
        doc(6, 6, "Misc", "", "boat that is red"),
        doc(7, 7, "Unrelated", "", "a red car"),
    ])
}

#[test]
fn several_terms_rank_sentence_then_title_terms_then_excerpt_then_content() {
    assert_eq!(ids(&ranked(), "red boat"), [1, 2, 3, 4, 5, 6]);
}

#[test]
fn one_term_ranks_title_matches_first_then_newest() {
    assert_eq!(ids(&ranked(), "boat"), [2, 1, 6, 5, 4, 3]);
    assert_eq!(ids(&ranked(), "red"), [3, 2, 1, 7, 6, 5, 4]);
}

#[test]
fn matching_is_case_insensitive() {
    assert_eq!(ids(&ranked(), "RED BOAT"), [1, 2, 3, 4, 5, 6]);
}

#[test]
fn a_quoted_phrase_is_one_term() {
    assert_eq!(ids(&ranked(), r#""red boat""#), [1, 5, 4]);
}

#[test]
fn excluded_terms_filter_and_disable_sentence_ranks() {
    // Every other red post mentions a boat somewhere, the content included.
    assert_eq!(ids(&ranked(), "red -boat"), [7]);
    // 1 would rank first on its sentence; with a dash, title terms decide, then date.
    let index = index(vec![
        doc(1, 1, "red boat", "", "x"),
        doc(2, 2, "boat red", "", "x"),
        doc(3, 3, "red", "", "boat"),
    ]);
    assert_eq!(ids(&index, "red boat -zzz"), [2, 1, 3]);
    assert_eq!(ids(&index, "red boat"), [1, 2, 3]);
    // A dash inside a word is not an exclusion and keeps sentence ranks.
    assert_eq!(ids(&index, "red boat x-y"), Vec::<u64>::new());
}

#[test]
fn stopwords_drop_out_but_stay_in_the_sentence() {
    let index = index(vec![
        doc(1, 1, "The red one", "", "x"),
        doc(2, 2, "Red", "", "x"),
        doc(3, 3, "Other", "", "see the red"),
        doc(4, 4, "Other", "", "red"),
    ]);
    // "the" is dropped as a filter: 4 matches without it. Sentence "the red" still ranks 1 and 3.
    assert_eq!(ids(&index, "the red"), [1, 2, 3, 4]);
}

#[test]
fn only_stopwords_search_the_whole_sentence() {
    let index = index(vec![
        doc(1, 1, "Other", "", "x"),
        doc(2, 2, "Plain", "", "x"),
        doc(3, 3, "Plain", "", "in the end"),
    ]);
    assert_eq!(ids(&index, "the"), [1, 3]);
    assert_eq!(ids(&index, "in the"), [3]);
}

#[test]
fn seven_or_more_terms_skip_title_term_ranks() {
    let index = index(vec![
        doc(1, 1, "a1 a2 a3 a4 a5 a6 a7", "", "x"),
        doc(2, 2, "z", "", "a7 a6 a5 a4 a3 a2 a1"),
    ]);
    assert_eq!(ids(&index, "a1 a2 a3 a4 a5 a6 a7"), [1, 2]);
    assert_eq!(ids(&index, "a7 a6 a5 a4 a3 a2 a1"), [2, 1]);
    assert_eq!(ids(&index, "a2 a1"), [1, 2]);
}

#[test]
fn an_empty_search_lists_everything_newest_first() {
    assert_eq!(ids(&ranked(), ""), [7, 6, 5, 4, 3, 2, 1]);
}

#[test]
fn zero_is_searched_without_relevance() {
    let index = index(vec![
        doc(1, 1, "Top 10", "", "x"),
        doc(2, 2, "Plain", "", "100 ways"),
        doc(3, 3, "Plain", "", "none"),
    ]);
    assert_eq!(ids(&index, "0"), [2, 1]);
}

#[test]
fn markup_is_not_searchable_content() {
    let index = index(vec![doc(1, 1, "T", "", "<!-- wp:paragraph --><p>hi</p>")]);
    assert!(ids(&index, "paragraph").is_empty());
    assert_eq!(ids(&index, "hi"), [1]);
}

#[test]
fn results_page_ten_at_a_time() {
    let docs = (1..=23)
        .map(|n| doc(n, u32::try_from(n).unwrap_or(1), "Post", "", "word"))
        .collect();
    let index = index(docs);
    let terms = Terms::parse("word");
    let first = index.search(&terms, 1, PER_PAGE);
    assert_eq!((first.total, first.pages, first.hits.len()), (23, 3, 10));
    assert_eq!(first.hits[0].id, 23);
    let third = index.search(&terms, 3, PER_PAGE);
    assert_eq!(
        third.hits.iter().map(|d| d.id).collect::<Vec<_>>(),
        [3, 2, 1]
    );
    assert!(index.search(&terms, 4, PER_PAGE).hits.is_empty());
    assert_eq!(index.search(&terms, 0, PER_PAGE).hits[0].id, 23);
    assert_eq!(index.search(&terms, 1, 0).hits.len(), 1);
}

#[test]
fn same_date_breaks_ties_by_newer_id() {
    let index = index(vec![doc(1, 1, "w", "", ""), doc(2, 1, "w", "", "")]);
    assert_eq!(ids(&index, "w"), [2, 1]);
}

#[test]
fn builds_are_deterministic_and_content_addressed() {
    let a = build(vec![doc(2, 2, "b", "", ""), doc(1, 1, "a", "", "")]).expect("builds");
    let b = build(vec![doc(1, 1, "a", "", ""), doc(2, 2, "b", "", "")]).expect("builds");
    assert_eq!(a, b);
    assert_eq!(a.key.len(), "search/".len() + 64 + ".json".len());
    let c = build(vec![doc(1, 1, "a", "", "")]).expect("builds");
    assert_ne!(a.key, c.key);
    assert!(String::from_utf8_lossy(&a.bytes).starts_with(r#"{"v":1,"docs":[{"i":1,"k":"post""#));
}
