//! The read-model table: route items for the read path, version-gated writes for the projector.
//!
//! Item layout (`docs/aws/architecture.md` section 4): `pk` and `sk` strings; route items are
//! `ROUTE#<path>` / `R` with `kind` (`page`, `object`, `redirect`, `gone`), `status` (N),
//! `content_type`, `etag`, `last_modified`, `shell` and `body` (pages), `body` (objects),
//! `location` (redirects) and `version`.

use cms_readmodel::{BlobKey, Content, PortError, Route, RouteKey, RouteTable};
use serde_json::{Value, json};

use crate::{AwsError, DynamoDb, Item, Page, Write};

const ROUTE_SK: &str = "R";

/// `ROUTE#<path>`: the partition key of a route item.
#[must_use]
pub fn route_pk(key: &RouteKey) -> String {
    format!("ROUTE#{key}")
}

/// `TERM#<taxonomy>#<slug>`: the partition key of a term listing.
#[must_use]
pub fn term_pk(taxonomy: &str, slug: &str) -> String {
    format!("TERM#{taxonomy}#{slug}")
}

/// The `DynamoDB` item for `route` under `key` (without `version`, which the write adds).
#[must_use]
pub fn encode_route(key: &RouteKey, route: &Route) -> Item {
    let mut item = primary_key(&route_pk(key), ROUTE_SK);
    let mut put = |name: &str, value: &str| {
        item.insert(name.to_owned(), json!({ "S": value }));
    };
    put("content_type", &route.content_type);
    put("etag", &route.etag);
    put("last_modified", &route.last_modified);
    match &route.content {
        Content::Page { shell, body } => {
            put("kind", "page");
            put("shell", shell.as_str());
            put("body", body.as_str());
        }
        Content::Object { body } => {
            put("kind", "object");
            put("body", body.as_str());
        }
        Content::Redirect { location } => {
            put("kind", "redirect");
            put("location", location);
        }
        Content::Gone => put("kind", "gone"),
    }
    item.insert(
        "status".to_owned(),
        json!({ "N": route.status.to_string() }),
    );
    item
}

/// Reads a route item written by [`encode_route`].
///
/// # Errors
///
/// [`PortError::Store`] naming the missing or mistyped attribute.
pub fn decode_route(item: &Item) -> Result<Route, PortError> {
    let text = |name: &str| -> Result<Box<str>, PortError> {
        item.get(name)
            .and_then(|value| value.get("S"))
            .and_then(Value::as_str)
            .map(Into::into)
            .ok_or_else(|| PortError::Store(format!("route item has no string {name}").into()))
    };
    let blob = |name: &str| text(name).map(|key| BlobKey::from(&*key));
    let content = match &*text("kind")? {
        "page" => Content::Page {
            shell: blob("shell")?,
            body: blob("body")?,
        },
        "object" => Content::Object {
            body: blob("body")?,
        },
        "redirect" => Content::Redirect {
            location: text("location")?,
        },
        "gone" => Content::Gone,
        other => {
            return Err(PortError::Store(
                format!("unknown route kind {other}").into(),
            ));
        }
    };
    let status = item
        .get("status")
        .and_then(|value| value.get("N"))
        .and_then(Value::as_str)
        .and_then(|number| number.parse().ok())
        .ok_or_else(|| PortError::Store("route item has no numeric status".into()))?;
    Ok(Route {
        status,
        content_type: text("content_type")?,
        etag: text("etag")?,
        last_modified: text("last_modified")?,
        content,
    })
}

fn primary_key(pk: &str, sk: &str) -> Item {
    let mut key = Item::new();
    key.insert("pk".to_owned(), json!({ "S": pk }));
    key.insert("sk".to_owned(), json!({ "S": sk }));
    key
}

/// The read-model table: [`RouteTable`] for the read path and the projector's writer API.
#[derive(Clone, Debug)]
pub struct ReadTable {
    db: DynamoDb,
    table: Box<str>,
}

