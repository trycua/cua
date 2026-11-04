// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teleport an app's session (Firefox profile, ...) into the Space, through
//! the Spaces tools `teleport_manifest` and `teleport_app`.
//!
//! Consent is a value, not a flag: the manifest lists what would move, an
//! approver (the app's dialog, or the scenario's explicit callback) answers
//! with a [`Decision`], and only then is the teleport requested, naming the
//! approved items and whether the human acknowledged the sensitive ones.
//!
//! Session teleport ships with Cua Spaces (source-available). This app runs
//! the MIT Spaces runtime in process, where no teleport extension is
//! registered, so both calls fail with `HostCapabilityMissing` and a message
//! that says so ([`ships_with_cua_spaces`] recognizes it). With the Cua
//! Spaces extensions registered on the runtime the same calls move the
//! session.

use crate::{Core, Error, Result};
use cua_spaces::Space;
use serde_json::json;

/// One item a teleport would move.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct TeleportItem {
    /// Bundle-relative path (what a [`Decision`] names).
    pub relative_path: String,
    pub label: String,
    pub estimated_bytes: u64,
    /// Credentials, cookies, tokens, transcripts.
    pub is_sensitive: bool,
    /// In the provider's default selection.
    pub is_checked_by_default: bool,
}

/// Exactly what would leave this machine (the `teleport_manifest` result).
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct TeleportManifest {
    pub app: String,
    pub display_name: String,
    /// `full` or `tabs`.
    #[serde(default)]
    pub scope: String,
    pub items: Vec<TeleportItem>,
    #[serde(default)]
    pub total_estimated_bytes: u64,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// What a teleport moved.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct TeleportReceipt {
    pub app: String,
    pub space: String,
    pub transferred_paths: Vec<String>,
    pub bundle_bytes: u64,
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
}

/// The approver's answer.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct Decision {
    /// Exact items (`relative_path`) to send; `None` means the provider's
    /// default-checked set.
    pub include: Option<Vec<String>>,
    /// The human saw and accepted the sensitive items (cookies, logins).
    pub acknowledge_sensitive: bool,
}

/// What a teleport would move, for the approval dialog.
pub async fn manifest(core: &Core, app: &str, scope: Option<&str>) -> Result<TeleportManifest> {
    let v = core
        .spaces()
        .call_extension(
            "teleport",
            "teleport_manifest",
            json!({"app": app, "scope": scope}),
        )
        .await?;
    Ok(serde_json::from_value(v)?)
}

/// Asks `approve` about the manifest and teleports what it approved.
/// `None` from the approver cancels with `TeleportRefused`.
pub async fn teleport(
    core: &Core,
    space: &Space,
    app: &str,
    scope: Option<&str>,
    approve: impl FnOnce(&TeleportManifest) -> Option<Decision>,
) -> Result<TeleportReceipt> {
    let m = manifest(core, app, scope).await?;
    let decision = approve(&m).ok_or_else(|| {
        Error::Spaces(cua_spaces::Error::TeleportRefused(
            "the approver declined".into(),
        ))
    })?;
    // #region docs:rs-teleport
    let v = core
        .spaces()
        .call_extension(
            "teleport",
            "teleport.send",
            json!({
                "space": space.id().to_string(),
                "app": m.app,
                "scope": m.scope,
                "include": decision.include,
                "acknowledge_sensitive": decision.acknowledge_sensitive,
            }),
        )
        .await?;
    Ok(serde_json::from_value(v)?)
    // #endregion docs:rs-teleport
}

/// Whether `e` is the runtime saying teleport ships with Cua Spaces (no
/// teleport extension in this process).
pub fn ships_with_cua_spaces(e: &Error) -> bool {
    matches!(
        e,
        Error::Spaces(cua_spaces::Error::HostCapabilityMissing { why, .. })
            if why.contains("Cua Spaces")
    )
}

