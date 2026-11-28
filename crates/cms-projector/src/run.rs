//! One projector run: fetch, plan, render, write in order, invalidate, delete.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use cms_db::{Store, TablePrefix};
use cms_outbox::{Aggregate, Row};
use cms_readmodel::{BlobKey, Content, Route, RouteKey};
use cms_render::{CardSource, Rendered};
use sha2::{Digest, Sha256};

use crate::index::{Index, Rendering};
use crate::plan::{self, Affected};
use crate::{
    Applied, BATCH, BlobWriter, Clock, Invalidator, Item, ItemKey, Nudger, Paths, PostDoc,
    ProjectError, Renderer, RouteWriter, Source, Target, imf_fixdate, version,
};

/// More paths than this in one batch invalidate the whole base path instead; `CloudFront` caps
/// one request at 3,000 paths.
const MAX_PATHS: usize = 1000;

/// Sitemap file families `cms-render` can emit, for removing a shard that no longer exists.
const SITEMAP_FAMILIES: [&str; 5] = [
    "posts-post",
    "posts-page",
    "taxonomies-category",
    "taxonomies-post_tag",
    "users",
];

/// The projector's collaborators for one run.
#[expect(
    missing_debug_implementations,
    reason = "holds trait-object ports that need not implement Debug"
)]
pub struct Projector<'a, S: Store> {
    /// Database connection holding the outbox (autocommit; no transaction is needed).
    pub store: &'a mut S,
    /// Site table prefix.
    pub prefix: &'a TablePrefix,
    /// Route key layout.
    pub paths: &'a Paths,
    /// Current state.
    pub source: &'a mut dyn Source,
    /// The theme.
    pub theme: &'a dyn Renderer,
    /// Listing card data for `[[...]]` markers.
    pub cards: &'a mut dyn CardSource,
    /// Object store.
    pub blobs: &'a mut dyn BlobWriter,
    /// Read-model table.
    pub routes: &'a mut dyn RouteWriter,
    /// Edge cache.
    pub invalidator: &'a mut dyn Invalidator,
    /// Time source.
    pub clock: &'a dyn Clock,
    /// Requests a follow-up run when this one filled its batch.
    pub nudger: &'a mut dyn Nudger,
}

/// What one run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Outbox rows projected and deleted.
    pub rows: usize,
    /// Age in seconds of the oldest row at the start of the run (0 when the outbox was empty).
    pub age_seconds: u64,
    /// Objects put.
    pub blobs: usize,
    /// Item writes that took effect.
    pub written: usize,
    /// Item writes skipped because a same-or-newer version was stored.
    pub stale: usize,
    /// Card markers rendered as nothing because the source had no card.
    pub unresolved: usize,
    /// Paths sent in the one invalidation (empty when nothing was sent).
    pub invalidated: Vec<Box<str>>,
    /// Whether a follow-up run was requested because the batch was full.
    pub nudged: bool,
}

impl Report {
    const fn count(&mut self, applied: Applied) {
        match applied {
            Applied::Written => self.written += 1,
            Applied::Stale => self.stale += 1,
        }
    }
}

// The planned writes, kept apart so they can be executed in the required order.
#[derive(Default)]
struct Writes {
    blobs: BTreeMap<Box<str>, Rendered>,
    puts: Vec<Item>,
    deletes: Vec<ItemKey>,
    routes: BTreeSet<Box<str>>,
    bodies: HashMap<u64, BlobKey>,
    unresolved: usize,
}

// What every route written in one run shares.
struct Page<'s> {
    shell: &'s Rendered,
    shell_key: BlobKey,
    modified: Box<str>,
}

