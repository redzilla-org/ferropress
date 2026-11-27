//! [`History`] lookups and the comment-plus-outbox transaction on a [`cms_db::Store`].

use cms_db::{Cell, Dialect, Reply, ResultSet, Sql, Store, TableName, TablePrefix, TableStem};

use crate::decide::{Approval, Candidate, History, Post};
use crate::error::Error;

/// One site's comment tables behind a [`Store`], spelled in the store's [`Dialect`].
///
/// Request-affine like the store it wraps.
#[derive(Debug)]
pub struct SqlIntake<S> {
    store: S,
    dialect: Dialect,
    comments: TableName,
    posts: TableName,
    users: TableName,
    outbox: TableName,
}

impl<S: Store> SqlIntake<S> {
    /// Tables `{prefix}comments`, `{prefix}posts`, `{prefix}users` and `{prefix}outbox`.
    ///
    /// # Errors
    ///
    /// [`Error::Table`] when a table name cannot be formed.
    pub fn new(store: S, dialect: Dialect, prefix: &TablePrefix) -> Result<Self, Error> {
        let table = |stem: &str| TableStem::new(stem).map(|stem| prefix.table(&stem));
        Ok(Self {
            store,
            dialect,
            comments: table("comments")?,
            posts: table("posts")?,
            users: table("users")?,
            outbox: table("outbox")?,
        })
    }

    /// The wrapped store, for callers that reuse its connection.
    pub fn into_store(self) -> S {
        self.store
    }

    /// Stores `candidate` in one transaction with its outbox row and, when approved, the
    /// post's refreshed approved-comment count. Returns the new comment id.
    ///
    /// # Errors
    ///
    /// [`Error::Write`] when the server refuses a statement (the transaction is rolled back),
    /// [`Error::Unavailable`] when the store fails, [`Error::Literal`] for unstorable text.
    pub fn commit(&mut self, candidate: &Candidate) -> Result<u64, Error> {
        let insert = self.insert_comment(candidate)?;
        self.exec(&Sql::from("BEGIN"))?;
        let outcome = self.write_all(candidate, &insert);
        if outcome.is_err() {
            // Best effort: the server also discards an unfinished transaction on disconnect.
            let _rolled_back = self.store.query(&Sql::from("ROLLBACK"));
        }
        outcome
    }

    fn write_all(&mut self, candidate: &Candidate, insert: &Sql) -> Result<u64, Error> {
        let id = match self.exec(insert)? {
            Reply::Done { insert_id, .. } => insert_id,
            Reply::Rows(set) => first_text(&set)
                .and_then(|text| text.parse().ok())
                .unwrap_or(0),
            _ => 0,
        };
        if id == 0 {
            return Err(Error::Write("insert returned no comment id".into()));
        }
        if candidate.approval == Approval::Approved {
            let count = self.count_update(candidate.post_id);
            self.exec(&count)?;
        }
        let outbox = self.outbox_row(candidate, id)?;
        self.exec(&outbox)?;
        self.exec(&Sql::from("COMMIT"))?;
        Ok(id)
    }

    fn insert_comment(&self, c: &Candidate) -> Result<Sql, Error> {
        let d = self.dialect;
        let mut sql = b"INSERT INTO ".to_vec();
        sql.extend(d.ident(self.comments.as_bytes()));
        sql.extend_from_slice(b" (");
        let columns: [&[u8]; 14] = [
            b"comment_post_ID",
            b"comment_author_IP",
            b"comment_author",
            b"comment_author_email",
            b"comment_author_url",
            b"comment_date",
            b"comment_date_gmt",
            b"comment_content",
            b"comment_karma",
            b"comment_approved",
            b"comment_agent",
            b"comment_type",
            b"comment_parent",
            b"user_id",
        ];
        join(&mut sql, columns.iter().map(|column| d.ident(column)));
        sql.extend_from_slice(b") VALUES (");
        let texts = [
            &*c.ip,
            &*c.author,
            &*c.email,
            &*c.url,
            &*c.date,
            &*c.date_gmt,
            &*c.content,
        ];
        let mut values = vec![c.post_id.to_string().into_bytes()];
        for text in texts {
            values.push(d.literal(text.as_bytes())?);
        }
        values.push(b"0".to_vec());
        values.push(d.literal(c.approval.cell().as_bytes())?);
        values.push(d.literal(c.agent.as_bytes())?);
        values.push(d.literal(b"comment")?);
        values.push(c.parent.to_string().into_bytes());
        values.push(c.user_id.to_string().into_bytes());
        join(&mut sql, values);
        sql.push(b')');
        if d == Dialect::Postgres {
            sql.extend_from_slice(b" RETURNING ");
            sql.extend(d.ident(b"comment_ID"));
        }
        Ok(Sql::new(sql))
    }

