//! The DSQL adapter against the in-process fake PostgreSQL server (plain TCP on 127.0.0.1).
//!
//! Frame shapes follow the PostgreSQL v3 protocol documentation ("Message Formats" and "Simple
//! Query"); DSQL conflict codes OC000/OC001 come from the DSQL concurrency-control docs.

use std::time::Duration;

use cms_db::{Cell, Reply, Sql, Store, store_contract};
use infra_dsql::fake::{
    FakeServer, Frames, Greeting, ListedTokens, ManualClock, PlainTcp, Session, test_roots,
};
use infra_dsql::{
    Connector, Dsql, Endpoint, Error, MigrateError, Migration, Retry, Tls, TransactionError,
};

const TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);

type Store_ = Dsql<PlainTcp, ListedTokens>;

fn store(server: &FakeServer, tokens: &[&str], clock: &ManualClock) -> (Store_, ListedTokens) {
    let tokens = ListedTokens::new(tokens, TOKEN_LIFETIME);
    let dsql = Dsql::new(server.endpoint(), PlainTcp, tokens.clone())
        .with_clock(clock.clone())
        .with_seed(7);
    (dsql, tokens)
}

fn step(sql: &str, frames: Frames) -> (&str, Frames) {
    (sql, frames)
}

fn begin() -> (&'static str, Frames) {
    ("BEGIN", Frames::new().complete("BEGIN").ready(b'T'))
}

fn commit() -> (&'static str, Frames) {
    ("COMMIT", Frames::new().complete("COMMIT").ready(b'I'))
}

fn rollback() -> (&'static str, Frames) {
    ("ROLLBACK", Frames::new().complete("ROLLBACK").ready(b'I'))
}

fn in_block(tag: &str) -> Frames {
    Frames::new().complete(tag).ready(b'T')
}

#[test]
fn the_store_contract_holds_over_the_wire() {
    let clock = ManualClock::default();
    let mut server = None;
    store_contract(
        |script: Vec<(Sql, Reply)>| {
            let steps: Vec<(String, Frames)> = script
                .iter()
                .map(|(sql, reply)| {
                    (
                        String::from_utf8_lossy(sql.as_bytes()).into_owned(),
                        Frames::from_reply(reply),
                    )
                })
                .collect();
            let session = Session::new(
                "token-1",
                steps
                    .iter()
                    .map(|(sql, frames)| (sql.as_str(), frames.clone()))
                    .collect(),
            );
            let started = FakeServer::start(vec![session]).expect("fake server binds");
            let (dsql, _) = store(&started, &["token-1"], &clock);
            server = Some(started);
            dsql
        },
        |dsql| assert!(dsql.is_connected()),
    );
    let seen = server.expect("contract built the server").seen();
    assert_eq!(
        seen.logins,
        [(
            "app".to_owned(),
            "postgres".to_owned(),
            "token-1".to_owned()
        )]
    );
    assert_eq!(seen.statements.len(), 3);
}

#[test]
fn nulls_empty_queries_notices_and_multi_results_decode() {
    let server = FakeServer::start(vec![Session::new(
        "t",
        vec![
            step(
                "SELECT a, b",
                Frames::new()
                    .notice()
                    .rows(&["a", "b"], &[&[None, Some("x")]])
                    .complete("SELECT 1")
                    .ready(b'I'),
            ),
            step("", Frames::new().empty().ready(b'I')),
            step(
                "SELECT 1; UPDATE t",
                Frames::new()
                    .rows(&["n"], &[&[Some("1")]])
                    .complete("SELECT 1")
                    .complete("UPDATE 4")
                    .ready(b'I'),
            ),
            step(
                "INSERT RETURNING",
                Frames::new()
                    .rows(&["ID"], &[&[Some("77")]])
                    .complete("INSERT 0 1")
                    .ready(b'I'),
            ),
        ],
    )])
    .expect("fake server binds");
    let (mut dsql, _) = store(&server, &["t"], &ManualClock::default());

    let rows = dsql.query(&Sql::from("SELECT a, b")).expect("rows");
    let set = rows.as_rows().expect("a result set");
    assert_eq!(set.rows()[0].cells(), [Cell::Null, Cell::bytes("x")]);
    assert_eq!(
        dsql.query(&Sql::from("")).ok().and_then(|r| r.as_done()),
        Some((0, 0))
    );
    assert_eq!(
        dsql.query(&Sql::from("SELECT 1; UPDATE t"))
            .ok()
            .and_then(|r| r.as_done()),
        Some((4, 0))
    );
    assert_eq!(
        dsql.query(&Sql::from("INSERT RETURNING"))
            .ok()
            .and_then(|r| r.as_done()),
        Some((1, 77))
    );
    let shown = format!("{dsql:?}");
    assert!(shown.contains("connected: true"));
    assert!(!shown.contains("\"t\""));
}

