//! Ports and response assembly for one read request.

use crate::{BasePath, BlobKey, Content, Resolved, Route, RouteKey};

/// Where the body fragment goes inside a page shell.
pub const BODY_MARK: &[u8] = b"<!--fp:body-->";

/// Where a fragment's per-page head part (title, canonical, metadata) goes inside a page shell.
pub const HEAD_MARK: &[u8] = b"<!--fp:head-->";

/// Ends the per-page head part at the start of a page fragment; the rest is the body.
pub const HEAD_END: &[u8] = b"<!--fp:/head-->";

/// A page shell lacks a mark the fragment needs.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MissingMark {
    /// The shell has no [`BODY_MARK`].
    #[error("shell has no body mark")]
    Body,
    /// The fragment has a head part but the shell has no [`HEAD_MARK`] before its body mark.
    #[error("shell has no head mark before its body mark")]
    Head,
}

// Browsers revalidate after a minute; the edge keeps a day and relies on invalidation.
const CACHEABLE: &str = "public, max-age=60, s-maxage=86400";

/// Route item lookup (`DynamoDB` `GetItem` in production).
pub trait RouteTable {
    /// The route item stored under `key`, if any.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the store cannot be reached or returns a malformed item.
    fn route(&mut self, key: &RouteKey) -> Result<Option<Route>, PortError>;
}

/// Pre-rendered object lookup (`S3` `GetObject` in production).
pub trait Blobs {
    /// The bytes stored under `key`, if any.
    ///
    /// # Errors
    ///
    /// [`PortError`] when the store cannot be reached.
    fn get(&mut self, key: &BlobKey) -> Result<Option<Vec<u8>>, PortError>;

    /// The bytes of a page shell. Shell keys are content hashes, so an adapter may keep them in
    /// memory across warm invocations; the default fetches like [`Blobs::get`].
    ///
    /// # Errors
    ///
    /// [`PortError`] when the store cannot be reached.
    fn shell(&mut self, key: &BlobKey) -> Result<Option<Vec<u8>>, PortError> {
        self.get(key)
    }
}

/// A storage port failed, or the read model is inconsistent.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PortError {
    /// The backing store failed; the message names the operation, never secrets.
    #[error("read model store failed: {0}")]
    Store(Box<str>),
    /// A route item names an object that does not exist. The projector writes objects before
    /// route items, so this is a defect, not a cache race.
    #[error("route points at missing object {0}")]
    MissingBlob(Box<str>),
    /// A page shell has no body mark.
    #[error("page shell {0} has no body mark")]
    NoBodyMark(Box<str>),
    /// A page fragment carries a head part but its shell has no head mark.
    #[error("page shell {0} has no head mark")]
    NoHeadMark(Box<str>),
}

/// Request method as far as the read path cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `HEAD`: same headers, no body.
    Head,
    /// Anything else; the read path serves no writes.
    Other,
}

/// One read request, already decoded from the platform event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The method.
    pub method: Method,
    /// The path without the query string.
    pub path: Box<str>,
    /// `If-None-Match` header value, if sent.
    pub if_none_match: Option<Box<str>>,
}

/// The response to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// Header names and values, in send order.
    pub headers: Vec<(&'static str, Box<str>)>,
    /// Body bytes (empty for `HEAD`, 304 and redirects).
    pub body: Vec<u8>,
}

impl Response {
    fn bare(status: u16, cache: &str) -> Self {
        Self {
            status,
            headers: vec![("cache-control", cache.into())],
            body: Vec::new(),
        }
    }
}

/// Serves `request` from the read model under `base`.
///
/// A missing route serves the not-found route item stored at `<base>404`, which the projector
/// renders with the theme; if that is missing too, an empty 404.
///
/// # Errors
///
/// [`PortError`] when a port fails or the read model is inconsistent.
pub fn serve<R: RouteTable, B: Blobs>(
    base: &BasePath,
    request: &Request,
    routes: &mut R,
    blobs: &mut B,
) -> Result<Response, PortError> {
    if request.method == Method::Other {
        let mut response = Response::bare(405, "no-store");
        response.headers.push(("allow", "GET, HEAD".into()));
        return Ok(response);
    }
    let key = match base.resolve(&request.path) {
        Resolved::Key(key) => key,
        Resolved::AddSlash(location) => return Ok(redirect(&location)),
        Resolved::Outside => return Ok(Response::bare(404, "no-store")),
    };
    let route = if let Some(route) = routes.route(&key)? {
        route
    } else {
        let missing = RouteKey::from(format!("{}404", base.as_str()).as_str());
        let Some(route) = routes.route(&missing)? else {
            return Ok(Response::bare(404, CACHEABLE));
        };
        route
    };
    respond(request, &route, blobs)
}

