//! Trusted standalone-daemon assembly for the macOS Computer History preview.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};

#[cfg(target_os = "macos")]
use cua_driver_core::history::{
    HistoryConfig, HistoryManager, DEFAULT_QUOTA_BYTES, DEFAULT_RETENTION_DAYS,
};
use cua_driver_core::{history::HISTORY_DIR_ENV, tool::ToolRegistry};

static HISTORY_ADMITTED: AtomicBool = AtomicBool::new(false);
static DAEMON_LAUNCH_STATE: OnceLock<Mutex<DaemonLaunchState>> = OnceLock::new();
const RELEASE_TEAM_IDENTIFIER: &str = "YCK386LBJ7";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DaemonLaunchState {
    pub claude_code_compat: bool,
    pub grants: Vec<String>,
}

pub fn configure_admission(admitted: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    if admitted {
        verify_installed_app_for_history()?;
    }
    HISTORY_ADMITTED.store(admitted, Ordering::Release);
    Ok(())
}

pub fn configure_daemon_launch_state(claude_code_compat: bool, grants: &[String]) {
    let state = DAEMON_LAUNCH_STATE.get_or_init(|| Mutex::new(DaemonLaunchState::default()));
    *state.lock().unwrap() = DaemonLaunchState {
        claude_code_compat,
        grants: grants.to_vec(),
    };
}

pub fn daemon_launch_state() -> DaemonLaunchState {
    DAEMON_LAUNCH_STATE
        .get_or_init(|| Mutex::new(DaemonLaunchState::default()))
        .lock()
        .unwrap()
        .clone()
}

pub fn preview_admitted_preference() -> bool {
    preview_admitted_preference_at(&admission_preference_path())
}

fn preview_admitted_preference_at(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    if bytes.len() > 1024 {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .get("history_preview_admitted")
                .and_then(|value| value.as_bool())
        })
        == Some(true)
}

pub fn set_preview_admitted_preference(admitted: bool) -> anyhow::Result<()> {
    set_preview_admitted_preference_at(&admission_preference_path(), admitted)
}

fn set_preview_admitted_preference_at(path: &Path, admitted: bool) -> anyhow::Result<()> {
    let root = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("admission preference has no parent"))?;
    fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    }
    let temporary = root.join("admission.json.tmp");
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(
        serde_json::json!({"history_preview_admitted": admitted})
            .to_string()
            .as_bytes(),
    )?;
    file.sync_data()?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn run_offline_purge_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() != ["history", "purge-offline", "--yes"] {
        return None;
    }
    let expected =
        PathBuf::from(crate::bundle::app_bundle_path()).join("Contents/MacOS/cua-driver");
    let current = std::env::current_exe().ok();
    if current
        .as_deref()
        .is_none_or(|current| !is_exact_packaged_helper(current, &expected))
        || verify_installed_app_for_history().is_err()
    {
        eprintln!(
            "history_purge_incomplete: purge requires the exact verified installed CuaDriver.app helper"
        );
        return Some(1);
    }
    let provider = platform_macos::history::MacosKeychainKeyProvider::default();
    match cua_driver_core::history::purge_offline(
        &history_root(),
        crate::bundle::state_namespace(),
        &provider,
    ) {
        Ok(result) => {
            println!(
                "history purge complete: destroyed {} key(s), removed {} file(s)",
                result.destroyed_keys, result.removed_files
            );
            Some(0)
        }
        Err(error) => {
            eprintln!("history_purge_incomplete: {:?}", error.category);
            Some(1)
        }
    }
}

fn is_exact_packaged_helper(current: &Path, expected: &Path) -> bool {
    fs::canonicalize(current).ok() == fs::canonicalize(expected).ok()
        && fs::canonicalize(expected).is_ok()
}

#[cfg(target_os = "macos")]
pub(crate) fn verify_history_cli_executable_path(path: &Path) -> anyhow::Result<()> {
    let expected = PathBuf::from(crate::bundle::app_bundle_path())
        .join("Contents/MacOS")
        .join(crate::bundle::cli_name());
    if !is_exact_packaged_helper(path, &expected) {
        anyhow::bail!("history control peer is not the exact installed Cua Driver helper");
    }
    verify_installed_app_for_history()
}

