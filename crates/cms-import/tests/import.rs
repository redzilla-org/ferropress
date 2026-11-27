//! Import runs against the in-memory fake store: round trip, re-import, resume and failures.

mod common;

use common::{FakeStore, MapFetcher, MemoryCheckpoint, MemoryUploads, Record};
use cms_db::{Dialect, TablePrefix};
use cms_import::{ImportError, Options, Ports, Report, import};

const SITE: &str = include_str!("fixtures/site.xml");
const PHOTO_URL: &str = "https://old.example.com/wp-content/uploads/2026/01/tower.jpg";

fn options() -> Options {
    let Ok(prefix) = TablePrefix::new("fp_") else {
        unreachable!("a literal prefix without nul");
    };
    let mut options = Options::new(prefix, Dialect::Postgres, "site/imports/example.xml");
    options.target_url = Some("https://new.example.org".to_owned());
    options.registered = "2026-11-27 00:00:00".to_owned();
    options
}

fn seeded() -> FakeStore {
    let mut store = FakeStore::default();
    store.seed("fp_users", &[("ID", "1"), ("user_login", "admin")]);
    store.seed(
        "fp_posts",
        &[("ID", "14"), ("guid", "seeded"), ("post_type", "post")],
    );
    store
}

fn fetcher() -> MapFetcher {
    let mut fetcher = MapFetcher::default();
    fetcher
        .files
        .insert(PHOTO_URL.to_owned(), b"jpeg bytes".to_vec());
    fetcher
}

struct Host {
    store: FakeStore,
    checkpoint: MemoryCheckpoint,
    fetcher: MapFetcher,
    uploads: MemoryUploads,
}

impl Host {
    fn new() -> Self {
        Self {
            store: seeded(),
            checkpoint: MemoryCheckpoint::default(),
            fetcher: fetcher(),
            uploads: MemoryUploads::default(),
        }
    }

    fn run(&mut self, options: &Options, xml: &str) -> Result<Report, ImportError> {
        let ports = Ports {
            checkpoint: &mut self.checkpoint,
            fetcher: &mut self.fetcher,
            uploads: &mut self.uploads,
        };
        import(&mut self.store, ports, options, xml.as_bytes())
    }
}

/// The first `table` row whose `key` is `value`.
fn row<'a>(host: &'a Host, table: &str, key: &str, value: &str) -> Option<&'a Record> {
    host.store
        .tables
        .get(table)?
        .iter()
        .find(|row| row.get(key).map(Vec::as_slice) == Some(value.as_bytes()))
}

fn cell(record: &Record, column: &str) -> String {
    record
        .get(column)
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default()
}

