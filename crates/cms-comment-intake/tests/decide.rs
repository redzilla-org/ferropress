//! Every decision branch of the reference's comment submission, one table row each.
//!
//! The reference lab is not run here (it needs Docker); these rows encode the reference's
//! documented behavior and stand in for the lab scenarios until the lab runs against the port.
#![expect(clippy::enum_glob_use, reason = "the tables name most variants")]
#![expect(clippy::too_many_lines, reason = "one table row per decision branch")]

use cms_comment_intake::{
    Approval, Candidate, Commenter, Error, History, Post, Rejection, Settings, Submission, decide,
};

/// 2026-11-18 11:06:40 UTC.
const NOW: i64 = 1_795_000_000;

#[derive(Clone)]
struct Fake {
    post: Option<Post>,
    parent: Option<Box<str>>,
    duplicate: bool,
    last: Option<Box<str>>,
    previously: bool,
    fail: bool,
}

impl History for Fake {
    fn post(&mut self, _id: u64) -> Result<Option<Post>, Error> {
        if self.fail {
            return Err(Error::Unavailable);
        }
        Ok(self.post.clone())
    }
    fn comment_status(&mut self, _id: u64) -> Result<Option<Box<str>>, Error> {
        Ok(self.parent.clone())
    }
    fn duplicate(&mut self, _candidate: &Candidate) -> Result<bool, Error> {
        Ok(self.duplicate)
    }
    fn last_comment_gmt(&mut self, _c: &Candidate, since: &str) -> Result<Option<Box<str>>, Error> {
        assert_eq!(since, "2026-11-18 10:06:40");
        Ok(self.last.clone())
    }
    fn previously_approved(&mut self, _author: &str, _email: &str) -> Result<bool, Error> {
        Ok(self.previously)
    }
}

struct Case {
    settings: Settings,
    history: Fake,
    submission: Submission,
    commenter: Option<Commenter>,
}

fn base() -> Case {
    Case {
        settings: Settings::from_values(|_| None).unwrap_or_else(|_| unreachable_settings()),
        history: Fake {
            post: Some(Post {
                id: 7,
                author: 1,
                status: "publish".into(),
                comment_status: "open".into(),
                post_type: "post".into(),
                has_password: false,
                date_gmt: "2026-11-01 00:00:00".into(),
            }),
            parent: None,
            duplicate: false,
            last: None,
            previously: true,
            fail: false,
        },
        submission: Submission {
            post_id: 7,
            parent: 0,
            author: "Ada".into(),
            email: "ada@example.com".into(),
            url: String::new().into(),
            content: "Hello there".into(),
            ip: "203.0.113.9".into(),
            agent: "UA/1".into(),
        },
        commenter: None,
    }
}

// The defaults never fail to parse; a mismatch shows up as failing rows, not a panic.
fn unreachable_settings() -> Settings {
    Settings {
        require_name_email: false,
        registration: true,
        moderation: true,
        previously_approved: true,
        max_links: 0,
        moderation_keys: "".into(),
        disallowed_keys: "".into(),
        close_after_days: None,
        offset_seconds: 0,
        trash: true,
    }
}

fn user(id: u64, moderator: bool) -> Commenter {
    Commenter {
        id,
        name: "Grace".into(),
        email: "grace@example.com".into(),
        url: "https://grace.example".into(),
        can_moderate: moderator,
        can_manage_options: moderator,
    }
}

fn run(mut case: Case) -> Result<Approval, Rejection> {
    let outcome = decide(
        &case.settings,
        &mut case.history,
        &case.submission,
        case.commenter.as_ref(),
        NOW,
    );
    match outcome {
        Ok(Ok(candidate)) => Ok(candidate.approval),
        Ok(Err(rejection)) => Err(rejection),
        Err(_) => Err(Rejection::Method),
    }
}

type Edit = fn(&mut Case);

