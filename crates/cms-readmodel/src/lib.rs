//! The published read model served by the read path (`docs/aws/architecture.md` section 3).
//!
//! The projector job writes route items and pre-rendered fragments; the read path only resolves a
//! request path to one route item and assembles the response. Storage sits behind the
//! [`RouteTable`] and [`Blobs`] ports so the read Lambda can link a minimal adapter.
//!
//! # Examples
//!
//! ```
//! use cms_readmodel::{BasePath, Resolved};
//!
//! let base = BasePath::new("/blog/")?;
//! assert_eq!(base.resolve("/blog/hello/"), Resolved::Key("/blog/hello/".into()));
//! assert_eq!(base.resolve("/blog/hello"), Resolved::AddSlash("/blog/hello/".into()));
//! assert_eq!(base.resolve("/other/"), Resolved::Outside);
//! # Ok::<(), cms_readmodel::PathError>(())
//! ```

mod path;
mod route;
mod serve;

pub use path::{BasePath, PathError, Resolved, RouteKey};
pub use route::{BlobKey, Content, Route};
pub use serve::{BODY_MARK, Blobs, Method, PortError, Request, Response, RouteTable, serve};
