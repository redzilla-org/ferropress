//! Reference date cells (`YYYY-MM-DD hh:mm:ss`) from Unix seconds and back.

/// Seconds since the Unix epoch as the reference's fixed-width date text.
///
/// Uses the civil-from-days conversion over the proleptic Gregorian calendar, so no time zone
/// database is needed; `offset` (seconds east of UTC) shifts GMT to site-local time.
#[must_use]
pub(crate) fn format(unix: i64, offset: i64) -> String {
    let at = unix + offset;
    let days = at.div_euclid(86_400);
    let second = at.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        second / 3600,
        second % 3600 / 60,
        second % 60
    )
}

/// Unix seconds for a GMT date cell, or `None` for the zero date or malformed text.
#[must_use]
pub(crate) fn parse(text: &[u8]) -> Option<i64> {
    // Fixed width: "YYYY-MM-DD hh:mm:ss".
    if text.len() != 19 || text[4] != b'-' || text[7] != b'-' || text[10] != b' ' {
        return None;
    }
    let field = |range: std::ops::Range<usize>| -> Option<i64> {
        std::str::from_utf8(&text[range]).ok()?.parse().ok()
    };
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

// Howard Hinnant's public-domain algorithms, era-based so they hold for every year.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::{format, parse};

    #[test]
    fn known_instants_format_and_parse_back() {
        let cases = [
            (0, "1970-01-01 00:00:00"),
            (951_782_400, "2000-02-29 00:00:00"),
            (1_795_000_000, "2026-11-18 11:06:40"),
            (-1, "1969-12-31 23:59:59"),
        ];
        for (unix, text) in cases {
            assert_eq!(format(unix, 0), text);
            assert_eq!(parse(text.as_bytes()), Some(unix), "{text}");
        }
        assert_eq!(format(0, -3600), "1969-12-31 23:00:00");
        assert_eq!(format(0, 19_800), "1970-01-01 05:30:00");
    }

    #[test]
    fn zero_and_malformed_dates_do_not_parse() {
        for text in [
            "0000-00-00 00:00:00",
            "2026-13-01 00:00:00",
            "2026-01-00 00:00:00",
            "2026/01/01 00:00:00",
            "2026-01-01T00:00:00",
            "2026-01-01 0a:00:00",
            "short",
        ] {
            assert_eq!(parse(text.as_bytes()), None, "{text}");
        }
        assert_eq!(parse(&[0xff; 19]), None);
    }
}
