//! The JSON index: building it, decoding it and answering queries from it.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Document, Terms};

/// Route key of the item naming the current index object (`Content::Object { body }`).
pub const INDEX_ROUTE: &str = "SEARCH";

/// Results per page: the reference's default posts-per-page.
pub const PER_PAGE: usize = 10;

// Bumped whenever the JSON layout changes, so a Lambda never misreads an older index.
const FORMAT: u32 = 1;

// Sanity limit after which the reference stops ranking by title terms.
const RANKED_TERMS: usize = 7;

/// A built index: its bytes and the content-addressed key to store them under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    /// `search/<sha256 hex>.json`.
    pub key: String,
    /// The JSON index.
    pub bytes: Vec<u8>,
}

/// The index bytes could not be built or read.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IndexError {
    /// The bytes are not a JSON index.
    #[error("search index is not valid JSON: {0}")]
    Json(Box<str>),
    /// The index was written in another format version.
    #[error("search index format {0} is not supported")]
    Format(u32),
}

#[derive(Serialize, Deserialize)]
struct Stored {
    v: u32,
    docs: Vec<Document>,
}

/// Builds the index for `docs` (published, unprotected posts the projector selected).
///
/// Documents are ordered by id, so the same posts always give the same bytes and key.
///
/// # Errors
///
/// [`IndexError::Json`] if serialization fails.
pub fn build(mut docs: Vec<Document>) -> Result<Built, IndexError> {
    docs.sort_by_key(|doc| doc.id);
    let bytes = serde_json::to_vec(&Stored { v: FORMAT, docs }).map_err(json)?;
    let key = Sha256::digest(&bytes)
        .iter()
        .fold(String::from("search/"), |mut key, byte| {
            // Writing to a String cannot fail.
            let _ = write!(key, "{byte:02x}");
            key
        });
    Ok(Built {
        key: key + ".json",
        bytes,
    })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a map_err callback, which passes the error by value"
)]
fn json(error: serde_json::Error) -> IndexError {
    IndexError::Json(error.to_string().into())
}

// A document with lower-cased copies of the matched fields, computed once per load.
#[derive(Debug)]
struct Entry {
    doc: Document,
    title: String,
    excerpt: String,
    content: String,
}

impl Entry {
    fn has(&self, needle: &str) -> bool {
        self.title.contains(needle)
            || self.excerpt.contains(needle)
            || self.content.contains(needle)
    }
}

/// A decoded index, newest document first.
#[derive(Debug)]
pub struct Index {
    entries: Vec<Entry>,
}

/// One page of results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<'a> {
    /// The documents on this page, in result order.
    pub hits: Vec<&'a Document>,
    /// How many documents match in all.
    pub total: usize,
    /// How many pages the matches fill.
    pub pages: usize,
}

impl Index {
    /// Decodes index bytes written by [`build`].
    ///
    /// # Errors
    ///
    /// [`IndexError::Json`] for malformed bytes, [`IndexError::Format`] for another format version.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, IndexError> {
        let stored: Stored = serde_json::from_slice(bytes).map_err(json)?;
        if stored.v != FORMAT {
            return Err(IndexError::Format(stored.v));
        }
        let mut entries: Vec<Entry> = stored
            .docs
            .into_iter()
            .map(|doc| Entry {
                title: doc.title.to_lowercase(),
                excerpt: doc.excerpt.to_lowercase(),
                content: doc.content.to_lowercase(),
                doc,
            })
            .collect();
        // Date descending is the reference's order after relevance; id descending breaks ties.
        entries.sort_by(|a, b| (&b.doc.date, b.doc.id).cmp(&(&a.doc.date, a.doc.id)));
        Ok(Self { entries })
    }

    /// An index with no documents, for a site that has never been projected.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// How many documents the index holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index holds no documents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Page `page` (from 1; 0 counts as 1) of `per_page` results (at least 1) for `terms`.
    ///
    /// Matching is case-insensitive substring search: each included term must occur in the title,
    /// excerpt or content, and each excluded term in none of them. An empty search matches every
    /// document. See [`Terms`] for how the string is split and the crate docs for the ordering.
    #[must_use]
    pub fn search(&self, terms: &Terms, page: usize, per_page: usize) -> Page<'_> {
        let filter = Filter::new(terms);
        let mut matched: Vec<&Entry> = self.entries.iter().filter(|e| filter.matches(e)).collect();
        if let Some(rank) = Rank::new(terms, &filter) {
            // Stable, so equal ranks keep the date order the entries are stored in.
            matched.sort_by_key(|entry| rank.position(entry));
        }
        let per_page = per_page.max(1);
        let skip = (page.max(1) - 1).saturating_mul(per_page);
        Page {
            total: matched.len(),
            pages: matched.len().div_ceil(per_page),
            hits: matched
                .into_iter()
                .skip(skip)
                .take(per_page)
                .map(|entry| &entry.doc)
                .collect(),
        }
    }
}

