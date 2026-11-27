//! Comment writes on a `PostgreSQL` store (serverless profile, ADR-0010).

use cms_comments::{CommentId, Comments, Draft, Error, Field};
use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row, Scripted, Sql, TablePrefix};

fn opened(steps: Vec<(Sql, Reply)>) -> Option<Comments<Scripted>> {
    let prefix = TablePrefix::new("fp_").ok()?;
    Comments::new(Scripted::with_dialect(Dialect::Postgres, steps), &prefix).ok()
}

#[test]
fn postgres_insert_reads_the_returned_id() {
    let insert = Sql::from(
        "INSERT INTO \"fp_comments\" (\"comment_post_ID\", \"comment_author_IP\", \"comment_author\", \"comment_author_email\", \"comment_author_url\", \"comment_date\", \"comment_date_gmt\", \"comment_content\", \"comment_karma\", \"comment_approved\", \"comment_agent\", \"comment_type\", \"comment_parent\", \"user_id\") VALUES (0, '', '', '', '', '', '', 'hi', 0, '', '', '', 0, 0) RETURNING \"comment_ID\"",
    );
    let returned = ResultSet::new([Column::new("comment_ID")], [Row::new([Cell::bytes("9")])]);
    let bag = opened(vec![(insert, Reply::returned(1, &returned))]);
    assert!(bag.is_some());
    if let Some(mut comments) = bag {
        let mut draft = Draft::new();
        draft.set(Field::Content, b"hi");
        let written = comments.insert(&draft);
        assert!(
            written
                .as_ref()
                .is_ok_and(|write| write.inserted_id().map(CommentId::as_u64) == Some(9))
        );
    }
}

#[test]
fn postgres_refuses_nul_before_the_server() {
    let bag = opened(Vec::new());
    let id = CommentId::new(1);
    assert!(bag.is_some() && id.is_ok());
    if let (Some(mut comments), Ok(id)) = (bag, id) {
        let mut draft = Draft::new();
        draft.set(Field::Content, b"a\0b");
        assert!(matches!(comments.insert(&draft), Err(Error::Literal(_))));
        assert!(matches!(
            comments.update(id, &draft),
            Err(Error::Literal(_))
        ));
    }
}
