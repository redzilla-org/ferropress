//! Media uploads for the serverless profile (`docs/aws/architecture.md` sections 3-4, issue #10).
//!
//! Two halves, each behind narrow ports so the Lambda binaries only wire adapters:
//!
//! - [`sign`]: checks an upload request and returns a presigned S3 PUT for
//!   `site/incoming/<uuid>/<name>` ([`Presigner`], [`Uploaders`]).
//! - [`process`]: turns one incoming object into the reference's upload files under
//!   `site/uploads/YYYY/MM/` ([`Objects`]) and commits the attachment post row, its two meta rows
//!   and an outbox row in one transaction on a [`cms_db::Store`].
//!
//! # Examples
//!
//! ```
//! use cms_media::{sanitize_file_name, sized_name};
//!
//! assert_eq!(sanitize_file_name("My Photo (1).JPG")?, "My-Photo-1.JPG");
//! assert_eq!(sized_name("photo", "jpg", 300, 200), "photo-300x200.jpg");
//! # Ok::<(), cms_media::Error>(())
//! ```

mod error;
mod kind;
mod meta;
mod name;
mod ports;
mod process;
mod sign;
mod sizes;
mod sql;

pub use error::{Error, PortError};
pub use kind::ImageKind;
pub use meta::{Metadata, SizeEntry};
pub use name::{sanitize_file_name, sanitize_slug, scaled_name, sized_name, unique_stem};
pub use ports::{Object, Objects, Presigner, Uploaders};
pub use process::{Clock, Processed, Site, process};
pub use sign::{AUTHOR_HEADER, SignPolicy, SignRequest, Signed, incoming_key, sign};
pub use sizes::{BIG_IMAGE_THRESHOLD, DEFAULT_SIZES, Plan, SubSize, constrain, resize_plan};
