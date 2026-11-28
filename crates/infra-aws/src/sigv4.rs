//! AWS Signature Version 4: header signing and query-string presigning.
//!
//! Implemented from the published `SigV4` specification and checked against AWS's signing test
//! suite (see the tests below).

use crate::digest::{hex, hmac_sha256, sha256_hex};
use crate::{AmzDate, Credentials};

/// SHA-256 of the empty payload.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// Who signs, for which region and service.
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    /// Signing credentials.
    pub credentials: &'a Credentials,
    /// Signing region, such as `us-east-1`.
    pub region: &'a str,
    /// Signing service name, such as `dynamodb`, `s3`, `cloudfront` or `dsql`.
    pub service: &'a str,
}

/// How the canonical URI is derived from the path sent on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStyle {
    /// Every service but S3: the wire path is URI-encoded once more.
    Standard,
    /// S3: the wire path (already encoded once) is the canonical URI.
    S3,
}

/// The parts of a request the signature covers.
#[derive(Clone, Copy, Debug)]
pub struct Signable<'a> {
    /// HTTP method.
    pub method: &'a str,
    /// Path as sent on the wire, starting with `/`.
    pub path: &'a str,
    /// Decoded query parameters; they are encoded and sorted here.
    pub query: &'a [(&'a str, &'a str)],
    /// Headers to sign; must include `host`.
    pub headers: &'a [(&'a str, &'a str)],
    /// Lowercase hex SHA-256 of the body (or `UNSIGNED-PAYLOAD` where a service allows it).
    pub payload_sha256: &'a str,
    /// Canonical URI derivation.
    pub path_style: PathStyle,
}

// Everything derived while signing; tests compare each step with the suite.
#[derive(Debug)]
struct Parts {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "inspected by the suite tests only")
    )]
    canonical_request: String,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "inspected by the suite tests only")
    )]
    string_to_sign: String,
    signed_headers: String,
    signature: String,
    credential: String,
}

/// URI-encodes `input` the `SigV4` way: unreserved characters stay, everything else becomes
/// `%XX` with uppercase hex; `/` stays only when `keep_slash`.
#[must_use]
pub fn uri_encode(input: &str, keep_slash: bool) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && byte == b'/')
        {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(DIGITS[usize::from(byte >> 4)]));
            out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
    }
    out
}

/// Encoded, sorted `key=value&...` text for decoded `query` pairs; also the wire query string.
#[must_use]
pub fn canonical_query(query: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(key, value)| (uri_encode(key, false), uri_encode(value, false)))
        .collect();
    pairs.sort();
    let mut out = String::new();
    for (key, value) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&key);
        out.push('=');
        out.push_str(&value);
    }
    out
}

/// Signs `request` in headers. Returns the headers to add, in order: `x-amz-date`,
/// `x-amz-security-token` (temporary credentials only) and `authorization`.
#[must_use]
pub fn sign(scope: &Scope<'_>, request: &Signable<'_>, at: AmzDate) -> Vec<(&'static str, String)> {
    let stamp = at.stamp();
    let mut added = vec![("x-amz-date", stamp)];
    if let Some(token) = scope.credentials.session_token() {
        added.push(("x-amz-security-token", token.to_owned()));
    }
    let extra: Vec<(&str, &str)> = added.iter().map(|(n, v)| (*n, v.as_str())).collect();
    let parts = parts(scope, request, &extra, request.query, at);
    added.push((
        "authorization",
        format!(
            "{ALGORITHM} Credential={}, SignedHeaders={}, Signature={}",
            parts.credential, parts.signed_headers, parts.signature
        ),
    ));
    added
}

