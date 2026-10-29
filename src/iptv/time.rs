//! Calendar arithmetic for guide times, without a date library.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Unix seconds as UTC calendar fields.
pub fn civil(t: i64) -> Civil {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    Civil { year, month, day, hour: (secs / 3600) as u32, minute: (secs % 3600 / 60) as u32, second: (secs % 60) as u32 }
}

/// UTC calendar fields as Unix seconds.
pub fn unix(c: Civil) -> i64 {
    let y = c.year - i64::from(c.month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(c.month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(c.day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + i64::from(c.hour) * 3600 + i64::from(c.minute) * 60 + i64::from(c.second)
}

/// An XMLTV time, `20261029200000 +0100` (the offset and seconds are optional).
pub fn parse_xmltv(s: &str) -> Option<i64> {
    let s = s.trim();
    let (stamp, zone) = s.split_once(' ').map_or((s, ""), |(a, b)| (a, b.trim()));
    let digits = |r: std::ops::Range<usize>| stamp.get(r).and_then(|d| d.parse::<u32>().ok());
    let c = Civil {
        year: i64::from(digits(0..4)?),
        month: digits(4..6)?,
        day: digits(6..8)?,
        hour: digits(8..10).unwrap_or(0),
        minute: digits(10..12).unwrap_or(0),
        second: digits(12..14).unwrap_or(0),
    };
    if !(1..=12).contains(&c.month) || !(1..=31).contains(&c.day) {
        return None;
    }
    Some(unix(c) - offset(zone).unwrap_or(0))
}

/// `+0100` / `-0530` as seconds east of UTC.
fn offset(zone: &str) -> Option<i64> {
    let (sign, rest) = match zone.as_bytes().first()? {
        b'+' => (1, &zone[1..]),
        b'-' => (-1, &zone[1..]),
        _ => return None,
    };
    let rest = rest.replace(':', "");
    let h: i64 = rest.get(0..2)?.parse().ok()?;
    let m: i64 = rest.get(2..4).and_then(|m| m.parse().ok()).unwrap_or(0);
    Some(sign * (h * 3600 + m * 60))
}

/// `2026-10-29 21:00:00` (a server's wall clock) as if it were UTC.
pub fn parse_plain(s: &str) -> Option<i64> {
    let d: String = s.chars().filter(char::is_ascii_digit).collect();
    parse_xmltv(&d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for t in [0, 951_782_400, 1_793_304_000, 4_102_444_799, -86_400] {
            assert_eq!(unix(civil(t)), t);
        }
        assert_eq!(civil(1_793_304_000), Civil { year: 2026, month: 10, day: 29, hour: 20, minute: 0, second: 0 });
        assert_eq!(civil(951_782_400), Civil { year: 2000, month: 2, day: 29, hour: 0, minute: 0, second: 0 });
    }

    #[test]
    fn parses_xmltv_times() {
        assert_eq!(parse_xmltv("20261029200000 +0000"), Some(1_793_304_000));
        assert_eq!(parse_xmltv("20261029210000 +0100"), Some(1_793_304_000));
        assert_eq!(parse_xmltv("20261029150000 -05:00"), Some(1_793_304_000));
        assert_eq!(parse_xmltv("202610292000"), Some(1_793_304_000));
        assert_eq!(parse_xmltv("garbage"), None);
        assert_eq!(parse_plain("2026-10-29 20:00:00"), Some(1_793_304_000));
    }
}
