//! An S3 event drives one processing run through in-process fakes.

use std::collections::BTreeMap;
use std::io::Cursor;

use cms_db::{Cell, Dialect, Reply, Sql, Store, StoreError, TablePrefix};
use cms_media::{Clock, Object, Objects, PortError, Site};
use image::{DynamicImage, ImageFormat, RgbImage};
use lambda_media::process_invocation;

#[derive(Default)]
struct Bucket(BTreeMap<String, Object>);

impl Objects for Bucket {
    fn get(&mut self, key: &str) -> Result<Option<Object>, PortError> {
        Ok(self.0.get(key).cloned())
    }

    fn put(&mut self, key: &str, _content_type: &str, bytes: &[u8]) -> Result<(), PortError> {
        let object = Object {
            bytes: bytes.to_vec(),
            metadata: Vec::new(),
        };
        self.0.insert(key.to_owned(), object);
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<(), PortError> {
        self.0.remove(key);
        Ok(())
    }

    fn list(&mut self, prefix: &str) -> Result<Vec<String>, PortError> {
        Ok(self.0.keys().filter(|k| k.starts_with(prefix)).cloned().collect())
    }
}

// Free slugs, post id 40, every write acknowledged.
struct Db;

impl Store for Db {
    fn query(&mut self, sql: &Sql) -> Result<Reply, StoreError> {
        let text = sql.as_bytes();
        Ok(if text.starts_with(b"SELECT") {
            Reply::single_column("post_name", [])
        } else if text.starts_with(br#"INSERT INTO "fp_posts""#) {
            Reply::single_column("ID", [Cell::bytes("40")])
        } else {
            Reply::Done {
                affected: 1,
                insert_id: 0,
            }
        })
    }
}

#[test]
fn s3_event_processes_each_record_once() {
    let mut png = Vec::new();
    let written = DynamicImage::ImageRgb8(RgbImage::new(400, 300))
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png);
    assert!(written.is_ok());
    let key = "site/incoming/00000000-0000-4000-8000-000000000000/my pic.png";
    let mut bucket = Bucket::default();
    bucket.0.insert(
        key.to_owned(),
        Object {
            bytes: png,
            metadata: vec![("fp-author".into(), "2".into())],
        },
    );
    let prefix = TablePrefix::new("fp_");
    assert!(prefix.is_ok());
    let Ok(prefix) = prefix else { return };
    let site = Site {
        prefix,
        dialect: Dialect::Postgres,
        uploads_url: "https://example.com/blog/wp-content/uploads".into(),
        comment_status: "open".into(),
    };
    let clock = Clock {
        local: "2026-01-05 08:00:00".into(),
        gmt: "2026-01-05 08:00:00".into(),
    };
    let event = br#"{"Records":[{"s3":{"object":{"key":"site/incoming/00000000-0000-4000-8000-000000000000/my+pic.png"}}}]}"#;

    let first = process_invocation(event, &site, &clock, &mut bucket, &mut Db);
    assert_eq!(
        first.map(String::from_utf8),
        Ok(Ok(format!(r#"{{"processed":[{{"id":40,"key":"{key}"}}]}}"#)))
    );
    assert!(bucket.0.contains_key("site/uploads/2026/01/my pic-300x225.png"));
    assert!(!bucket.0.contains_key(key));

    // Redelivery after success is a no-op.
    let again = process_invocation(event, &site, &clock, &mut bucket, &mut Db);
    assert_eq!(
        again.map(String::from_utf8),
        Ok(Ok(format!(r#"{{"processed":[{{"id":null,"key":"{key}"}}]}}"#)))
    );

    assert_eq!(
        process_invocation(b"{}", &site, &clock, &mut bucket, &mut Db),
        Err("invocation is not an S3 event: Records".to_owned())
    );
    let bad = br#"{"Records":[{"s3":{"object":{"key":"site/other/x.png"}}}]}"#;
    assert_eq!(
        process_invocation(bad, &site, &clock, &mut bucket, &mut Db),
        Err("site/other/x.png: not an incoming upload key: site/other/x.png".to_owned())
    );
}