impl ReadTable {
    /// The table named `table` reached through `db`.
    #[must_use]
    pub fn new(db: DynamoDb, table: &str) -> Self {
        Self {
            db,
            table: table.into(),
        }
    }

    /// Writes the route item for `key` unless a newer `version` is stored.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn put_route(
        &self,
        key: &RouteKey,
        route: &Route,
        version: &str,
    ) -> Result<Write, AwsError> {
        self.db
            .put_item_versioned(&self.table, encode_route(key, route), version)
    }

    /// Writes any item (post doc `POST#<id>`/`D`, term listing row `TERM#…`/`<date>#<id>`)
    /// with `pk`, `sk` and `attributes` unless a newer `version` is stored.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn put_versioned(
        &self,
        pk: &str,
        sk: &str,
        attributes: Item,
        version: &str,
    ) -> Result<Write, AwsError> {
        let mut item = primary_key(pk, sk);
        item.extend(attributes);
        self.db.put_item_versioned(&self.table, item, version)
    }

    /// Deletes the item at `pk` / `sk`.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn delete(&self, pk: &str, sk: &str) -> Result<(), AwsError> {
        self.db.delete_item(&self.table, &primary_key(pk, sk))
    }

    /// One page of a term listing, newest first.
    ///
    /// # Errors
    ///
    /// [`AwsError`] when the call fails.
    pub fn term_listing(
        &self,
        taxonomy: &str,
        slug: &str,
        limit: u32,
        start: Option<Item>,
    ) -> Result<Page, AwsError> {
        self.db
            .query_descending(&self.table, &term_pk(taxonomy, slug), limit, start)
    }
}

impl RouteTable for ReadTable {
    fn route(&mut self, key: &RouteKey) -> Result<Option<Route>, PortError> {
        let item = self
            .db
            .get_item(&self.table, &primary_key(&route_pk(key), ROUTE_SK))
            .map_err(|error| PortError::Store(error.to_string().into()))?;
        item.as_ref().map(decode_route).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_route, encode_route, route_pk, term_pk};
    use cms_readmodel::{Content, PortError, Route, RouteKey};
    use serde_json::json;

    fn route(content: Content) -> Route {
        Route {
            status: 200,
            content_type: "text/html; charset=UTF-8".into(),
            etag: "\"e1\"".into(),
            last_modified: "Fri, 27 Nov 2026 00:00:00 GMT".into(),
            content,
        }
    }

    #[test]
    fn every_route_kind_round_trips() {
        let key = RouteKey::from("/blog/hello/");
        for content in [
            Content::Page {
                shell: "fragments/shell/s.html".into(),
                body: "rendered/b.html".into(),
            },
            Content::Object {
                body: "rendered/f.xml".into(),
            },
            Content::Redirect {
                location: "/blog/new/".into(),
            },
            Content::Gone,
        ] {
            let original = route(content);
            let item = encode_route(&key, &original);
            assert_eq!(item["pk"], json!({ "S": "ROUTE#/blog/hello/" }));
            assert_eq!(item["sk"], json!({ "S": "R" }));
            assert_eq!(decode_route(&item), Ok(original));
        }
        assert_eq!(route_pk(&key), "ROUTE#/blog/hello/");
        assert_eq!(term_pk("category", "news"), "TERM#category#news");
    }

    #[test]
    fn malformed_items_name_the_attribute() {
        let mut item = encode_route(&RouteKey::from("/"), &route(Content::Gone));
        item.insert("kind".to_owned(), json!({ "S": "video" }));
        assert_eq!(
            decode_route(&item),
            Err(PortError::Store("unknown route kind video".into()))
        );
        item.insert("kind".to_owned(), json!({ "N": "1" }));
        assert_eq!(
            decode_route(&item),
            Err(PortError::Store("route item has no string kind".into()))
        );
        let mut item = encode_route(&RouteKey::from("/"), &route(Content::Gone));
        item.insert("status".to_owned(), json!({ "N": "x" }));
        assert_eq!(
            decode_route(&item),
            Err(PortError::Store("route item has no numeric status".into()))
        );
    }
}
