//! Imports a real export file named by `CMS_IMPORT_WXR` into the fake store and prints the
//! counts. Ignored by default: the file is private and never part of the repository.
//!
//! `CMS_IMPORT_WXR=/path/to/export.xml cargo test -p cms-import --test real_export -- --ignored --nocapture`

#[expect(dead_code, reason = "this target uses only part of the shared fakes")]
mod common;

use std::fs::File;
use std::io::BufReader;

use cms_db::{Dialect, TablePrefix};
use cms_import::{Options, Ports, import};
use common::{FakeStore, MapFetcher, MemoryCheckpoint, MemoryUploads};

#[test]
#[ignore = "needs CMS_IMPORT_WXR pointing at a local export file"]
fn a_real_export_imports_and_reimports_cleanly() {
    let path = std::env::var("CMS_IMPORT_WXR").expect("CMS_IMPORT_WXR");
    let options = Options::new(
        TablePrefix::new("fp_").expect("prefix"),
        Dialect::Postgres,
        "site/imports/export.xml",
    );
    let mut store = FakeStore::default();
    let run = |store: &mut FakeStore| {
        let mut checkpoint = MemoryCheckpoint::default();
        let mut fetcher = MapFetcher::default();
        let mut uploads = MemoryUploads::default();
        let ports = Ports {
            checkpoint: &mut checkpoint,
            fetcher: &mut fetcher,
            uploads: &mut uploads,
        };
        let input = BufReader::new(File::open(&path).expect("open export"));
        import(store, ports, &options, input).expect("import")
    };
    let first = run(&mut store);
    let rows: Vec<(String, usize)> = store
        .tables
        .iter()
        .map(|(table, rows)| (table.clone(), rows.len()))
        .collect();
    let before = store.dump();
    let inserts = store.inserts;
    let second = run(&mut store);
    println!("first run: {first:#?}");
    println!("rows per table: {rows:?}");
    println!(
        "second run: posts_created={:?} posts_matched={} inserts_added={}",
        second.posts_created,
        second.posts_matched,
        store.inserts - inserts
    );
    assert_eq!(store.dump(), before);
    assert_eq!(store.inserts, inserts);
}
