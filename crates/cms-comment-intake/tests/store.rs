//! Exact statements for the lookups and the comment-plus-outbox transaction.

use cms_comment_intake::{
    Approval, Candidate, Error, History, Nudge, RateLimiter, Rejection, Settings, SqlIntake,
    Submission, accept, rate_key,
};
use cms_db::{Cell, Dialect, Reply, Scripted, ServerError, Sql, TablePrefix};

const NOW: i64 = 1_795_000_000;

fn done() -> Reply {
    Reply::Done {
        affected: 0,
        insert_id: 0,
    }
}

fn empty() -> Reply {
    Reply::single_column("x", [])
}

fn one(value: &str) -> Reply {
    Reply::single_column("x", [Cell::bytes(value)])
}

fn intake(dialect: Dialect, steps: Vec<(&str, Reply)>) -> Option<SqlIntake<Scripted>> {
    let prefix = TablePrefix::new("fp_").ok()?;
    let store = Scripted::new(
        steps
            .into_iter()
            .map(|(sql, reply)| (Sql::from(sql), reply)),
    );
    SqlIntake::new(store, dialect, &prefix).ok()
}

fn candidate(approval: Approval) -> Candidate {
    Candidate {
        post_id: 7,
        parent: 0,
        author: "Ada".into(),
        email: "ada@example.com".into(),
        url: "".into(),
        content: "It's".into(),
        ip: "203.0.113.9".into(),
        agent: "UA/1".into(),
        user_id: 0,
        date: "2026-11-18 11:06:40".into(),
        date_gmt: "2026-11-18 11:06:40".into(),
        approval,
    }
}

const POST: &str = r#"SELECT "ID", "post_author", "post_status", "comment_status", "post_type", "post_password", "post_date_gmt" FROM "fp_posts" WHERE "ID" = 7 LIMIT 1"#;
const INSERT: &str = r#"INSERT INTO "fp_comments" ("comment_post_ID", "comment_author_IP", "comment_author", "comment_author_email", "comment_author_url", "comment_date", "comment_date_gmt", "comment_content", "comment_karma", "comment_approved", "comment_agent", "comment_type", "comment_parent", "user_id") VALUES (7, '203.0.113.9', 'Ada', 'ada@example.com', '', '2026-11-18 11:06:40', '2026-11-18 11:06:40', 'It''s', 0, '1', 'UA/1', 'comment', 0, 0) RETURNING "comment_ID""#;
const COUNT: &str = r#"UPDATE "fp_posts" SET "comment_count" = (SELECT COUNT(*) FROM "fp_comments" WHERE "comment_post_ID" = 7 AND "comment_approved" = '1') WHERE "ID" = 7"#;
const OUTBOX: &str = r#"INSERT INTO "fp_outbox" ("aggregate", "aggregate_id", "change", "payload") VALUES ('comment', 41, 'insert', '{"post_id":7,"approved":"1"}')"#;
const DUPLICATE: &str = r#"SELECT "comment_ID" FROM "fp_comments" WHERE "comment_post_ID" = 7 AND "comment_parent" = 0 AND "comment_approved" != 'trash' AND ("comment_author" = 'Ada' OR "comment_author_email" = 'ada@example.com') AND "comment_content" = 'It''s' LIMIT 1"#;
const LAST: &str = r#"SELECT "comment_date_gmt" FROM "fp_comments" WHERE "comment_date_gmt" >= '2026-11-18 10:06:40' AND ("comment_author_IP" = '203.0.113.9' OR "comment_author_email" = 'ada@example.com') ORDER BY "comment_date_gmt" DESC LIMIT 1"#;
const USER: &str = r#"SELECT "ID" FROM "fp_users" WHERE "user_email" = 'ada@example.com' LIMIT 1"#;
const BY_AUTHOR: &str = r#"SELECT "comment_approved" FROM "fp_comments" WHERE "comment_author" = 'Ada' AND "comment_author_email" = 'ada@example.com' AND "comment_approved" = '1' LIMIT 1"#;

#[test]
fn post_rows_are_parsed_by_position() {
    let row = Reply::Rows(cms_db::ResultSet::new(
        vec![cms_db::Column::new("ID")],
        vec![cms_db::Row::new(vec![
            Cell::bytes("7"),
            Cell::bytes("2"),
            Cell::bytes("publish"),
            Cell::bytes("open"),
            Cell::bytes("post"),
            Cell::bytes("secret"),
            Cell::Null,
        ])],
    ));
    let mut sql = intake(Dialect::Postgres, vec![(POST, row), (POST, empty())]);
    assert!(sql.is_some());
    if let Some(sql) = sql.as_mut() {
        let post = sql.post(7);
        assert!(post.as_ref().is_ok_and(Option::is_some));
        if let Ok(Some(post)) = post {
            assert_eq!((post.id, post.author), (7, 2));
            assert_eq!((&*post.status, &*post.comment_status), ("publish", "open"));
            assert!(post.has_password);
            assert_eq!(&*post.date_gmt, "");
        }
        assert_eq!(sql.post(7), Ok(None));
    }
}

