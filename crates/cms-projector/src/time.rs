//! Item versions and HTTP dates from the projector clock.

/// The version written on every item in a run: the run time as fixed-width UTC
/// `YYYYMMDDThhmmss.ffffffZ`, then `#` and the newest outbox id in the batch.
///
/// The run clock, not `post_modified_gmt`, leads the version because aggregate routes (home,
/// archives, feeds) must move forward even when the newest post is deleted. Runs are serial
/// (reserved concurrency 1), so each run's version sorts after every earlier one, and a replay
/// at the same clock is a conditional no-op.
#[must_use]
pub fn version(now_micros: i64, newest_id: &str) -> Box<str> {
    let micros = now_micros.max(0);
    let seconds = micros / 1_000_000;
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}.{:06}Z#{newest_id}",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
        micros % 1_000_000
    )
    .into()
}

const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `micros` since the epoch as an IMF-fixdate (`Thu, 01 Jan 1970 00:00:00 GMT`), the
/// `Last-Modified` format.
#[must_use]
pub fn imf_fixdate(micros: i64) -> String {
    let seconds = micros.max(0) / 1_000_000;
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days);
    // 1970-01-01 was a Thursday, which is index 0 of DAYS.
    let weekday = DAYS[usize::try_from(days % 7).unwrap_or_default()];
    let month_name = MONTHS[usize::try_from(month - 1).unwrap_or_default()];
    format!(
        "{weekday}, {day:02} {month_name} {year:04} {:02}:{:02}:{:02} GMT",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

// Days since 1970-01-01 to a proleptic Gregorian date (era-based civil-from-days algorithm).
const fn civil(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let of_era = shifted - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{imf_fixdate, version};

    #[test]
    fn versions_sort_by_run_time() {
        let early = version(1_000, "b");
        let late = version(20_000, "a");
        assert_eq!(&*early, "19700101T000000.001000Z#b");
        assert!(early < late);
        assert_eq!(&*version(-5, "x"), "19700101T000000.000000Z#x");
        // 2026-11-27 12:34:56.5 UTC.
        assert_eq!(
            &*version(1_795_782_896_500_000, "id"),
            "20261127T123456.500000Z#id"
        );
    }

    #[test]
    fn http_dates_match_known_instants() {
        assert_eq!(imf_fixdate(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        // 2026-11-27 12:34:56 UTC, a Friday.
        assert_eq!(
            imf_fixdate(1_795_782_896_000_000),
            "Fri, 27 Nov 2026 12:34:56 GMT"
        );
        // 2024-02-29, a leap day and a Thursday.
        assert_eq!(
            imf_fixdate(1_709_164_800_000_000),
            "Thu, 29 Feb 2024 00:00:00 GMT"
        );
        assert_eq!(imf_fixdate(-1), imf_fixdate(0));
    }
}
