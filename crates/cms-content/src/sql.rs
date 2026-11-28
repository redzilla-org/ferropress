//! Statements the reference issues for post reads and writes.

use cms_db::{Dialect, LiteralError, Sql, Statement, TableName};

use crate::PostId;
use crate::draft::{Draft, Field};

/// The posts primary key, returned by `PostgreSQL` inserts.
const ID: &[u8] = b"ID";

/// Path candidates: `get_page_by_path` IN query.
pub(crate) fn select_path(
    dialect: Dialect,
    table: &TableName,
    names: &[Box<[u8]>],
    types: &[&[u8]],
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("\n\t\tSELECT ")
        .bare(ID)
        .raw(", ")
        .bare("post_name")
        .raw(", ")
        .bare("post_parent")
        .raw(", ")
        .bare("post_type")
        .raw("\n\t\tFROM ")
        .bare(table.as_bytes())
        .raw("\n\t\tWHERE ")
        // Slugs match case-insensitively, as the reference collation does.
        .case_fold(|s| s.bare("post_name"))
        .raw(" IN (");
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            sql.raw(",");
        }
        sql.case_fold(|s| s.literal(name));
    }
    sql.raw(")\n\t\tAND ").bare("post_type").raw(" IN (");
    for (index, kind) in types.iter().enumerate() {
        if index > 0 {
            sql.raw(",");
        }
        sql.literal(kind);
    }
    sql.raw(")\n\t");
    sql.finish()
}

/// One row by slug: `SELECT * … WHERE post_name = {quoted} LIMIT 1`.
pub(crate) fn select_name(
    dialect: Dialect,
    table: &TableName,
    name: &[u8],
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT * FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .case_fold(|s| s.bare("post_name"))
        .raw(" = ")
        .case_fold(|s| s.literal(name))
        .raw(" LIMIT 1");
    sql.finish()
}

/// One row: `SELECT * … WHERE ID = {id} LIMIT 1`.
pub(crate) fn select_row(
    dialect: Dialect,
    table: &TableName,
    id: PostId,
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
        format_cell(&mut sql, *field, draft.get(*field));
    }
    sql.raw(")").returning(ID);
    sql.finish()
}

