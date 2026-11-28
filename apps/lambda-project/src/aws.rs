//! The projector's ports over the `infra-aws` clients: S3 objects, the read-model table and
//! `CloudFront` invalidations, plus the system clock and the in-process nudger.

use std::time::{SystemTime, UNIX_EPOCH};

use cms_projector::{
    Applied, BlobWriter, Clock, Invalidator, Item, ItemKey, Kind, Nudger, PortError, PostDoc,
    PostFacts, RouteWriter, TermRef,
};
use cms_readmodel::BlobKey;
use infra_aws::{AwsError, CloudFront, DynamoDb, ReadTable, S3, Write, route_pk, term_pk};
use serde_json::{Map, Value, json};

/// Rendered objects are content-addressed and never change, so edges and browsers keep them.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

const DOC_SK: &str = "D";

fn port(error: &AwsError) -> PortError {
    PortError(error.to_string().into())
}

/// [`BlobWriter`] over S3 `PutObject`.
#[derive(Debug)]
pub struct Objects(pub S3);

impl BlobWriter for Objects {
    fn put(&mut self, key: &BlobKey, bytes: &[u8], content_type: &str) -> Result<(), PortError> {
        self.0
            .put_object(key.as_str(), bytes, content_type, IMMUTABLE)
            .map_err(|error| port(&error))
    }
}

/// [`RouteWriter`] over the read-model table.
#[derive(Debug)]
pub struct Table {
    read: ReadTable,
    db: DynamoDb,
    name: Box<str>,
}

impl Table {
    /// The table `name` reached through `db`.
    #[must_use]
    pub fn new(db: DynamoDb, name: &str) -> Self {
        Self {
            read: ReadTable::new(db.clone(), name),
            db,
            name: name.into(),
        }
    }
}

const fn applied(write: Write) -> Applied {
    match write {
        Write::Applied => Applied::Written,
        Write::Stale => Applied::Stale,
    }
}

impl RouteWriter for Table {
    fn post_doc(&mut self, id: u64) -> Result<Option<PostDoc>, PortError> {
        let mut key = Map::new();
        key.insert("pk".to_owned(), json!({ "S": doc_pk(id) }));
        key.insert("sk".to_owned(), json!({ "S": DOC_SK }));
        let item = self.db.get_item(&self.name, &key).map_err(|error| port(&error))?;
        item.as_ref().map(decode_doc).transpose()
    }

    fn put(&mut self, item: &Item, version: &str) -> Result<Applied, PortError> {
        let written = match item {
            Item::Route(key, route) => self.read.put_route(key, route, version),
            Item::Doc(doc) => self
                .read
                .put_versioned(&doc_pk(doc.facts.id), DOC_SK, encode_doc(doc), version),
            Item::Listing(listing) => {
                let mut attributes = Map::new();
                attributes.insert("post_id".to_owned(), json!({ "N": listing.post_id.to_string() }));
                self.read.put_versioned(
                    &term_pk(&listing.term.taxonomy, &listing.term.slug),
                    &listing.sort,
                    attributes,
                    version,
                )
            }
        };
        written.map(applied).map_err(|error| port(&error))
    }

    // `ReadTable::delete` is unconditional. The projector runs at concurrency 1 and only
    // deletes what the current state no longer has, so no newer write can be lost.
    fn delete(&mut self, key: &ItemKey, _version: &str) -> Result<Applied, PortError> {
        let (pk, sk) = match key {
            ItemKey::Route(route) => (route_pk(route), "R".to_owned()),
            ItemKey::Doc(id) => (doc_pk(*id), DOC_SK.to_owned()),
            ItemKey::Listing(listing) => (
                term_pk(&listing.term.taxonomy, &listing.term.slug),
                listing.sort.to_string(),
            ),
        };
        self.read
            .delete(&pk, &sk)
            .map(|()| Applied::Written)
            .map_err(|error| port(&error))
    }
}

fn doc_pk(id: u64) -> String {
    format!("POST#{id}")
}

/// The post doc's attributes (without `pk`, `sk` and `version`).
#[must_use]
pub fn encode_doc(doc: &PostDoc) -> Map<String, Value> {
    let facts = &doc.facts;
    let (year, month, day) = facts.date;
    let terms: Vec<Value> = facts
        .terms
        .iter()
        .map(|term| json!({ "S": format!("{}#{}", term.taxonomy, term.slug) }))
        .collect();
    let mut item = Map::new();
    let mut text = |name: &str, value: &str| {
        item.insert(name.to_owned(), json!({ "S": value }));
    };
    text("kind", if facts.kind == Kind::Page { "page" } else { "post" });
    text("permalink", &facts.permalink);
    text("author", &facts.author);
    text("date", &format!("{year:04}-{month:02}-{day:02}"));
    text("date_gmt", &facts.date_gmt);
    text("modified_gmt", &facts.modified_gmt);
    text("title", &doc.title);
    text("excerpt", &doc.excerpt);
    text("author_name", &doc.author_name);
    text("body", doc.body.as_str());
    item.insert("id".to_owned(), json!({ "N": facts.id.to_string() }));
    item.insert("terms".to_owned(), json!({ "L": terms }));
    item
}

