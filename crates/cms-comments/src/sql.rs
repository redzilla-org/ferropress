//! Statements the reference issues for comment reads and writes.

use cms_db::{Dialect, LiteralError, Sql, Statement, TableName};

use crate::CommentId;
use crate::draft::{Draft, Field};

/// The comments primary key, returned by `PostgreSQL` inserts.
const ID: &[u8] = b"comment_ID";

/// One row: `SELECT * … WHERE comment_ID = {id} LIMIT 1`.
pub(crate) fn select_row(
    dialect: Dialect,
    table: &TableName,
    id: CommentId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT * FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare(ID)
        .raw(" = ")
        .number(id.as_u64())
        .raw(" LIMIT 1");
    sql.finish()
}

/// Compact insert of the caller-supplied cells.
pub(crate) fn insert_row(
    dialect: Dialect,
    table: &TableName,
    draft: &Draft,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("INSERT INTO ").ident(table.as_bytes()).raw(" (");
    for (index, field) in Field::INSERT.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.ident(field.column());
    }
    sql.raw(") VALUES (");
    for (index, field) in Field::INSERT.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        format_cell(&mut sql, *field, draft.get(*field));
    }
    sql.raw(")").returning(ID);
    sql.finish()
}

/// Compact update of the caller-supplied cells for `id`.
pub(crate) fn update_row(
    dialect: Dialect,
    table: &TableName,
    id: CommentId,
    draft: &Draft,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("UPDATE ").ident(table.as_bytes()).raw(" SET ");
    for (index, field) in Field::UPDATE.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.ident(field.column()).raw(" = ");
        format_cell(&mut sql, *field, draft.get(*field));
    }
    sql.raw(" WHERE ")
        .ident(ID)
        .raw(" = ")
        .number(id.as_u64());
    sql.finish()
}

/// Remove one id.
pub(crate) fn delete_row(
    dialect: Dialect,
    table: &TableName,
    id: CommentId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("DELETE FROM ")
        .ident(table.as_bytes())
        .raw(" WHERE ")
        .ident(ID)
        .raw(" = ")
        .number(id.as_u64());
    sql.finish()
}

// Integer columns go unquoted, as the reference's %d placeholders render them.
fn format_cell(sql: &mut Statement, field: Field, bytes: &[u8]) {
    if field.is_integer() {
        sql.number(format_int(bytes));
    } else {
        sql.literal(bytes);
    }
}