/// Compact update of the caller-supplied cells for `id`.
pub(crate) fn update_row(
    dialect: Dialect,
    table: &TableName,
    id: PostId,
    draft: &Draft,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("UPDATE ").ident(table.as_bytes()).raw(" SET ");
    for (index, field) in Field::ALL.iter().enumerate() {
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
    id: PostId,
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
    use super::{
        delete_row, format_int, insert_row, select_name, select_path, select_row, update_row,
    };
    use crate::PostId;
    use crate::draft::{Draft, Field};
    use cms_db::{Dialect, LiteralError, Sql, TablePrefix, TableStem};

    const MY: Dialect = Dialect::MySql;
    const PG: Dialect = Dialect::Postgres;

    fn bytes(sql: Result<Sql, LiteralError>) -> Vec<u8> {
        sql.map(|sql| sql.as_bytes().to_vec()).unwrap_or_default()
    }

    fn posts_table() -> Option<(cms_db::TableName, PostId)> {
        let prefix = TablePrefix::new("wp_");
        let stem = TableStem::new("posts");
        let id = PostId::new(12);
        if let (Ok(prefix), Ok(stem), Ok(id)) = (prefix, stem, id) {
            return Some((prefix.table(&stem), id));
        }
        None
    }

    fn sample_draft() -> Draft {
        let mut draft = Draft::new();
        draft
            .set(Field::Author, b"1")
            .set(Field::Date, b"2026-01-01 00:00:00")
            .set(Field::DateGmt, b"2026-01-01 00:00:00")
            .set(Field::Content, b"body")
            .set(Field::Title, b"Hello")
            .set(Field::Status, b"publish")
            .set(Field::TypeName, b"post")
            .set(Field::CommentStatus, b"open")
            .set(Field::PingStatus, b"open")
            .set(Field::Name, b"hello")
            .set(Field::Modified, b"2026-01-01 00:00:00")
            .set(Field::ModifiedGmt, b"2026-01-01 00:00:00");
        draft
    }

    #[test]
    fn statements_match_the_lab_prefix() {
        let opened = posts_table();
        assert!(opened.is_some());
        if let Some((table, id)) = opened {
            assert_eq!(
                bytes(select_row(MY, &table, id)),
                b"SELECT * FROM wp_posts WHERE ID = 12 LIMIT 1"
            );
            assert_eq!(
                bytes(select_path(MY, &table, &[Box::from(*b"about")], &[b"page", b"attachment"])),
                b"\n\t\tSELECT ID, post_name, post_parent, post_type\n\t\tFROM wp_posts\n\t\tWHERE post_name IN ('about')\n\t\tAND post_type IN ('page','attachment')\n\t"
            );
            assert_eq!(
                bytes(select_name(MY, &table, b"hello")),
                b"SELECT * FROM wp_posts WHERE post_name = 'hello' LIMIT 1"
            );
            let draft = sample_draft();
            assert_eq!(
                bytes(insert_row(MY, &table, &draft)),
                b"INSERT INTO `wp_posts` (`post_author`, `post_date`, `post_date_gmt`, `post_content`, `post_content_filtered`, `post_title`, `post_excerpt`, `post_status`, `post_type`, `comment_status`, `ping_status`, `post_password`, `post_name`, `to_ping`, `pinged`, `post_modified`, `post_modified_gmt`, `post_parent`, `menu_order`, `post_mime_type`, `guid`) VALUES (1, '2026-01-01 00:00:00', '2026-01-01 00:00:00', 'body', '', 'Hello', '', 'publish', 'post', 'open', 'open', '', 'hello', '', '', '2026-01-01 00:00:00', '2026-01-01 00:00:00', 0, 0, '', '')"
            );
            assert_eq!(
                bytes(update_row(MY, &table, id, &draft)),
                b"UPDATE `wp_posts` SET `post_author` = 1, `post_date` = '2026-01-01 00:00:00', `post_date_gmt` = '2026-01-01 00:00:00', `post_content` = 'body', `post_content_filtered` = '', `post_title` = 'Hello', `post_excerpt` = '', `post_status` = 'publish', `post_type` = 'post', `comment_status` = 'open', `ping_status` = 'open', `post_password` = '', `post_name` = 'hello', `to_ping` = '', `pinged` = '', `post_modified` = '2026-01-01 00:00:00', `post_modified_gmt` = '2026-01-01 00:00:00', `post_parent` = 0, `menu_order` = 0, `post_mime_type` = '', `guid` = '' WHERE `ID` = 12"
            );
            assert_eq!(
                bytes(delete_row(MY, &table, id)),
                b"DELETE FROM `wp_posts` WHERE `ID` = 12"
            );
        }
    }

    #[test]
    fn postgres_statements_quote_fold_and_return_the_id() {
        let opened = posts_table();
        assert!(opened.is_some());
        if let Some((table, id)) = opened {
            assert_eq!(
                bytes(select_row(PG, &table, id)),
                b"SELECT * FROM \"wp_posts\" WHERE \"ID\" = 12 LIMIT 1"
            );
            assert_eq!(
                bytes(select_path(PG, &table, &[Box::from(*b"about"), Box::from(*b"team")], &[b"page", b"attachment"])),
                b"\n\t\tSELECT \"ID\", \"post_name\", \"post_parent\", \"post_type\"\n\t\tFROM \"wp_posts\"\n\t\tWHERE lower(\"post_name\") IN (lower('about'),lower('team'))\n\t\tAND \"post_type\" IN ('page','attachment')\n\t"
            );
            assert_eq!(
                bytes(select_name(PG, &table, b"it's")),
                b"SELECT * FROM \"wp_posts\" WHERE lower(\"post_name\") = lower('it''s') LIMIT 1"
            );
            let mut draft = Draft::new();
            draft.set(Field::Author, b"1").set(Field::Title, b"a\\b");
            assert_eq!(
                bytes(insert_row(PG, &table, &draft)),
                b"INSERT INTO \"wp_posts\" (\"post_author\", \"post_date\", \"post_date_gmt\", \"post_content\", \"post_content_filtered\", \"post_title\", \"post_excerpt\", \"post_status\", \"post_type\", \"comment_status\", \"ping_status\", \"post_password\", \"post_name\", \"to_ping\", \"pinged\", \"post_modified\", \"post_modified_gmt\", \"post_parent\", \"menu_order\", \"post_mime_type\", \"guid\") VALUES (1, '', '', '', '', 'a\\b', '', '', '', '', '', '', '', '', '', '', '', 0, 0, '', '') RETURNING \"ID\""
            );
            let update = bytes(update_row(PG, &table, id, &draft));
            assert!(update.starts_with(b"UPDATE \"wp_posts\" SET \"post_author\" = 1, "));
            assert!(update.ends_with(b"\"guid\" = '' WHERE \"ID\" = 12"));
            assert_eq!(
                bytes(delete_row(PG, &table, id)),
                b"DELETE FROM \"wp_posts\" WHERE \"ID\" = 12"
            );
            // PostgreSQL text cannot hold NUL, so the statement is refused before the server.
            draft.set(Field::Content, b"a\0b");
            assert_eq!(insert_row(PG, &table, &draft), Err(LiteralError::Nul));
            assert_eq!(select_name(PG, &table, b"\0"), Err(LiteralError::Nul));
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
        let opened = posts_table();
        assert!(opened.is_some());
        if let Some((table, _)) = opened {
            let mut draft = Draft::new();
            draft
                .set(Field::Author, b"nope")
                .set(Field::Title, b"a'b")
                .set(Field::Parent, b"3")
                .set(Field::MenuOrder, b"4");
            let sql = bytes(insert_row(MY, &table, &draft));
            assert!(sql.starts_with(b"INSERT INTO `wp_posts`"));
            assert!(sql.windows(11).any(|w| w == b"VALUES (0, "));
            assert!(sql.windows(6).any(|w| w == br"'a\'b'"));
            assert!(sql.windows(6).any(|w| w == b", 3, 4"));
        }
    }

    proptest::proptest! {
        #[test]
        fn ids_are_decimal_and_unquoted(id in 1_u64..=10_000) {
            let prefix = TablePrefix::new("wp_");
            let stem = TableStem::new("posts");
            let post = PostId::new(id);
            proptest::prop_assert!(prefix.is_ok() && stem.is_ok() && post.is_ok());
            if let (Ok(prefix), Ok(stem), Ok(post)) = (prefix, stem, post) {
                let table = prefix.table(&stem);
                let select = bytes(select_row(MY, &table, post));
                let expected = format!("SELECT * FROM wp_posts WHERE ID = {id} LIMIT 1");
                proptest::prop_assert_eq!(select, expected.as_bytes());
                let delete = bytes(delete_row(MY, &table, post));
                let expected = format!("DELETE FROM `wp_posts` WHERE `ID` = {id}");
                proptest::prop_assert_eq!(delete, expected.as_bytes());
            }
        }
    }
}
