//! Editor workflows for the serverless admin (`docs/aws/architecture.md` sections 3 and 5,
//! issue #9).
//!
//! Every write runs in one store transaction that also commits an `fp_outbox` row, then nudges
//! the projector. Storage goes through [`Repo`]; sessions, capabilities, the projector and the
//! clock are narrow ports the composition root adapts. Previews render the stored row with the
//! built-in theme (`cms-render`) and are never cached.

mod date;
mod editor;
mod error;
mod http;
mod model;
mod pages;
mod ports;
mod preview;
mod repo;
mod router;
mod slug;

pub use date::format as format_date;
pub use editor::{Editor, PER_PAGE, Page, PostInput, Saved, Site, Tags, delete_caps, edit_caps};
pub use error::Error;
pub use http::{Method, Request, Response};
pub use model::{
    Change, OutboxEvent, PostRow, PostWrite, Status, StatusFilter, TermRow, ZERO_DATE,
};
pub use ports::{CapsPermissions, Clock, Permissions, Projector, Sessions};
pub use repo::{Repo, SqlRepo};
pub use router::{ADMIN_ACTION, Ports, REST_ACTION, SESSION_COOKIE, handle};
pub use slug::{sanitize as sanitize_slug, unique as unique_slug};