/// Presigns `request` for `expires_secs` seconds. Returns the complete query string (the
/// caller's parameters plus the `X-Amz-*` ones, ending with `X-Amz-Signature`).
#[must_use]
pub fn presign(
    scope: &Scope<'_>,
    request: &Signable<'_>,
    at: AmzDate,
    expires_secs: u64,
) -> String {
    let (_, signed_headers) = canonical_headers(request.headers, &[]);
    let credential = credential(scope, at);
    let stamp = at.stamp();
    let expires = expires_secs.to_string();
    let mut query: Vec<(&str, &str)> = request.query.to_vec();
    query.extend([
        ("X-Amz-Algorithm", ALGORITHM),
        ("X-Amz-Credential", credential.as_str()),
        ("X-Amz-Date", stamp.as_str()),
        ("X-Amz-Expires", expires.as_str()),
        ("X-Amz-SignedHeaders", signed_headers.as_str()),
    ]);
    if let Some(token) = scope.credentials.session_token() {
        query.push(("X-Amz-Security-Token", token));
    }
    let parts = parts(scope, request, &[], &query, at);
    format!(
        "{}&X-Amz-Signature={}",
        canonical_query(&query),
        parts.signature
    )
}

fn credential(scope: &Scope<'_>, at: AmzDate) -> String {
    format!(
        "{}/{}",
        scope.credentials.access_key_id(),
        credential_scope(scope, at)
    )
}

fn credential_scope(scope: &Scope<'_>, at: AmzDate) -> String {
    format!(
        "{}/{}/{}/aws4_request",
        at.day(),
        scope.region,
        scope.service
    )
}

fn parts(
    scope: &Scope<'_>,
    request: &Signable<'_>,
    extra_headers: &[(&str, &str)],
    query: &[(&str, &str)],
    at: AmzDate,
) -> Parts {
    let (headers, signed_headers) = canonical_headers(request.headers, extra_headers);
    let uri = match request.path_style {
        PathStyle::Standard => uri_encode(request.path, true),
        PathStyle::S3 => request.path.to_owned(),
    };
    let canonical_request = format!(
        "{}\n{uri}\n{}\n{headers}\n{signed_headers}\n{}",
        request.method,
        canonical_query(query),
        request.payload_sha256
    );
    let string_to_sign = format!(
        "{ALGORITHM}\n{}\n{}\n{}",
        at.stamp(),
        credential_scope(scope, at),
        sha256_hex(canonical_request.as_bytes())
    );
    // Key derivation chain from the spec: date, region, service, terminator.
    let secret = format!("AWS4{}", scope.credentials.secret_access_key());
    let key = hmac_sha256(secret.as_bytes(), at.day().as_bytes());
    let key = hmac_sha256(&key, scope.region.as_bytes());
    let key = hmac_sha256(&key, scope.service.as_bytes());
    let key = hmac_sha256(&key, b"aws4_request");
    let signature = hex(&hmac_sha256(&key, string_to_sign.as_bytes()));
    Parts {
        canonical_request,
        string_to_sign,
        signed_headers,
        signature,
        credential: credential(scope, at),
    }
}

