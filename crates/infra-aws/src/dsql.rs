//! Aurora DSQL IAM authentication tokens.
//!
//! A token is a SigV4-presigned `GET https://<cluster endpoint>/?Action=DbConnect[Admin]` for
//! service `dsql`, with the scheme removed. It is used as the `PostgreSQL` password.

use crate::sigv4::{EMPTY_SHA256, PathStyle, Scope, Signable, presign};
use crate::{AmzDate, Credentials};

/// AWS's default token lifetime (15 minutes).
pub const DEFAULT_TOKEN_TTL_SECS: u64 = 900;

/// Which database role the token authenticates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The `admin` role (`DbConnectAdmin`), for migrations.
    Admin,
    /// A custom role mapped to the IAM principal (`DbConnect`), for application traffic.
    Custom,
}

impl Role {
    /// The presigned action name.
    #[must_use]
    pub const fn action(self) -> &'static str {
        match self {
            Self::Admin => "DbConnectAdmin",
            Self::Custom => "DbConnect",
        }
    }
}

/// An auth token for `hostname` (the cluster endpoint, no scheme or port), valid for
/// `ttl_secs` from `at`.
#[must_use]
pub fn auth_token(
    credentials: &Credentials,
    region: &str,
    hostname: &str,
    role: Role,
    at: AmzDate,
    ttl_secs: u64,
) -> String {
    let scope = Scope {
        credentials,
        region,
        service: "dsql",
    };
    let headers = [("host", hostname)];
    let query = [("Action", role.action())];
    let signable = Signable {
        method: "GET",
        path: "/",
        query: &query,
        headers: &headers,
        payload_sha256: EMPTY_SHA256,
        path_style: PathStyle::Standard,
    };
    format!("{hostname}/?{}", presign(&scope, &signable, at, ttl_secs))
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TOKEN_TTL_SECS, Role, auth_token};
    use crate::sigv4::{EMPTY_SHA256, PathStyle, Scope, Signable, presign};
    use crate::{AmzDate, Credentials};

    const HOST: &str = "abc123.dsql.us-east-1.on.aws";

    #[test]
    fn tokens_are_presigned_connect_urls_without_scheme() {
        let credentials = Credentials::new("AKIDEXAMPLE", "secret", Some("tok/en+"));
        let at = AmzDate::from_unix(1_440_938_160);
        let token = auth_token(
            &credentials,
            "us-east-1",
            HOST,
            Role::Custom,
            at,
            DEFAULT_TOKEN_TTL_SECS,
        );
        assert!(token.starts_with(
            "abc123.dsql.us-east-1.on.aws/?Action=DbConnect&X-Amz-Algorithm=AWS4-HMAC-SHA256&\
             X-Amz-Credential=AKIDEXAMPLE%2F20150830%2Fus-east-1%2Fdsql%2Faws4_request&\
             X-Amz-Date=20150830T123600Z&X-Amz-Expires=900&X-Amz-Security-Token=tok%2Fen%2B&\
             X-Amz-SignedHeaders=host&X-Amz-Signature="
        ));
        let signature = token.rsplit('=').next().unwrap_or_default();
        assert_eq!(signature.len(), 64);
        assert!(signature.bytes().all(|b| b.is_ascii_hexdigit()));
        // Same bytes as a direct presign of the documented request.
        let scope = Scope {
            credentials: &credentials,
            region: "us-east-1",
            service: "dsql",
        };
        let direct = presign(
            &scope,
            &Signable {
                method: "GET",
                path: "/",
                query: &[("Action", "DbConnect")],
                headers: &[("host", HOST)],
                payload_sha256: EMPTY_SHA256,
                path_style: PathStyle::Standard,
            },
            at,
            900,
        );
        assert_eq!(token, format!("{HOST}/?{direct}"));
    }

    #[test]
    fn admin_tokens_use_the_admin_action() {
        let credentials = Credentials::new("AKID", "secret", None);
        let token = auth_token(
            &credentials,
            "us-east-1",
            HOST,
            Role::Admin,
            AmzDate::from_unix(0),
            60,
        );
        assert!(token.contains("/?Action=DbConnectAdmin&"));
        assert!(token.contains("X-Amz-Expires=60&"));
        assert!(!token.contains("Security-Token"));
        assert_eq!(Role::Admin.action(), "DbConnectAdmin");
        assert_eq!(Role::Custom.action(), "DbConnect");
    }
}
