//! End-to-end read path over in-memory ports: a published post served from route item + fragments.

use std::collections::HashMap;

use cms_readmodel::{
    BasePath, BlobKey, Blobs, Content, Method, PortError, Request, Response, Route, RouteKey,
    RouteTable, serve,
};

#[derive(Default)]
struct Memory {
    routes: HashMap<RouteKey, Route>,
    blobs: HashMap<BlobKey, Vec<u8>>,
    fail: bool,
}

impl RouteTable for Memory {
    fn route(&mut self, key: &RouteKey) -> Result<Option<Route>, PortError> {
        if self.fail {
            return Err(PortError::Store("GetItem".into()));
        }
        Ok(self.routes.get(key).cloned())
    }
}

struct Objects(HashMap<BlobKey, Vec<u8>>);

impl Blobs for Objects {
    fn get(&mut self, key: &BlobKey) -> Result<Option<Vec<u8>>, PortError> {
        Ok(self.0.get(key).cloned())
    }
}

fn route(status: u16, content: Content) -> Route {
    Route {
        status,
        content_type: "text/html; charset=UTF-8".into(),
        etag: "\"e1\"".into(),
        last_modified: "Fri, 27 Nov 2026 00:00:00 GMT".into(),
        content,
    }
}

fn site() -> Option<(BasePath, Memory)> {
    let mut memory = Memory::default();
    let page = |body: &str| Content::Page {
        shell: "fragments/shell/s1.html".into(),
        body: body.into(),
    };
    memory
        .routes
        .insert("/blog/hello/".into(), route(200, page("rendered/p1.html")));
    memory
        .routes
        .insert("/blog/404".into(), route(404, page("rendered/nf.html")));
    memory.routes.insert(
        "/blog/old-hello/".into(),
        route(
            200,
            Content::Redirect {
                location: "/blog/hello/".into(),
            },
        ),
    );
    memory
        .routes
        .insert("/blog/removed/".into(), route(200, Content::Gone));
    memory.routes.insert(
        "/blog/broken/".into(),
        route(200, page("rendered/missing.html")),
    );
    memory.blobs.insert(
        "fragments/shell/s1.html".into(),
        b"<html><main><!--fp:body--></main></html>".to_vec(),
    );
    memory
        .blobs
        .insert("rendered/p1.html".into(), b"<h1>Hello</h1>".to_vec());
    memory
        .blobs
        .insert("rendered/nf.html".into(), b"<h1>Not found</h1>".to_vec());
    BasePath::new("/blog/").ok().map(|base| (base, memory))
}

fn get(path: &str) -> Request {
    Request {
        method: Method::Get,
        path: path.into(),
        if_none_match: None,
    }
}

fn run(memory: &mut Memory, base: &BasePath, request: &Request) -> Result<Response, PortError> {
    let mut objects = Objects(std::mem::take(&mut memory.blobs));
    let result = serve(base, request, memory, &mut objects);
    memory.blobs = objects.0;
    result
}

fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| &**value)
}

#[test]
fn a_published_post_is_the_shell_with_its_body() {
    let site = site();
    assert!(site.is_some());
    let Some((base, mut memory)) = site else {
        return;
    };
    let response = run(&mut memory, &base, &get("/blog/hello/"));
    assert!(response.is_ok());
    if let Ok(response) = response {
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"<html><main><h1>Hello</h1></main></html>");
        assert_eq!(header(&response, "etag"), Some("\"e1\""));
        assert_eq!(
            header(&response, "cache-control"),
            Some("public, max-age=60, s-maxage=86400")
        );
        assert_eq!(
            header(&response, "content-type"),
            Some("text/html; charset=UTF-8")
        );
    }
}

#[test]
fn head_and_matching_etag_send_no_body() {
    let site = site();
    assert!(site.is_some());
    let Some((base, mut memory)) = site else {
        return;
    };
    let mut head = get("/blog/hello/");
    head.method = Method::Head;
    let response = run(&mut memory, &base, &head);
    assert!(response.is_ok_and(|r| r.status == 200 && r.body.is_empty()));
    let mut cached = get("/blog/hello/");
    cached.if_none_match = Some("\"e1\"".into());
    let response = run(&mut memory, &base, &cached);
    assert!(response.is_ok_and(|r| r.status == 304 && r.body.is_empty()));
}

#[test]
fn unknown_paths_serve_the_rendered_not_found_page() {
    let site = site();
    assert!(site.is_some());
    let Some((base, mut memory)) = site else {
        return;
    };
    let response = run(&mut memory, &base, &get("/blog/nope/"));
    assert!(response.is_ok_and(|r| r.status == 404 && r.body.ends_with(b"</html>")));
    memory.routes.remove(&RouteKey::from("/blog/404"));
    let response = run(&mut memory, &base, &get("/blog/nope/"));
    assert!(response.is_ok_and(|r| r.status == 404 && r.body.is_empty()));
    let response = run(&mut memory, &base, &get("/elsewhere/"));
    assert!(response.is_ok_and(|r| r.status == 404));
}

#[test]
fn redirects_gone_and_methods() {
    let site = site();
    assert!(site.is_some());
    let Some((base, mut memory)) = site else {
        return;
    };
    let response = run(&mut memory, &base, &get("/blog/old-hello/"));
    assert!(response.as_ref().is_ok_and(|r| r.status == 301));
    if let Ok(response) = response {
        assert_eq!(header(&response, "location"), Some("/blog/hello/"));
    }
    let response = run(&mut memory, &base, &get("/blog/hello"));
    assert!(response.is_ok_and(|r| r.status == 301));
    let response = run(&mut memory, &base, &get("/blog/removed/"));
    assert!(response.is_ok_and(|r| r.status == 410));
    let mut post = get("/blog/hello/");
    post.method = Method::Other;
    let response = run(&mut memory, &base, &post);
    assert!(response.as_ref().is_ok_and(|r| r.status == 405));
    if let Ok(response) = response {
        assert_eq!(header(&response, "allow"), Some("GET, HEAD"));
    }
}

#[test]
fn inconsistent_read_models_and_store_failures_are_errors() {
    let site = site();
    assert!(site.is_some());
    let Some((base, mut memory)) = site else {
        return;
    };
    let response = run(&mut memory, &base, &get("/blog/broken/"));
    assert_eq!(
        response,
        Err(PortError::MissingBlob("rendered/missing.html".into()))
    );
    memory.blobs.insert(
        "fragments/shell/s1.html".into(),
        b"<html>no mark</html>".to_vec(),
    );
    let response = run(&mut memory, &base, &get("/blog/hello/"));
    assert_eq!(
        response.map_err(|e| e.to_string()),
        Err("page shell fragments/shell/s1.html has no body mark".to_owned())
    );
    memory.fail = true;
    let response = run(&mut memory, &base, &get("/blog/hello/"));
    assert_eq!(
        response.map_err(|e| e.to_string()),
        Err("read model store failed: GetItem".to_owned())
    );
}
