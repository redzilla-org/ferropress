//! Prebuilt search index for the serverless profile (`docs/aws/architecture.md`, issue #12).
//!
//! The projector turns published posts into [`Document`]s and calls [`build`] on every publish;
//! the search Lambda decodes the bytes once per warm container with [`Index::from_slice`] and
//! answers each query with [`Index::search`]. Matching and ordering follow the reference's search
//! query: [`Terms::parse`] splits the search string, every included term must appear in the title,
//! excerpt or content, every `-` term must not, and hits are ranked by where the sentence and the
//! terms occur, then newest first.
//!
//! # Examples
//!
//! ```
//! use cms_search::{Document, Index, PER_PAGE, Terms, build};
//!
//! let built = build(vec![
//!     Document::new(1, "post", "2026-11-01 10:00:00", "/blog/a/", "Red boat", "", "<p>sails</p>"),
//!     Document::new(2, "post", "2026-11-02 10:00:00", "/blog/b/", "Blue car", "", "a red door"),
//! ])?;
//! assert!(built.key.starts_with("search/") && built.key.ends_with(".json"));
//! let index = Index::from_slice(&built.bytes)?;
//! let page = index.search(&Terms::parse("red"), 1, PER_PAGE);
//! // Both match; the title match ranks first although it is older.
//! assert_eq!(page.hits.iter().map(|d| d.id).collect::<Vec<_>>(), [1, 2]);
//! # Ok::<(), cms_search::IndexError>(())
//! ```

mod doc;
mod index;
mod terms;

pub use doc::{Document, text_of};
pub use index::{Built, INDEX_ROUTE, Index, IndexError, PER_PAGE, Page, build};
pub use terms::{STOPWORDS, Terms};
