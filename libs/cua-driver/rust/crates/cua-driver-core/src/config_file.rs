// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Helpers for the persisted `~/.cua-driver/config.json` file that the
//! per-platform `set_config` / `get_config` tools read and write.

/// Canonical `~/.cua-driver/config.json` path matching what the per-platform
/// `set_config` tools write to. Resolves `$HOME` first (Unix/macOS) and falls
/// back to `%USERPROFILE%` (Windows, where `HOME` is usually unset). Returns
/// `None` when neither is set (sandboxed CI).
pub fn default_config_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".cua-driver")
                .join("config.json")
        })
}

/// Read a single key from `~/.cua-driver/config.json` as a raw JSON value,
/// returning `None` when the file is missing/malformed or the key is absent.
/// Used by the per-platform `load_driver_config` helpers to rehydrate the
/// in-memory `DriverConfig` at process startup so `set_config` writes survive
/// across stateless `cua-driver call` invocations.
pub fn read_config_value(key: &str) -> Option<serde_json::Value> {
    let path = default_config_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get(key).cloned()
}

/// Merge a single `key`/`value` into `~/.cua-driver/config.json`,
/// preserving any other keys that are already there. Used by the
/// per-platform `set_config` tools to persist settings so the next daemon
/// restart picks them up.
pub fn write_config_key(key: &str, value: serde_json::Value) -> Result<(), String> {
    let path = default_config_path().ok_or_else(|| "$HOME is not set".to_string())?;
    let mut json: serde_json::Value = path
        .exists()
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    json[key] = value;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(())
}
