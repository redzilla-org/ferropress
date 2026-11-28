//! Secrets Manager `GetSecretValue` over its JSON protocol (`x-amz-json-1.1`).

use serde_json::{Value, json};

use crate::http::Call;
use crate::sigv4::PathStyle;
use crate::{AwsClient, AwsError, Endpoint};

/// A Secrets Manager client for one endpoint.
#[derive(Clone, Debug)]
pub struct SecretsManager {
    aws: AwsClient,
    endpoint: Endpoint,
}

impl SecretsManager {
    /// A client reaching `endpoint`.
    #[must_use]
    pub fn new(aws: AwsClient, endpoint: Endpoint) -> Self {
        Self { aws, endpoint }
    }

    /// The public regional endpoint `https://secretsmanager.<region>.amazonaws.com`.
    #[must_use]
    pub fn regional_endpoint(region: &str) -> Endpoint {
        Endpoint::https(&format!("secretsmanager.{region}.amazonaws.com"))
    }

    /// The current `SecretString` of `secret_id` (name or ARN). The caller owns the secret:
    /// it must never be logged, and no error carries it.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails or the secret has no string value.
    pub fn get_secret_string(&self, secret_id: &str) -> Result<String, AwsError> {
        const OP: &str = "SecretsManager.GetSecretValue";
        let body = json!({ "SecretId": secret_id }).to_string();
        let reply = self
            .aws
            .send(&Call {
                operation: OP,
                service: "secretsmanager",
                region: None,
                path_style: PathStyle::Standard,
                endpoint: &self.endpoint,
                method: "POST",
                path: "/",
                query: &[],
                headers: &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("x-amz-target", "secretsmanager.GetSecretValue"),
                ],
                body: body.as_bytes(),
            })?
            .success(OP)?;
        // Only the string form is used; a binary-only secret is a deployment defect.
        match serde_json::from_slice::<Value>(&reply.body) {
            Ok(Value::Object(mut map)) => match map.remove("SecretString") {
                Some(Value::String(secret)) => Ok(secret),
                _ => Err(AwsError::Malformed {
                    operation: OP,
                    what: "SecretString is missing",
                }),
            },
            _ => Err(AwsError::Malformed {
                operation: OP,
                what: "body is not a JSON object",
            }),
        }
    }
}
