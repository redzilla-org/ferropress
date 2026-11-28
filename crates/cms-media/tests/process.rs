//! End-to-end processing of generated fixture images through in-process fakes.

use std::collections::BTreeMap;
use std::io::Cursor;

use cms_db::{Cell, Dialect, Reply, ServerError, Sql, Store, StoreError, TablePrefix};
use cms_media::{Clock, Error, Metadata, Object, Objects, PortError, Processed, Site, process};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

const UUID: &str = "00000000-0000-4000-8000-000000000000";

// A gradient so every encoder has real content to compress.
fn fixture(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let byte = |value: u32| u8::try_from(value % 256).unwrap_or_default();
    let pixels = RgbImage::from_fn(width, height, |x, y| Rgb([byte(x), byte(y), byte(x + y)]));
    let image = match format {
        ImageFormat::Gif | ImageFormat::WebP => {
            DynamicImage::ImageRgba8(DynamicImage::ImageRgb8(pixels).to_rgba8())
        }
        _ => DynamicImage::ImageRgb8(pixels),
    };
    let mut out = Vec::new();
    let written = image.write_to(&mut Cursor::new(&mut out), format);
    assert!(written.is_ok(), "{written:?}");
    out
}

#[derive(Default)]
struct Bucket {
    objects: BTreeMap<String, Object>,
    deleted: Vec<String>,
}

impl Bucket {
    fn with(mut self, key: &str, bytes: Vec<u8>, author: Option<&str>) -> Self {
        let metadata = author
            .map(|id| vec![("fp-author".to_owned(), id.to_owned())])
            .unwrap_or_default();
        self.objects
            .insert(key.to_owned(), Object { bytes, metadata });
        self
    }

    fn size_of(&self, key: &str) -> Option<(u32, u32, u64)> {
        let object = self.objects.get(key)?;
        let image = image::load_from_memory(&object.bytes).ok()?;
        Some((image.width(), image.height(), object.bytes.len() as u64))
    }
}

impl Objects for Bucket {
    fn get(&mut self, key: &str) -> Result<Option<Object>, PortError> {
        Ok(self.objects.get(key).cloned())
    }

