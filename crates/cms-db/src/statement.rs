//! One statement assembled in a chosen [`Dialect`].
//!
//! Every domain crate's statement builders write through this type, so quoting, case folding and
//! insert-id spelling live in one place and the `MySQL` bytes stay what the reference sends.

use std::fmt::Display;

use crate::{Dialect, LiteralError, ResultSet, Sql};

/// A statement under construction. The first literal that cannot be written is kept and returned
/// by [`Self::finish`], so builders chain without checking every step.
#[derive(Debug)]
pub struct Statement {
    dialect: Dialect,
    bytes: Vec<u8>,
    error: Option<LiteralError>,
}

impl Statement {
    /// An empty statement in `dialect`.
    #[must_use]
    pub const fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            bytes: Vec::new(),
            error: None,
        }
    }

    /// The dialect this statement is spelled in, for clauses that differ by server.
    #[must_use]
    pub const fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Appends keyword or punctuation text exactly as given.
    pub fn raw(&mut self, text: impl AsRef<[u8]>) -> &mut Self {
        self.bytes.extend_from_slice(text.as_ref());
        self
    }

    /// Appends an identifier the reference quotes (`` `name` `` or `"name"`).
    pub fn ident(&mut self, name: impl AsRef<[u8]>) -> &mut Self {
        self.bytes.extend(self.dialect.ident(name.as_ref()));
        self
    }

    /// Appends an identifier the reference leaves bare: bare on `MySql`, quoted on `Postgres` so
    /// mixed-case names such as `ID` are not folded to lower case.
    pub fn bare(&mut self, name: impl AsRef<[u8]>) -> &mut Self {
        match self.dialect {
            Dialect::MySql => self.bytes.extend_from_slice(name.as_ref()),
            Dialect::Postgres => self.bytes.extend(self.dialect.ident(name.as_ref())),
        }
        self
    }

    /// Appends a string literal; a literal the dialect cannot hold is reported by [`Self::finish`].
    pub fn literal(&mut self, bytes: impl AsRef<[u8]>) -> &mut Self {
        match self.dialect.literal(bytes.as_ref()) {
            Ok(quoted) => self.bytes.extend(quoted),
            // Keep the first failure; the statement is unusable either way.
            Err(error) => {
                self.error.get_or_insert(error);
            }
        }
        self
    }

    /// Appends a number in decimal.
    pub fn number(&mut self, value: impl Display) -> &mut Self {
        self.bytes.extend_from_slice(value.to_string().as_bytes());
        self
    }

    /// Appends what `part` writes, wrapped in `lower(…)` on `Postgres`.
    ///
    /// `MySQL` compares reference text columns case-insensitively through their collation;
    /// `PostgreSQL` on DSQL has only the `C` collation, so both sides are folded explicitly and an
    /// expression index on `lower(col)` serves the lookup.
    pub fn case_fold(&mut self, part: impl FnOnce(&mut Self) -> &mut Self) -> &mut Self {
        match self.dialect {
            Dialect::MySql => {
                part(self);
            }
            Dialect::Postgres => {
                self.bytes.extend_from_slice(b"lower(");
                part(self);
                self.bytes.push(b')');
            }
        }
        self
    }

    /// Ends an `INSERT` so the new `column` value comes back: nothing on `MySql` (the server
    /// reports the insert id), ` RETURNING "column"` on `Postgres`. Adapters turn the returned
    /// row into an insert id with [`crate::Reply::returned`].
    pub fn returning(&mut self, column: impl AsRef<[u8]>) -> &mut Self {
        if self.dialect == Dialect::Postgres {
            self.bytes.extend_from_slice(b" RETURNING ");
            self.bytes.extend(self.dialect.ident(column.as_ref()));
        }
        self
    }

    /// The finished statement.
    ///
    /// # Errors
    ///
    /// The first [`LiteralError`] a [`Self::literal`] call hit.
    pub fn finish(self) -> Result<Sql, LiteralError> {
        match self.error {
            Some(error) => Err(error),
            None => Ok(Sql::new(self.bytes)),
        }
    }
}

/// The id in the first cell of a `RETURNING` result, or `0` when there is none or it is not a
/// positive decimal (the same "no id" value `MySQL` reports).
pub(crate) fn returned_id(set: &ResultSet) -> u64 {
    let Some(crate::Cell::Bytes(bytes)) = set.rows().first().and_then(|row| row.cells().first())
    else {
        return 0;
    };
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Statement, returned_id};
    use crate::{Cell, Column, Dialect, LiteralError, ResultSet, Row};

    fn render(statement: Statement) -> Vec<u8> {
        statement
            .finish()
            .map(|sql| sql.as_bytes().to_vec())
            .unwrap_or_default()
    }

    #[test]
    fn mysql_keeps_bare_names_bare_and_folds_nothing() {
        let mut sql = Statement::new(Dialect::MySql);
        assert_eq!(sql.dialect(), Dialect::MySql);
        sql.raw("SELECT * FROM ")
            .bare("wp_posts")
            .raw(" WHERE ")
            .case_fold(|s| s.bare("post_name"))
            .raw(" = ")
            .case_fold(|s| s.literal("a'b"))
            .raw(" AND ")
            .ident("ID")
            .raw(" = ")
            .number(12_u64)
            .returning("ID");
        assert_eq!(
            render(sql),
            br"SELECT * FROM wp_posts WHERE post_name = 'a\'b' AND `ID` = 12"
        );
    }

    #[test]
    fn postgres_quotes_every_name_folds_lookups_and_returns_the_id() {
        let mut sql = Statement::new(Dialect::Postgres);
        assert_eq!(sql.dialect(), Dialect::Postgres);
        sql.raw("SELECT * FROM ")
            .bare("wp_posts")
            .raw(" WHERE ")
            .case_fold(|s| s.bare("post_name"))
            .raw(" = ")
            .case_fold(|s| s.literal("a'b"))
            .raw(" AND ")
            .ident("ID")
            .raw(" = ")
            .number(12_u64)
            .returning("ID");
        assert_eq!(
            render(sql),
            b"SELECT * FROM \"wp_posts\" WHERE lower(\"post_name\") = lower('a''b') AND \"ID\" = 12 RETURNING \"ID\""
        );
    }

    #[test]
    fn the_first_unwritable_literal_fails_the_statement() {
        let mut sql = Statement::new(Dialect::Postgres);
        sql.literal("a\0b").literal("ok").literal("\0");
        assert_eq!(sql.finish(), Err(LiteralError::Nul));
        let mut sql = Statement::new(Dialect::MySql);
        sql.literal("a\0b");
        assert_eq!(render(sql), br"'a\0b'");
    }

    #[test]
    fn returned_ids_come_from_the_first_cell() {
        let set = |cells: Vec<Row>| ResultSet::new(vec![Column::new("ID")], cells);
        assert_eq!(
            returned_id(&set(vec![Row::new(vec![Cell::bytes("42")])])),
            42
        );
        assert_eq!(returned_id(&set(vec![])), 0);
        assert_eq!(returned_id(&set(vec![Row::new(vec![Cell::Null])])), 0);
        assert_eq!(returned_id(&set(vec![Row::new(vec![])])), 0);
        assert_eq!(returned_id(&set(vec![Row::new(vec![Cell::bytes("x")])])), 0);
        assert_eq!(
            returned_id(&set(vec![Row::new(vec![Cell::bytes([0xff])])])),
            0
        );
    }
}