impl<S: Store> Projector<'_, S> {
    /// Projects up to [`BATCH`] outbox rows.
    ///
    /// The rows are deleted only after every object, item and the invalidation succeeded, so a
    /// failure anywhere leaves them for the next run, which repeats the work harmlessly.
    ///
    /// # Errors
    ///
    /// [`ProjectError`] when the outbox, a port, the card source or page assembly fails.
    pub fn run_once(&mut self) -> Result<Report, ProjectError> {
        let now = self.clock.now_micros();
        let rows = cms_outbox::fetch(self.store, self.prefix, BATCH)?;
        let (Some(newest), Some(oldest)) = (
            rows.last(),
            rows.iter().map(|row| row.created_micros).min(),
        ) else {
            return Ok(Report::default());
        };
        let version = version(now, &newest.id);
        let site = self.source.site()?;
        let posts = self.source.published()?;
        let paths = self.paths;
        let index = Index::new(&site, paths, self.theme, posts)?;
        let (whole_site, affected) = self.plan(&rows, &index)?;
        let shell = self.theme.shell(&site);
        let page = Page {
            shell_key: blob_key("fragments/shell/", &shell),
            shell: &shell,
            modified: imf_fixdate(now).into(),
        };
        let writes = self.render(&index, &affected, &page, rows.len())?;

        // S3 first, so no route item ever names a missing object.
        self.blobs
            .put(&page.shell_key, &shell.bytes, shell.content_type)?;
        for (key, rendered) in &writes.blobs {
            self.blobs
                .put(&BlobKey::from(&**key), &rendered.bytes, rendered.content_type)?;
        }
        // Then the DynamoDB pointers, each conditional on the version.
        let mut report = Report {
            rows: rows.len(),
            age_seconds: u64::try_from((now - oldest) / 1_000_000).unwrap_or_default(),
            blobs: writes.blobs.len() + 1,
            unresolved: writes.unresolved,
            ..Report::default()
        };
        for item in &writes.puts {
            report.count(self.routes.put(item, &version)?);
        }
        for key in &writes.deletes {
            report.count(self.routes.delete(key, &version)?);
        }
        // Then one invalidation of every affected path, even for stale writes: a replay after a
        // crash between the table and the edge must still reach the edge.
        report.invalidated = edge_paths(paths, whole_site, &writes.routes);
        if !report.invalidated.is_empty() {
            self.invalidator.invalidate(&report.invalidated)?;
        }
        let ids: Vec<Box<str>> = rows.iter().map(|row| row.id.clone()).collect();
        cms_outbox::delete(self.store, self.prefix, &ids)?;
        // A full batch may have left rows behind; a fresh invocation drains them.
        if rows.len() >= usize::try_from(BATCH).unwrap_or(usize::MAX) {
            self.nudger.nudge()?;
            report.nudged = true;
        }
        Ok(report)
    }

    // Post and comment events touch one post; anything else changes what every page shows.
    fn plan(&mut self, rows: &[Row], index: &Index<'_>) -> Result<(bool, Affected), ProjectError> {
        let mut whole_site = false;
        let mut posts = BTreeSet::new();
        for row in rows {
            let event = &row.event;
            let post = match event.aggregate() {
                Aggregate::Post => Some(event.aggregate_id()),
                // A comment changes its post's page; the writer names the post, because a
                // deleted comment can no longer be looked up.
                Aggregate::Comment => event
                    .payload()
                    .get("post_id")
                    .and_then(serde_json::Value::as_i64),
                Aggregate::Term
                | Aggregate::Option
                | Aggregate::User
                | Aggregate::Theme
                | Aggregate::Site => None,
            };
            // An event naming no usable post falls back to a full rebuild, which is always safe.
            match post.and_then(|id| u64::try_from(id).ok()) {
                Some(id) => {
                    posts.insert(id);
                }
                None => whole_site = true,
            }
        }
        let mut affected = Affected::default();
        if whole_site {
            affected.targets.extend(plan::site_targets());
            posts.extend(index.ids());
        }
        for id in posts {
            let previous = self.routes.post_doc(id)?;
            affected.merge(plan::affected(previous.as_ref(), index.facts(id)));
        }
        Ok((whole_site, affected))
    }

    fn render(
        &mut self,
        index: &Index<'_>,
        affected: &Affected,
        page: &Page<'_>,
        events: usize,
    ) -> Result<Writes, ProjectError> {
        let mut writes = Writes::default();
        // Each event removes at most one post from a listing, so at most this many trailing
        // pages can have disappeared since the last run.
        let shrink = u32::try_from(events).unwrap_or(u32::MAX);
        for target in &affected.targets {
            if target.page().is_none() {
                let rendering = index.render(target, self.cards)?;
                place(&mut writes, self.paths, target, rendering, page)?;
                continue;
            }
            let pages = index.pages(target);
            for number in 1..=pages {
                let numbered = target.at(number);
                let rendering = index.render(&numbered, self.cards)?;
                place(&mut writes, self.paths, &numbered, rendering, page)?;
            }
            for number in (pages + 1)..=pages.saturating_add(shrink) {
                drop_route(&mut writes, self.paths.key(&target.at(number)));
            }
        }
        let rendered = writes.routes.clone();
        // Redirects and removals never override a route rendered in this run (a reused slug).
        for (from, to) in &affected.redirects {
            if !rendered.contains(from) {
                let route = Route {
                    status: 301,
                    content_type: cms_render::HTML.into(),
                    etag: Box::default(),
                    last_modified: page.modified.clone(),
                    content: Content::Redirect {
                        location: to.clone(),
                    },
                };
                writes.routes.insert(from.clone());
                writes.puts.push(Item::Route(RouteKey::from(&**from), route));
            }
        }
        for path in &affected.removed {
            if !rendered.contains(path) {
                drop_route(&mut writes, RouteKey::from(&**path));
            }
        }
        docs(&mut writes, index, affected);
        Ok(writes)
    }
}