    fn put(&mut self, key: &str, content_type: &str, bytes: &[u8]) -> Result<(), PortError> {
        assert!(content_type.starts_with("image/"));
        self.objects.insert(
            key.to_owned(),
            Object {
                bytes: bytes.to_vec(),
                metadata: Vec::new(),
            },
        );
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<(), PortError> {
        self.objects.remove(key);
        self.deleted.push(key.to_owned());
        Ok(())
    }

    fn list(&mut self, prefix: &str) -> Result<Vec<String>, PortError> {
        Ok(self
            .objects
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
}

// Answers like DSQL: slug checks find `taken`, the post insert returns id 12.
#[derive(Default)]
struct Db {
    seen: Vec<String>,
    taken: Vec<String>,
    reject: Option<&'static str>,
    down: bool,
}

impl Store for Db {
    fn query(&mut self, sql: &Sql) -> Result<Reply, StoreError> {
        if self.down {
            return Err(StoreError::Unavailable);
        }
        let text = String::from_utf8_lossy(sql.as_bytes()).into_owned();
        self.seen.push(text.clone());
        if self.reject.is_some_and(|needle| text.contains(needle)) {
            return Ok(Reply::Failed(ServerError::new(40001, "change conflicts")));
        }
        if text.starts_with("SELECT") {
            let hit = self
                .taken
                .iter()
                .any(|slug| text.contains(&format!("= '{slug}' ")));
            let cells = if hit {
                vec![Cell::bytes("x")]
            } else {
                Vec::new()
            };
            return Ok(Reply::single_column("post_name", cells));
        }
        if text.starts_with(r#"INSERT INTO "fp_posts""#) {
            return Ok(Reply::single_column("ID", [Cell::bytes("12")]));
        }
        Ok(Reply::Done {
            affected: 1,
            insert_id: 0,
        })
    }
}

fn site() -> Option<Site> {
    Some(Site {
        prefix: TablePrefix::new("fp_").ok()?,
        dialect: Dialect::Postgres,
        uploads_url: "https://example.com/blog/wp-content/uploads".into(),
        comment_status: "open".into(),
    })
}

fn clock() -> Clock {
    Clock {
        local: "2026-11-27 10:00:00".into(),
        gmt: "2026-11-27 18:00:00".into(),
    }
}

fn incoming(name: &str) -> String {
    format!("site/incoming/{UUID}/{name}")
}

fn run(key: &str, bucket: &mut Bucket, db: &mut Db) -> Result<Option<Processed>, Error> {
    let site = site().ok_or(Error::Literal)?;
    process(key, &site, &clock(), bucket, db)
}

fn sizes(meta: &Metadata) -> Vec<(&str, &str, u32, u32)> {
    meta.sizes
        .iter()
        .map(|s| (s.name, s.file.as_str(), s.width, s.height))
        .collect()
}

#[test]
fn big_jpeg_is_scaled_and_gets_every_sub_size() {
    let key = incoming("photo.jpg");
    let mut bucket = Bucket::default().with(&key, fixture(2600, 100, ImageFormat::Jpeg), Some("7"));
    let mut db = Db::default();
    let done = run(&key, &mut bucket, &mut db);
    assert!(matches!(done, Ok(Some(_))), "{done:?}");
    let Ok(Some(done)) = done else { return };

    let dir = "site/uploads/2026/11/";
    let files = [
        "photo.jpg",
        "photo-scaled.jpg",
        "photo-300x12.jpg",
        "photo-1024x39.jpg",
        "photo-150x100.jpg",
        "photo-768x30.jpg",
        "photo-1536x59.jpg",
        "photo-2048x79.jpg",
    ];
    assert_eq!(done.keys, files.map(|file| format!("{dir}{file}")).to_vec());
    assert_eq!(done.id, 12);
    assert_eq!(done.attached_file, "2026/11/photo-scaled.jpg");
    let meta = &done.metadata;
    assert_eq!((meta.width, meta.height), (2560, 98));
    assert_eq!(meta.file, "2026/11/photo-scaled.jpg");
    assert_eq!(meta.original_image.as_deref(), Some("photo.jpg"));
    assert_eq!(
        bucket.size_of(&format!("{dir}photo-scaled.jpg")),
        Some((2560, 98, meta.filesize))
    );
    assert_eq!(
        sizes(meta),
        [
            ("medium", "photo-300x12.jpg", 300, 12),
            ("large", "photo-1024x39.jpg", 1024, 39),
            ("thumbnail", "photo-150x100.jpg", 150, 100),
            ("medium_large", "photo-768x30.jpg", 768, 30),
            ("1536x1536", "photo-1536x59.jpg", 1536, 59),
            ("2048x2048", "photo-2048x79.jpg", 2048, 79),
        ]
    );
    // Every recorded size matches the stored file's real dimensions and byte count.
    for size in &meta.sizes {
        assert_eq!(
            bucket.size_of(&format!("{dir}{}", size.file)),
            Some((size.width, size.height, size.filesize)),
            "{}",
            size.name
        );
        assert_eq!(size.mime, "image/jpeg");
    }
    assert_eq!(bucket.deleted, [key.clone()]);
    assert!(!bucket.objects.contains_key(&key));

    let serialized = String::from_utf8_lossy(&meta.serialize()).into_owned();
    assert!(serialized.starts_with(
        r#"a:7:{s:5:"width";i:2560;s:6:"height";i:98;s:4:"file";s:24:"2026/11/photo-scaled.jpg";"#
    ));
    assert!(serialized.contains(r#"s:6:"medium";a:5:{s:4:"file";s:16:"photo-300x12.jpg";s:5:"width";i:300;s:6:"height";i:12;s:9:"mime-type";s:10:"image/jpeg";"#));
    assert!(serialized.ends_with(r#"s:14:"original_image";s:9:"photo.jpg";}"#));

    assert_eq!(db.seen.len(), 7);
    assert_eq!(db.seen[0], "BEGIN");
    assert_eq!(
        db.seen[1],
        r#"SELECT "post_name" FROM "fp_posts" WHERE "post_name" = 'photo' AND "ID" != 0 LIMIT 1"#
    );
    assert!(db.seen[2].ends_with(
        r#"VALUES (7, '2026-11-27 10:00:00', '2026-11-27 18:00:00', '', '', 'photo', '', 'inherit', 'attachment', 'open', 'closed', '', 'photo', '', '', '2026-11-27 10:00:00', '2026-11-27 18:00:00', 0, 0, 'image/jpeg', 'https://example.com/blog/wp-content/uploads/2026/11/photo.jpg') RETURNING "ID""#
    ));
    assert_eq!(
        db.seen[3],
        r#"INSERT INTO "fp_postmeta" ("post_id", "meta_key", "meta_value") VALUES (12, '_wp_attached_file', '2026/11/photo-scaled.jpg')"#
    );
    assert_eq!(
        db.seen[4],
        format!(
            r#"INSERT INTO "fp_postmeta" ("post_id", "meta_key", "meta_value") VALUES (12, '_wp_attachment_metadata', '{}')"#,
            serialized.replace('\'', "''")
        )
    );
    assert!(db.seen[5].starts_with(r#"INSERT INTO "fp_outbox""#));
    assert_eq!(db.seen[6], "COMMIT");
}

#[test]
fn small_png_takes_a_unique_name_and_only_the_thumbnail() {
    let key = incoming("a.png");
    let mut bucket = Bucket::default()
        .with(&key, fixture(200, 100, ImageFormat::Png), Some("3"))
        .with("site/uploads/2026/11/a.png", vec![1], None);
    let mut db = Db::default();
    let done = run(&key, &mut bucket, &mut db);
    assert!(matches!(done, Ok(Some(_))), "{done:?}");
    let Ok(Some(done)) = done else { return };
    assert_eq!(done.attached_file, "2026/11/a-1.png");
    assert_eq!(done.metadata.original_image, None);
    assert_eq!((done.metadata.width, done.metadata.height), (200, 100));
    assert_eq!(
        sizes(&done.metadata),
        [("thumbnail", "a-1-150x100.png", 150, 100)]
    );
    assert_eq!(
        bucket.size_of("site/uploads/2026/11/a-1-150x100.png"),
        Some((150, 100, done.metadata.sizes[0].filesize))
    );
    // The title keeps the client's name; only the file is renumbered.
    assert!(db.seen[2].contains("'a', '', 'inherit', 'attachment'"));
    assert!(db.seen[2].contains("/2026/11/a-1.png'"));
}

#[test]
fn gif_sizes_follow_the_reference_rules() {
    let key = incoming("anim.gif");
    let mut bucket = Bucket::default().with(&key, fixture(1000, 800, ImageFormat::Gif), Some("1"));
    let done = run(&key, &mut bucket, &mut Db::default());
    assert!(matches!(done, Ok(Some(_))), "{done:?}");
    let Ok(Some(done)) = done else { return };
    assert_eq!(
        sizes(&done.metadata),
        [
            ("medium", "anim-300x240.gif", 300, 240),
            ("thumbnail", "anim-150x150.gif", 150, 150),
            ("medium_large", "anim-768x614.gif", 768, 614),
        ]
    );
    for size in &done.metadata.sizes {
        assert_eq!(
            bucket.size_of(&format!("site/uploads/2026/11/{}", size.file)),
            Some((size.width, size.height, size.filesize))
        );
    }
}

#[test]
fn webp_slug_conflicts_get_numbered() {
    let key = incoming("w.webp");
    let mut bucket = Bucket::default().with(&key, fixture(800, 600, ImageFormat::WebP), Some("1"));
    let mut db = Db {
        taken: vec!["w".into(), "w-2".into()],
        ..Db::default()
    };
    let done = run(&key, &mut bucket, &mut db);
    assert!(matches!(done, Ok(Some(_))), "{done:?}");
    let Ok(Some(done)) = done else { return };
    assert_eq!(
        sizes(&done.metadata),
        [
            ("medium", "w-300x225.webp", 300, 225),
            ("thumbnail", "w-150x150.webp", 150, 150),
            ("medium_large", "w-768x576.webp", 768, 576),
        ]
    );
    assert!(db.seen[1].contains("= 'w' "));
    assert!(db.seen[2].contains("= 'w-2' "));
    assert!(db.seen[3].contains("= 'w-3' "));
    assert!(db.seen[4].contains("'inherit', 'attachment', 'open', 'closed', '', 'w-3',"));
}

#[test]
fn a_rejected_commit_rolls_back_and_removes_written_files() {
    let key = incoming("a.png");
    let mut bucket = Bucket::default().with(&key, fixture(200, 100, ImageFormat::Png), Some("3"));
    let mut db = Db {
        reject: Some("fp_outbox"),
        ..Db::default()
    };
    let done = run(&key, &mut bucket, &mut db);
    assert_eq!(
        done.map(|d| d.map(|d| d.id)),
        Err(Error::Rejected("change conflicts".into()))
    );
    assert_eq!(db.seen.last().map(String::as_str), Some("ROLLBACK"));
    assert!(!db.seen.iter().any(|sql| sql == "COMMIT"));
    assert_eq!(
        bucket.deleted,
        [
            "site/uploads/2026/11/a.png",
            "site/uploads/2026/11/a-150x100.png"
        ]
    );
    assert!(bucket.objects.contains_key(&key));
    assert_eq!(bucket.objects.len(), 1);
}

#[test]
fn unavailable_store_is_reported() {
    let key = incoming("a.png");
    let mut bucket = Bucket::default().with(&key, fixture(200, 100, ImageFormat::Png), Some("3"));
    let mut db = Db {
        down: true,
        ..Db::default()
    };
    assert_eq!(
        run(&key, &mut bucket, &mut db).map(|d| d.is_some()),
        Err(Error::Unavailable)
    );
    assert!(bucket.objects.contains_key(&key));
}

#[test]
fn refused_inputs() {
    let mut db = Db::default();
    // Already processed: nothing to do.
    assert_eq!(
        run(&incoming("gone.png"), &mut Bucket::default(), &mut db).map(|d| d.is_none()),
        Ok(true)
    );
    let png = fixture(20, 10, ImageFormat::Png);
    let cases = [
        ("a.jpg", Some("1"), Error::Decode("jpeg")),
        ("a.bmp", Some("1"), Error::FileType),
        ("a.png", None, Error::Forbidden),
        ("a.png", Some("x"), Error::Forbidden),
    ];
    for (name, author, expected) in cases {
        let key = incoming(name);
        let mut bucket = Bucket::default().with(&key, png.clone(), author);
        assert_eq!(
            run(&key, &mut bucket, &mut db).map(|d| d.is_some()),
            Err(expected),
            "{name}"
        );
    }
    let key = incoming("t.png");
    let mut bucket =
        Bucket::default().with(&key, b"\x89PNG\r\n\x1a\ntruncated".to_vec(), Some("1"));
    assert_eq!(
        run(&key, &mut bucket, &mut db).map(|d| d.is_some()),
        Err(Error::Decode("png"))
    );
    assert_eq!(
        run("site/other/x.png", &mut Bucket::default(), &mut db).map(|d| d.is_some()),
        Err(Error::Key("site/other/x.png".into()))
    );
    assert!(db.seen.is_empty());

    let site = site();
    assert!(site.is_some());
    if let Some(site) = site {
        let key = incoming("a.png");
        let mut bucket = Bucket::default().with(&key, png, Some("1"));
        let bad = Clock {
            local: "tomorrow".into(),
            gmt: String::new(),
        };
        assert_eq!(
            process(&key, &site, &bad, &mut bucket, &mut db).map(|d| d.is_some()),
            Err(Error::Clock("tomorrow".into()))
        );
    }
}
