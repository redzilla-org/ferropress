//! Discussion options that decide whether a comment is accepted, held or refused.

use crate::error::Error;

/// Seconds a commenter must wait between two comments (the reference's flood interval).
pub const FLOOD_SECONDS: i64 = 15;

/// Comments accepted per IP per minute before the rate limiter answers 429.
pub const RATE_PER_MINUTE: u64 = 5;

/// The option values the acceptance rules read, already parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one reference option"
)]
pub struct Settings {
    /// `require_name_email`: anonymous commenters must give a name and an email.
    pub require_name_email: bool,
    /// `comment_registration`: only logged-in users may comment.
    pub registration: bool,
    /// `comment_moderation`: every comment is held.
    pub moderation: bool,
    /// `comment_previously_approved`: the author needs an earlier approved comment.
    pub previously_approved: bool,
    /// `comment_max_links`: hold at or above this many links; `0` disables the check.
    pub max_links: u64,
    /// `moderation_keys`: one word or fragment per line; a match holds the comment.
    pub moderation_keys: Box<str>,
    /// `disallowed_keys`: one word or fragment per line; a match trashes the comment.
    pub disallowed_keys: Box<str>,
    /// `close_comments_for_old_posts` together with `close_comments_days_old`, in days.
    pub close_after_days: Option<u64>,
    /// `gmt_offset` in seconds east of UTC, for the local `comment_date`.
    pub offset_seconds: i64,
    /// Whether the trash is enabled (the reference's default 30-day trash); when it is not,
    /// disallowed comments become spam instead of trash.
    pub trash: bool,
}

impl Settings {
    /// Option names [`Self::from_values`] asks for, in the order it asks.
    pub const NAMES: [&'static str; 11] = [
        "require_name_email",
        "comment_registration",
        "comment_moderation",
        "comment_previously_approved",
        "comment_max_links",
        "moderation_keys",
        "disallowed_keys",
        "close_comments_for_old_posts",
        "close_comments_days_old",
        "gmt_offset",
        "timezone_string",
    ];

    /// Parses the options returned by `value`, using the reference's install default for every
    /// missing row.
    ///
    /// # Errors
    ///
    /// [`Error::TimeZone`] when `timezone_string` names a zone; local dates for named zones need
    /// a time zone database, which this profile does not ship.
    pub fn from_values(mut value: impl FnMut(&str) -> Option<Box<[u8]>>) -> Result<Self, Error> {
        let mut text = |name: &str, default: &str| -> String {
            value(name).map_or_else(
                || default.to_owned(),
                |bytes| String::from_utf8_lossy(&bytes).into_owned(),
            )
        };
        let require_name_email = truthy(&text("require_name_email", "1"));
        let registration = truthy(&text("comment_registration", "0"));
        let moderation = is_one(&text("comment_moderation", "0"));
        let previously_approved = is_one(&text("comment_previously_approved", "1"));
        let max_links = leading_int(&text("comment_max_links", "2"))
            .max(0)
            .unsigned_abs();
        let moderation_keys = text("moderation_keys", "").into();
        let disallowed_keys = text("disallowed_keys", "").into();
        let close = truthy(&text("close_comments_for_old_posts", "0"));
        let days = leading_int(&text("close_comments_days_old", "14"));
        let offset = text("gmt_offset", "0");
        let zone = text("timezone_string", "");
        if !zone.is_empty() && zone != "UTC" {
            return Err(Error::TimeZone(zone.into()));
        }
        Ok(Self {
            require_name_email,
            registration,
            moderation,
            previously_approved,
            max_links,
            moderation_keys,
            disallowed_keys,
            // A zero or negative day count leaves comments open, as the reference does.
            close_after_days: (close && days > 0).then_some(days.unsigned_abs()),
            offset_seconds: offset_seconds(&offset),
            trash: true,
        })
    }
}

// PHP truthiness of a stored option string: only "" and "0" are false.
fn truthy(text: &str) -> bool {
    !(text.is_empty() || text == "0")
}

