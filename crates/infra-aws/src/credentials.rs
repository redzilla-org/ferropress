//! Credentials and region from the Lambda environment, passed in explicitly.

use std::fmt;

/// An access key pair plus the session token Lambda execution roles always carry.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    access_key_id: Box<str>,
    secret_access_key: Box<str>,
    session_token: Option<Box<str>>,
}

impl Credentials {
    /// Credentials from their parts.
    #[must_use]
    pub fn new(access_key_id: &str, secret_access_key: &str, session_token: Option<&str>) -> Self {
        Self {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token: session_token.map(Into::into),
        }
    }

    /// The access key id (not secret; it appears in every signed request).
    #[must_use]
    pub fn access_key_id(&self) -> &str {
        &self.access_key_id
    }

    /// The session token of temporary credentials.
    #[must_use]
    pub fn session_token(&self) -> Option<&str> {
        self.session_token.as_deref()
    }

    pub(crate) fn secret_access_key(&self) -> &str {
        &self.secret_access_key
    }
}

// The secret and token never reach logs (rust-docs-observability.mdc redaction rule).
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// A required configuration variable is missing or empty.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    /// The named variable is unset or empty.
    #[error("environment variable {0} is not set")]
    Missing(&'static str),
    /// The named variable has a value this program cannot use.
    #[error("environment variable {0} is invalid")]
    Invalid(&'static str),
}

/// What every AWS client needs from the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    /// Signing credentials.
    pub credentials: Credentials,
    /// The function's region, such as `us-east-1`.
    pub region: Box<str>,
}

impl Environment {
    /// Reads `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` (optional) and
    /// `AWS_REGION` through `lookup`, normally `|name| std::env::var(name).ok()`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Missing`] naming the first required variable that is unset or empty.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(lookup: F) -> Result<Self, ConfigError> {
        let access = required(&lookup, "AWS_ACCESS_KEY_ID")?;
        let secret = required(&lookup, "AWS_SECRET_ACCESS_KEY")?;
        let region = required(&lookup, "AWS_REGION")?;
        let token = lookup("AWS_SESSION_TOKEN").filter(|value| !value.is_empty());
        Ok(Self {
            credentials: Credentials::new(&access, &secret, token.as_deref()),
            region: region.into(),
        })
    }
}

/// Reads one required, non-empty variable through `lookup`.
///
/// # Errors
///
/// [`ConfigError::Missing`] when it is unset or empty.
pub fn required<F: Fn(&str) -> Option<String>>(
    lookup: &F,
    name: &'static str,
) -> Result<String, ConfigError> {
    lookup(name)
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Missing(name))
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, Credentials, Environment};

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn lambda_variables_become_an_environment() {
        let env = Environment::from_lookup(lookup(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_SESSION_TOKEN", "token"),
            ("AWS_REGION", "us-west-2"),
        ]));
        assert_eq!(
            env,
            Ok(Environment {
                credentials: Credentials::new("AKID", "secret", Some("token")),
                region: "us-west-2".into(),
            })
        );
        let no_token = Environment::from_lookup(lookup(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_SESSION_TOKEN", ""),
            ("AWS_REGION", "us-west-2"),
        ]));
        assert!(no_token.is_ok_and(|env| env.credentials.session_token().is_none()));
    }

    #[test]
    fn missing_variables_are_named() {
        assert_eq!(
            Environment::from_lookup(lookup(&[])),
            Err(ConfigError::Missing("AWS_ACCESS_KEY_ID"))
        );
        let err = Environment::from_lookup(lookup(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_REGION", ""),
        ]));
        assert_eq!(err, Err(ConfigError::Missing("AWS_REGION")));
        assert_eq!(
            ConfigError::Missing("X").to_string(),
            "environment variable X is not set"
        );
        assert_eq!(
            ConfigError::Invalid("X").to_string(),
            "environment variable X is invalid"
        );
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let text = format!("{:?}", Credentials::new("AKID", "s3cr3t", Some("t0k3n")));
        assert!(text.contains("AKID"));
        assert!(!text.contains("s3cr3t"));
        assert!(!text.contains("t0k3n"));
        let bare = format!("{:?}", Credentials::new("AKID", "s3cr3t", None));
        assert!(bare.contains("session_token: None"));
    }
}
