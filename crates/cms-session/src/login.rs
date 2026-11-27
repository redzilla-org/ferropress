//! The login service: credentials in, a session and a Set-Cookie out; logout; current user.

use std::fmt;

use cms_auth::{Actor, Caps, GrantMap, RoleTable, map_meta};
use cms_db::Store;
use cms_users::{User, UserId, Users};

use crate::cookie::{CookiePath, read};
use crate::error::Error;
use crate::mac::sha256_hex;
use crate::password::{Password, hash_password, needs_rehash, verify};
use crate::ports::{RateKey, RateLimiter, Session, SessionStore};
use crate::token::{Entropy, SessionToken};

/// Session lifetime without remember-me (two days), the reference default.
pub const SESSION_SECONDS: u64 = 2 * 86_400;
/// Session lifetime with remember-me (fourteen days), the reference default.
pub const REMEMBER_SECONDS: u64 = 14 * 86_400;

/// Login attempt limits. Each attempt counts before the password is checked, so a flood cannot
/// buy bcrypt work either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Attempts allowed from one client address per window.
    pub per_ip: u64,
    /// Attempts allowed against one login name per window.
    pub per_login: u64,
    /// Fixed window length in seconds.
    pub window: u64,
}

impl Default for Limits {
    /// 20 attempts per address and 5 per login name in each 5-minute window.
    fn default() -> Self {
        Self {
            per_ip: 20,
            per_login: 5,
            window: 300,
        }
    }
}

/// A submitted login form. [`Debug`] shows the login name, never the password.
#[derive(Clone, Debug)]
pub struct LoginRequest {
    /// User login or email address as typed.
    pub login: String,
    /// Password as typed.
    pub password: Password,
    /// Client address, as the edge reported it.
    pub ip: String,
    /// The request's User-Agent; only its sha256 is stored.
    pub user_agent: String,
    /// The remember-me checkbox.
    pub remember: bool,
}

/// Why a login did not produce a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// The login name or the password was empty.
    Empty,
    /// No user has that login name or email address.
    UnknownUser,
    /// The password did not match.
    BadPassword,
    /// Too many attempts from this address or against this login name.
    Limited,
}

/// A successful login. [`Debug`] omits the token and the cookie.
#[derive(Clone, PartialEq, Eq)]
pub struct LoggedIn {
    user: User,
    token: SessionToken,
    session: Session,
    set_cookie: String,
    rehashed: bool,
}

impl LoggedIn {
    /// The user that logged in.
    #[must_use]
    pub fn user(&self) -> &User {
        &self.user
    }

    /// The new token, for nonces issued in the same response. Never log it.
    #[must_use]
    pub fn token(&self) -> &SessionToken {
        &self.token
    }

    /// The stored session.
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The Set-Cookie header value. It carries the token; never log it.
    #[must_use]
    pub fn set_cookie(&self) -> &str {
        &self.set_cookie
    }

    /// Whether the stored hash was replaced by the current form.
    #[must_use]
    pub fn rehashed(&self) -> bool {
        self.rehashed
    }
}

impl fmt::Debug for LoggedIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoggedIn")
            .field("user", &self.user.id())
            .field("session", &self.session)
            .field("rehashed", &self.rehashed)
            .finish_non_exhaustive()
    }
}

/// What a login attempt produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A session was created.
    LoggedIn(Box<LoggedIn>),
    /// No session; the reason is for the login page, not for the client verbatim.
    Rejected(Rejection),
}

/// The user and session behind a request's cookie. [`Debug`] omits the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The session's user row.
    pub user: User,
    /// The stored session.
    pub session: Session,
    /// The cookie token, for nonces. Never log it.
    pub token: SessionToken,
}

impl fmt::Debug for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolved")
            .field("user", &self.user.id())
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

/// The login service over a user store, a session store, a rate limiter and a random source.
pub struct Login<S, T, R, E> {
    users: Users<S>,
    sessions: T,
    limiter: R,
    entropy: E,
    path: CookiePath,
    limits: Limits,
}

impl<S: Store, T: SessionStore, R: RateLimiter, E: Entropy> Login<S, T, R, E> {
    /// A service issuing cookies scoped to `path`.
    pub fn new(
        users: Users<S>,
        sessions: T,
        limiter: R,
        entropy: E,
        path: CookiePath,
        limits: Limits,
    ) -> Self {
        Self {
            users,
            sessions,
            limiter,
            entropy,
            path,
            limits,
        }
    }

