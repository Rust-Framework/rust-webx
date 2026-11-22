//! RFC 3339 timestamps without a calendar dependency.
//!
//! WeChat Pay expresses `time_expire` and the `*_time` notification fields as
//! RFC 3339 (`2015-05-20T13:29:35+08:00`). The framework does not depend on a
//! date/time crate, and the adapter only needs two operations: render Unix
//! seconds as Beijing time and parse the RFC 3339 timestamps WeChat sends.
//! Both are implemented with the well-known civil-from-days algorithm
//! (Howard Hinnant) on 64-bit integers, so there is no floating point and no
//! timezone database.

use std::time::{SystemTime, UNIX_EPOCH};

/// Offset of the WeChat Pay wire timezone (China Standard Time, UTC+8, no DST).
const CST_OFFSET_SECONDS: i64 = 8 * 60 * 60;

/// Current Unix time in seconds (saturating on pre-epoch clocks).
pub(crate) fn unix_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// Render Unix seconds as RFC 3339 in China Standard Time.
///
/// WeChat Pay's documented examples use `+08:00`; emitting the merchant-local
/// offset keeps the value identical to what the official SDKs produce.
pub(crate) fn format_rfc3339(timestamp: i64) -> String {
    let shifted = timestamp.saturating_add(CST_OFFSET_SECONDS);
    let days = shifted.div_euclid(86_400);
    let seconds_of_day = shifted.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+08:00")
}

/// Parse the RFC 3339 subset WeChat Pay emits.
///
/// Accepts `YYYY-MM-DDTHH:MM:SS` followed by `Z`, `z` or `±HH:MM`, with an
/// optional fractional-seconds part (ignored at sub-second precision). Returns
/// Unix seconds, or `None` for malformed input.
pub(crate) fn parse_rfc3339(value: &str) -> Option<i64> {
    let text = value.trim();
    if !text.is_ascii() || text.len() < 20 {
        return None;
    }
    let bytes = text.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b't' && bytes[10] != b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let hour: u32 = text.get(11..13)?.parse().ok()?;
    let minute: u32 = text.get(14..16)?.parse().ok()?;
    let second: u32 = text.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = text.get(19..)?;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = &after_dot[digits..];
    }
    let offset_seconds = match rest {
        "Z" | "z" => 0,
        zone => {
            let zone = zone.as_bytes();
            if zone.len() != 6 || zone[3] != b':' {
                return None;
            }
            let sign = match zone[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone_hour: i64 = std::str::from_utf8(&zone[1..3]).ok()?.parse().ok()?;
            let zone_minute: i64 = std::str::from_utf8(&zone[4..6]).ok()?.parse().ok()?;
            if zone_hour > 23 || zone_minute > 59 {
                return None;
            }
            sign * (zone_hour * 3_600 + zone_minute * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    let seconds = i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second);
    Some(days * 86_400 + seconds - offset_seconds)
}

/// Days since 1970-01-01 to a proleptic Gregorian `(year, month, day)`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// Proleptic Gregorian `(year, month, day)` to days since 1970-01-01.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let shifted_month = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_beijing_time_with_explicit_offset() {
        // 2018-06-08T10:34:56+08:00 == 1528425296 UTC
        assert_eq!(format_rfc3339(1_528_425_296), "2018-06-08T10:34:56+08:00");
        assert_eq!(format_rfc3339(0), "1970-01-01T08:00:00+08:00");
        assert_eq!(format_rfc3339(1_514_764_800), "2018-01-01T08:00:00+08:00");
        // Leap day.
        assert_eq!(format_rfc3339(1_582_934_400), "2020-02-29T08:00:00+08:00");
    }

    #[test]
    fn parses_documented_rfc3339_forms() {
        assert_eq!(
            parse_rfc3339("2018-06-08T10:34:56+08:00"),
            Some(1_528_425_296)
        );
        assert_eq!(parse_rfc3339("2018-06-08T02:34:56Z"), Some(1_528_425_296));
        assert_eq!(
            parse_rfc3339("2018-06-08T10:34:56.123456789+08:00"),
            Some(1_528_425_296)
        );
        assert_eq!(
            parse_rfc3339("2018-06-07T21:34:56-05:00"),
            Some(1_528_425_296)
        );
        assert_eq!(parse_rfc3339("2020-02-29T00:00:00Z"), Some(1_582_934_400));
    }

    #[test]
    fn rejects_malformed_timestamps() {
        for value in [
            "",
            "not-a-date",
            "2018-06-08",
            "2018-13-08T10:34:56+08:00",
            "2018-06-08T24:00:00+08:00",
            "2018-06-08T10:34:56",
            "2018-06-08T10:34:56+8:00",
            "2018-06-08T10:34:56.12x+08:00",
            "２０１８-06-08T10:34:56+08:00",
        ] {
            assert_eq!(
                parse_rfc3339(value),
                None,
                "expected rejection of {value:?}"
            );
        }
    }

    #[test]
    fn roundtrips_over_a_wide_date_range() {
        for timestamp in [
            0,
            1,
            1_000_000_000,
            1_528_425_296,
            2_147_483_647,
            4_102_444_800,
        ] {
            let text = format_rfc3339(timestamp);
            assert_eq!(parse_rfc3339(&text), Some(timestamp), "{text}");
        }
    }
}
