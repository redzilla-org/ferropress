//! Lambda Runtime API loop for the projector (blocking; reserved concurrency 1).

use std::io::Write;

use cms_projector::{Clock, ProjectError, Report, emf_line};
use serde_json::{Value, json};

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

/// An invocation payload that is neither the empty nudge nor a JSON object.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("projector payload is not a JSON object: {0}")]
pub struct TriggerError(pub Box<str>);

/// Accepts the nudge (empty body or `{}`) and the scheduled event (any JSON object) alike.
///
/// # Errors
///
/// [`TriggerError`] for anything else, so a misrouted event fails loudly.
pub fn trigger(payload: &[u8]) -> Result<(), TriggerError> {
    if payload.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    match serde_json::from_slice::<Value>(payload) {
        Ok(Value::Object(_)) => Ok(()),
        Ok(other) => Err(TriggerError(other.to_string().into())),
        Err(error) => Err(TriggerError(error.to_string().into())),
    }
}

/// The invocation response: what the run did, without paths (they are in the logs' EMF line).
#[must_use]
pub fn summary(report: &Report) -> Value {
    json!({
        "rows": report.rows,
        "ageSeconds": report.age_seconds,
        "blobs": report.blobs,
        "written": report.written,
        "stale": report.stale,
        "unresolved": report.unresolved,
        "invalidated": report.invalidated.len(),
        "nudged": report.nudged,
    })
}

/// Serves invocations from the Runtime API at `api` forever, running `job` once per invocation
/// and writing its EMF line to `metrics` (stdout in production).
///
/// A failed run is reported on the invocation's error endpoint; its rows stay in the outbox and
/// the next nudge or schedule retries them.
///
/// # Errors
///
/// [`RuntimeError`] only when the Runtime API fails.
pub fn run<J, W>(api: &str, clock: &dyn Clock, metrics: &mut W, mut job: J) -> Result<(), RuntimeError>
where
    J: FnMut() -> Result<Report, ProjectError>,
    W: Write,
{
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
        let outcome = trigger(&payload).map_err(|error| error.to_string()).and_then(|()| {
            // A full batch nudges; keep draining in this invocation, one EMF line per run.
            loop {
                let report = job().map_err(|error| error.to_string())?;
                let line = emf_line(clock.now_micros(), report.age_seconds, report.rows);
                writeln!(metrics, "{line}").map_err(|error| format!("metrics sink: {error}"))?;
                if !report.nudged {
                    return Ok(report);
                }
            }
        });
        let (path, body) = match outcome {
            Ok(report) => ("response", summary(&report)),
            Err(message) => (
                "error",
                json!({ "errorMessage": message, "errorType": "ProjectorError" }),
            ),
        };
        agent
            .post(format!("{root}/{id}/{path}"))
            .header("content-type", "application/json")
            .send(body.to_string().as_bytes())
            .map_err(|error| http(&error))?;
    }
}

fn http(error: &ureq::Error) -> RuntimeError {
    RuntimeError::Http(error.to_string().into())
}

#[cfg(test)]
mod tests {
    use cms_projector::Report;

    use super::{TriggerError, summary, trigger};

    #[test]
    fn the_nudge_and_the_schedule_are_the_same_trigger() {
        let scheduled = br#"{"version":"0","detail-type":"Scheduled Event","source":"aws.scheduler","detail":{}}"#;
        for payload in [&b""[..], b"  \n", b"{}", scheduled] {
            assert_eq!(trigger(payload), Ok(()));
        }
        assert_eq!(trigger(b"[1]"), Err(TriggerError("[1]".into())));
        assert!(trigger(b"{").is_err());
    }

    #[test]
    fn the_summary_counts_without_listing_paths() {
        let report = Report {
            rows: 2,
            invalidated: vec!["/blog/".into(), "/blog/feed/".into()],
            nudged: true,
            ..Report::default()
        };
        let value = summary(&report);
        assert_eq!(value["rows"], 2);
        assert_eq!(value["invalidated"], 2);
        assert_eq!(value["nudged"], true);
    }
}
