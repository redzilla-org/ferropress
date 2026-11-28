//! Rows and values the editor workflows read and write.

use cms_content::PostId;

/// The zero date the reference stores for a draft's unset GMT date.
pub const ZERO_DATE: &str = "0000-00-00 00:00:00";

/// A post status the native editor can set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// `draft`.
    Draft,
    /// `pending` (submitted for review).
    Pending,
    /// `publish`.
    Publish,
    /// `trash`.
    Trash,
}

impl Status {
    /// The stored `post_status` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Pending => "pending",
            Self::Publish => "publish",
            Self::Trash => "trash",
        }
    }

    /// Parses a stored or submitted status; other reference statuses are not editable here.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "draft" => Some(Self::Draft),
            "pending" => Some(Self::Pending),
            "publish" => Some(Self::Publish),
            "trash" => Some(Self::Trash),
            _ => None,
        }
    }
}

/// Which posts the list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusFilter {
    /// Every status except `trash` and `auto-draft`, as the reference's "All" view.
    All,
    /// Exactly one status.
    Only(Status),
}

/// The columns of one posts row the admin reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostRow {
    /// `ID`.
    pub id: PostId,
    /// `post_author`.
    pub author: u64,
    /// `post_date` (site local time).
    pub date: String,
    /// `post_date_gmt`.
    pub date_gmt: String,
    /// `post_content`.
    pub content: String,
    /// `post_title`.
    pub title: String,
    /// `post_excerpt`.
    pub excerpt: String,
    /// `post_status`, as stored.
    pub status: String,
    /// `post_name`.
    pub name: String,
    /// `post_type`.
    pub type_name: String,
    /// `post_modified`.
    pub modified: String,
    /// `post_modified_gmt`.
    pub modified_gmt: String,
}

impl PostRow {
    /// Whether the stored status is `publish`.
    #[must_use]
    pub fn is_published(&self) -> bool {
        self.status == Status::Publish.as_str()
    }
}

/// The editable columns written on insert or update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostWrite {
    /// `post_author` (insert only).
    pub author: u64,
    /// `post_title`.
    pub title: String,
    /// `post_content`.
    pub content: String,
    /// `post_excerpt`.
    pub excerpt: String,
    /// `post_status`.
    pub status: Status,
    /// `post_name`.
    pub name: String,
    /// `post_date`.
    pub date: String,
    /// `post_date_gmt`.
    pub date_gmt: String,
    /// `post_modified`.
    pub modified: String,
    /// `post_modified_gmt`.
    pub modified_gmt: String,
}

/// A term joined to its taxonomy row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermRow {
    /// `term_id`.
    pub term_id: u64,
    /// `term_taxonomy_id`.
    pub taxonomy_id: u64,
    /// `taxonomy` (`category` or `post_tag`).
    pub taxonomy: String,
    /// `name`.
    pub name: String,
    /// `slug`.
    pub slug: String,
}

/// What a committed write did to a post, as the projector reads it from the outbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// A new post that is not public.
    Created,
    /// A post changed without entering or leaving `publish`.
    Updated,
    /// A post became public.
    Published,
    /// A public post left `publish` for a non-trash status.
    Unpublished,
    /// A post moved to the trash.
    Trashed,
}

impl Change {
    /// The `change` column value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Published => "published",
            Self::Unpublished => "unpublished",
            Self::Trashed => "trashed",
        }
    }

    /// The change for a write from `before` (none for an insert) to `after`.
    #[must_use]
    pub fn between(before: Option<&str>, after: Status) -> Self {
        let was_public = before == Some(Status::Publish.as_str());
        match (before, after) {
            (_, Status::Trash) => Self::Trashed,
            (_, Status::Publish) if !was_public => Self::Published,
            (_, Status::Publish) => Self::Updated,
            (None, _) => Self::Created,
            (Some(_), _) if was_public => Self::Unpublished,
            (Some(_), _) => Self::Updated,
        }
    }
}

/// One `fp_outbox` row for a post write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxEvent {
    /// `aggregate_id`: the post id.
    pub post: PostId,
    /// What the editor did, carried in the payload as `action`.
    pub change: Change,
    /// The payload object.
    pub payload: serde_json::Map<String, serde_json::Value>,
}

