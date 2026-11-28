//! Statements of the attachment transaction, spelled for the store's dialect.

use cms_content::{Draft, Field};
use cms_db::{Dialect, Sql, TableName};

use crate::Error;

/// The tables one site's attachment transaction touches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tables {
    pub(crate) posts: TableName,
    pub(crate) postmeta: TableName,
    pub(crate) outbox: TableName,
}

/// Builds statements in one dialect.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Statements<'a> {
    pub(crate) dialect: Dialect,
    pub(crate) tables: &'a Tables,
}

impl Statements<'_> {
    /// Whether `slug` is held by any post; the reference's attachment slug check for a new row.
    pub(crate) fn slug_taken(&self, slug: &str) -> Result<Sql, Error> {
        let d = self.dialect;
        let mut sql = b"SELECT ".to_vec();
        sql.extend(d.ident(b"post_name"));
        sql.extend_from_slice(b" FROM ");
        sql.extend(d.ident(self.tables.posts.as_bytes()));
        sql.extend_from_slice(b" WHERE ");
        sql.extend(d.ident(b"post_name"));
        sql.extend_from_slice(b" = ");
        sql.extend(literal(d, slug.as_bytes())?);
        sql.extend_from_slice(b" AND ");
        sql.extend(d.ident(b"ID"));
        sql.extend_from_slice(b" != 0 LIMIT 1");
        Ok(Sql::new(sql))
    }

    /// Compact insert of the attachment row; `PostgreSQL` returns the new id.
    pub(crate) fn insert_post(&self, draft: &Draft) -> Result<Sql, Error> {
        let d = self.dialect;
        let mut sql = b"INSERT INTO ".to_vec();
        sql.extend(d.ident(self.tables.posts.as_bytes()));
        sql.extend_from_slice(b" (");
        for (index, field) in Field::ALL.iter().enumerate() {
            if index > 0 {
                sql.extend_from_slice(b", ");
            }
            sql.extend(d.ident(field.column()));
        }
        sql.extend_from_slice(b") VALUES (");
        for (index, field) in Field::ALL.iter().enumerate() {
            if index > 0 {
                sql.extend_from_slice(b", ");
            }
            let cell = draft.get(*field);
            if field.is_integer() {
                sql.extend(integer(cell));
            } else {
                sql.extend(literal(d, cell)?);
            }
        }
        sql.push(b')');
        if d == Dialect::Postgres {
            sql.extend_from_slice(b" RETURNING ");
            sql.extend(d.ident(b"ID"));
        }
        Ok(Sql::new(sql))
    }

    /// One postmeta row.
    pub(crate) fn insert_meta(&self, post_id: u64, key: &str, value: &[u8]) -> Result<Sql, Error> {
        let d = self.dialect;
        let mut sql = b"INSERT INTO ".to_vec();
        sql.extend(d.ident(self.tables.postmeta.as_bytes()));
        sql.extend_from_slice(b" (");
        sql.extend(d.ident(b"post_id"));
        sql.extend_from_slice(b", ");
        sql.extend(d.ident(b"meta_key"));
        sql.extend_from_slice(b", ");
        sql.extend(d.ident(b"meta_value"));
        sql.extend_from_slice(b") VALUES (");
        sql.extend_from_slice(post_id.to_string().as_bytes());
        sql.extend_from_slice(b", ");
        sql.extend(literal(d, key.as_bytes())?);
        sql.extend_from_slice(b", ");
        sql.extend(literal(d, value)?);
        sql.push(b')');
        Ok(Sql::new(sql))
    }

    /// The outbox row announcing the new attachment to the projector.
    pub(crate) fn insert_outbox(&self, post_id: u64) -> Sql {
        let d = self.dialect;
        let mut sql = b"INSERT INTO ".to_vec();
        sql.extend(d.ident(self.tables.outbox.as_bytes()));
        sql.extend_from_slice(b" (");
        for (index, column) in ["aggregate", "aggregate_id", "change", "payload"]
            .iter()
            .enumerate()
        {
            if index > 0 {
                sql.extend_from_slice(b", ");
            }
            sql.extend(d.ident(column.as_bytes()));
        }
        sql.extend_from_slice(b") VALUES ('post', ");
        sql.extend_from_slice(post_id.to_string().as_bytes());
        sql.extend_from_slice(br#", 'insert', '{"post_type":"attachment"}')"#);
        Sql::new(sql)
    }
}

