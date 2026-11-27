//! Route items as the projector writes them.

/// Key of one pre-rendered object in the blob store (S3 key under the site prefix).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlobKey(Box<str>);

impl BlobKey {
    /// The object key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for BlobKey {
    fn from(key: &str) -> Self {
        Self(key.into())
    }
}

/// What a route serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// An HTML page: the body fragment inserted into the shared shell at [`crate::BODY_MARK`].
    /// Split so a theme or menu change re-renders one shell, not every page.
    Page {
        /// Theme shell (head, header, footer, sidebar).
        shell: BlobKey,
        /// The page's own markup.
        body: BlobKey,
    },
    /// A complete object such as a feed, sitemap or robots file.
    Object {
        /// The full response body.
        body: BlobKey,
    },
    /// A permanent redirect, for example an old slug.
    Redirect {
        /// Absolute path or URL to send the client to.
        location: Box<str>,
    },
    /// Content that existed and was removed.
    Gone,
}

/// One route item: the HTTP metadata plus where the bytes live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// Response status for served content (200 for pages, 404 for the not-found page).
    pub status: u16,
    /// `Content-Type` header value.
    pub content_type: Box<str>,
    /// Strong validator computed by the projector from the assembled bytes.
    pub etag: Box<str>,
    /// `Last-Modified` header value (IMF-fixdate).
    pub last_modified: Box<str>,
    /// The content.
    pub content: Content,
}

#[cfg(test)]
mod tests {
    use super::BlobKey;

    #[test]
    fn blob_keys_keep_their_text() {
        assert_eq!(
            BlobKey::from("rendered/ab.html").as_str(),
            "rendered/ab.html"
        );
    }
}
