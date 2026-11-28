//! The editor workflows: list, load, save (draft, pending, publish), trash.

use std::collections::BTreeSet;
use std::fmt;

use cms_content::PostId;
use cms_users::UserId;

use crate::date::format;
use crate::model::{
    Change, OutboxEvent, PostRow, PostWrite, Status, StatusFilter, TermRow, ZERO_DATE,
};
use crate::slug::{sanitize, unique};
use crate::{Clock, Error, Permissions, Projector, Repo};

/// Posts per admin list page, the reference's default screen option.
pub const PER_PAGE: u64 = 20;

/// Site settings the workflows read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// The theme's site model: home URL and base path (both with a trailing slash), title.
    pub theme: cms_render::Site,
    /// Offset of site local time from UTC, in seconds (the `gmt_offset` option).
    pub gmt_offset: i64,
}

impl Site {
    /// Site local time for `unix` seconds, as a date column value.
    #[must_use]
    pub fn local(&self, unix: i64) -> String {
        format(unix + self.gmt_offset)
    }
}

/// Tags as the REST API sends them (term ids) or as the native editor sends them (names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tags {
    /// Existing `post_tag` term ids.
    Ids(Vec<u64>),
    /// Tag names; missing tags are created.
    Names(Vec<String>),
}

/// Submitted fields. `None` keeps the stored value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PostInput {
    /// `post_title`.
    pub title: Option<String>,
    /// `post_content` (HTML).
    pub content: Option<String>,
    /// `post_excerpt`.
    pub excerpt: Option<String>,
    /// Requested slug; sanitized before use.
    pub slug: Option<String>,
    /// Target status.
    pub status: Option<Status>,
    /// Category term ids.
    pub categories: Option<Vec<u64>>,
    /// Tags.
    pub tags: Option<Tags>,
}

/// A committed write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The post id.
    pub id: PostId,
    /// What the outbox row says happened.
    pub change: Change,
    /// The columns as written.
    pub row: PostWrite,
}

/// One list page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// Up to [`PER_PAGE`] posts.
    pub posts: Vec<PostRow>,
    /// Whether a later page exists.
    pub has_next: bool,
}

/// The workflows over one request's ports.
pub struct Editor<'a> {
    repo: &'a mut dyn Repo,
    permissions: &'a mut dyn Permissions,
    projector: &'a mut dyn Projector,
    clock: &'a dyn Clock,
    site: &'a Site,
}

impl<'a> Editor<'a> {
    /// Workflows for one request.
    pub fn new(
        repo: &'a mut dyn Repo,
        permissions: &'a mut dyn Permissions,
        projector: &'a mut dyn Projector,
        clock: &'a dyn Clock,
        site: &'a Site,
    ) -> Self {
        Self {
            repo,
            permissions,
            projector,
            clock,
            site,
        }
    }

    /// The site settings.
    #[must_use]
    pub const fn site(&self) -> &Site {
        self.site
    }

    /// Whether `user` holds `cap`.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when capabilities cannot be read.
    pub fn can(&mut self, user: UserId, cap: &str) -> Result<bool, Error> {
        self.permissions.has(user, &[cap.as_bytes()])
    }

    /// Page `page` (from 1) of posts matching `filter`. Requires `edit_posts`.
    ///
    /// # Errors
    ///
    /// [`Error::Forbidden`] or a repository failure.
    pub fn list(&mut self, user: UserId, filter: StatusFilter, page: u64) -> Result<Page, Error> {
        self.require(user, &["edit_posts"])?;
        let offset = page.saturating_sub(1).saturating_mul(PER_PAGE);
        let mut posts = self.repo.list(filter, offset, PER_PAGE + 1)?;
        let has_next = posts.len() > usize::try_from(PER_PAGE).unwrap_or(usize::MAX);
        posts.truncate(usize::try_from(PER_PAGE).unwrap_or(usize::MAX));
        Ok(Page { posts, has_next })
    }

    /// One post and its terms, for editing or preview. Requires the post's edit capabilities.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`], [`Error::Forbidden`] or a repository failure.
    pub fn load(&mut self, user: UserId, id: PostId) -> Result<(PostRow, Vec<TermRow>), Error> {
        let row = self.existing(id)?;
        self.require(user, &edit_caps(user, &row))?;
        let terms = self.repo.post_terms(id)?;
        Ok((row, terms))
    }

