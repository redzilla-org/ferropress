//! `Projector::run_once` against recording fakes: write order, failure, replay and batching.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::error::Error;
use std::rc::Rc;

use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row as DbRow, Sql, Store, StoreError};
use cms_db::TablePrefix;
use cms_projector::{
    Applied, BlobWriter, Clock, Invalidator, Item, ItemKey, Native, Nudger, Paths, PortError,
    PostDoc, ProjectError, Projector, Report, RouteWriter, Source,
};
use cms_readmodel::{BasePath, BlobKey};
use cms_render::{Author, NoCards, Post, PostKind, Site, Stamp, Term};

type Outcome = Result<(), Box<dyn Error>>;

// One shared log so the test can see the order across every port.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    S3(Box<str>, Vec<u8>),
    Ddb(Box<str>),
    Invalidate(Vec<Box<str>>),
    OutboxDelete,
    Nudge,
}

type Log = Rc<RefCell<Vec<Call>>>;

// The outbox: answers the fetch with fixed rows, records the delete.
struct Outbox {
    rows: Vec<DbRow>,
    log: Log,
}

impl Store for Outbox {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn query(&mut self, sql: &Sql) -> Result<Reply, StoreError> {
        if sql.as_bytes().starts_with(b"SELECT") {
            let columns: Vec<Column> = ["id", "created", "aggregate", "aggregate_id", "change", "payload"]
                .into_iter()
                .map(Column::new)
                .collect();
            return Ok(Reply::Rows(ResultSet::new(columns, self.rows.clone())));
        }
        self.log.borrow_mut().push(Call::OutboxDelete);
        Ok(Reply::Done {
            affected: 1,
            insert_id: 0,
        })
    }
}

struct Posts(Vec<Post>);

impl Source for Posts {
    fn site(&mut self) -> Result<Site, PortError> {
        Ok(site())
    }

    fn published(&mut self) -> Result<Vec<Post>, PortError> {
        Ok(self.0.clone())
    }
}

struct Bucket {
    objects: BTreeMap<Box<str>, Vec<u8>>,
    log: Log,
}

impl BlobWriter for Bucket {
    fn put(&mut self, key: &BlobKey, bytes: &[u8], _content_type: &str) -> Result<(), PortError> {
        self.objects.insert(key.as_str().into(), bytes.to_vec());
        self.log
            .borrow_mut()
            .push(Call::S3(key.as_str().into(), bytes.to_vec()));
        Ok(())
    }
}

// A version-gated table, as `attribute_not_exists(version) OR version < :new` behaves.
struct Table {
    items: BTreeMap<Box<str>, (Box<str>, Item)>,
    log: Log,
}

fn spell(key: &ItemKey) -> Box<str> {
    format!("{key:?}").into()
}

impl RouteWriter for Table {
    fn post_doc(&mut self, id: u64) -> Result<Option<PostDoc>, PortError> {
        Ok(match self.items.get(&spell(&ItemKey::Doc(id))) {
            Some((_, Item::Doc(doc))) => Some(doc.clone()),
            _ => None,
        })
    }

    fn put(&mut self, item: &Item, version: &str) -> Result<Applied, PortError> {
        self.log.borrow_mut().push(Call::Ddb(spell(&item.key())));
        let key = spell(&item.key());
        if self.items.get(&key).is_some_and(|(stored, _)| &**stored >= version) {
            return Ok(Applied::Stale);
        }
        self.items.insert(key, (version.into(), item.clone()));
        Ok(Applied::Written)
    }

    fn delete(&mut self, key: &ItemKey, version: &str) -> Result<Applied, PortError> {
        self.log.borrow_mut().push(Call::Ddb(spell(key)));
        match self.items.get(&spell(key)) {
            Some((stored, _)) if &**stored < version => {
                self.items.remove(&spell(key));
                Ok(Applied::Written)
            }
            _ => Ok(Applied::Stale),
        }
    }
}

struct Edge {
    fail: bool,
    log: Log,
}

impl Invalidator for Edge {
    fn invalidate(&mut self, paths: &[Box<str>]) -> Result<(), PortError> {
        if self.fail {
            return Err(PortError("cloudfront unavailable".into()));
        }
        self.log.borrow_mut().push(Call::Invalidate(paths.to_vec()));
        Ok(())
    }
}

