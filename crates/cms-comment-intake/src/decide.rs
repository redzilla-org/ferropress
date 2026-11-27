//! The acceptance decision: refuse, or a stored comment with its approval status.

use crate::checks::{link_count, matches_keys};
use crate::email::is_email;
use crate::error::Error;
use crate::markup::{filter_content, sanitize_author, sanitize_ip, sanitize_url, strip_tags};
use crate::settings::{FLOOD_SECONDS, Settings};
use crate::time;

/// What the commenter's browser posted, before any sanitizing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Submission {
    /// `comment_post_ID`.
    pub post_id: u64,
    /// `comment_parent`; `0` for a top-level comment.
    pub parent: u64,
    /// `author`.
    pub author: Box<str>,
    /// `email`.
    pub email: Box<str>,
    /// `url`.
    pub url: Box<str>,
    /// `comment`.
    pub content: Box<str>,
    /// The client address.
    pub ip: Box<str>,
    /// The `User-Agent` header.
    pub agent: Box<str>,
}

/// The logged-in user posting, resolved from the session by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commenter {
    /// User id.
    pub id: u64,
    /// `display_name`, or `user_login` when that is empty.
    pub name: Box<str>,
    /// `user_email`.
    pub email: Box<str>,
    /// `user_url`.
    pub url: Box<str>,
    /// Holds `moderate_comments`.
    pub can_moderate: bool,
    /// Holds `manage_options`.
    pub can_manage_options: bool,
}

/// The post row fields the rules read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Post {
    /// `ID`.
    pub id: u64,
    /// `post_author`.
    pub author: u64,
    /// `post_status`.
    pub status: Box<str>,
    /// `comment_status`.
    pub comment_status: Box<str>,
    /// `post_type`.
    pub post_type: Box<str>,
    /// Whether `post_password` is non-empty.
    pub has_password: bool,
    /// `post_date_gmt`.
    pub date_gmt: Box<str>,
}

/// Lookups the rules need from stored comments, posts and users.
pub trait History {
    /// The post, or `None` when there is no such row.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn post(&mut self, id: u64) -> Result<Option<Post>, Error>;

    /// `comment_approved` of comment `id`, or `None` when there is no such row.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn comment_status(&mut self, id: u64) -> Result<Option<Box<str>>, Error>;

    /// Whether a non-trashed comment on the same post and parent, by the same author name or
    /// email, already has this exact content.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn duplicate(&mut self, candidate: &Candidate) -> Result<bool, Error>;

    /// The newest `comment_date_gmt` at or after `since` from this IP or, when non-empty, this
    /// email (anonymous) or this user id (logged in).
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn last_comment_gmt(
        &mut self,
        candidate: &Candidate,
        since: &str,
    ) -> Result<Option<Box<str>>, Error>;

    /// Whether this author already has an approved comment: by the user owning `email` when
    /// there is one, otherwise by the same author name and email.
    ///
    /// # Errors
    ///
    /// [`Error`] when the store fails.
    fn previously_approved(&mut self, author: &str, email: &str) -> Result<bool, Error>;
}

/// The stored status of an accepted comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    /// `1`: published.
    Approved,
    /// `0`: held for moderation.
    Hold,
    /// `spam`.
    Spam,
    /// `trash`.
    Trash,
}

impl Approval {
    /// The `comment_approved` cell.
    #[must_use]
    pub const fn cell(self) -> &'static str {
        match self {
            Self::Approved => "1",
            Self::Hold => "0",
            Self::Spam => "spam",
            Self::Trash => "trash",
        }
    }
}

/// A comment that passed every refusal check, sanitized and ready to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// `comment_post_ID`.
    pub post_id: u64,
    /// `comment_parent`.
    pub parent: u64,
    /// `comment_author`.
    pub author: Box<str>,
    /// `comment_author_email`.
    pub email: Box<str>,
    /// `comment_author_url`.
    pub url: Box<str>,
    /// `comment_content`.
    pub content: Box<str>,
    /// `comment_author_IP`.
    pub ip: Box<str>,
    /// `comment_agent`.
    pub agent: Box<str>,
    /// `user_id`; `0` when anonymous.
    pub user_id: u64,
    /// `comment_date` (site-local).
    pub date: Box<str>,
    /// `comment_date_gmt`.
    pub date_gmt: Box<str>,
    /// `comment_approved`.
    pub approval: Approval,
}

