//! Platform failures. A refused comment is a [`crate::Rejection`], not an error.

/// The submission could not be decided or stored because a dependency failed.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// `timezone_string` names a zone this profile cannot convert without a zone database.
    #[error("named time zone {0:?} is not supported; set gmt_offset instead")]
    TimeZone(Box<str>),
    /// The SQL store could not be reached.
    #[error("comment store unavailable")]
    Unavailable,
    /// The server refused a statement of the write transaction; it was rolled back.
    #[error("comment write failed: {0}")]
    Write(Box<str>),
    /// A value cannot be written as a literal in the store's dialect.
    #[error("comment field cannot be stored: {0}")]
    Literal(#[from] cms_db::LiteralError),
    /// A table name could not be formed.
    #[error("comment table name: {0}")]
    Table(#[from] cms_db::NameError),
    /// A port (rate limiter, session, permalink, nudge) failed.
    #[error("{port} failed: {message}")]
    Port {
        /// Which port.
        port: &'static str,
        /// What it reported.
        message: Box<str>,
    },
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(
            Error::TimeZone("Europe/Paris".into()).to_string(),
            "named time zone \"Europe/Paris\" is not supported; set gmt_offset instead"
        );
        assert_eq!(Error::Unavailable.to_string(), "comment store unavailable");
        assert_eq!(
            Error::Write("40001".into()).to_string(),
            "comment write failed: 40001"
        );
        assert_eq!(
            Error::from(cms_db::LiteralError::Nul).to_string(),
            "comment field cannot be stored: a PostgreSQL text literal must not contain a nul byte"
        );
        let port = Error::Port {
            port: "rate limiter",
            message: "throttled".into(),
        };
        assert_eq!(port.to_string(), "rate limiter failed: throttled");
    }
}