fn format_int(bytes: &[u8]) -> i64 {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{delete_row, format_int, insert_row, select_row, update_row};
    use crate::CommentId;
    use crate::draft::{Draft, Field};
    use cms_db::{Dialect, LiteralError, Sql, TablePrefix, TableStem};

    const MY: Dialect = Dialect::MySql;
    const PG: Dialect = Dialect::Postgres;

    fn bytes(sql: Result<Sql, LiteralError>) -> Vec<u8> {
        sql.map(|sql| sql.as_bytes().to_vec()).unwrap_or_default()
    }

    fn comments_table() -> Option<(cms_db::TableName, CommentId)> {
        let prefix = TablePrefix::new("wp_");
        let stem = TableStem::new("comments");
        let id = CommentId::new(12);
        if let (Ok(prefix), Ok(stem), Ok(id)) = (prefix, stem, id) {
            return Some((prefix.table(&stem), id));
        }
        None
    }

    fn sample_draft() -> Draft {
        let mut draft = Draft::new();
        draft
            .set(Field::Post, b"3")
            .set(Field::AuthorIp, b"203.0.113.4")
            .set(Field::Author, b"Ada")
            .set(Field::AuthorEmail, b"ada@example.test")
            .set(Field::AuthorUrl, b"https://ada.test")
            .set(Field::Date, b"2026-01-01 00:00:00")
            .set(Field::DateGmt, b"2026-01-01 00:00:00")
            .set(Field::Content, b"hello")
            .set(Field::Karma, b"0")
            .set(Field::Approved, b"1")
            .set(Field::Agent, b"agent")
            .set(Field::TypeName, b"comment")
            .set(Field::Parent, b"0")
            .set(Field::UserId, b"2");
        draft
    }

    #[test]
    fn statements_match_the_lab_prefix() {
        let opened = comments_table();
        assert!(opened.is_some());
        if let Some((table, id)) = opened {
            assert_eq!(
                bytes(select_row(MY, &table, id)),
                b"SELECT * FROM wp_comments WHERE comment_ID = 12 LIMIT 1"
            );
            let draft = sample_draft();
            assert_eq!(
                bytes(insert_row(MY, &table, &draft)),
                b"INSERT INTO `wp_comments` (`comment_post_ID`, `comment_author_IP`, `comment_author`, `comment_author_email`, `comment_author_url`, `comment_date`, `comment_date_gmt`, `comment_content`, `comment_karma`, `comment_approved`, `comment_agent`, `comment_type`, `comment_parent`, `user_id`) VALUES (3, '203.0.113.4', 'Ada', 'ada@example.test', 'https://ada.test', '2026-01-01 00:00:00', '2026-01-01 00:00:00', 'hello', 0, '1', 'agent', 'comment', 0, 2)"
            );
            assert_eq!(
                bytes(update_row(MY, &table, id, &draft)),
                b"UPDATE `wp_comments` SET `comment_post_ID` = 3, `comment_author` = 'Ada', `comment_author_email` = 'ada@example.test', `comment_author_url` = 'https://ada.test', `comment_author_IP` = '203.0.113.4', `comment_date` = '2026-01-01 00:00:00', `comment_date_gmt` = '2026-01-01 00:00:00', `comment_content` = 'hello', `comment_karma` = 0, `comment_approved` = '1', `comment_agent` = 'agent', `comment_type` = 'comment', `comment_parent` = 0, `user_id` = 2 WHERE `comment_ID` = 12"
            );
            assert_eq!(
                bytes(delete_row(MY, &table, id)),
                b"DELETE FROM `wp_comments` WHERE `comment_ID` = 12"
            );
        }
    }

    #[test]
    fn postgres_statements_quote_every_name_and_return_the_id() {
        let opened = comments_table();
        assert!(opened.is_some());
        if let Some((table, id)) = opened {
            assert_eq!(
                bytes(select_row(PG, &table, id)),
                b"SELECT * FROM \"wp_comments\" WHERE \"comment_ID\" = 12 LIMIT 1"
            );
            let draft = sample_draft();
            assert_eq!(
                bytes(insert_row(PG, &table, &draft)),
                b"INSERT INTO \"wp_comments\" (\"comment_post_ID\", \"comment_author_IP\", \"comment_author\", \"comment_author_email\", \"comment_author_url\", \"comment_date\", \"comment_date_gmt\", \"comment_content\", \"comment_karma\", \"comment_approved\", \"comment_agent\", \"comment_type\", \"comment_parent\", \"user_id\") VALUES (3, '203.0.113.4', 'Ada', 'ada@example.test', 'https://ada.test', '2026-01-01 00:00:00', '2026-01-01 00:00:00', 'hello', 0, '1', 'agent', 'comment', 0, 2) RETURNING \"comment_ID\""
            );
            let update = bytes(update_row(PG, &table, id, &draft));
            assert!(update.starts_with(b"UPDATE \"wp_comments\" SET \"comment_post_ID\" = 3, "));
            assert!(update.ends_with(b"\"user_id\" = 2 WHERE \"comment_ID\" = 12"));
            assert_eq!(
                bytes(delete_row(PG, &table, id)),
                b"DELETE FROM \"wp_comments\" WHERE \"comment_ID\" = 12"
            );
            let mut bad = Draft::new();
            bad.set(Field::Content, b"a\0b");
            assert_eq!(insert_row(PG, &table, &bad), Err(LiteralError::Nul));
        }
    }

    #[test]
    fn integers_are_unquoted_and_junk_is_zero() {
        assert_eq!(format_int(b"7"), 7);
        assert_eq!(format_int(b"01"), 1);
        assert_eq!(format_int(b"-2"), -2);
        assert_eq!(format_int(b""), 0);
        assert_eq!(format_int(b"x"), 0);
        assert_eq!(format_int(&[0xff]), 0);
        let opened = comments_table();
        assert!(opened.is_some());
        if let Some((table, _)) = opened {
            let mut draft = Draft::new();
            draft
                .set(Field::Post, b"nope")
                .set(Field::Author, b"a'b")
                .set(Field::Karma, b"3")
                .set(Field::Parent, b"4")
                .set(Field::UserId, b"5");
            let sql = bytes(insert_row(MY, &table, &draft));
            assert!(sql.starts_with(b"INSERT INTO `wp_comments`"));
            assert!(sql.windows(11).any(|window| window == b"VALUES (0, "));
            assert!(sql.windows(6).any(|window| window == br"'a\'b'"));
            assert!(sql.windows(7).any(|window| window == b", 3, ''"));
            assert!(sql.ends_with(b", 4, 5)"));
        }
    }

    proptest::proptest! {
        #[test]
        fn ids_are_decimal_and_unquoted(id in 1_u64..=10_000) {
            let prefix = TablePrefix::new("wp_");
            let stem = TableStem::new("comments");
            let comment = CommentId::new(id);
            proptest::prop_assert!(prefix.is_ok() && stem.is_ok() && comment.is_ok());
            if let (Ok(prefix), Ok(stem), Ok(comment)) = (prefix, stem, comment) {
                let table = prefix.table(&stem);
                let select = bytes(select_row(MY, &table, comment));
                let expected = format!("SELECT * FROM wp_comments WHERE comment_ID = {id} LIMIT 1");
                proptest::prop_assert_eq!(select, expected.as_bytes());
                let delete = bytes(delete_row(MY, &table, comment));
                let expected = format!("DELETE FROM `wp_comments` WHERE `comment_ID` = {id}");
                proptest::prop_assert_eq!(delete, expected.as_bytes());
            }
        }
    }
}
