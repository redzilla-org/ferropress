//! One search request from event to response.

use cms_query::SearchVar;
use cms_readmodel::{BasePath, Blobs, Method, PortError, Response, RouteTable};
use cms_search::{IndexError, PER_PAGE, Terms};

use crate::query::param;
use crate::{Cache, SearchRequest, decode, encode, html, rest};

/// Search answers change with every publish and are not invalidated by path, so the edge keeps them
/// five minutes and browsers one.
pub(crate) const CACHE: &str = "public, max-age=60, s-maxage=300";

// The REST search route under the base path.
const REST_SEARCH: &str = "wp-json/wp/v2/search";

/// A search request could not be answered.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SearchError {
    /// A storage port failed or names a missing object.
    #[error(transparent)]
    Port(#[from] PortError),
    /// The index object is malformed.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// The `SEARCH` route item is not an object item.
    #[error("the search route item does not name an object")]
    IndexRoute,
    /// The home page shell has no body mark.
    #[error("the home page shell has no body mark")]
    NoBodyMark,
}

/// Answers `request` for the site mounted at `base`.
///
/// # Errors
///
/// [`SearchError`] when the index or the shell cannot be loaded.
pub fn handle<R: RouteTable, B: Blobs>(
    base: &BasePath,
    request: &SearchRequest,
    cache: &mut Cache,
    routes: &mut R,
    blobs: &mut B,
) -> Result<Response, SearchError> {
    if request.method == Method::Other {
        let mut response = bare(405, "no-store");
        response.headers.push(("allow", "GET, HEAD".into()));
        return Ok(response);
    }
    let Some(local) = request.path.strip_prefix(base.as_str()) else {
        return Ok(bare(404, "no-store"));
    };
    let mut response = if local.trim_end_matches('/') == REST_SEARCH {
        rest::search(&request.query, cache.index(routes, blobs)?)
    } else if let Some(search) = param(&request.query, "s") {
        if let Some(page) = results_page(local, &request.query) {
            page_response(base, &search, page, cache, routes, blobs)?
        } else {
            // A search under another archive is not scoped here; send it to the site-wide search.
            let mut response = bare(302, CACHE);
            let location = format!("{}?{}", base.as_str(), request.query);
            response.headers.push(("location", location.into()));
            response
        }
    } else {
        // CloudFront only routes searches here.
        bare(404, "no-store")
    };
    if request.method == Method::Head {
        response.body.clear();
    }
    Ok(response)
}

/// Decodes `payload`, answers it and encodes the response; the error text is what the Runtime API
/// is told.
///
/// # Errors
///
/// The message of the [`crate::EventError`] or [`SearchError`] that stopped the request.
pub fn invoke<R: RouteTable, B: Blobs>(
    base: &BasePath,
    payload: &[u8],
    cache: &mut Cache,
    routes: &mut R,
    blobs: &mut B,
) -> Result<Vec<u8>, String> {
    let request = decode(payload).map_err(|error| error.to_string())?;
    let response =
        handle(base, &request, cache, routes, blobs).map_err(|error| error.to_string())?;
    encode(response).map_err(|error| error.to_string())
}

// The page number for a search at `local` (the path under the base): the base itself, with an
// optional `paged` var, or the pretty `page/N/` form. `None` for any other path.
fn results_page(local: &str, query: &str) -> Option<usize> {
    if local.is_empty() {
        // The reference reads `paged` as a non-negative integer, where 0 and junk mean page 1.
        let paged = param(query, "paged")
            .and_then(|value| String::from_utf8(value).ok()?.parse::<usize>().ok());
        return Some(paged.unwrap_or(1).max(1));
    }
    let number = local.strip_prefix("page/")?.strip_suffix('/')?;
    number.parse::<usize>().ok().filter(|&n| n >= 1)
}

fn page_response<R: RouteTable, B: Blobs>(
    base: &BasePath,
    search: &[u8],
    page: usize,
    cache: &mut Cache,
    routes: &mut R,
    blobs: &mut B,
) -> Result<Response, SearchError> {
    let text = String::from_utf8_lossy(SearchVar::parse(Some(search)).as_bytes()).into_owned();
    let found = cache
        .index(routes, blobs)?
        .search(&Terms::parse(&text), page, PER_PAGE);
    let fragment = html::results(base.as_str(), &text, page, &found);
    let shell = cache.shell(base, routes, blobs)?;
    let body = html::document(shell, &fragment).ok_or(SearchError::NoBodyMark)?;
    // The reference never answers 404 for a search, even past the last page.
    let mut response = bare(200, CACHE);
    response
        .headers
        .push(("content-type", "text/html; charset=UTF-8".into()));
    response.body = body;
    Ok(response)
}

fn bare(status: u16, cache: &str) -> Response {
    Response {
        status,
        headers: vec![("cache-control", cache.into())],
        body: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{SearchError, results_page};

    #[test]
    fn result_pages_come_from_the_path_or_paged() {
        assert_eq!(results_page("", "s=x"), Some(1));
        assert_eq!(results_page("", "s=x&paged=3"), Some(3));
        assert_eq!(results_page("", "s=x&paged=0"), Some(1));
        assert_eq!(results_page("", "s=x&paged=junk"), Some(1));
        assert_eq!(results_page("page/2/", "s=x"), Some(2));
        assert_eq!(results_page("page/0/", "s=x"), None);
        assert_eq!(results_page("page/2", "s=x"), None);
        assert_eq!(results_page("category/news/", "s=x"), None);
    }

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(
            SearchError::IndexRoute.to_string(),
            "the search route item does not name an object"
        );
        assert_eq!(
            SearchError::NoBodyMark.to_string(),
            "the home page shell has no body mark"
        );
    }
}
