//! Signed blocking HTTP(S) shared by the service clients.

use std::time::Duration;

use crate::sigv4::{PathStyle, Scope, Signable, canonical_query, sign};
use crate::{AmzDate, Credentials, Environment, sha256_hex};

/// Where a service is reached: scheme plus authority, with no path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    base: Box<str>,
    host: Box<str>,
}

impl Endpoint {
    /// Parses `https://host[:port]` or `http://host[:port]` (plain HTTP only for local fakes and
    /// emulators).
    ///
    /// # Errors
    ///
    /// [`AwsError::Endpoint`] for another scheme, an empty host, or a path.
    pub fn parse(url: &str) -> Result<Self, AwsError> {
        let invalid = || AwsError::Endpoint(url.into());
        let host = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .ok_or_else(invalid)?
            .trim_end_matches('/');
        if host.is_empty() || host.contains(['/', '?', '#', ' ']) {
            return Err(invalid());
        }
        Ok(Self {
            base: url.trim_end_matches('/').into(),
            host: host.into(),
        })
    }

    /// `https://<host>`.
    #[must_use]
    pub fn https(host: &str) -> Self {
        Self {
            base: format!("https://{host}").into(),
            host: host.into(),
        }
    }

    /// The `Host` header value (authority, with the port when one was given).
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The scheme and authority.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }
}

/// An AWS call failed.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AwsError {
    /// The connection or HTTP exchange failed.
    #[error("{operation} transport failed: {message}")]
    Transport {
        /// Operation such as `DynamoDB.GetItem`.
        operation: &'static str,
        /// Transport error text (no request data).
        message: Box<str>,
    },
    /// The service answered with an error status.
    #[error("{operation} returned HTTP {status} {code}")]
    Service {
        /// Operation such as `S3.GetObject`.
        operation: &'static str,
        /// HTTP status.
        status: u16,
        /// AWS error code (`ConditionalCheckFailedException`, `NoSuchKey`), empty if none.
        code: Box<str>,
    },
    /// The service answered 2xx with a body this client cannot read.
    #[error("{operation} response malformed: {what}")]
    Malformed {
        /// Operation.
        operation: &'static str,
        /// What was wrong.
        what: &'static str,
    },
    /// A caller argument cannot be sent.
    #[error("{operation} rejected input: {what}")]
    Input {
        /// Operation.
        operation: &'static str,
        /// What was wrong.
        what: &'static str,
    },
    /// An endpoint URL is not `http(s)://authority`.
    #[error("invalid endpoint {0}")]
    Endpoint(Box<str>),
    /// The system clock reads before the Unix epoch, so nothing can be signed.
    #[error("system clock is before the Unix epoch")]
    Clock,
}

/// A raw service reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

/// One request to sign and send.
pub(crate) struct Call<'a> {
    pub(crate) operation: &'static str,
    pub(crate) service: &'static str,
    /// Overrides the client region (`CloudFront` always signs for `us-east-1`).
    pub(crate) region: Option<&'static str>,
    pub(crate) path_style: PathStyle,
    pub(crate) endpoint: &'a Endpoint,
    pub(crate) method: &'static str,
    pub(crate) path: &'a str,
    pub(crate) query: &'a [(&'a str, &'a str)],
    pub(crate) headers: &'a [(&'a str, &'a str)],
    pub(crate) body: &'a [u8],
}

/// Connection pool plus signing context shared by [`crate::DynamoDb`], [`crate::S3`] and
/// [`crate::CloudFront`]. Cloning shares the pool.
#[derive(Clone, Debug)]
pub struct AwsClient {
    agent: ureq::Agent,
    credentials: Credentials,
    region: Box<str>,
}

impl AwsClient {
    /// A client for `environment` whose every call (connect through body) is bounded by
    /// `timeout`.
    #[must_use]
    pub fn new(environment: Environment, timeout: Duration) -> Self {
        // Error statuses are replies to inspect (404 object, failed condition), not transport
        // errors.
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(timeout))
            .build()
            .new_agent();
        Self {
            agent,
            credentials: environment.credentials,
            region: environment.region,
        }
    }

    /// The signing region.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    pub(crate) fn send(&self, call: &Call<'_>) -> Result<Reply, AwsError> {
        let at = AmzDate::now().map_err(|_before_epoch| AwsError::Clock)?;
        let payload = sha256_hex(call.body);
        let mut headers: Vec<(&str, &str)> = Vec::with_capacity(call.headers.len() + 2);
        headers.push(("host", call.endpoint.host()));
        headers.push(("x-amz-content-sha256", &payload));
        headers.extend_from_slice(call.headers);
        let scope = Scope {
            credentials: &self.credentials,
            region: call.region.unwrap_or(&self.region),
            service: call.service,
        };
        let signable = Signable {
            method: call.method,
            path: call.path,
            query: call.query,
            headers: &headers,
            payload_sha256: &payload,
            path_style: call.path_style,
        };
        let added = sign(&scope, &signable, at);
        let mut url = format!("{}{}", call.endpoint.base(), call.path);
        if !call.query.is_empty() {
            url.push('?');
            url.push_str(&canonical_query(call.query));
        }
        let transport = |message: String| AwsError::Transport {
            operation: call.operation,
            message: message.into(),
        };
        let mut builder = ureq::http::Request::builder().method(call.method).uri(url);
        // ureq derives Host from the URL authority, which is what was signed.
        for (name, value) in headers.iter().skip(1) {
            builder = builder.header(*name, *value);
        }
        for (name, value) in &added {
            builder = builder.header(*name, value.as_str());
        }
        let request = builder
            .body(call.body)
            .map_err(|error| transport(error.to_string()))?;
        let response = self
            .agent
            .run(request)
            .map_err(|error| transport(error.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .read_to_vec()
            .map_err(|error| transport(error.to_string()))?;
        Ok(Reply { status, body })
    }
}

