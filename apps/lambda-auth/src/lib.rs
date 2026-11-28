//! Auth Lambda (`docs/aws/architecture.md` section 7, issue #8).
//!
//! [`Auth`] serves the login form, login and logout over [`cms_session::Login`]; [`run`] polls
//! the Lambda Runtime API and feeds it Function URL events. [`connect`] builds the sessions
//! and limits tables and reads the site secret once at init. The user store is a
//! [`cms_db::Store`] passed in: the binary entry point lands with the DSQL `Store` adapter
//! (issue #5).

mod config;
mod event;
mod form;
mod handler;
mod runtime;

pub use config::{Adapters, Config, StartError, connect};
pub use event::{AuthReply, AuthRequest, EventError, decode, encode};
pub use handler::{Auth, LOGOUT_ACTION};
pub use runtime::{RuntimeError, run};