    /// The post rendered by the built-in theme from the stored row, for `/admin/preview/<id>`.
    /// Requires the post's edit capabilities.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`], [`Error::Forbidden`], a repository or render failure.
    pub fn preview(
        &mut self,
        user: UserId,
        id: PostId,
        cards: &mut dyn cms_render::CardSource,
    ) -> Result<Vec<u8>, Error> {
        let (row, terms) = self.load(user, id)?;
        let author = self.repo.author(row.author)?;
        crate::preview::render(self.site, &row, &terms, author, self.clock.now(), cards)
    }

    /// Every category, for the editor's checkboxes.
    ///
    /// # Errors
    ///
    /// A repository failure.
    pub fn categories(&mut self) -> Result<Vec<TermRow>, Error> {
        self.repo.terms("category")
    }

    /// Creates (`id` is `None`) or updates a post in one transaction with its outbox row, then
    /// nudges the projector. A `trash` status is routed to [`Self::trash`].
    ///
    /// # Errors
    ///
    /// [`Error::Forbidden`] before anything is written; any failure inside the transaction rolls
    /// it back.
    pub fn save(
        &mut self,
        user: UserId,
        id: Option<PostId>,
        input: &PostInput,
    ) -> Result<Saved, Error> {
        let before = id.map(|id| self.existing(id)).transpose()?;
        let status = input
            .status
            .or_else(|| before.as_ref().and_then(|row| Status::parse(&row.status)))
            .unwrap_or(Status::Draft);
        if let (Status::Trash, Some(id)) = (status, id) {
            return self.trash(user, id);
        }
        if status == Status::Trash {
            return Err(Error::Invalid("a new post cannot be trashed".into()));
        }
        match &before {
            Some(row) => self.require(user, &edit_caps(user, row))?,
            None => self.require(user, &["edit_posts"])?,
        }
        if status == Status::Publish && !before.as_ref().is_some_and(PostRow::is_published) {
            self.require(user, &["publish_posts"])?;
        }
        let row = self.compose(user, before.as_ref(), input, status);
        self.transaction(|editor| editor.write(before.as_ref(), row, input))
    }

    /// Moves a post to the trash, recording its previous status as the reference does.
    ///
    /// # Errors
    ///
    /// [`Error::Forbidden`] before anything is written; any failure inside the transaction rolls
    /// it back.
    pub fn trash(&mut self, user: UserId, id: PostId) -> Result<Saved, Error> {
        let before = self.existing(id)?;
        if before.status == Status::Trash.as_str() {
            return Err(Error::Invalid("the post is already in the trash".into()));
        }
        self.require(user, &delete_caps(user, &before))?;
        let now = self.clock.now();
        let row = PostWrite {
            author: before.author,
            title: before.title.clone(),
            content: before.content.clone(),
            excerpt: before.excerpt.clone(),
            status: Status::Trash,
            name: before.name.clone(),
            date: before.date.clone(),
            date_gmt: before.date_gmt.clone(),
            modified: self.site.local(now),
            modified_gmt: format(now),
        };
        self.transaction(|editor| {
            editor
                .repo
                .add_meta(id, "_wp_trash_meta_status", &before.status)?;
            editor
                .repo
                .add_meta(id, "_wp_trash_meta_time", &now.to_string())?;
            editor.repo.update_post(id, &row)?;
            let linked: Vec<u64> = editor
                .repo
                .post_terms(id)?
                .iter()
                .map(|term| term.taxonomy_id)
                .collect();
            for taxonomy_id in linked {
                editor.repo.recount(taxonomy_id)?;
            }
            let event = OutboxEvent::post(id, Change::Trashed, &row, Some(&before));
            editor.repo.outbox(&event)?;
            Ok(Saved {
                id,
                change: Change::Trashed,
                row,
            })
        })
    }

    fn existing(&mut self, id: PostId) -> Result<PostRow, Error> {
        self.repo
            .post(id)?
            .filter(|row| row.type_name == "post")
            .ok_or(Error::NotFound("post"))
    }

