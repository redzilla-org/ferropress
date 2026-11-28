//! The storage operations the editor workflows need, and their SQL over [`cms_db::Store`].

use std::fmt;

use cms_content::{Field, PostId};
use cms_db::{Cell, Reply, ResultSet, Sql, Statement, Store};

use crate::Error;
use crate::model::{OutboxEvent, PostRow, PostWrite, StatusFilter, TermRow};

/// Typed storage operations. [`SqlRepo`] is the production implementation; workflows depend on
/// this trait so their tests can run against an in-memory model.
pub trait Repo {
    /// Opens the write transaction.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn begin(&mut self) -> Result<(), Error>;
    /// Commits it.
    ///
    /// # Errors
    ///
    /// [`Error::Conflict`] when the server refuses the commit.
    fn commit(&mut self) -> Result<(), Error>;
    /// Abandons it.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn rollback(&mut self) -> Result<(), Error>;
    /// One post row.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn post(&mut self, id: PostId) -> Result<Option<PostRow>, Error>;
    /// Posts of type `post` newest first, `limit` rows from `offset`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn list(
        &mut self,
        filter: StatusFilter,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<PostRow>, Error>;
    /// Whether another post of `type_name` already uses `slug`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn slug_taken(
        &mut self,
        slug: &str,
        type_name: &str,
        except: Option<PostId>,
    ) -> Result<bool, Error>;
    /// Inserts a `post` row and returns its id.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails or returns no id.
    fn insert_post(&mut self, row: &PostWrite) -> Result<PostId, Error>;
    /// Rewrites the editable columns of `id`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn update_post(&mut self, id: PostId, row: &PostWrite) -> Result<(), Error>;
    /// Sets `guid` (written once, after insert).
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn set_guid(&mut self, id: PostId, guid: &str) -> Result<(), Error>;
    /// Every value of `key` on `id`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn meta(&mut self, id: PostId, key: &str) -> Result<Vec<String>, Error>;
    /// Adds one meta row.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn add_meta(&mut self, id: PostId, key: &str, value: &str) -> Result<(), Error>;
    /// Deletes meta rows of `key`, only those equal to `value` when given.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn delete_meta(&mut self, id: PostId, key: &str, value: Option<&str>) -> Result<(), Error>;
    /// Every term of `taxonomy`, by name.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn terms(&mut self, taxonomy: &str) -> Result<Vec<TermRow>, Error>;
    /// Categories and tags attached to `id`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn post_terms(&mut self, id: PostId) -> Result<Vec<TermRow>, Error>;
    /// Terms of `taxonomy` with these term ids (unknown ids are absent).
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn find_terms(&mut self, taxonomy: &str, ids: &[u64]) -> Result<Vec<TermRow>, Error>;
    /// The term of `taxonomy` with `slug`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn term_by_slug(&mut self, taxonomy: &str, slug: &str) -> Result<Option<TermRow>, Error>;
    /// Inserts a term and its taxonomy row.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails or returns no id.
    fn insert_term(&mut self, taxonomy: &str, name: &str, slug: &str) -> Result<TermRow, Error>;
    /// Attaches one term taxonomy id to `id`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn link(&mut self, id: PostId, taxonomy_id: u64) -> Result<(), Error>;
    /// Detaches term taxonomy ids from `id`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn unlink(&mut self, id: PostId, taxonomy_ids: &[u64]) -> Result<(), Error>;
    /// Recomputes `count` from published posts, as the reference's post term count does.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn recount(&mut self, taxonomy_id: u64) -> Result<(), Error>;
    /// One option value.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn option(&mut self, name: &str) -> Result<Option<String>, Error>;
    /// Writes one outbox row.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn outbox(&mut self, event: &OutboxEvent) -> Result<(), Error>;
    /// A user's `display_name` and `user_nicename`, for bylines.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn author(&mut self, id: u64) -> Result<Option<(String, String)>, Error>;
}

/// Post columns the admin reads, in select order.
const POST_COLUMNS: [&str; 12] = [
    "ID",
    "post_author",
    "post_date",
    "post_date_gmt",
    "post_content",
    "post_title",
    "post_excerpt",
    "post_status",
    "post_name",
    "post_type",
    "post_modified",
    "post_modified_gmt",
];

/// [`Repo`] over one store connection, spelled in the store's dialect.
pub struct SqlRepo<'a> {
    store: &'a mut dyn Store,
    prefix: cms_db::TablePrefix,
}

