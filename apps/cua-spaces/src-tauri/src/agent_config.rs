// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings "Teleport permissions": a per-app policy of which sensitivity
//! tiers may teleport without a biometric prompt. Changing the policy is
//! itself biometric-gated so it cannot be flipped off casually. The provider
//! list comes from the SDK (`cua_spaces_ext::teleport::AppSessions::catalog`).
//!
//! The "AI agents" section (skills and the cua MCP server in each coding
//! agent) lives in `agent_setup.rs` on the SDK's `cua-agent-setup`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct TeleportAppInfo {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub allow_sensitive: bool,
    pub allow_non_sensitive: bool,
}

/// One app's teleport-without-biometric allowances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppTeleportPolicy {
    #[serde(default)]
    pub allow_sensitive: bool,
    /// Non-sensitive teleport is unattended by default (only sensitive items
    /// are biometric-gated).
    #[serde(default = "default_true")]
    pub allow_non_sensitive: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppTeleportPolicy {
    fn default() -> Self {
        Self {
            allow_sensitive: false,
            allow_non_sensitive: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// The shared teleport-policy file both the app and the Spaces MCP read
/// (`$CUA_HOME`, else `~/.cua`, like the rest of the cua home).
pub fn policy_path() -> PathBuf {
    cua_auth::cua_home().join("spaces-teleport-policy.json")
}

/// Cached `(alias, provider id)` pairs from the SDK's teleport providers. The
/// window picker resolves one per enumerated window, so build it once.
static PROVIDER_INDEX: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();

fn provider_catalog() -> Vec<cua_spaces_ext::teleport::ProviderInfo> {
    cua_spaces_ext::teleport::AppSessions::builtin().catalog()
}

/// Every alias (app ids, display name, id) of every provider, lowercased.
pub fn alias_index(catalog: &[cua_spaces_ext::teleport::ProviderInfo]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for p in catalog {
        for alias in p
            .app_ids
            .iter()
            .cloned()
            .chain([p.display_name.clone(), p.id.clone()])
        {
            let alias = alias.trim().to_ascii_lowercase();
            if !alias.is_empty() {
                pairs.push((alias, p.id.clone()));
            }
        }
    }
    pairs
}

fn provider_index() -> &'static Vec<(String, String)> {
    PROVIDER_INDEX.get_or_init(|| alias_index(&provider_catalog()))
}

/// The teleport provider id that handles `app` (a macOS app display name,
/// bundle id, or provider id), or `None` when no provider claims it.
pub fn provider_id_for_app(app: &str) -> Option<String> {
    let needle = app.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    provider_index()
        .iter()
        .find(|(alias, _)| *alias == needle)
        .map(|(_, id)| id.clone())
}

// ---------------------------------------------------------------------------
// Commands: teleport permissions
// ---------------------------------------------------------------------------

fn read_policy_map() -> std::collections::HashMap<String, AppTeleportPolicy> {
    std::fs::read(policy_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_policy_map(
    map: &std::collections::HashMap<String, AppTeleportPolicy>,
) -> Result<(), String> {
    let path = policy_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Whether `app_id` is allow-listed for unattended (no-biometric) teleport
/// of sensitive items. Stored for the SDK's sender: `cua_spaces` has no
/// skip-auth hook on `AppSessions` yet, so today the provider's own host
/// gate still prompts (the safe direction).
#[allow(dead_code)]
pub fn teleport_unattended(app_id: &str) -> bool {
    read_policy_map()
        .get(app_id)
        .map(|p| p.allow_sensitive)
        .unwrap_or(false)
}

#[tauri::command]
pub fn list_teleportable_apps() -> Vec<TeleportAppInfo> {
    let policy = read_policy_map();
    provider_catalog()
        .into_iter()
        // Only offer apps this host can actually export.
        .filter(|p| p.supported_here)
        .map(|p| {
            let rule = policy.get(&p.id).cloned().unwrap_or_default();
            TeleportAppInfo {
                id: p.id,
                name: p.display_name,
                installed: p.installed,
                allow_sensitive: rule.allow_sensitive,
                allow_non_sensitive: rule.allow_non_sensitive,
            }
        })
        .collect()
}

#[tauri::command]
pub fn set_teleport_policy(
    app: String,
    allow_sensitive: bool,
    allow_non_sensitive: bool,
) -> Result<(), String> {
    // Changing an unattended-teleport allowance must be authorized by the device
    // owner, so it can't be silently loosened.
    crate::biometric::authorize(&format!("change unattended-teleport permissions for {app}"))?;
    let mut map = read_policy_map();
    map.insert(
        app,
        AppTeleportPolicy {
            allow_sensitive,
            allow_non_sensitive,
        },
    );
    write_policy_map(&map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_display_names_and_bundle_ids() {
        let catalog = vec![cua_spaces_ext::teleport::ProviderInfo {
            id: "unity-hub".into(),
            display_name: "Unity Hub".into(),
            app_ids: vec!["com.unity3d.unityhub".into()],
            supported_here: true,
            installed: false,
        }];
        let idx = alias_index(&catalog);
        let find = |a: &str| idx.iter().find(|(k, _)| k == a).map(|(_, v)| v.as_str());
        assert_eq!(find("unity hub"), Some("unity-hub"));
        assert_eq!(find("com.unity3d.unityhub"), Some("unity-hub"));
        assert_eq!(find("unity-hub"), Some("unity-hub"));
    }

    #[test]
    fn a_policy_file_round_trips_with_safe_defaults() {
        let p: AppTeleportPolicy = serde_json::from_str("{}").unwrap();
        assert!(!p.allow_sensitive);
        assert!(p.allow_non_sensitive);
    }
}