#[cfg(target_os = "macos")]
pub fn verify_installed_app_for_history() -> anyhow::Result<()> {
    use std::process::{Command, Stdio};

    let path = crate::bundle::app_bundle_path();
    let metadata = fs::metadata(&path)?;
    if !metadata.is_dir() {
        anyhow::bail!("installed Cua Driver app path is not a directory");
    }
    let verified = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", &path])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !verified.success() {
        anyhow::bail!("installed Cua Driver app failed strict code-signature verification");
    }
    let expected_helper = PathBuf::from(&path)
        .join("Contents/MacOS")
        .join(crate::bundle::cli_name());
    let current_helper = std::env::current_exe().map_err(|error| {
        anyhow::anyhow!("current Cua Driver executable is unavailable: {error}")
    })?;
    if !is_exact_packaged_helper(&current_helper, &expected_helper) {
        anyhow::bail!(
            "history admission requires the exact executable inside the verified installed app"
        );
    }
    let helper = expected_helper.to_string_lossy();
    let helper_verified = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", helper.as_ref()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !helper_verified.success() {
        anyhow::bail!("installed Cua Driver helper failed strict code-signature verification");
    }
    let detail = Command::new("/usr/bin/codesign")
        .args(["-d", "--verbose=4", helper.as_ref()])
        .output()?;
    if !detail.status.success() {
        anyhow::bail!("installed Cua Driver signing identity could not be inspected");
    }
    let stderr = String::from_utf8_lossy(&detail.stderr);
    let requirement = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-", helper.as_ref()])
        .output()?;
    if !requirement.status.success() {
        anyhow::bail!("installed Cua Driver designated requirement could not be inspected");
    }
    // Unlike `codesign -d --verbose`, `codesign -d -r-` writes the
    // designated requirement itself to stdout (and a short executable note to
    // stderr). Read the semantic payload from stdout so a valid
    // certificate-backed installation is not misclassified as ad-hoc.
    let requirement = String::from_utf8_lossy(&requirement.stdout);
    let team_identifier = stderr
        .lines()
        .find_map(|line| line.trim().strip_prefix("TeamIdentifier="))
        .filter(|value| !value.is_empty() && *value != "not set")
        .ok_or_else(|| anyhow::anyhow!("installed Cua Driver signature has no team identifier"))?;
    let entitlements = Command::new("/usr/bin/codesign")
        .args(["-d", "--entitlements", "-", "--xml", helper.as_ref()])
        .output()?;
    if !entitlements.status.success() {
        anyhow::bail!("installed Cua Driver entitlements could not be inspected");
    }
    let compact: String = String::from_utf8_lossy(&entitlements.stdout)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    validate_history_app_signature(
        &stderr,
        &requirement,
        team_identifier,
        &compact,
        crate::bundle::bundle_id(),
        crate::bundle::state_namespace() == "cua-driver",
    )
}

fn validate_history_app_signature(
    detail: &str,
    requirement: &str,
    team_identifier: &str,
    compact_entitlements: &str,
    bundle_id: &str,
    require_release_entitlements: bool,
) -> anyhow::Result<()> {
    let expected = format!("Identifier={bundle_id}");
    if !detail.lines().any(|line| line.trim() == expected) {
        anyhow::bail!(
            "installed Cua Driver signing identifier does not match the selected namespace"
        );
    }
    if !requirement.contains("certificate leaf") {
        anyhow::bail!("installed Cua Driver signature is not certificate-backed");
    }
    if team_identifier.is_empty() || team_identifier == "not set" {
        anyhow::bail!("installed Cua Driver signature has no team identifier");
    }
    if !require_release_entitlements {
        return Ok(());
    }
    if team_identifier != RELEASE_TEAM_IDENTIFIER {
        anyhow::bail!("installed Cua Driver signature does not match the release signing team");
    }
    if !requirement.contains("anchor apple generic") {
        anyhow::bail!(
            "installed Cua Driver signature is not anchored to Apple's code-signing trust chain"
        );
    }
    let application_identifier = format!("{team_identifier}.{bundle_id}");
    let application_entitlement = format!(
        "<key>com.apple.application-identifier</key><string>{application_identifier}</string>"
    );
    let keychain_entitlement = format!(
        "<key>keychain-access-groups</key><array><string>{application_identifier}</string></array>"
    );
    if !compact_entitlements.contains(&application_entitlement)
        || !compact_entitlements.contains(&keychain_entitlement)
    {
        anyhow::bail!(
            "installed Cua Driver lacks the device-protected Keychain entitlements required by Computer History"
        );
    }
    Ok(())
}

fn admission_preference_path() -> PathBuf {
    history_root().join("admission.json")
}

#[cfg(target_os = "macos")]
pub fn register_into(registry: &mut ToolRegistry) {
    if !HISTORY_ADMITTED.load(Ordering::Acquire) {
        return;
    }
    let manager = HistoryManager::new(
        HistoryConfig {
            root: history_root(),
            namespace: crate::bundle::state_namespace().to_owned(),
            admitted: true,
            platform: "macos".to_owned(),
            retention_days: DEFAULT_RETENTION_DAYS,
            quota_bytes: DEFAULT_QUOTA_BYTES,
        },
        platform_macos::history::MacosKeychainKeyProvider::shared(),
        Some(platform_macos::history::application_identity_provider()),
    );
    registry.register_history_tools(manager);
}