#[test]
fn a_synthetic_export_lands_with_source_ids_links_and_rewritten_urls() {
    let mut host = Host::new();
    let report = host.run(&options(), SITE).expect("import");

    assert_eq!(report.records, 12);
    assert_eq!((report.authors_created, report.authors_matched), (2, 1));
    assert_eq!((report.terms_created, report.terms_matched), (5, 0));
    assert_eq!(report.posts_created.get("post"), Some(&2));
    assert_eq!(report.posts_created.get("page"), Some(&2));
    assert_eq!(report.posts_created.get("attachment"), Some(&1));
    assert_eq!(report.posts_matched, 0);
    assert_eq!(report.attachments_fetched, 1);
    assert_eq!(report.postmeta_rows, 2);
    assert_eq!(report.termmeta_rows, 1);
    assert_eq!((report.comments_created, report.commentmeta_rows), (2, 1));
    assert_eq!(report.links_created, 5);
    assert_eq!(report.fixups, 3);
    assert_eq!(report.batches, 1);
    assert_eq!(report.event.change, "rebuild-all");
    assert_eq!(
        report.event.payload,
        "{\"import\":\"site/imports/example.xml\"}"
    );
    let max: Vec<(&str, u64)> = report.max_ids.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(
        max,
        vec![
            ("commentmeta", 1),
            ("comments", 8),
            ("postmeta", 2),
            ("posts", 15),
            ("term_taxonomy", 7),
            ("termmeta", 1),
            ("terms", 7),
            ("users", 3),
        ]
    );
    assert_eq!(host.store.advanced.len(), 8);
    assert!(host.store.advanced.contains(
        &"SELECT setval(pg_get_serial_sequence('\"fp_posts\"', 'ID'), 15)".to_owned()
    ));
    assert_eq!(host.checkpoint.records, Some(12));

    let get = |table: &str, key: &str, value: &str| {
        row(&host, table, key, value).unwrap_or_else(|| panic!("{table} {key}={value}"))
    };

    // Users: a free source id is kept, a missing creator is created without a password.
    let editor = get("fp_users", "user_login", "editor");
    assert_eq!(cell(editor, "ID"), "2");
    assert_eq!(cell(editor, "display_name"), "The Editor");
    assert_eq!(cell(editor, "user_pass"), "");
    assert_eq!(cell(editor, "user_registered"), "2026-11-27 00:00:00");
    assert_eq!(cell(get("fp_users", "user_login", "newcomer"), "ID"), "3");

    // Posts: source ids kept, the taken one moved past the maximum, parents fixed up.
    let attachment = get("fp_posts", "ID", "10");
    assert_eq!(cell(attachment, "post_parent"), "11");
    assert_eq!(
        cell(attachment, "guid"),
        "https://new.example.org/wp-content/uploads/2026/01/tower.jpg"
    );
    assert_eq!(
        host.uploads.objects.get("2026/01/tower.jpg").map(Vec::as_slice),
        Some(&b"jpeg bytes"[..])
    );
    let post = get("fp_posts", "ID", "11");
    assert_eq!(
        cell(post, "post_content"),
        "<p>Views &amp; light.</p><img src=\"https://new.example.org/wp-content/uploads/2026/01/tower.jpg\" alt=\"\" /><a href=\"https://elsewhere.example.net/wp-content/uploads/x.pdf\">x</a>"
    );
    assert_eq!(cell(post, "post_excerpt"), "Short &lt;summary&gt;");
    assert_eq!(cell(post, "post_author"), "2");
    assert_eq!(cell(post, "comment_count"), "1");
    let draft = get("fp_posts", "ID", "12");
    assert_eq!(cell(draft, "post_parent"), "13");
    assert_eq!(cell(draft, "post_date_gmt"), "0000-00-00 00:00:00");
    assert_eq!(cell(draft, "post_modified"), "0000-00-00 00:00:00");
    assert_eq!(cell(draft, "post_status"), "draft");
    assert_eq!(cell(draft, "menu_order"), "2");
    assert_eq!(cell(draft, "post_author"), "3");
    assert_eq!(
        cell(get("fp_posts", "guid", "https://old.example.com/?p=14"), "ID"),
        "15"
    );

    // Terms: the child declared before its parent points at it; undeclared terms are created.
    let local = get("fp_term_taxonomy", "term_taxonomy_id", "4");
    assert_eq!(cell(local, "parent"), "3");
    assert_eq!(cell(get("fp_terms", "slug", "extra"), "term_id"), "7");
    assert_eq!(
        cell(get("fp_terms", "slug", "places"), "name"),
        "Places &amp; Spaces"
    );
    assert_eq!(cell(get("fp_termmeta", "term_id", "6"), "meta_value"), "blue");
    assert_eq!(host.store.tables["fp_term_relationships"].len(), 5);

    // Meta and comments: the edit lock is dropped, replies point at their parents.
    assert_eq!(host.store.tables["fp_postmeta"].len(), 2);
    let reply = get("fp_comments", "comment_ID", "7");
    assert_eq!(cell(reply, "comment_parent"), "8");
    assert_eq!(cell(reply, "comment_type"), "comment");
    assert_eq!(cell(reply, "comment_post_ID"), "11");
    assert_eq!(
        cell(get("fp_comments", "comment_ID", "8"), "user_id"),
        "2"
    );
    assert_eq!(
        cell(get("fp_commentmeta", "comment_id", "7"), "meta_key"),
        "rating"
    );
}

#[test]
fn a_second_import_of_the_same_file_writes_nothing() {
    let mut host = Host::new();
    host.run(&options(), SITE).expect("first import");
    let before = host.store.dump();
    let inserts = host.store.inserts;

    // A fresh checkpoint, as for a second upload of the same file.
    host.checkpoint = MemoryCheckpoint::default();
    let again = host.run(&options(), SITE).expect("second import");
    assert_eq!(host.store.dump(), before);
    assert_eq!(host.store.inserts, inserts);
    assert!(again.posts_created.is_empty());
    assert_eq!(again.posts_matched, 5);
    assert_eq!((again.authors_created, again.authors_matched), (0, 2));
    assert_eq!((again.terms_created, again.terms_matched), (0, 4));
    assert_eq!(again.attachments_fetched, 0);
    assert_eq!(host.fetcher.fetched.len(), 1);
    assert_eq!(again.max_ids.get("posts"), Some(&15));
    assert_eq!(again.event.change, "rebuild-all");

    // Running past a finished checkpoint only looks rows up.
    let resumed = host.run(&options(), SITE).expect("replay");
    assert_eq!(resumed.resumed_from, 12);
    assert_eq!(host.store.dump(), before);
}