fn literal(dialect: Dialect, bytes: &[u8]) -> Result<Vec<u8>, Error> {
    dialect.literal(bytes).map_err(|_unquotable| Error::Literal)
}

// Integer columns hold decimal text the caller built; anything else is written as 0.
fn integer(bytes: &[u8]) -> Vec<u8> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .map_or_else(|| b"0".to_vec(), |number| number.to_string().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::{Statements, Tables, integer};
    use crate::Error;
    use cms_content::{Draft, Field};
    use cms_db::{Dialect, TablePrefix, TableStem};

    fn tables() -> Option<Tables> {
        let prefix = TablePrefix::new("fp_").ok()?;
        let table = |stem: &str| TableStem::new(stem).ok().map(|s| prefix.table(&s));
        Some(Tables {
            posts: table("posts")?,
            postmeta: table("postmeta")?,
            outbox: table("outbox")?,
        })
    }

    fn text(sql: Result<cms_db::Sql, Error>) -> String {
        sql.map(|sql| String::from_utf8_lossy(sql.as_bytes()).into_owned())
            .unwrap_or_default()
    }

    #[test]
    fn postgres_statements() {
        let tables = tables();
        assert!(tables.is_some());
        let Some(tables) = tables else { return };
        let pg = Statements {
            dialect: Dialect::Postgres,
            tables: &tables,
        };
        assert_eq!(
            text(pg.slug_taken("it's")),
            r#"SELECT "post_name" FROM "fp_posts" WHERE "post_name" = 'it''s' AND "ID" != 0 LIMIT 1"#
        );
        let mut draft = Draft::new();
        draft.set(Field::Author, b"7").set(Field::Title, b"a");
        let insert = text(pg.insert_post(&draft));
        assert!(insert.starts_with(r#"INSERT INTO "fp_posts" ("post_author", "post_date""#));
        assert!(insert.contains(r"VALUES (7, '', '', '', '', 'a', "));
        assert!(insert.ends_with(r#", 0, 0, '', '') RETURNING "ID""#));
        assert_eq!(
            text(pg.insert_meta(12, "_wp_attached_file", b"2026/11/a.jpg")),
            r#"INSERT INTO "fp_postmeta" ("post_id", "meta_key", "meta_value") VALUES (12, '_wp_attached_file', '2026/11/a.jpg')"#
        );
        assert_eq!(
            text(Ok(pg.insert_outbox(12))),
            r#"INSERT INTO "fp_outbox" ("aggregate", "aggregate_id", "change", "payload") VALUES ('post', 12, 'insert', '{"post_type":"attachment"}')"#
        );
        assert_eq!(pg.insert_meta(1, "k", b"a\0b"), Err(Error::Literal));
        draft.set(Field::Title, b"\0");
        assert_eq!(pg.insert_post(&draft), Err(Error::Literal));
        assert_eq!(pg.slug_taken("\0"), Err(Error::Literal));
    }

    #[test]
    fn mysql_statements_use_insert_id() {
        let tables = tables();
        assert!(tables.is_some());
        let Some(tables) = tables else { return };
        let my = Statements {
            dialect: Dialect::MySql,
            tables: &tables,
        };
        let insert = text(my.insert_post(&Draft::new()));
        assert!(insert.starts_with("INSERT INTO `fp_posts` (`post_author`"));
        assert!(insert.ends_with(", 0, 0, '', '')"));
        assert!(!insert.contains("RETURNING"));
    }

    #[test]
    fn integers_fall_back_to_zero() {
        assert_eq!(integer(b"42"), b"42");
        assert_eq!(integer(b"x"), b"0");
        assert_eq!(integer(&[0xff]), b"0");
    }
}
