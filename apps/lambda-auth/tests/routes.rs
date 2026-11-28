//! The auth routes end to end over a scripted user store and in-memory session/limit fakes.

use cms_db::{Cell, Column, Reply, ResultSet, Row, Scripted, Sql, TablePrefix};
use cms_session::memory::{MemoryLimits, MemorySessions};
use cms_session::{
    CookiePath, Entropy, Error, Limits, Login, NonceKey, Password, hash_password,
};
use cms_users::Users;
use lambda_auth::{Auth, AuthReply, AuthRequest};

const NOW: u64 = 1_780_920_000;

// Deterministic randomness: distinct, repeatable tokens and salts.
struct Counter(u8);

impl Entropy for Counter {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        self.0 = self.0.wrapping_add(1);
        buf.fill(self.0);
        Ok(())
    }
}

type Handler = Auth<Scripted, MemorySessions, MemoryLimits, Counter>;

fn user_row(hash: &str) -> Reply {
    Reply::Rows(ResultSet::new(
        [Column::new("ID"), Column::new("user_login"), Column::new("user_pass")],
        [Row::new([Cell::bytes("7"), Cell::bytes("ada"), Cell::bytes(hash)])],
    ))
}

fn by_login(name: &str) -> Sql {
    Sql::from(format!("SELECT * FROM wp_users WHERE user_login = '{name}' LIMIT 1").as_str())
}

// One scripted lookup per expected login attempt, in order.
fn auth(lookups: usize, limits: Limits) -> Handler {
    let hash = hash_password(&Password::new("pw"), &mut Counter(100)).unwrap();
    let steps: Vec<(Sql, Reply)> = (0..lookups).map(|_| (by_login("ada"), user_row(&hash))).collect();
    let prefix = TablePrefix::new("wp_").unwrap();
    let base = CookiePath::new("/blog/").unwrap();
    let login = Login::new(
        Users::new(Scripted::new(steps), &prefix).unwrap(),
        MemorySessions::default(),
        MemoryLimits::default(),
        Counter(0),
        base.clone(),
        limits,
    );
    Auth::new(login, NonceKey::new(b"key", b"nonce"), base)
}

fn request(method: &str, path: &str, cookie: &str, body: &str) -> AuthRequest {
    AuthRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        cookie: cookie.to_owned(),
        body: body.as_bytes().to_vec(),
        ip: "198.51.100.7".to_owned(),
        user_agent: "UA".to_owned(),
    }
}

fn header<'a>(reply: &'a AuthReply, name: &str) -> &'a str {
    reply.headers.iter().find(|(key, _)| *key == name).map_or("", |(_, value)| value.as_str())
}

// `fp_session=<token>` from a Set-Cookie value.
fn session_cookie(reply: &AuthReply) -> String {
    reply.cookies[0].split(';').next().unwrap_or_default().to_owned()
}

#[test]
fn the_form_renders_uncached_and_unknown_routes_are_refused() {
    let mut auth = auth(0, Limits::default());
    let form = auth.handle(&request("GET", "/blog/login", "", ""), NOW).unwrap();
    assert_eq!(form.status, 200);
    assert_eq!(header(&form, "cache-control"), "no-store");
    assert!(form.body.contains("action=\"/blog/login\"") && form.body.contains("name=\"pwd\""));
    let missing = auth.handle(&request("GET", "/blog/elsewhere", "", ""), NOW).unwrap();
    assert_eq!(missing.status, 404);
    let put = auth.handle(&request("PUT", "/blog/login", "", ""), NOW).unwrap();
    assert_eq!((put.status, header(&put, "allow")), (405, "GET, POST"));
    let get = auth.handle(&request("GET", "/blog/login/logout", "", ""), NOW).unwrap();
    assert_eq!((get.status, header(&get, "allow")), (405, "POST"));
}

#[test]
fn login_sets_the_cookie_and_follows_only_same_site_redirects() {
    let mut auth = auth(3, Limits::default());
    let wrong = auth
        .handle(&request("POST", "/blog/login", "", "log=ada&pwd=nope&redirect_to=%2Fblog%2F%22x"), NOW)
        .unwrap();
    assert_eq!(wrong.status, 200);
    assert!(wrong.cookies.is_empty());
    assert!(wrong.body.contains("The username or password is incorrect."));
    assert!(wrong.body.contains("value=\"ada\"") && wrong.body.contains("/blog/&quot;x"));
    let remembered = auth
        .handle(
            &request("POST", "/blog/login", "", "log=ada&pwd=pw&rememberme=forever&redirect_to=%2Fblog%2Fadmin%2Fpost%2F"),
            NOW,
        )
        .unwrap();
    assert_eq!(remembered.status, 302);
    assert_eq!(header(&remembered, "location"), "/blog/admin/post/");
    assert!(remembered.cookies[0].contains("Max-Age=1209600; Path=/blog/; HttpOnly; Secure; SameSite=Lax"));
    let offsite = auth
        .handle(&request("POST", "/blog/login", "", "log=ada&pwd=pw&redirect_to=%2Fblog%2F%2Fevil.test"), NOW)
        .unwrap();
    assert_eq!(header(&offsite, "location"), "/blog/admin/");
    assert!(!offsite.cookies[0].contains("Max-Age"));
    // Debug of the reply never shows the cookie value.
    assert!(!format!("{offsite:?}").contains(&session_cookie(&offsite)));
}

#[test]
fn logout_needs_the_sessions_own_nonce() {
    let mut auth = auth(1, Limits::default());
    let done = auth.handle(&request("POST", "/blog/login", "", "log=ada&pwd=pw"), NOW).unwrap();
    let cookie = session_cookie(&done);
    let nonce = auth.logout_nonce(&cookie, NOW).unwrap().unwrap();
    let forged = auth
        .handle(&request("POST", "/blog/login/logout", &cookie, "_wpnonce=0123456789"), NOW)
        .unwrap();
    assert_eq!(forged.status, 403);
    assert!(forged.cookies.is_empty());
    let out = auth
        .handle(&request("POST", "/blog/login/logout", &cookie, &format!("_wpnonce={nonce}")), NOW + 60)
        .unwrap();
    assert_eq!((out.status, header(&out, "location")), (302, "/blog/login"));
    assert_eq!(out.cookies, ["fp_session=; Max-Age=0; Path=/blog/; HttpOnly; Secure; SameSite=Lax"]);
    assert_eq!(auth.logout_nonce(&cookie, NOW + 61), Ok(None));
    // Without a live session logout just clears the cookie.
    let anonymous = auth.handle(&request("POST", "/blog/login/logout", "", ""), NOW).unwrap();
    assert_eq!(anonymous.status, 302);
}

#[test]
fn floods_are_limited_and_outages_are_errors() {
    let limits = Limits { per_ip: 1, per_login: 10, window: 300 };
    let mut auth = auth(1, limits);
    let first = auth.handle(&request("POST", "/blog/login", "", "log=ada&pwd=x"), NOW).unwrap();
    assert_eq!(first.status, 200);
    let second = auth.handle(&request("POST", "/blog/login", "", "log=ada&pwd=x"), NOW).unwrap();
    assert_eq!(second.status, 429);
    let empty = auth.handle(&request("POST", "/blog/login", "", "log=&pwd="), NOW + 600).unwrap();
    assert!(empty.body.contains("Enter a username and a password."));
    // The scripted store is exhausted: the next lookup is an outage, reported as an error.
    assert_eq!(
        auth.handle(&request("POST", "/blog/login", "", "log=ada&pwd=x"), NOW + 1200),
        Err(Error::Unavailable)
    );
}
