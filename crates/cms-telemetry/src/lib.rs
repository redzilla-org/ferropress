//! Telemetry for the serverless profile: one-line JSON logs, EMF metrics and route classes.
//!
//! Every Lambda writes its log records and metric lines to standard output, where the Lambda
//! platform forwards them to `CloudWatch` Logs; EMF lines become `CloudWatch` metrics with no API
//! call. Secrets are redacted when a field is added, so a record can never hold them.
//!
//! # Examples
//!
//! ```
//! use cms_telemetry::{Level, MetricSet, Record, Scope, Unit, names};
//!
//! let scope = Scope::new("read", "example");
//! let mut out = Vec::new();
//! Record::new(Level::Info, "route served")
//!     .field("status", 200)
//!     .field("cookie", "session=abc")
//!     .write_line(&scope, &mut out)?;
//! MetricSet::new(&scope, 1_700_000_000_000)
//!     .metric(names::READ_HIT, Unit::Count, 1.0)
//!     .write_line(&mut out)?;
//! let text = String::from_utf8_lossy(&out);
//! assert!(!text.contains("abc"));
//! # Ok::<(), std::io::Error>(())
//! ```

mod emf;
mod log;
pub mod names;
pub mod redact;
mod route_class;

pub use emf::{MetricSet, NAMESPACE, Unit, now_ms};
pub use log::{Level, Record};
pub use route_class::RouteClass;

/// Which function and site a log record or metric belongs to; the EMF dimension set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    function: Box<str>,
    site: Box<str>,
}

impl Scope {
    /// A scope for `function` (for example `read`) serving `site` (the site environment name).
    #[must_use]
    pub fn new(function: &str, site: &str) -> Self {
        Self {
            function: function.into(),
            site: site.into(),
        }
    }

    /// The function name.
    #[must_use]
    pub fn function(&self) -> &str {
        &self.function
    }

    /// The site environment name.
    #[must_use]
    pub fn site(&self) -> &str {
        &self.site
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;

    #[test]
    fn scope_keeps_function_and_site() {
        let scope = Scope::new("projector", "example-prod");
        assert_eq!(scope.function(), "projector");
        assert_eq!(scope.site(), "example-prod");
    }
}