/// Reads a post doc written by [`encode_doc`].
///
/// # Errors
///
/// [`PortError`] naming the missing or malformed attribute.
pub fn decode_doc(item: &Map<String, Value>) -> Result<PostDoc, PortError> {
    let bad = |name: &str| PortError(format!("post doc has no valid {name}").into());
    let typed = |name: &str, kind: &str| -> Result<Box<str>, PortError> {
        item.get(name)
            .and_then(|value| value.get(kind))
            .and_then(Value::as_str)
            .map(Into::into)
            .ok_or_else(|| bad(name))
    };
    let text = |name: &str| typed(name, "S");
    let date = text("date")?;
    let mut parts = date.split('-').map(str::parse::<u16>);
    let (Some(Ok(year)), Some(Ok(month)), Some(Ok(day)), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad("date"));
    };
    let terms = item
        .get("terms")
        .and_then(|value| value.get("L"))
        .and_then(Value::as_array)
        .ok_or_else(|| bad("terms"))?
        .iter()
        .map(|term| {
            term.get("S")
                .and_then(Value::as_str)
                .and_then(|text| text.split_once('#'))
                .map(|(taxonomy, slug)| TermRef {
                    taxonomy: taxonomy.into(),
                    slug: slug.into(),
                })
                .ok_or_else(|| bad("terms"))
        })
        .collect::<Result<_, _>>()?;
    let kind = match &*text("kind")? {
        "page" => Kind::Page,
        "post" => Kind::Post,
        _ => return Err(bad("kind")),
    };
    Ok(PostDoc {
        facts: PostFacts {
            id: typed("id", "N")?.parse().map_err(|_| bad("id"))?,
            kind,
            permalink: text("permalink")?,
            terms,
            author: text("author")?,
            date: (
                year,
                u8::try_from(month).map_err(|_| bad("date"))?,
                u8::try_from(day).map_err(|_| bad("date"))?,
            ),
            date_gmt: text("date_gmt")?,
            modified_gmt: text("modified_gmt")?,
        },
        title: text("title")?,
        excerpt: text("excerpt")?,
        author_name: text("author_name")?,
        body: BlobKey::from(&*text("body")?),
    })
}

/// [`Invalidator`] over `CloudFront` `CreateInvalidation`.
#[derive(Debug)]
pub struct Edge {
    client: CloudFront,
    distribution: Box<str>,
}

impl Edge {
    /// Invalidations on `distribution` through `client`.
    #[must_use]
    pub fn new(client: CloudFront, distribution: &str) -> Self {
        Self {
            client,
            distribution: distribution.into(),
        }
    }
}

impl Invalidator for Edge {
    fn invalidate(&mut self, paths: &[Box<str>]) -> Result<(), PortError> {
        let paths: Vec<&str> = paths.iter().map(|path| &**path).collect();
        // A fresh reference per call: a reused one would make CloudFront return an earlier
        // invalidation for the same paths instead of clearing the new objects.
        let reference = format!("fp-{}", SystemClock.now_micros());
        self.client
            .create_invalidation(&self.distribution, &paths, &reference)
            .map(drop)
            .map_err(|error| port(&error))
    }
}

/// The system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_micros(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| i64::try_from(since.as_micros()).unwrap_or(i64::MAX))
    }
}

/// Acknowledges a nudge: the runtime loop sees `Report::nudged` and runs again in the same
/// invocation. `infra-aws` has no Lambda client yet, so there is no async self-invoke.
#[derive(Clone, Copy, Debug, Default)]
pub struct InProcess;

impl Nudger for InProcess {
    fn nudge(&mut self) -> Result<(), PortError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use cms_projector::{Kind, PostDoc, PostFacts, TermRef};
    use cms_readmodel::BlobKey;
    use serde_json::json;

    use super::{decode_doc, encode_doc};

    #[test]
    fn post_docs_round_trip_and_bad_ones_are_named() {
        let doc = PostDoc {
            facts: PostFacts {
                id: 7,
                kind: Kind::Post,
                permalink: "/blog/hello/".into(),
                terms: vec![TermRef {
                    taxonomy: "category".into(),
                    slug: "news".into(),
                }],
                author: "ann".into(),
                date: (2026, 3, 7),
                date_gmt: "2026-03-07T09:00:00+00:00".into(),
                modified_gmt: "2026-03-08T09:00:00+00:00".into(),
            },
            title: "Hello".into(),
            excerpt: "Hi".into(),
            author_name: "Ann".into(),
            body: BlobKey::from("rendered/b.html"),
        };
        let mut item = encode_doc(&doc);
        assert_eq!(decode_doc(&item), Ok(doc));
        item.insert("date".to_owned(), json!({ "S": "2026-03" }));
        assert_eq!(
            decode_doc(&item).map_err(|error| error.to_string()),
            Err("post doc has no valid date".to_owned())
        );
    }
}
