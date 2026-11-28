//! Transactional outbox for the serverless profile (`docs/aws/architecture.md` section 5).
//!
//! Every write path calls [`write`] on the same [`Store`] that carries its open transaction, so
//! the change and the [`Event`] commit together. The projector job drains rows with [`fetch`]
//! and removes them with [`delete`]. The table exists only on Aurora DSQL, so the `MySql`
//! dialect is refused rather than sent to a server that has no outbox.
//!
//! # Examples
//!
//! ```
//! use cms_outbox::{Aggregate, Change, Event};
//!
//! let event = Event::new(Aggregate::Post, 12, Change::Saved);
//! assert_eq!(event.aggregate().as_str(), "post");
//! assert_eq!(Event::rebuild_all().change(), Change::RebuildAll);
//! ```

use cms_db::{Cell, Dialect, LiteralError, Reply, Row as DbRow, Statement, Store, StoreError};
use cms_db::{TablePrefix, TableStem};
use serde_json::{Map, Value};

/// What kind of record changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Aggregate {
    /// A post, page or attachment row.
    Post,
    /// A term or its taxonomy row.
    Term,
    /// A site option.
    Option,
    /// A comment.
    Comment,
    /// A user.
    User,
    /// The active theme or its menus.
    Theme,
    /// The whole site (used by [`Event::rebuild_all`]).
    Site,
}

impl Aggregate {
    // One table drives both directions so the stored spelling cannot drift.
    const ALL: [(Self, &'static str); 7] = [
        (Self::Post, "post"),
        (Self::Term, "term"),
        (Self::Option, "option"),
        (Self::Comment, "comment"),
        (Self::User, "user"),
        (Self::Theme, "theme"),
        (Self::Site, "site"),
    ];

    /// The value stored in the `aggregate` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(aggregate, _)| *aggregate == self)
            .map_or("", |(_, text)| text)
    }

    /// The aggregate stored as `text`, if known.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, name)| *name == text)
            .map(|(aggregate, _)| *aggregate)
    }
}

/// What happened to the aggregate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Change {
    /// Created or updated (including status changes such as publish or trash).
    Saved,
    /// Removed for good.
    Deleted,
    /// Re-render everything (import finished, theme switched, repair).
    RebuildAll,
}

impl Change {
    const ALL: [(Self, &'static str); 3] = [
        (Self::Saved, "saved"),
        (Self::Deleted, "deleted"),
        (Self::RebuildAll, "rebuild-all"),
    ];

    /// The value stored in the `change` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(change, _)| *change == self)
            .map_or("", |(_, text)| text)
    }

    /// The change stored as `text`, if known.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, name)| *name == text)
            .map(|(change, _)| *change)
    }
}

/// One domain change to project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    aggregate: Aggregate,
    aggregate_id: i64,
    change: Change,
    payload: Map<String, Value>,
}

impl Event {
    /// `change` to `aggregate` number `aggregate_id`, with an empty payload.
    #[must_use]
    pub fn new(aggregate: Aggregate, aggregate_id: i64, change: Change) -> Self {
        Self {
            aggregate,
            aggregate_id,
            change,
            payload: Map::new(),
        }
    }

    /// The site-wide re-render event.
    #[must_use]
    pub fn rebuild_all() -> Self {
        Self::new(Aggregate::Site, 0, Change::RebuildAll)
    }

    /// The same event carrying `payload`, such as an option name or the paths a deleted post
    /// served. The projector loads current state from the database, so payloads hold only what
    /// the database can no longer tell it.
    #[must_use]
    pub fn with_payload(mut self, payload: Map<String, Value>) -> Self {
        self.payload = payload;
        self
    }

    /// The aggregate kind.
    #[must_use]
    pub const fn aggregate(&self) -> Aggregate {
        self.aggregate
    }

    /// The aggregate's id (0 for [`Aggregate::Site`]).
    #[must_use]
    pub const fn aggregate_id(&self) -> i64 {
        self.aggregate_id
    }

    /// The change.
    #[must_use]
    pub const fn change(&self) -> Change {
        self.change
    }

    /// The payload object.
    #[must_use]
    pub const fn payload(&self) -> &Map<String, Value> {
        &self.payload
    }
}

/// One stored outbox row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The row's uuid, as text.
    pub id: Box<str>,
    /// `created_at` as microseconds since the Unix epoch.
    pub created_micros: i64,
    /// The event.
    pub event: Event,
}

