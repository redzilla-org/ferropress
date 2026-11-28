//! Incoming upload to reference upload files plus one attachment transaction.

use std::io::Cursor;

use cms_content::{Draft, Field};
use cms_db::{Dialect, Reply, Sql, Store, TablePrefix, TableStem};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView};

use crate::meta::{Metadata, SizeEntry};
use crate::name::{sanitize_slug, scaled_name, sized_name, split, unique_stem};
use crate::sizes::{BIG_IMAGE_THRESHOLD, DEFAULT_SIZES, constrain, resize_plan};
use crate::sql::{Statements, Tables};
use crate::{Error, ImageKind, Objects};

const INCOMING: &str = "site/incoming/";
const UPLOADS: &str = "site/uploads/";
// The reference's default JPEG quality for generated files.
const JPEG_QUALITY: u8 = 82;

/// Site settings the processor needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// Table prefix (`fp_`).
    pub prefix: TablePrefix,
    /// The store's SQL spelling.
    pub dialect: Dialect,
    /// Public uploads URL without a trailing slash (`https://example.com/blog/wp-content/uploads`).
    pub uploads_url: String,
    /// `default_comment_status` for new attachments (`open` by default).
    pub comment_status: String,
}

/// The upload instant as the reference stores it, `YYYY-MM-DD hh:mm:ss`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clock {
    /// Site-local time; also picks the `YYYY/MM` upload directory.
    pub local: String,
    /// UTC.
    pub gmt: String,
}

/// What one processed upload produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Processed {
    /// The attachment post id.
    pub id: u64,
    /// `_wp_attached_file`.
    pub attached_file: String,
    /// `_wp_attachment_metadata` before serializing.
    pub metadata: Metadata,
    /// Every upload key written, original first.
    pub keys: Vec<String>,
}

/// Processes the incoming object at `key`. `Ok(None)` means it is already gone (a redelivered
/// event after success).
///
/// Files are written first, then the rows commit in one transaction, then the incoming object
/// is deleted. A failed transaction is rolled back and the written files are removed.
///
/// # Errors
///
/// [`Error::Key`], [`Error::FileType`], [`Error::Decode`], [`Error::Forbidden`] (no signed
/// author), [`Error::Clock`], [`Error::Encode`], [`Error::Rejected`], [`Error::Unavailable`],
/// [`Error::Literal`] and [`Error::Port`].
pub fn process(
    key: &str,
    site: &Site,
    clock: &Clock,
    objects: &mut impl Objects,
    store: &mut impl Store,
) -> Result<Option<Processed>, Error> {
    let name = incoming_name(key)?;
    let Some(object) = objects.get(key)? else {
        return Ok(None);
    };
    let (stem, extension) = split(name);
    let kind = ImageKind::from_extension(extension).ok_or(Error::FileType)?;
    let image = decode(&object.bytes, kind)?;
    let author = object
        .metadata
        .iter()
        .find(|(name, _)| name == "fp-author")
        .and_then(|(_, value)| value.parse::<u64>().ok())
        .ok_or(Error::Forbidden)?;
    let subdir = month_dir(&clock.local)?;
    let files = render(
        &image,
        &object.bytes,
        kind,
        stem,
        extension,
        &subdir,
        objects,
    )?;
    let row = AttachmentRow {
        author,
        title: stem,
        mime: kind.mime(),
        guid: format!("{}/{subdir}/{}", site.uploads_url, files.original),
    };
    let committed = commit(site, clock, &row, &files.metadata, store);
    let id = match committed {
        Ok(id) => id,
        Err(error) => {
            // Leave no orphan files behind a rolled-back row; a retry starts clean.
            for written in &files.keys {
                let _ = objects.delete(written);
            }
            return Err(error);
        }
    };
    objects.delete(key)?;
    Ok(Some(Processed {
        id,
        attached_file: files.metadata.file.clone(),
        metadata: files.metadata,
        keys: files.keys,
    }))
}

// `site/incoming/<36-char uuid>/<name>` with a plain name.
fn incoming_name(key: &str) -> Result<&str, Error> {
    let refused = || Error::Key(key.into());
    let rest = key.strip_prefix(INCOMING).ok_or_else(refused)?;
    let (uuid, name) = rest.split_once('/').ok_or_else(refused)?;
    if uuid.len() != 36 || name.is_empty() || name.contains('/') {
        return Err(refused());
    }
    Ok(name)
}

// The bytes must really be the type the extension names.
fn decode(bytes: &[u8], kind: ImageKind) -> Result<DynamicImage, Error> {
    match image::guess_format(bytes) {
        Ok(format) if format == kind.format() => {}
        _ => return Err(Error::Decode(kind.label())),
    }
    image::load_from_memory_with_format(bytes, kind.format())
        .map_err(|_| Error::Decode(kind.label()))
}

// `YYYY/MM` from `YYYY-MM-DD hh:mm:ss`.
fn month_dir(local: &str) -> Result<String, Error> {
    let bytes = local.as_bytes();
    let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    let shaped = bytes.len() == 19
        && digits.iter().all(|&i| bytes[i].is_ascii_digit())
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b' '
        && bytes[13] == b':'
        && bytes[16] == b':';
    if !shaped {
        return Err(Error::Clock(local.into()));
    }
    Ok(format!("{}/{}", &local[..4], &local[5..7]))
}

