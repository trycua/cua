//! Wire timestamps shared by every platform adapter.

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// Format Unix epoch seconds as RFC 3339 UTC with whole seconds:
/// `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Returns `None` when the instant is outside the range RFC 3339 can
/// represent (years 0000 through 9999).
pub fn unix_secs_to_rfc3339(secs: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::unix_secs_to_rfc3339;

    #[test]
    fn formats_whole_second_utc_timestamps() {
        for (secs, expected) in [
            (0, "1970-01-01T00:00:00Z"),
            // Pre-epoch.
            (-1, "1969-12-31T23:59:59Z"),
            (-365 * 86_400, "1969-01-01T00:00:00Z"),
            // Leap day, and the day after Feb 28 in a non-leap year.
            (1_582_977_600, "2020-02-29T12:00:00Z"),
            (1_551_312_000, "2019-02-28T00:00:00Z"),
            (1_551_312_000 + 86_400, "2019-03-01T00:00:00Z"),
            // Year wrap.
            (1_704_067_199, "2023-12-31T23:59:59Z"),
            (1_704_067_200, "2024-01-01T00:00:00Z"),
            (1_718_459_130, "2024-06-15T13:45:30Z"),
            (647_105_400, "1990-07-04T15:30:00Z"),
            (1_234_567_890, "2009-02-13T23:31:30Z"),
        ] {
            assert_eq!(unix_secs_to_rfc3339(secs).as_deref(), Some(expected));
        }
        assert_eq!(unix_secs_to_rfc3339(i64::MAX), None);
    }
}