    fn count_update(&self, post_id: u64) -> Sql {
        let d = self.dialect;
        let mut sql = b"UPDATE ".to_vec();
        sql.extend(d.ident(self.posts.as_bytes()));
        sql.extend_from_slice(b" SET ");
        sql.extend(d.ident(b"comment_count"));
        sql.extend_from_slice(b" = (SELECT COUNT(*) FROM ");
        sql.extend(d.ident(self.comments.as_bytes()));
        sql.extend_from_slice(b" WHERE ");
        sql.extend(d.ident(b"comment_post_ID"));
        sql.extend_from_slice(format!(" = {post_id} AND ").as_bytes());
        sql.extend(d.ident(b"comment_approved"));
        sql.extend_from_slice(b" = '1') WHERE ");
        sql.extend(d.ident(b"ID"));
        sql.extend_from_slice(format!(" = {post_id}").as_bytes());
        Sql::new(sql)
    }

    fn outbox_row(&self, candidate: &Candidate, id: u64) -> Result<Sql, Error> {
        let d = self.dialect;
        let mut sql = b"INSERT INTO ".to_vec();
        sql.extend(d.ident(self.outbox.as_bytes()));
        sql.extend_from_slice(b" (");
        join(
            &mut sql,
            [&b"aggregate"[..], b"aggregate_id", b"change", b"payload"]
                .iter()
                .map(|column| d.ident(column)),
        );
        sql.extend_from_slice(b") VALUES (");
        // Both payload values are digits or fixed status words, so the JSON needs no escaping.
        let payload = format!(
            "{{\"post_id\":{},\"approved\":\"{}\"}}",
            candidate.post_id,
            candidate.approval.cell()
        );
        join(
            &mut sql,
            [
                d.literal(b"comment")?,
                id.to_string().into_bytes(),
                d.literal(b"insert")?,
                d.literal(payload.as_bytes())?,
            ],
        );
        sql.push(b')');
        Ok(Sql::new(sql))
    }

    // A statement whose failure aborts the transaction.
    fn exec(&mut self, sql: &Sql) -> Result<Reply, Error> {
        match self.store.query(sql) {
            Ok(Reply::Failed(error)) => Err(Error::Write(error.message().into())),
            Ok(reply) => Ok(reply),
            Err(_unreachable) => Err(Error::Unavailable),
        }
    }

    // A lookup: rows, or an error for a refused statement or a store failure.
    fn rows(&mut self, sql: &Sql) -> Result<ResultSet, Error> {
        match self.exec(sql)? {
            Reply::Rows(set) => Ok(set),
            _ => Ok(ResultSet::new(Vec::new(), Vec::new())),
        }
    }

    fn select(&self, columns: &[&[u8]], table: &TableName) -> Vec<u8> {
        let mut sql = b"SELECT ".to_vec();
        join(
            &mut sql,
            columns.iter().map(|column| self.dialect.ident(column)),
        );
        sql.extend_from_slice(b" FROM ");
        sql.extend(self.dialect.ident(table.as_bytes()));
        sql.extend_from_slice(b" WHERE ");
        sql
    }

    // `"column" = 'text'`.
    fn equals_text(&self, sql: &mut Vec<u8>, column: &[u8], text: &str) -> Result<(), Error> {
        sql.extend(self.dialect.ident(column));
        sql.extend_from_slice(b" = ");
        sql.extend(self.dialect.literal(text.as_bytes())?);
        Ok(())
    }

    // `"column" = 12`.
    fn equals_number(&self, sql: &mut Vec<u8>, column: &[u8], number: u64) {
        sql.extend(self.dialect.ident(column));
        sql.extend_from_slice(format!(" = {number}").as_bytes());
    }
}

impl<S: Store> History for SqlIntake<S> {
    fn post(&mut self, id: u64) -> Result<Option<Post>, Error> {
        let columns: [&[u8]; 7] = [
            b"ID",
            b"post_author",
            b"post_status",
            b"comment_status",
            b"post_type",
            b"post_password",
            b"post_date_gmt",
        ];
        let mut sql = self.select(&columns, &self.posts);
        self.equals_number(&mut sql, b"ID", id);
        sql.extend_from_slice(b" LIMIT 1");
        let set = self.rows(&Sql::new(sql))?;
        let Some(row) = set.rows().first() else {
            return Ok(None);
        };
        let cell = |index: usize| -> Box<str> {
            match row.cells().get(index) {
                Some(Cell::Bytes(bytes)) => String::from_utf8_lossy(bytes).into(),
                _ => "".into(),
            }
        };
        let number = |index: usize| cell(index).parse::<u64>().unwrap_or(0);
        Ok(Some(Post {
            id: number(0),
            author: number(1),
            status: cell(2),
            comment_status: cell(3),
            post_type: cell(4),
            has_password: !cell(5).is_empty(),
            date_gmt: cell(6),
        }))
    }

