//! Lambda Runtime API loop (blocking; one invocation at a time per execution environment).

use cms_readmodel::{BasePath, Blobs, RouteTable};
use serde_json::json;

use crate::{Cache, invoke};

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
}

/// Serves invocations from the Runtime API at `api` (the `AWS_LAMBDA_RUNTIME_API` value) forever,
/// keeping one [`Cache`] for the life of the container.
///
/// A failed request is reported on the invocation's error endpoint so the platform logs it and the
/// Function URL answers 502; the loop then continues with the next invocation.
///
/// # Errors
///
/// [`RuntimeError`] only when the Runtime API itself cannot be reached.
pub fn run<R: RouteTable, B: Blobs>(
    api: &str,
    base: &BasePath,
    routes: &mut R,
    blobs: &mut B,
) -> Result<(), RuntimeError> {
    // One agent reuses the localhost connection; `next` long-polls, so no timeout is set.
    let agent = ureq::Agent::new_with_defaults();
    let root = format!("http://{api}/2018-06-01/runtime/invocation");
    let mut cache = Cache::new();
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
        let (endpoint, body) = match invoke(base, &payload, &mut cache, routes, blobs) {
            Ok(body) => ("response", body),
            Err(message) => (
                "error",
                json!({ "errorMessage": message, "errorType": "SearchError" })
                    .to_string()
                    .into_bytes(),
            ),
        };
        agent
            .post(format!("{root}/{id}/{endpoint}"))
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
    }
}
