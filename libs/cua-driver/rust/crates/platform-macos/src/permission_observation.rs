use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u8 = 1;
const SOURCE: &str = "permissions_grant";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DirectCaptureVerification {
    pub source: String,
    pub verified_at: String,
    pub bundle_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedVerification {
    schema_version: u8,
    source: String,
    verified_at_unix_seconds: i64,
    bundle_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectCaptureEvidenceStore {
    path: PathBuf,
    bundle_id: String,
}

impl DirectCaptureEvidenceStore {
    pub(crate) fn new(path: PathBuf, bundle_id: impl Into<String>) -> Self {
        Self {
            path,
            bundle_id: bundle_id.into(),
        }
    }

    pub(crate) fn record_now(&self) -> Result<DirectCaptureVerification, String> {
        self.record_at(time::OffsetDateTime::now_utc().unix_timestamp())?;
        self.load()
            .ok_or_else(|| "recorded direct-capture verification did not validate".to_owned())
    }

    pub(crate) fn load(&self) -> Option<DirectCaptureVerification> {
        let bytes = std::fs::read(&self.path).ok()?;
        let record: PersistedVerification = serde_json::from_slice(&bytes).ok()?;
        if record.schema_version != SCHEMA_VERSION
            || record.source != SOURCE
            || record.verified_at_unix_seconds <= 0
            || record.bundle_id != self.bundle_id
        {
            return None;
        }
        let verified_at =
            time::OffsetDateTime::from_unix_timestamp(record.verified_at_unix_seconds)
                .ok()?
                .format(&time::format_description::well_known::Rfc3339)
                .ok()?;
        Some(DirectCaptureVerification {
            source: record.source,
            verified_at,
            bundle_id: record.bundle_id,
        })
    }

    pub(crate) fn clear(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("remove {}: {error}", self.path.display())),
        }
    }

    fn record_at(&self, verified_at_unix_seconds: i64) -> Result<(), String> {
        if verified_at_unix_seconds <= 0 {
            return Err("verification timestamp is unavailable".to_owned());
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "direct-capture verification path has no parent".to_owned())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
        let record = PersistedVerification {
            schema_version: SCHEMA_VERSION,
            source: SOURCE.to_owned(),
            verified_at_unix_seconds,
            bundle_id: self.bundle_id.clone(),
        };
        write_json_atomic(&self.path, &record)
    }
}

fn write_json_atomic(path: &Path, value: &PersistedVerification) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        let mut bytes = serde_json::to_vec_pretty(value)
            .map_err(|error| format!("serialize direct-capture verification: {error}"))?;
        bytes.push(b'\n');
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("replace {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_BUNDLE_ID: &str = "com.trycua.driver";
    const VERIFIED_AT: i64 = 1_754_352_000;

    fn store(path: PathBuf) -> DirectCaptureEvidenceStore {
        DirectCaptureEvidenceStore::new(path, RELEASE_BUNDLE_ID)
    }

    #[test]
    fn verification_round_trips_with_honest_consent_provenance() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(temp.path().join("verification.json"));
        store.record_at(VERIFIED_AT).expect("record verification");

        assert_eq!(
            store.load(),
            Some(DirectCaptureVerification {
                source: SOURCE.to_owned(),
                verified_at: "2025-08-05T00:00:00Z".to_owned(),
                bundle_id: RELEASE_BUNDLE_ID.to_owned(),
            })
        );
    }

    #[test]
    fn verification_rejects_wrong_identity_schema_source_and_timestamp() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("verification.json");
        let store = store(path.clone());
        let valid = PersistedVerification {
            schema_version: SCHEMA_VERSION,
            source: SOURCE.to_owned(),
            verified_at_unix_seconds: VERIFIED_AT,
            bundle_id: RELEASE_BUNDLE_ID.to_owned(),
        };
        let invalid_records = [
            PersistedVerification {
                schema_version: SCHEMA_VERSION + 1,
                ..valid.clone()
            },
            PersistedVerification {
                source: "unknown".to_owned(),
                ..valid.clone()
            },
            PersistedVerification {
                verified_at_unix_seconds: 0,
                ..valid.clone()
            },
            PersistedVerification {
                bundle_id: "com.trycua.driver.local".to_owned(),
                ..valid
            },
        ];

        for record in invalid_records {
            std::fs::write(
                &path,
                serde_json::to_vec(&record).expect("serialize record"),
            )
            .expect("write invalid record");
            assert_eq!(store.load(), None);
        }
    }

    #[test]
    fn clear_removes_existing_evidence_and_tolerates_absence() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = store(temp.path().join("verification.json"));
        store.record_at(VERIFIED_AT).expect("record verification");

        store.clear().expect("clear verification");
        store.clear().expect("clear absent verification");
        assert_eq!(store.load(), None);
    }
}