// Lowercase names, trimmed values with inner whitespace runs collapsed, sorted by name, and
// repeated names joined with commas. Returns the header block (each line ends in `\n`) and the
// `;`-joined signed header list.
fn canonical_headers(headers: &[(&str, &str)], extra: &[(&str, &str)]) -> (String, String) {
    let mut all: Vec<(String, String)> = headers
        .iter()
        .chain(extra)
        .map(|(name, value)| {
            (
                name.to_ascii_lowercase(),
                value.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let mut merged: Vec<(String, String)> = Vec::with_capacity(all.len());
    for (name, value) in all {
        match merged.last_mut() {
            Some(last) if last.0 == name => {
                last.1.push(',');
                last.1.push_str(&value);
            }
            _ => merged.push((name, value)),
        }
    }
    let mut block = String::new();
    let mut names = String::new();
    for (name, value) in &merged {
        block.push_str(name);
        block.push(':');
        block.push_str(value);
        block.push('\n');
        if !names.is_empty() {
            names.push(';');
        }
        names.push_str(name);
    }
    (block, names)
}

#[cfg(test)]
mod tests {
    //! Vectors transcribed from AWS's `SigV4` signing test suite (`v4/<name>/`): every case uses
    //! the suite credentials, region `us-east-1`, service `service` and 2015-08-30T12:36:00Z;
    //! presigned cases expire in 3600 s.

    use super::{
        EMPTY_SHA256, PathStyle, Scope, Signable, canonical_query, parts, presign, sign, uri_encode,
    };
    use crate::{AmzDate, Credentials};

    const AT: AmzDate = AmzDate::from_unix(1_440_938_160);
    const HOST: (&str, &str) = ("Host", "example.amazonaws.com");
    const TOKEN: &str = "6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267";
    const FORM_SHA256: &str = "9095672bbd1f56dfc5b65f3e153adc8731a4a654192329106275f4c7b24d0b6e";

    fn suite(token: Option<&str>) -> Credentials {
        Credentials::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            token,
        )
    }

    fn request<'a>(
        method: &'a str,
        path: &'a str,
        query: &'a [(&'a str, &'a str)],
        headers: &'a [(&'a str, &'a str)],
        payload_sha256: &'a str,
    ) -> Signable<'a> {
        Signable {
            method,
            path,
            query,
            headers,
            payload_sha256,
            path_style: PathStyle::Standard,
        }
    }

    fn authorization(credentials: &Credentials, request: &Signable<'_>) -> Option<String> {
        let scope = Scope {
            credentials,
            region: "us-east-1",
            service: "service",
        };
        sign(&scope, request, AT)
            .into_iter()
            .find(|(name, _)| *name == "authorization")
            .map(|(_, value)| value)
    }

    fn header_signature(credentials: &Credentials, request: &Signable<'_>) -> String {
        authorization(credentials, request)
            .and_then(|value| value.rsplit("Signature=").next().map(str::to_owned))
            .unwrap_or_default()
    }

    fn query_signature(credentials: &Credentials, request: &Signable<'_>) -> String {
        let scope = Scope {
            credentials,
            region: "us-east-1",
            service: "service",
        };
        presign(&scope, request, AT, 3600)
            .rsplit("X-Amz-Signature=")
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn get_vanilla() {
        let credentials = suite(None);
        let headers = [HOST];
        let req = request("GET", "/", &[], &headers, EMPTY_SHA256);
        let scope = Scope {
            credentials: &credentials,
            region: "us-east-1",
            service: "service",
        };
        let step = parts(&scope, &req, &[("x-amz-date", "20150830T123600Z")], &[], AT);
        assert_eq!(
            step.canonical_request,
            "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\n\
             host;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            step.string_to_sign,
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
        );
        assert_eq!(
            authorization(&credentials, &req).as_deref(),
            Some(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders=host;x-amz-date, \
                 Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
            )
        );
        assert_eq!(
            query_signature(&credentials, &req),
            "e93c787ed7f371d5c6b165c1b38ede9550f4dce4144713e844b25b7192d3865d"
        );
        let scope_text = presign(&scope, &req, AT, 3600);
        assert!(scope_text.starts_with(
            "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20150830%2F\
             us-east-1%2Fservice%2Faws4_request&X-Amz-Date=20150830T123600Z&\
             X-Amz-Expires=3600&X-Amz-SignedHeaders=host&X-Amz-Signature="
        ));
    }

    #[test]
    fn get_vanilla_query_order_key_case() {
        let credentials = suite(None);
        let headers = [HOST];
        let query = [("Param2", "value2"), ("Param1", "value1")];
        let req = request("GET", "/", &query, &headers, EMPTY_SHA256);
        assert_eq!(
            header_signature(&credentials, &req),
            "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
        );
        assert_eq!(
            query_signature(&credentials, &req),
            "86012e2c9ad4d77369f5d81c11f75158aae4f895a085212cc6d3f923d300bed5"
        );
    }

    #[test]
    fn get_header_value_trim() {
        let credentials = suite(None);
        let headers = [
            HOST,
            ("My-Header1", " value1"),
            ("My-Header2", " \"a   b   c\""),
        ];
        let req = request("GET", "/", &[], &headers, EMPTY_SHA256);
        assert_eq!(
            authorization(&credentials, &req).as_deref(),
            Some(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders=host;my-header1;my-header2;x-amz-date, \
                 Signature=acc3ed3afb60bb290fc8d2dd0098b9911fcaa05412b367055dee359757a9c736"
            )
        );
        assert_eq!(
            query_signature(&credentials, &req),
            "e7bb0fd515e125e1aec2ecc4c0c17484fb06f6846b927c35e46005dd3df3acd4"
        );
    }

    #[test]
    fn get_utf8_and_utf8_query() {
        let credentials = suite(None);
        let headers = [HOST];
        let path = request("GET", "/\u{1234}", &[], &headers, EMPTY_SHA256);
        assert_eq!(
            header_signature(&credentials, &path),
            "8318018e0b0f223aa2bbf98705b62bb787dc9c0e678f255a891fd03141be5d85"
        );
        let query = [("\u{1234}", "bar")];
        let keyed = request("GET", "/", &query, &headers, EMPTY_SHA256);
        assert_eq!(
            header_signature(&credentials, &keyed),
            "2cdec8eed098649ff3a119c94853b13c643bcf08f8b0a1d91e12c9027818dd04"
        );
    }

    #[test]
    fn get_vanilla_with_session_token() {
        let credentials = suite(Some(TOKEN));
        let headers = [HOST];
        let req = request("GET", "/", &[], &headers, EMPTY_SHA256);
        assert_eq!(
            authorization(&credentials, &req).as_deref(),
            Some(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders=host;x-amz-date;x-amz-security-token, \
                 Signature=07ec1639c89043aa0e3e2de82b96708f198cceab042d4a97044c66dd9f74e7f8"
            )
        );
        assert_eq!(
            query_signature(&credentials, &req),
            "7ff2b50b376cb4d151970630573d6291dc128cc5c2a12ffb237f73cc53f67b6c"
        );
    }

    #[test]
    fn post_vanilla() {
        let credentials = suite(None);
        let headers = [HOST];
        let req = request("POST", "/", &[], &headers, EMPTY_SHA256);
        assert_eq!(
            header_signature(&credentials, &req),
            "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
        );
    }

    #[test]
    fn post_x_www_form_urlencoded() {
        let credentials = suite(None);
        let base = [
            ("Content-Type", "application/x-www-form-urlencoded"),
            HOST,
            ("Content-Length", "13"),
        ];
        // The suite's header case signs the body hash as x-amz-content-sha256; the query case
        // does not send that header.
        let mut signed_body = base.to_vec();
        signed_body.push(("x-amz-content-sha256", FORM_SHA256));
        let header_req = request("POST", "/", &[], &signed_body, FORM_SHA256);
        assert_eq!(
            authorization(&credentials, &header_req).as_deref(),
            Some(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders=content-length;content-type;host;x-amz-content-sha256;x-amz-date, \
                 Signature=d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b"
            )
        );
        let query_req = request("POST", "/", &[], &base, FORM_SHA256);
        assert_eq!(
            query_signature(&credentials, &query_req),
            "89a40deed0f26f9461242825a082d2222717248abc7ab41f552ad84a94ad46e9"
        );
    }

    #[test]
    fn encoding_and_header_merging_rules() {
        assert_eq!(uri_encode("a b/c~d", true), "a%20b/c~d");
        assert_eq!(uri_encode("a b/c", false), "a%20b%2Fc");
        assert_eq!(
            canonical_query(&[("b", "2"), ("a", "x y"), ("a", "1")]),
            "a=1&a=x%20y&b=2"
        );
        let credentials = suite(None);
        let headers = [HOST, ("X-Dup", "a"), ("x-dup", "b")];
        let mut req = request("GET", "/k%20ey", &[], &headers, EMPTY_SHA256);
        req.path_style = PathStyle::S3;
        let scope = Scope {
            credentials: &credentials,
            region: "us-east-1",
            service: "s3",
        };
        let step = parts(&scope, &req, &[], &[], AT);
        assert!(step.canonical_request.starts_with("GET\n/k%20ey\n\n"));
        assert!(step.canonical_request.contains("\nx-dup:a,b\n"));
        assert_eq!(step.signed_headers, "host;x-dup");
    }
}