#[test]
fn reuses_one_connection_then_reconnects_at_max_age_with_a_fresh_token() {
    let ok = || Frames::new().complete("UPDATE 1").ready(b'I');
    let server = FakeServer::start(vec![
        Session::new(
            "first",
            vec![step("UPDATE a", ok()), step("UPDATE b", ok())],
        ),
        Session::new("second", vec![step("UPDATE c", ok())]),
    ])
    .expect("fake server binds");
    let clock = ManualClock::default();
    let (mut dsql, tokens) = store(&server, &["first", "second"], &clock);
    let dsql_ref = &mut dsql;

    assert!(dsql_ref.query(&Sql::from("UPDATE a")).is_ok());
    clock.advance(Duration::from_secs(10 * 60));
    assert!(dsql_ref.query(&Sql::from("UPDATE b")).is_ok());
    assert_eq!(tokens.calls(), 1);

    // Past 55 minutes the connection is replaced; the 15-minute token is long expired too.
    clock.advance(Duration::from_secs(46 * 60));
    assert!(dsql_ref.query(&Sql::from("UPDATE c")).is_ok());
    assert_eq!(tokens.calls(), 2);
    assert_eq!(server.seen().logins.len(), 2);
    dsql.disconnect();
    assert!(!dsql.is_connected());
}

#[test]
fn a_rejected_token_is_replaced_once() {
    let server = FakeServer::start(vec![
        Session::new("good", vec![]),
        Session::new(
            "good",
            vec![step(
                "UPDATE a",
                Frames::new().complete("UPDATE 1").ready(b'I'),
            )],
        ),
    ])
    .expect("fake server binds");
    let (mut dsql, tokens) = store(&server, &["stale", "good"], &ManualClock::default());
    assert!(dsql.query(&Sql::from("UPDATE a")).is_ok());
    assert_eq!(tokens.calls(), 2);

    let refusing = FakeServer::start(vec![
        Session::new("good", vec![]),
        Session::new("good", vec![]),
    ])
    .expect("fake server binds");
    let (mut dsql, tokens) = store(&refusing, &["bad", "worse"], &ManualClock::default());
    assert!(
        dsql.query(&Sql::from("UPDATE a"))
            .is_err_and(|error| error.is_unavailable())
    );
    assert_eq!(tokens.calls(), 2);

    let (mut dsql, _) = store(&refusing, &[], &ManualClock::default());
    assert!(dsql.query(&Sql::from("UPDATE a")).is_err());
}

#[test]
fn unsupported_auth_and_broken_replies_are_unavailable_and_drop_the_connection() {
    let sasl = Session {
        greeting: Greeting::Frames(Frames::new().raw(b'R', &10_i32.to_be_bytes())),
        steps: Vec::new(),
    };
    let odd = Session {
        greeting: Greeting::Frames(Frames::new().raw(b'W', &[])),
        steps: Vec::new(),
    };
    let server = FakeServer::start(vec![
        sasl,
        odd,
        Session::new("t", vec![step("SELECT 1", Frames::new().ready(b'I'))]),
        Session::new("t", vec![step("SELECT 2", Frames::new().raw(b'W', &[]))]),
        Session::new(
            "t",
            vec![step(
                "SELECT 3",
                Frames::new().complete("SELECT 0").ready(b'I'),
            )],
        ),
    ])
    .expect("fake server binds");
    let tokens: Vec<&str> = vec!["t"; 5];
    let (mut dsql, _) = store(&server, &tokens, &ManualClock::default());
    let unavailable = |dsql: &mut Store_, sql: &str| {
        dsql.query(&Sql::from(sql))
            .is_err_and(|e| e.is_unavailable())
    };
    assert!(unavailable(&mut dsql, "SELECT 0"));
    assert!(unavailable(&mut dsql, "SELECT 0"));
    assert!(unavailable(&mut dsql, "SELECT 1"));
    assert!(!dsql.is_connected());
    assert!(unavailable(&mut dsql, "SELECT 2"));
    assert!(dsql.query(&Sql::from("SELECT 3")).is_ok());
    // A NUL cannot travel in a simple query and is refused before sending.
    assert!(unavailable(&mut dsql, "SELECT '\0'"));
}

