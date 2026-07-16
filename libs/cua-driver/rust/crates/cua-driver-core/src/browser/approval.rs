//! Explicit approval evidence for mutating browser preparation.
//!
//! MCP-host approval is carried on an internal transport marker. Direct CLI
//! and raw callers instead use a short-lived, single-use artifact minted by
//! the interactive `browser-approve` command. Public tool arguments never
//! treat a Boolean as consent.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::platform::{PrepareProfile, PrepareProfileMode};
use super::refusal::{BrowserRefusal, BrowserRefusalCode};

pub const MCP_HOST_APPROVAL_ARG: &str = "_cua_browser_prepare_mcp_host_approved";
pub const MAX_APPROVAL_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Serialize, Deserialize)]
struct ApprovalArtifact {
    schema: String,
    token: String,
    pid: i64,
    profile: PrepareProfile,
    expires_unix_ms: u128,
}

fn refusal(message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(BrowserRefusalCode::BrowserConsentRequired, message)
}

fn now_unix_ms() -> Result<u128, BrowserRefusal> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .map_err(|_| refusal("the system clock cannot validate browser preparation approval"))
}

fn approval_root() -> PathBuf {
    std::env::temp_dir().join("cua-driver-browser-approvals-v1")
}

fn artifact_path(token: &str) -> Result<PathBuf, BrowserRefusal> {
    let parsed = Uuid::parse_str(token)
        .map_err(|_| refusal("browser preparation approval token is malformed or expired"))?;
    Ok(approval_root().join(format!("{parsed}.json")))
}

pub fn validate_profile(profile: &PrepareProfile) -> Result<(), BrowserRefusal> {
    match profile.mode {
        PrepareProfileMode::IsolatedNew => {
            if profile.name.is_some() {
                return Err(refusal("profile.name is not valid with mode=isolated_new"));
            }
        }
        PrepareProfileMode::IsolatedNamed => {
            let Some(name) = profile.name.as_deref() else {
                return Err(refusal("profile.name is required with mode=isolated_named"));
            };
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(refusal(
                    "profile.name must be 1-64 ASCII letters, digits, '-' or '_'",
                ));
            }
        }
    }
    Ok(())
}

/// Mint approval evidence after an interactive caller has confirmed the exact
/// operation. The returned token is the only public value passed to the tool.
pub fn mint_prepare_approval(pid: i64, profile: PrepareProfile) -> Result<String, BrowserRefusal> {
    validate_profile(&profile)?;
    let token = Uuid::new_v4().to_string();
    let root = approval_root();
    fs::create_dir_all(&root)
        .map_err(|error| refusal(format!("could not create approval directory: {error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .map_err(|error| refusal(format!("could not restrict approval directory: {error}")))?;
    }

    let artifact = ApprovalArtifact {
        schema: "cua-browser-prepare-approval-v1".to_owned(),
        token: token.clone(),
        pid,
        profile,
        expires_unix_ms: now_unix_ms()? + MAX_APPROVAL_TTL.as_millis(),
    };
    let path = artifact_path(&token)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| refusal(format!("could not create approval artifact: {error}")))?;
    let bytes = serde_json::to_vec(&artifact)
        .map_err(|error| refusal(format!("could not encode approval artifact: {error}")))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| refusal(format!("could not persist approval artifact: {error}")))?;
    Ok(token)
}

/// Consume approval evidence. The artifact is removed before any semantic
/// validation so malformed, mismatched, and expired attempts are single-use.
pub fn consume_prepare_approval(
    token: &str,
    pid: i64,
    profile: &PrepareProfile,
) -> Result<(), BrowserRefusal> {
    validate_profile(profile)?;
    let path = artifact_path(token)?;
    let bytes = fs::read(&path)
        .map_err(|_| refusal("browser preparation approval token is missing or expired"))?;
    fs::remove_file(&path)
        .map_err(|error| refusal(format!("could not consume approval artifact: {error}")))?;
    let artifact: ApprovalArtifact = serde_json::from_slice(&bytes)
        .map_err(|_| refusal("browser preparation approval artifact is invalid"))?;
    if artifact.schema != "cua-browser-prepare-approval-v1"
        || artifact.token != token
        || artifact.pid != pid
        || artifact.profile != *profile
        || artifact.expires_unix_ms < now_unix_ms()?
    {
        return Err(refusal(
            "browser preparation approval does not match this pid/profile request or has expired",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> PrepareProfile {
        PrepareProfile {
            mode: PrepareProfileMode::IsolatedNamed,
            name: Some(name.to_owned()),
        }
    }

    #[test]
    fn approval_is_bound_and_single_use() {
        let profile = named("approval-test");
        let token = mint_prepare_approval(42, profile.clone()).unwrap();
        consume_prepare_approval(&token, 42, &profile).unwrap();
        assert!(consume_prepare_approval(&token, 42, &profile).is_err());
    }

    #[test]
    fn mismatch_consumes_the_artifact() {
        let profile = named("approval-mismatch");
        let token = mint_prepare_approval(42, profile.clone()).unwrap();
        assert!(consume_prepare_approval(&token, 7, &profile).is_err());
        assert!(consume_prepare_approval(&token, 42, &profile).is_err());
    }

    #[test]
    fn named_profile_is_path_safe() {
        for name in ["", "../Default", "a/b", "with space", "."] {
            assert!(validate_profile(&named(name)).is_err(), "accepted {name:?}");
        }
        validate_profile(&named("research_2026-07")).unwrap();
    }
}
