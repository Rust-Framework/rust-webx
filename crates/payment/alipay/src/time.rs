//! Alipay timestamp helpers.
//!
//! Alipay's gateway and async notifications use `yyyy-MM-dd HH:mm:ss` in
//! **GMT+8** (official SDK: `SimpleDateFormat("yyyy-MM-dd HH:mm:ss")` with
//! `TimeZone.getTimeZone("GMT+8")`). The adapter keeps that conversion local so
//! it needs no extra date/time crate: civil-date arithmetic is done with the
//! well-known days-from-civil algorithm.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds between UTC and Alipay's `GMT+8` wall clock.
const ALIPAY_OFFSET_SECONDS: i64 = 8 * 60 * 60;

/// Current Unix time in seconds.
pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Format Unix seconds as Alipay's `yyyy-MM-dd HH:mm:ss` in GMT+8.
pub(crate) fn format_datetime(unix_seconds: i64) -> String {
    let local = unix_seconds.saturating_add(ALIPAY_OFFSET_SECONDS);
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

/// Parse Alipay's `yyyy-MM-dd HH:mm:ss` (GMT+8) into Unix seconds.
///
/// Returns `None` for anything that is not exactly the documented shape; a
/// lenient parse would silently shift event timestamps.
pub(crate) fn parse_datetime(value: &str) -> Option<i64> {
    let bytes = value.trim().as_bytes();
    if bytes.len() != 19 {
        return None;
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b' '
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = parse_digits(bytes, 0, 4)?;
    let month = parse_digits(bytes, 5, 2)?;
    let day = parse_digits(bytes, 8, 2)?;
    let hour = parse_digits(bytes, 11, 2)?;
    let minute = parse_digits(bytes, 14, 2)?;
    let second = parse_digits(bytes, 17, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - ALIPAY_OFFSET_SECONDS)
}

fn parse_digits(bytes: &[u8], start: usize, len: usize) -> Option<i64> {
    let mut value = 0i64;
    for byte in bytes.get(start..start + len)? {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i64::from(byte - b'0');
    }
    Some(value)
}

/// Convert days since 1970-01-01 to a proleptic Gregorian `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, valid for the whole `i64` range used by
/// the adapter (certificate/event timestamps are far inside it).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Convert a proleptic Gregorian date to days since 1970-01-01.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let month_prime = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_gmt_plus_eight_wall_clock() {
        assert_eq!(format_datetime(0), "1970-01-01 08:00:00");
        // 2023-11-14T22:13:20Z + 8h.
        assert_eq!(format_datetime(1_700_000_000), "2023-11-15 06:13:20");
    }

    #[test]
    fn parses_back_to_the_same_instant() {
        for timestamp in [0i64, 1_700_000_000, 951_782_400 /* 2000-02-29 */] {
            let text = format_datetime(timestamp);
            assert_eq!(
                parse_datetime(&text),
                Some(timestamp),
                "timestamp {timestamp}"
            );
        }
        assert_eq!(parse_datetime("2023-11-15 06:13:20"), Some(1_700_000_000));
    }

    #[test]
    fn rejects_malformed_timestamps() {
        for value in [
            "",
            "2023-11-15",
            "2023-11-15T06:13:20",
            "2023-13-15 06:13:20",
            "2023-11-32 06:13:20",
            "2023-11-15 24:00:00",
            "2023-11-15 06:60:00",
            "2023-11-15 06:13:60",
            "abcd-11-15 06:13:20",
        ] {
            assert_eq!(parse_datetime(value), None, "value {value:?}");
        }
    }
}