    fn require(&mut self, user: UserId, caps: &[&'static str]) -> Result<(), Error> {
        for cap in caps {
            if !self.can(user, cap)? {
                return Err(Error::Forbidden(cap));
            }
        }
        Ok(())
    }

    // Column values before uniqueness: dates follow the reference (a draft keeps a zero GMT
    // date; publishing a never-published post stamps both dates with now).
    fn compose(
        &self,
        user: UserId,
        before: Option<&PostRow>,
        input: &PostInput,
        status: Status,
    ) -> PostWrite {
        let now = self.clock.now();
        let local = self.site.local(now);
        let gmt = format(now);
        let keep = |given: &Option<String>, stored: Option<&String>| {
            given
                .clone()
                .or_else(|| stored.cloned())
                .unwrap_or_default()
        };
        let title = keep(&input.title, before.map(|row| &row.title));
        let (date, date_gmt) = match before {
            Some(row) if row.date_gmt != ZERO_DATE || status != Status::Publish => {
                (row.date.clone(), row.date_gmt.clone())
            }
            _ if status == Status::Publish => (local.clone(), gmt.clone()),
            _ => (local.clone(), ZERO_DATE.to_owned()),
        };
        let mut name = input
            .slug
            .as_deref()
            .map(sanitize)
            .or_else(|| before.map(|row| row.name.clone()))
            .unwrap_or_default();
        if name.is_empty() && !matches!(status, Status::Draft | Status::Pending) {
            name = sanitize(&title);
        }
        PostWrite {
            author: before.map_or(user.as_u64(), |row| row.author),
            content: keep(&input.content, before.map(|row| &row.content)),
            excerpt: keep(&input.excerpt, before.map(|row| &row.excerpt)),
            title,
            status,
            name,
            date,
            date_gmt,
            modified: local,
            modified_gmt: gmt,
        }
    }

    fn write(
        &mut self,
        before: Option<&PostRow>,
        mut row: PostWrite,
        input: &PostInput,
    ) -> Result<Saved, Error> {
        let known = before.map(|row| row.id);
        row.name = unique(&mut *self.repo, &row.name, row.status, known)?;
        let id = if let Some(id) = known {
            self.repo.update_post(id, &row)?;
            id
        } else {
            let id = self.repo.insert_post(&row)?;
            let guid = self.site.theme.url(&format!("?p={}", id.as_u64()));
            self.repo.set_guid(id, &guid)?;
            // An untitled published post takes its id as slug, as the reference's fallback.
            if row.name.is_empty() && row.status == Status::Publish {
                row.name = unique(
                    &mut *self.repo,
                    &id.as_u64().to_string(),
                    row.status,
                    Some(id),
                )?;
                self.repo.update_post(id, &row)?;
            }
            id
        };
        self.assign(id, before.is_some(), input)?;
        if let Some(before) = before {
            self.record_old_slug(id, before, &row)?;
        }
        let change = Change::between(before.map(|row| row.status.as_str()), row.status);
        self.repo
            .outbox(&OutboxEvent::post(id, change, &row, before))?;
        Ok(Saved { id, change, row })
    }

    // The reference keeps a published post's previous slugs as `_wp_old_slug` meta so old links
    // redirect, and drops an entry once the post takes that slug back.
    fn record_old_slug(
        &mut self,
        id: PostId,
        before: &PostRow,
        row: &PostWrite,
    ) -> Result<(), Error> {
        // Only a slug that was public can have links pointing at it.
        if before.name == row.name || row.status != Status::Publish || !before.is_published() {
            return Ok(());
        }
        let old = self.repo.meta(id, "_wp_old_slug")?;
        if !before.name.is_empty() && !old.contains(&before.name) {
            self.repo.add_meta(id, "_wp_old_slug", &before.name)?;
        }
        if old.contains(&row.name) {
            self.repo.delete_meta(id, "_wp_old_slug", Some(&row.name))?;
        }
        Ok(())
    }

    // Replaces the post's categories and tags, defaulting an empty category set to the
    // `default_category` option, then recounts every term touched.
    fn assign(&mut self, id: PostId, existing: bool, input: &PostInput) -> Result<(), Error> {
        let current = if existing {
            self.repo.post_terms(id)?
        } else {
            Vec::new()
        };
        let of = |taxonomy: &str| -> Vec<TermRow> {
            current
                .iter()
                .filter(|term| term.taxonomy == taxonomy)
                .cloned()
                .collect()
        };
        let mut categories = match &input.categories {
            Some(ids) => self.known("category", ids)?,
            None => of("category"),
        };
        if categories.is_empty() {
            let fallback = self
                .repo
                .option("default_category")?
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|id| *id > 0);
            if let Some(fallback) = fallback {
                categories = self.repo.find_terms("category", &[fallback])?;
            }
        }
        let tags = match &input.tags {
            Some(Tags::Ids(ids)) => self.known("post_tag", ids)?,
            Some(Tags::Names(names)) => self.tags_named(names)?,
            None => of("post_tag"),
        };
        let before: BTreeSet<u64> = current.iter().map(|term| term.taxonomy_id).collect();
        let after: BTreeSet<u64> = categories
            .iter()
            .chain(tags.iter())
            .map(|term| term.taxonomy_id)
            .collect();
        let removed: Vec<u64> = before.difference(&after).copied().collect();
        self.repo.unlink(id, &removed)?;
        for added in after.difference(&before) {
            self.repo.link(id, *added)?;
        }
        for touched in before.union(&after) {
            self.repo.recount(*touched)?;
        }
        Ok(())
    }