struct Fixed;

impl Clock for Fixed {
    fn now_micros(&self) -> i64 {
        // 2026-11-27 12:34:56 UTC, 60 seconds after the outbox rows were created.
        1_795_782_896_000_000
    }
}

struct Again(Log);

impl Nudger for Again {
    fn nudge(&mut self) -> Result<(), PortError> {
        self.0.borrow_mut().push(Call::Nudge);
        Ok(())
    }
}

fn site() -> Site {
    Site {
        title: "Example".into(),
        description: "Notes".into(),
        home: "https://example.com/blog/".into(),
        base: "/blog/".into(),
        language: "en-US".into(),
        posts_per_page: 10,
        nav: Vec::new(),
    }
}

fn stamp(text: &str) -> Result<Stamp, Box<dyn Error>> {
    Ok(Stamp::parse(text.as_bytes())?)
}

fn post(id: u64, kind: PostKind) -> Result<Post, Box<dyn Error>> {
    Ok(Post {
        id,
        kind,
        title: format!("Post {id}").into(),
        slug: format!("post-{id}").into(),
        permalink: format!("https://example.com/blog/post-{id}/").into(),
        guid: format!("https://example.com/blog/?p={id}").into(),
        date: stamp("2026-11-27 10:00:00")?,
        date_gmt: stamp("2026-11-27 18:00:00")?,
        modified: stamp("2026-11-27 10:00:00")?,
        modified_gmt: stamp("2026-11-27 18:00:00")?,
        author: Author {
            name: "Ann".into(),
            slug: "ann".into(),
            link: "https://example.com/blog/author/ann/".into(),
        },
        categories: vec![Term {
            name: "News".into(),
            slug: "news".into(),
            link: "https://example.com/blog/category/news/".into(),
        }],
        tags: Vec::new(),
        excerpt: "".into(),
        content: "Hello [[92014]] world.".into(),
    })
}

// An outbox row for `post <id> saved`, created one minute before the fixed clock.
fn saved(row: &str, id: u64) -> DbRow {
    DbRow::new(vec![
        Cell::bytes(row),
        Cell::bytes("1795782836000000"),
        Cell::bytes("post"),
        Cell::bytes(id.to_string()),
        Cell::bytes("saved"),
        Cell::bytes("{}"),
    ])
}

struct World {
    log: Log,
    outbox: Outbox,
    source: Posts,
    bucket: Bucket,
    table: Table,
    edge: Edge,
    prefix: TablePrefix,
    paths: Paths,
}

impl World {
    fn new(rows: Vec<DbRow>) -> Result<Self, Box<dyn Error>> {
        let log = Log::default();
        Ok(Self {
            outbox: Outbox {
                rows,
                log: Rc::clone(&log),
            },
            source: Posts(vec![post(1, PostKind::Post)?, post(2, PostKind::Page)?]),
            bucket: Bucket {
                objects: BTreeMap::new(),
                log: Rc::clone(&log),
            },
            table: Table {
                items: BTreeMap::new(),
                log: Rc::clone(&log),
            },
            edge: Edge {
                fail: false,
                log: Rc::clone(&log),
            },
            prefix: TablePrefix::new("fp_")?,
            paths: Paths::new(BasePath::new("/blog/")?),
            log,
        })
    }

    fn run(&mut self) -> Result<Report, ProjectError> {
        let mut nudger = Again(Rc::clone(&self.log));
        Projector {
            store: &mut self.outbox,
            prefix: &self.prefix,
            paths: &self.paths,
            source: &mut self.source,
            theme: &Native,
            cards: &mut NoCards,
            blobs: &mut self.bucket,
            routes: &mut self.table,
            invalidator: &mut self.edge,
            clock: &Fixed,
            nudger: &mut nudger,
        }
        .run_once()
    }

    fn calls(&self) -> Vec<Call> {
        self.log.borrow().clone()
    }
}

// The positions of every call matching `wanted`.
fn positions(calls: &[Call], wanted: fn(&Call) -> bool) -> Vec<usize> {
    calls
        .iter()
        .enumerate()
        .filter(|(_, call)| wanted(call))
        .map(|(index, _)| index)
        .collect()
}