#[test]
fn lookups_use_the_dialect_spelling() {
    let parent = r#"SELECT "comment_approved" FROM "fp_comments" WHERE "comment_ID" = 3 LIMIT 1"#;
    let by_user = r#"SELECT "comment_approved" FROM "fp_comments" WHERE "user_id" = 9 AND "comment_approved" = '1' LIMIT 1"#;
    let last_user = r#"SELECT "comment_date_gmt" FROM "fp_comments" WHERE "comment_date_gmt" >= '2026-11-18 10:06:40' AND ("user_id" = 5 OR "comment_author_IP" = '203.0.113.9') ORDER BY "comment_date_gmt" DESC LIMIT 1"#;
    let no_email = r#"SELECT "comment_ID" FROM "fp_comments" WHERE "comment_post_ID" = 7 AND "comment_parent" = 0 AND "comment_approved" != 'trash' AND ("comment_author" = 'Ada') AND "comment_content" = 'It''s' LIMIT 1"#;
    let mut sql = intake(
        Dialect::Postgres,
        vec![
            (parent, one("0")),
            (DUPLICATE, one("12")),
            (no_email, empty()),
            (LAST, one("2026-11-18 11:00:00")),
            (last_user, Reply::single_column("x", [Cell::Null])),
            (USER, one("9")),
            (by_user, one("1")),
            (USER, empty()),
            (BY_AUTHOR, empty()),
        ],
    );
    assert!(sql.is_some());
    if let Some(sql) = sql.as_mut() {
        assert_eq!(sql.comment_status(3), Ok(Some("0".into())));
        let mut c = candidate(Approval::Hold);
        assert_eq!(sql.duplicate(&c), Ok(true));
        c.email = "".into();
        assert_eq!(sql.duplicate(&c), Ok(false));
        let c = candidate(Approval::Hold);
        let since = "2026-11-18 10:06:40";
        assert_eq!(
            sql.last_comment_gmt(&c, since),
            Ok(Some("2026-11-18 11:00:00".into()))
        );
        let mut logged_in = candidate(Approval::Hold);
        logged_in.user_id = 5;
        assert_eq!(sql.last_comment_gmt(&logged_in, since), Ok(None));
        assert_eq!(sql.previously_approved("Ada", "ada@example.com"), Ok(true));
        assert_eq!(sql.previously_approved("Ada", "ada@example.com"), Ok(false));
    }
}

#[test]
fn refused_lookups_and_unstorable_text_are_errors() {
    let failed = Reply::Failed(ServerError::new(1, "relation does not exist"));
    let mut sql = intake(Dialect::Postgres, vec![(POST, failed)]);
    if let Some(sql) = sql.as_mut() {
        assert_eq!(
            sql.post(7),
            Err(Error::Write("relation does not exist".into()))
        );
        // The script is exhausted, so the store now reports itself unreachable.
        assert_eq!(sql.post(7), Err(Error::Unavailable));
        let mut c = candidate(Approval::Hold);
        c.content = "a\0b".into();
        assert_eq!(
            sql.duplicate(&c),
            Err(Error::Literal(cms_db::LiteralError::Nul))
        );
    }
}

#[test]
fn approved_comments_commit_with_count_and_outbox() {
    let returned = Reply::single_column("comment_ID", [Cell::bytes("41")]);
    let mut sql = intake(
        Dialect::Postgres,
        vec![
            ("BEGIN", done()),
            (INSERT, returned),
            (COUNT, done()),
            (OUTBOX, done()),
            ("COMMIT", done()),
        ],
    );
    assert!(sql.is_some());
    if let Some(mut sql) = sql.take() {
        assert_eq!(sql.commit(&candidate(Approval::Approved)), Ok(41));
        assert_eq!(sql.into_store().seen().len(), 5);
    }
}

#[test]
fn held_comments_skip_the_count_and_mysql_reads_the_insert_id() {
    let insert = INSERT
        .replace('"', "`")
        .replace("'It''s'", r"'It\'s'")
        .replace("'1', 'UA/1'", "'0', 'UA/1'")
        .replace(" RETURNING `comment_ID`", "");
    let outbox = r#"INSERT INTO `fp_outbox` (`aggregate`, `aggregate_id`, `change`, `payload`) VALUES ('comment', 8, 'insert', '{\"post_id\":7,\"approved\":\"0\"}')"#;
    let mut sql = intake(
        Dialect::MySql,
        vec![
            ("BEGIN", done()),
            (
                insert.as_str(),
                Reply::Done {
                    affected: 1,
                    insert_id: 8,
                },
            ),
            (outbox, done()),
            ("COMMIT", done()),
        ],
    );
    assert!(sql.is_some());
    if let Some(sql) = sql.as_mut() {
        assert_eq!(sql.commit(&candidate(Approval::Hold)), Ok(8));
    }
}

