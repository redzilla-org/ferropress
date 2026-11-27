//! The statements the import issues, spelled for the store's [`Dialect`].
//!
//! The domain crates' writers assign ids through the server's auto-increment and have no
//! natural-key lookups, while the import must keep source ids. These shapes are therefore built
//! here from the domain crates' column inventories; identifiers are always quoted, so result
//! columns keep the reference names in both dialects.

use cms_db::{Dialect, LiteralError, NameError, Sql, TableName, TablePrefix, TableStem};

/// A cell value: an unquoted integer or a quoted literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Value<'a> {
    /// Written as a decimal integer.
    Int(u64),
    /// Written as a string literal.
    Text(&'a [u8]),
}

/// The tables an import writes, by stem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Table {
    Users,
    Posts,
    PostMeta,
    Terms,
    Taxonomy,
    TermMeta,
    Comments,
    CommentMeta,
    Links,
}

impl Table {
    /// Every table, in the order their identities are advanced.
    pub(crate) const ALL: [Self; 9] = [
        Self::Users,
        Self::Terms,
        Self::Taxonomy,
        Self::TermMeta,
        Self::Posts,
        Self::PostMeta,
        Self::Comments,
        Self::CommentMeta,
        Self::Links,
    ];

    /// The inventory stem.
    pub(crate) const fn stem(self) -> &'static str {
        match self {
            Self::Users => "users",
            Self::Posts => "posts",
            Self::PostMeta => "postmeta",
            Self::Terms => "terms",
            Self::Taxonomy => "term_taxonomy",
            Self::TermMeta => "termmeta",
            Self::Comments => "comments",
            Self::CommentMeta => "commentmeta",
            Self::Links => "term_relationships",
        }
    }

    /// The identity column, or `None` for the relationships table, which has a composite key.
    pub(crate) const fn id_column(self) -> Option<&'static str> {
        match self {
            Self::Users | Self::Posts => Some("ID"),
            Self::PostMeta | Self::TermMeta | Self::CommentMeta => Some("meta_id"),
            Self::Terms => Some("term_id"),
            Self::Taxonomy => Some("term_taxonomy_id"),
            Self::Comments => Some("comment_ID"),
            Self::Links => None,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// Statement builder for one site's tables in one dialect.
#[derive(Debug)]
pub(crate) struct Statements {
    dialect: Dialect,
    names: Vec<TableName>,
}

impl Statements {
    /// Statements for the tables under `prefix`.
    pub(crate) fn new(dialect: Dialect, prefix: &TablePrefix) -> Result<Self, NameError> {
        let mut names = Vec::with_capacity(Table::ALL.len());
        // Declaration order of `Table` is the index order.
        for table in [
            Table::Users,
            Table::Posts,
            Table::PostMeta,
            Table::Terms,
            Table::Taxonomy,
            Table::TermMeta,
            Table::Comments,
            Table::CommentMeta,
            Table::Links,
        ] {
            names.push(prefix.table(&TableStem::new(table.stem())?));
        }
        Ok(Self { dialect, names })
    }

    fn table(&self, table: Table) -> Vec<u8> {
        self.dialect.ident(self.names[table.index()].as_bytes())
    }

    fn value(&self, value: Value<'_>) -> Result<Vec<u8>, LiteralError> {
        match value {
            Value::Int(number) => Ok(number.to_string().into_bytes()),
            Value::Text(bytes) => self.dialect.literal(bytes),
        }
    }

    /// `SELECT out FROM t WHERE c = v AND …`, limited to one row when `one`.
    pub(crate) fn select(
        &self,
        table: Table,
        out: &[u8],
        conditions: &[(&[u8], Value<'_>)],
        one: bool,
    ) -> Result<Sql, LiteralError> {
        let mut sql = Vec::from(&b"SELECT "[..]);
        sql.extend(self.dialect.ident(out));
        sql.extend_from_slice(b" FROM ");
        sql.extend(self.table(table));
        for (index, (column, value)) in conditions.iter().enumerate() {
            sql.extend_from_slice(if index == 0 { b" WHERE " } else { b" AND " });
            sql.extend(self.dialect.ident(column));
            sql.extend_from_slice(b" = ");
            sql.extend(self.value(*value)?);
        }
        if one {
            sql.extend_from_slice(b" LIMIT 1");
        }
        Ok(Sql::new(sql))
    }

    /// `SELECT MAX(id) FROM t`, or `None` for a table without an identity.
    pub(crate) fn max(&self, table: Table) -> Option<Sql> {
        let column = table.id_column()?;
        let mut sql = Vec::from(&b"SELECT MAX("[..]);
        sql.extend(self.dialect.ident(column.as_bytes()));
        sql.extend_from_slice(b") FROM ");
        sql.extend(self.table(table));
        Some(Sql::new(sql))
    }

    /// `INSERT INTO t (c, …) VALUES (v, …)`.
    pub(crate) fn insert(
        &self,
        table: Table,
        cells: &[(&[u8], Value<'_>)],
    ) -> Result<Sql, LiteralError> {
        let mut sql = Vec::from(&b"INSERT INTO "[..]);
        sql.extend(self.table(table));
        sql.extend_from_slice(b" (");
        for (index, (column, _)) in cells.iter().enumerate() {
            if index > 0 {
                sql.extend_from_slice(b", ");
            }
            sql.extend(self.dialect.ident(column));
        }
        sql.extend_from_slice(b") VALUES (");
        for (index, (_, value)) in cells.iter().enumerate() {
            if index > 0 {
                sql.extend_from_slice(b", ");
            }
            sql.extend(self.value(*value)?);
        }
        sql.push(b')');
        Ok(Sql::new(sql))
    }

    /// `UPDATE t SET c = v, … WHERE key = id`.
    pub(crate) fn update(
        &self,
        table: Table,
        cells: &[(&[u8], Value<'_>)],
        key: &[u8],
        id: u64,
    ) -> Result<Sql, LiteralError> {
        let mut sql = Vec::from(&b"UPDATE "[..]);
        sql.extend(self.table(table));
        sql.extend_from_slice(b" SET ");
        for (index, (column, value)) in cells.iter().enumerate() {
            if index > 0 {
                sql.extend_from_slice(b", ");
            }
            sql.extend(self.dialect.ident(column));
            sql.extend_from_slice(b" = ");
            sql.extend(self.value(*value)?);
        }
        sql.extend_from_slice(b" WHERE ");
        sql.extend(self.dialect.ident(key));
        sql.extend_from_slice(b" = ");
        sql.extend_from_slice(id.to_string().as_bytes());
        Ok(Sql::new(sql))
    }

    /// Opens a transaction; both servers accept `BEGIN`.
    pub(crate) fn begin() -> Sql {
        Sql::from("BEGIN")
    }

    /// Commits the open transaction.
    pub(crate) fn commit() -> Sql {
        Sql::from("COMMIT")
    }

    /// Moves `table`'s identity so the next generated id is `max + 1`, or `None` for a table
    /// without an identity.
    ///
    /// `PostgreSQL` takes `setval` on the identity's sequence, a plain query that DSQL can run
    /// outside DDL limits; `MySQL` takes `AUTO_INCREMENT`.
    pub(crate) fn advance(&self, table: Table, max: u64) -> Result<Option<Sql>, LiteralError> {
        let Some(column) = table.id_column() else {
            return Ok(None);
        };
        let mut sql = Vec::new();
        match self.dialect {
            Dialect::Postgres => {
                sql.extend_from_slice(b"SELECT setval(pg_get_serial_sequence(");
                sql.extend(self.dialect.literal(&self.table(table))?);
                sql.extend_from_slice(b", ");
                sql.extend(self.dialect.literal(column.as_bytes())?);
                sql.extend_from_slice(b"), ");
                sql.extend_from_slice(max.to_string().as_bytes());
                sql.push(b')');
            }
            Dialect::MySql => {
                sql.extend_from_slice(b"ALTER TABLE ");
                sql.extend(self.table(table));
                sql.extend_from_slice(b" AUTO_INCREMENT = ");
                sql.extend_from_slice(max.saturating_add(1).to_string().as_bytes());
            }
        }
        Ok(Some(Sql::new(sql)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Statements, Table, Value};
    use cms_db::{Dialect, LiteralError, TablePrefix};

    fn statements(dialect: Dialect) -> Option<Statements> {
        let prefix = TablePrefix::new("fp_").ok()?;
        Statements::new(dialect, &prefix).ok()
    }

    fn text(sql: &cms_db::Sql) -> String {
        String::from_utf8_lossy(sql.as_bytes()).into_owned()
    }

    fn c(name: &str) -> &[u8] {
        name.as_bytes()
    }

    #[test]
    fn postgres_statements_quote_reference_names() {
        let built = statements(Dialect::Postgres);
        assert!(built.is_some());
        let Some(sql) = built else { return };
        let select = sql.select(
            Table::Posts,
            c("ID"),
            &[
                (c("guid"), Value::Text(b"a'b")),
                (c("post_type"), Value::Text(b"post")),
            ],
            true,
        );
        assert_eq!(
            select.as_ref().map(text).ok().as_deref(),
            Some("SELECT \"ID\" FROM \"fp_posts\" WHERE \"guid\" = 'a''b' AND \"post_type\" = 'post' LIMIT 1")
        );
        let all = sql.select(
            Table::Terms,
            c("term_id"),
            &[(c("slug"), Value::Text(b"x"))],
            false,
        );
        assert_eq!(
            all.as_ref().map(text).ok().as_deref(),
            Some("SELECT \"term_id\" FROM \"fp_terms\" WHERE \"slug\" = 'x'")
        );
        assert_eq!(
            sql.max(Table::Comments).as_ref().map(text).as_deref(),
            Some("SELECT MAX(\"comment_ID\") FROM \"fp_comments\"")
        );
        assert!(sql.max(Table::Links).is_none());
        let insert = sql.insert(
            Table::Links,
            &[
                (c("object_id"), Value::Int(3)),
                (c("term_taxonomy_id"), Value::Int(4)),
            ],
        );
        assert_eq!(
            insert.as_ref().map(text).ok().as_deref(),
            Some("INSERT INTO \"fp_term_relationships\" (\"object_id\", \"term_taxonomy_id\") VALUES (3, 4)")
        );
        let update = sql.update(
            Table::Taxonomy,
            &[
                (c("parent"), Value::Int(2)),
                (c("description"), Value::Text(b"d")),
            ],
            c("term_taxonomy_id"),
            9,
        );
        assert_eq!(
            update.as_ref().map(text).ok().as_deref(),
            Some("UPDATE \"fp_term_taxonomy\" SET \"parent\" = 2, \"description\" = 'd' WHERE \"term_taxonomy_id\" = 9")
        );
        let advance = sql.advance(Table::Posts, 41);
        assert_eq!(
            advance.ok().flatten().as_ref().map(text).as_deref(),
            Some("SELECT setval(pg_get_serial_sequence('\"fp_posts\"', 'ID'), 41)")
        );
        assert!(sql.advance(Table::Links, 1).is_ok_and(|none| none.is_none()));
        assert!(matches!(
            sql.insert(Table::Posts, &[(c("post_title"), Value::Text(b"a\0b"))]),
            Err(LiteralError::Nul)
        ));
        let nul = [(c("guid"), Value::Text(b"\0"))];
        assert!(sql.select(Table::Posts, c("ID"), &nul, true).is_err());
        assert!(sql.update(Table::Posts, &nul, c("ID"), 1).is_err());
    }

    #[test]
    fn mysql_statements_use_backticks_and_auto_increment() {
        let built = statements(Dialect::MySql);
        assert!(built.is_some());
        let Some(sql) = built else { return };
        let insert = sql.insert(
            Table::Users,
            &[
                (c("ID"), Value::Int(1)),
                (c("user_login"), Value::Text(b"o'k")),
            ],
        );
        assert_eq!(
            insert.as_ref().map(text).ok().as_deref(),
            Some("INSERT INTO `fp_users` (`ID`, `user_login`) VALUES (1, 'o\\'k')")
        );
        let advance = sql.advance(Table::Users, 41);
        assert_eq!(
            advance.ok().flatten().as_ref().map(text).as_deref(),
            Some("ALTER TABLE `fp_users` AUTO_INCREMENT = 42")
        );
        assert_eq!(text(&Statements::begin()), "BEGIN");
        assert_eq!(text(&Statements::commit()), "COMMIT");
    }

    #[test]
    fn every_table_has_a_stem_and_identity_except_links() {
        for table in Table::ALL {
            assert!(!table.stem().is_empty());
            assert_eq!(table.id_column().is_none(), table == Table::Links);
        }
        let prefix = TablePrefix::new("a");
        assert!(prefix.is_ok_and(|prefix| Statements::new(Dialect::MySql, &prefix).is_ok()));
    }
}
