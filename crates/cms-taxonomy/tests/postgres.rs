//! Term writes on a `PostgreSQL` store (serverless profile, ADR-0010).

use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row, Scripted, Sql, TablePrefix};
use cms_taxonomy::{Draft, Error, Field, TermId, TermTaxonomyId, Terms};

fn opened(steps: Vec<(Sql, Reply)>) -> Option<Terms<Scripted>> {
    let prefix = TablePrefix::new("fp_").ok()?;
    Terms::new(Scripted::with_dialect(Dialect::Postgres, steps), &prefix).ok()
}

fn returned(column: &str, id: &str) -> Reply {
    Reply::returned(
        1,
        &ResultSet::new([Column::new(column)], [Row::new([Cell::bytes(id)])]),
    )
}

#[test]
fn postgres_insert_reads_both_returned_ids() {
    let bag = opened(vec![
        (
            Sql::from(
                "INSERT INTO \"fp_terms\" (\"name\", \"slug\", \"term_group\") VALUES ('News', 'news', 0) RETURNING \"term_id\"",
            ),
            returned("term_id", "5"),
        ),
        (
            Sql::from(
                "INSERT INTO \"fp_term_taxonomy\" (\"term_id\", \"taxonomy\", \"description\", \"parent\", \"count\") VALUES (5, 'category', '', 0, 0) RETURNING \"term_taxonomy_id\"",
            ),
            returned("term_taxonomy_id", "6"),
        ),
    ]);
    assert!(bag.is_some());
    if let Some(mut terms) = bag {
        let mut draft = Draft::new();
        draft
            .set(Field::Name, b"News")
            .set(Field::Slug, b"news")
            .set(Field::Taxonomy, b"category");
        let written = terms.insert(&draft);
        assert!(written.as_ref().is_ok_and(|write| {
            write
                .inserted_ids()
                .map(|(term, taxonomy)| (term.as_u64(), taxonomy.as_u64()))
                == Some((5, 6))
        }));
    }
}

#[test]
fn postgres_refuses_nul_before_the_server() {
    let bag = opened(Vec::new());
    let ids = (TermId::new(1), TermTaxonomyId::new(2));
    assert!(bag.is_some());
    if let (Some(mut terms), (Ok(term), Ok(taxonomy))) = (bag, ids) {
        let mut draft = Draft::new();
        draft.set(Field::Name, b"a\0b");
        assert!(matches!(terms.insert(&draft), Err(Error::Literal(_))));
        assert!(matches!(
            terms.update(term, taxonomy, &draft),
            Err(Error::Literal(_))
        ));
    }
}