// Turns one rendering into blobs and route items.
fn place(
    writes: &mut Writes,
    paths: &Paths,
    target: &Target,
    rendering: Rendering,
    page: &Page<'_>,
) -> Result<(), ProjectError> {
    let key = paths.key(target);
    match rendering {
        Rendering::Missing => drop_route(writes, key),
        Rendering::Page {
            fragment,
            unresolved,
        } => {
            writes.unresolved += unresolved;
            let body = blob_key("rendered/", &fragment);
            // The ETag covers exactly the bytes the read path will serve.
            let served = cms_readmodel::assemble(&page.shell.bytes, &fragment.bytes)?;
            let route = Route {
                status: if *target == Target::NotFound { 404 } else { 200 },
                content_type: fragment.content_type.into(),
                etag: quoted(&hex(&served)),
                last_modified: page.modified.clone(),
                content: Content::Page {
                    shell: page.shell_key.clone(),
                    body: body.clone(),
                },
            };
            if let Target::Post { id, .. } = target {
                writes.bodies.insert(*id, body.clone());
            }
            writes.blobs.insert(body.as_str().into(), fragment);
            put_route(writes, key, route);
        }
        Rendering::Object { body, unresolved } => {
            writes.unresolved += unresolved;
            let folder = if *target == Target::Search {
                "search/"
            } else {
                "rendered/"
            };
            object(writes, key, folder, body, page);
        }
        Rendering::Files(files) => {
            for family in SITEMAP_FAMILIES {
                let prefix = paths.file(&format!("wp-sitemap-{family}-"));
                let present = files
                    .iter()
                    .filter(|(key, _)| key.as_str().starts_with(prefix.as_str()))
                    .count();
                // A shard past the last one existed only if the provider just shrank.
                drop_route(writes, paths.file(&format!("wp-sitemap-{family}-{}.xml", present + 1)));
            }
            for (key, rendered) in files {
                object(writes, key, "rendered/", rendered, page);
            }
        }
    }
    Ok(())
}