#[test]
fn objects_then_items_then_one_invalidation_then_the_delete() -> Outcome {
    let mut world = World::new(vec![saved("a", 1)])?;
    let report = world.run()?;
    let calls = world.calls();
    let s3 = positions(&calls, |call| matches!(call, Call::S3(..)));
    let ddb = positions(&calls, |call| matches!(call, Call::Ddb(_)));
    let edge = positions(&calls, |call| matches!(call, Call::Invalidate(_)));
    let delete = positions(&calls, |call| *call == Call::OutboxDelete);
    assert!(!s3.is_empty() && !ddb.is_empty());
    assert_eq!((edge.len(), delete.len()), (1, 1));
    assert!(s3.iter().max() < ddb.iter().min());
    assert!(ddb.iter().max() < edge.first());
    assert!(edge.first() < delete.first());
    // Every object is content-addressed under the rendered/ or fragments/ folders.
    assert!(world.bucket.objects.keys().all(|key| {
        key.starts_with("rendered/") || key.starts_with("fragments/shell/") || key.starts_with("search/")
    }));
    // The post's page, its listings and the feed are among the invalidated paths.
    for path in ["/blog/post-1/", "/blog/", "/blog/category/news/", "/blog/feed/"] {
        assert!(report.invalidated.iter().any(|p| &**p == path), "{path}");
    }
    assert!(!report.invalidated.iter().any(|p| &**p == "SEARCH"));
    assert_eq!(report.rows, 1);
    assert_eq!(report.age_seconds, 60);
    // The ZIP marker has no card: once on the page, once in each feed that carries the post.
    assert!(report.unresolved >= 1);
    assert_eq!(report.stale, 0);
    assert!(!report.nudged);
    Ok(())
}

#[test]
fn a_failed_invalidation_leaves_the_rows_for_the_next_run() -> Outcome {
    let mut world = World::new(vec![saved("a", 1)])?;
    world.edge.fail = true;
    assert_eq!(
        world.run(),
        Err(ProjectError::Port(PortError("cloudfront unavailable".into())))
    );
    assert!(!world.calls().contains(&Call::OutboxDelete));
    Ok(())
}

#[test]
fn replaying_a_batch_at_the_same_clock_changes_nothing() -> Outcome {
    let mut world = World::new(vec![saved("a", 1)])?;
    let first = world.run()?;
    let (objects, items) = (world.bucket.objects.clone(), world.table.items.clone());
    let puts = |calls: &[Call]| -> Vec<Call> {
        calls
            .iter()
            .filter(|call| matches!(call, Call::S3(..)))
            .cloned()
            .collect()
    };
    let first_puts = puts(&world.calls());
    world.log.borrow_mut().clear();
    let second = world.run()?;
    assert_eq!(world.bucket.objects, objects);
    assert_eq!(world.table.items, items);
    assert_eq!(puts(&world.calls()), first_puts);
    assert_eq!(second.written, 0);
    assert_eq!(second.stale, first.written + first.stale);
    assert_eq!(second.invalidated, first.invalidated);
    Ok(())
}

#[test]
fn a_site_wide_event_invalidates_the_base_path() -> Outcome {
    let row = DbRow::new(vec![
        Cell::bytes("s"),
        Cell::bytes("1795782836000000"),
        Cell::bytes("site"),
        Cell::bytes("0"),
        Cell::bytes("rebuild-all"),
        Cell::bytes("{}"),
    ]);
    let mut world = World::new(vec![row])?;
    let report = world.run()?;
    assert_eq!(report.invalidated, vec![Box::<str>::from("/blog/*")]);
    // Every published post and page is re-rendered.
    for id in [1_u64, 2] {
        assert!(world.table.items.contains_key(&spell(&ItemKey::Doc(id))));
    }
    Ok(())
}

#[test]
fn an_empty_outbox_does_nothing_and_a_full_batch_nudges() -> Outcome {
    let mut empty = World::new(Vec::new())?;
    assert_eq!(empty.run()?, Report::default());
    assert!(empty.calls().is_empty());
    let rows = (0..100).map(|n| saved(&format!("r{n:03}"), 1)).collect();
    let mut full = World::new(rows)?;
    let report = full.run()?;
    assert!(report.nudged);
    assert_eq!(full.calls().last(), Some(&Call::Nudge));
    Ok(())
}
