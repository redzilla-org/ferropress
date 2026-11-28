//! Post reads and writes on a `PostgreSQL` store (serverless profile, ADR-0010).

use cms_content::{Draft, Error, Field, PostId, Posts, Write};
use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row, Scripted, Sql, TablePrefix};

fn opened(steps: Vec<(Sql, Reply)>) -> Option<Posts<Scripted>> {
    let prefix = TablePrefix::new("fp_").ok()?;
    Posts::new(Scripted::with_dialect(Dialect::Postgres, steps), &prefix).ok()
}

fn row(id: &str, name: &str) -> Reply {
    Reply::Rows(ResultSet::new(
        [Column::new("ID"), Column::new("post_name")],
        [Row::new([Cell::bytes(id), Cell::bytes(name)])],
    ))
}

#[test]
fn postgres_insert_reads_the_returned_id_and_lookups_fold_case() {
    let mut draft = Draft::new();
    draft.set(Field::Author, b"1").set(Field::Name, b"hello");
    let insert = Sql::from(
        "INSERT INTO \"fp_posts\" (\"post_author\", \"post_date\", \"post_date_gmt\", \"post_content\", \"post_content_filtered\", \"post_title\", \"post_excerpt\", \"post_status\", \"post_type\", \"comment_status\", \"ping_status\", \"post_password\", \"post_name\", \"to_ping\", \"pinged\", \"post_modified\", \"post_modified_gmt\", \"post_parent\", \"menu_order\", \"post_mime_type\", \"guid\") VALUES (1, '', '', '', '', '', '', '', '', '', '', '', 'hello', '', '', '', '', 0, 0, '', '') RETURNING \"ID\"",
    );
    let returned = ResultSet::new([Column::new("ID")], [Row::new([Cell::bytes("42")])]);
    let by_name =
        Sql::from("SELECT * FROM \"fp_posts\" WHERE lower(\"post_name\") = lower('hello') LIMIT 1");
    let bag = opened(vec![
        (insert, Reply::returned(1, &returned)),
        (by_name, row("42", "hello")),
        (
            Sql::from("DELETE FROM \"fp_posts\" WHERE \"ID\" = 42"),
            Reply::Done {
                affected: 1,
                insert_id: 0,
            },
        ),
    ]);
    assert!(bag.is_some());
    if let Some(mut posts) = bag {
        let written = posts.insert(&draft);
        assert!(
            written
                .as_ref()
                .is_ok_and(|write| write.inserted_id().map(PostId::as_u64) == Some(42))
        );
        let found = posts.by_name(b"hello");
        assert!(
            found
                .as_ref()
                .is_ok_and(|post| post.as_ref().is_some_and(|post| post.id().as_u64() == 42))
        );
        let id = PostId::new(42);
        assert!(id.is_ok());
        if let Ok(id) = id {
            assert!(posts.delete(id).is_ok_and(|write| write == Write::Removed));
        }
    }
}

#[test]
fn postgres_refuses_nul_before_the_server() {
    let bag = opened(Vec::new());
    let id = PostId::new(1);
    assert!(bag.is_some() && id.is_ok());
    if let (Some(mut posts), Ok(id)) = (bag, id) {
        let mut draft = Draft::new();
        draft.set(Field::Content, b"a\0b");
        assert!(matches!(posts.insert(&draft), Err(Error::Literal(_))));
        assert!(matches!(posts.update(id, &draft), Err(Error::Literal(_))));
    }
}
