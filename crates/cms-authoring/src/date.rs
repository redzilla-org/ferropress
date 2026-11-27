//! `YYYY-MM-DD hh:mm:ss` text for post date columns.

/// Formats `unix` seconds (UTC, shifted by the caller for local time) as a date column value.
#[must_use]
pub fn format(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        seconds / 3_600,
        seconds % 3_600 / 60,
        seconds % 60
    )
}

// Days since 1970-01-01 to a proleptic Gregorian date, by 400-year eras.
fn civil(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::format;

    #[test]
    fn known_instants_format_as_date_columns() {
        assert_eq!(format(0), "1970-01-01 00:00:00");
        assert_eq!(format(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(format(1_798_761_599), "2026-12-31 23:59:59");
        assert_eq!(format(1_764_201_600 + 3_723), "2025-11-27 01:02:03");
        assert_eq!(format(-1), "1969-12-31 23:59:59");
    }
}
