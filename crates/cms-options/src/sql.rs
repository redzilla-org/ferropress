//! Statements the reference issues for option reads and writes.

use cms_db::{Dialect, LiteralError, Sql, Statement, TableName};

use crate::autoload::Autoload;
use crate::name::OptionName;

/// The three columns every option write names.
const COLUMNS: [&[u8]; 3] = [b"option_name", b"option_value", b"autoload"];

/// Autoloaded rows: `yes`, `on`, `auto-on`, `auto`.
pub(crate) fn select_autoloaded(dialect: Dialect, table: &TableName) -> Result<Sql, LiteralError> {
    let mut sql = select_names(dialect, table);
    sql.raw(" WHERE ")
        .bare("autoload")
        .raw(" IN ( 'yes', 'on', 'auto-on', 'auto' )");
    sql.finish()
}

/// Every row, used when the autoload select is empty or rejected.
pub(crate) fn select_all(dialect: Dialect, table: &TableName) -> Result<Sql, LiteralError> {
    select_names(dialect, table).finish()
}

fn select_names(dialect: Dialect, table: &TableName) -> Statement {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT ")
        .bare("option_name")
        .raw(", ")
        .bare("option_value")
        .raw(" FROM ")
        .bare(table.as_bytes());
    sql
}

/// One stored value.
pub(crate) fn select_value(
    dialect: Dialect,
    table: &TableName,
    name: &OptionName,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT ")
        .bare("option_value")
        .raw(" FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ");
    name_is(&mut sql, false, name).raw(" LIMIT 1");
    sql.finish()
}

/// Autoload column for one name (unquoted table, no `LIMIT`).
pub(crate) fn select_autoload(
    dialect: Dialect,
    table: &TableName,
    name: &OptionName,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT ")
        .bare("autoload")
        .raw(" FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ");
    name_is(&mut sql, false, name);
    sql.finish()
}

/// Insert or replace one row.
pub(crate) fn insert_row(
    dialect: Dialect,
    table: &TableName,
    name: &OptionName,
    value: &[u8],
    autoload: Autoload,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("INSERT INTO ").ident(table.as_bytes()).raw(" (");
    for (index, column) in COLUMNS.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.ident(column);
    }
    sql.raw(") VALUES (")
        .literal(name.as_bytes())
        .raw(", ")
        .literal(value)
        .raw(", ")
        .literal(autoload.as_bytes())
        .raw(")");
    // Same upsert, spelled per server; the Postgres target is the lower() unique index that keeps
    // names unique case-insensitively, as the reference collation does.
    match dialect {
        Dialect::MySql => sql.raw(" ON DUPLICATE KEY UPDATE "),
        Dialect::Postgres => sql
            .raw(" ON CONFLICT (")
            .case_fold(|s| s.ident("option_name"))
            .raw(") DO UPDATE SET "),
    };
    for (index, column) in COLUMNS.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.ident(column).raw(" = ");
        match dialect {
            Dialect::MySql => sql.raw("VALUES(").ident(column).raw(")"),
            Dialect::Postgres => sql.raw("EXCLUDED.").ident(column),
        };
    }
    sql.finish()
}

/// Write the value, and optionally the autoload token.
pub(crate) fn update_row(
    dialect: Dialect,
    table: &TableName,
    name: &OptionName,
    value: &[u8],
    autoload: Option<Autoload>,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("UPDATE ")
        .ident(table.as_bytes())
        .raw(" SET ")
        .ident("option_value")
        .raw(" = ")
        .literal(value);
    if let Some(autoload) = autoload {
        sql.raw(", ")
            .ident("autoload")
            .raw(" = ")
            .literal(autoload.as_bytes());
    }
    sql.raw(" WHERE ");
    name_is(&mut sql, true, name);
    sql.finish()
}

/// Remove one name.
pub(crate) fn delete_row(
    dialect: Dialect,
    table: &TableName,
    name: &OptionName,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("DELETE FROM ")
        .ident(table.as_bytes())
        .raw(" WHERE ");
    name_is(&mut sql, true, name);
    sql.finish()
}