    fn comment_status(&mut self, id: u64) -> Result<Option<Box<str>>, Error> {
        let mut sql = self.select(&[b"comment_approved"], &self.comments);
        self.equals_number(&mut sql, b"comment_ID", id);
        sql.extend_from_slice(b" LIMIT 1");
        let set = self.rows(&Sql::new(sql))?;
        Ok(first_text(&set).map(Into::into))
    }

    fn duplicate(&mut self, c: &Candidate) -> Result<bool, Error> {
        let mut sql = self.select(&[b"comment_ID"], &self.comments);
        self.equals_number(&mut sql, b"comment_post_ID", c.post_id);
        sql.extend_from_slice(b" AND ");
        self.equals_number(&mut sql, b"comment_parent", c.parent);
        sql.extend_from_slice(b" AND ");
        sql.extend(self.dialect.ident(b"comment_approved"));
        sql.extend_from_slice(b" != 'trash' AND (");
        self.equals_text(&mut sql, b"comment_author", &c.author)?;
        if !c.email.is_empty() {
            sql.extend_from_slice(b" OR ");
            self.equals_text(&mut sql, b"comment_author_email", &c.email)?;
        }
        sql.extend_from_slice(b") AND ");
        self.equals_text(&mut sql, b"comment_content", &c.content)?;
        sql.extend_from_slice(b" LIMIT 1");
        Ok(!self.rows(&Sql::new(sql))?.rows().is_empty())
    }

    fn last_comment_gmt(&mut self, c: &Candidate, since: &str) -> Result<Option<Box<str>>, Error> {
        let mut sql = self.select(&[b"comment_date_gmt"], &self.comments);
        sql.extend(self.dialect.ident(b"comment_date_gmt"));
        sql.extend_from_slice(b" >= ");
        sql.extend(self.dialect.literal(since.as_bytes())?);
        sql.extend_from_slice(b" AND (");
        // Logged in: same user or same IP; anonymous: same IP or same email.
        if c.user_id == 0 {
            self.equals_text(&mut sql, b"comment_author_IP", &c.ip)?;
            sql.extend_from_slice(b" OR ");
            self.equals_text(&mut sql, b"comment_author_email", &c.email)?;
        } else {
            self.equals_number(&mut sql, b"user_id", c.user_id);
            sql.extend_from_slice(b" OR ");
            self.equals_text(&mut sql, b"comment_author_IP", &c.ip)?;
        }
        sql.extend_from_slice(b") ORDER BY ");
        sql.extend(self.dialect.ident(b"comment_date_gmt"));
        sql.extend_from_slice(b" DESC LIMIT 1");
        let set = self.rows(&Sql::new(sql))?;
        Ok(first_text(&set).map(Into::into))
    }

    fn previously_approved(&mut self, author: &str, email: &str) -> Result<bool, Error> {
        let mut lookup = self.select(&[b"ID"], &self.users);
        self.equals_text(&mut lookup, b"user_email", email)?;
        lookup.extend_from_slice(b" LIMIT 1");
        let user = first_text(&self.rows(&Sql::new(lookup))?)
            .and_then(|text| text.parse::<u64>().ok())
            .filter(|&id| id > 0);
        let mut sql = self.select(&[b"comment_approved"], &self.comments);
        match user {
            Some(id) => self.equals_number(&mut sql, b"user_id", id),
            None => {
                self.equals_text(&mut sql, b"comment_author", author)?;
                sql.extend_from_slice(b" AND ");
                self.equals_text(&mut sql, b"comment_author_email", email)?;
            }
        }
        sql.extend_from_slice(b" AND ");
        sql.extend(self.dialect.ident(b"comment_approved"));
        sql.extend_from_slice(b" = '1' LIMIT 1");
        Ok(!self.rows(&Sql::new(sql))?.rows().is_empty())
    }
}

fn join(sql: &mut Vec<u8>, parts: impl IntoIterator<Item = Vec<u8>>) {
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            sql.extend_from_slice(b", ");
        }
        sql.extend(part);
    }
}

fn first_text(set: &ResultSet) -> Option<String> {
    match set.rows().first()?.cells().first()? {
        Cell::Bytes(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        Cell::Null => None,
    }
}
