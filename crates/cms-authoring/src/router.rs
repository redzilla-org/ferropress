//! Routes admin and REST requests to the editor workflows.

use std::fmt;

use cms_content::PostId;
use cms_render::CardSource;
use cms_users::UserId;
use serde_json::{Value, json};

use crate::editor::{Editor, PostInput, Saved, Site, Tags};
use crate::http::{Method, Request, Response, encode, field, form};
use crate::model::{Change, PostRow, Status, StatusFilter, TermRow};
use crate::pages::{self, HTML, SCRIPT};
use crate::{Clock, Error, Permissions, Projector, Repo, Sessions};

/// The session cookie the auth function sets.
pub const SESSION_COOKIE: &str = "fp_session";
/// Nonce action for admin form posts (sent as the `_fpnonce` field).
pub const ADMIN_ACTION: &str = "fp_admin";
/// Nonce action for REST writes (sent as the `x-fp-nonce` header).
pub const REST_ACTION: &str = "fp_rest";

const JSON: &str = "application/json; charset=UTF-8";

/// Everything one request reaches through ports.
pub struct Ports<'a> {
    /// Storage (one connection; writes open their own transaction).
    pub repo: &'a mut dyn Repo,
    /// Sessions and nonces.
    pub sessions: &'a mut dyn Sessions,
    /// Capability checks.
    pub permissions: &'a mut dyn Permissions,
    /// The projector nudge.
    pub projector: &'a mut dyn Projector,
    /// Wall clock.
    pub clock: &'a dyn Clock,
    /// Card data for `[[zip]]` / `[[id]]` markers in previews.
    pub cards: &'a mut dyn CardSource,
}

impl fmt::Debug for Ports<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ports").finish_non_exhaustive()
    }
}

/// Answers one request under `site.theme.base` (`admin/...` and `api/...`).
#[must_use]
pub fn handle(site: &Site, ports: Ports<'_>, request: &Request) -> Response {
    let mut response = route(site, ports, request);
    if request.method == Method::Head {
        response.body.clear();
    }
    response
}

fn route(site: &Site, ports: Ports<'_>, request: &Request) -> Response {
    let Ports {
        repo,
        sessions,
        permissions,
        projector,
        clock,
        cards,
    } = ports;
    let Some(rest) = request.path.strip_prefix(&*site.theme.base) else {
        return Response::new(404, JSON, Vec::new());
    };
    // The script holds no data, so it is served without a session.
    if rest == "admin/editor.js" && request.method.is_read() {
        return Response::new(
            200,
            "text/javascript; charset=UTF-8",
            SCRIPT.as_bytes().to_vec(),
        );
    }
    let api = rest.starts_with("api/");
    let fail = |error: &Error| {
        if api {
            json_error(error)
        } else {
            Response::new(
                error.status(),
                HTML,
                pages::error(site, error.status(), &error.to_string()),
            )
        }
    };
    let token = request.cookie(SESSION_COOKIE).unwrap_or_default();
    let user = if token.is_empty() {
        Ok(None)
    } else {
        sessions.user(token)
    };
    let user = match user {
        Ok(Some(user)) => user,
        Ok(None) if api => {
            return json_message(401, "fp_not_logged_in", "You are not logged in.");
        }
        Ok(None) => {
            let target = format!(
                "{}login?redirect_to={}",
                site.theme.base,
                encode(&request.path)
            );
            return Response::redirect(302, &target);
        }
        Err(error) => return fail(&error),
    };
    // Every write must carry a nonce bound to this session, so a cross-site form cannot post.
    let pairs = if api { Vec::new() } else { form(&request.body) };
    if !request.method.is_read() {
        let (action, nonce) = if api {
            (REST_ACTION, request.header("x-fp-nonce"))
        } else {
            (ADMIN_ACTION, field(&pairs, "_fpnonce"))
        };
        if !nonce.is_some_and(|nonce| sessions.verify(token, action, nonce)) {
            return fail(&Error::Forbidden("a valid nonce"));
        }
    }
    let nonce = sessions.nonce(token, ADMIN_ACTION);
    let mut editor = Editor::new(repo, permissions, projector, clock, site);
    let segments: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    let query = form(request.query.as_bytes());
    let outcome = if api {
        rest_api(&mut editor, user, request, &segments, &query)
    } else {
        let call = Call {
            user,
            request,
            pairs: &pairs,
            query: &query,
            nonce: &nonce,
            cards,
        };
        admin(&mut editor, call, &segments)
    };
    outcome.unwrap_or_else(|error| fail(&error))
}

struct Call<'r> {
    user: UserId,
    request: &'r Request,
    pairs: &'r [(String, String)],
    query: &'r [(String, String)],
    nonce: &'r str,
    cards: &'r mut dyn CardSource,
}