fn respond<B: Blobs>(
    request: &Request,
    route: &Route,
    blobs: &mut B,
) -> Result<Response, PortError> {
    let body = match &route.content {
        Content::Redirect { location } => return Ok(redirect(location)),
        Content::Gone => return Ok(Response::bare(410, CACHEABLE)),
        Content::Object { .. } | Content::Page { .. }
            if request.if_none_match.as_deref() == Some(&*route.etag) =>
        {
            let mut response = Response::bare(304, CACHEABLE);
            response.headers.push(("etag", route.etag.clone()));
            return Ok(response);
        }
        Content::Object { body } => fetch(blobs, body)?,
        Content::Page { shell, body } => {
            let shell_bytes = blobs
                .shell(shell)?
                .ok_or_else(|| PortError::MissingBlob(shell.as_str().into()))?;
            page(&shell_bytes, shell, &fetch(blobs, body)?)?
        }
    };
    let mut response = Response::bare(route.status, CACHEABLE);
    response.headers.extend([
        ("content-type", route.content_type.clone()),
        ("etag", route.etag.clone()),
        ("last-modified", route.last_modified.clone()),
    ]);
    if request.method == Method::Get {
        response.body = body;
    }
    Ok(response)
}

fn redirect(location: &str) -> Response {
    let mut response = Response::bare(301, CACHEABLE);
    response.headers.push(("location", location.into()));
    response
}

fn fetch<B: Blobs>(blobs: &mut B, key: &BlobKey) -> Result<Vec<u8>, PortError> {
    blobs
        .get(key)?
        .ok_or_else(|| PortError::MissingBlob(key.as_str().into()))
}

fn page(shell: &[u8], shell_key: &BlobKey, body: &[u8]) -> Result<Vec<u8>, PortError> {
    assemble(shell, body).map_err(|missing| {
        let key = shell_key.as_str().into();
        match missing {
            MissingMark::Body => PortError::NoBodyMark(key),
            MissingMark::Head => PortError::NoHeadMark(key),
        }
    })
}

/// Assembles a page from its shell and fragment: the fragment's head part (everything before
/// [`HEAD_END`], if present) replaces [`HEAD_MARK`], the rest replaces [`BODY_MARK`]. A shell's
/// head mark with no head part in the fragment is removed. The projector hashes this output, so
/// entity tags cover exactly the served bytes.
///
/// # Errors
///
/// [`MissingMark`] when the shell lacks a mark the fragment needs.
pub fn assemble(shell: &[u8], fragment: &[u8]) -> Result<Vec<u8>, MissingMark> {
    let (head, body) = match find(fragment, HEAD_END) {
        Some(at) => (&fragment[..at], &fragment[at + HEAD_END.len()..]),
        None => (&fragment[..0], fragment),
    };
    let body_at = find(shell, BODY_MARK).ok_or(MissingMark::Body)?;
    // Only a head mark ahead of the body mark counts; the head precedes `<main>` in every shell.
    let head_at = find(&shell[..body_at], HEAD_MARK);
    if head_at.is_none() && !head.is_empty() {
        return Err(MissingMark::Head);
    }
    let mut out = Vec::with_capacity(shell.len() + fragment.len());
    let rest = match head_at {
        Some(at) => {
            out.extend_from_slice(&shell[..at]);
            out.extend_from_slice(head);
            at + HEAD_MARK.len()
        }
        None => 0,
    };
    out.extend_from_slice(&shell[rest..body_at]);
    out.extend_from_slice(body);
    out.extend_from_slice(&shell[body_at + BODY_MARK.len()..]);
    Ok(out)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{MissingMark, assemble};

    #[test]
    fn fragments_fill_the_head_and_body_marks() {
        let shell = b"<head><!--fp:head--></head><main><!--fp:body--></main>";
        assert_eq!(
            assemble(shell, b"<title>T</title><!--fp:/head--><h1>B</h1>"),
            Ok(b"<head><title>T</title></head><main><h1>B</h1></main>".to_vec())
        );
        assert_eq!(
            assemble(shell, b"<h1>B</h1>"),
            Ok(b"<head></head><main><h1>B</h1></main>".to_vec())
        );
        assert_eq!(
            assemble(b"<main><!--fp:body--></main>", b"<h1>B</h1>"),
            Ok(b"<main><h1>B</h1></main>".to_vec())
        );
    }

    #[test]
    fn missing_marks_are_errors() {
        assert_eq!(assemble(b"<main></main>", b"x"), Err(MissingMark::Body));
        assert_eq!(
            assemble(b"<main><!--fp:body--></main>", b"t<!--fp:/head-->x"),
            Err(MissingMark::Head)
        );
        assert_eq!(
            assemble(b"<!--fp:body--><!--fp:head-->", b"t<!--fp:/head-->x"),
            Err(MissingMark::Head)
        );
        assert_eq!(MissingMark::Body.to_string(), "shell has no body mark");
        assert_eq!(
            MissingMark::Head.to_string(),
            "shell has no head mark before its body mark"
        );
    }
}