impl OutboxEvent {
    /// The event for a committed post write. The payload names the action and the slugs the
    /// projector must re-render or redirect: the current one and the previous one.
    #[must_use]
    pub fn post(post: PostId, change: Change, after: &PostWrite, before: Option<&PostRow>) -> Self {
        use serde_json::json;
        let mut payload = serde_json::Map::new();
        for (key, value) in [
            ("id", json!(post.as_u64())),
            ("type", json!("post")),
            ("action", json!(change.as_str())),
            ("status", json!(after.status.as_str())),
            ("slug", json!(after.name)),
            ("old_status", json!(before.map(|row| row.status.as_str()))),
            ("old_slug", json!(before.map(|row| row.name.as_str()))),
        ] {
            payload.insert(key.to_owned(), value);
        }
        Self {
            post,
            change,
            payload,
        }
    }

    /// The shared outbox event the projector drains (`cms-outbox`). Every post write is a
    /// `saved` change on the `post` aggregate: the projector re-renders from current DSQL state
    /// and reads old permalinks from its previous post doc, so the editor's finer action rides
    /// in the payload only.
    #[must_use]
    pub fn to_event(&self) -> cms_outbox::Event {
        let id = i64::try_from(self.post.as_u64()).unwrap_or(i64::MAX);
        cms_outbox::Event::new(cms_outbox::Aggregate::Post, id, cms_outbox::Change::Saved)
            .with_payload(self.payload.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{Change, OutboxEvent, PostRow, PostWrite, Status, ZERO_DATE};
    use cms_content::PostId;

    fn row(status: &str) -> Option<PostRow> {
        Some(PostRow {
            id: PostId::new(5).ok()?,
            author: 2,
            date: ZERO_DATE.into(),
            date_gmt: ZERO_DATE.into(),
            content: String::new(),
            title: "T".into(),
            excerpt: String::new(),
            status: status.into(),
            name: "old".into(),
            type_name: "post".into(),
            modified: ZERO_DATE.into(),
            modified_gmt: ZERO_DATE.into(),
        })
    }

    #[test]
    fn statuses_round_trip_and_reject_others() {
        for status in [
            Status::Draft,
            Status::Pending,
            Status::Publish,
            Status::Trash,
        ] {
            assert_eq!(Status::parse(status.as_str()), Some(status));
        }
        assert_eq!(Status::parse("future"), None);
        assert!(row("publish").is_some_and(|row| row.is_published()));
        assert!(row("draft").is_some_and(|row| !row.is_published()));
    }

    #[test]
    fn changes_follow_the_status_transition() {
        assert_eq!(Change::between(None, Status::Draft), Change::Created);
        assert_eq!(Change::between(None, Status::Publish), Change::Published);
        assert_eq!(
            Change::between(Some("draft"), Status::Publish),
            Change::Published
        );
        assert_eq!(
            Change::between(Some("publish"), Status::Publish),
            Change::Updated
        );
        assert_eq!(
            Change::between(Some("publish"), Status::Draft),
            Change::Unpublished
        );
        assert_eq!(
            Change::between(Some("draft"), Status::Pending),
            Change::Updated
        );
        assert_eq!(
            Change::between(Some("publish"), Status::Trash),
            Change::Trashed
        );
        let names: Vec<&str> = [
            Change::Created,
            Change::Updated,
            Change::Published,
            Change::Unpublished,
            Change::Trashed,
        ]
        .iter()
        .map(|change| change.as_str())
        .collect();
        assert_eq!(
            names,
            ["created", "updated", "published", "unpublished", "trashed"]
        );
    }

    #[test]
    fn outbox_payload_names_both_slugs() {
        let id = PostId::new(5);
        let before = row("draft");
        assert!(id.is_ok() && before.is_some());
        if let (Ok(id), Some(before)) = (id, before) {
            let after = PostWrite {
                author: 2,
                title: "T".into(),
                content: String::new(),
                excerpt: String::new(),
                status: Status::Publish,
                name: "new".into(),
                date: ZERO_DATE.into(),
                date_gmt: ZERO_DATE.into(),
                modified: ZERO_DATE.into(),
                modified_gmt: ZERO_DATE.into(),
            };
            let event = OutboxEvent::post(id, Change::Published, &after, Some(&before));
            assert_eq!(event.change, Change::Published);
            assert_eq!(
                serde_json::Value::Object(event.payload.clone()).to_string(),
                r#"{"action":"published","id":5,"old_slug":"old","old_status":"draft","slug":"new","status":"publish","type":"post"}"#
            );
            let shared = event.to_event();
            assert_eq!(shared.aggregate(), cms_outbox::Aggregate::Post);
            assert_eq!(shared.aggregate_id(), 5);
            assert_eq!(shared.change(), cms_outbox::Change::Saved);
            let fresh = OutboxEvent::post(id, Change::Created, &after, None);
            assert_eq!(
                fresh.payload.get("old_slug"),
                Some(&serde_json::Value::Null)
            );
        }
    }
}
