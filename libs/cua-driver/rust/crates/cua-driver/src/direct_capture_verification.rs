use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 1;
const FILE_NAME: &str = "direct-capture-verification.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectCaptureVerification {
    pub schema_version: u32,
    pub status: String,
    pub verified_at: String,
    pub bundle_id: String,
    pub driver_version: String,
    pub source: String,
}

impl DirectCaptureVerification {
    fn new(bundle_id: &str, driver_version: &str, verified_at: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            status: "verified".to_owned(),
            verified_at,
            bundle_id: bundle_id.to_owned(),
            driver_version: driver_version.to_owned(),
            source: "permissions_grant".to_owned(),
        }
    }

    fn matches_identity(&self, bundle_id: &str) -> bool {
        self.schema_version == SCHEMA_VERSION
            && self.status == "verified"
            && self.source == "permissions_grant"
            && self.bundle_id == bundle_id
            && !self.verified_at.is_empty()
    }
}

fn state_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(crate::bundle::user_home_subdirectory())
            .join(FILE_NAME),
    )
}

pub fn record_now() -> Result<DirectCaptureVerification, String> {
    let path = state_path().ok_or_else(|| "user home directory is unavailable".to_owned())?;
    let verified_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| format!("format verification time: {error}"))?;
    let verification = DirectCaptureVerification::new(
        crate::bundle::bundle_id(),
        env!("CARGO_PKG_VERSION"),
        verified_at,
    );
    persist_to_path(&path, &verification)?;
    Ok(verification)
}

pub fn load_for_current_identity() -> Option<DirectCaptureVerification> {
    let path = state_path()?;
    load_from_path(&path, crate::bundle::bundle_id())
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

fn load_from_path(path: &Path, bundle_id: &str) -> Option<DirectCaptureVerification> {
    let bytes = std::fs::read(path).ok()?;
    let verification: DirectCaptureVerification = serde_json::from_slice(&bytes).ok()?;
    verification
        .matches_identity(bundle_id)
        .then_some(verification)
}

fn persist_to_path(path: &Path, verification: &DirectCaptureVerification) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "direct-capture verification path has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("secure {}: {error}", parent.display()))?;
    }

    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        let bytes = serde_json::to_vec_pretty(verification)
            .map_err(|error| format!("serialize direct-capture verification: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(path)
                .map_err(|error| format!("replace {}: {error}", path.display()))?;
        }
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

    fn verification(bundle_id: &str) -> DirectCaptureVerification {
        DirectCaptureVerification::new(bundle_id, "0.18.0", "2026-08-06T12:34:56Z".to_owned())
    }

    #[test]
    fn verification_round_trips_for_the_same_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        let expected = verification("com.trycua.driver.local");

        persist_to_path(&path, &expected).expect("persist verification");

        assert_eq!(
            load_from_path(&path, "com.trycua.driver.local"),
            Some(expected)
        );
    }

    #[test]
    fn verification_does_not_cross_product_identities() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        persist_to_path(&path, &verification("com.trycua.driver.local"))
            .expect("persist verification");

        assert_eq!(load_from_path(&path, "com.trycua.driver"), None);
    }

    #[test]
    fn newer_verification_replaces_the_previous_observation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        let mut latest = verification("com.trycua.driver");
        persist_to_path(&path, &latest).expect("persist first verification");
        latest.verified_at = "2026-08-06T13:45:00Z".to_owned();

        persist_to_path(&path, &latest).expect("replace verification");

        assert_eq!(load_from_path(&path, "com.trycua.driver"), Some(latest));
    }

    #[test]
    fn malformed_or_incomplete_verification_fails_closed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        std::fs::write(&path, b"not json").expect("write malformed record");
        assert_eq!(load_from_path(&path, "com.trycua.driver"), None);

        let mut incomplete = verification("com.trycua.driver");
        incomplete.verified_at.clear();
        persist_to_path(&path, &incomplete).expect("persist incomplete record");
        assert_eq!(load_from_path(&path, "com.trycua.driver"), None);
    }
}
