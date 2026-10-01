// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Small helpers every module shares: a deterministic string collation (so
//! lists sort the same in Rust, Swift and the webview) and an RFC 3339
//! timestamp parser (no clock or time zone database needed).

use std::cmp::Ordering;

/// Collation used for every user-visible sort. Case-insensitive first, then
/// lowercase before uppercase, then byte order: the same order the webview's
/// `localeCompare` gives for the ASCII names Spaces carry, but identical on
/// every host.
pub fn collate(a: &str, b: &str) -> Ordering {
    let fold = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<String>();
    fold(a)
        .cmp(&fold(b))
        .then_with(|| {
            // Lowercase first on a case-only tie ("ada" < "Ada").
            let rank = |s: &str| {
                s.chars()
                    .map(|c| if c.is_uppercase() { 1u8 } else { 0 })
                    .collect::<Vec<_>>()
            };
            rank(a).cmp(&rank(b))
        })
        .then_with(|| a.cmp(b))
}

/// Milliseconds since the Unix epoch for an RFC 3339 timestamp
/// (`2026-08-30T12:00:00Z`, fractional seconds and `+hh:mm` offsets
/// accepted). `None` when it does not parse.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        let part = s.get(from..to)?;
        if part.bytes().all(|c| c.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    if b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let hour = num(11, 13)?;
    let minute = num(14, 16)?;
    let second = num(17, 19)?;
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &s[19..];
    let mut millis = 0i64;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        let padded = format!("{digits:0<3}");
        millis = padded[..3].parse().ok()?;
        rest = &frac[digits.len()..];
    }
    let offset_min = match rest {
        "Z" | "z" => 0,
        "" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let off = &rest[1..];
            if off.len() != 5 || off.as_bytes()[2] != b':' {
                return None;
            }
            let h: i64 = off[..2].parse().ok()?;
            let m: i64 = off[3..].parse().ok()?;
            sign * (h * 60 + m)
        }
    };
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_min * 60;
    Some(secs * 1_000 + millis)
}

/// JavaScript's `Number.prototype.toFixed`: ties round up (away from zero
/// for positives), so every shell prints the same digits.
pub fn to_fixed(v: f64, digits: u32) -> String {
    let scale = 10f64.powi(digits as i32);
    let rounded = (v * scale).round() / scale;
    format!("{rounded:.*}", digits as usize)
}

/// `word` appears in `hay` with non-word characters (or the ends) around it,
/// like a regex `\bword\b`.
pub fn contains_word(hay: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    hay.match_indices(word).any(|(i, m)| {
        hay[..i].chars().next_back().is_none_or(|c| !is_word(c))
            && hay[i + m.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_word(c))
    })
}

/// Days since 1970-01-01 (Howard Hinnant's civil-from-days inverse).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_matches_known_epochs() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2026-08-30T12:00:00Z"),
            Some(1_788_091_200_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-08-30T14:00:00.250+02:00"),
            Some(1_788_091_200_250)
        );
        assert_eq!(parse_rfc3339_ms("yesterday"), None);
        assert_eq!(parse_rfc3339_ms("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn collation_is_case_insensitive_then_lowercase_first() {
        let mut v = vec!["beta", "Alpha", "alpha", "Gamma"];
        v.sort_by(|a, b| collate(a, b));
        assert_eq!(v, ["alpha", "Alpha", "beta", "Gamma"]);
    }
}