#[test]
fn failed_writes_roll_back() {
    let refused = Reply::Failed(ServerError::new(
        40001,
        "change conflicts with another transaction",
    ));
    let mut sql = intake(
        Dialect::Postgres,
        vec![("BEGIN", done()), (INSERT, refused), ("ROLLBACK", done())],
    );
    if let Some(mut sql) = sql.take() {
        assert_eq!(
            sql.commit(&candidate(Approval::Approved)),
            Err(Error::Write(
                "change conflicts with another transaction".into()
            ))
        );
        assert_eq!(sql.into_store().seen().last(), Some(&Sql::from("ROLLBACK")));
    }
    let mut sql = intake(
        Dialect::Postgres,
        vec![("BEGIN", done()), (INSERT, done()), ("ROLLBACK", done())],
    );
    if let Some(sql) = sql.as_mut() {
        assert_eq!(
            sql.commit(&candidate(Approval::Approved)),
            Err(Error::Write("insert returned no comment id".into()))
        );
    }
    let mut sql = intake(Dialect::Postgres, vec![]);
    if let Some(sql) = sql.as_mut() {
        assert_eq!(
            sql.commit(&candidate(Approval::Approved)),
            Err(Error::Unavailable)
        );
        let mut c = candidate(Approval::Approved);
        c.agent = "\0".into();
        assert_eq!(
            sql.commit(&c),
            Err(Error::Literal(cms_db::LiteralError::Nul))
        );
    }
}

struct Limiter(u64);

impl RateLimiter for Limiter {
    fn hit(&mut self, key: &str) -> Result<u64, Error> {
        assert_eq!(key, "RL#comments#203.0.113.9#29916666");
        self.0 += 1;
        Ok(self.0)
    }
}

struct Waker(bool);

impl Nudge for Waker {
    fn nudge(&mut self) -> Result<(), Error> {
        if self.0 {
            Ok(())
        } else {
            Err(Error::Port {
                port: "nudge",
                message: "throttled".into(),
            })
        }
    }
}

fn submission() -> Submission {
    Submission {
        post_id: 7,
        author: "Ada".into(),
        email: "ada@example.com".into(),
        content: "It's".into(),
        ip: "203.0.113.9".into(),
        agent: "UA/1".into(),
        ..Submission::default()
    }
}

fn open_post() -> Reply {
    Reply::Rows(cms_db::ResultSet::new(
        vec![cms_db::Column::new("ID")],
        vec![cms_db::Row::new(
            [
                "7",
                "1",
                "publish",
                "open",
                "post",
                "",
                "2026-11-01 00:00:00",
            ]
            .map(Cell::bytes),
        )],
    ))
}

#[test]
fn accept_runs_the_whole_submission() {
    assert_eq!(
        rate_key("203.0.113.9", NOW),
        "RL#comments#203.0.113.9#29916666"
    );
    let settings = Settings::from_values(|_| None);
    assert!(settings.is_ok());
    let Ok(settings) = settings else { return };
    let returned = Reply::single_column("comment_ID", [Cell::bytes("41")]);
    let mut sql = intake(
        Dialect::Postgres,
        vec![
            (POST, open_post()),
            (DUPLICATE, empty()),
            (LAST, empty()),
            (USER, empty()),
            (BY_AUTHOR, one("1")),
            ("BEGIN", done()),
            (INSERT, returned),
            (COUNT, done()),
            (OUTBOX, done()),
            ("COMMIT", done()),
        ],
    );
    if let Some(sql) = sql.as_mut() {
        let outcome = accept(
            &settings,
            sql,
            &mut Limiter(0),
            &mut Waker(false),
            &submission(),
            None,
            NOW,
        );
        assert!(outcome.as_ref().is_ok_and(Result::is_ok));
        if let Ok(Ok(accepted)) = outcome {
            assert_eq!(accepted.id, 41);
            assert_eq!(accepted.candidate.approval, Approval::Approved);
            assert!(accepted.nudge_error.is_some());
        }
    }
    let mut sql = intake(Dialect::Postgres, vec![(POST, empty())]);
    if let Some(sql) = sql.as_mut() {
        let outcome = accept(
            &settings,
            sql,
            &mut Limiter(0),
            &mut Waker(true),
            &submission(),
            None,
            NOW,
        );
        assert_eq!(outcome, Ok(Err(Rejection::PostMissing)));
    }
}

#[test]
fn the_rate_limit_answers_before_any_query() {
    let settings = Settings::from_values(|_| None);
    let Ok(settings) = settings else { return };
    let mut sql = intake(Dialect::Postgres, vec![]);
    if let Some(sql) = sql.as_mut() {
        let outcome = accept(
            &settings,
            sql,
            &mut Limiter(5),
            &mut Waker(true),
            &submission(),
            None,
            NOW,
        );
        assert_eq!(outcome, Ok(Err(Rejection::RateLimited)));
    }
}
