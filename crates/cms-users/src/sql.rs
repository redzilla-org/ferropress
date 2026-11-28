//! Statements the reference issues for user reads and writes.

use cms_db::{Dialect, LiteralError, Sql, Statement, TableName};

use crate::UserId;
use crate::draft::{Draft, Field};
use crate::id::MetaId;
use crate::key::MetaKey;

/// The users primary key, returned by `PostgreSQL` inserts.
const ID: &[u8] = b"ID";
/// The usermeta primary key, returned by `PostgreSQL` inserts.
const META_ID: &[u8] = b"umeta_id";

/// One user's meta rows: `SELECT … WHERE user_id IN ({id}) ORDER BY umeta_id`.
pub(crate) fn select_meta(
    dialect: Dialect,
    table: &TableName,
    id: UserId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT ")
        .bare("user_id")
        .raw(", ")
        .bare("meta_key")
        .raw(", ")
        .bare("meta_value")
        .raw(" FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare("user_id")
        .raw(" IN (")
        .number(id.as_u64())
        .raw(") ORDER BY ")
        .bare(META_ID)
        .raw(" ASC");
    sql.finish()
}

/// One row: `SELECT * … WHERE ID = '{id}' LIMIT 1`.
pub(crate) fn select_row(
    dialect: Dialect,
    table: &TableName,
    id: UserId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    // The reference quotes the id (%s); both servers coerce the text to the column type.
    sql.raw("SELECT * FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare(ID)
        .raw(" = '")
        .number(id.as_u64())
        .raw("' LIMIT 1");
    sql.finish()
}

/// Insert one usermeta row: `user_id` `%d`, key and value `%s`.
pub(crate) fn insert_meta(
    dialect: Dialect,
    table: &TableName,
    id: UserId,
    key: &MetaKey,
    value: &[u8],
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("INSERT INTO ")
        .ident(table.as_bytes())
        .raw(" (")
        .ident("user_id")
        .raw(", ")
        .ident("meta_key")
        .raw(", ")
        .ident("meta_value")
        .raw(") VALUES (")
        .number(id.as_u64())
        .raw(", ")
        .literal(key.as_bytes())
        .raw(", ")
        .literal(value)
        .raw(")")
        .returning(META_ID);
    sql.finish()
}

/// Rewrite `meta_value` for one user and key.
pub(crate) fn update_meta(
    dialect: Dialect,
    table: &TableName,
    id: UserId,
    key: &MetaKey,
    value: &[u8],
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("UPDATE ")
        .ident(table.as_bytes())
        .raw(" SET ")
        .ident("meta_value")
        .raw(" = ")
        .literal(value)
        .raw(" WHERE ")
        .ident("user_id")
        .raw(" = ")
        .number(id.as_u64())
        .raw(" AND ")
        .ident("meta_key")
        .raw(" = ")
        .literal(key.as_bytes());
    sql.finish()
}

/// Row ids for one user and key.
pub(crate) fn select_meta_ids(
    dialect: Dialect,
    table: &TableName,
    id: UserId,
    key: &MetaKey,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT ")
        .bare(META_ID)
        .raw(" FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare("meta_key")
        .raw(" = ")
        .literal(key.as_bytes())
        .raw(" AND ")
        .bare("user_id")
        .raw(" = ")
        .number(id.as_u64());
    sql.finish()
}

/// Delete the selected `umeta_id` list. `ids` must not be empty.
pub(crate) fn delete_meta_ids(
    dialect: Dialect,
    table: &TableName,
    ids: &[MetaId],
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("DELETE FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare(META_ID)
        .raw(" IN( ");
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            sql.raw(",");
        }
        sql.number(id.as_u64());
    }
    sql.raw(" )");
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
        sql.literal(draft.get(*field));
    }
    sql.raw(")").returning(ID);
    sql.finish()
}

/// Compact update of the caller-supplied cells for `id`.
pub(crate) fn update_row(
    dialect: Dialect,
    table: &TableName,
    id: UserId,
    draft: &Draft,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("UPDATE ").ident(table.as_bytes()).raw(" SET ");
    for (index, field) in Field::UPDATE.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.ident(field.column())
            .raw(" = ")
            .literal(draft.get(*field));
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
    id: UserId,
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

#[cfg(test)]
mod tests {
    use super::{
        delete_meta_ids, delete_row, insert_meta, insert_row, select_meta, select_meta_ids,
        select_row, update_meta, update_row,
    };
    use crate::UserId;
    use crate::draft::{Draft, Field};
    use crate::id::MetaId;
    use crate::key::MetaKey;
    use cms_db::{Dialect, LiteralError, Sql, TableName, TablePrefix, TableStem};

    const MY: Dialect = Dialect::MySql;
    const PG: Dialect = Dialect::Postgres;

    fn bytes(sql: Result<Sql, LiteralError>) -> Vec<u8> {
        sql.map(|sql| sql.as_bytes().to_vec()).unwrap_or_default()
    }

    fn users_table() -> Option<(cms_db::TableName, UserId)> {
        let prefix = TablePrefix::new("wp_");
        let stem = TableStem::new("users");
        let id = UserId::new(12);
        if let (Ok(prefix), Ok(stem), Ok(id)) = (prefix, stem, id) {
            return Some((prefix.table(&stem), id));
        }
        None
    }

    fn meta_table() -> Option<TableName> {
        let prefix = TablePrefix::new("wp_").ok()?;
        let stem = TableStem::new("usermeta").ok()?;
        Some(prefix.table(&stem))
    }

    fn sample_draft() -> Draft {
        let mut draft = Draft::new();
        draft
            .set(Field::Password, b"hash")
            .set(Field::Nicename, b"ada")
            .set(Field::Email, b"ada@example.test")
            .set(Field::Url, b"https://ada.test")
            .set(Field::Registered, b"2026-01-01 00:00:00")
            .set(Field::ActivationKey, b"")
            .set(Field::DisplayName, b"Ada")
            .set(Field::Login, b"ada");
        draft
    }

    #[test]
    fn statements_match_the_lab_prefix() {
        let opened = users_table();
        assert!(opened.is_some());
        if let Some((table, id)) = opened {
            assert_eq!(
                bytes(select_row(MY, &table, id)),
                b"SELECT * FROM wp_users WHERE ID = '12' LIMIT 1"
            );
            let meta = TableStem::new("usermeta");
            assert!(meta.is_ok());
            if let Ok(meta) = meta {
                let prefix = TablePrefix::new("wp_");
                assert!(prefix.is_ok());
                if let Ok(prefix) = prefix {
                    assert_eq!(
                        bytes(select_meta(MY, &prefix.table(&meta), id)),
                        b"SELECT user_id, meta_key, meta_value FROM wp_usermeta WHERE user_id IN (12) ORDER BY umeta_id ASC"
                    );
                    let key = MetaKey::new("nickname");
                    let mid = MetaId::new(7);
                    let other = MetaId::new(8);
                    assert!(key.is_ok() && mid.is_ok() && other.is_ok());
                    if let (Ok(key), Ok(mid), Ok(other)) = (key, mid, other) {
                        let table = prefix.table(&meta);
                        assert_eq!(
                            bytes(insert_meta(MY, &table, id, &key, b"Ada")),
                            b"INSERT INTO `wp_usermeta` (`user_id`, `meta_key`, `meta_value`) VALUES (12, 'nickname', 'Ada')"
                        );
                        assert_eq!(
                            bytes(update_meta(MY, &table, id, &key, b"Ada")),
                            b"UPDATE `wp_usermeta` SET `meta_value` = 'Ada' WHERE `user_id` = 12 AND `meta_key` = 'nickname'"
                        );
                        assert_eq!(
                            bytes(select_meta_ids(MY, &table, id, &key)),
                            b"SELECT umeta_id FROM wp_usermeta WHERE meta_key = 'nickname' AND user_id = 12"
                        );
                        assert_eq!(
                            bytes(delete_meta_ids(MY, &table, &[mid, other])),
                            b"DELETE FROM wp_usermeta WHERE umeta_id IN( 7,8 )"
                        );
                    }
                }
            }
            let draft = sample_draft();
            assert_eq!(
                bytes(insert_row(MY, &table, &draft)),
                b"INSERT INTO `wp_users` (`user_pass`, `user_nicename`, `user_email`, `user_url`, `user_registered`, `user_activation_key`, `display_name`, `user_login`) VALUES ('hash', 'ada', 'ada@example.test', 'https://ada.test', '2026-01-01 00:00:00', '', 'Ada', 'ada')"
            );
            assert_eq!(
                bytes(update_row(MY, &table, id, &draft)),
                b"UPDATE `wp_users` SET `user_pass` = 'hash', `user_nicename` = 'ada', `user_email` = 'ada@example.test', `user_url` = 'https://ada.test', `user_registered` = '2026-01-01 00:00:00', `user_activation_key` = '', `display_name` = 'Ada' WHERE `ID` = 12"
            );
            assert_eq!(
                bytes(delete_row(MY, &table, id)),
                b"DELETE FROM `wp_users` WHERE `ID` = 12"
            );
        }
    }

    #[test]
    fn postgres_statements_quote_every_name_and_return_ids() {
        let opened = users_table();
        let meta = meta_table();
        let key = MetaKey::new("nickname");
        let ids = (MetaId::new(7), MetaId::new(8));
        assert!(opened.is_some() && meta.is_some() && key.is_ok());
        if let (Some((table, id)), Some(meta), Ok(key), (Ok(mid), Ok(other))) =
            (opened, meta, key, ids)
        {
            assert_eq!(
                bytes(select_row(PG, &table, id)),
                b"SELECT * FROM \"wp_users\" WHERE \"ID\" = '12' LIMIT 1"
            );
            assert_eq!(
                bytes(select_meta(PG, &meta, id)),
                b"SELECT \"user_id\", \"meta_key\", \"meta_value\" FROM \"wp_usermeta\" WHERE \"user_id\" IN (12) ORDER BY \"umeta_id\" ASC"
            );
            assert_eq!(
                bytes(insert_meta(PG, &meta, id, &key, b"Ada")),
                b"INSERT INTO \"wp_usermeta\" (\"user_id\", \"meta_key\", \"meta_value\") VALUES (12, 'nickname', 'Ada') RETURNING \"umeta_id\""
            );
            assert_eq!(
                bytes(update_meta(PG, &meta, id, &key, b"Ada")),
                b"UPDATE \"wp_usermeta\" SET \"meta_value\" = 'Ada' WHERE \"user_id\" = 12 AND \"meta_key\" = 'nickname'"
            );
            assert_eq!(
                bytes(select_meta_ids(PG, &meta, id, &key)),
                b"SELECT \"umeta_id\" FROM \"wp_usermeta\" WHERE \"meta_key\" = 'nickname' AND \"user_id\" = 12"
            );
            assert_eq!(
                bytes(delete_meta_ids(PG, &meta, &[mid, other])),
                b"DELETE FROM \"wp_usermeta\" WHERE \"umeta_id\" IN( 7,8 )"
            );
            let draft = sample_draft();
            assert_eq!(
                bytes(insert_row(PG, &table, &draft)),
                b"INSERT INTO \"wp_users\" (\"user_pass\", \"user_nicename\", \"user_email\", \"user_url\", \"user_registered\", \"user_activation_key\", \"display_name\", \"user_login\") VALUES ('hash', 'ada', 'ada@example.test', 'https://ada.test', '2026-01-01 00:00:00', '', 'Ada', 'ada') RETURNING \"ID\""
            );
            let update = bytes(update_row(PG, &table, id, &draft));
            assert!(update.starts_with(b"UPDATE \"wp_users\" SET \"user_pass\" = 'hash', "));
            assert!(update.ends_with(b"\"display_name\" = 'Ada' WHERE \"ID\" = 12"));
            assert_eq!(
                bytes(delete_row(PG, &table, id)),
                b"DELETE FROM \"wp_users\" WHERE \"ID\" = 12"
            );
            assert_eq!(
                insert_meta(PG, &meta, id, &key, b"a\0b"),
                Err(LiteralError::Nul)
            );
        }
    }

    #[test]
    fn cells_are_quoted_and_login_is_insert_only() {
        let opened = users_table();
        assert!(opened.is_some());
        if let Some((table, id)) = opened {
            let mut draft = Draft::new();
            draft.set(Field::Login, b"a'b").set(Field::Nicename, b"x");
            let insert = bytes(insert_row(MY, &table, &draft));
            assert!(insert.starts_with(b"INSERT INTO `wp_users`"));
            assert!(insert.ends_with(br", 'a\'b')"));
            let update = bytes(update_row(MY, &table, id, &draft));
            assert!(update.starts_with(b"UPDATE `wp_users`"));
            assert!(!update.windows(10).any(|window| window == b"user_login"));
            assert!(update.ends_with(b"WHERE `ID` = 12"));
        }
    }

    proptest::proptest! {
        #[test]
        fn ids_are_quoted_on_select_and_unquoted_on_delete(id in 1_u64..=10_000) {
            let prefix = TablePrefix::new("wp_");
            let stem = TableStem::new("users");
            let user = UserId::new(id);
            proptest::prop_assert!(prefix.is_ok() && stem.is_ok() && user.is_ok());
            if let (Ok(prefix), Ok(stem), Ok(user)) = (prefix, stem, user) {
                let table = prefix.table(&stem);
                let select = bytes(select_row(MY, &table, user));
                let expected = format!("SELECT * FROM wp_users WHERE ID = '{id}' LIMIT 1");
                proptest::prop_assert_eq!(select, expected.as_bytes());
                let delete = bytes(delete_row(MY, &table, user));
                let expected = format!("DELETE FROM `wp_users` WHERE `ID` = {id}");
                proptest::prop_assert_eq!(delete, expected.as_bytes());
                let meta = TableStem::new("usermeta");
                proptest::prop_assert!(meta.is_ok());
                if let Ok(meta) = meta {
                    let select = bytes(select_meta(MY, &prefix.table(&meta), user));
                    let expected = format!(
                        "SELECT user_id, meta_key, meta_value FROM wp_usermeta WHERE user_id IN ({id}) ORDER BY umeta_id ASC"
                    );
                    proptest::prop_assert_eq!(select, expected.as_bytes());
                }
            }
        }
    }
}