#[test]
fn transactions_commit_and_rerun_on_conflicts_with_bounded_jitter() {
    let server = FakeServer::start(vec![Session::new(
        "t",
        vec![
            begin(),
            step("UPDATE a", in_block("UPDATE 1")),
            step(
                "COMMIT",
                Frames::new()
                    .error("OC000", "change conflicts with another transaction")
                    .ready(b'I'),
            ),
            begin(),
            step(
                "UPDATE a",
                Frames::new()
                    .error("40001", "could not serialize access")
                    .ready(b'E'),
            ),
            rollback(),
            begin(),
            step("UPDATE a", in_block("UPDATE 1")),
            commit(),
        ],
    )])
    .expect("fake server binds");
    let clock = ManualClock::default();
    let (dsql, _) = store(&server, &["t"], &clock);
    let mut dsql = dsql.with_retry(Retry {
        attempts: 5,
        base: Duration::from_millis(10),
        cap: Duration::from_millis(15),
    });
    let mut runs = 0;
    let committed = dsql.with_transaction(|tx| {
        runs += 1;
        tx.query(&Sql::from("UPDATE a"))
            .map(|reply| reply.as_done())
    });
    assert_eq!(committed.ok(), Some(Some((1, 0))));
    assert_eq!(runs, 3);
    let sleeps = clock.sleeps();
    assert_eq!(sleeps.len(), 2);
    assert!(sleeps[0] <= Duration::from_millis(10));
    assert!(sleeps[1] <= Duration::from_millis(15));
}

#[test]
fn conflicts_past_the_attempt_limit_report_the_last_one() {
    let conflict = || Frames::new().error("OC001", "schema changed").ready(b'I');
    let server = FakeServer::start(vec![Session::new(
        "t",
        vec![
            begin(),
            step("COMMIT", conflict()),
            begin(),
            step("COMMIT", conflict()),
        ],
    )])
    .expect("fake server binds");
    let (dsql, _) = store(&server, &["t"], &ManualClock::default());
    let mut dsql = dsql.with_retry(Retry {
        attempts: 2,
        ..Retry::default()
    });
    let result = dsql.with_transaction(|_tx| Ok::<(), ()>(()));
    assert!(matches!(
        result,
        Err(TransactionError::Conflict(error)) if error.code() == 1213 && error.message().contains("OC001")
    ));
}

#[test]
fn closure_errors_failed_blocks_and_commit_errors_roll_back_or_abort() {
    let server = FakeServer::start(vec![Session::new(
        "t",
        vec![
            begin(),
            rollback(),
            begin(),
            step(
                "INSERT dup",
                Frames::new().error("23505", "duplicate key").ready(b'E'),
            ),
            rollback(),
            begin(),
            step(
                "COMMIT",
                Frames::new().error("XX000", "internal").ready(b'I'),
            ),
            (
                "BEGIN",
                Frames::new().error("XX000", "cannot begin").ready(b'I'),
            ),
            (
                "BEGIN",
                Frames::new()
                    .rows(&["x"], &[])
                    .complete("SELECT 0")
                    .ready(b'I'),
            ),
        ],
    )])
    .expect("fake server binds");
    let (mut dsql, _) = store(&server, &["t"], &ManualClock::default());

    let closure = dsql.with_transaction(|_tx| Err::<(), _>("domain refused"));
    assert!(matches!(
        closure,
        Err(TransactionError::Closure("domain refused"))
    ));

    // The closure ignored a failed statement; COMMIT would silently roll back, so it is refused.
    let ignored = dsql.with_transaction(|tx| tx.query(&Sql::from("INSERT dup")).map(|_| ()));
    assert!(
        matches!(ignored, Err(TransactionError::Aborted(error)) if error.message().contains("25P02"))
    );

    let commit = dsql.with_transaction(|_tx| Ok::<(), ()>(()));
    assert!(matches!(commit, Err(TransactionError::Aborted(error)) if error.code() == 1105));

    let begin = dsql.with_transaction(|_tx| Ok::<(), ()>(()));
    assert!(matches!(begin, Err(TransactionError::Aborted(_))));

    let rows = dsql.with_transaction(|_tx| Ok::<(), ()>(()));
    assert!(matches!(rows, Err(TransactionError::Store(_))));
    assert!(rows.is_err_and(|error| error.to_string().starts_with("transaction lost")));
}

