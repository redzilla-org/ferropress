//! Attachment metadata in the reference's serialized layout.

use php_serialize::Writer;

// Image metadata keys and their defaults, in the reference's order. With no EXIF/IPTC block
// read (owner decision on #10) every value is the default; the reference stores the numeric
// defaults as the string "0".
const IMAGE_META: [(&str, &str); 11] = [
    ("aperture", "0"),
    ("credit", ""),
    ("camera", ""),
    ("caption", ""),
    ("created_timestamp", "0"),
    ("copyright", ""),
    ("focal_length", "0"),
    ("iso", "0"),
    ("shutter_speed", "0"),
    ("title", ""),
    ("orientation", "0"),
];

/// One generated sub-size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SizeEntry {
    /// Size name (`thumbnail`, `medium`, ...).
    pub name: &'static str,
    /// Base file name, `stem-WxH.ext`.
    pub file: String,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Mime type.
    pub mime: &'static str,
    /// Bytes written.
    pub filesize: u64,
}

/// The `_wp_attachment_metadata` value of one image attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    /// Width of the attached file.
    pub width: u32,
    /// Height of the attached file.
    pub height: u32,
    /// Attached file relative to the uploads root, `YYYY/MM/name.ext`.
    pub file: String,
    /// Bytes of the attached file.
    pub filesize: u64,
    /// Generated sub-sizes in creation order.
    pub sizes: Vec<SizeEntry>,
    /// Base name of the unscaled original when the attached file is the `-scaled` copy.
    pub original_image: Option<String>,
}

impl Metadata {
    /// The serialized bytes, key order as the reference builds the array.
    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Writer::with_capacity(512 + 160 * self.sizes.len());
        out.array(6 + usize::from(self.original_image.is_some()));
        pair_int(&mut out, "width", self.width.into());
        pair_int(&mut out, "height", self.height.into());
        pair_str(&mut out, "file", &self.file);
        pair_int(&mut out, "filesize", self.filesize);
        out.string(b"sizes");
        out.array(self.sizes.len());
        for size in &self.sizes {
            out.string(size.name.as_bytes());
            out.array(5);
            pair_str(&mut out, "file", &size.file);
            pair_int(&mut out, "width", size.width.into());
            pair_int(&mut out, "height", size.height.into());
            pair_str(&mut out, "mime-type", size.mime);
            pair_int(&mut out, "filesize", size.filesize);
            out.end();
        }
        out.end();
        out.string(b"image_meta");
        out.array(IMAGE_META.len() + 1);
        for (key, value) in IMAGE_META {
            pair_str(&mut out, key, value);
        }
        out.string(b"keywords");
        out.array(0);
        out.end();
        out.end();
        if let Some(original) = &self.original_image {
            pair_str(&mut out, "original_image", original);
        }
        out.end();
        out.into_bytes()
    }
}

fn pair_str(out: &mut Writer, key: &str, value: &str) {
    out.string(key.as_bytes());
    out.string(value.as_bytes());
}

// Sizes are far below i64::MAX; saturate rather than wrap if a caller ever passes more.
fn pair_int(out: &mut Writer, key: &str, value: u64) {
    out.string(key.as_bytes());
    out.int(i64::try_from(value).unwrap_or(i64::MAX));
}

#[cfg(test)]
mod tests {
    use super::{Metadata, SizeEntry};

    const IMAGE_META: &str = r#"s:10:"image_meta";a:12:{s:8:"aperture";s:1:"0";s:6:"credit";s:0:"";s:6:"camera";s:0:"";s:7:"caption";s:0:"";s:17:"created_timestamp";s:1:"0";s:9:"copyright";s:0:"";s:12:"focal_length";s:1:"0";s:3:"iso";s:1:"0";s:13:"shutter_speed";s:1:"0";s:5:"title";s:0:"";s:11:"orientation";s:1:"0";s:8:"keywords";a:0:{}}"#;

    #[test]
    fn plain_image_metadata_matches_the_reference_layout() {
        let meta = Metadata {
            width: 200,
            height: 100,
            file: "2026/11/a.png".into(),
            filesize: 1234,
            sizes: vec![SizeEntry {
                name: "thumbnail",
                file: "a-150x100.png".into(),
                width: 150,
                height: 100,
                mime: "image/png",
                filesize: 99,
            }],
            original_image: None,
        };
        let expected = format!(
            r#"a:6:{{s:5:"width";i:200;s:6:"height";i:100;s:4:"file";s:13:"2026/11/a.png";s:8:"filesize";i:1234;s:5:"sizes";a:1:{{s:9:"thumbnail";a:5:{{s:4:"file";s:13:"a-150x100.png";s:5:"width";i:150;s:6:"height";i:100;s:9:"mime-type";s:9:"image/png";s:8:"filesize";i:99;}}}}{IMAGE_META}}}"#
        );
        assert_eq!(String::from_utf8_lossy(&meta.serialize()), expected);
        assert!(php_serialize::decode(&meta.serialize()).is_ok());
    }

    #[test]
    fn scaled_image_appends_the_original_name() {
        let meta = Metadata {
            width: 2560,
            height: 1920,
            file: "2026/11/p-scaled.jpg".into(),
            filesize: u64::MAX,
            sizes: Vec::new(),
            original_image: Some("p.jpg".into()),
        };
        let bytes = String::from_utf8_lossy(&meta.serialize()).into_owned();
        assert!(bytes.starts_with(r#"a:7:{s:5:"width";i:2560;"#));
        assert!(bytes.contains(r#"s:8:"filesize";i:9223372036854775807;s:5:"sizes";a:0:{}"#));
        assert!(bytes.ends_with(r#"s:14:"original_image";s:5:"p.jpg";}"#));
    }
}