struct Files {
    original: String,
    metadata: Metadata,
    keys: Vec<String>,
}

// Writes the original, the optional `-scaled` copy and every sub-size, and builds the metadata.
fn render(
    image: &DynamicImage,
    bytes: &[u8],
    kind: ImageKind,
    stem: &str,
    extension: &str,
    subdir: &str,
    objects: &mut impl Objects,
) -> Result<Files, Error> {
    let directory = format!("{UPLOADS}{subdir}/");
    let listed = objects.list(&directory)?;
    let existing: Vec<&str> = listed
        .iter()
        .filter_map(|key| key.strip_prefix(&directory))
        .collect();
    let stem = unique_stem(stem, extension, &existing);
    let original = format!("{stem}.{extension}");
    let mut keys = Vec::new();
    let mut put = |file: &str, data: &[u8], keys: &mut Vec<String>| -> Result<(), Error> {
        let key = format!("{directory}{file}");
        objects.put(&key, kind.mime(), data)?;
        keys.push(key);
        Ok(())
    };
    put(&original, bytes, &mut keys)?;

    let (width, height) = image.dimensions();
    let mut metadata = Metadata {
        width,
        height,
        file: format!("{subdir}/{original}"),
        filesize: bytes.len() as u64,
        sizes: Vec::new(),
        original_image: None,
    };
    // Big images: the attached file becomes a copy fitted into the threshold square.
    if width > BIG_IMAGE_THRESHOLD || height > BIG_IMAGE_THRESHOLD {
        let (w, h) = constrain(width, height, BIG_IMAGE_THRESHOLD, BIG_IMAGE_THRESHOLD);
        let scaled = encode(&image.resize_exact(w, h, FilterType::Triangle), kind)?;
        let file = scaled_name(&stem, extension);
        put(&file, &scaled, &mut keys)?;
        metadata.width = w;
        metadata.height = h;
        metadata.file = format!("{subdir}/{file}");
        metadata.filesize = scaled.len() as u64;
        metadata.original_image = Some(original.clone());
    }
    // Sub-sizes are cut from the original, judged against the original's dimensions.
    for size in DEFAULT_SIZES {
        let Some(plan) = resize_plan(width, height, size.width, size.height, size.crop) else {
            continue;
        };
        let resized = image
            .crop_imm(plan.src_x, plan.src_y, plan.src_w, plan.src_h)
            .resize_exact(plan.width, plan.height, FilterType::Triangle);
        let data = encode(&resized, kind)?;
        let file = sized_name(&stem, extension, plan.width, plan.height);
        put(&file, &data, &mut keys)?;
        metadata.sizes.push(SizeEntry {
            name: size.name,
            file,
            width: plan.width,
            height: plan.height,
            mime: kind.mime(),
            filesize: data.len() as u64,
        });
    }
    Ok(Files {
        original,
        metadata,
        keys,
    })
}

// JPEG at the reference's quality; GIF and WebP (lossless, owner decision on #10) need RGBA.
fn encode(image: &DynamicImage, kind: ImageKind) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let failed = |error: image::ImageError| Error::Encode(error.to_string().into());
    match kind {
        ImageKind::Jpeg => DynamicImage::ImageRgb8(image.to_rgb8())
            .write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))
            .map_err(failed)?,
        ImageKind::Png => image
            .write_to(&mut Cursor::new(&mut out), kind.format())
            .map_err(failed)?,
        ImageKind::Gif | ImageKind::Webp => DynamicImage::ImageRgba8(image.to_rgba8())
            .write_to(&mut Cursor::new(&mut out), kind.format())
            .map_err(failed)?,
    }
    Ok(out)
}

struct AttachmentRow<'a> {
    author: u64,
    title: &'a str,
    mime: &'static str,
    guid: String,
}

fn tables(prefix: &TablePrefix) -> Result<Tables, Error> {
    let table = |stem: &str| {
        TableStem::new(stem)
            .map(|stem| prefix.table(&stem))
            .map_err(|_| Error::Literal)
    };
    Ok(Tables {
        posts: table("posts")?,
        postmeta: table("postmeta")?,
        outbox: table("outbox")?,
    })
}