fn admin(editor: &mut Editor<'_>, call: Call<'_>, segments: &[&str]) -> Result<Response, Error> {
    let Call {
        user,
        request,
        pairs,
        query,
        nonce,
        cards,
    } = call;
    let site = editor.site();
    let base = site.theme.base.clone();
    let read = request.method.is_read();
    let post = request.method == Method::Post;
    match segments {
        ["admin"] | ["admin", "posts"] if read => {
            let filter = status_filter(field(query, "post_status"))?;
            let number = page_number(field(query, "paged"))?;
            let page = editor.list(user, filter, number)?;
            Ok(html(200, pages::list(editor.site(), filter, number, &page)))
        }
        ["admin", "posts", "new"] if read => {
            if !editor.can(user, "edit_posts")? {
                return Err(Error::Forbidden("edit_posts"));
            }
            let categories = editor.categories()?;
            Ok(html(
                200,
                pages::editor(editor.site(), None, &categories, nonce, None),
            ))
        }
        ["admin", "posts", id, "edit"] if read => {
            let (row, terms) = editor.load(user, post_id(id)?)?;
            let categories = editor.categories()?;
            let notice = field(query, "message").and_then(notice);
            Ok(html(
                200,
                pages::editor(
                    editor.site(),
                    Some((&row, &terms)),
                    &categories,
                    nonce,
                    notice,
                ),
            ))
        }
        ["admin", "preview", id] if read => {
            let bytes = editor.preview(user, post_id(id)?, cards)?;
            let mut response = html(200, bytes);
            response
                .headers
                .push(("x-robots-tag", "noindex, nofollow".to_owned()));
            Ok(response)
        }
        ["admin", "posts"] if post => {
            let saved = editor.save(user, None, &form_input(pairs)?)?;
            Ok(after_save(&base, &saved))
        }
        ["admin", "posts", id] if post => {
            let saved = editor.save(user, Some(post_id(id)?), &form_input(pairs)?)?;
            Ok(after_save(&base, &saved))
        }
        _ => Err(Error::NotFound("admin page")),
    }
}

// Post/redirect/get: a successful save answers 303 so a reload never resubmits.
fn after_save(base: &str, saved: &Saved) -> Response {
    let target = if saved.change == Change::Trashed {
        format!("{base}admin/?post_status=trash")
    } else {
        format!(
            "{base}admin/posts/{}/edit?message={}",
            saved.id.as_u64(),
            saved.change.as_str()
        )
    };
    Response::redirect(303, &target)
}

fn notice(message: &str) -> Option<&'static str> {
    match message {
        "created" | "updated" => Some("Post saved."),
        "published" => Some("Post published."),
        "unpublished" => Some("Post moved back to draft."),
        _ => None,
    }
}

// The editor's buttons name the target status; unpublishing is a save as draft.
fn form_input(pairs: &[(String, String)]) -> Result<PostInput, Error> {
    let text = |name: &str| field(pairs, name).map(str::to_owned);
    let status = match field(pairs, "do").unwrap_or("draft") {
        "draft" | "unpublish" => Status::Draft,
        "pending" => Status::Pending,
        "publish" => Status::Publish,
        "trash" => Status::Trash,
        _ => return Err(Error::Invalid("unknown action".into())),
    };
    let categories = if field(pairs, "categories_sent").is_some() {
        let ids = pairs
            .iter()
            .filter(|(key, _)| key == "category")
            .map(|(_, value)| term_id(value))
            .collect::<Result<Vec<u64>, Error>>()?;
        Some(ids)
    } else {
        None
    };
    let tags = field(pairs, "tags").map(|names| {
        Tags::Names(
            names
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    });
    Ok(PostInput {
        title: text("title"),
        content: text("content"),
        excerpt: text("excerpt"),
        slug: text("slug"),
        status: Some(status),
        categories,
        tags,
    })
}

fn rest_api(
    editor: &mut Editor<'_>,
    user: UserId,
    request: &Request,
    segments: &[&str],
    query: &[(String, String)],
) -> Result<Response, Error> {
    let method = request.method;
    match segments {
        ["api", "posts"] if method.is_read() => {
            let filter = status_filter(field(query, "status"))?;
            let number = page_number(field(query, "page"))?;
            let page = editor.list(user, filter, number)?;
            let items: Vec<Value> = page.posts.iter().map(|row| post_json(row, None)).collect();
            let mut response = json_response(200, &Value::Array(items));
            response
                .headers
                .push(("x-fp-has-next", page.has_next.to_string()));
            Ok(response)
        }
        ["api", "posts"] if method == Method::Post => {
            let saved = editor.save(user, None, &json_input(&request.body)?)?;
            saved_json(editor, user, &saved, 201)
        }
        ["api", "posts", id] if method.is_read() => {
            let (row, terms) = editor.load(user, post_id(id)?)?;
            Ok(json_response(200, &post_json(&row, Some(&terms))))
        }
        ["api", "posts", id] if matches!(method, Method::Put | Method::Post) => {
            let saved = editor.save(user, Some(post_id(id)?), &json_input(&request.body)?)?;
            saved_json(editor, user, &saved, 200)
        }
        ["api", "posts", id] if method == Method::Delete => {
            let saved = editor.trash(user, post_id(id)?)?;
            saved_json(editor, user, &saved, 200)
        }
        _ => Err(Error::NotFound("route")),
    }
}

fn saved_json(
    editor: &mut Editor<'_>,
    user: UserId,
    saved: &Saved,
    status: u16,
) -> Result<Response, Error> {
    let (row, terms) = editor.load(user, saved.id)?;
    Ok(json_response(status, &post_json(&row, Some(&terms))))
}

fn json_input(body: &[u8]) -> Result<PostInput, Error> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| Error::Invalid(format!("body is not JSON: {error}").into()))?;
    let Value::Object(object) = value else {
        return Err(Error::Invalid("body must be a JSON object".into()));
    };
    let text = |name: &str| -> Result<Option<String>, Error> {
        match object.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(Error::Invalid(format!("{name} must be a string").into())),
        }
    };
    let ids = |name: &str| -> Result<Option<Vec<u64>>, Error> {
        match object.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_u64()
                        .ok_or_else(|| Error::Invalid(format!("{name} must hold term ids").into()))
                })
                .collect::<Result<Vec<u64>, Error>>()
                .map(Some),
            Some(_) => Err(Error::Invalid(format!("{name} must be an array").into())),
        }
    };
    let status = text("status")?
        .map(|status| Status::parse(&status).ok_or_else(|| Error::Invalid("unknown status".into())))
        .transpose()?;
    Ok(PostInput {
        title: text("title")?,
        content: text("content")?,
        excerpt: text("excerpt")?,
        slug: text("slug")?,
        status,
        categories: ids("categories")?,
        tags: ids("tags")?.map(Tags::Ids),
    })
}