#[cfg(not(target_os = "macos"))]
pub fn register_into(_registry: &mut ToolRegistry) {}

pub fn register_host_tools(registry: &mut ToolRegistry) {
    crate::check_update_tool::register_into(registry);
    register_into(registry);
}

pub fn history_root() -> PathBuf {
    if let Some(path) = std::env::var_os(HISTORY_DIR_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library")
        .join("Application Support")
        .join(crate::bundle::state_namespace())
        .join("computer-history")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_root_is_namespace_specific() {
        let root = history_root();
        assert!(root.ends_with(format!(
            "{}/computer-history",
            crate::bundle::state_namespace()
        )));
    }

    #[test]
    fn admission_preference_roundtrips_and_rejects_malformed_state() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("admission.json");
        assert!(!preview_admitted_preference_at(&path));
        set_preview_admitted_preference_at(&path, true).unwrap();
        assert!(preview_admitted_preference_at(&path));
        fs::write(&path, b"not-json").unwrap();
        assert!(!preview_admitted_preference_at(&path));
    }

    #[test]
    fn offline_purge_identity_guard_requires_the_exact_packaged_helper() {
        let temp = tempfile::tempdir().unwrap();
        let expected = temp.path().join("CuaDriver.app/Contents/MacOS/cua-driver");
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        fs::write(&expected, b"synthetic").unwrap();
        assert!(is_exact_packaged_helper(&expected, &expected));
        let source = temp.path().join("target/debug/cua-driver");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"synthetic").unwrap();
        assert!(!is_exact_packaged_helper(&source, &expected));
    }

    #[test]
    fn local_history_accepts_exact_certificate_identity_without_release_entitlements() {
        validate_history_app_signature(
            "Identifier=com.trycua.driver.local\nTeamIdentifier=TEAM123",
            "designated => identifier \"com.trycua.driver.local\" and certificate leaf[subject.OU] = TEAM123",
            "TEAM123",
            "",
            "com.trycua.driver.local",
            false,
        )
        .unwrap();
    }

    #[test]
    fn history_admission_rejects_adhoc_or_wrong_bundle_identity() {
        let adhoc = validate_history_app_signature(
            "Identifier=com.trycua.driver.local\nTeamIdentifier=TEAM123",
            "designated => cdhash H\"1234\"",
            "TEAM123",
            "",
            "com.trycua.driver.local",
            false,
        );
        assert!(adhoc.is_err());
        let wrong_bundle = validate_history_app_signature(
            "Identifier=com.trycua.driver\nTeamIdentifier=TEAM123",
            "designated => identifier \"com.trycua.driver\" and certificate leaf[subject.OU] = TEAM123",
            "TEAM123",
            "",
            "com.trycua.driver.local",
            false,
        );
        assert!(wrong_bundle.is_err());
    }

    #[test]
    fn release_history_still_requires_device_protected_keychain_entitlements() {
        let detail =
            format!("Identifier=com.trycua.driver\nTeamIdentifier={RELEASE_TEAM_IDENTIFIER}");
        let requirement = format!(
            "designated => anchor apple generic and identifier \"com.trycua.driver\" and certificate leaf[subject.OU] = {RELEASE_TEAM_IDENTIFIER}"
        );
        assert!(validate_history_app_signature(
            &detail,
            &requirement,
            RELEASE_TEAM_IDENTIFIER,
            "",
            "com.trycua.driver",
            true,
        )
        .is_err());
        let entitlements = format!(
            "<key>com.apple.application-identifier</key><string>{RELEASE_TEAM_IDENTIFIER}.com.trycua.driver</string><key>keychain-access-groups</key><array><string>{RELEASE_TEAM_IDENTIFIER}.com.trycua.driver</string></array>"
        );
        validate_history_app_signature(
            &detail,
            &requirement,
            RELEASE_TEAM_IDENTIFIER,
            &entitlements,
            "com.trycua.driver",
            true,
        )
        .unwrap();

        assert!(validate_history_app_signature(
            "Identifier=com.trycua.driver\nTeamIdentifier=OTHERTEAM",
            "designated => identifier \"com.trycua.driver\" and certificate leaf[subject.OU] = OTHERTEAM",
            "OTHERTEAM",
            "<key>com.apple.application-identifier</key><string>OTHERTEAM.com.trycua.driver</string><key>keychain-access-groups</key><array><string>OTHERTEAM.com.trycua.driver</string></array>",
            "com.trycua.driver",
            true,
        )
        .is_err());
    }
}
