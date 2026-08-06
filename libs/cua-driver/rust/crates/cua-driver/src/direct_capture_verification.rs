use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_NAME: &str = "direct-capture-verification.json";
const SCHEMA_VERSION: u8 = 1;
const SOURCE: &str = "permissions_grant";

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct VerificationRecord {
    schema_version: u8,
    source: String,
    verified_at_unix_seconds: u64,
    bundle_id: String,
}

fn state_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(crate::bundle::user_home_subdirectory())
            .join(FILE_NAME),
    )
}

pub fn record() -> Result<(), String> {
    let path = state_path().ok_or_else(|| "user home directory is unavailable".to_owned())?;
    let verified_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("read system time: {error}"))?
        .as_secs();
    record_to_path(&path, crate::bundle::bundle_id(), verified_at_unix_seconds)
}

pub fn status_value() -> Option<serde_json::Value> {
    let path = state_path()?;
    let record = load_from_path(&path, crate::bundle::bundle_id())?;
    Some(serde_json::json!({
        "source": record.source,
        "verified_at": iso8601(record.verified_at_unix_seconds),
        "bundle_id": record.bundle_id,
    }))
}

pub fn clear() -> Result<(), String> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

fn load_from_path(path: &Path, expected_bundle_id: &str) -> Option<VerificationRecord> {
    let content = std::fs::read(path).ok()?;
    let record: VerificationRecord = serde_json::from_slice(&content).ok()?;
    (record.schema_version == SCHEMA_VERSION
        && record.source == SOURCE
        && record.verified_at_unix_seconds > 0
        && record.bundle_id == expected_bundle_id)
        .then_some(record)
}

fn record_to_path(
    path: &Path,
    bundle_id: &str,
    verified_at_unix_seconds: u64,
) -> Result<(), String> {
    if verified_at_unix_seconds == 0 {
        return Err("verification timestamp is unavailable".to_owned());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "direct-capture verification path has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let record = VerificationRecord {
        schema_version: SCHEMA_VERSION,
        source: SOURCE.to_owned(),
        verified_at_unix_seconds,
        bundle_id: bundle_id.to_owned(),
    };
    let mut content = serde_json::to_vec_pretty(&record)
        .map_err(|error| format!("serialize direct-capture verification: {error}"))?;
    content.push(b'\n');
    std::fs::write(path, content).map_err(|error| format!("write {}: {error}", path.display()))
}

fn iso8601(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds_of_day = (unix_seconds % 86_400) as u32;
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = (z - era * 146_097) as u32;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_BUNDLE_ID: &str = "com.trycua.driver";
    const VERIFIED_AT: u64 = 1_754_352_000;

    #[test]
    fn verification_round_trips_with_timestamp_and_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        record_to_path(&path, RELEASE_BUNDLE_ID, VERIFIED_AT).expect("record verification");

        let record = load_from_path(&path, RELEASE_BUNDLE_ID).expect("load verification");
        assert_eq!(record.source, SOURCE);
        assert_eq!(record.verified_at_unix_seconds, VERIFIED_AT);
        assert_eq!(record.bundle_id, RELEASE_BUNDLE_ID);
        assert_eq!(
            iso8601(record.verified_at_unix_seconds),
            "2025-08-05T00:00:00Z"
        );
    }

    #[test]
    fn verification_rejects_a_different_bundle_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        record_to_path(&path, RELEASE_BUNDLE_ID, VERIFIED_AT).expect("record verification");

        assert!(load_from_path(&path, "com.trycua.driver.local").is_none());
    }

    #[test]
    fn verification_rejects_invalid_provenance() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        let invalid_records = [
            serde_json::json!({
                "schema_version": SCHEMA_VERSION + 1,
                "source": SOURCE,
                "verified_at_unix_seconds": VERIFIED_AT,
                "bundle_id": RELEASE_BUNDLE_ID,
            }),
            serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "source": "unknown",
                "verified_at_unix_seconds": VERIFIED_AT,
                "bundle_id": RELEASE_BUNDLE_ID,
            }),
            serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "source": SOURCE,
                "verified_at_unix_seconds": 0,
                "bundle_id": RELEASE_BUNDLE_ID,
            }),
        ];

        for record in invalid_records {
            std::fs::write(
                &path,
                serde_json::to_vec(&record).expect("serialize record"),
            )
            .expect("write invalid record");
            assert!(load_from_path(&path, RELEASE_BUNDLE_ID).is_none());
        }
    }

    #[test]
    fn legacy_marker_is_not_treated_as_provenance() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        std::fs::write(&path, b"permissions_grant\n").expect("write legacy marker");
        assert!(load_from_path(&path, RELEASE_BUNDLE_ID).is_none());
    }
}