#[test]
fn a_run_that_dies_anywhere_resumes_to_the_same_rows() {
    let mut small = options();
    small.batch_rows = 6;
    let mut clean = Host::new();
    let report = clean.run(&small, SITE).expect("clean run");
    assert!(report.batches > 2, "batches {}", report.batches);
    let expected = clean.store.dump();
    let total = clean.store.statements;

    for fail_at in 1..=total {
        let mut host = Host::new();
        host.store.fail_at = Some(fail_at);
        let first = host.run(&small, SITE);
        assert!(
            matches!(first, Err(ImportError::Store(_))),
            "statement {fail_at}: {first:?}"
        );
        host.store.crash();
        host.store.fail_at = None;
        let resumed = host.run(&small, SITE);
        assert!(resumed.is_ok(), "statement {fail_at}: {resumed:?}");
        assert_eq!(host.store.dump(), expected, "statement {fail_at}");
        assert_eq!(host.uploads.objects, clean.uploads.objects);
    }
}

#[test]
fn failures_stop_the_run_with_their_cause() {
    let mut host = Host::new();
    host.checkpoint.fail = true;
    assert!(matches!(
        host.run(&options(), SITE),
        Err(ImportError::Checkpoint(_))
    ));

    let mut host = Host::new();
    host.fetcher.files.clear();
    assert!(matches!(
        host.run(&options(), SITE),
        Err(ImportError::Fetch { url, .. }) if url.as_ref() == PHOTO_URL
    ));

    let mut host = Host::new();
    host.uploads.fail = true;
    assert!(matches!(
        host.run(&options(), SITE),
        Err(ImportError::Upload { path, .. }) if path.as_ref() == "2026/01/tower.jpg"
    ));

    let mut host = Host::new();
    let mut tiny = options();
    tiny.batch_bytes = 100;
    assert!(matches!(
        host.run(&tiny, SITE),
        Err(ImportError::RecordTooLarge { index: 1, .. })
    ));

    let mut host = Host::new();
    host.checkpoint.records = Some(100);
    assert!(matches!(
        host.run(&options(), SITE),
        Err(ImportError::CheckpointMismatch(1))
    ));

    let no_guid = SITE.replacen(
        "<guid isPermaLink=\"false\">example:draft-guide</guid>",
        "",
        1,
    );
    let mut host = Host::new();
    assert!(matches!(
        host.run(&options(), &no_guid),
        Err(ImportError::MissingGuid(12))
    ));

    let outside = SITE
        .replacen(
            "<wp:attachment_url><![CDATA[https://old.example.com/wp-content/uploads/2026/01/tower.jpg]]>",
            "<wp:attachment_url><![CDATA[https://cdn.example.net/tower.jpg]]>",
            1,
        )
        .replacen("_wp_attached_file", "_other", 1);
    let mut host = Host::new();
    assert!(matches!(
        host.run(&options(), &outside),
        Err(ImportError::AttachmentPath(10))
    ));

    // A taken source comment id moves past the maximum instead of colliding.
    let mut host = Host::new();
    host.store.seed("fp_comments", &[("comment_ID", "7")]);
    host.run(&options(), SITE).expect("import beside a taken comment id");
    assert_eq!(
        row(&host, "fp_comments", "comment_ID", "8").map(|reply| cell(reply, "comment_parent")),
        Some("9".to_owned())
    );

    let mut host = Host::new();
    assert!(matches!(
        host.run(&options(), "<rss><channel><item><wp:post_id>x</wp:post_id></item></channel></rss>"),
        Err(ImportError::Parse(_))
    ));
}

#[test]
fn server_rejections_and_odd_replies_are_errors() {
    use cms_db::{Reply, ServerError, Sql, Store, StoreError};

    struct Always(Reply);
    impl Store for Always {
        fn query(&mut self, _sql: &Sql) -> Result<Reply, StoreError> {
            Ok(self.0.clone())
        }
    }
    let run = |reply: Reply| {
        let mut store = Always(reply);
        let mut checkpoint = MemoryCheckpoint::default();
        let mut fetcher = fetcher();
        let mut uploads = MemoryUploads::default();
        let ports = Ports {
            checkpoint: &mut checkpoint,
            fetcher: &mut fetcher,
            uploads: &mut uploads,
        };
        import(&mut store, ports, &options(), SITE.as_bytes())
    };
    assert!(matches!(
        run(Reply::Failed(ServerError::new(40001, "conflict"))),
        Err(ImportError::Server { code: 40001, .. })
    ));
    assert!(matches!(
        run(Reply::Done {
            affected: 1,
            insert_id: 0
        }),
        Err(ImportError::Reply(_))
    ));
    assert!(matches!(
        run(Reply::single_column("ID", [cms_db::Cell::bytes("x")])),
        Err(ImportError::Reply(_))
    ));
    assert!(matches!(
        run(Reply::Rows(cms_db::ResultSet::new(
            vec![cms_db::Column::new("ID")],
            vec![cms_db::Row::new(Vec::new())]
        ))),
        Err(ImportError::Reply(_))
    ));
}