// The reference compares `1 == $option`, a numeric comparison for numeric strings.
fn is_one(text: &str) -> bool {
    text.trim()
        .parse::<f64>()
        .is_ok_and(|number| (number - 1.0).abs() < f64::EPSILON)
}

// PHP integer conversion of a string: optional sign then leading digits, else 0.
fn leading_int(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end]
        .parse::<i64>()
        .map_or(0, |number| sign * number)
}

// `gmt_offset` is hours, possibly fractional (5.5, -9.5); malformed text means UTC.
fn offset_seconds(text: &str) -> i64 {
    let hours = text.trim().parse::<f64>().unwrap_or(0.0);
    if !hours.is_finite() || hours.abs() > 24.0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded to +-86400 seconds above"
    )]
    let seconds = (hours * 3600.0).round() as i64;
    seconds
}

#[cfg(test)]
mod tests {
    use super::{Settings, is_one, leading_int, offset_seconds, truthy};
    use crate::error::Error;

    fn settings(pairs: &[(&str, &str)]) -> Result<Settings, Error> {
        Settings::from_values(|name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| Box::from(value.as_bytes()))
        })
    }

    #[test]
    fn missing_rows_take_the_install_defaults() {
        let defaults = settings(&[]);
        assert_eq!(
            defaults,
            Ok(Settings {
                require_name_email: true,
                registration: false,
                moderation: false,
                previously_approved: true,
                max_links: 2,
                moderation_keys: "".into(),
                disallowed_keys: "".into(),
                close_after_days: None,
                offset_seconds: 0,
                trash: true,
            })
        );
        assert_eq!(Settings::NAMES.len(), 11);
    }

    #[test]
    fn stored_rows_are_parsed_like_the_reference() {
        let parsed = settings(&[
            ("require_name_email", ""),
            ("comment_registration", "1"),
            ("comment_moderation", "1.0"),
            ("comment_previously_approved", "0"),
            ("comment_max_links", "5 links"),
            ("moderation_keys", "casino\nloan"),
            ("disallowed_keys", "spam"),
            ("close_comments_for_old_posts", "1"),
            ("close_comments_days_old", "30"),
            ("gmt_offset", "-5.5"),
            ("timezone_string", "UTC"),
        ]);
        assert!(parsed.is_ok());
        if let Ok(parsed) = parsed {
            assert!(!parsed.require_name_email);
            assert!(parsed.registration);
            assert!(parsed.moderation);
            assert!(!parsed.previously_approved);
            assert_eq!(parsed.max_links, 5);
            assert_eq!(&*parsed.moderation_keys, "casino\nloan");
            assert_eq!(&*parsed.disallowed_keys, "spam");
            assert_eq!(parsed.close_after_days, Some(30));
            assert_eq!(parsed.offset_seconds, -19_800);
        }
        let closed_zero_days = settings(&[
            ("close_comments_for_old_posts", "1"),
            ("close_comments_days_old", "0"),
            ("comment_max_links", "-3"),
        ]);
        assert!(closed_zero_days.is_ok_and(|s| s.close_after_days.is_none() && s.max_links == 0));
    }

    #[test]
    fn named_zones_are_refused() {
        let refused = settings(&[("timezone_string", "Europe/Paris")]);
        assert_eq!(refused, Err(Error::TimeZone("Europe/Paris".into())));
    }

    #[test]
    fn php_conversions_match_their_edge_cases() {
        assert!(truthy("1") && truthy("yes") && truthy("00"));
        assert!(!truthy("") && !truthy("0"));
        assert!(is_one("1") && is_one(" 1 ") && is_one("1.0"));
        assert!(!is_one("0") && !is_one("1abc") && !is_one(""));
        assert_eq!(leading_int("12abc"), 12);
        assert_eq!(leading_int(" -4"), -4);
        assert_eq!(leading_int("+7"), 7);
        assert_eq!(leading_int("x"), 0);
        assert_eq!(leading_int(""), 0);
        assert_eq!(offset_seconds("2"), 7200);
        assert_eq!(offset_seconds("bad"), 0);
        assert_eq!(offset_seconds("99"), 0);
        assert_eq!(offset_seconds("NaN"), 0);
    }
}
