//! Environment configuration and the init-time adapters (sessions, limits, site secret).

use std::time::Duration;

use cms_session::{CookiePath, NonceKey};
use infra_aws::{
    AwsClient, AwsError, ConfigError, DynamoDb, Endpoint, Environment, LimitsTable,
    SecretsManager, SessionsTable, required,
};

// The function timeout is 10 s; a login makes at most about six AWS calls.
const AWS_TIMEOUT: Duration = Duration::from_secs(1);

/// The nonce salt joined to the site secret, separating nonce MACs from other uses of it.
const NONCE_SALT: &[u8] = b"nonce";

/// Everything the auth Lambda reads from its environment. Holds no secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `AWS_LAMBDA_RUNTIME_API`: host and port of the Runtime API.
    pub runtime_api: String,
    /// `FP_BASE_PATH`: the mount path, such as `/blog/`; also the cookie path.
    pub base: CookiePath,
    /// `FP_SESSIONS_TABLE`.
    pub sessions_table: String,
    /// `FP_LIMITS_TABLE`.
    pub limits_table: String,
    /// `FP_SECRET_ARN`: the site secret, read once at init.
    pub secret_arn: String,
    /// `FP_DYNAMODB_ENDPOINT` when set (local emulators), else the regional endpoint.
    pub dynamodb: Endpoint,
    /// `FP_SECRETS_ENDPOINT` when set (local emulators), else the regional endpoint.
    pub secrets: Endpoint,
    /// Credentials and region.
    pub environment: Environment,
}

/// The function could not start.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartError {
    /// A variable is missing or invalid.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The site secret could not be read.
    #[error(transparent)]
    Secret(#[from] AwsError),
}

impl Config {
    /// Reads the configuration through `lookup`, normally `|name| std::env::var(name).ok()`.
    ///
    /// # Errors
    ///
    /// [`StartError::Config`] naming the first missing or invalid variable.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(lookup: F) -> Result<Self, StartError> {
        let environment = Environment::from_lookup(&lookup)?;
        let base = required(&lookup, "FP_BASE_PATH")?;
        let base = CookiePath::new(&base)
            .filter(|_| base.ends_with('/'))
            .ok_or(ConfigError::Invalid("FP_BASE_PATH"))?;
        let endpoint = |name: &'static str, regional: Endpoint| {
            match lookup(name).filter(|value| !value.is_empty()) {
                Some(url) => Endpoint::parse(&url).map_err(|_bad| ConfigError::Invalid(name)),
                None => Ok(regional),
            }
        };
        Ok(Self {
            runtime_api: required(&lookup, "AWS_LAMBDA_RUNTIME_API")?,
            base,
            sessions_table: required(&lookup, "FP_SESSIONS_TABLE")?,
            limits_table: required(&lookup, "FP_LIMITS_TABLE")?,
            secret_arn: required(&lookup, "FP_SECRET_ARN")?,
            dynamodb: endpoint(
                "FP_DYNAMODB_ENDPOINT",
                DynamoDb::regional_endpoint(&environment.region),
            )?,
            secrets: endpoint(
                "FP_SECRETS_ENDPOINT",
                SecretsManager::regional_endpoint(&environment.region),
            )?,
            environment,
        })
    }
}

/// The init-time adapters. [`Debug`] omits the nonce key.
#[derive(Debug)]
pub struct Adapters {
    /// The sessions table.
    pub sessions: SessionsTable,
    /// The rate-limits table.
    pub limits: LimitsTable,
    /// The nonce key derived from the site secret.
    pub nonce: NonceKey,
}

/// Builds the table adapters and reads the site secret once (architecture section 7).
///
/// # Errors
///
/// [`StartError::Secret`] when the secret cannot be read.
pub fn connect(config: &Config) -> Result<Adapters, StartError> {
    let aws = AwsClient::new(config.environment.clone(), AWS_TIMEOUT);
    let secret = SecretsManager::new(aws.clone(), config.secrets.clone())
        .get_secret_string(&config.secret_arn)?;
    let db = DynamoDb::new(aws, config.dynamodb.clone());
    Ok(Adapters {
        sessions: SessionsTable::new(db.clone(), &config.sessions_table),
        limits: LimitsTable::new(db, &config.limits_table),
        nonce: NonceKey::new(secret.as_bytes(), NONCE_SALT),
    })
}

#[cfg(test)]
mod tests {
    use super::{Config, StartError};
    use infra_aws::{ConfigError, Endpoint};

    fn lookup(extra: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            [
                ("AWS_ACCESS_KEY_ID", "AKID"),
                ("AWS_SECRET_ACCESS_KEY", "secret"),
                ("AWS_REGION", "us-east-1"),
                ("AWS_LAMBDA_RUNTIME_API", "127.0.0.1:9001"),
                ("FP_BASE_PATH", "/blog/"),
                ("FP_SESSIONS_TABLE", "fp-sessions"),
                ("FP_LIMITS_TABLE", "fp-limits"),
                ("FP_SECRET_ARN", "arn:aws:secretsmanager:us-east-1:1:secret:s"),
            ]
            .iter()
            .chain(extra)
            .rev()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn defaults_use_regional_endpoints_and_overrides_point_at_emulators() {
        let config = Config::from_lookup(lookup(&[])).unwrap();
        assert_eq!(config.dynamodb, Endpoint::https("dynamodb.us-east-1.amazonaws.com"));
        assert_eq!(config.secrets, Endpoint::https("secretsmanager.us-east-1.amazonaws.com"));
        assert_eq!(config.base.as_str(), "/blog/");
        let local = Config::from_lookup(lookup(&[
            ("FP_DYNAMODB_ENDPOINT", "http://127.0.0.1:8000"),
            ("FP_SECRETS_ENDPOINT", "http://127.0.0.1:4566"),
        ]))
        .unwrap();
        assert_eq!(local.dynamodb.host(), "127.0.0.1:8000");
        assert_eq!(local.secrets.host(), "127.0.0.1:4566");
    }

    #[test]
    fn bad_values_are_named() {
        const BAD: [&[(&str, &str)]; 3] = [
            &[("FP_BASE_PATH", "blog/")],
            &[("FP_BASE_PATH", "/blog")],
            &[("FP_BASE_PATH", "/a b/")],
        ];
        for extra in BAD {
            assert_eq!(
                Config::from_lookup(lookup(extra)),
                Err(StartError::Config(ConfigError::Invalid("FP_BASE_PATH"))),
                "{extra:?}"
            );
        }
        assert_eq!(
            Config::from_lookup(lookup(&[("FP_SECRETS_ENDPOINT", "ftp://x")])),
            Err(StartError::Config(ConfigError::Invalid("FP_SECRETS_ENDPOINT")))
        );
        assert_eq!(
            Config::from_lookup(lookup(&[("FP_SECRET_ARN", "")])).map_err(|e| e.to_string()),
            Err("environment variable FP_SECRET_ARN is not set".to_owned())
        );
    }
}