/// Why a submission was refused, with the reference's message and HTTP status as data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// The method was not `POST`.
    Method,
    /// No such post.
    PostMissing,
    /// Comments are closed on the post.
    Closed,
    /// The post is in the trash.
    Trashed,
    /// The post is not public or private (draft, pending, future).
    Draft,
    /// The post is password protected.
    PasswordProtected,
    /// The parent comment is missing or not approved.
    UnapprovedParent,
    /// Only logged-in users may comment.
    NotLoggedIn,
    /// Name or email missing.
    RequiredFields,
    /// Email fails the syntax check.
    InvalidEmail,
    /// Empty comment text.
    EmptyComment,
    /// Name longer than 245 bytes.
    AuthorTooLong,
    /// Email longer than 100 bytes.
    EmailTooLong,
    /// URL longer than 200 bytes.
    UrlTooLong,
    /// Comment longer than 65525 bytes.
    ContentTooLong,
    /// Same comment already posted.
    Duplicate,
    /// Less than the flood interval since this commenter's last comment.
    Flood,
    /// The per-IP rate limit was exceeded.
    RateLimited,
}

impl Rejection {
    /// The HTTP status the reference answers with.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::Method => 405,
            Self::Closed | Self::UnapprovedParent | Self::NotLoggedIn => 403,
            Self::Duplicate => 409,
            Self::Flood | Self::RateLimited => 429,
            _ => 200,
        }
    }

    /// The reference's message, or `None` where it ends the request with an empty body.
    #[must_use]
    pub const fn message(self) -> Option<&'static str> {
        Some(match self {
            Self::Method
            | Self::PostMissing
            | Self::Trashed
            | Self::Draft
            | Self::PasswordProtected => return None,
            Self::Closed => "Sorry, comments are closed for this item.",
            Self::UnapprovedParent => "Sorry, replies to unapproved comments are not allowed.",
            Self::NotLoggedIn => "Sorry, you must be logged in to comment.",
            Self::RequiredFields => "<strong>Error:</strong> Please fill the required fields.",
            Self::InvalidEmail => "<strong>Error:</strong> Please enter a valid email address.",
            Self::EmptyComment => "<strong>Error:</strong> Please type your comment text.",
            Self::AuthorTooLong => "<strong>Error:</strong> Your name is too long.",
            Self::EmailTooLong => "<strong>Error:</strong> Your email address is too long.",
            Self::UrlTooLong => "<strong>Error:</strong> Your URL is too long.",
            Self::ContentTooLong => "<strong>Error:</strong> Your comment is too long.",
            Self::Duplicate => {
                "Duplicate comment detected; it looks as though you&#8217;ve already said that!"
            }
            // The rate limit reuses the flood text: both mean "slow down".
            Self::Flood | Self::RateLimited => "You are posting comments too quickly. Slow down.",
        })
    }
}

/// Decides one submission at `now` (Unix seconds).
///
/// Runs the reference's checks in its order: the post, the parent, login, required fields,
/// empty text, field lengths, duplicates and flooding; then picks the approval status.
///
/// # Errors
///
/// [`Error`] when a [`History`] lookup fails.
pub fn decide(
    settings: &Settings,
    history: &mut impl History,
    submission: &Submission,
    commenter: Option<&Commenter>,
    now: i64,
) -> Result<Result<Candidate, Rejection>, Error> {
    let Some(post) = history.post(submission.post_id)? else {
        return Ok(Err(Rejection::PostMissing));
    };
    if let Some(refused) = post_refusal(settings, &post, now) {
        return Ok(Err(refused));
    }
    let moderator = commenter.is_some_and(|user| user.can_moderate);
    if submission.parent != 0 && !moderator {
        let status = history.comment_status(submission.parent)?;
        if status.as_deref() != Some("1") {
            return Ok(Err(Rejection::UnapprovedParent));
        }
    }
    // A logged-in user's profile replaces whatever the form sent.
    let (author, email, url) = match commenter {
        Some(user) => (user.name.trim(), user.email.trim(), user.url.trim()),
        None => {
            if settings.registration || &*post.status == "private" {
                return Ok(Err(Rejection::NotLoggedIn));
            }
            (
                submission.author.trim(),
                submission.email.trim(),
                submission.url.trim(),
            )
        }
    };
    let content = submission.content.trim();
    if settings.require_name_email && commenter.is_none() {
        if email.len() < 6 || author.is_empty() {
            return Ok(Err(Rejection::RequiredFields));
        }
        if !is_email(email) {
            return Ok(Err(Rejection::InvalidEmail));
        }
    }
    if content.is_empty() {
        return Ok(Err(Rejection::EmptyComment));
    }
    if let Some(refused) = length_refusal(author, email, url, content) {
        return Ok(Err(refused));
    }
    let mut candidate = Candidate {
        post_id: post.id,
        parent: submission.parent,
        author: sanitize_author(author).into(),
        email: if is_email(email) { email } else { "" }.into(),
        url: sanitize_url(url).into(),
        content: filter_content(content).into(),
        ip: sanitize_ip(&submission.ip).into(),
        agent: truncate(&submission.agent, 254).into(),
        user_id: commenter.map_or(0, |user| user.id),
        date: time::format(now, settings.offset_seconds).into(),
        date_gmt: time::format(now, 0).into(),
        approval: Approval::Hold,
    };
    if history.duplicate(&candidate)? {
        return Ok(Err(Rejection::Duplicate));
    }
    let exempt = commenter.is_some_and(|user| user.can_moderate || user.can_manage_options);
    if !exempt && (!candidate.ip.is_empty() || !candidate.email.is_empty()) {
        let since = time::format(now - 3600, 0);
        let last = history.last_comment_gmt(&candidate, &since)?;
        if last
            .and_then(|text| time::parse(text.as_bytes()))
            .is_some_and(|last| now - last < FLOOD_SECONDS)
        {
            return Ok(Err(Rejection::Flood));
        }
    }
    candidate.approval = approval(settings, history, &candidate, &post, commenter)?;
    Ok(Ok(candidate))
}