impl<'a> SqlRepo<'a> {
    /// Operations on the tables named `{prefix}posts` and so on.
    pub fn new(store: &'a mut dyn Store, prefix: &cms_db::TablePrefix) -> Self {
        Self {
            store,
            prefix: prefix.clone(),
        }
    }

    fn statement(&self) -> Statement {
        Statement::new(self.store.dialect())
    }

    fn table(&self, stem: &str) -> Vec<u8> {
        let mut name = self.prefix.as_bytes().to_vec();
        name.extend_from_slice(stem.as_bytes());
        name
    }

    // A server rejection inside a write must abort it, so it becomes an error here.
    fn run(&mut self, statement: Statement) -> Result<Reply, Error> {
        let sql = statement.finish()?;
        match self.store.query(&sql) {
            Ok(Reply::Failed(error)) => Err(Error::Rejected(error.message().into())),
            Ok(reply) => Ok(reply),
            Err(_store) => Err(Error::Unavailable),
        }
    }

    fn rows(&mut self, statement: Statement) -> Result<ResultSet, Error> {
        match self.run(statement)? {
            Reply::Rows(set) => Ok(set),
            _ => Err(Error::Rejected("expected rows".into())),
        }
    }

    fn done(&mut self, statement: Statement) -> Result<(u64, u64), Error> {
        self.run(statement)?
            .as_done()
            .ok_or_else(|| Error::Rejected("expected a write acknowledgement".into()))
    }

    fn plain(&mut self, text: &str) -> Result<Reply, Error> {
        match self.store.query(&Sql::from(text)) {
            Ok(reply) => Ok(reply),
            Err(_store) => Err(Error::Unavailable),
        }
    }

    fn select_posts(&self) -> Statement {
        let mut sql = self.statement();
        sql.raw("SELECT ");
        for (index, column) in POST_COLUMNS.iter().enumerate() {
            if index > 0 {
                sql.raw(", ");
            }
            sql.ident(column);
        }
        sql.raw(" FROM ").ident(self.table("posts"));
        sql
    }

    fn select_terms(&self) -> Statement {
        let mut sql = self.statement();
        sql.raw("SELECT ");
        qualified(&mut sql, "t", "term_id")
            .raw(", ")
            .ident("tt")
            .raw(".")
            .ident("term_taxonomy_id")
            .raw(", ");
        qualified(&mut sql, "tt", "taxonomy").raw(", ");
        qualified(&mut sql, "t", "name").raw(", ");
        qualified(&mut sql, "t", "slug")
            .raw(" FROM ")
            .ident(self.table("terms"))
            .raw(" AS ")
            .ident("t")
            .raw(" INNER JOIN ")
            .ident(self.table("term_taxonomy"))
            .raw(" AS ")
            .ident("tt")
            .raw(" ON ");
        qualified(&mut sql, "t", "term_id").raw(" = ");
        qualified(&mut sql, "tt", "term_id");
        sql
    }
}