// BEGIN, unique slug, post row, two meta rows, outbox row, COMMIT; ROLLBACK on a rejection.
fn commit(
    site: &Site,
    clock: &Clock,
    row: &AttachmentRow<'_>,
    metadata: &Metadata,
    store: &mut impl Store,
) -> Result<u64, Error> {
    let tables = tables(&site.prefix)?;
    let statements = Statements {
        dialect: site.dialect,
        tables: &tables,
    };
    run(store, &Sql::from("BEGIN"))?;
    let result = (|| {
        let slug = unique_slug(&statements, &sanitize_slug(row.title), store)?;
        let mut draft = Draft::new();
        draft
            .set(Field::Author, row.author.to_string())
            .set(Field::Date, &clock.local)
            .set(Field::DateGmt, &clock.gmt)
            .set(Field::Title, row.title)
            .set(Field::Status, b"inherit")
            .set(Field::TypeName, b"attachment")
            .set(Field::CommentStatus, &site.comment_status)
            .set(Field::PingStatus, b"closed")
            .set(Field::Name, slug)
            .set(Field::Modified, &clock.local)
            .set(Field::ModifiedGmt, &clock.gmt)
            .set(Field::Parent, b"0")
            .set(Field::MenuOrder, b"0")
            .set(Field::MimeType, row.mime)
            .set(Field::Guid, &row.guid);
        let id = inserted_id(run(store, &statements.insert_post(&draft)?)?)?;
        run(
            store,
            &statements.insert_meta(id, "_wp_attached_file", metadata.file.as_bytes())?,
        )?;
        run(
            store,
            &statements.insert_meta(id, "_wp_attachment_metadata", &metadata.serialize())?,
        )?;
        run(store, &statements.insert_outbox(id)?)?;
        run(store, &Sql::from("COMMIT"))?;
        Ok(id)
    })();
    if result.is_err() {
        // The adapter may already have lost the connection; the rollback is best effort.
        let _ = store.query(&Sql::from("ROLLBACK"));
    }
    result
}

// The reference appends -2, -3, ... until no post holds the slug.
fn unique_slug(
    statements: &Statements<'_>,
    base: &str,
    store: &mut impl Store,
) -> Result<String, Error> {
    let mut candidate = base.to_owned();
    let mut suffix = 2_u64;
    loop {
        match run(store, &statements.slug_taken(&candidate)?)? {
            Reply::Rows(set) if set.rows().is_empty() => return Ok(candidate),
            Reply::Rows(_) => {
                candidate = format!("{base}-{suffix}");
                suffix += 1;
            }
            other => return Err(unexpected(&other)),
        }
    }
}

// Runs one statement; a server rejection becomes an error.
fn run(store: &mut impl Store, sql: &Sql) -> Result<Reply, Error> {
    match store.query(sql) {
        Ok(Reply::Failed(error)) => Err(Error::Rejected(error.message().into())),
        Ok(reply) => Ok(reply),
        Err(_) => Err(Error::Unavailable),
    }
}

// `RETURNING "ID"` arrives as one row; the `MySQL` adapter reports `insert_id`.
fn inserted_id(reply: Reply) -> Result<u64, Error> {
    let id = match &reply {
        Reply::Done { insert_id, .. } => Some(*insert_id),
        Reply::Rows(set) => set
            .rows()
            .first()
            .and_then(|row| row.cells().first())
            .and_then(|cell| match cell {
                cms_db::Cell::Bytes(bytes) => std::str::from_utf8(bytes).ok()?.parse().ok(),
                cms_db::Cell::Null => None,
            }),
        _ => None,
    };
    match id {
        Some(id) if id > 0 => Ok(id),
        _ => Err(unexpected(&reply)),
    }
}

fn unexpected(reply: &Reply) -> Error {
    Error::Rejected(format!("unexpected reply {reply:?}").into())
}

#[cfg(test)]
mod tests {
    use super::{incoming_name, inserted_id, month_dir};
    use crate::Error;
    use cms_db::{Cell, Reply};

    #[test]
    fn incoming_keys_must_be_uuid_and_name() {
        let uuid = "00000000-0000-4000-8000-000000000000";
        assert_eq!(
            incoming_name(&format!("site/incoming/{uuid}/a.jpg")),
            Ok("a.jpg")
        );
        for bad in [
            "site/uploads/x/a.jpg".to_owned(),
            "site/incoming/short/a.jpg".to_owned(),
            format!("site/incoming/{uuid}"),
            format!("site/incoming/{uuid}/"),
            format!("site/incoming/{uuid}/a/b.jpg"),
        ] {
            assert_eq!(incoming_name(&bad), Err(Error::Key(bad.as_str().into())));
        }
    }

    #[test]
    fn month_directories_come_from_local_time() {
        assert_eq!(month_dir("2026-11-27 10:00:00"), Ok("2026/11".to_owned()));
        for bad in [
            "2026-11-27",
            "2026/11/27 10:00:00",
            "2026-11-27T10:00:00",
            "20x6-11-27 10:00:00",
        ] {
            assert_eq!(month_dir(bad), Err(Error::Clock(bad.into())));
        }
    }

    #[test]
    fn insert_ids_come_from_either_reply_shape() {
        assert_eq!(
            inserted_id(Reply::Done {
                affected: 1,
                insert_id: 9
            }),
            Ok(9)
        );
        assert_eq!(
            inserted_id(Reply::single_column("ID", [Cell::bytes("12")])),
            Ok(12)
        );
        assert!(
            inserted_id(Reply::Done {
                affected: 1,
                insert_id: 0
            })
            .is_err()
        );
        assert!(inserted_id(Reply::single_column("ID", [Cell::Null])).is_err());
        assert!(inserted_id(Reply::single_column("ID", [])).is_err());
        assert!(inserted_id(Reply::single_column("ID", [Cell::bytes("x")])).is_err());
    }
}