#[test]
fn every_branch_matches_the_reference() {
    use Approval::{Approved, Hold, Spam, Trash};
    use Rejection::*;
    let rows: Vec<(&str, Edit, Result<Approval, Rejection>)> = vec![
        ("baseline", |_| {}, Ok(Approved)),
        ("post missing", |c| c.history.post = None, Err(PostMissing)),
        (
            "comments closed",
            |c| set_post(c, |p| p.comment_status = "closed".into()),
            Err(Closed),
        ),
        (
            "post trashed",
            |c| set_post(c, |p| p.status = "trash".into()),
            Err(Trashed),
        ),
        (
            "post draft",
            |c| set_post(c, |p| p.status = "draft".into()),
            Err(Draft),
        ),
        (
            "post scheduled",
            |c| set_post(c, |p| p.status = "future".into()),
            Err(Draft),
        ),
        (
            "password",
            |c| set_post(c, |p| p.has_password = true),
            Err(PasswordProtected),
        ),
        (
            "old post closed",
            |c| c.settings.close_after_days = Some(14),
            Err(Closed),
        ),
        (
            "old post young enough",
            |c| c.settings.close_after_days = Some(30),
            Ok(Approved),
        ),
        (
            "old page stays open",
            |c| {
                c.settings.close_after_days = Some(14);
                set_post(c, |p| p.post_type = "page".into());
            },
            Ok(Approved),
        ),
        (
            "parent held",
            |c| reply_to(c, Some("0")),
            Err(UnapprovedParent),
        ),
        (
            "parent missing",
            |c| reply_to(c, None),
            Err(UnapprovedParent),
        ),
        ("parent approved", |c| reply_to(c, Some("1")), Ok(Approved)),
        (
            "moderator replies to held parent",
            |c| {
                reply_to(c, Some("0"));
                c.commenter = Some(user(5, true));
            },
            Ok(Approved),
        ),
        (
            "registration required",
            |c| c.settings.registration = true,
            Err(NotLoggedIn),
        ),
        (
            "private post anonymous",
            |c| set_post(c, |p| p.status = "private".into()),
            Err(NotLoggedIn),
        ),
        (
            "private post logged in",
            |c| {
                set_post(c, |p| p.status = "private".into());
                c.commenter = Some(user(5, false));
            },
            Ok(Approved),
        ),
        (
            "name missing",
            |c| c.submission.author = "  ".into(),
            Err(RequiredFields),
        ),
        (
            "email too short",
            |c| c.submission.email = "a@b.c".into(),
            Err(RequiredFields),
        ),
        (
            "email invalid",
            |c| c.submission.email = "not-an-email".into(),
            Err(InvalidEmail),
        ),
        (
            "fields optional but unknown author is held",
            |c| {
                c.settings.require_name_email = false;
                c.submission.author = "".into();
                c.submission.email = "".into();
            },
            Ok(Hold),
        ),
        (
            "empty comment",
            |c| c.submission.content = " \n ".into(),
            Err(EmptyComment),
        ),
        (
            "name too long",
            |c| c.submission.author = "a".repeat(246).into(),
            Err(AuthorTooLong),
        ),
        (
            "email too long",
            |c| c.submission.email = format!("{}@example.com", "a".repeat(89)).into(),
            Err(EmailTooLong),
        ),
        (
            "url too long",
            |c| c.submission.url = format!("https://{}", "u".repeat(193)).into(),
            Err(UrlTooLong),
        ),
        (
            "comment too long",
            |c| c.submission.content = "c".repeat(65_526).into(),
            Err(ContentTooLong),
        ),
        ("duplicate", |c| c.history.duplicate = true, Err(Duplicate)),
        (
            "flood",
            |c| c.history.last = Some("2026-11-18 11:06:30".into()),
            Err(Flood),
        ),
        (
            "after the flood interval",
            |c| c.history.last = Some("2026-11-18 11:06:25".into()),
            Ok(Approved),
        ),
        (
            "moderators skip the flood check",
            |c| {
                c.history.last = Some("2026-11-18 11:06:39".into());
                c.commenter = Some(user(5, true));
            },
            Ok(Approved),
        ),
        (
            "no ip and no email skips the flood check",
            |c| {
                c.settings.require_name_email = false;
                c.submission.email = "".into();
                c.submission.ip = "".into();
                c.history.last = Some("2026-11-18 11:06:39".into());
            },
            Ok(Hold),
        ),
        ("moderation on", |c| c.settings.moderation = true, Ok(Hold)),
        (
            "post author bypasses moderation",
            |c| {
                c.settings.moderation = true;
                c.commenter = Some(user(1, false));
            },
            Ok(Approved),
        ),
        (
            "moderator bypasses moderation",
            |c| {
                c.settings.moderation = true;
                c.commenter = Some(user(5, true));
            },
            Ok(Approved),
        ),
        (
            "other user is moderated",
            |c| {
                c.settings.moderation = true;
                c.commenter = Some(user(5, false));
            },
            Ok(Hold),
        ),
        (
            "two links hold",
            |c| c.submission.content = "https://a.example http://b.example".into(),
            Ok(Hold),
        ),
        (
            "link limit off",
            |c| {
                c.settings.max_links = 0;
                c.submission.content = "https://a.example http://b.example".into();
            },
            Ok(Approved),
        ),
        (
            "one link passes",
            |c| c.submission.content = "see https://a.example".into(),
            Ok(Approved),
        ),
        (
            "moderation key in text",
            |c| c.settings.moderation_keys = "\nTHERE\n".into(),
            Ok(Hold),
        ),
        (
            "moderation key in ip",
            |c| c.settings.moderation_keys = "203.0.113".into(),
            Ok(Hold),
        ),
        (
            "moderation key in agent",
            |c| c.settings.moderation_keys = "ua/".into(),
            Ok(Hold),
        ),
        (
            "no earlier approved comment",
            |c| c.history.previously = false,
            Ok(Hold),
        ),
        (
            "earlier approval not required",
            |c| {
                c.history.previously = false;
                c.settings.previously_approved = false;
            },
            Ok(Approved),
        ),
        (
            "disallowed word",
            |c| c.settings.disallowed_keys = "hello".into(),
            Ok(Trash),
        ),
        (
            "disallowed word without trash",
            |c| {
                c.settings.disallowed_keys = "hello".into();
                c.settings.trash = false;
            },
            Ok(Spam),
        ),
        (
            "disallowed word behind markup",
            |c| {
                c.settings.disallowed_keys = "hello".into();
                c.submission.content = "<b>hel</b>lo".into();
            },
            Ok(Trash),
        ),
        (
            "disallowed overrides a moderator",
            |c| {
                c.settings.disallowed_keys = "grace@".into();
                c.commenter = Some(user(5, true));
            },
            Ok(Trash),
        ),
    ];
    for (name, edit, expected) in rows {
        let mut case = base();
        edit(&mut case);
        assert_eq!(run(case), expected, "{name}");
    }
}

