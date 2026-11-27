//! Secret redaction applied to every log field and metric property before it is stored.
//!
//! Matching is by field or parameter name, ignoring case and every non-alphanumeric character,
//! so `Set-Cookie`, `set_cookie` and `SETCOOKIE` are the same name.

use serde_json::Value;

/// Replacement text for a redacted value.
pub const REDACTED: &str = "[redacted]";

/// Whole normalized names that are secrets but too short to match as a suffix safely
/// (`pass` as a suffix would also catch `bypass`).
const SECRET_NAMES: &[&str] = &["pass", "pwd", "userpass"];

/// Normalized name endings that mark a secret: credentials, session material, signed-request
/// headers (`x-amz-security-token`, `x-amz-signature`) and form nonces.
const SECRET_SUFFIXES: &[&str] = &[
    "password",
    "passwd",
    "token",
    "secret",
    "nonce",
    "salt",
    "cookie",
    "cookies",
    "authorization",
    "apikey",
    "accesskey",
    "secretkey",
    "privatekey",
    "signature",
    "credential",
    "credentials",
    "sessionid",
];

/// Whether a field, header or query parameter called `name` holds a secret.
///
/// # Examples
///
/// ```
/// use cms_telemetry::redact::is_secret;
///
/// assert!(is_secret("X-Amz-Security-Token"));
/// assert!(is_secret("post_password"));
/// assert!(!is_secret("route_kind"));
/// ```
#[must_use]
pub fn is_secret(name: &str) -> bool {
    let normalized: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    SECRET_NAMES.contains(&normalized.as_str())
        || SECRET_SUFFIXES
            .iter()
            .any(|suffix| normalized.ends_with(suffix))
}

/// Returns `value` with every secret removed, as stored under the field `key`.
///
/// A secret key replaces the whole value with [`REDACTED`]. Objects are scrubbed entry by entry
/// (so a header map loses `authorization` and `cookie`), arrays element by element, and strings
/// under a query or URL key lose the values of secret query parameters.
#[must_use]
pub fn scrub(key: &str, value: Value) -> Value {
    if is_secret(key) {
        return Value::String(REDACTED.to_owned());
    }
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(name, inner)| {
                    let clean = scrub(&name, inner);
                    (name, clean)
                })
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|item| scrub(key, item)).collect())
        }
        Value::String(text) if holds_query(key) => Value::String(redact_url(&text)),
        other => other,
    }
}

/// Whether strings stored under `key` are query strings or URLs that may carry one.
fn holds_query(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "query",
        "querystring",
        "url",
        "uri",
        "path",
        "location",
        "referer",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
}

/// Redacts secret parameters in a URL or path, or in a bare query string with no `?`.
fn redact_url(text: &str) -> String {
    match text.split_once('?') {
        Some((head, query)) => format!("{head}?{}", redact_query(query)),
        None if text.contains('=') => redact_query(text),
        None => text.to_owned(),
    }
}

/// Replaces the values of secret parameters in a raw query string (`a=1&post_password=x`).
///
/// Parameter order and every non-secret pair are kept byte for byte.
///
/// # Examples
///
/// ```
/// use cms_telemetry::redact::redact_query;
///
/// assert_eq!(redact_query("p=4&_nonce=ab12"), "p=4&_nonce=[redacted]");
/// ```
#[must_use]
pub fn redact_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) if is_secret(name) => format!("{name}={REDACTED}"),
            _ => pair.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{REDACTED, is_secret, redact_query, scrub};

    #[test]
    fn every_listed_secret_name_is_recognized_in_any_spelling() {
        for name in [
            "cookie",
            "Cookie",
            "Set-Cookie",
            "authorization",
            "Proxy-Authorization",
            "x-amz-security-token",
            "X-Amz-Signature",
            "X-Amz-Credential",
            "password",
            "user_pass",
            "pass",
            "pwd",
            "post_password",
            "PostPassword",
            "token",
            "refresh_token",
            "_nonce",
            "auth_salt",
            "client_secret",
            "api_key",
            "session_id",
        ] {
            assert!(is_secret(name), "{name}");
        }
    }

    #[test]
    fn ordinary_names_are_not_secrets() {
        for name in [
            "route_kind",
            "status",
            "bypass",
            "key",
            "monkey",
            "passage",
            "request_id",
            "site",
        ] {
            assert!(!is_secret(name), "{name}");
        }
    }

    #[test]
    fn secret_keys_lose_their_whole_value() {
        assert_eq!(scrub("cookie", json!("a=b")), json!(REDACTED));
        assert_eq!(scrub("token", json!({"inner": 1})), json!(REDACTED));
    }

    #[test]
    fn objects_and_arrays_are_scrubbed_recursively() {
        let headers = json!({
            "Authorization": "AWS4-HMAC-SHA256 Credential=x",
            "accept": "text/html",
            "nested": [{"x-amz-security-token": "t", "etag": "\"e\""}]
        });
        assert_eq!(
            scrub("headers", headers),
            json!({
                "Authorization": REDACTED,
                "accept": "text/html",
                "nested": [{"x-amz-security-token": REDACTED, "etag": "\"e\""}]
            })
        );
    }

    #[test]
    fn query_and_url_strings_lose_secret_parameters() {
        assert_eq!(
            scrub("raw_query", json!("p=1&post_password=hunter2")),
            json!("p=1&post_password=[redacted]")
        );
        assert_eq!(
            scrub("url", json!("/blog/?pwd=x&paged=2")),
            json!("/blog/?pwd=[redacted]&paged=2")
        );
        assert_eq!(scrub("path", json!("/blog/a/")), json!("/blog/a/"));
        assert_eq!(scrub("message", json!("pwd=x")), json!("pwd=x"));
        assert_eq!(scrub("status", json!(200)), json!(200));
    }

    #[test]
    fn query_redaction_keeps_order_and_bare_flags() {
        assert_eq!(
            redact_query("feed&token=a&b=2&Secret=c"),
            "feed&token=[redacted]&b=2&Secret=[redacted]"
        );
        assert_eq!(redact_query(""), "");
    }
}
