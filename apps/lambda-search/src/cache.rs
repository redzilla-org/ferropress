//! What a warm container keeps between invocations: the decoded index and the theme shell.

use cms_readmodel::{BasePath, BlobKey, Blobs, Content, PortError, RouteKey, RouteTable};
use cms_search::{INDEX_ROUTE, Index};

use crate::SearchError;

/// Per-container cache. Both objects are content-addressed, so an unchanged key means unchanged
/// bytes and a check costs one route item read, never an object download.
#[derive(Debug)]
pub struct Cache {
    index_key: Option<BlobKey>,
    index: Index,
    shell: Option<(BlobKey, Box<[u8]>)>,
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

impl Cache {
    /// An empty cache for a cold container.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            index_key: None,
            index: Index::empty(),
            shell: None,
        }
    }

    /// The current index. A site without a `SEARCH` route item has not been projected yet and
    /// searches an empty index.
    ///
    /// # Errors
    ///
    /// [`SearchError`] when a port fails, the item is not an object, or the object is missing or
    /// malformed.
    pub fn index<R: RouteTable, B: Blobs>(
        &mut self,
        routes: &mut R,
        blobs: &mut B,
    ) -> Result<&Index, SearchError> {
        let key = match routes.route(&RouteKey::from(INDEX_ROUTE))? {
            None => None,
            Some(route) => match route.content {
                Content::Object { body } => Some(body),
                _ => return Err(SearchError::IndexRoute),
            },
        };
        if key != self.index_key {
            self.index = match &key {
                None => Index::empty(),
                Some(key) => Index::from_slice(&fetch(blobs, key)?)?,
            };
            self.index_key = key;
        }
        Ok(&self.index)
    }

    /// The theme shell of the home page under `base`, or `None` while the site has no rendered
    /// home page.
    ///
    /// # Errors
    ///
    /// [`SearchError::Port`] when a port fails or the shell object is missing.
    pub fn shell<R: RouteTable, B: Blobs>(
        &mut self,
        base: &BasePath,
        routes: &mut R,
        blobs: &mut B,
    ) -> Result<Option<&[u8]>, SearchError> {
        let Some(route) = routes.route(&RouteKey::from(base.as_str()))? else {
            return Ok(None);
        };
        let Content::Page { shell, .. } = route.content else {
            return Ok(None);
        };
        if self
            .shell
            .as_ref()
            .is_none_or(|(cached, _)| *cached != shell)
        {
            let bytes = fetch(blobs, &shell)?.into_boxed_slice();
            self.shell = Some((shell, bytes));
        }
        Ok(self.shell.as_ref().map(|(_, bytes)| &**bytes))
    }
}

fn fetch<B: Blobs>(blobs: &mut B, key: &BlobKey) -> Result<Vec<u8>, PortError> {
    blobs
        .get(key)?
        .ok_or_else(|| PortError::MissingBlob(key.as_str().into()))
}
