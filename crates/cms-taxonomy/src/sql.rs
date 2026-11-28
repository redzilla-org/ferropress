//! Statements the reference issues for term reads and writes.

use cms_db::{Dialect, LiteralError, Sql, Statement, TableName};

use crate::draft::{Draft, Field};
use crate::{ObjectId, TermId, TermTaxonomyId};

/// Joined `terms` / `term_taxonomy` rows for one id.
pub(crate) fn select_rows(
    dialect: Dialect,
    terms: &TableName,
    taxonomy: &TableName,
    id: TermId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT t.*, tt.* FROM ")
        .bare(terms.as_bytes())
        .raw(" AS t INNER JOIN ")
        .bare(taxonomy.as_bytes())
        .raw(" AS tt ON t.")
        .bare("term_id")
        .raw(" = tt.")
        .bare("term_id")
        .raw(" WHERE t.")
        .bare("term_id")
        .raw(" = ")
        .number(id.as_u64());
    sql.finish()
}

/// Compact insert of the terms-table cells.
pub(crate) fn insert_term(
    dialect: Dialect,
    table: &TableName,
    draft: &Draft,
) -> Result<Sql, LiteralError> {
    let mut sql = insert_named(
        dialect,
        table,
        &[Field::Name, Field::Slug, Field::TermGroup],
        draft,
        None,
    );
    sql.returning("term_id");
    sql.finish()
}

/// Compact insert of the taxonomy-table cells for `term_id`.
pub(crate) fn insert_taxonomy(
    dialect: Dialect,
    table: &TableName,
    term_id: TermId,
    draft: &Draft,
) -> Result<Sql, LiteralError> {
    let mut sql = insert_named(
        dialect,
        table,
        &[
            Field::Taxonomy,
            Field::Description,
            Field::Parent,
            Field::Count,
        ],
        draft,
        Some(term_id),
    );
    sql.returning("term_taxonomy_id");
    sql.finish()
}

/// Compact update of the terms-table cells for `id`.
pub(crate) fn update_term(
    dialect: Dialect,
    table: &TableName,
    id: TermId,
    draft: &Draft,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("UPDATE ").ident(table.as_bytes()).raw(" SET ");
    append_set(
        &mut sql,
        &[Field::Name, Field::Slug, Field::TermGroup],
        draft,
    );
    sql.raw(" WHERE ")
        .ident("term_id")
        .raw(" = ")
        .number(id.as_u64());
    sql.finish()
}

/// Compact update of the taxonomy-table cells for `taxonomy_id`.
pub(crate) fn update_taxonomy(
    dialect: Dialect,
    table: &TableName,
    term_id: TermId,
    taxonomy_id: TermTaxonomyId,
    draft: &Draft,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("UPDATE ")
        .ident(table.as_bytes())
        .raw(" SET ")
        .ident("term_id")
        .raw(" = ")
        .number(term_id.as_u64())
        .raw(", ");
    append_set(
        &mut sql,
        &[Field::Taxonomy, Field::Description, Field::Parent],
        draft,
    );
    sql.raw(" WHERE ")
        .ident("term_taxonomy_id")
        .raw(" = ")
        .number(taxonomy_id.as_u64());
    sql.finish()
}

/// Remove one taxonomy row.
pub(crate) fn delete_taxonomy(
    dialect: Dialect,
    table: &TableName,
    id: TermTaxonomyId,
) -> Result<Sql, LiteralError> {
    delete_where(dialect, table, b"term_taxonomy_id", id.as_u64())
}

/// How many taxonomy rows still use `id`.
pub(crate) fn count_taxonomies(
    dialect: Dialect,
    table: &TableName,
    id: TermId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT COUNT(*) FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare("term_id")
        .raw(" = ")
        .number(id.as_u64());
    sql.finish()
}

/// Remove one terms row.
pub(crate) fn delete_term(
    dialect: Dialect,
    table: &TableName,
    id: TermId,
) -> Result<Sql, LiteralError> {
    delete_where(dialect, table, b"term_id", id.as_u64())
}