impl Repo for SqlRepo<'_> {
    fn begin(&mut self) -> Result<(), Error> {
        match self.plain("BEGIN")? {
            Reply::Failed(error) => Err(Error::Rejected(error.message().into())),
            _ => Ok(()),
        }
    }

    fn commit(&mut self) -> Result<(), Error> {
        match self.plain("COMMIT")? {
            Reply::Failed(_conflict) => Err(Error::Conflict),
            _ => Ok(()),
        }
    }

    fn rollback(&mut self) -> Result<(), Error> {
        self.plain("ROLLBACK").map(|_reply| ())
    }

    fn post(&mut self, id: PostId) -> Result<Option<PostRow>, Error> {
        let mut sql = self.select_posts();
        sql.raw(" WHERE ")
            .ident("ID")
            .raw(" = ")
            .number(id.as_u64())
            .raw(" LIMIT 1");
        Ok(post_rows(&self.rows(sql)?).into_iter().next())
    }

    fn list(
        &mut self,
        filter: StatusFilter,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<PostRow>, Error> {
        let mut sql = self.select_posts();
        sql.raw(" WHERE ")
            .ident("post_type")
            .raw(" = ")
            .literal("post")
            .raw(" AND ");
        match filter {
            StatusFilter::All => {
                sql.ident("post_status")
                    .raw(" NOT IN (")
                    .literal("trash")
                    .raw(", ")
                    .literal("auto-draft")
                    .raw(")");
            }
            StatusFilter::Only(status) => {
                sql.ident("post_status").raw(" = ").literal(status.as_str());
            }
        }
        sql.raw(" ORDER BY ")
            .ident("post_date")
            .raw(" DESC, ")
            .ident("ID")
            .raw(" DESC LIMIT ")
            .number(limit)
            .raw(" OFFSET ")
            .number(offset);
        Ok(post_rows(&self.rows(sql)?))
    }

    fn slug_taken(
        &mut self,
        slug: &str,
        type_name: &str,
        except: Option<PostId>,
    ) -> Result<bool, Error> {
        let mut sql = self.statement();
        sql.raw("SELECT ")
            .ident("post_name")
            .raw(" FROM ")
            .ident(self.table("posts"))
            .raw(" WHERE ")
            .case_fold(|s| s.ident("post_name"))
            .raw(" = ")
            .case_fold(|s| s.literal(slug))
            .raw(" AND ")
            .ident("post_type")
            .raw(" = ")
            .literal(type_name)
            .raw(" AND ")
            .ident("ID")
            .raw(" <> ")
            .number(except.map_or(0, PostId::as_u64))
            .raw(" LIMIT 1");
        Ok(!self.rows(sql)?.rows().is_empty())
    }

    fn insert_post(&mut self, row: &PostWrite) -> Result<PostId, Error> {
        let mut sql = self.statement();
        sql.raw("INSERT INTO ").ident(self.table("posts")).raw(" (");
        for (index, field) in Field::ALL.iter().enumerate() {
            if index > 0 {
                sql.raw(", ");
            }
            sql.ident(field.column());
        }
        sql.raw(") VALUES (");
        for (index, field) in Field::ALL.iter().enumerate() {
            if index > 0 {
                sql.raw(", ");
            }
            insert_value(&mut sql, *field, row);
        }
        sql.raw(")").returning("ID");
        let (_affected, id) = self.done(sql)?;
        PostId::new(id).map_err(|_zero| Error::Rejected("insert returned no id".into()))
    }

    fn update_post(&mut self, id: PostId, row: &PostWrite) -> Result<(), Error> {
        let cells: [(&str, &str); 9] = [
            ("post_title", &row.title),
            ("post_content", &row.content),
            ("post_excerpt", &row.excerpt),
            ("post_status", row.status.as_str()),
            ("post_name", &row.name),
            ("post_date", &row.date),
            ("post_date_gmt", &row.date_gmt),
            ("post_modified", &row.modified),
            ("post_modified_gmt", &row.modified_gmt),
        ];
        let mut sql = self.statement();
        sql.raw("UPDATE ").ident(self.table("posts")).raw(" SET ");
        for (index, (column, value)) in cells.iter().enumerate() {
            if index > 0 {
                sql.raw(", ");
            }
            sql.ident(column).raw(" = ").literal(value);
        }
        sql.raw(" WHERE ")
            .ident("ID")
            .raw(" = ")
            .number(id.as_u64());
        match self.done(sql)? {
            (0, _) => Err(Error::NotFound("post")),
            _ => Ok(()),
        }
    }

    fn set_guid(&mut self, id: PostId, guid: &str) -> Result<(), Error> {
        let mut sql = self.statement();
        sql.raw("UPDATE ")
            .ident(self.table("posts"))
            .raw(" SET ")
            .ident("guid")
            .raw(" = ")
            .literal(guid)
            .raw(" WHERE ")
            .ident("ID")
            .raw(" = ")
            .number(id.as_u64());
        self.done(sql).map(|_done| ())
    }

    fn meta(&mut self, id: PostId, key: &str) -> Result<Vec<String>, Error> {
        let mut sql = self.statement();
        sql.raw("SELECT ")
            .ident("meta_value")
            .raw(" FROM ")
            .ident(self.table("postmeta"))
            .raw(" WHERE ")
            .ident("post_id")
            .raw(" = ")
            .number(id.as_u64())
            .raw(" AND ")
            .ident("meta_key")
            .raw(" = ")
            .literal(key)
            .raw(" ORDER BY ")
            .ident("meta_id");
        let set = self.rows(sql)?;
        Ok(set
            .rows()
            .iter()
            .map(|row| row.cells().first().map(text).unwrap_or_default())
            .collect())
    }

    fn add_meta(&mut self, id: PostId, key: &str, value: &str) -> Result<(), Error> {
        let mut sql = self.statement();
        sql.raw("INSERT INTO ")
            .ident(self.table("postmeta"))
            .raw(" (")
            .ident("post_id")
            .raw(", ")
            .ident("meta_key")
            .raw(", ")
            .ident("meta_value")
            .raw(") VALUES (")
            .number(id.as_u64())
            .raw(", ")
            .literal(key)
            .raw(", ")
            .literal(value)
            .raw(")");
        self.done(sql).map(|_done| ())
    }

    fn delete_meta(&mut self, id: PostId, key: &str, value: Option<&str>) -> Result<(), Error> {
        let mut sql = self.statement();
        sql.raw("DELETE FROM ")
            .ident(self.table("postmeta"))
            .raw(" WHERE ")
            .ident("post_id")
            .raw(" = ")
            .number(id.as_u64())
            .raw(" AND ")
            .ident("meta_key")
            .raw(" = ")
            .literal(key);
        if let Some(value) = value {
            sql.raw(" AND ")
                .ident("meta_value")
                .raw(" = ")
                .literal(value);
        }
        self.done(sql).map(|_done| ())
    }

    fn terms(&mut self, taxonomy: &str) -> Result<Vec<TermRow>, Error> {
        let mut sql = self.select_terms();
        sql.raw(" WHERE ");
        qualified(&mut sql, "tt", "taxonomy")
            .raw(" = ")
            .literal(taxonomy)
            .raw(" ORDER BY ");
        qualified(&mut sql, "t", "name").raw(" ASC");
        Ok(term_rows(&self.rows(sql)?))
    }

    fn post_terms(&mut self, id: PostId) -> Result<Vec<TermRow>, Error> {
        let mut sql = self.select_terms();
        sql.raw(" INNER JOIN ")
            .ident(self.table("term_relationships"))
            .raw(" AS ")
            .ident("tr")
            .raw(" ON ");
        qualified(&mut sql, "tr", "term_taxonomy_id").raw(" = ");
        qualified(&mut sql, "tt", "term_taxonomy_id").raw(" WHERE ");
        qualified(&mut sql, "tr", "object_id")
            .raw(" = ")
            .number(id.as_u64())
            .raw(" AND ");
        qualified(&mut sql, "tt", "taxonomy")
            .raw(" IN (")
            .literal("category")
            .raw(", ")
            .literal("post_tag")
            .raw(") ORDER BY ");
        qualified(&mut sql, "t", "name").raw(" ASC");
        Ok(term_rows(&self.rows(sql)?))
    }

    fn find_terms(&mut self, taxonomy: &str, ids: &[u64]) -> Result<Vec<TermRow>, Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut sql = self.select_terms();
        sql.raw(" WHERE ");
        qualified(&mut sql, "tt", "taxonomy")
            .raw(" = ")
            .literal(taxonomy)
            .raw(" AND ");
        qualified(&mut sql, "t", "term_id").raw(" IN (");
        numbers(&mut sql, ids).raw(")");
        Ok(term_rows(&self.rows(sql)?))
    }

    fn term_by_slug(&mut self, taxonomy: &str, slug: &str) -> Result<Option<TermRow>, Error> {
        let mut sql = self.select_terms();
        sql.raw(" WHERE ");
        qualified(&mut sql, "tt", "taxonomy")
            .raw(" = ")
            .literal(taxonomy)
            .raw(" AND ")
            .case_fold(|s| qualified(s, "t", "slug"))
            .raw(" = ")
            .case_fold(|s| s.literal(slug))
            .raw(" LIMIT 1");
        Ok(term_rows(&self.rows(sql)?).into_iter().next())
    }

    fn insert_term(&mut self, taxonomy: &str, name: &str, slug: &str) -> Result<TermRow, Error> {
        let mut sql = self.statement();
        sql.raw("INSERT INTO ")
            .ident(self.table("terms"))
            .raw(" (")
            .ident("name")
            .raw(", ")
            .ident("slug")
            .raw(", ")
            .ident("term_group")
            .raw(") VALUES (")
            .literal(name)
            .raw(", ")
            .literal(slug)
            .raw(", 0)")
            .returning("term_id");
        let (_affected, term_id) = self.done(sql)?;
        let mut sql = self.statement();
        sql.raw("INSERT INTO ")
            .ident(self.table("term_taxonomy"))
            .raw(" (")
            .ident("term_id")
            .raw(", ")
            .ident("taxonomy")
            .raw(", ")
            .ident("description")
            .raw(", ")
            .ident("parent")
            .raw(", ")
            .ident("count")
            .raw(") VALUES (")
            .number(term_id)
            .raw(", ")
            .literal(taxonomy)
            .raw(", ")
            .literal("")
            .raw(", 0, 0)")
            .returning("term_taxonomy_id");
        let (_affected, taxonomy_id) = self.done(sql)?;
        if term_id == 0 || taxonomy_id == 0 {
            return Err(Error::Rejected("term insert returned no id".into()));
        }
        Ok(TermRow {
            term_id,
            taxonomy_id,
            taxonomy: taxonomy.into(),
            name: name.into(),
            slug: slug.into(),
        })
    }

    fn link(&mut self, id: PostId, taxonomy_id: u64) -> Result<(), Error> {
        let mut sql = self.statement();
        sql.raw("INSERT INTO ")
            .ident(self.table("term_relationships"))
            .raw(" (")
            .ident("object_id")
            .raw(", ")
            .ident("term_taxonomy_id")
            .raw(", ")
            .ident("term_order")
            .raw(") VALUES (")
            .number(id.as_u64())
            .raw(", ")
            .number(taxonomy_id)
            .raw(", 0)");
        self.done(sql).map(|_done| ())
    }

    fn unlink(&mut self, id: PostId, taxonomy_ids: &[u64]) -> Result<(), Error> {
        if taxonomy_ids.is_empty() {
            return Ok(());
        }
        let mut sql = self.statement();
        sql.raw("DELETE FROM ")
            .ident(self.table("term_relationships"))
            .raw(" WHERE ")
            .ident("object_id")
            .raw(" = ")
            .number(id.as_u64())
            .raw(" AND ")
            .ident("term_taxonomy_id")
            .raw(" IN (");
        numbers(&mut sql, taxonomy_ids).raw(")");
        self.done(sql).map(|_done| ())
    }

    fn recount(&mut self, taxonomy_id: u64) -> Result<(), Error> {
        let mut sql = self.statement();
        sql.raw("UPDATE ")
            .ident(self.table("term_taxonomy"))
            .raw(" SET ")
            .ident("count")
            .raw(" = (SELECT COUNT(*) FROM ")
            .ident(self.table("term_relationships"))
            .raw(" AS ")
            .ident("tr")
            .raw(" INNER JOIN ")
            .ident(self.table("posts"))
            .raw(" AS ")
            .ident("p")
            .raw(" ON ");
        qualified(&mut sql, "p", "ID").raw(" = ");
        qualified(&mut sql, "tr", "object_id").raw(" WHERE ");
        qualified(&mut sql, "tr", "term_taxonomy_id")
            .raw(" = ")
            .number(taxonomy_id)
            .raw(" AND ");
        qualified(&mut sql, "p", "post_status")
            .raw(" = ")
            .literal("publish")
            .raw(" AND ");
        qualified(&mut sql, "p", "post_type")
            .raw(" = ")
            .literal("post")
            .raw(") WHERE ")
            .ident("term_taxonomy_id")
            .raw(" = ")
            .number(taxonomy_id);
        self.done(sql).map(|_done| ())
    }

    fn option(&mut self, name: &str) -> Result<Option<String>, Error> {
        let mut sql = self.statement();
        sql.raw("SELECT ")
            .ident("option_value")
            .raw(" FROM ")
            .ident(self.table("options"))
            .raw(" WHERE ")
            .ident("option_name")
            .raw(" = ")
            .literal(name)
            .raw(" LIMIT 1");
        let set = self.rows(sql)?;
        Ok(set
            .rows()
            .first()
            .and_then(|row| row.cells().first())
            .map(text))
    }

    // The shared writer owns the row's shape, so the projector can always parse what the admin
    // writes; it runs on this connection, inside the caller's open transaction.
    fn outbox(&mut self, event: &OutboxEvent) -> Result<(), Error> {
        cms_outbox::write(
            &mut Through(&mut *self.store),
            &self.prefix,
            &event.to_event(),
        )
        .map_err(|error| match error {
            cms_outbox::OutboxError::Store(_) => Error::Unavailable,
            other => Error::Rejected(other.to_string().into()),
        })
    }

    fn author(&mut self, id: u64) -> Result<Option<(String, String)>, Error> {
        let mut sql = self.statement();
        sql.raw("SELECT ")
            .ident("display_name")
            .raw(", ")
            .ident("user_nicename")
            .raw(" FROM ")
            .ident(self.table("users"))
            .raw(" WHERE ")
            .ident("ID")
            .raw(" = ")
            .number(id)
            .raw(" LIMIT 1");
        let set = self.rows(sql)?;
        Ok(set.rows().first().map(|row| {
            let get = named(&set, row);
            (get("display_name"), get("user_nicename"))
        }))
    }
}