// Post-level refusals in the reference's order: closed, trash, not public, password.
fn post_refusal(settings: &Settings, post: &Post, now: i64) -> Option<Rejection> {
    if !comments_open(settings, post, now) {
        return Some(Rejection::Closed);
    }
    match &*post.status {
        "trash" => Some(Rejection::Trashed),
        "publish" | "private" if post.has_password => Some(Rejection::PasswordProtected),
        "publish" | "private" => None,
        _ => Some(Rejection::Draft),
    }
}

fn comments_open(settings: &Settings, post: &Post, now: i64) -> bool {
    if &*post.comment_status != "open" {
        return false;
    }
    // The old-post rule applies to the `post` type only, as the reference's default.
    let Some(days) = settings.close_after_days else {
        return true;
    };
    if &*post.post_type != "post" {
        return true;
    }
    let age_limit = i64::try_from(days.saturating_mul(86_400)).unwrap_or(i64::MAX);
    time::parse(post.date_gmt.as_bytes()).is_none_or(|posted| posted >= now - age_limit)
}

fn length_refusal(author: &str, email: &str, url: &str, content: &str) -> Option<Rejection> {
    // Byte lengths: the reference measures these columns in 8-bit units.
    if author.len() > 245 {
        Some(Rejection::AuthorTooLong)
    } else if email.len() > 100 {
        Some(Rejection::EmailTooLong)
    } else if url.len() > 200 {
        Some(Rejection::UrlTooLong)
    } else if content.len() > 65_525 {
        Some(Rejection::ContentTooLong)
    } else {
        None
    }
}

// The post author and moderators are approved outright; everyone else passes the moderation
// rules or is held; a disallowed-list match overrides both.
fn approval(
    settings: &Settings,
    history: &mut impl History,
    candidate: &Candidate,
    post: &Post,
    commenter: Option<&Commenter>,
) -> Result<Approval, Error> {
    let trusted = commenter.is_some_and(|user| user.id == post.author || user.can_moderate);
    let mut approval = if trusted || passes_moderation(settings, history, candidate)? {
        Approval::Approved
    } else {
        Approval::Hold
    };
    if is_disallowed(settings, candidate) {
        approval = if settings.trash {
            Approval::Trash
        } else {
            Approval::Spam
        };
    }
    Ok(approval)
}

fn passes_moderation(
    settings: &Settings,
    history: &mut impl History,
    candidate: &Candidate,
) -> Result<bool, Error> {
    if settings.moderation {
        return Ok(false);
    }
    if settings.max_links > 0 && link_count(&candidate.content) >= settings.max_links {
        return Ok(false);
    }
    let keys = settings.moderation_keys.trim();
    if scan(keys, candidate) {
        return Ok(false);
    }
    if settings.previously_approved {
        if candidate.author.is_empty() || candidate.email.is_empty() {
            return Ok(false);
        }
        let known = history.previously_approved(&candidate.author, &candidate.email)?;
        // The reference also refuses when the whole key list occurs in the email.
        return Ok(known && (keys.is_empty() || !candidate.email.contains(keys)));
    }
    Ok(true)
}

fn is_disallowed(settings: &Settings, candidate: &Candidate) -> bool {
    scan(settings.disallowed_keys.trim(), candidate)
}

// Author, email, URL, text, text without markup, IP and user agent: every field the keys scan.
fn scan(keys: &str, candidate: &Candidate) -> bool {
    let stripped = strip_tags(&candidate.content);
    matches_keys(
        keys,
        &[
            &candidate.author,
            &candidate.email,
            &candidate.url,
            &candidate.content,
            &stripped,
            &candidate.ip,
            &candidate.agent,
        ],
    )
}

fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