/// Whether one object–taxonomy pair exists.
pub(crate) fn select_link(
    dialect: Dialect,
    table: &TableName,
    object: ObjectId,
    taxonomy_id: TermTaxonomyId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT ")
        .bare("term_taxonomy_id")
        .raw(" FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare("object_id")
        .raw(" = ")
        .number(object.as_u64())
        .raw(" AND ")
        .bare("term_taxonomy_id")
        .raw(" = ")
        .number(taxonomy_id.as_u64());
    sql.finish()
}

/// Object ids that use one taxonomy row.
pub(crate) fn select_objects(
    dialect: Dialect,
    table: &TableName,
    taxonomy_id: TermTaxonomyId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("SELECT ")
        .bare("object_id")
        .raw(" FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare("term_taxonomy_id")
        .raw(" = ")
        .number(taxonomy_id.as_u64());
    sql.finish()
}

/// Insert one object–taxonomy pair. The table has a composite key, so no id comes back.
pub(crate) fn insert_link(
    dialect: Dialect,
    table: &TableName,
    object: ObjectId,
    taxonomy_id: TermTaxonomyId,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("INSERT INTO ")
        .ident(table.as_bytes())
        .raw(" (")
        .ident("object_id")
        .raw(", ")
        .ident("term_taxonomy_id")
        .raw(") VALUES (")
        .number(object.as_u64())
        .raw(", ")
        .number(taxonomy_id.as_u64())
        .raw(")");
    sql.finish()
}

/// Remove one object's listed taxonomy rows.
pub(crate) fn delete_links(
    dialect: Dialect,
    table: &TableName,
    object: ObjectId,
    taxonomy_ids: &[TermTaxonomyId],
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("DELETE FROM ")
        .bare(table.as_bytes())
        .raw(" WHERE ")
        .bare("object_id")
        .raw(" = ")
        .number(object.as_u64())
        .raw(" AND ")
        .bare("term_taxonomy_id")
        .raw(" IN (");
    for (index, id) in taxonomy_ids.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        // The reference quotes these ids (%s); both servers coerce the text to the column type.
        sql.raw("'").number(id.as_u64()).raw("'");
    }
    sql.raw(")");
    sql.finish()
}

fn insert_named(
    dialect: Dialect,
    table: &TableName,
    fields: &[Field],
    draft: &Draft,
    term_id: Option<TermId>,
) -> Statement {
    let mut sql = Statement::new(dialect);
    sql.raw("INSERT INTO ").ident(table.as_bytes()).raw(" (");
    if term_id.is_some() {
        sql.ident("term_id").raw(", ");
    }
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.ident(field.column());
    }
    sql.raw(") VALUES (");
    if let Some(term_id) = term_id {
        sql.number(term_id.as_u64()).raw(", ");
    }
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        format_cell(&mut sql, *field, draft.get(*field));
    }
    sql.raw(")");
    sql
}

fn append_set(sql: &mut Statement, fields: &[Field], draft: &Draft) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            sql.raw(", ");
        }
        sql.ident(field.column()).raw(" = ");
        format_cell(sql, *field, draft.get(*field));
    }
}