impl fmt::Debug for SqlRepo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqlRepo")
            .field("prefix", &String::from_utf8_lossy(self.prefix.as_bytes()))
            .finish_non_exhaustive()
    }
}

// `cms_outbox::write` takes a sized store; this forwards to the repo's borrowed connection.
struct Through<'s>(&'s mut dyn Store);

impl Store for Through<'_> {
    fn dialect(&self) -> cms_db::Dialect {
        self.0.dialect()
    }

    fn query(&mut self, sql: &Sql) -> Result<Reply, cms_db::StoreError> {
        self.0.query(sql)
    }
}

fn qualified<'s>(sql: &'s mut Statement, alias: &str, column: &str) -> &'s mut Statement {
    sql.ident(alias).raw(".").ident(column)
}

fn numbers<'s>(sql: &'s mut Statement, values: &[u64]) -> &'s mut Statement {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.number(value);
    }
    sql
}

// Insert values follow the reference's compact list; columns the editor does not set take the
// reference defaults (comments open, no parent, empty ping lists).
fn insert_value(sql: &mut Statement, field: Field, row: &PostWrite) {
    match field {
        Field::Author => {
            sql.number(row.author);
        }
        Field::Parent | Field::MenuOrder => {
            sql.number(0);
        }
        Field::Date => {
            sql.literal(&row.date);
        }
        Field::DateGmt => {
            sql.literal(&row.date_gmt);
        }
        Field::Content => {
            sql.literal(&row.content);
        }
        Field::Title => {
            sql.literal(&row.title);
        }
        Field::Excerpt => {
            sql.literal(&row.excerpt);
        }
        Field::Status => {
            sql.literal(row.status.as_str());
        }
        Field::TypeName => {
            sql.literal("post");
        }
        Field::CommentStatus | Field::PingStatus => {
            sql.literal("open");
        }
        Field::Name => {
            sql.literal(&row.name);
        }
        Field::Modified => {
            sql.literal(&row.modified);
        }
        Field::ModifiedGmt => {
            sql.literal(&row.modified_gmt);
        }
        Field::ContentFiltered
        | Field::Password
        | Field::ToPing
        | Field::Pinged
        | Field::MimeType
        | Field::Guid => {
            sql.literal("");
        }
    }
}

