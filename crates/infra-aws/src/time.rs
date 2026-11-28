//! UTC instants in the `YYYYMMDDTHHMMSSZ` form `SigV4` signs.

use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};

/// A UTC instant at second precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AmzDate {
    secs: u64,
}

impl AmzDate {
    /// The instant `secs` seconds after the Unix epoch.
    #[must_use]
    pub const fn from_unix(secs: u64) -> Self {
        Self { secs }
    }

    /// The current system time.
    ///
    /// # Errors
    ///
    /// [`SystemTimeError`] when the clock reads before the Unix epoch.
    pub fn now() -> Result<Self, SystemTimeError> {
        Ok(Self::from_unix(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        ))
    }

    /// `YYYYMMDDTHHMMSSZ` (the `X-Amz-Date` value).
    #[must_use]
    pub fn stamp(self) -> String {
        let (year, month, day) = civil(self.secs / 86_400);
        let second = self.secs % 86_400;
        format!(
            "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
            second / 3600,
            second / 60 % 60,
            second % 60
        )
    }

    /// `YYYYMMDD` (the credential scope date).
    #[must_use]
    pub fn day(self) -> String {
        let (year, month, day) = civil(self.secs / 86_400);
        format!("{year:04}{month:02}{day:02}")
    }
}

// Days since 1970-01-01 to a proleptic Gregorian date, by 400-year eras shifted to start in
// March so the leap day falls at the end of the year (all quantities stay non-negative).
fn civil(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::AmzDate;

    #[test]
    fn stamps_format_utc_dates() {
        assert_eq!(AmzDate::from_unix(0).stamp(), "19700101T000000Z");
        let suite = AmzDate::from_unix(1_440_938_160);
        assert_eq!(suite.stamp(), "20150830T123600Z");
        assert_eq!(suite.day(), "20150830");
        assert_eq!(
            AmzDate::from_unix(1_709_164_800 + 86_399).stamp(),
            "20240229T235959Z"
        );
        assert_eq!(AmzDate::from_unix(951_868_800).day(), "20000301");
    }

    #[test]
    fn now_is_after_the_suite_date() {
        assert!(AmzDate::now().is_ok_and(|now| now > AmzDate::from_unix(1_440_938_160)));
    }
}
