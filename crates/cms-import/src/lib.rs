//! Streaming WXR import into a site's tables over a [`cms_db::Store`].
//!
//! [`Wxr`] reads an export file one record at a time (bounded memory). [`import`] writes the
//! records in transactions of at most [`MAX_BATCH_ROWS`] rows or [`MAX_BATCH_BYTES`] bytes,
//! saving a [`Checkpoint`] after each commit so a timed-out run resumes. Source ids are kept
//! where free, re-runs match existing rows by guid plus post type (terms by slug plus taxonomy,
//! users by login), and attachments go from a [`Fetcher`] to an [`UploadSink`]. A finished run
//! returns one [`OutboxEvent`] asking the projector to rebuild every route. Design:
//! `docs/aws/architecture.md` section 9.
//!
//! # Examples
//!
//! ```
//! use cms_import::{Record, Wxr};
//!
//! let xml = "<rss><channel><wp:base_site_url>https://example.com</wp:base_site_url>\
//!            </channel></rss>";
//! let mut wxr = Wxr::new(xml.as_bytes());
//! assert_eq!(wxr.next_record()?, Some(Record::Site("https://example.com".into())));
//! assert_eq!(wxr.next_record()?, None);
//! # Ok::<(), cms_import::ParseError>(())
//! ```

mod error;
mod import;
mod ports;
mod record;
mod sql;
mod text;
mod tree;
mod wxr;

pub use error::{ImportError, ParseError, PortError};
pub use import::{MAX_BATCH_BYTES, MAX_BATCH_ROWS, Options, OutboxEvent, Ports, Report, import};
pub use ports::{Checkpoint, Fetcher, UploadSink};
pub use record::{Author, Comment, Item, Meta, Record, Term, TermRef};
pub use wxr::Wxr;
