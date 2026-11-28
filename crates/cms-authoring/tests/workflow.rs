//! Issue #9 exit criteria through the router with in-memory fakes: an editor creates, edits,
//! previews and publishes; a contributor cannot publish; nonces guard writes; a failed statement
//! rolls back without a nudge.

use std::collections::BTreeMap;

use cms_authoring::{
    Change, Clock, Error, Method, OutboxEvent, Permissions, Ports, PostRow, PostWrite, Projector,
    Repo, Request, Response, Sessions, Site, StatusFilter, TermRow, ZERO_DATE, handle,
};
use cms_content::PostId;
use cms_render::NoCards;
use cms_users::UserId;

// ---- fakes ----------------------------------------------------------------------------------

// Everything the repository stores; cloned at BEGIN so ROLLBACK can restore it.
#[derive(Clone, Default)]
struct State {
    posts: Vec<PostRow>,
    meta: Vec<(u64, String, String)>,
    terms: Vec<TermRow>,
    links: Vec<(u64, u64)>,
    outbox: Vec<OutboxEvent>,
    options: BTreeMap<String, String>,
    next_post: u64,
    next_term: u64,
}

// An in-memory repository that logs each operation and can fail a named one.
#[derive(Default)]
struct Memory {
    state: State,
    saved: Option<State>,
    log: Vec<&'static str>,
    fail_on: Option<&'static str>,
}

impl Memory {
    fn step(&mut self, op: &'static str) -> Result<(), Error> {
        self.log.push(op);
        if self.fail_on == Some(op) {
            return Err(Error::Rejected(format!("{op} failed").into()));
        }
        Ok(())
    }

    fn post_mut(&mut self, id: PostId) -> Option<&mut PostRow> {
        self.state.posts.iter_mut().find(|row| row.id == id)
    }

    fn changes(&self) -> Vec<Change> {
        self.state.outbox.iter().map(|event| event.change).collect()
    }
}

fn fill(row: &mut PostRow, write: &PostWrite) {
    row.title.clone_from(&write.title);
    row.content.clone_from(&write.content);
    row.excerpt.clone_from(&write.excerpt);
    row.status = write.status.as_str().to_owned();
    row.name.clone_from(&write.name);
    row.date.clone_from(&write.date);
    row.date_gmt.clone_from(&write.date_gmt);
    row.modified.clone_from(&write.modified);
    row.modified_gmt.clone_from(&write.modified_gmt);
}

impl Repo for Memory {
    fn begin(&mut self) -> Result<(), Error> {
        self.step("begin")?;
        self.saved = Some(self.state.clone());
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        self.step("commit")?;
        self.saved = None;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        self.step("rollback")?;
        if let Some(saved) = self.saved.take() {
            self.state = saved;
        }
        Ok(())
    }

    fn post(&mut self, id: PostId) -> Result<Option<PostRow>, Error> {
        self.step("post")?;
        Ok(self.state.posts.iter().find(|row| row.id == id).cloned())
    }

