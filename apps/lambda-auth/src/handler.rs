//! The auth routes under the base path: the login form, login and logout.

use cms_db::Store;
use cms_session::{
    CookiePath, Entropy, Error, Login, LoginRequest, NonceKey, Outcome, Password, RateLimiter,
    Rejection, SessionStore, create_nonce, verify_nonce,
};

use crate::event::{AuthReply, AuthRequest};
use crate::form::{escape, field};

/// Nonce action of the logout form, as in the reference.
pub const LOGOUT_ACTION: &str = "log-out";

/// The auth Lambda's request handler over the login service and the site's nonce key.
pub struct Auth<S, T, R, E> {
    login: Login<S, T, R, E>,
    nonce: NonceKey,
    base: CookiePath,
}

impl<S, T, R, E> std::fmt::Debug for Auth<S, T, R, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth")
            .field("login", &self.login)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl<S: Store, T: SessionStore, R: RateLimiter, E: Entropy> Auth<S, T, R, E> {
    /// A handler for routes under `base` (which ends in `/`, like the cookie path).
    pub fn new(login: Login<S, T, R, E>, nonce: NonceKey, base: CookiePath) -> Self {
        Self { login, nonce, base }
    }

    /// Serve one request at unix time `now`.
    ///
    /// Routes: `GET <base>login` (form), `POST <base>login`, `POST <base>login/logout`
    /// (needs the `log-out` nonce in `_wpnonce`). Anything else is 404 or 405.
    ///
    /// # Errors
    ///
    /// [`Error`] when a store, the limiter or the random source fails; the runtime reports it
    /// and the Function URL answers 502.
    pub fn handle(&mut self, request: &AuthRequest, now: u64) -> Result<AuthReply, Error> {
        let route = request.path.strip_prefix(self.base.as_str());
        match (route, request.method.as_str()) {
            (Some("login"), "GET") => Ok(self.form(200, "", None, "")),
            (Some("login"), "POST") => self.post_login(request, now),
            (Some("login/logout"), "POST") => self.logout(request, now),
            (Some("login"), _) => Ok(not_allowed("GET, POST")),
            (Some("login/logout"), _) => Ok(not_allowed("POST")),
            _ => Ok(page(404, "<p>Not found.</p>")),
        }
    }

    fn post_login(&mut self, request: &AuthRequest, now: u64) -> Result<AuthReply, Error> {
        let text = |name| {
            String::from_utf8_lossy(&field(&request.body, name).unwrap_or_default()).into_owned()
        };
        let login = text("log");
        let redirect = text("redirect_to");
        let attempt = LoginRequest {
            login: login.clone(),
            password: Password::new(field(&request.body, "pwd").unwrap_or_default()),
            ip: request.ip.clone(),
            user_agent: request.user_agent.clone(),
            remember: field(&request.body, "rememberme").is_some_and(|v| v == b"forever"),
        };
        match self.login.login(&attempt, now)? {
            Outcome::LoggedIn(done) => Ok(AuthReply {
                status: 302,
                headers: vec![
                    ("location", self.safe_redirect(&redirect)),
                    ("cache-control", "no-store".to_owned()),
                ],
                cookies: vec![done.set_cookie().to_owned()],
                body: String::new(),
            }),
            // One message for unknown users and wrong passwords, so names cannot be probed.
            Outcome::Rejected(Rejection::Limited) => Ok(self.form(
                429,
                "Too many login attempts. Try again later.",
                None,
                &redirect,
            )),
            Outcome::Rejected(Rejection::Empty) => Ok(self.form(
                200,
                "Enter a username and a password.",
                Some(&login),
                &redirect,
            )),
            Outcome::Rejected(Rejection::UnknownUser | Rejection::BadPassword) => Ok(self.form(
                200,
                "The username or password is incorrect.",
                Some(&login),
                &redirect,
            )),
        }
    }

    fn logout(&mut self, request: &AuthRequest, now: u64) -> Result<AuthReply, Error> {
        // A live session is only ended with its own nonce, so a cross-site POST cannot log out.
        if let Some(current) = self.login.resolve(&request.cookie, now)? {
            let nonce =
                String::from_utf8_lossy(&field(&request.body, "_wpnonce").unwrap_or_default())
                    .into_owned();
            let uid = current.user.id().as_u64();
            if verify_nonce(
                &self.nonce,
                &nonce,
                LOGOUT_ACTION,
                uid,
                Some(&current.token),
                now,
            )
            .is_none()
            {
                return Ok(page(
                    403,
                    "<p>The link you followed has expired.</p>",
                ));
            }
        }
        let clear = self.login.logout(&request.cookie)?;
        Ok(AuthReply {
            status: 302,
            headers: vec![
                ("location", format!("{}login", self.base.as_str())),
                ("cache-control", "no-store".to_owned()),
            ],
            cookies: vec![clear],
            body: String::new(),
        })
    }

    /// The logout nonce for the session behind `cookie`, for pages that render a logout form.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when a store is down.
    pub fn logout_nonce(&mut self, cookie: &str, now: u64) -> Result<Option<String>, Error> {
        Ok(self.login.resolve(cookie, now)?.map(|current| {
            create_nonce(
                &self.nonce,
                LOGOUT_ACTION,
                current.user.id().as_u64(),
                Some(&current.token),
                now,
            )
        }))
    }

    // Only a same-site path under the base is followed; anything else goes to the admin.
    fn safe_redirect(&self, target: &str) -> String {
        let base = self.base.as_str();
        let safe = target.starts_with(base)
            && !target.contains("//")
            && !target.contains('\\')
            && !target.bytes().any(|b| b.is_ascii_control() || b == b' ');
        if safe {
            target.to_owned()
        } else {
            format!("{base}admin/")
        }
    }

    fn form(&self, status: u16, message: &str, login: Option<&str>, redirect: &str) -> AuthReply {
        let action = format!("{}login", self.base.as_str());
        let notice = if message.is_empty() {
            String::new()
        } else {
            format!("<p role=\"alert\">{}</p>", escape(message))
        };
        page(
            status,
            &format!(
                "<h1>Log in</h1>{notice}<form method=\"post\" action=\"{action}\">\
                 <p><label for=\"user_login\">Username or Email Address</label>\
                 <input type=\"text\" name=\"log\" id=\"user_login\" value=\"{login}\" autocomplete=\"username\" required></p>\
                 <p><label for=\"user_pass\">Password</label>\
                 <input type=\"password\" name=\"pwd\" id=\"user_pass\" autocomplete=\"current-password\" required></p>\
                 <p><label><input type=\"checkbox\" name=\"rememberme\" value=\"forever\"> Remember Me</label></p>\
                 <input type=\"hidden\" name=\"redirect_to\" value=\"{redirect}\">\
                 <p><button type=\"submit\">Log In</button></p></form>",
                action = escape(&action),
                login = escape(login.unwrap_or_default()),
                redirect = escape(redirect),
            ),
        )
    }
}

fn not_allowed(allow: &str) -> AuthReply {
    let mut reply = page(405, "<p>Method not allowed.</p>");
    reply.headers.push(("allow", allow.to_owned()));
    reply
}

// Every auth page is personal: never cached, never framed.
fn page(status: u16, main: &str) -> AuthReply {
    AuthReply {
        status,
        headers: vec![
            ("content-type", "text/html; charset=utf-8".to_owned()),
            ("cache-control", "no-store".to_owned()),
            ("x-frame-options", "DENY".to_owned()),
        ],
        cookies: Vec::new(),
        body: format!(
            "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"robots\" content=\"noindex\"><title>Log In</title></head>\
             <body><main>{main}</main></body></html>"
        ),
    }
}