fn post_json(row: &PostRow, terms: Option<&[TermRow]>) -> Value {
    let mut value = json!({
        "id": row.id.as_u64(),
        "author": row.author,
        "status": row.status,
        "slug": row.name,
        "title": row.title,
        "content": row.content,
        "excerpt": row.excerpt,
        "date": row.date,
        "date_gmt": row.date_gmt,
        "modified": row.modified,
        "modified_gmt": row.modified_gmt,
    });
    if let (Some(terms), Value::Object(object)) = (terms, &mut value) {
        let of = |taxonomy: &str| -> Vec<u64> {
            terms
                .iter()
                .filter(|term| term.taxonomy == taxonomy)
                .map(|term| term.term_id)
                .collect()
        };
        object.insert("categories".into(), json!(of("category")));
        object.insert("tags".into(), json!(of("post_tag")));
    }
    value
}

fn status_filter(value: Option<&str>) -> Result<StatusFilter, Error> {
    match value {
        None | Some("" | "all") => Ok(StatusFilter::All),
        Some(text) => Status::parse(text)
            .map(StatusFilter::Only)
            .ok_or_else(|| Error::Invalid("unknown status".into())),
    }
}

fn page_number(value: Option<&str>) -> Result<u64, Error> {
    match value {
        None | Some("") => Ok(1),
        Some(text) => text
            .parse::<u64>()
            .ok()
            .filter(|page| *page > 0)
            .ok_or_else(|| Error::Invalid("page must be a positive integer".into())),
    }
}

fn post_id(text: &str) -> Result<PostId, Error> {
    text.parse::<u64>()
        .ok()
        .and_then(|id| PostId::new(id).ok())
        .ok_or(Error::NotFound("post"))
}

fn term_id(text: &str) -> Result<u64, Error> {
    text.parse::<u64>()
        .map_err(|_not_a_number| Error::Invalid("category ids must be integers".into()))
}

fn html(status: u16, body: Vec<u8>) -> Response {
    Response::new(status, HTML, body)
}

fn json_response(status: u16, value: &Value) -> Response {
    Response::new(status, JSON, value.to_string().into_bytes())
}

fn json_message(status: u16, code: &str, message: &str) -> Response {
    json_response(
        status,
        &json!({ "code": code, "message": message, "data": { "status": status } }),
    )
}

fn json_error(error: &Error) -> Response {
    let code = match error {
        Error::Forbidden(_) => "fp_forbidden",
        Error::NotFound(_) => "fp_not_found",
        Error::Invalid(_) => "fp_invalid",
        Error::Conflict => "fp_conflict",
        Error::Unavailable => "fp_unavailable",
        _ => "fp_error",
    };
    json_message(error.status(), code, &error.to_string())
}