    fn list(
        &mut self,
        filter: StatusFilter,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<PostRow>, Error> {
        self.step("list")?;
        let mut rows: Vec<PostRow> = self
            .state
            .posts
            .iter()
            .filter(|row| match filter {
                StatusFilter::All => row.status != "trash",
                StatusFilter::Only(status) => row.status == status.as_str(),
            })
            .cloned()
            .collect();
        rows.reverse();
        Ok(rows
            .into_iter()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect())
    }

    fn slug_taken(
        &mut self,
        slug: &str,
        type_name: &str,
        except: Option<PostId>,
    ) -> Result<bool, Error> {
        self.step("slug_taken")?;
        Ok(self
            .state
            .posts
            .iter()
            .any(|row| row.name == slug && row.type_name == type_name && Some(row.id) != except))
    }

    fn insert_post(&mut self, write: &PostWrite) -> Result<PostId, Error> {
        self.step("insert_post")?;
        self.state.next_post += 1;
        let id =
            PostId::new(self.state.next_post).map_err(|_zero| Error::Rejected("zero id".into()))?;
        let mut row = PostRow {
            id,
            author: write.author,
            date: String::new(),
            date_gmt: String::new(),
            content: String::new(),
            title: String::new(),
            excerpt: String::new(),
            status: String::new(),
            name: String::new(),
            type_name: "post".into(),
            modified: String::new(),
            modified_gmt: String::new(),
        };
        fill(&mut row, write);
        self.state.posts.push(row);
        Ok(id)
    }

    fn update_post(&mut self, id: PostId, write: &PostWrite) -> Result<(), Error> {
        self.step("update_post")?;
        let row = self.post_mut(id).ok_or(Error::NotFound("post"))?;
        fill(row, write);
        Ok(())
    }

    fn set_guid(&mut self, _id: PostId, _guid: &str) -> Result<(), Error> {
        self.step("set_guid")
    }

    fn meta(&mut self, id: PostId, key: &str) -> Result<Vec<String>, Error> {
        self.step("meta")?;
        Ok(self
            .state
            .meta
            .iter()
            .filter(|(post, name, _)| *post == id.as_u64() && name == key)
            .map(|(_, _, value)| value.clone())
            .collect())
    }

    fn add_meta(&mut self, id: PostId, key: &str, value: &str) -> Result<(), Error> {
        self.step("add_meta")?;
        self.state
            .meta
            .push((id.as_u64(), key.to_owned(), value.to_owned()));
        Ok(())
    }

    fn delete_meta(&mut self, id: PostId, key: &str, value: Option<&str>) -> Result<(), Error> {
        self.step("delete_meta")?;
        self.state.meta.retain(|(post, name, stored)| {
            !(*post == id.as_u64() && name == key && value.is_none_or(|value| value == stored))
        });
        Ok(())
    }

    fn terms(&mut self, taxonomy: &str) -> Result<Vec<TermRow>, Error> {
        self.step("terms")?;
        Ok(self
            .state
            .terms
            .iter()
            .filter(|term| term.taxonomy == taxonomy)
            .cloned()
            .collect())
    }

    fn post_terms(&mut self, id: PostId) -> Result<Vec<TermRow>, Error> {
        self.step("post_terms")?;
        let linked: Vec<u64> = self
            .state
            .links
            .iter()
            .filter(|(post, _)| *post == id.as_u64())
            .map(|(_, taxonomy_id)| *taxonomy_id)
            .collect();
        Ok(self
            .state
            .terms
            .iter()
            .filter(|term| linked.contains(&term.taxonomy_id))
            .cloned()
            .collect())
    }

    fn find_terms(&mut self, taxonomy: &str, ids: &[u64]) -> Result<Vec<TermRow>, Error> {
        self.step("find_terms")?;
        Ok(self
            .state
            .terms
            .iter()
            .filter(|term| term.taxonomy == taxonomy && ids.contains(&term.term_id))
            .cloned()
            .collect())
    }

    fn term_by_slug(&mut self, taxonomy: &str, slug: &str) -> Result<Option<TermRow>, Error> {
        self.step("term_by_slug")?;
        Ok(self
            .state
            .terms
            .iter()
            .find(|term| term.taxonomy == taxonomy && term.slug == slug)
            .cloned())
    }

    fn insert_term(&mut self, taxonomy: &str, name: &str, slug: &str) -> Result<TermRow, Error> {
        self.step("insert_term")?;
        self.state.next_term += 1;
        let term = TermRow {
            term_id: self.state.next_term,
            taxonomy_id: self.state.next_term,
            taxonomy: taxonomy.into(),
            name: name.into(),
            slug: slug.into(),
        };
        self.state.terms.push(term.clone());
        Ok(term)
    }

    fn link(&mut self, id: PostId, taxonomy_id: u64) -> Result<(), Error> {
        self.step("link")?;
        self.state.links.push((id.as_u64(), taxonomy_id));
        Ok(())
    }

    fn unlink(&mut self, id: PostId, taxonomy_ids: &[u64]) -> Result<(), Error> {
        self.step("unlink")?;
        self.state
            .links
            .retain(|(post, tt)| !(*post == id.as_u64() && taxonomy_ids.contains(tt)));
        Ok(())
    }

    fn recount(&mut self, _taxonomy_id: u64) -> Result<(), Error> {
        self.step("recount")
    }

    fn option(&mut self, name: &str) -> Result<Option<String>, Error> {
        self.step("option")?;
        Ok(self.state.options.get(name).cloned())
    }

    fn outbox(&mut self, event: &OutboxEvent) -> Result<(), Error> {
        self.step("outbox")?;
        self.state.outbox.push(event.clone());
        Ok(())
    }

    fn author(&mut self, _id: u64) -> Result<Option<(String, String)>, Error> {
        self.step("author")?;
        Ok(Some(("Ed Itor".into(), "editor".into())))
    }
}

// Tokens name their user; a nonce is the token joined to its action.
struct FakeSessions;

impl Sessions for FakeSessions {
    fn user(&mut self, token: &str) -> Result<Option<UserId>, Error> {
        let id = match token {
            "editor-token" => 1,
            "contrib-token" => 2,
            _ => return Ok(None),
        };
        Ok(UserId::new(id).ok())
    }

