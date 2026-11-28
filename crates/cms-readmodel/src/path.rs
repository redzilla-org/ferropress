//! Base path and route keys.

use std::fmt;

/// The path prefix the site is mounted at, such as `/blog/` or `/` on its own subdomain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasePath(Box<str>);

/// A base path was rejected.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathError {
    /// A base path starts and ends with `/`.
    #[error("a base path must start and end with '/'")]
    Slashes,
}

/// Normalized request path; the key the projector writes route items under.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RouteKey(Box<str>);

/// Where a request path lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// Look up this route item.
    Key(RouteKey),
    /// A directory-style path without its trailing slash; redirect to this location.
    AddSlash(Box<str>),
    /// Not under the base path; `CloudFront` should never send it here.
    Outside,
}

impl BasePath {
    /// A base path with these characters.
    ///
    /// # Errors
    ///
    /// [`PathError::Slashes`] unless `path` starts and ends with `/`.
    pub fn new(path: &str) -> Result<Self, PathError> {
        if !path.starts_with('/') || !path.ends_with('/') {
            return Err(PathError::Slashes);
        }
        Ok(Self(path.into()))
    }

    /// The prefix as configured.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolves a request path (no query string) to a route key or a redirect.
    ///
    /// Directory-style paths get the trailing slash the reference's permalinks use, so the cache
    /// holds one entry per page. A last segment with a dot (`feed.xml`, `robots.txt`) is a file.
    #[must_use]
    pub fn resolve(&self, path: &str) -> Resolved {
        // The mount point itself without its slash (`/blog`) is the home page.
        let bare = self.0.strip_suffix('/').unwrap_or(&self.0);
        if !bare.is_empty() && path == bare {
            return Resolved::AddSlash(self.0.clone());
        }
        if !path.starts_with(&*self.0) {
            return Resolved::Outside;
        }
        let last = path.rsplit('/').next().unwrap_or_default();
        if last.is_empty() || last.contains('.') {
            return Resolved::Key(RouteKey(path.into()));
        }
        let mut location = String::with_capacity(path.len() + 1);
        location.push_str(path);
        location.push('/');
        Resolved::AddSlash(location.into())
    }
}

impl RouteKey {
    /// The normalized path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for RouteKey {
    fn from(path: &str) -> Self {
        Self(path.into())
    }
}

impl fmt::Display for RouteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{BasePath, PathError, Resolved, RouteKey};

    #[test]
    fn base_paths_need_both_slashes() {
        assert_eq!(BasePath::new("blog/"), Err(PathError::Slashes));
        assert_eq!(BasePath::new("/blog"), Err(PathError::Slashes));
        assert_eq!(BasePath::new("/blog/").map(|b| b.as_str().len()), Ok(6));
        assert_eq!(
            PathError::Slashes.to_string(),
            "a base path must start and end with '/'"
        );
    }

    #[test]
    fn paths_resolve_to_keys_redirects_or_outside() {
        let Ok(base) = BasePath::new("/blog/") else {
            return;
        };
        assert_eq!(base.resolve("/blog/"), Resolved::Key("/blog/".into()));
        assert_eq!(base.resolve("/blog"), Resolved::AddSlash("/blog/".into()));
        assert_eq!(
            base.resolve("/blog/2026/11/post"),
            Resolved::AddSlash("/blog/2026/11/post/".into())
        );
        assert_eq!(
            base.resolve("/blog/sitemap.xml"),
            Resolved::Key(RouteKey::from("/blog/sitemap.xml"))
        );
        assert_eq!(base.resolve("/blogger/"), Resolved::Outside);
        assert_eq!(base.resolve("/"), Resolved::Outside);
    }

    #[test]
    fn a_root_mount_serves_every_path() {
        let Ok(base) = BasePath::new("/") else {
            return;
        };
        assert_eq!(base.resolve("/"), Resolved::Key("/".into()));
        assert_eq!(base.resolve("/about"), Resolved::AddSlash("/about/".into()));
        let key = RouteKey::from("/about/");
        assert_eq!(key.to_string(), key.as_str());
    }
}