fn set_post(case: &mut Case, edit: impl FnOnce(&mut Post)) {
    if let Some(post) = case.history.post.as_mut() {
        edit(post);
    }
}

fn reply_to(case: &mut Case, status: Option<&str>) {
    case.submission.parent = 3;
    case.history.parent = status.map(Into::into);
}

#[test]
fn accepted_comments_are_sanitized_and_dated() {
    let mut case = base();
    case.settings.offset_seconds = -18_000;
    case.submission.author = " <i>Ada</i>  Lovelace ".into();
    case.submission.url = "ada.example".into();
    case.submission.content = " <p>Hi</p> <script>x</script> ".into();
    case.submission.agent = "\u{e9}".repeat(200).into();
    let outcome = decide(
        &case.settings,
        &mut case.history,
        &case.submission,
        None,
        NOW,
    );
    let candidate = outcome.ok().and_then(Result::ok);
    assert!(candidate.is_some());
    if let Some(candidate) = candidate {
        assert_eq!(&*candidate.author, "Ada Lovelace");
        assert_eq!(&*candidate.url, "http://ada.example");
        assert_eq!(&*candidate.content, "Hi x");
        assert_eq!(candidate.agent.len(), 254);
        assert_eq!(&*candidate.date_gmt, "2026-11-18 11:06:40");
        assert_eq!(&*candidate.date, "2026-11-18 06:06:40");
        assert_eq!(candidate.user_id, 0);
        assert_eq!(candidate.post_id, 7);
    }
}

