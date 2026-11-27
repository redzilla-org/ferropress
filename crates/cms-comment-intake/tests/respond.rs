//! The reference's post-submit redirect and commenter cookies.

use cms_comment_intake::{
    Approval, COOKIE_SECONDS, Candidate, Target, cookie_hash, cookies, location, moderation_hash,
};

fn candidate(approval: Approval, email: &str) -> Candidate {
    Candidate {
        post_id: 7,
        parent: 0,
        author: "Ada L".into(),
        email: email.into(),
        url: "https://ada.example/".into(),
        content: "Hi".into(),
        ip: "203.0.113.9".into(),
        agent: "UA".into(),
        user_id: 0,
        date: "2026-11-18 11:06:40".into(),
        date_gmt: "2026-11-18 11:06:40".into(),
        approval,
    }
}

const TARGET: Target<'static> = Target {
    redirect_to: None,
    permalink: "https://example.com/blog/hello/",
    home: "https://example.com/blog",
    salt: b"nonce-salt",
};

#[test]
fn redirects_follow_the_reference() {
    let approved = candidate(Approval::Approved, "ada@example.com");
    assert_eq!(
        location(41, &approved, &TARGET),
        "https://example.com/blog/hello/#comment-41"
    );
    let custom = Target {
        redirect_to: Some("/blog/thanks/"),
        ..TARGET
    };
    assert_eq!(location(41, &approved, &custom), "/blog/thanks/#comment-41");
    let blank = Target {
        redirect_to: Some(""),
        ..TARGET
    };
    assert_eq!(
        location(41, &approved, &blank),
        "https://example.com/blog/hello/#comment-41"
    );

    let held = candidate(Approval::Hold, "ada@example.com");
    let hash = moderation_hash(b"nonce-salt", "2026-11-18 11:06:40");
    assert_eq!(
        location(41, &held, &TARGET),
        format!("https://example.com/blog/hello/?unapproved=41&moderation-hash={hash}#comment-41")
    );
    // Without an email the author cannot be shown the held comment, so no query is added.
    let anonymous = candidate(Approval::Hold, "");
    assert_eq!(
        location(41, &anonymous, &TARGET),
        "https://example.com/blog/hello/#comment-41"
    );
    let spam = candidate(Approval::Spam, "ada@example.com");
    assert_eq!(
        location(41, &spam, &TARGET),
        "https://example.com/blog/hello/#comment-41"
    );
}

#[test]
fn off_site_and_injected_targets_are_neutralized() {
    let approved = candidate(Approval::Approved, "ada@example.com");
    let evil = Target {
        redirect_to: Some("https://evil.example/"),
        ..TARGET
    };
    assert_eq!(location(1, &approved, &evil), "https://example.com/blog");
    let split = Target {
        redirect_to: Some("/blog/x\r\nSet-Cookie: a=b"),
        ..TARGET
    };
    assert_eq!(
        location(1, &approved, &split),
        "/blog/xSet-Cookie: a=b#comment-1"
    );
}

#[test]
fn cookies_need_consent_and_an_anonymous_commenter() {
    let hash = cookie_hash("https://example.com/blog");
    let anonymous = candidate(Approval::Approved, "ada@example.com");
    let set = cookies(&anonymous, true, "https://example.com/blog", "/blog/");
    assert_eq!(
        set,
        vec![
            format!(
                "comment_author_{hash}=Ada%20L; Max-Age={COOKIE_SECONDS}; Path=/blog/; SameSite=Lax; Secure"
            ),
            format!(
                "comment_author_email_{hash}=ada%40example.com; Max-Age={COOKIE_SECONDS}; Path=/blog/; SameSite=Lax; Secure"
            ),
            format!(
                "comment_author_url_{hash}=https%3A%2F%2Fada.example%2F; Max-Age={COOKIE_SECONDS}; Path=/blog/; SameSite=Lax; Secure"
            ),
        ]
    );
    let cleared = cookies(&anonymous, false, "http://example.com", "/");
    let plain = cookie_hash("http://example.com");
    assert_eq!(
        cleared[0],
        format!("comment_author_{plain}=; Max-Age=0; Path=/; SameSite=Lax")
    );
    let mut logged_in = anonymous;
    logged_in.user_id = 5;
    assert!(cookies(&logged_in, true, "https://example.com", "/").is_empty());
}