/// The outbox could not be read or written.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutboxError {
    /// The store is not on the serverless profile, which is the only one with an outbox.
    #[error("the outbox needs the postgres dialect")]
    Dialect,
    /// A value could not be written as a literal.
    #[error("outbox literal rejected: {0}")]
    Literal(#[from] LiteralError),
    /// The store could not be reached.
    #[error("outbox store failed: {0}")]
    Store(#[from] StoreError),
    /// The server rejected the statement.
    #[error("outbox statement failed: {0}")]
    Server(Box<str>),
    /// A fetched row does not have the outbox shape.
    #[error("malformed outbox row: {0}")]
    Row(Box<str>),
}

/// The physical outbox table for `prefix`.
fn table(prefix: &TablePrefix) -> Result<Vec<u8>, OutboxError> {
    // "outbox" is a constant non-empty stem, so this cannot fail; map it anyway, never panic.
    let stem =
        TableStem::new("outbox").map_err(|error| OutboxError::Row(error.to_string().into()))?;
    Ok(prefix.table(&stem).as_bytes().to_vec())
}

fn postgres(store: &impl Store) -> Result<Statement, OutboxError> {
    match store.dialect() {
        Dialect::Postgres => Ok(Statement::new(Dialect::Postgres)),
        Dialect::MySql => Err(OutboxError::Dialect),
    }
}

fn run(store: &mut impl Store, statement: Statement) -> Result<Reply, OutboxError> {
    let reply = store.query(&statement.finish()?)?;
    match reply {
        Reply::Failed(error) => Err(OutboxError::Server(error.message().into())),
        other => Ok(other),
    }
}

/// Inserts `event` through `store`, which must hold the caller's open transaction.
///
/// This never begins or commits; the caller's commit makes the event visible atomically with
/// the change it describes.
///
/// # Errors
///
/// [`OutboxError`] when the dialect is not `Postgres`, a value cannot be quoted, or the store
/// or server fails.
pub fn write(
    store: &mut impl Store,
    prefix: &TablePrefix,
    event: &Event,
) -> Result<(), OutboxError> {
    let mut sql = postgres(store)?;
    let payload = Value::Object(event.payload.clone()).to_string();
    sql.raw("INSERT INTO ")
        .ident(table(prefix)?)
        .raw(" (")
        .ident("aggregate")
        .raw(", ")
        .ident("aggregate_id")
        .raw(", ")
        .ident("change")
        .raw(", ")
        .ident("payload")
        .raw(") VALUES (")
        .literal(event.aggregate.as_str())
        .raw(", ")
        .number(event.aggregate_id)
        .raw(", ")
        .literal(event.change.as_str())
        .raw(", ")
        .literal(payload)
        .raw("::jsonb)");
    run(store, sql).map(|_| ())
}

/// Up to `limit` rows, oldest first (ties broken by id so replays see the same order).
///
/// # Errors
///
/// [`OutboxError`] when the dialect is wrong, the store or server fails, or a row is malformed.
pub fn fetch(
    store: &mut impl Store,
    prefix: &TablePrefix,
    limit: u32,
) -> Result<Vec<Row>, OutboxError> {
    let mut sql = postgres(store)?;
    sql.raw("SELECT ")
        .ident("id")
        .raw("::text, floor(extract(epoch from ")
        .ident("created_at")
        .raw(") * 1000000)::bigint, ")
        .ident("aggregate")
        .raw(", ")
        .ident("aggregate_id")
        .raw(", ")
        .ident("change")
        .raw(", ")
        .ident("payload")
        .raw("::text FROM ")
        .ident(table(prefix)?)
        .raw(" ORDER BY ")
        .ident("created_at")
        .raw(", ")
        .ident("id")
        .raw(" LIMIT ")
        .number(limit);
    let reply = run(store, sql)?;
    let set = reply
        .as_rows()
        .ok_or_else(|| OutboxError::Row("expected a result set".into()))?;
    set.rows().iter().map(parse_row).collect()
}

/// Removes the rows with these ids. An empty list sends nothing.
///
/// # Errors
///
/// [`OutboxError`] when the dialect is wrong, an id cannot be quoted, or the store or server
/// fails.
pub fn delete(
    store: &mut impl Store,
    prefix: &TablePrefix,
    ids: &[Box<str>],
) -> Result<(), OutboxError> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut sql = postgres(store)?;
    sql.raw("DELETE FROM ")
        .ident(table(prefix)?)
        .raw(" WHERE ")
        .ident("id")
        .raw("::text IN (");
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.literal(id.as_bytes());
    }
    sql.raw(")");
    run(store, sql).map(|_| ())
}

// Cells come back as text in the column order `fetch` selects.
fn parse_row(row: &DbRow) -> Result<Row, OutboxError> {
    let text = |index: usize| -> Result<&str, OutboxError> {
        match row.cells().get(index) {
            Some(Cell::Bytes(bytes)) => std::str::from_utf8(bytes)
                .map_err(|error| OutboxError::Row(format!("column {index}: {error}").into())),
            Some(Cell::Null) | None => Err(OutboxError::Row(
                format!("column {index} is missing").into(),
            )),
        }
    };
    let number = |index: usize| -> Result<i64, OutboxError> {
        text(index)?
            .parse()
            .map_err(|error| OutboxError::Row(format!("column {index}: {error}").into()))
    };
    let (aggregate, change) = (text(2)?, text(4)?);
    let aggregate = Aggregate::parse(aggregate)
        .ok_or_else(|| OutboxError::Row(format!("unknown aggregate {aggregate}").into()))?;
    let change = Change::parse(change)
        .ok_or_else(|| OutboxError::Row(format!("unknown change {change}").into()))?;
    let payload = match serde_json::from_str(text(5)?) {
        Ok(Value::Object(map)) => map,
        Ok(_) => return Err(OutboxError::Row("payload is not an object".into())),
        Err(error) => return Err(OutboxError::Row(format!("payload: {error}").into())),
    };
    Ok(Row {
        id: text(0)?.into(),
        created_micros: number(1)?,
        event: Event::new(aggregate, number(3)?, change).with_payload(payload),
    })
}

#[cfg(test)]
mod tests;