    /// Check credentials at unix time `now` and create a session.
    ///
    /// Order: rate limits, empty fields, lookup by login then by email, password check, rehash,
    /// session write.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when a store or the limiter is down, [`Error::Entropy`] or
    /// [`Error::Hash`] when a token or a rehash cannot be made.
    pub fn login(&mut self, request: &LoginRequest, now: u64) -> Result<Outcome, Error> {
        let login = normalize_login(&request.login);
        if self.limited(&request.ip, &login, now)? {
            return Ok(Outcome::Rejected(Rejection::Limited));
        }
        // The reference trims surrounding whitespace from the password before checking it.
        let password = request.password.trimmed();
        if login.is_empty() || password.is_empty() {
            return Ok(Outcome::Rejected(Rejection::Empty));
        }
        let Some(user) = self.lookup(&login)? else {
            return Ok(Outcome::Rejected(Rejection::UnknownUser));
        };
        if !verify(&password, user.password()) {
            return Ok(Outcome::Rejected(Rejection::BadPassword));
        }
        let rehashed = needs_rehash(user.password());
        if rehashed {
            let hash = hash_password(&password, &mut self.entropy)?;
            self.users.set_password(user.id(), hash.as_bytes())?;
        }
        let token = SessionToken::generate(&mut self.entropy)?;
        let (lifetime, max_age) = if request.remember {
            (REMEMBER_SECONDS, Some(REMEMBER_SECONDS))
        } else {
            (SESSION_SECONDS, None)
        };
        let session = Session {
            digest: token.digest(),
            user: user.id(),
            created: now,
            expires: now.saturating_add(lifetime),
            agent: sha256_hex(request.user_agent.as_bytes()),
        };
        self.sessions.put(&session)?;
        let set_cookie = self.path.set(&token, max_age);
        Ok(Outcome::LoggedIn(Box::new(LoggedIn {
            user,
            token,
            session,
            set_cookie,
            rehashed,
        })))
    }

    /// The live session and user behind a Cookie header at unix time `now`, or `None`.
    ///
    /// An expired session, or one whose user is gone, is deleted.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when a store is down.
    pub fn resolve(&mut self, cookie_header: &str, now: u64) -> Result<Option<Resolved>, Error> {
        let Some(token) = read(cookie_header) else {
            return Ok(None);
        };
        let digest = token.digest();
        let Some(session) = self.sessions.get(&digest)? else {
            return Ok(None);
        };
        if session.expires <= now {
            self.sessions.delete(&digest)?;
            return Ok(None);
        }
        let Some(user) = self.users.get(session.user)? else {
            self.sessions.delete(&digest)?;
            return Ok(None);
        };
        Ok(Some(Resolved {
            user,
            session,
            token,
        }))
    }

    /// End the session behind a Cookie header, if any, and return the clearing Set-Cookie value.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the session store is down.
    pub fn logout(&mut self, cookie_header: &str) -> Result<String, Error> {
        if let Some(token) = read(cookie_header) {
            self.sessions.delete(&token.digest())?;
        }
        Ok(self.path.clear())
    }

    /// End every session of `user`, returning how many were removed.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when the session store is down.
    pub fn logout_everywhere(&mut self, user: UserId) -> Result<u64, Error> {
        self.sessions.delete_all_for_user(user)
    }

    // Both counters move on every attempt; either one over its limit rejects.
    fn limited(&mut self, ip: &str, login: &str, now: u64) -> Result<bool, Error> {
        let window = self.limits.window.max(1);
        let bucket = now / window;
        let expires = (bucket + 2).saturating_mul(window);
        let by_ip = self.limiter.increment(&RateKey {
            name: format!("RL#login-ip#{ip}#{bucket}"),
            expires,
        })?;
        let by_login = self.limiter.increment(&RateKey {
            name: format!(
                "RL#login-user#{}#{bucket}",
                sha256_hex(login.to_lowercase().as_bytes())
            ),
            expires,
        })?;
        Ok(by_ip > self.limits.per_ip || by_login > self.limits.per_login)
    }

    // Login name first; an address-shaped input that names no login is tried as an email.
    fn lookup(&mut self, login: &str) -> Result<Option<User>, Error> {
        if let Some(user) = self.users.by_login(login.as_bytes())? {
            return Ok(Some(user));
        }
        if login.contains('@') {
            return Ok(self.users.by_email(login.as_bytes())?);
        }
        Ok(None)
    }
}

