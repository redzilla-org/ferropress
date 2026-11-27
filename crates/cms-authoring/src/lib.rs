//! Editor workflows for the serverless admin (`docs/aws/architecture.md` sections 3 and 5,
//! issue #9).
//!
//! Every write runs in one store transaction that also commits an `fp_outbox` row, then nudges
//! the projector. Storage goes through [`Repo`]; sessions, capabilities, the projector and the
//! clock are narrow ports the composition root adapts.

mod error;
mod model;
mod ports;
mod repo;

pub use error::Error;
pub use model::{
    Change, OutboxEvent, PostRow, PostWrite, Status, StatusFilter, TermRow, ZERO_DATE,
};
pub use ports::{CapsPermissions, Clock, Permissions, Projector, Sessions};
pub use repo::{Repo, SqlRepo};
