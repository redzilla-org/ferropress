//! Save-context slugs and the reference's numeric-suffix uniqueness rule.

use cms_content::PostId;

use crate::model::Status;
use crate::{Error, Repo};

/// The reference's `post_name` column width.
const MAX: usize = 200;

/// Typographic marks the save context turns into a dash, as percent-encoded UTF-8.
const DASHES: [&str; 3] = ["%c2%a0", "%e2%80%93", "%e2%80%94"];

/// Quotes and marks the save context drops, as percent-encoded UTF-8.
const DROPPED: [&str; 12] = [
    "%e2%80%98",
    "%e2%80%99",
    "%e2%80%9a",
    "%e2%80%9b",
    "%e2%80%9c",
    "%e2%80%9d",
    "%e2%80%9e",
    "%e2%80%9f",
    "%e2%80%a6",
    "%c2%ab",
    "%c2%bb",
    "%c2%a1",
];

/// Turns a title or a submitted slug into a `post_name` (save context).
///
/// Tags are stripped, letters lower-cased, non-ASCII percent-encoded, typographic dashes become
/// `-`, quotes are dropped, and anything else that is not `[a-z0-9_-]` is removed. Accent folding
/// (the reference's `remove_accents`) is not applied; accented letters stay percent-encoded.
#[must_use]
pub fn sanitize(title: &str) -> String {
    let lowered = strip_tags(title).to_lowercase();
    let mut encoded = String::with_capacity(lowered.len());
    for ch in lowered.chars() {
        match ch {
            '.' | '/' => encoded.push('-'),
            '&' | '%' => encoded.push(' '),
            ch if ch.is_ascii() => encoded.push(ch),
            ch => {
                let mut buffer = [0; 4];
                for byte in ch.encode_utf8(&mut buffer).bytes() {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    encoded.push('%');
                    encoded.push(char::from(HEX[usize::from(byte >> 4)]));
                    encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
                }
            }
        }
    }
    for dash in DASHES {
        encoded = encoded.replace(dash, "-");
    }
    for dropped in DROPPED {
        encoded = encoded.replace(dropped, "");
    }
    encoded = encoded.replace("%c3%97", "x");
    let mut out = String::with_capacity(encoded.len());
    for ch in encoded.chars() {
        let mapped = match ch {
            'a'..='z' | '0'..='9' | '_' | '%' => ch,
            '-' => '-',
            ch if ch.is_whitespace() => '-',
            _ => continue,
        };
        // Runs of dashes and spaces collapse to one dash.
        if mapped == '-' && out.ends_with('-') {
            continue;
        }
        out.push(mapped);
    }
    truncate(out.trim_matches('-'), MAX).trim_matches('-').to_owned()
}

/// The slug a post saves with: drafts and pending posts keep theirs as is; otherwise a `-2`,
/// `-3`, ... suffix is added while another post of the same type holds it.
///
/// # Errors
///
/// [`Error`] when the repository fails.
pub fn unique(
    repo: &mut dyn Repo,
    slug: &str,
    status: Status,
    id: Option<PostId>,
) -> Result<String, Error> {
    if matches!(status, Status::Draft | Status::Pending) || slug.is_empty() {
        return Ok(slug.to_owned());
    }
    if !repo.slug_taken(slug, "post", id)? {
        return Ok(slug.to_owned());
    }
    let mut suffix: u64 = 2;
    loop {
        let tail = format!("-{suffix}");
        let candidate = format!("{}{tail}", truncate(slug, MAX - tail.len()));
        if !repo.slug_taken(&candidate, "post", id)? {
            return Ok(candidate);
        }
        suffix += 1;
    }
}

// Cuts at `max` bytes without splitting a `%xx` octet.
fn truncate(slug: &str, max: usize) -> &str {
    if slug.len() <= max {
        return slug;
    }
    let mut end = max;
    for back in 1..=2 {
        if end >= back && slug.as_bytes()[end - back] == b'%' {
            end -= back;
            break;
        }
    }
    &slug[..end]
}

fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            ch if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{sanitize, truncate};

    #[test]
    fn titles_become_lowercase_dashed_slugs() {
        assert_eq!(sanitize("Hello World"), "hello-world");
        assert_eq!(sanitize("  Hello,   World!  "), "hello-world");
        assert_eq!(sanitize("<b>Bold</b> move"), "bold-move");
        assert_eq!(sanitize("v1.2 release"), "v1-2-release");
        assert_eq!(sanitize("Fish & Chips"), "fish-chips");
        assert_eq!(sanitize("snake_case"), "snake_case");
        assert_eq!(sanitize("Café"), "caf%c3%a9");
        assert_eq!(sanitize("a\u{2014}b"), "a-b");
        assert_eq!(sanitize("\u{201c}quoted\u{201d}"), "quoted");
        assert_eq!(sanitize("3\u{d7}4"), "3x4");
        assert_eq!(sanitize("---"), "");
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn long_slugs_are_cut_without_splitting_octets() {
        assert_eq!(sanitize(&"a".repeat(250)).len(), 200);
        assert_eq!(truncate("ab%c3%a9", 4), "ab");
        assert_eq!(truncate("ab%c3%a9", 3), "ab");
        assert_eq!(truncate("ab%c3%a9", 5), "ab%c3");
        assert_eq!(truncate("abc", 5), "abc");
    }
}
