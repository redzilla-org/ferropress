//! Warm-invocation cache of page shells.

use std::collections::HashMap;

use cms_readmodel::{BlobKey, Blobs, PortError};

// A site has a handful of live shells (one per template variant); old ones age out by clearing.
const CAPACITY: usize = 16;

/// Keeps page shells in memory across warm invocations. Shell keys are content hashes, so a
/// cached shell can never be stale; a theme change writes a new key. Bodies pass straight
/// through.
#[derive(Debug)]
pub struct ShellCache<B> {
    inner: B,
    shells: HashMap<BlobKey, Vec<u8>>,
}

impl<B: Blobs> ShellCache<B> {
    /// Wraps `inner`.
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            shells: HashMap::new(),
        }
    }

    /// How many shells are held.
    pub fn len(&self) -> usize {
        self.shells.len()
    }

    /// Whether no shell is held.
    pub fn is_empty(&self) -> bool {
        self.shells.is_empty()
    }
}

impl<B: Blobs> Blobs for ShellCache<B> {
    fn get(&mut self, key: &BlobKey) -> Result<Option<Vec<u8>>, PortError> {
        self.inner.get(key)
    }

    fn shell(&mut self, key: &BlobKey) -> Result<Option<Vec<u8>>, PortError> {
        if let Some(bytes) = self.shells.get(key) {
            return Ok(Some(bytes.clone()));
        }
        let fetched = self.inner.shell(key)?;
        if let Some(bytes) = &fetched {
            if self.shells.len() >= CAPACITY {
                self.shells.clear();
            }
            self.shells.insert(key.clone(), bytes.clone());
        }
        Ok(fetched)
    }
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, ShellCache};
    use cms_readmodel::{BlobKey, Blobs, PortError};

    #[derive(Default)]
    struct Counting {
        fetches: usize,
    }

    impl Blobs for Counting {
        fn get(&mut self, key: &BlobKey) -> Result<Option<Vec<u8>>, PortError> {
            self.fetches += 1;
            Ok((key.as_str() != "missing").then(|| key.as_str().as_bytes().to_vec()))
        }
    }

    #[test]
    fn shells_are_fetched_once_and_bodies_every_time() {
        let mut cache = ShellCache::new(Counting::default());
        let shell = BlobKey::from("fragments/shell/s1.html");
        assert!(cache.is_empty());
        for _ in 0..3 {
            assert_eq!(
                cache.shell(&shell),
                Ok(Some(shell.as_str().as_bytes().to_vec()))
            );
        }
        assert_eq!(cache.inner.fetches, 1);
        let body = BlobKey::from("rendered/b.html");
        assert!(cache.get(&body).is_ok());
        assert!(cache.get(&body).is_ok());
        assert_eq!(cache.inner.fetches, 3);
        assert_eq!(cache.shell(&BlobKey::from("missing")), Ok(None));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_full_cache_starts_over() {
        let mut cache = ShellCache::new(Counting::default());
        for n in 0..=CAPACITY {
            assert!(
                cache
                    .shell(&BlobKey::from(format!("s{n}").as_str()))
                    .is_ok()
            );
        }
        assert_eq!(cache.len(), 1);
    }
}
