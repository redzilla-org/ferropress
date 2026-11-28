//! Admin and editor Lambda (`docs/aws/architecture.md` section 3, issue #9).
//!
//! [`run`] polls the Lambda Runtime API; [`serve`] decodes each Function URL event, answers it
//! through [`cms_authoring::handle`] over the request's ports and encodes the Function URL
//! response. The `fp_session` cookie is resolved by the [`cms_authoring::Sessions`] port.

mod event;
mod runtime;

pub use event::{EventError, decode, encode};
pub use runtime::{RuntimeError, run};

use cms_authoring::{Ports, Site, handle};

/// Answers one Function URL event payload.
///
/// # Errors
///
/// The message of an [`EventError`] when the payload is not a Function URL event or the answer
/// is not text; the runtime reports it on the invocation's error endpoint.
pub fn serve(site: &Site, ports: Ports<'_>, payload: &[u8]) -> Result<Vec<u8>, String> {
    let request = decode(payload).map_err(|error| error.to_string())?;
    encode(handle(site, ports, &request)).map_err(|error| error.to_string())
}
