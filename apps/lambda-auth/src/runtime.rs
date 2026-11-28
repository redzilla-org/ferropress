//! Lambda Runtime API loop (blocking; one invocation at a time per execution environment).

use std::time::{SystemTime, UNIX_EPOCH};

use cms_db::Store;
use cms_session::{Entropy, RateLimiter, SessionStore};
use serde_json::json;

use crate::Auth;
use crate::event::{decode, encode};

/// The Runtime API itself failed; the process exits and Lambda restarts the environment.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeError {
    /// HTTP to the Runtime API failed.
    #[error("runtime API request failed: {0}")]
    Http(Box<str>),
    /// `next` returned no request id header.
    #[error("runtime API sent no request id")]
    NoRequestId,
    /// The clock reads before the Unix epoch, so sessions cannot be dated.
    #[error("system clock is before the Unix epoch")]
    Clock,
}

/// Serves invocations from the Runtime API at `api` (the `AWS_LAMBDA_RUNTIME_API` value)
/// forever. A failed request is reported on the invocation's error endpoint (the Function URL
/// answers 502) and the loop continues. Error messages carry no request data.
///
/// # Errors
///
/// [`RuntimeError`] only when the Runtime API or the clock fails.
pub fn run<S: Store, T: SessionStore, R: RateLimiter, E: Entropy>(
    api: &str,
    auth: &mut Auth<S, T, R, E>,
) -> Result<(), RuntimeError> {
    // One agent reuses the localhost connection; `next` long-polls, so no timeout is set.
    let agent = ureq::Agent::new_with_defaults();
    let root = format!("http://{api}/2018-06-01/runtime/invocation");
    loop {
        let mut next = agent
            .get(format!("{root}/next"))
            .call()
            .map_err(|error| http(&error))?;
        let id = next
            .headers()
            .get("lambda-runtime-aws-request-id")
            .and_then(|value| value.to_str().ok())
            .ok_or(RuntimeError::NoRequestId)?
            .to_owned();
        let payload = next
            .body_mut()
            .read_to_vec()
            .map_err(|error| http(&error))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_before_epoch| RuntimeError::Clock)?
            .as_secs();
        let outcome = decode(&payload)
            .map_err(|error| error.to_string())
            .and_then(|request| auth.handle(&request, now).map_err(|error| error.to_string()))
            .map(encode);
        let (path, body) = match outcome {
            Ok(body) => ("response", body),
            Err(message) => (
                "error",
                json!({ "errorMessage": message, "errorType": "AuthPathError" })
                    .to_string()
                    .into_bytes(),
            ),
        };
        agent
            .post(format!("{root}/{id}/{path}"))
            .header("content-type", "application/json")
            .send(&body[..])
            .map_err(|error| http(&error))?;
    }
}

fn http(error: &ureq::Error) -> RuntimeError {
    RuntimeError::Http(error.to_string().into())
}

#[cfg(test)]
mod tests {
    use super::RuntimeError;

    #[test]
    fn runtime_errors_describe_themselves() {
        assert_eq!(
            RuntimeError::NoRequestId.to_string(),
            "runtime API sent no request id"
        );
        assert_eq!(
            RuntimeError::Http("refused".into()).to_string(),
            "runtime API request failed: refused"
        );
        assert_eq!(
            RuntimeError::Clock.to_string(),
            "system clock is before the Unix epoch"
        );
    }
}
