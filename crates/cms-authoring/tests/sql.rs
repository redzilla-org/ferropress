//! The production repository over a recording Postgres store: a publish is one BEGIN ... COMMIT
//! holding the post row, its term links and exactly one shared-shape outbox row.

use cms_authoring::{
    Clock, Editor, Error, Permissions, PostInput, Projector, Site, SqlRepo, Status,
};
use cms_db::{Dialect, Reply, ResultSet, Sql, Store, StoreError, TablePrefix};
use cms_users::UserId;

// Answers every statement plausibly and records its text: selects are empty, inserts return
// id 7, and a statement containing `fail` (if set) is rejected by the server.
struct Recording {
    statements: Vec<String>,
    fail: Option<&'static str>,
}

impl Store for Recording {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn query(&mut self, sql: &Sql) -> Result<Reply, StoreError> {
        let text = String::from_utf8_lossy(sql.as_bytes()).into_owned();
        self.statements.push(text.clone());
        if self.fail.is_some_and(|fail| text.contains(fail)) {
            return Ok(Reply::Failed(cms_db::ServerError::new(0, "rejected")));
        }
        Ok(if text.starts_with("SELECT") {
            Reply::Rows(ResultSet::new(Vec::new(), Vec::new()))
        } else {
            Reply::Done {
                affected: 1,
                insert_id: if text.starts_with("INSERT") { 7 } else { 0 },
            }
        })
    }
}

struct Everything;

impl Permissions for Everything {
    fn has(&mut self, _user: UserId, _required: &[&[u8]]) -> Result<bool, Error> {
        Ok(true)
    }
}

#[derive(Default)]
struct Nudges(u32);

impl Projector for Nudges {
    fn nudge(&mut self) -> Result<(), Error> {
        self.0 += 1;
        Ok(())
    }
}

struct Fixed;

impl Clock for Fixed {
    fn now(&self) -> i64 {
        1_795_780_800
    }
}

fn site() -> Site {
    Site {
        theme: cms_render::Site {
            title: "Example".into(),
            description: String::new().into(),
            home: "https://example.test/blog/".into(),
            base: "/blog/".into(),
            language: "en-US".into(),
            posts_per_page: 10,
            nav: Vec::new(),
        },
        gmt_offset: 0,
    }
}

fn publish(store: &mut Recording, nudges: &mut Nudges) -> Result<(), Error> {
    let prefix = TablePrefix::new("fp_").map_err(|_bad| Error::Unavailable)?;
    let user = UserId::new(1).map_err(|_zero| Error::Unavailable)?;
    let mut repo = SqlRepo::new(store, &prefix);
    let site = site();
    let mut everything = Everything;
    let mut editor = Editor::new(&mut repo, &mut everything, nudges, &Fixed, &site);
    let input = PostInput {
        title: Some("Hello World".into()),
        content: Some("<p>Hi</p>".into()),
        status: Some(Status::Publish),
        tags: Some(cms_authoring::Tags::Names(vec!["Rust".into()])),
        ..PostInput::default()
    };
    editor.save(user, None, &input).map(|_saved| ())
}

#[test]
fn a_publish_is_one_transaction_with_one_outbox_row() {
    let mut store = Recording {
        statements: Vec::new(),
        fail: None,
    };
    let mut nudges = Nudges::default();
    assert_eq!(publish(&mut store, &mut nudges), Ok(()));
    let statements = &store.statements;
    assert_eq!(statements.first().map(String::as_str), Some("BEGIN"));
    assert_eq!(statements.last().map(String::as_str), Some("COMMIT"));
    let position = |needle: &str| statements.iter().position(|s| s.contains(needle));
    let post = position(r#"INSERT INTO "fp_posts""#);
    let link = position(r#"INSERT INTO "fp_term_relationships""#);
    let outbox = position(r#"INSERT INTO "fp_outbox""#);
    assert!(
        post.is_some() && link.is_some() && outbox.is_some(),
        "{statements:#?}"
    );
    assert!(post < link && link < outbox);
    let outbox_rows: Vec<&String> = statements
        .iter()
        .filter(|s| s.contains(r#""fp_outbox""#))
        .collect();
    assert_eq!(outbox_rows.len(), 1);
    let row = outbox_rows[0];
    assert!(row.contains("'post', 7, 'saved'"), "{row}");
    assert!(
        row.contains(r#""action":"published""#) && row.contains("::jsonb"),
        "{row}"
    );
    assert!(row.contains(r#""slug":"hello-world""#), "{row}");
    assert_eq!(nudges.0, 1);
}

#[test]
fn a_rejected_statement_rolls_back_and_skips_the_nudge() {
    let mut store = Recording {
        statements: Vec::new(),
        fail: Some(r#"INSERT INTO "fp_outbox""#),
    };
    let mut nudges = Nudges::default();
    let outcome = publish(&mut store, &mut nudges);
    assert!(matches!(outcome, Err(Error::Rejected(_))), "{outcome:?}");
    assert_eq!(
        store.statements.last().map(String::as_str),
        Some("ROLLBACK")
    );
    assert!(!store.statements.iter().any(|s| s == "COMMIT"));
    assert_eq!(nudges.0, 0);
}