impl Reply {
    /// The reply as an error when its status is not 2xx.
    pub(crate) fn success(self, operation: &'static str) -> Result<Self, AwsError> {
        if (200..300).contains(&self.status) {
            Ok(self)
        } else {
            Err(self.error(operation))
        }
    }

    pub(crate) fn error(&self, operation: &'static str) -> AwsError {
        AwsError::Service {
            operation,
            status: self.status,
            code: error_code(&self.body).into(),
        }
    }
}

// JSON protocols put the code in `__type` (after `#`); XML protocols in `<Code>`.
pub(crate) fn error_code(body: &[u8]) -> String {
    let text = std::str::from_utf8(body).unwrap_or_default();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(text) {
        let kind = map
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        return kind.rsplit('#').next().unwrap_or_default().to_owned();
    }
    xml_text(text, "Code").unwrap_or_default().to_owned()
}

// The text of the first `<tag>…</tag>`; enough for the flat AWS XML replies read here.
pub(crate) fn xml_text<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(&text[start..end])
}

#[cfg(test)]
mod tests {
    use super::{AwsError, Endpoint, Reply, error_code, xml_text};

    #[test]
    fn endpoints_parse_scheme_and_authority() {
        let local = Endpoint::parse("http://127.0.0.1:8000/");
        assert_eq!(local.as_ref().map(Endpoint::host), Ok("127.0.0.1:8000"));
        assert_eq!(
            local.as_ref().map(Endpoint::base),
            Ok("http://127.0.0.1:8000")
        );
        assert_eq!(
            Endpoint::parse("https://dynamodb.us-east-1.amazonaws.com"),
            Ok(Endpoint::https("dynamodb.us-east-1.amazonaws.com"))
        );
        for bad in ["ftp://x", "https://", "https://x/path", "x"] {
            assert_eq!(Endpoint::parse(bad), Err(AwsError::Endpoint(bad.into())));
        }
        assert_eq!(
            AwsError::Endpoint("x".into()).to_string(),
            "invalid endpoint x"
        );
    }

    #[test]
    fn error_codes_come_from_json_or_xml() {
        assert_eq!(
            error_code(
                br#"{"__type":"com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException"}"#
            ),
            "ConditionalCheckFailedException"
        );
        assert_eq!(
            error_code(b"<Error><Code>NoSuchKey</Code></Error>"),
            "NoSuchKey"
        );
        assert_eq!(error_code(b"{}"), "");
        assert_eq!(error_code(b"\xff"), "");
        assert_eq!(xml_text("<a>1</a>", "b"), None);
        assert_eq!(xml_text("<a>1", "a"), None);
    }

    #[test]
    fn non_success_replies_become_service_errors() {
        let reply = Reply {
            status: 500,
            body: b"<Error><Code>InternalError</Code></Error>".to_vec(),
        };
        let error = reply.success("S3.GetObject");
        assert_eq!(
            error.map_err(|e| e.to_string()),
            Err("S3.GetObject returned HTTP 500 InternalError".to_owned())
        );
        let ok = Reply {
            status: 204,
            body: Vec::new(),
        };
        assert!(ok.success("S3.DeleteObject").is_ok());
        let messages = [
            AwsError::Transport {
                operation: "op",
                message: "refused".into(),
            }
            .to_string(),
            AwsError::Malformed {
                operation: "op",
                what: "json",
            }
            .to_string(),
            AwsError::Input {
                operation: "op",
                what: "empty",
            }
            .to_string(),
            AwsError::Clock.to_string(),
        ];
        assert_eq!(
            messages,
            [
                "op transport failed: refused",
                "op response malformed: json",
                "op rejected input: empty",
                "system clock is before the Unix epoch",
            ]
        );
    }
}
