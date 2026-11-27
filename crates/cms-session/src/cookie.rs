//! The `fp_session` cookie: Set-Cookie values and reading it back from a Cookie header.

use crate::token::SessionToken;

/// Cookie name.
pub const NAME: &str = "fp_session";

/// Where the cookie is scoped. Built from the site's base path, for example `/blog/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookiePath(String);

impl CookiePath {
    /// `None` unless `path` starts with `/` and holds no byte that would break the attribute.
    #[must_use]
    pub fn new(path: &str) -> Option<Self> {
        let clean = path.starts_with('/')
            && path
                .bytes()
                .all(|b| b.is_ascii_graphic() && b != b';' && b != b',');
        clean.then(|| Self(path.to_owned()))
    }

    /// The path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Set-Cookie value for a new session. `max_age` is `Some` for remember-me (a persistent
    /// cookie) and `None` for a browser-session cookie, as the reference does.
    #[must_use]
    pub fn set(&self, token: &SessionToken, max_age: Option<u64>) -> String {
        let mut out = format!("{NAME}={}", token.expose());
        if let Some(seconds) = max_age {
            out.push_str("; Max-Age=");
            out.push_str(&seconds.to_string());
        }
        self.attributes(&mut out);
        out
    }

    /// Set-Cookie value that removes the cookie.
    #[must_use]
    pub fn clear(&self) -> String {
        let mut out = format!("{NAME}=; Max-Age=0");
        self.attributes(&mut out);
        out
    }

    fn attributes(&self, out: &mut String) {
        out.push_str("; Path=");
        out.push_str(&self.0);
        out.push_str("; HttpOnly; Secure; SameSite=Lax");
    }
}

/// The session token in a Cookie request header, if one is present and well formed.
///
/// The first `fp_session` pair wins, as browsers send the most specific path first.
#[must_use]
pub fn read(header: &str) -> Option<SessionToken> {
    header
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == NAME)
        .and_then(|(_, value)| SessionToken::parse(value.trim_matches('"')))
}

#[cfg(test)]
mod tests {
    use super::{CookiePath, read};
    use crate::token::SessionToken;

    fn token() -> Option<SessionToken> {
        SessionToken::parse(&"ab".repeat(32))
    }

    #[test]
    fn set_and_clear_carry_every_attribute() {
        let path = CookiePath::new("/blog/");
        assert!(path.is_some());
        if let (Some(path), Some(token)) = (path, token()) {
            assert_eq!(path.as_str(), "/blog/");
            let hex = "ab".repeat(32);
            assert_eq!(
                path.set(&token, None),
                format!("fp_session={hex}; Path=/blog/; HttpOnly; Secure; SameSite=Lax")
            );
            assert_eq!(
                path.set(&token, Some(1_209_600)),
                format!(
                    "fp_session={hex}; Max-Age=1209600; Path=/blog/; HttpOnly; Secure; SameSite=Lax"
                )
            );
            assert_eq!(
                path.clear(),
                "fp_session=; Max-Age=0; Path=/blog/; HttpOnly; Secure; SameSite=Lax"
            );
        }
        assert!(CookiePath::new("blog/").is_none());
        assert!(CookiePath::new("/a;b").is_none());
        assert!(CookiePath::new("/a b").is_none());
        assert!(CookiePath::new("/a,b").is_none());
    }

    #[test]
    fn read_finds_the_first_well_formed_session_pair() {
        let hex = "ab".repeat(32);
        assert_eq!(read(&format!("a=1; fp_session={hex}; b=2")), token());
        assert_eq!(read(&format!("fp_session=\"{hex}\"")), token());
        assert_eq!(read(&format!("fp_session={hex}; fp_session=00")), token());
        assert!(read("fp_session=nope").is_none());
        assert!(read("other=1").is_none());
        assert!(read("").is_none());
    }
}
