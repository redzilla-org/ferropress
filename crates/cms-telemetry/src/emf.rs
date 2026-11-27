//! `CloudWatch` Embedded Metric Format lines: metrics extracted from log output, no API call.

use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Number, Value, json};

use crate::Scope;
use crate::redact::scrub;

/// `CloudWatch` namespace of every Ferropress metric.
pub const NAMESPACE: &str = "Ferropress";

/// `CloudWatch` unit of a metric value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// An event count.
    Count,
    /// A duration in milliseconds.
    Milliseconds,
    /// A duration or age in seconds.
    Seconds,
    /// A unitless value.
    None,
}

impl Unit {
    /// The unit name `CloudWatch` expects.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Count => "Count",
            Self::Milliseconds => "Milliseconds",
            Self::Seconds => "Seconds",
            Self::None => "None",
        }
    }
}

/// Milliseconds since the Unix epoch, the EMF timestamp; 0 if the clock is before the epoch.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Metrics sharing one timestamp and the `[function, site]` dimension set, written as one line.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSet<'s> {
    scope: &'s Scope,
    timestamp_ms: u64,
    metrics: Vec<(&'static str, Unit, f64)>,
    properties: Map<String, Value>,
}

impl<'s> MetricSet<'s> {
    /// An empty set for `scope` stamped `timestamp_ms` (see [`now_ms`]).
    #[must_use]
    pub fn new(scope: &'s Scope, timestamp_ms: u64) -> Self {
        Self {
            scope,
            timestamp_ms,
            metrics: Vec::new(),
            properties: Map::new(),
        }
    }

    /// Adds a metric; a second value for the same name replaces the first.
    #[must_use]
    pub fn metric(mut self, name: &'static str, unit: Unit, value: f64) -> Self {
        match self
            .metrics
            .iter_mut()
            .find(|(existing, ..)| *existing == name)
        {
            Some(slot) => *slot = (name, unit, value),
            None => self.metrics.push((name, unit, value)),
        }
        self
    }

    /// Adds a searchable property that is not a dimension (for example `route_kind`).
    ///
    /// Properties are redacted like log fields. A property named like a dimension or a metric is
    /// overwritten by it.
    #[must_use]
    pub fn property(mut self, key: &str, value: impl Into<Value>) -> Self {
        let clean = scrub(key, value.into());
        self.properties.insert(key.to_owned(), clean);
        self
    }

    /// The EMF document.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidInput`] if a metric value is NaN or infinite, which `CloudWatch`
    /// would silently drop.
    pub fn to_value(&self) -> io::Result<Value> {
        let mut out = self.properties.clone();
        let definitions: Vec<Value> = self
            .metrics
            .iter()
            .map(|(name, unit, _)| json!({"Name": name, "Unit": unit.as_str()}))
            .collect();
        out.insert(
            "_aws".into(),
            json!({
                "Timestamp": self.timestamp_ms,
                "CloudWatchMetrics": [{
                    "Namespace": NAMESPACE,
                    "Dimensions": [["function", "site"]],
                    "Metrics": definitions,
                }],
            }),
        );
        out.insert("function".into(), self.scope.function().into());
        out.insert("site".into(), self.scope.site().into());
        for (name, _, value) in &self.metrics {
            let number = Number::from_f64(*value).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("metric {name} is not finite"),
                )
            })?;
            out.insert((*name).to_owned(), Value::Number(number));
        }
        Ok(Value::Object(out))
    }

    /// Writes the set as one JSON line followed by `\n`.
    ///
    /// # Errors
    ///
    /// A non-finite value (see [`MetricSet::to_value`]) or any error from `out`.
    pub fn write_line<W: Write>(&self, out: &mut W) -> io::Result<()> {
        let mut line = serde_json::to_vec(&self.to_value()?)?;
        line.push(b'\n');
        out.write_all(&line)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{MetricSet, NAMESPACE, Unit, now_ms};
    use crate::Scope;
    use crate::names;
    use crate::redact::REDACTED;

    #[test]
    fn units_use_cloudwatch_names() {
        assert_eq!(Unit::Count.as_str(), "Count");
        assert_eq!(Unit::Milliseconds.as_str(), "Milliseconds");
        assert_eq!(Unit::Seconds.as_str(), "Seconds");
        assert_eq!(Unit::None.as_str(), "None");
    }

    #[test]
    fn the_clock_is_after_2020() {
        assert!(now_ms() > 1_577_836_800_000);
    }

    #[test]
    fn a_set_matches_the_emf_shape() {
        let scope = Scope::new("read", "example");
        let value = MetricSet::new(&scope, 1_700_000_000_000)
            .metric(names::READ_HIT, Unit::Count, 1.0)
            .metric(names::READ_LATENCY, Unit::Milliseconds, 7.5)
            .property("route_kind", "post")
            .to_value()
            .unwrap_or(Value::Null);
        assert_eq!(
            value,
            json!({
                "_aws": {
                    "Timestamp": 1_700_000_000_000_u64,
                    "CloudWatchMetrics": [{
                        "Namespace": NAMESPACE,
                        "Dimensions": [["function", "site"]],
                        "Metrics": [
                            {"Name": "ReadHit", "Unit": "Count"},
                            {"Name": "ReadLatency", "Unit": "Milliseconds"}
                        ]
                    }]
                },
                "function": "read",
                "site": "example",
                "ReadHit": 1.0,
                "ReadLatency": 7.5,
                "route_kind": "post"
            })
        );
    }

    #[test]
    fn repeated_metrics_replace_and_dimensions_win_over_properties() {
        let scope = Scope::new("projector", "example");
        let value = MetricSet::new(&scope, 0)
            .metric(names::ROUTES_RENDERED, Unit::Count, 1.0)
            .metric(names::ROUTES_RENDERED, Unit::Count, 4.0)
            .property("site", "spoofed")
            .property("token", "abc")
            .to_value()
            .unwrap_or(Value::Null);
        assert_eq!(value["RoutesRendered"], 4.0);
        assert_eq!(
            value["_aws"]["CloudWatchMetrics"][0]["Metrics"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(value["site"], "example");
        assert_eq!(value["token"], REDACTED);
    }

    #[test]
    fn non_finite_values_are_rejected() {
        let scope = Scope::new("read", "example");
        let mut out = Vec::new();
        let error = MetricSet::new(&scope, 0)
            .metric(names::READ_LATENCY, Unit::Milliseconds, f64::NAN)
            .write_line(&mut out)
            .map_err(|error| error.kind());
        assert_eq!(error, Err(std::io::ErrorKind::InvalidInput));
        assert!(out.is_empty());
    }

    #[test]
    fn lines_end_with_a_newline() {
        let scope = Scope::new("read", "example");
        let mut out = Vec::new();
        let written = MetricSet::new(&scope, 1)
            .metric(names::READ_NOT_FOUND, Unit::Count, 1.0)
            .write_line(&mut out);
        assert!(written.is_ok());
        assert_eq!(out.last(), Some(&b'\n'));
    }
}
