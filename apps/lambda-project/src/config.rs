//! Environment configuration and the composition root: env -> adapters -> runtime loop.

use std::time::Duration;

use cms_db::{Store, TablePrefix};
use cms_projector::{DbSource, Native, Paths, Projector};
use cms_readmodel::BasePath;
use cms_render::NoCards;
use infra_aws::{
    Addressing, AwsClient, CloudFront, ConfigError, DynamoDb, Endpoint, Environment, S3, required,
};

use crate::aws::{Edge, InProcess, Objects, SystemClock, Table};
use crate::{RuntimeError, run};

// Writes are small; a slow call fails the run and the rows are retried by the next one.
const AWS_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything the projector Lambda reads from its environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `AWS_LAMBDA_RUNTIME_API`: host and port of the Runtime API.
    pub runtime_api: String,
    /// `FP_BASE_PATH`: the mount path, such as `/blog/`.
    pub base: BasePath,
    /// `FP_TABLE_PREFIX`: the DSQL table prefix, such as `fp_`.
    pub prefix: TablePrefix,
    /// `FP_TABLE`: the read-model table name.
    pub table: String,
    /// `FP_BUCKET`: the site bucket.
    pub bucket: String,
    /// `FP_DISTRIBUTION_ID`: the `CloudFront` distribution to invalidate.
    pub distribution: String,
    /// `FP_DYNAMODB_ENDPOINT` when set (local emulators), else the regional endpoint.
    pub dynamodb: Endpoint,
    /// `FP_S3_ENDPOINT` when set (path-style, local emulators), else virtual-hosted AWS.
    pub s3: Option<Endpoint>,
    /// `FP_CLOUDFRONT_ENDPOINT` when set (fakes), else the global endpoint.
    pub cloudfront: Endpoint,
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
        let prefix = TablePrefix::new(&required(&lookup, "FP_TABLE_PREFIX")?)
            .map_err(|_name| ConfigError::Invalid("FP_TABLE_PREFIX"))?;
        let endpoint = |name: &'static str| {
            lookup(name)
                .filter(|value| !value.is_empty())
                .map(|url| Endpoint::parse(&url).map_err(|_bad| ConfigError::Invalid(name)))
                .transpose()
        };
        let dynamodb = endpoint("FP_DYNAMODB_ENDPOINT")?
            .unwrap_or_else(|| DynamoDb::regional_endpoint(&environment.region));
        let cloudfront =
            endpoint("FP_CLOUDFRONT_ENDPOINT")?.unwrap_or_else(CloudFront::global_endpoint);
        Ok(Self {
            runtime_api: required(&lookup, "AWS_LAMBDA_RUNTIME_API")?,
            base,
            prefix,
            table: required(&lookup, "FP_TABLE")?,
            bucket: required(&lookup, "FP_BUCKET")?,
            distribution: required(&lookup, "FP_DISTRIBUTION_ID")?,
            dynamodb,
            s3: endpoint("FP_S3_ENDPOINT")?,
            cloudfront,
            environment,
        })
    }
}

/// Builds the adapters from `config` and serves invocations until the Runtime API fails.
///
/// `outbox` and `state` are two connections to the same database: the outbox is fetched and
/// deleted on one while the source reads current state on the other.
///
/// # Errors
///
/// [`StartError::Runtime`] when the Runtime API cannot be reached.
pub fn start<O: Store, S: Store>(config: Config, mut outbox: O, state: S) -> Result<(), StartError> {
    let aws = AwsClient::new(config.environment, AWS_TIMEOUT);
    let mut table = Table::new(DynamoDb::new(aws.clone(), config.dynamodb), &config.table);
    let mut edge = Edge::new(CloudFront::new(aws.clone(), config.cloudfront), &config.distribution);
    let mut objects = Objects(match config.s3 {
        Some(endpoint) => S3::new(aws, endpoint, &config.bucket, Addressing::Path),
        None => S3::regional(aws, &config.bucket),
    });
    let paths = Paths::new(config.base);
    let mut source = DbSource::new(state, config.prefix.clone());
    let mut metrics = std::io::stdout();
    run(&config.runtime_api, &SystemClock, &mut metrics, || {
        // Options may have changed since the last run.
        source.refresh();
        Projector {
            store: &mut outbox,
            prefix: &config.prefix,
            paths: &paths,
            source: &mut source,
            theme: &Native,
            cards: &mut NoCards,
            blobs: &mut objects,
            routes: &mut table,
            invalidator: &mut edge,
            clock: &SystemClock,
            nudger: &mut InProcess,
        }
        .run_once()
    })?;
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
                ("FP_TABLE_PREFIX", "fp_"),
                ("FP_TABLE", "fp-read"),
                ("FP_BUCKET", "site-bucket"),
                ("FP_DISTRIBUTION_ID", "E123"),
            ]
            .iter()
            .chain(extra)
            .rev()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn defaults_use_aws_endpoints_and_bad_values_are_named() {
        let config = Config::from_lookup(lookup(&[]));
        assert!(config.is_ok_and(|c| c.s3.is_none()
            && c.dynamodb == Endpoint::https("dynamodb.us-east-1.amazonaws.com")
            && c.cloudfront == Endpoint::https("cloudfront.amazonaws.com")
            && c.distribution == "E123"));
        assert_eq!(
            Config::from_lookup(lookup(&[("FP_CLOUDFRONT_ENDPOINT", "ftp://x")])),
            Err(StartError::Config(ConfigError::Invalid(
                "FP_CLOUDFRONT_ENDPOINT"
            )))
        );
        assert_eq!(
            Config::from_lookup(lookup(&[("FP_DISTRIBUTION_ID", "")])).map_err(|e| e.to_string()),
            Err("environment variable FP_DISTRIBUTION_ID is not set".to_owned())
        );
    }
}