#[test]
fn tls_sessions_verify_the_server_and_carry_queries() {
    let server = FakeServer::start_tls(vec![Session::new(
        "t",
        vec![step(
            "SELECT 1",
            Frames::new()
                .rows(&["n"], &[&[Some("1")]])
                .complete("SELECT 1")
                .ready(b'I'),
        )],
    )])
    .expect("fake server binds");
    let tls = Tls::with_roots(test_roots().expect("test roots load")).expect("tls config");
    let tokens = ListedTokens::new(&["t"], TOKEN_LIFETIME);
    let mut dsql = Dsql::new(server.endpoint(), tls, tokens);
    let reply = dsql.query(&Sql::from("SELECT 1")).expect("query over tls");
    assert_eq!(reply.as_rows().map(|set| set.rows().len()), Some(1));
    let seen = server.seen();
    assert_eq!(seen.ssl_requests, 1);
    assert_eq!(seen.logins[0].2, "t");
}

#[test]
fn tls_refusals_untrusted_servers_and_odd_answers_fail_the_connect() {
    let plain = FakeServer::start(vec![Session::new("t", vec![])]).expect("fake server binds");
    let tls = Tls::with_roots(test_roots().expect("test roots load")).expect("tls config");
    assert!(matches!(
        tls.connect(&plain.endpoint()),
        Err(Error::TlsRefused)
    ));

    let untrusted =
        FakeServer::start_tls(vec![Session::new("t", vec![])]).expect("fake server binds");
    let public_roots = Tls::new().expect("tls config");
    assert!(format!("{public_roots:?}").starts_with("Tls"));
    assert!(matches!(
        public_roots.connect(&untrusted.endpoint()),
        Err(Error::Tls(_))
    ));

    assert!(matches!(
        tls.connect(&Endpoint::new("bad host name", 1, "app", "postgres")),
        Err(Error::Tls(_))
    ));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0_u8; 8];
            let _read = std::io::Read::read_exact(&mut stream, &mut request);
            let _wrote = std::io::Write::write_all(&mut stream, b"X");
        }
    });
    assert!(matches!(
        tls.connect(&Endpoint::new("127.0.0.1", port, "app", "postgres")),
        Err(Error::Protocol(_))
    ));
}

fn core() -> Vec<Migration> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/aws/dsql");
    Migration::load_dir(&dir).expect("migrations parse")
}

fn ledger_steps(migrations: &[Migration]) -> Vec<(String, Frames)> {
    let mut steps = Vec::new();
    for migration in migrations {
        for (index, statement) in migration.statements().iter().enumerate() {
            let text = String::from_utf8_lossy(statement.as_bytes()).into_owned();
            steps.push(("BEGIN".to_owned(), begin().1));
            steps.push((text, in_block("CREATE TABLE")));
            steps.push(("COMMIT".to_owned(), commit().1));
            steps.push(("BEGIN".to_owned(), begin().1));
            steps.push((
                format!(
                    "INSERT INTO \"fp_schema_migrations\" (\"version\", \"statement\") VALUES ('{}', {})",
                    migration.version(),
                    index + 1
                ),
                in_block("INSERT 0 1"),
            ));
            steps.push(("COMMIT".to_owned(), commit().1));
        }
    }
    steps
}

const LEDGER: &str = "SELECT \"version\", \"statement\" FROM \"fp_schema_migrations\"";

