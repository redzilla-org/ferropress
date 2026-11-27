//! One-line structured JSON log records.

use std::io::{self, Write};

use serde_json::{Map, Value};

use crate::Scope;
use crate::redact::scrub;

/// Severity of a log record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Detail for diagnosing one request.
    Debug,
    /// Normal operation worth keeping.
    Info,
    /// Degraded but handled.
    Warn,
    /// The request or job failed.
    Error,
}

impl Level {
    /// The level as written in the `level` field.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// One log record: a fixed message plus redacted fields, written as a single JSON line.
///
/// The message is `&'static str` so values are always fields (searchable, redactable) and never
/// interpolated into the text. Fields live under `fields` so they cannot shadow `level`, `msg`,
/// `function`, `site` or `request_id`.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    level: Level,
    msg: &'static str,
    request_id: Option<Box<str>>,
    fields: Map<String, Value>,
}

impl Record {
    /// A record at `level` with the message template `msg`.
    #[must_use]
    pub fn new(level: Level, msg: &'static str) -> Self {
        Self {
            level,
            msg,
            request_id: None,
            fields: Map::new(),
        }
    }

    /// Attaches the Lambda request id.
    #[must_use]
    pub fn request_id(mut self, id: &str) -> Self {
        self.request_id = Some(id.into());
        self
    }

    /// Adds a field; secrets are redacted here, so the record never holds them.
    #[must_use]
    pub fn field(mut self, key: &str, value: impl Into<Value>) -> Self {
        let clean = scrub(key, value.into());
        self.fields.insert(key.to_owned(), clean);
        self
    }

    /// Adds a header map as one object field, redacting secret headers by name.
    #[must_use]
    pub fn headers<'h>(
        self,
        key: &str,
        headers: impl IntoIterator<Item = (&'h str, &'h str)>,
    ) -> Self {
        let map: Map<String, Value> = headers
            .into_iter()
            .map(|(name, value)| (name.to_owned(), Value::String(value.to_owned())))
            .collect();
        self.field(key, Value::Object(map))
    }

    /// The record as a JSON object scoped to `scope`.
    #[must_use]
    pub fn to_value(&self, scope: &Scope) -> Value {
        let mut out = Map::new();
        out.insert("level".into(), self.level.as_str().into());
        out.insert("msg".into(), self.msg.into());
        out.insert("function".into(), scope.function().into());
        out.insert("site".into(), scope.site().into());
        if let Some(id) = &self.request_id {
            out.insert("request_id".into(), id.as_ref().into());
        }
        if !self.fields.is_empty() {
            out.insert("fields".into(), Value::Object(self.fields.clone()));
        }
        Value::Object(out)
    }

    /// Writes the record as one JSON line followed by `\n`.
    ///
    /// # Errors
    ///
    /// Any error from `out`.
    pub fn write_line<W: Write>(&self, scope: &Scope, out: &mut W) -> io::Result<()> {
        let mut line = serde_json::to_vec(&self.to_value(scope))?;
        line.push(b'\n');
        out.write_all(&line)
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use serde_json::{Value, json};

    use super::{Level, Record};
    use crate::Scope;
    use crate::redact::REDACTED;

    fn scope() -> Scope {
        Scope::new("read", "example")
    }

    #[test]
    fn levels_have_lowercase_names_and_order() {
        assert_eq!(Level::Debug.as_str(), "debug");
        assert_eq!(Level::Info.as_str(), "info");
        assert_eq!(Level::Warn.as_str(), "warn");
        assert_eq!(Level::Error.as_str(), "error");
        assert!(Level::Debug < Level::Error);
    }

    #[test]
    fn a_bare_record_has_only_the_fixed_keys() {
        let value = Record::new(Level::Info, "cold start").to_value(&scope());
        assert_eq!(
            value,
            json!({"level": "info", "msg": "cold start", "function": "read", "site": "example"})
        );
    }

    #[test]
    fn fields_are_nested_and_cannot_shadow_fixed_keys() {
        let value = Record::new(Level::Warn, "slow store")
            .request_id("req-1")
            .field("level", "fake")
            .field("elapsed_ms", 812)
            .to_value(&scope());
        assert_eq!(value["level"], "warn");
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["fields"], json!({"level": "fake", "elapsed_ms": 812}));
    }

    #[test]
    fn secrets_never_reach_the_written_line() {
        let mut out = Vec::new();
        let written = Record::new(Level::Error, "login failed")
            .field("user_pass", "hunter2")
            .field("post_password", "letmein")
            .field("query", "p=3&_nonce=ff00")
            .headers(
                "headers",
                [
                    ("Cookie", "fp_session=s3cr3t"),
                    ("authorization", "AWS4-HMAC-SHA256 Signature=deadbeef"),
                    ("x-amz-security-token", "IQoJb3JpZ2lu"),
                    ("accept", "text/html"),
                ],
            )
            .write_line(&scope(), &mut out);
        assert!(written.is_ok());
        let text = String::from_utf8_lossy(&out);
        for secret in [
            "hunter2",
            "letmein",
            "ff00",
            "s3cr3t",
            "deadbeef",
            "IQoJb3JpZ2lu",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert!(text.ends_with('\n'));
        assert_eq!(text.lines().count(), 1);
        let parsed: Value = serde_json::from_str(text.trim_end()).unwrap_or(Value::Null);
        assert_eq!(parsed["fields"]["headers"]["accept"], "text/html");
        assert_eq!(parsed["fields"]["headers"]["Cookie"], REDACTED);
    }

    #[test]
    fn writer_errors_are_returned() {
        struct Broken;
        impl io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let result = Record::new(Level::Info, "x").write_line(&scope(), &mut Broken);
        assert_eq!(
            result.map_err(|error| error.to_string()),
            Err("closed".to_owned())
        );
    }
}