impl<S, T, R, E> fmt::Debug for Login<S, T, R, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Login")
            .field("path", &self.path)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// Whether the request's user (or the anonymous actor when `None`) is allowed `cap`.
///
/// # Errors
///
/// [`Error::Unavailable`] when the capability store is down.
pub fn can<C: Store>(
    user: Option<UserId>,
    caps: &mut Caps<C>,
    roles: &RoleTable,
    cap: &[u8],
) -> Result<bool, Error> {
    if let Some(id) = user {
        return Ok(caps.can(id, roles, cap)?);
    }
    // Logged out: the anonymous actor holds no grants, as in `cms_auth::Current`.
    let mapped = map_meta(cap, Actor::Anonymous);
    let required: Vec<&[u8]> = mapped.names().collect();
    Ok(GrantMap::empty().has(&required))
}

// The reference trims a login name and collapses inner whitespace runs to one space.
fn normalize_login(login: &str) -> String {
    login.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        Limits, Login, LoginRequest, Outcome, REMEMBER_SECONDS, Rejection, SESSION_SECONDS, can,
        normalize_login,
    };
    use crate::cookie::CookiePath;
    use crate::error::Error;
    use crate::password::Password;
    use crate::ports::SessionStore;
    use crate::ports::memory::{MemoryLimits, MemorySessions};
    use crate::token::{Entropy, SessionToken};
    use cms_auth::{Caps, RoleTable};
    use cms_db::{Cell, Column, Reply, ResultSet, Row, Scripted, Sql, TablePrefix};
    use cms_users::{UserId, Users};

    const NOW: u64 = 1_780_920_000;
    // Portable fixture for "test12345" (phpass 0.3 test vector); always needs a rehash.
    const PORTABLE: &str = "$P$9IQRaTwmfeRo7ud9Fh4E2PdI0S3r.L0";

    struct Counter(u8);

    impl Entropy for Counter {
        fn fill(&mut self, buf: &mut [u8]) -> Result<(), Error> {
            self.0 = self.0.wrapping_add(1);
            buf.fill(self.0);
            Ok(())
        }
    }

    struct Broken;

    impl Entropy for Broken {
        fn fill(&mut self, _: &mut [u8]) -> Result<(), Error> {
            Err(Error::Entropy)
        }
    }

    fn by(column: &str, value: &str) -> Sql {
        Sql::from(format!("SELECT * FROM wp_users WHERE {column} = '{value}' LIMIT 1").as_str())
    }

    fn row(id: &str, login: &str, pass: &str) -> Reply {
        Reply::Rows(ResultSet::new(
            [
                Column::new("ID"),
                Column::new("user_login"),
                Column::new("user_pass"),
            ],
            [Row::new([
                Cell::bytes(id),
                Cell::bytes(login),
                Cell::bytes(pass),
            ])],
        ))
    }

    fn empty() -> Reply {
        Reply::Rows(ResultSet::new([Column::new("ID")], Vec::<Row>::new()))
    }

    fn service<E: Entropy>(
        steps: Vec<(Sql, Reply)>,
        entropy: E,
    ) -> Login<Scripted, MemorySessions, MemoryLimits, E> {
        let prefix = TablePrefix::new("wp_").unwrap();
        let users = Users::new(Scripted::new(steps), &prefix).unwrap();
        let path = CookiePath::new("/blog/").unwrap();
        Login::new(
            users,
            MemorySessions::default(),
            MemoryLimits::default(),
            entropy,
            path,
            Limits::default(),
        )
    }

    fn request(login: &str, password: &str, remember: bool) -> LoginRequest {
        LoginRequest {
            login: login.to_owned(),
            password: Password::new(password),
            ip: "192.0.2.1".to_owned(),
            user_agent: "agent".to_owned(),
            remember,
        }
    }

    #[test]
    fn a_failed_rehash_write_is_an_outage_and_creates_no_session() {
        let mut login = service(
            vec![(by("user_login", "ada"), row("7", "ada", PORTABLE))],
            Counter(0),
        );
        // Name and password are trimmed, so this verifies; the script has no UPDATE step.
        let outcome = login.login(&request("  ada ", " test12345\n", false), NOW);
        assert_eq!(outcome, Err(Error::Unavailable));
        assert!(login.sessions.sessions.is_empty());
        let secret = request("ada", "s3cret-pw", true);
        assert!(!format!("{secret:?}").contains("s3cret-pw"));
    }

    #[test]
    fn a_current_hash_logs_in_without_rehash_and_round_trips_the_cookie() {
        let mut seed = service(Vec::new(), Counter(0));
        let hash = crate::password::hash_password(&Password::new("pw"), &mut seed.entropy).unwrap();
        let steps = vec![(by("user_login", "ada"), row("7", "ada", &hash))];
        let mut login = service(steps, Counter(10));
        let Ok(Outcome::LoggedIn(done)) = login.login(&request("ada", "pw", true), NOW) else {
            panic!("expected a session");
        };
        assert!(!done.rehashed());
        assert_eq!(done.user().id().as_u64(), 7);
        assert_eq!(done.session().expires, NOW + REMEMBER_SECONDS);
        assert_eq!(done.session().digest, done.token().digest());
        assert!(done.set_cookie().contains("Max-Age=1209600"));
        // No secret reaches Debug output.
        let shown = format!(
            "{done:?} {login:?} {outcome:?}",
            outcome = Outcome::LoggedIn(done.clone())
        );
        assert!(!shown.contains(done.token().expose()));
        let header = format!("x=1; fp_session={}", done.token().expose());
        // The row is cached from the login lookup, so resolve needs no extra statement...
        let resolved = login.resolve(&header, NOW + 1).unwrap().unwrap();
        assert_eq!(resolved.user.id().as_u64(), 7);
        assert!(!format!("{resolved:?}").contains(done.token().expose()));
        // ...and an expired session is deleted.
        assert!(
            login
                .resolve(&header, NOW + REMEMBER_SECONDS)
                .unwrap()
                .is_none()
        );
        assert!(login.sessions.sessions.is_empty());
    }

    #[test]
    fn rehash_writes_the_current_form_for_a_legacy_hash() {
        // Pin the entropy so the rehash statement is known in advance.
        let mut probe = Counter(0);
        let expected =
            crate::password::hash_password(&Password::new("test12345"), &mut probe).unwrap();
        let update = Sql::from(
            format!(
                "UPDATE `wp_users` SET `user_pass` = '{expected}', `user_activation_key` = '' WHERE `ID` = 7"
            )
            .as_str(),
        );
        let steps = vec![
            (by("user_login", "ada"), row("7", "ada", PORTABLE)),
            (
                update,
                Reply::Done {
                    affected: 1,
                    insert_id: 0,
                },
            ),
        ];
        let mut login = service(steps, Counter(0));
        let Ok(Outcome::LoggedIn(done)) = login.login(&request("ada", "test12345", false), NOW)
        else {
            panic!("expected a session");
        };
        assert!(done.rehashed());
        assert_eq!(done.session().expires, NOW + SESSION_SECONDS);
        assert!(!done.set_cookie().contains("Max-Age"));
        assert_eq!(login.sessions.sessions.len(), 1);
    }

    #[test]
    fn rejections_cover_empty_unknown_wrong_and_email_fallback() {
        let steps = vec![
            (by("user_login", "nobody"), empty()),
            (by("user_login", "a@example.test"), empty()),
            (by("user_email", "a@example.test"), row("8", "a", PORTABLE)),
            (by("user_login", "ada"), row("7", "ada", PORTABLE)),
        ];
        let mut login = service(steps, Counter(0));
        let mut attempt = |name: &str, pw: &str| login.login(&request(name, pw, false), NOW);
        assert_eq!(attempt("", "x"), Ok(Outcome::Rejected(Rejection::Empty)));
        assert_eq!(
            attempt("ada", "  "),
            Ok(Outcome::Rejected(Rejection::Empty))
        );
        assert_eq!(
            attempt("nobody", "x"),
            Ok(Outcome::Rejected(Rejection::UnknownUser))
        );
        assert_eq!(
            attempt("a@example.test", "wrong"),
            Ok(Outcome::Rejected(Rejection::BadPassword))
        );
        assert_eq!(
            attempt("ada", "wrong"),
            Ok(Outcome::Rejected(Rejection::BadPassword))
        );
        // The store script is exhausted: the next lookup is an outage.
        assert_eq!(attempt("ada", "wrong"), Err(Error::Unavailable));
    }

    #[test]
    fn limits_reject_before_any_lookup_and_outages_fail_closed() {
        let mut login = service(vec![(by("user_login", "ada"), empty())], Counter(0));
        login.limits = Limits {
            per_ip: 100,
            per_login: 1,
            window: 0,
        };
        let first = login.login(&request("ada", "x", false), NOW);
        assert_eq!(first, Ok(Outcome::Rejected(Rejection::UnknownUser)));
        // Same login name in other case: same counter, now over the limit, no store call.
        let second = login.login(&request("ADA", "x", false), NOW);
        assert_eq!(second, Ok(Outcome::Rejected(Rejection::Limited)));
        login.limits.per_login = 100;
        login.limits.per_ip = 2;
        let third = login.login(&request("bob", "x", false), NOW);
        assert_eq!(third, Ok(Outcome::Rejected(Rejection::Limited)));
        login.limiter.down = true;
        assert_eq!(
            login.login(&request("ada", "x", false), NOW),
            Err(Error::Unavailable)
        );
    }

    #[test]
    fn token_failure_and_session_outage_are_errors() {
        let mut seed = Counter(0);
        let hash = crate::password::hash_password(&Password::new("pw"), &mut seed).unwrap();
        let mut broken = service(
            vec![(by("user_login", "ada"), row("7", "ada", &hash))],
            Broken,
        );
        assert_eq!(
            broken.login(&request("ada", "pw", false), NOW),
            Err(Error::Entropy)
        );
        let mut down = service(
            vec![(by("user_login", "ada"), row("7", "ada", &hash))],
            Counter(0),
        );
        down.sessions.down = true;
        assert_eq!(
            down.login(&request("ada", "pw", false), NOW),
            Err(Error::Unavailable)
        );
    }

    #[test]
    fn resolve_and_logout_handle_missing_unknown_and_orphaned_sessions() {
        let get = Sql::from("SELECT * FROM wp_users WHERE ID = '9' LIMIT 1");
        let mut login = service(vec![(get, empty())], Counter(0));
        assert!(login.resolve("", NOW).unwrap().is_none());
        let token = SessionToken::parse(&"cd".repeat(32)).unwrap();
        let header = format!("fp_session={}", token.expose());
        assert!(login.resolve(&header, NOW).unwrap().is_none());
        let session = crate::ports::Session {
            digest: token.digest(),
            user: UserId::new(9).unwrap(),
            created: NOW,
            expires: NOW + 10,
            agent: String::new(),
        };
        login.sessions.put(&session).unwrap();
        // The user row is gone: the session is dropped.
        assert!(login.resolve(&header, NOW).unwrap().is_none());
        assert!(login.sessions.sessions.is_empty());
        login.sessions.put(&session).unwrap();
        assert_eq!(
            login.logout(&header).unwrap(),
            "fp_session=; Max-Age=0; Path=/blog/; HttpOnly; Secure; SameSite=Lax"
        );
        assert!(login.sessions.sessions.is_empty());
        assert!(login.logout("").is_ok());
        login.sessions.put(&session).unwrap();
        assert_eq!(login.logout_everywhere(session.user), Ok(1));
        login.sessions.down = true;
        assert_eq!(login.resolve(&header, NOW), Err(Error::Unavailable));
        assert_eq!(login.logout(&header), Err(Error::Unavailable));
    }

    #[test]
    fn capability_checks_use_stored_caps_or_the_anonymous_actor() {
        let caps_sql = Sql::from(
            "SELECT user_id, meta_key, meta_value FROM wp_usermeta WHERE user_id IN (7) ORDER BY umeta_id ASC",
        );
        let caps_row = Reply::Rows(ResultSet::new(
            [
                Column::new("user_id"),
                Column::new("meta_key"),
                Column::new("meta_value"),
            ],
            [Row::new([
                Cell::bytes("7"),
                Cell::bytes("wp_capabilities"),
                Cell::bytes("a:1:{s:4:\"read\";b:1;}"),
            ])],
        ));
        let prefix = TablePrefix::new("wp_").unwrap();
        let mut caps = Caps::new(Scripted::new([(caps_sql, caps_row)]), &prefix).unwrap();
        let roles = RoleTable::empty();
        let id = UserId::new(7).ok();
        assert_eq!(can(id, &mut caps, &roles, b"read"), Ok(true));
        assert_eq!(can(id, &mut caps, &roles, b"manage_options"), Ok(false));
        assert_eq!(can(None, &mut caps, &roles, b"read"), Ok(false));
        let mut down = Caps::new(Scripted::new([]), &prefix).unwrap();
        assert_eq!(can(id, &mut down, &roles, b"read"), Err(Error::Unavailable));
    }

    #[test]
    fn login_names_are_trimmed_and_collapsed() {
        assert_eq!(normalize_login("  a \t b  "), "a b");
        assert_eq!(normalize_login(""), "");
    }
}