#[test]
fn migrations_apply_once_and_skip_recorded_statements() {
    let migrations = core();
    let mut first: Vec<(String, Frames)> = vec![(
        LEDGER.to_owned(),
        Frames::new()
            .error("42P01", "relation \"fp_schema_migrations\" does not exist")
            .ready(b'I'),
    )];
    first.extend(ledger_steps(&migrations));
    let numbers: Vec<String> = (1..=44).map(|n| n.to_string()).collect();
    let recorded: Vec<[Option<&str>; 2]> = numbers
        .iter()
        .map(|n| [Some("0001-core"), Some(n.as_str())])
        .collect();
    let rows: Vec<&[Option<&str>]> = recorded.iter().map(|row| &row[..]).collect();
    let second = vec![(
        LEDGER.to_owned(),
        Frames::new()
            .rows(&["version", "statement"], &rows)
            .complete("SELECT 44")
            .ready(b'I'),
    )];
    let as_session = |steps: &[(String, Frames)]| {
        Session::new(
            "t",
            steps
                .iter()
                .map(|(sql, frames)| (sql.as_str(), frames.clone()))
                .collect(),
        )
    };
    let server = FakeServer::start(vec![as_session(&first)]).expect("fake server binds");
    let (mut dsql, _) = store(&server, &["t"], &ManualClock::default());
    let applied = dsql.migrate(&migrations).expect("first run applies");
    assert_eq!((applied.applied, applied.skipped), (44, 0));

    let server = FakeServer::start(vec![as_session(&second)]).expect("fake server binds");
    let (mut dsql, _) = store(&server, &["t"], &ManualClock::default());
    let applied = dsql.migrate(&migrations).expect("second run skips");
    assert_eq!((applied.applied, applied.skipped), (0, 44));
    assert_eq!(server.seen().statements.len(), 1);
}

#[test]
fn migration_failures_stop_the_run() {
    let one = Migration::parse("0002-x", "CREATE TABLE \"x\" (\"a\" text);\n").expect("parses");
    let empty_ledger = || {
        Frames::new()
            .rows(&["version", "statement"], &[])
            .complete("SELECT 0")
            .ready(b'I')
    };
    let server = FakeServer::start(vec![
        Session::new(
            "t",
            vec![
                (LEDGER, empty_ledger()),
                begin(),
                (
                    "CREATE TABLE \"x\" (\"a\" text)",
                    Frames::new().error("42P07", "relation exists").ready(b'E'),
                ),
                rollback(),
            ],
        ),
        Session::new(
            "t",
            vec![(LEDGER, Frames::new().complete("UPDATE 0").ready(b'I'))],
        ),
        Session::new(
            "t",
            vec![(
                LEDGER,
                Frames::new()
                    .rows(&["version", "statement"], &[&[Some("v"), Some("one")]])
                    .complete("SELECT 1")
                    .ready(b'I'),
            )],
        ),
        Session::new(
            "t",
            vec![
                (LEDGER, empty_ledger()),
                begin(),
                (
                    "CREATE TABLE \"x\" (\"a\" text)",
                    Frames::new().raw(b'W', &[]),
                ),
            ],
        ),
    ])
    .expect("fake server binds");
    let (mut dsql, _) = store(&server, &["t", "t", "t", "t"], &ManualClock::default());
    let failed = dsql.migrate(std::slice::from_ref(&one));
    assert!(matches!(
        failed,
        Err(MigrateError::Statement { statement: 1, error, .. }) if error.code() == 1050
    ));
    dsql.disconnect();
    assert!(matches!(
        dsql.migrate(std::slice::from_ref(&one)),
        Err(MigrateError::BadLedger)
    ));
    dsql.disconnect();
    assert!(matches!(
        dsql.migrate(std::slice::from_ref(&one)),
        Err(MigrateError::BadLedger)
    ));
    dsql.disconnect();
    assert!(matches!(
        dsql.migrate(std::slice::from_ref(&one)),
        Err(MigrateError::Transaction(_))
    ));
    assert!(matches!(
        dsql.migrate(std::slice::from_ref(&one)),
        Err(MigrateError::Store(_))
    ));
}
