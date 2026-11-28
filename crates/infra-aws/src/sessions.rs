//! The sessions and rate-limits tables as the `cms-session` ports (architecture section 7).

use cms_session::{Error, RateKey, RateLimiter, Session, SessionStore, TokenDigest};
use cms_users::UserId;
use serde_json::{Value, json};

use crate::{DynamoDb, Item};

/// The `user_id` global secondary index that serves "log out everywhere".
pub const USER_INDEX: &str = "user_id";

/// The sessions table: `pk` = sha256 hex of the token, `expires` is the TTL attribute.
#[derive(Clone, Debug)]
pub struct SessionsTable {
    db: DynamoDb,
    table: String,
}

impl SessionsTable {
    /// The adapter over `table`.
    #[must_use]
    pub fn new(db: DynamoDb, table: &str) -> Self {
        Self {
            db,
            table: table.to_owned(),
        }
    }
}

// Every AWS failure is an outage to the login service; it fails closed.
fn down<E>(_cause: E) -> Error {
    Error::Unavailable
}

fn key(digest: &str) -> Item {
    item(json!({ "pk": { "S": digest } }))
}

fn item(value: Value) -> Item {
    match value {
        Value::Object(map) => map,
        _ => Item::new(),
    }
}

// `{"S": "…"}` or `{"N": "…"}` text of attribute `name`.
fn text<'a>(item: &'a Item, name: &str, kind: &str) -> Option<&'a str> {
    item.get(name)?.get(kind)?.as_str()
}

fn number(item: &Item, name: &str) -> Option<u64> {
    text(item, name, "N")?.parse().ok()
}

// A stored item that does not decode is a defect: treated as an outage, never as "no session".
fn decode(item: &Item) -> Option<Session> {
    Some(Session {
        digest: TokenDigest::parse(text(item, "pk", "S")?)?,
        user: UserId::new(number(item, "user_id")?).ok()?,
        created: number(item, "created")?,
        expires: number(item, "expires")?,
        agent: text(item, "agent", "S")?.to_owned(),
    })
}

impl SessionStore for SessionsTable {
    fn put(&mut self, session: &Session) -> Result<(), Error> {
        let stored = item(json!({
            "pk": { "S": session.digest.as_str() },
            "user_id": { "N": session.user.as_u64().to_string() },
            "created": { "N": session.created.to_string() },
            "expires": { "N": session.expires.to_string() },
            "agent": { "S": session.agent },
        }));
        self.db.put_item(&self.table, stored).map_err(down)
    }

    fn get(&mut self, digest: &TokenDigest) -> Result<Option<Session>, Error> {
        match self.db.get_item(&self.table, &key(digest.as_str())).map_err(down)? {
            None => Ok(None),
            Some(found) => decode(&found).map(Some).ok_or(Error::Unavailable),
        }
    }

    fn delete(&mut self, digest: &TokenDigest) -> Result<(), Error> {
        self.db
            .delete_item(&self.table, &key(digest.as_str()))
            .map_err(down)
    }

    fn delete_all_for_user(&mut self, user: UserId) -> Result<u64, Error> {
        // The index projects keys only; each page's sessions are deleted by `pk`.
        let value = json!({ "N": user.as_u64().to_string() });
        let mut removed = 0;
        let mut start = None;
        loop {
            let page = self
                .db
                .query_index(&self.table, USER_INDEX, "user_id", &value, start)
                .map_err(down)?;
            for found in &page.items {
                let digest = text(found, "pk", "S").ok_or(Error::Unavailable)?;
                self.db.delete_item(&self.table, &key(digest)).map_err(down)?;
                removed += 1;
            }
            match page.next {
                Some(next) => start = Some(next),
                None => return Ok(removed),
            }
        }
    }
}

/// The rate-limits table: `pk` = counter name, `count` an atomic counter, `expires` the TTL.
#[derive(Clone, Debug)]
pub struct LimitsTable {
    db: DynamoDb,
    table: String,
}

impl LimitsTable {
    /// The adapter over `table`.
    #[must_use]
    pub fn new(db: DynamoDb, table: &str) -> Self {
        Self {
            db,
            table: table.to_owned(),
        }
    }
}

impl RateLimiter for LimitsTable {
    fn increment(&mut self, rate: &RateKey) -> Result<u64, Error> {
        // ADD creates the counter at one; the TTL is set only by the first increment.
        let updated = self
            .db
            .update_item(
                &self.table,
                &key(&rate.name),
                "ADD #c :one SET #e = if_not_exists(#e, :exp)",
                &json!({ "#c": "count", "#e": "expires" }),
                &json!({ ":one": { "N": "1" }, ":exp": { "N": rate.expires.to_string() } }),
            )
            .map_err(down)?;
        number(&updated, "count").ok_or(Error::Unavailable)
    }
}
