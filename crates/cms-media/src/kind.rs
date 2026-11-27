//! The image types this profile accepts and processes.

use image::ImageFormat;

/// One accepted raster image type.
///
/// The reference allows many more upload types; this profile accepts only the four it can
/// decode and resize (owner decision on #10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// `jpg|jpeg|jpe`, `image/jpeg`.
    Jpeg,
    /// `png`, `image/png`.
    Png,
    /// `gif`, `image/gif`.
    Gif,
    /// `webp`, `image/webp`.
    Webp,
}

impl ImageKind {
    /// The kind for a file extension, compared case-insensitively as the reference does.
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "jpe" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }

    /// The reference's mime type for this kind.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }

    /// Short lower-case label used in errors.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    // The decoder/encoder format of the image crate.
    pub(crate) const fn format(self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
            Self::Gif => ImageFormat::Gif,
            Self::Webp => ImageFormat::WebP,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ImageKind;
    use image::ImageFormat;

    #[test]
    fn extensions_map_to_reference_mimes() {
        for (ext, mime) in [
            ("jpg", "image/jpeg"),
            ("JPEG", "image/jpeg"),
            ("jpe", "image/jpeg"),
            ("png", "image/png"),
            ("gif", "image/gif"),
            ("WebP", "image/webp"),
        ] {
            assert_eq!(
                ImageKind::from_extension(ext).map(ImageKind::mime),
                Some(mime),
                "{ext}"
            );
        }
        assert_eq!(ImageKind::from_extension("pdf"), None);
        assert_eq!(ImageKind::from_extension(""), None);
    }

    #[test]
    fn labels_and_formats_line_up() {
        let kinds = [
            ImageKind::Jpeg,
            ImageKind::Png,
            ImageKind::Gif,
            ImageKind::Webp,
        ];
        let labels: Vec<_> = kinds.iter().map(|kind| kind.label()).collect();
        assert_eq!(labels, ["jpeg", "png", "gif", "webp"]);
        let formats: Vec<_> = kinds.iter().map(|kind| kind.format()).collect();
        assert_eq!(
            formats,
            [
                ImageFormat::Jpeg,
                ImageFormat::Png,
                ImageFormat::Gif,
                ImageFormat::WebP
            ]
        );
    }
}