/// The links the "Install Cua" affordance may open. Session teleport goes
/// through the Cua Keyvault, which refuses an embedded SDK by design
/// (`requires_cua_app`); the UI turns that refusal into a prompt
/// (`requiresCuaApp` in `@trycua/cua/teleport`) whose button asks the shell
/// to open one of these. Nothing else is opened.
pub const CUA_LINKS: [&str; 2] = ["https://cua.ai/install", "cua://keyvault"];

/// `url` when it is one of [`CUA_LINKS`], else an error.
pub fn cua_link(url: &str) -> Result<&'static str> {
    CUA_LINKS
        .iter()
        .copied()
        .find(|l| *l == url)
        .ok_or_else(|| Error::Invalid(format!("not a Cua link: {url}")))
}

/// Opens one of [`CUA_LINKS`] with the OS's default handler.
pub fn open_cua_link(url: &str) -> Result<()> {
    let url = cua_link(url)?;
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Keyvault refusal reaches the webview as the error string with its
    /// code intact (the UI matches `requires_cua_app`), and the affordance
    /// can open only the fixed Cua links. Nothing is opened here.
    #[test]
    fn requires_cua_app_reaches_the_ui_and_only_cua_links_open() {
        let refused = Error::Spaces(cua_spaces::Error::TeleportRefused(
            "requires_cua_app: teleport goes through the Cua Keyvault, which needs the Cua app"
                .into(),
        ));
        let wire = serde_json::to_string(&refused).unwrap();
        assert!(wire.contains("requires_cua_app"), "{wire}");
        assert_eq!(
            cua_link("https://cua.ai/install").unwrap(),
            "https://cua.ai/install"
        );
        assert_eq!(cua_link("cua://keyvault").unwrap(), "cua://keyvault");
        for bad in [
            "https://evil.example",
            "https://cua.ai/install?next=https://evil.example",
            "file:///etc/passwd",
            "",
        ] {
            assert!(cua_link(bad).is_err(), "{bad}");
            assert!(open_cua_link(bad).is_err(), "{bad}");
        }
    }

    /// The in-process MIT runtime has no teleport extension: the manifest is
    /// refused with `HostCapabilityMissing`, and the message the UI shows
    /// says teleport ships with Cua Spaces.
    #[tokio::test]
    async fn the_manifest_says_teleport_ships_with_cua_spaces() {
        let d = tempfile::tempdir().unwrap();
        let core = Core::new(crate::CoreConfig::in_dir(d.path())).unwrap();
        let e = manifest(&core, "firefox", Some("full")).await.unwrap_err();
        assert!(ships_with_cua_spaces(&e), "{e:?}");
        let wire = serde_json::to_string(&e).unwrap();
        assert!(wire.contains("ships with Cua Spaces"), "{wire}");
        assert!(wire.contains("source-available"), "{wire}");
        assert!(!ships_with_cua_spaces(&Error::Invalid("x".into())));
    }

    #[test]
    fn manifests_and_receipts_read_the_tool_json() {
        let m: TeleportManifest = serde_json::from_value(json!({
            "app": "firefox",
            "display_name": "Firefox",
            "scope": "full",
            "items": [{
                "relative_path": "profile/cookies.sqlite",
                "label": "Cookies",
                "estimated_bytes": 10,
                "is_sensitive": true,
                "is_checked_by_default": true,
                "count": 3
            }],
            "total_estimated_bytes": 10,
            "notes": []
        }))
        .unwrap();
        assert_eq!(m.items[0].relative_path, "profile/cookies.sqlite");
        let r: TeleportReceipt = serde_json::from_value(json!({
            "app": "firefox",
            "space": "direct:h:1",
            "method": "import_session",
            "transferred_paths": ["profile/cookies.sqlite"],
            "bundle_bytes": 10,
            "imported": ["profile"],
            "skipped": [],
            "launched": false
        }))
        .unwrap();
        assert_eq!(r.imported, ["profile"]);
    }
}