fn delete_where(
    dialect: Dialect,
    table: &TableName,
    column: &[u8],
    id: u64,
) -> Result<Sql, LiteralError> {
    let mut sql = Statement::new(dialect);
    sql.raw("DELETE FROM ")
        .ident(table.as_bytes())
        .raw(" WHERE ")
        .ident(column)
        .raw(" = ")
        .number(id);
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
        count_taxonomies, delete_links, delete_taxonomy, delete_term, format_int, insert_link,
        insert_taxonomy, insert_term, select_link, select_objects, select_rows, update_taxonomy,
        update_term,
    };
    use crate::draft::{Draft, Field};
    use crate::{ObjectId, TermId, TermTaxonomyId};
    use cms_db::{Dialect, LiteralError, Sql, TableName, TablePrefix, TableStem};

    const MY: Dialect = Dialect::MySql;
    const PG: Dialect = Dialect::Postgres;

    fn bytes(sql: Result<Sql, LiteralError>) -> Vec<u8> {
        sql.map(|sql| sql.as_bytes().to_vec()).unwrap_or_default()
    }

    fn tables() -> Option<(TableName, TableName, TermId, TermTaxonomyId)> {
        let prefix = TablePrefix::new("wp_");
        let terms = TableStem::new("terms");
        let taxonomy = TableStem::new("term_taxonomy");
        let term = TermId::new(12);
        let taxonomy_id = TermTaxonomyId::new(9);
        if let (Ok(prefix), Ok(terms), Ok(taxonomy), Ok(term), Ok(taxonomy_id)) =
            (prefix, terms, taxonomy, term, taxonomy_id)
        {
            return Some((
                prefix.table(&terms),
                prefix.table(&taxonomy),
                term,
                taxonomy_id,
            ));
        }
        None
    }

    fn links() -> Option<(TableName, ObjectId, TermTaxonomyId)> {
        let prefix = TablePrefix::new("wp_").ok()?;
        let rel = TableStem::new("term_relationships").ok()?;
        Some((
            prefix.table(&rel),
            ObjectId::new(7).ok()?,
            TermTaxonomyId::new(12).ok()?,
        ))
    }

    fn sample_draft() -> Draft {
        let mut draft = Draft::new();
        draft
            .set(Field::Name, b"News")
            .set(Field::Slug, b"news")
            .set(Field::Taxonomy, b"category")
            .set(Field::Description, b"desc");
        draft
    }

    #[test]
    fn statements_match_the_lab_prefix() {
        let opened = tables();
        assert!(opened.is_some());
        if let Some((terms, taxonomy, term, taxonomy_id)) = opened {
            let draft = sample_draft();
            assert_eq!(
                bytes(select_rows(MY, &terms, &taxonomy, term)),
                b"SELECT t.*, tt.* FROM wp_terms AS t INNER JOIN wp_term_taxonomy AS tt ON t.term_id = tt.term_id WHERE t.term_id = 12"
            );
            assert_eq!(
                bytes(insert_term(MY, &terms, &draft)),
                b"INSERT INTO `wp_terms` (`name`, `slug`, `term_group`) VALUES ('News', 'news', 0)"
            );
            assert_eq!(
                bytes(insert_taxonomy(MY, &taxonomy, term, &draft)),
                b"INSERT INTO `wp_term_taxonomy` (`term_id`, `taxonomy`, `description`, `parent`, `count`) VALUES (12, 'category', 'desc', 0, 0)"
            );
            assert_eq!(
                bytes(update_term(MY, &terms, term, &draft)),
                b"UPDATE `wp_terms` SET `name` = 'News', `slug` = 'news', `term_group` = 0 WHERE `term_id` = 12"
            );
            assert_eq!(
                bytes(update_taxonomy(MY, &taxonomy, term, taxonomy_id, &draft)),
                b"UPDATE `wp_term_taxonomy` SET `term_id` = 12, `taxonomy` = 'category', `description` = 'desc', `parent` = 0 WHERE `term_taxonomy_id` = 9"
            );
            assert_eq!(
                bytes(delete_taxonomy(MY, &taxonomy, taxonomy_id)),
                b"DELETE FROM `wp_term_taxonomy` WHERE `term_taxonomy_id` = 9"
            );
            assert_eq!(
                bytes(count_taxonomies(MY, &taxonomy, term)),
                b"SELECT COUNT(*) FROM wp_term_taxonomy WHERE term_id = 12"
            );
            assert_eq!(
                bytes(delete_term(MY, &terms, term)),
                b"DELETE FROM `wp_terms` WHERE `term_id` = 12"
            );
            let prefix = TablePrefix::new("wp_");
            let rel = TableStem::new("term_relationships");
            let object = ObjectId::new(7);
            assert!(prefix.is_ok() && rel.is_ok() && object.is_ok());
            if let (Ok(prefix), Ok(rel), Ok(object)) = (prefix, rel, object) {
                let table = prefix.table(&rel);
                assert_eq!(
                    bytes(select_link(MY, &table, object, taxonomy_id)),
                    b"SELECT term_taxonomy_id FROM wp_term_relationships WHERE object_id = 7 AND term_taxonomy_id = 9"
                );
                assert_eq!(
                    bytes(select_objects(MY, &table, taxonomy_id)),
                    b"SELECT object_id FROM wp_term_relationships WHERE term_taxonomy_id = 9"
                );
                assert_eq!(
                    bytes(insert_link(MY, &table, object, taxonomy_id)),
                    b"INSERT INTO `wp_term_relationships` (`object_id`, `term_taxonomy_id`) VALUES (7, 9)"
                );
                let twelve = TermTaxonomyId::new(12);
                assert!(twelve.is_ok());
                if let Ok(twelve) = twelve {
                    assert_eq!(
                        bytes(delete_links(MY, &table, object, &[taxonomy_id, twelve])),
                        b"DELETE FROM wp_term_relationships WHERE object_id = 7 AND term_taxonomy_id IN ('9', '12')"
                    );
                }
            }
        }
    }

    #[test]
    fn postgres_statements_quote_every_name_and_return_ids() {
        let opened = tables();
        let linked = links();
        assert!(opened.is_some() && linked.is_some());
        if let (Some((terms, taxonomy, term, taxonomy_id)), Some((rel, object, twelve))) =
            (opened, linked)
        {
            let draft = sample_draft();
            assert_eq!(
                bytes(select_rows(PG, &terms, &taxonomy, term)),
                b"SELECT t.*, tt.* FROM \"wp_terms\" AS t INNER JOIN \"wp_term_taxonomy\" AS tt ON t.\"term_id\" = tt.\"term_id\" WHERE t.\"term_id\" = 12"
            );
            assert_eq!(
                bytes(insert_term(PG, &terms, &draft)),
                b"INSERT INTO \"wp_terms\" (\"name\", \"slug\", \"term_group\") VALUES ('News', 'news', 0) RETURNING \"term_id\""
            );
            assert_eq!(
                bytes(insert_taxonomy(PG, &taxonomy, term, &draft)),
                b"INSERT INTO \"wp_term_taxonomy\" (\"term_id\", \"taxonomy\", \"description\", \"parent\", \"count\") VALUES (12, 'category', 'desc', 0, 0) RETURNING \"term_taxonomy_id\""
            );
            assert_eq!(
                bytes(update_term(PG, &terms, term, &draft)),
                b"UPDATE \"wp_terms\" SET \"name\" = 'News', \"slug\" = 'news', \"term_group\" = 0 WHERE \"term_id\" = 12"
            );
            assert_eq!(
                bytes(update_taxonomy(PG, &taxonomy, term, taxonomy_id, &draft)),
                b"UPDATE \"wp_term_taxonomy\" SET \"term_id\" = 12, \"taxonomy\" = 'category', \"description\" = 'desc', \"parent\" = 0 WHERE \"term_taxonomy_id\" = 9"
            );
            assert_eq!(
                bytes(delete_taxonomy(PG, &taxonomy, taxonomy_id)),
                b"DELETE FROM \"wp_term_taxonomy\" WHERE \"term_taxonomy_id\" = 9"
            );
            assert_eq!(
                bytes(count_taxonomies(PG, &taxonomy, term)),
                b"SELECT COUNT(*) FROM \"wp_term_taxonomy\" WHERE \"term_id\" = 12"
            );
            assert_eq!(
                bytes(delete_term(PG, &terms, term)),
                b"DELETE FROM \"wp_terms\" WHERE \"term_id\" = 12"
            );
            assert_eq!(
                bytes(select_link(PG, &rel, object, taxonomy_id)),
                b"SELECT \"term_taxonomy_id\" FROM \"wp_term_relationships\" WHERE \"object_id\" = 7 AND \"term_taxonomy_id\" = 9"
            );
            assert_eq!(
                bytes(select_objects(PG, &rel, taxonomy_id)),
                b"SELECT \"object_id\" FROM \"wp_term_relationships\" WHERE \"term_taxonomy_id\" = 9"
            );
            assert_eq!(
                bytes(insert_link(PG, &rel, object, taxonomy_id)),
                b"INSERT INTO \"wp_term_relationships\" (\"object_id\", \"term_taxonomy_id\") VALUES (7, 9)"
            );
            assert_eq!(
                bytes(delete_links(PG, &rel, object, &[taxonomy_id, twelve])),
                b"DELETE FROM \"wp_term_relationships\" WHERE \"object_id\" = 7 AND \"term_taxonomy_id\" IN ('9', '12')"
            );
            let mut bad = Draft::new();
            bad.set(Field::Name, b"a\0b");
            assert_eq!(insert_term(PG, &terms, &bad), Err(LiteralError::Nul));
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
        let opened = tables();
        assert!(opened.is_some());
        if let Some((terms, _, _, _)) = opened {
            let mut draft = Draft::new();
            draft
                .set(Field::Name, b"a'b")
                .set(Field::TermGroup, b"nope");
            let sql = bytes(insert_term(MY, &terms, &draft));
            assert!(sql.windows(6).any(|window| window == br"'a\'b'"));
            assert!(sql.ends_with(b", 0)"));
        }
    }

    proptest::proptest! {
        #[test]
        fn ids_are_decimal_and_unquoted(id in 1_u64..=10_000) {
            let prefix = TablePrefix::new("wp_");
            let terms = TableStem::new("terms");
            let taxonomy = TableStem::new("term_taxonomy");
            let term = TermId::new(id);
            proptest::prop_assert!(prefix.is_ok() && terms.is_ok() && taxonomy.is_ok() && term.is_ok());
            if let (Ok(prefix), Ok(terms), Ok(taxonomy), Ok(term)) = (prefix, terms, taxonomy, term) {
                let select = bytes(select_rows(MY, &prefix.table(&terms), &prefix.table(&taxonomy), term));
                let expected = format!(
                    "SELECT t.*, tt.* FROM wp_terms AS t INNER JOIN wp_term_taxonomy AS tt ON t.term_id = tt.term_id WHERE t.term_id = {id}"
                );
                proptest::prop_assert_eq!(select, expected.as_bytes());
                let count = bytes(count_taxonomies(MY, &prefix.table(&taxonomy), term));
                let expected = format!("SELECT COUNT(*) FROM wp_term_taxonomy WHERE term_id = {id}");
                proptest::prop_assert_eq!(count, expected.as_bytes());
            }
        }
    }
}
