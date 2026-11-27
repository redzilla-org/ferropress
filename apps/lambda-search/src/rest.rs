//! REST search: `wp-json/wp/v2/search` with `search`, `page`, `per_page`, `type`, `subtype`.

use cms_query::SearchVar;
use cms_readmodel::Response;
use cms_search::{Index, PER_PAGE, Terms};
use serde_json::{Value, json};

use crate::handle::CACHE;
use crate::query::param;

// The reference's REST collection limit.
const MAX_PER_PAGE: usize = 100;

/// The JSON response for the REST search request with query string `raw`.
pub(crate) fn search(raw: &str, index: &Index) -> Response {
    match answer(raw, index) {
        Ok((body, total, pages)) => {
            let mut response = reply(200, &body);
            response.headers.extend([
                ("x-wp-total", total.to_string().into()),
                ("x-wp-totalpages", pages.to_string().into()),
            ]);
            response
        }
        Err(error) => reply(400, &error),
    }
}

// The result items with the total and page count, or the reference's error body.
fn answer(raw: &str, index: &Index) -> Result<(Value, usize, usize), Value> {
    let page = number(raw, "page", 1, usize::MAX)?;
    let per_page = number(raw, "per_page", PER_PAGE, MAX_PER_PAGE)?;
    // Only post search over every indexed type is served; term and format searches and per-type
    // filters are rejected rather than answered wrongly.
    if param(raw, "type").is_some_and(|kind| kind != b"post") {
        return Err(invalid("type"));
    }
    if param(raw, "subtype").is_some_and(|kind| kind != b"any") {
        return Err(invalid("subtype"));
    }
    let bytes = param(raw, "search");
    let text = String::from_utf8_lossy(SearchVar::parse(bytes.as_deref()).as_bytes()).into_owned();
    let found = index.search(&Terms::parse(&text), page, per_page);
    if page > found.pages && found.total > 0 {
        return Err(json!({
            "code": "rest_search_invalid_page_number",
            "message": "The page number requested is larger than the number of pages available.",
            "data": { "status": 400 },
        }));
    }
    let items = found
        .hits
        .iter()
        .map(|doc| {
            json!({
                "id": doc.id,
                "title": &*doc.title,
                "url": &*doc.permalink,
                "type": "post",
                "subtype": &*doc.kind,
            })
        })
        .collect();
    Ok((Value::Array(items), found.total, found.pages))
}

// An integer param in 1..=max, `default` when absent.
fn number(raw: &str, name: &str, default: usize, max: usize) -> Result<usize, Value> {
    let Some(value) = param(raw, name) else {
        return Ok(default);
    };
    std::str::from_utf8(&value)
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .filter(|&n| (1..=max).contains(&n))
        .ok_or_else(|| invalid(name))
}

fn invalid(name: &str) -> Value {
    json!({
        "code": "rest_invalid_param",
        "message": format!("Invalid parameter(s): {name}"),
        "data": { "status": 400, "params": { name: format!("{name} is not valid.") } },
    })
}

fn reply(status: u16, body: &Value) -> Response {
    Response {
        status,
        headers: vec![
            ("cache-control", CACHE.into()),
            ("content-type", "application/json; charset=UTF-8".into()),
        ],
        body: body.to_string().into_bytes(),
    }
}
