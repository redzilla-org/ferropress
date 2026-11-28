//! Environment configuration and the composition root: env -> adapters -> runtime loop.

use std::time::Duration;

use cms_readmodel::BasePath;
use infra_aws::{
    Addressing, AwsClient, ConfigError, DynamoDb, Endpoint, Environment, ReadTable, S3, required,
};

use crate::{RuntimeError, ShellCache, run};

// The function timeout is 3 s; one AWS call must leave room for the other two.
const AWS_TIMEOUT: Duration = Duration::from_secs(1);

/// Everything the read Lambda reads from its environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `AWS_LAMBDA_RUNTIME_API`: host and port of the Runtime API.
    pub runtime_api: String,
    /// `FP_BASE_PATH`: the mount path, such as `/blog/`.
    pub base: BasePath,
    /// `FP_TABLE`: the read-model table name.
    pub table: String,
    /// `FP_BUCKET`: the site bucket.
    pub bucket: String,
    /// `FP_DYNAMODB_ENDPOINT` when set (local emulators), else the regional endpoint.
    pub dynamodb: Endpoint,
    /// `FP_S3_ENDPOINT` when set (path-style, local emulators), else virtual-hosted AWS.
    pub s3: Option<Endpoint>,
    /// Credentials and region.
    pub environment: Environment,
}

/// The function could not start or the Runtime API went away.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartError {
    /// A variable is missing or invalid.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The Runtime API failed.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

impl Config {
    /// Reads the configuration through `lookup`, normally `|name| std::env::var(name).ok()`.
    ///
    /// # Errors
    ///
    /// [`StartError::Config`] naming the first missing or invalid variable.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(lookup: F) -> Result<Self, StartError> {
        let environment = Environment::from_lookup(&lookup)?;
        let base = BasePath::new(&required(&lookup, "FP_BASE_PATH")?)
            .map_err(|_slashes| ConfigError::Invalid("FP_BASE_PATH"))?;
        let endpoint = |name: &'static str| {
            lookup(name)
                .filter(|value| !value.is_empty())
                .map(|url| Endpoint::parse(&url).map_err(|_bad| ConfigError::Invalid(name)))
                .transpose()
        };
        let dynamodb = endpoint("FP_DYNAMODB_ENDPOINT")?
            .unwrap_or_else(|| DynamoDb::regional_endpoint(&environment.region));
        Ok(Self {
            runtime_api: required(&lookup, "AWS_LAMBDA_RUNTIME_API")?,
            base,
            table: required(&lookup, "FP_TABLE")?,
            bucket: required(&lookup, "FP_BUCKET")?,
            dynamodb,
            s3: endpoint("FP_S3_ENDPOINT")?,
            environment,
        })
    }
}

/// Builds the adapters from `config` and serves invocations until the Runtime API fails.
/// The shell cache lives for the whole execution environment, so warm invocations reuse it.
///
/// # Errors
///
/// [`StartError::Runtime`] when the Runtime API cannot be reached.
pub fn start(config: Config) -> Result<(), StartError> {
    let aws = AwsClient::new(config.environment, AWS_TIMEOUT);
    let mut routes = ReadTable::new(DynamoDb::new(aws.clone(), config.dynamodb), &config.table);
    let s3 = match config.s3 {
        Some(endpoint) => S3::new(aws, endpoint, &config.bucket, Addressing::Path),
        None => S3::regional(aws, &config.bucket),
    };
    let mut blobs = ShellCache::new(s3);
    run(&config.runtime_api, &config.base, &mut routes, &mut blobs)?;
    Ok(())
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
                ("FP_TABLE", "fp-read"),
                ("FP_BUCKET", "site-bucket"),
            ]
            .iter()
            .chain(extra)
            .rev()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn defaults_use_regional_aws_endpoints() {
        let config = Config::from_lookup(lookup(&[]));
        assert!(config.as_ref().is_ok_and(|c| c.s3.is_none()
            && c.dynamodb == Endpoint::https("dynamodb.us-east-1.amazonaws.com")
            && c.base.as_str() == "/blog/"
            && c.runtime_api == "127.0.0.1:9001"));
    }

    #[test]
    fn endpoint_overrides_point_at_local_emulators() {
        let config = Config::from_lookup(lookup(&[
            ("FP_DYNAMODB_ENDPOINT", "http://127.0.0.1:8000"),
            ("FP_S3_ENDPOINT", "http://127.0.0.1:9000"),
        ]));
        assert!(config.is_ok_and(|c| c.dynamodb.host() == "127.0.0.1:8000"
            && c.s3.is_some_and(|s3| s3.host() == "127.0.0.1:9000")));
    }

    #[test]
    fn bad_values_are_named() {
        assert_eq!(
            Config::from_lookup(lookup(&[("FP_BASE_PATH", "blog")])),
            Err(StartError::Config(ConfigError::Invalid("FP_BASE_PATH")))
        );
        assert_eq!(
            Config::from_lookup(lookup(&[("FP_S3_ENDPOINT", "ftp://x")])),
            Err(StartError::Config(ConfigError::Invalid("FP_S3_ENDPOINT")))
        );
        assert_eq!(
            Config::from_lookup(lookup(&[("FP_TABLE", "")])).map_err(|e| e.to_string()),
            Err("environment variable FP_TABLE is not set".to_owned())
        );
    }
}