fn text(cell: &Cell) -> String {
    match cell {
        Cell::Null => String::new(),
        Cell::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

// Rows are read by column name, so column order in the reply does not matter.
fn named<'s>(set: &'s ResultSet, row: &'s cms_db::Row) -> impl Fn(&str) -> String + 's {
    move |name: &str| {
        set.columns()
            .iter()
            .position(|column| column.as_bytes() == name.as_bytes())
            .and_then(|index| row.cells().get(index))
            .map(text)
            .unwrap_or_default()
    }
}

fn number(text: &str) -> u64 {
    text.parse().unwrap_or(0)
}

fn post_rows(set: &ResultSet) -> Vec<PostRow> {
    set.rows()
        .iter()
        .filter_map(|row| {
            let get = named(set, row);
            Some(PostRow {
                id: PostId::new(number(&get("ID"))).ok()?,
                author: number(&get("post_author")),
                date: get("post_date"),
                date_gmt: get("post_date_gmt"),
                content: get("post_content"),
                title: get("post_title"),
                excerpt: get("post_excerpt"),
                status: get("post_status"),
                name: get("post_name"),
                type_name: get("post_type"),
                modified: get("post_modified"),
                modified_gmt: get("post_modified_gmt"),
            })
        })
        .collect()
}

fn term_rows(set: &ResultSet) -> Vec<TermRow> {
    set.rows()
        .iter()
        .map(|row| {
            let get = named(set, row);
            TermRow {
                term_id: number(&get("term_id")),
                taxonomy_id: number(&get("term_taxonomy_id")),
                taxonomy: get("taxonomy"),
                name: get("name"),
                slug: get("slug"),
            }
        })
        .collect()
}