    fn nonce(&self, token: &str, action: &str) -> String {
        format!("{token}.{action}")
    }

    fn verify(&self, token: &str, action: &str, nonce: &str) -> bool {
        nonce == self.nonce(token, action)
    }
}

// User 1 is an editor, user 2 a contributor (the reference's default role capabilities).
struct Roles;

impl Permissions for Roles {
    fn has(&mut self, user: UserId, required: &[&[u8]]) -> Result<bool, Error> {
        let held: &[&[u8]] = match user.as_u64() {
            1 => &[
                b"edit_posts",
                b"edit_others_posts",
                b"edit_published_posts",
                b"publish_posts",
                b"delete_posts",
                b"delete_others_posts",
                b"delete_published_posts",
            ],
            2 => &[b"edit_posts", b"delete_posts"],
            _ => &[],
        };
        Ok(required.iter().all(|cap| held.contains(cap)))
    }
}

#[derive(Default)]
struct Nudges(u32);

impl Projector for Nudges {
    fn nudge(&mut self) -> Result<(), Error> {
        self.0 += 1;
        Ok(())
    }
}

struct Fixed;

impl Clock for Fixed {
    fn now(&self) -> i64 {
        // 2026-11-27 12:00:00 UTC.
        1_795_780_800
    }
}

fn site() -> Site {
    Site {
        theme: cms_render::Site {
            title: "Example".into(),
            description: "A blog".into(),
            home: "https://example.test/blog/".into(),
            base: "/blog/".into(),
            language: "en-US".into(),
            posts_per_page: 10,
            nav: Vec::new(),
        },
        gmt_offset: 0,
    }
}

fn request(
    method: Method,
    path: &str,
    token: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Request {
    Request {
        method,
        path: path.into(),
        query: String::new(),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        cookies: vec![format!("fp_session={token}")],
        body: body.as_bytes().to_vec(),
    }
}

fn send(repo: &mut Memory, nudges: &mut Nudges, request: &Request) -> Response {
    let ports = Ports {
        repo,
        sessions: &mut FakeSessions,
        permissions: &mut Roles,
        projector: nudges,
        clock: &Fixed,
        cards: &mut NoCards,
    };
    handle(&site(), ports, request)
}

fn rest(method: Method, path: &str, token: &str, body: &str) -> Request {
    let nonce = format!("{token}.fp_rest");
    request(method, path, token, &[("x-fp-nonce", nonce.as_str())], body)
}

// ---- tests ----------------------------------------------------------------------------------

#[test]
fn an_editor_creates_edits_previews_and_publishes() {
    let mut repo = Memory::default();
    let mut nudges = Nudges::default();

    // Create a draft over REST: drafts keep an empty slug.
    let created = send(
        &mut repo,
        &mut nudges,
        &rest(
            Method::Post,
            "/blog/api/posts",
            "editor-token",
            r#"{"title":"Hello World","content":"<p>Hi there</p>","tags":[]}"#,
        ),
    );
    assert_eq!(
        created.status,
        201,
        "{:?}",
        String::from_utf8_lossy(&created.body)
    );
    assert_eq!(repo.state.posts.len(), 1);
    assert_eq!(repo.state.posts[0].status, "draft");
    assert_eq!(repo.state.posts[0].name, "");
    assert_eq!(repo.state.posts[0].date_gmt, ZERO_DATE);

    // Edit it.
    let edited = send(
        &mut repo,
        &mut nudges,
        &rest(
            Method::Put,
            "/blog/api/posts/1",
            "editor-token",
            r#"{"title":"Hello Again"}"#,
        ),
    );
    assert_eq!(edited.status, 200);
    assert_eq!(repo.state.posts[0].title, "Hello Again");

    // Preview from the stored row, never cached.
    let preview = send(
        &mut repo,
        &mut nudges,
        &request(
            Method::Get,
            "/blog/admin/preview/1",
            "editor-token",
            &[],
            "",
        ),
    );
    assert_eq!(preview.status, 200);
    assert_eq!(preview.header("cache-control"), Some("no-store"));
    assert!(String::from_utf8_lossy(&preview.body).contains("Hello Again"));

    // Publish through the admin form: the slug comes from the title, dates are stamped.
    let published = send(
        &mut repo,
        &mut nudges,
        &request(
            Method::Post,
            "/blog/admin/posts/1",
            "editor-token",
            &[],
            "_fpnonce=editor-token.fp_admin&do=publish&title=Hello+Again&categories_sent=1",
        ),
    );
    assert_eq!(published.status, 303);
    assert_eq!(
        published.header("location"),
        Some("/blog/admin/posts/1/edit?message=published")
    );
    let row = &repo.state.posts[0];
    assert_eq!(
        (row.status.as_str(), row.name.as_str()),
        ("publish", "hello-again")
    );
    assert_eq!(row.date_gmt, "2026-11-27 12:00:00");

    assert_eq!(
        repo.changes(),
        [Change::Created, Change::Updated, Change::Published]
    );
    let event = repo.state.outbox[2].to_event();
    assert_eq!(event.change(), cms_outbox::Change::Saved);
    assert_eq!(
        event.payload().get("action").and_then(|v| v.as_str()),
        Some("published")
    );
    assert_eq!(
        event.payload().get("slug").and_then(|v| v.as_str()),
        Some("hello-again")
    );
    assert_eq!(nudges.0, 3);
    // Each write was one transaction: three begins, three commits, no rollback.
    let count = |op: &str| repo.log.iter().filter(|logged| **logged == op).count();
    assert_eq!(
        (count("begin"), count("commit"), count("rollback")),
        (3, 3, 0)
    );

    // A second post with the same title takes a numeric suffix.
    let twin = send(
        &mut repo,
        &mut nudges,
        &rest(
            Method::Post,
            "/blog/api/posts",
            "editor-token",
            r#"{"title":"Hello Again","status":"publish"}"#,
        ),
    );
    assert_eq!(twin.status, 201);
    assert_eq!(repo.state.posts[1].name, "hello-again-2");

    // Renaming a published post keeps the old slug for redirects and names it in the payload.
    let renamed = send(
        &mut repo,
        &mut nudges,
        &rest(
            Method::Put,
            "/blog/api/posts/1",
            "editor-token",
            r#"{"slug":"Hello Renamed"}"#,
        ),
    );
    assert_eq!(renamed.status, 200);
    assert_eq!(repo.state.posts[0].name, "hello-renamed");
    assert_eq!(
        repo.state.meta,
        [(1, "_wp_old_slug".to_owned(), "hello-again".to_owned())]
    );
    let last = repo.state.outbox.last().map(OutboxEvent::to_event);
    assert_eq!(
        last.as_ref()
            .and_then(|e| e.payload().get("old_slug"))
            .and_then(|v| v.as_str()),
        Some("hello-again")
    );

    // Unpublish, then trash.
    let unpublished = send(
        &mut repo,
        &mut nudges,
        &request(
            Method::Post,
            "/blog/admin/posts/1",
            "editor-token",
            &[],
            "_fpnonce=editor-token.fp_admin&do=unpublish",
        ),
    );
    assert_eq!(unpublished.status, 303);
    assert_eq!(repo.state.posts[0].status, "draft");
    let trashed = send(
        &mut repo,
        &mut nudges,
        &rest(Method::Delete, "/blog/api/posts/1", "editor-token", ""),
    );
    assert_eq!(trashed.status, 200);
    assert_eq!(repo.state.posts[0].status, "trash");
    assert_eq!(
        repo.changes()[3..],
        [
            Change::Published,
            Change::Updated,
            Change::Unpublished,
            Change::Trashed
        ]
    );
    assert_eq!(repo.changes()[5..], [Change::Unpublished, Change::Trashed]);
    assert_eq!(nudges.0, 7);

    // The list shows the remaining post.
    let list = send(
        &mut repo,
        &mut nudges,
        &request(Method::Get, "/blog/admin/", "editor-token", &[], ""),
    );
    assert_eq!(list.status, 200);
    let html = String::from_utf8_lossy(&list.body);
    assert!(html.contains("admin/posts/2/edit") && !html.contains("admin/posts/1/edit"));
}

#[test]
fn a_contributor_cannot_publish_and_nothing_is_written() {
    let mut repo = Memory::default();
    let mut nudges = Nudges::default();
    let refused = send(
        &mut repo,
        &mut nudges,
        &rest(
            Method::Post,
            "/blog/api/posts",
            "contrib-token",
            r#"{"title":"Mine","status":"publish"}"#,
        ),
    );
    assert_eq!(refused.status, 403);
    assert!(repo.state.posts.is_empty() && repo.state.outbox.is_empty());
    assert!(!repo.log.contains(&"begin"));
    assert_eq!(nudges.0, 0);

    // A draft is fine, but publishing it later is refused too.
    let draft = send(
        &mut repo,
        &mut nudges,
        &rest(
            Method::Post,
            "/blog/api/posts",
            "contrib-token",
            r#"{"title":"Mine"}"#,
        ),
    );
    assert_eq!(draft.status, 201);
    let before = repo.state.outbox.len();
    let publish = send(
        &mut repo,
        &mut nudges,
        &rest(
            Method::Put,
            "/blog/api/posts/1",
            "contrib-token",
            r#"{"status":"publish"}"#,
        ),
    );
    assert_eq!(publish.status, 403);
    assert_eq!(repo.state.posts[0].status, "draft");
    assert_eq!(repo.state.outbox.len(), before);
}

#[test]
fn a_bad_or_missing_nonce_is_rejected_before_any_write() {
    let mut repo = Memory::default();
    let mut nudges = Nudges::default();
    let body = r#"{"title":"X"}"#;
    let missing = request(Method::Post, "/blog/api/posts", "editor-token", &[], body);
    let wrong = request(
        Method::Post,
        "/blog/api/posts",
        "editor-token",
        &[("x-fp-nonce", "contrib-token.fp_rest")],
        body,
    );
    // An admin nonce does not authorize REST, and a form without `_fpnonce` is refused.
    let other_action = request(
        Method::Post,
        "/blog/api/posts",
        "editor-token",
        &[("x-fp-nonce", "editor-token.fp_admin")],
        body,
    );
    let form = request(
        Method::Post,
        "/blog/admin/posts",
        "editor-token",
        &[],
        "do=draft&title=X",
    );
    for refused in [missing, wrong, other_action, form] {
        let response = send(&mut repo, &mut nudges, &refused);
        assert_eq!(response.status, 403, "{}", refused.path);
    }
    assert!(repo.state.posts.is_empty() && repo.state.outbox.is_empty());
    assert!(repo.log.iter().all(|op| *op != "begin"));
    assert_eq!(nudges.0, 0);

    // No session at all: REST answers 401, the admin redirects to the login form.
    let anonymous = request(Method::Get, "/blog/admin/", "", &[], "");
    let response = send(&mut repo, &mut nudges, &anonymous);
    assert_eq!(response.status, 302);
    assert_eq!(
        response.header("location"),
        Some("/blog/login?redirect_to=/blog/admin/")
    );
}

#[test]
fn a_failed_statement_rolls_back_without_a_nudge() {
    for failing in ["outbox", "link", "insert_post"] {
        let mut repo = Memory {
            fail_on: Some(failing),
            ..Memory::default()
        };
        repo.state.terms.push(TermRow {
            term_id: 1,
            taxonomy_id: 1,
            taxonomy: "category".into(),
            name: "News".into(),
            slug: "news".into(),
        });
        repo.state.next_term = 1;
        let mut nudges = Nudges::default();
        let response = send(
            &mut repo,
            &mut nudges,
            &rest(
                Method::Post,
                "/blog/api/posts",
                "editor-token",
                r#"{"title":"T","categories":[1],"status":"publish"}"#,
            ),
        );
        assert_eq!(response.status, 500, "{failing}");
        assert!(repo.state.posts.is_empty(), "{failing}");
        assert!(repo.state.links.is_empty() && repo.state.outbox.is_empty());
        assert!(repo.log.contains(&"rollback") && !repo.log.contains(&"commit"));
        assert_eq!(nudges.0, 0, "{failing}");
    }
}

#[test]
fn editor_workflows_check_capabilities_per_post() {
    let mut repo = Memory::default();
    let mut nudges = Nudges::default();
    // The editor's published post: a contributor may neither edit nor trash it.
    let created = send(
        &mut repo,
        &mut nudges,
        &rest(
            Method::Post,
            "/blog/api/posts",
            "editor-token",
            r#"{"title":"Theirs","status":"publish"}"#,
        ),
    );
    assert_eq!(created.status, 201);
    let writes = repo.state.outbox.len();
    for refused in [
        rest(
            Method::Put,
            "/blog/api/posts/1",
            "contrib-token",
            r#"{"title":"Mine now"}"#,
        ),
        rest(Method::Delete, "/blog/api/posts/1", "contrib-token", ""),
        request(
            Method::Get,
            "/blog/admin/posts/1/edit",
            "contrib-token",
            &[],
            "",
        ),
    ] {
        let response = send(&mut repo, &mut nudges, &refused);
        assert_eq!(response.status, 403, "{}", refused.path);
    }
    assert_eq!(repo.state.outbox.len(), writes);
    assert_eq!(repo.state.posts[0].title, "Theirs");
}