    fn known(&mut self, taxonomy: &str, ids: &[u64]) -> Result<Vec<TermRow>, Error> {
        let wanted: BTreeSet<u64> = ids.iter().copied().collect();
        let wanted: Vec<u64> = wanted.into_iter().collect();
        let found = self.repo.find_terms(taxonomy, &wanted)?;
        if found.len() == wanted.len() {
            Ok(found)
        } else {
            Err(Error::Invalid(format!("unknown {taxonomy} id").into()))
        }
    }

    fn tags_named(&mut self, names: &[String]) -> Result<Vec<TermRow>, Error> {
        let mut tags: Vec<TermRow> = Vec::new();
        for name in names.iter().map(|name| name.trim()) {
            let slug = sanitize(name);
            if slug.is_empty() || tags.iter().any(|tag| tag.slug == slug) {
                continue;
            }
            let tag = match self.repo.term_by_slug("post_tag", &slug)? {
                Some(tag) => tag,
                None => self.repo.insert_term("post_tag", name, &slug)?,
            };
            tags.push(tag);
        }
        Ok(tags)
    }

    // One transaction around `work`: commit on success, roll back on any failure, and nudge
    // the projector only after a commit.
    fn transaction(
        &mut self,
        work: impl FnOnce(&mut Self) -> Result<Saved, Error>,
    ) -> Result<Saved, Error> {
        self.repo.begin()?;
        let saved = match work(self) {
            Ok(saved) => saved,
            Err(error) => {
                // The original failure is what the caller needs; a failed rollback ends with the
                // connection, which the server then aborts.
                let _rolled_back = self.repo.rollback();
                return Err(error);
            }
        };
        self.repo.commit()?;
        // A lost nudge is recovered by the projector's schedule, so it does not fail the save.
        let _nudged = self.projector.nudge();
        Ok(saved)
    }
}

impl fmt::Debug for Editor<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Editor")
            .field("site", &self.site)
            .finish_non_exhaustive()
    }
}

/// Primitive capabilities the reference requires to edit `row` (its `edit_post` mapping).
#[must_use]
pub fn edit_caps(user: UserId, row: &PostRow) -> Vec<&'static str> {
    let mut caps = vec![if row.author == user.as_u64() {
        "edit_posts"
    } else {
        "edit_others_posts"
    }];
    if row.is_published() {
        caps.push("edit_published_posts");
    }
    caps
}

/// Primitive capabilities the reference requires to delete `row` (its `delete_post` mapping).
#[must_use]
pub fn delete_caps(user: UserId, row: &PostRow) -> Vec<&'static str> {
    let mut caps = vec![if row.author == user.as_u64() {
        "delete_posts"
    } else {
        "delete_others_posts"
    }];
    if row.is_published() {
        caps.push("delete_published_posts");
    }
    caps
}