// Lower-cased included and excluded terms.
struct Filter {
    all: bool,
    include: Vec<String>,
    exclude: Vec<String>,
}

impl Filter {
    fn new(terms: &Terms) -> Self {
        let mut filter = Self {
            all: terms.is_empty(),
            include: Vec::new(),
            exclude: Vec::new(),
        };
        for term in terms.terms() {
            match term.strip_prefix('-') {
                Some(excluded) => filter.exclude.push(excluded.to_lowercase()),
                None => filter.include.push(term.to_lowercase()),
            }
        }
        filter
    }

    fn matches(&self, entry: &Entry) -> bool {
        self.all
            || (self.include.iter().all(|term| entry.has(term))
                && !self.exclude.iter().any(|term| entry.has(term)))
    }
}

// The relevance ordering; lower ranks come first.
enum Rank<'a> {
    // One piece: title matches of the term first.
    Single(&'a str),
    // Several pieces: sentence and term positions, ranks 1 to 6.
    Many {
        sentence: Option<String>,
        include: &'a [String],
    },
}

impl<'a> Rank<'a> {
    // `None` when the reference applies no relevance order: no included term, or a falsy `s`.
    fn new(terms: &Terms, filter: &'a Filter) -> Option<Self> {
        let first = filter.include.first()?;
        if terms.is_empty() || terms.sentence() == "0" {
            return None;
        }
        if terms.raw_count() <= 1 {
            return Some(Self::Single(first));
        }
        // A dash at the start or after whitespace means exclusions: no sentence ranks then.
        let bytes = terms.sentence().as_bytes();
        let dashed = bytes
            .iter()
            .enumerate()
            .any(|(at, &b)| b == b'-' && (at == 0 || is_space(bytes[at - 1])));
        Some(Self::Many {
            sentence: (!dashed).then(|| terms.sentence().to_lowercase()),
            include: &filter.include,
        })
    }

    fn position(&self, entry: &Entry) -> u8 {
        match self {
            Self::Single(term) => u8::from(!entry.title.contains(term)),
            Self::Many { sentence, include } => {
                let sentence = sentence.as_deref();
                let ranked = include.len() < RANKED_TERMS;
                if sentence.is_some_and(|s| entry.title.contains(s)) {
                    1
                } else if ranked && include.iter().all(|t| entry.title.contains(t.as_str())) {
                    2
                } else if ranked
                    && include.len() > 1
                    && include.iter().any(|t| entry.title.contains(t.as_str()))
                {
                    3
                } else if sentence.is_some_and(|s| entry.excerpt.contains(s)) {
                    4
                } else if sentence.is_some_and(|s| entry.content.contains(s)) {
                    5
                } else {
                    6
                }
            }
        }
    }
}

// The whitespace class of the reference's dash check.
const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c')
}

#[cfg(test)]
mod tests {
    use super::{Index, IndexError, build};

    #[test]
    fn malformed_and_foreign_indexes_are_errors() {
        assert!(matches!(
            Index::from_slice(b"nope"),
            Err(IndexError::Json(_))
        ));
        assert_eq!(
            Index::from_slice(br#"{"v":2,"docs":[]}"#).map(|i| i.len()),
            Err(IndexError::Format(2))
        );
        assert_eq!(
            IndexError::Format(2).to_string(),
            "search index format 2 is not supported"
        );
        assert!(
            IndexError::Json("x".into())
                .to_string()
                .starts_with("search index is not valid JSON")
        );
    }

    #[test]
    fn empty_indexes_report_empty() {
        assert!(Index::empty().is_empty());
        let built = build(Vec::new()).map(|b| b.bytes);
        assert_eq!(built.as_deref(), Ok(&br#"{"v":1,"docs":[]}"#[..]));
        assert!(Index::from_slice(br#"{"v":1,"docs":[]}"#).is_ok_and(|i| i.is_empty()));
    }
}