fn object(writes: &mut Writes, key: RouteKey, folder: &str, rendered: Rendered, page: &Page<'_>) {
    let body = blob_key(folder, &rendered);
    let route = Route {
        status: 200,
        content_type: rendered.content_type.into(),
        etag: quoted(&hex(&rendered.bytes)),
        last_modified: page.modified.clone(),
        content: Content::Object { body: body.clone() },
    };
    writes.blobs.insert(body.as_str().into(), rendered);
    put_route(writes, key, route);
}

// A key written twice in one run keeps only its last item (a shard key is also a target key).
fn put_route(writes: &mut Writes, key: RouteKey, route: Route) {
    writes.routes.insert(key.as_str().into());
    writes
        .deletes
        .retain(|old| !matches!(old, ItemKey::Route(old) if *old == key));
    writes.puts.push(Item::Route(key, route));
}

fn drop_route(writes: &mut Writes, key: RouteKey) {
    let written = writes
        .puts
        .iter()
        .any(|item| matches!(item, Item::Route(old, _) if *old == key));
    if !written {
        writes.routes.insert(key.as_str().into());
        writes.deletes.push(ItemKey::Route(key));
    }
}

// Docs follow their rendered body; a post that vanished mid-run loses its doc instead.
fn docs(writes: &mut Writes, index: &Index<'_>, affected: &Affected) {
    for id in &affected.docs {
        let doc = writes
            .bodies
            .get(id)
            .zip(index.post(*id))
            .zip(index.facts(*id))
            .map(|((body, post), facts)| PostDoc {
                facts: facts.clone(),
                title: post.title.clone(),
                excerpt: cms_render::excerpt(post).into(),
                author_name: post.author.name.clone(),
                body: body.clone(),
            });
        match doc {
            Some(doc) => writes.puts.push(Item::Doc(doc)),
            None => writes.deletes.push(ItemKey::Doc(*id)),
        }
    }
    writes
        .deletes
        .extend(affected.dropped.iter().map(|id| ItemKey::Doc(*id)));
    writes
        .puts
        .extend(affected.listed.iter().cloned().map(Item::Listing));
    writes
        .deletes
        .extend(affected.unlisted.iter().cloned().map(ItemKey::Listing));
}

fn edge_paths(paths: &Paths, whole_site: bool, routes: &BTreeSet<Box<str>>) -> Vec<Box<str>> {
    let public: Vec<Box<str>> = routes
        .iter()
        .filter(|path| path.starts_with('/'))
        .cloned()
        .collect();
    if whole_site || public.len() > MAX_PATHS {
        return vec![paths.wildcard()];
    }
    public
}

// `<folder><sha256>.<ext>`: content-addressed, so a rewrite never mutates a served object.
fn blob_key(folder: &str, rendered: &Rendered) -> BlobKey {
    let kind = rendered.content_type;
    let ext = ["html", "xml", "json"]
        .into_iter()
        .find(|ext| kind.contains(ext))
        .unwrap_or("txt");
    BlobKey::from(format!("{folder}{}.{ext}", hex(&rendered.bytes)).as_str())
}

fn hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest.iter() {
        // Writing into a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn quoted(text: &str) -> Box<str> {
    format!("\"{text}\"").into()
}

#[cfg(test)]
mod tests {
    use cms_render::Rendered;

    use super::{blob_key, hex};

    #[test]
    fn blob_keys_hash_the_bytes_and_follow_the_type() {
        let rendered = |kind: &'static str| Rendered {
            bytes: b"abc".to_vec(),
            content_type: kind,
        };
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(hex(b"abc"), digest);
        assert_eq!(
            blob_key("rendered/", &rendered("text/html; charset=UTF-8")).as_str(),
            format!("rendered/{digest}.html")
        );
        assert!(blob_key("r/", &rendered("application/rss+xml")).as_str().ends_with(".xml"));
        assert!(blob_key("s/", &rendered("application/json")).as_str().ends_with(".json"));
        assert!(blob_key("r/", &rendered("text/plain")).as_str().ends_with(".txt"));
    }
}
