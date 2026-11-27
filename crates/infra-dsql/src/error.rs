//! Platform failures from the DSQL adapter.

use cms_db::StoreError;

use crate::SqlState;

/// The adapter could not reach a server, authenticate, or understand its replies.
///
/// Server-reported SQL failures after startup are [`cms_db::Reply::Failed`], not this type.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The socket failed or closed.
    #[error("DSQL connection failed: {0}")]
    Io(#[from] std::io::Error),
    /// The server sent something this client does not accept.
    #[error("DSQL protocol violation: {0}")]
    Protocol(&'static str),
    /// The server asked for an authentication method other than a cleartext password.
    #[error("DSQL asked for unsupported authentication method {0}")]
    UnsupportedAuth(i32),
    /// The server refused the connection during startup.
    #[error("DSQL refused the connection: {message} (SQLSTATE {state})")]
    Refused {
        /// The SQLSTATE the server sent.
        state: SqlState,
        /// The server's message.
        message: Box<str>,
    },
    /// The server answered the TLS request with `N`; sslmode=require forbids plaintext.
    #[error("DSQL server refused TLS")]
    TlsRefused,
    /// The TLS layer failed.
    #[error("DSQL TLS failed: {0}")]
    Tls(Box<str>),
    /// The token source could not produce an auth token.
    #[error("DSQL auth token unavailable: {0}")]
    Token(Box<str>),
}

impl From<Error> for StoreError {
    fn from(_error: Error) -> Self {
        // The port has one platform failure; the detail stays with the adapter's own callers.
        Self::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use crate::SqlState;
    use cms_db::StoreError;

    #[test]
    fn errors_render_and_map_to_the_store_port() {
        let refused = Error::Refused {
            state: SqlState::new(*b"28P01"),
            message: "password authentication failed".into(),
        };
        assert_eq!(
            refused.to_string(),
            "DSQL refused the connection: password authentication failed (SQLSTATE 28P01)"
        );
        assert_eq!(
            Error::UnsupportedAuth(10).to_string(),
            "DSQL asked for unsupported authentication method 10"
        );
        assert_eq!(Error::TlsRefused.to_string(), "DSQL server refused TLS");
        assert_eq!(
            Error::Tls("bad certificate".into()).to_string(),
            "DSQL TLS failed: bad certificate"
        );
        assert_eq!(
            Error::Token("expired credentials".into()).to_string(),
            "DSQL auth token unavailable: expired credentials"
        );
        assert_eq!(
            Error::Protocol("truncated").to_string(),
            "DSQL protocol violation: truncated"
        );
        let io = Error::from(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        assert!(io.to_string().starts_with("DSQL connection failed"));
        assert!(StoreError::from(io).is_unavailable());
    }
}
