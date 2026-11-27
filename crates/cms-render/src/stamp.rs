//! Stored date-times (`YYYY-MM-DD hh:mm:ss`) and the formats the outputs need.

use std::fmt::Write as _;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// A stored date-time was not a real calendar instant.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("not a date-time: {0:?}")]
pub struct StampError(Box<str>);

/// One valid date-time in the reference's stored form. The zero date of drafts is rejected,
/// so a published output can never carry it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

impl Stamp {
    /// Parses `YYYY-MM-DD hh:mm:ss`.
    ///
    /// # Errors
    ///
    /// [`StampError`] for any other shape, the zero date, or an impossible day or time.
    pub fn parse(text: &[u8]) -> Result<Self, StampError> {
        let fail = || StampError(String::from_utf8_lossy(text).into());
        let shape = b"0000-00-00 00:00:00";
        if text.len() != shape.len() {
            return Err(fail());
        }
        // Separators must sit exactly where the stored form puts them; every other byte is a digit.
        for (byte, want) in text.iter().zip(shape) {
            let ok = if *want == b'0' {
                byte.is_ascii_digit()
            } else {
                byte == want
            };
            if !ok {
                return Err(fail());
            }
        }
        let number = |from: usize, to: usize| {
            text[from..to]
                .iter()
                .fold(0u16, |n, d| n * 10 + u16::from(d - b'0'))
        };
        let narrow = |n: u16| u8::try_from(n).map_err(|_| fail());
        let stamp = Self {
            year: number(0, 4),
            month: narrow(number(5, 7))?,
            day: narrow(number(8, 10))?,
            hour: narrow(number(11, 13))?,
            minute: narrow(number(14, 16))?,
            second: narrow(number(17, 19))?,
        };
        let valid = stamp.year > 0
            && (1..=12).contains(&stamp.month)
            && stamp.day >= 1
            && stamp.day <= days_in_month(stamp.year, stamp.month)
            && stamp.hour < 24
            && stamp.minute < 60
            && stamp.second < 60;
        if valid { Ok(stamp) } else { Err(fail()) }
    }

    /// The year.
    #[must_use]
    pub const fn year(self) -> u16 {
        self.year
    }

    /// The month, 1 to 12.
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    /// The day of the month.
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }

    /// Seconds since 1970-01-01 00:00:00 in this stamp's own clock.
    #[must_use]
    pub fn seconds(self) -> i64 {
        let days = days_from_civil(i64::from(self.year), self.month, self.day);
        days * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
    }

    /// The display date, as the reference's default date format: `November 27, 2026`.
    #[must_use]
    pub fn display_date(self) -> String {
        format!("{} {}, {}", month_name(self.month), self.day, self.year)
    }

    /// RFC 822 form of a GMT stamp, as RSS writes it: `Fri, 27 Nov 2026 08:00:00 +0000`.
    #[must_use]
    pub fn rfc822(self) -> String {
        format!("{} +0000", self.dated())
    }

    /// IMF-fixdate of a GMT stamp, as HTTP `Last-Modified` writes it.
    #[must_use]
    pub fn http_date(self) -> String {
        format!("{} GMT", self.dated())
    }

    /// ISO 8601 of this local stamp with the offset implied by its GMT twin.
    #[must_use]
    pub fn iso8601(self, gmt: Self) -> String {
        let offset = (self.seconds() - gmt.seconds()) / 60;
        let sign = if offset < 0 { '-' } else { '+' };
        let offset = offset.abs();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            offset / 60,
            offset % 60
        )
    }

    fn dated(self) -> String {
        let days = days_from_civil(i64::from(self.year), self.month, self.day);
        // 1970-01-01 was a Thursday (index 3 from Monday).
        let weekday = usize::try_from((days + 3).rem_euclid(7)).unwrap_or_default();
        let mut out = String::with_capacity(26);
        let _ = write!(
            out,
            "{}, {:02} {} {:04} {:02}:{:02}:{:02}",
            DAYS[weekday],
            self.day,
            &month_name(self.month)[..3],
            self.year,
            self.hour,
            self.minute,
            self.second
        );
        out
    }
}

/// The English month name for 1 to 12 (empty otherwise).
#[must_use]
pub fn month_name(month: u8) -> &'static str {
    usize::from(month)
        .checked_sub(1)
        .and_then(|index| MONTHS.get(index))
        .copied()
        .unwrap_or_default()
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Days since 1970-01-01 of a proleptic Gregorian date (era-based civil-day count).
fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::{Stamp, month_name};

    fn stamp(text: &str) -> Stamp {
        Stamp::parse(text.as_bytes()).expect("valid stamp")
    }

    #[test]
    fn valid_stamps_parse_and_invalid_ones_do_not() {
        let s = stamp("2026-11-27 08:05:09");
        assert_eq!((s.year(), s.month(), s.day()), (2026, 11, 27));
        assert!(Stamp::parse(b"2024-02-29 00:00:00").is_ok());
        for bad in [
            "0000-00-00 00:00:00",
            "2025-02-29 00:00:00",
            "2026-13-01 00:00:00",
            "2026-04-31 00:00:00",
            "2026-11-27 24:00:00",
            "2026-11-27 23:60:00",
            "2026-11-27 23:59:60",
            "2026-11-27T08:05:09",
            "2026-11-2a 08:05:09",
            "2026-11-27",
            "1900-02-29 00:00:00",
        ] {
            assert!(Stamp::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert_eq!(
            Stamp::parse(b"x").map_err(|e| e.to_string()),
            Err("not a date-time: \"x\"".to_owned())
        );
        assert!(Stamp::parse(b"2000-02-29 00:00:00").is_ok());
        assert!(Stamp::parse(b"2026-06-30 00:00:00").is_ok());
        assert!(Stamp::parse(b"2026-02-28 00:00:00").is_ok());
    }

    #[test]
    fn formats_match_their_specs() {
        let gmt = stamp("2026-11-27 18:05:09");
        assert_eq!(gmt.rfc822(), "Fri, 27 Nov 2026 18:05:09 +0000");
        assert_eq!(gmt.http_date(), "Fri, 27 Nov 2026 18:05:09 GMT");
        assert_eq!(stamp("1970-01-01 00:00:00").seconds(), 0);
        assert_eq!(stamp("1969-12-31 00:00:00").rfc822(), "Wed, 31 Dec 1969 00:00:00 +0000");
        assert_eq!(stamp("2000-03-01 00:00:00").rfc822(), "Wed, 01 Mar 2000 00:00:00 +0000");
        let local = stamp("2026-11-27 10:05:09");
        assert_eq!(local.iso8601(gmt), "2026-11-27T10:05:09-08:00");
        assert_eq!(gmt.iso8601(local), "2026-11-27T18:05:09+08:00");
        assert_eq!(
            stamp("2026-01-01 05:30:00").iso8601(stamp("2026-01-01 00:00:00")),
            "2026-01-01T05:30:00+05:30"
        );
        assert_eq!(local.display_date(), "November 27, 2026");
        assert_eq!(month_name(1), "January");
        assert_eq!(month_name(0), "");
        assert_eq!(month_name(13), "");
    }
}