#[test]
fn logged_in_profiles_replace_the_form_fields() {
    let mut case = base();
    case.submission.author = "".into();
    case.submission.email = "".into();
    let grace = user(5, false);
    let outcome = decide(
        &case.settings,
        &mut case.history,
        &case.submission,
        Some(&grace),
        NOW,
    );
    let candidate = outcome.ok().and_then(Result::ok);
    assert!(candidate.is_some_and(|c| &*c.author == "Grace"
        && &*c.email == "grace@example.com"
        && &*c.url == "https://grace.example"
        && c.user_id == 5));
}

#[test]
fn invalid_optional_emails_are_not_stored() {
    let mut case = base();
    case.settings.require_name_email = false;
    case.submission.email = "nope".into();
    let outcome = decide(
        &case.settings,
        &mut case.history,
        &case.submission,
        None,
        NOW,
    );
    assert!(
        outcome.is_ok_and(|o| o.is_ok_and(|c| c.email.is_empty() && c.approval == Approval::Hold))
    );
}

#[test]
fn history_failures_are_errors() {
    let mut case = base();
    case.history.fail = true;
    let outcome = decide(
        &case.settings,
        &mut case.history,
        &case.submission,
        None,
        NOW,
    );
    assert_eq!(outcome, Err(Error::Unavailable));
}

#[test]
fn rejections_carry_the_reference_status_and_text() {
    use Rejection::*;
    let rows = [
        (Method, 405, None),
        (PostMissing, 200, None),
        (Trashed, 200, None),
        (Draft, 200, None),
        (PasswordProtected, 200, None),
        (
            Closed,
            403,
            Some("Sorry, comments are closed for this item."),
        ),
        (
            UnapprovedParent,
            403,
            Some("Sorry, replies to unapproved comments are not allowed."),
        ),
        (
            NotLoggedIn,
            403,
            Some("Sorry, you must be logged in to comment."),
        ),
        (
            RequiredFields,
            200,
            Some("<strong>Error:</strong> Please fill the required fields."),
        ),
        (
            InvalidEmail,
            200,
            Some("<strong>Error:</strong> Please enter a valid email address."),
        ),
        (
            EmptyComment,
            200,
            Some("<strong>Error:</strong> Please type your comment text."),
        ),
        (
            AuthorTooLong,
            200,
            Some("<strong>Error:</strong> Your name is too long."),
        ),
        (
            EmailTooLong,
            200,
            Some("<strong>Error:</strong> Your email address is too long."),
        ),
        (
            UrlTooLong,
            200,
            Some("<strong>Error:</strong> Your URL is too long."),
        ),
        (
            ContentTooLong,
            200,
            Some("<strong>Error:</strong> Your comment is too long."),
        ),
        (
            Duplicate,
            409,
            Some("Duplicate comment detected; it looks as though you&#8217;ve already said that!"),
        ),
        (
            Flood,
            429,
            Some("You are posting comments too quickly. Slow down."),
        ),
        (
            RateLimited,
            429,
            Some("You are posting comments too quickly. Slow down."),
        ),
    ];
    for (rejection, status, message) in rows {
        assert_eq!(rejection.status(), status, "{rejection:?}");
        assert_eq!(rejection.message(), message, "{rejection:?}");
    }
    let cells = [
        (Approval::Approved, "1"),
        (Approval::Hold, "0"),
        (Approval::Spam, "spam"),
        (Approval::Trash, "trash"),
    ];
    for (approval, cell) in cells {
        assert_eq!(approval.cell(), cell);
    }
}