// `option_name = 'x'`, case-insensitive like the reference collation. `quoted` follows whether
// the reference statement ticks the column.
fn name_is<'s>(sql: &'s mut Statement, quoted: bool, name: &OptionName) -> &'s mut Statement {
    sql.case_fold(|s| {
        if quoted {
            s.ident("option_name")
        } else {
            s.bare("option_name")
        }
    })
    .raw(" = ")
    .case_fold(|s| s.literal(name.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{
        delete_row, insert_row, select_all, select_autoload, select_autoloaded, select_value,
        update_row,
    };
    use crate::Autoload;
    use crate::OptionName;
    use cms_db::{Dialect, LiteralError, Sql, TableName, TablePrefix, TableStem};

    const MY: Dialect = Dialect::MySql;
    const PG: Dialect = Dialect::Postgres;

    fn bytes(sql: Result<Sql, LiteralError>) -> Vec<u8> {
        sql.map(|sql| sql.as_bytes().to_vec()).unwrap_or_default()
    }

    fn options_table() -> Option<TableName> {
        let prefix = TablePrefix::new("wp_").ok()?;
        let stem = TableStem::new("options").ok()?;
        Some(prefix.table(&stem))
    }

    #[test]
    fn statements_match_the_lab_prefix() {
        let prefix = TablePrefix::new("wp_");
        let stem = TableStem::new("options");
        assert!(prefix.is_ok() && stem.is_ok());
        if let (Ok(prefix), Ok(stem)) = (prefix, stem) {
            let table = prefix.table(&stem);
            assert_eq!(
                bytes(select_autoloaded(MY, &table)),
                b"SELECT option_name, option_value FROM wp_options WHERE autoload IN ( 'yes', 'on', 'auto-on', 'auto' )"
            );
            assert_eq!(
                bytes(select_all(MY, &table)),
                b"SELECT option_name, option_value FROM wp_options"
            );
            let home = OptionName::new("home");
            let key = OptionName::new("k");
            assert!(home.is_ok() && key.is_ok());
            if let (Ok(home), Ok(key)) = (home, key) {
                assert_eq!(
                    bytes(select_value(MY, &table, &home)),
                    b"SELECT option_value FROM wp_options WHERE option_name = 'home' LIMIT 1"
                );
                assert_eq!(
                    bytes(select_autoload(MY, &table, &key)),
                    b"SELECT autoload FROM wp_options WHERE option_name = 'k'"
                );
                assert_eq!(
                    bytes(insert_row(MY, &table, &key, b"v", Autoload::Auto)),
                    b"INSERT INTO `wp_options` (`option_name`, `option_value`, `autoload`) VALUES ('k', 'v', 'auto') ON DUPLICATE KEY UPDATE `option_name` = VALUES(`option_name`), `option_value` = VALUES(`option_value`), `autoload` = VALUES(`autoload`)"
                );
                assert_eq!(
                    bytes(update_row(MY, &table, &key, b"v3", None)),
                    b"UPDATE `wp_options` SET `option_value` = 'v3' WHERE `option_name` = 'k'"
                );
                assert_eq!(
                    bytes(update_row(MY, &table, &key, b"v2", Some(Autoload::On))),
                    b"UPDATE `wp_options` SET `option_value` = 'v2', `autoload` = 'on' WHERE `option_name` = 'k'"
                );
                assert_eq!(
                    bytes(delete_row(MY, &table, &key)),
                    b"DELETE FROM `wp_options` WHERE `option_name` = 'k'"
                );
            }
        }
    }

    #[test]
    fn postgres_statements_fold_names_and_upsert_on_the_lower_index() {
        let table = options_table();
        let key = OptionName::new("k");
        assert!(table.is_some() && key.is_ok());
        if let (Some(table), Ok(key)) = (table, key) {
            assert_eq!(
                bytes(select_autoloaded(PG, &table)),
                b"SELECT \"option_name\", \"option_value\" FROM \"wp_options\" WHERE \"autoload\" IN ( 'yes', 'on', 'auto-on', 'auto' )"
            );
            assert_eq!(
                bytes(select_all(PG, &table)),
                b"SELECT \"option_name\", \"option_value\" FROM \"wp_options\""
            );
            assert_eq!(
                bytes(select_value(PG, &table, &key)),
                b"SELECT \"option_value\" FROM \"wp_options\" WHERE lower(\"option_name\") = lower('k') LIMIT 1"
            );
            assert_eq!(
                bytes(select_autoload(PG, &table, &key)),
                b"SELECT \"autoload\" FROM \"wp_options\" WHERE lower(\"option_name\") = lower('k')"
            );
            assert_eq!(
                bytes(insert_row(PG, &table, &key, b"it's", Autoload::Auto)),
                b"INSERT INTO \"wp_options\" (\"option_name\", \"option_value\", \"autoload\") VALUES ('k', 'it''s', 'auto') ON CONFLICT (lower(\"option_name\")) DO UPDATE SET \"option_name\" = EXCLUDED.\"option_name\", \"option_value\" = EXCLUDED.\"option_value\", \"autoload\" = EXCLUDED.\"autoload\""
            );
            assert_eq!(
                bytes(update_row(PG, &table, &key, b"v2", Some(Autoload::On))),
                b"UPDATE \"wp_options\" SET \"option_value\" = 'v2', \"autoload\" = 'on' WHERE lower(\"option_name\") = lower('k')"
            );
            assert_eq!(
                bytes(delete_row(PG, &table, &key)),
                b"DELETE FROM \"wp_options\" WHERE lower(\"option_name\") = lower('k')"
            );
            assert_eq!(
                update_row(PG, &table, &key, b"a\0b", None),
                Err(LiteralError::Nul)
            );
        }
    }
}
