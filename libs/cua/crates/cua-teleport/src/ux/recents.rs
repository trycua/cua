// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Recently teleported apps, so the picker can list them first.
//!
//! One small JSON file of `{catalog id: last used (Unix ms)}`, capped at
//! [`MAX_RECENTS`]. Only catalog ids are stored, never paths or app data.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Kept entries.
pub const MAX_RECENTS: usize = 12;

/// The default recents file: `$CUA_HOME/teleport-recents.json`, else
/// `~/.cua/teleport-recents.json`.
pub fn default_path() -> Option<PathBuf> {
    if crate::host::host_effects_forbidden() {
        return None;
    }
    if let Some(h) = std::env::var_os("CUA_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h).join("teleport-recents.json"));
    }
    crate::host::HostEffects::home_dir(&crate::host::RealHost)
        .map(|h| h.join(".cua/teleport-recents.json"))
}

/// Reads the recents (empty when missing or unreadable).
pub fn load(path: &Path) -> BTreeMap<String, u64> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Records `id` as used at `now_ms` and keeps the newest [`MAX_RECENTS`].
pub fn record(path: &Path, id: &str, now_ms: u64) -> std::io::Result<BTreeMap<String, u64>> {
    let mut map = load(path);
    map.insert(id.to_string(), now_ms);
    if map.len() > MAX_RECENTS {
        let mut by_age: Vec<(String, u64)> = map.clone().into_iter().collect();
        by_age.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        map = by_age.into_iter().take(MAX_RECENTS).collect();
    }
    cua_home::guard_write(path)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&map)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(map)
}

/// Now, in Unix ms.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r/teleport-recents.json");
        assert!(load(&path).is_empty());
        for i in 0..20u64 {
            record(&path, &format!("app{i}"), i).unwrap();
        }
        let map = record(&path, "app0", 100).unwrap();
        assert_eq!(map.len(), MAX_RECENTS);
        assert_eq!(map.get("app0"), Some(&100));
        assert!(!map.contains_key("app1"));
        assert_eq!(load(&path), map);
        std::fs::write(&path, "not json").unwrap();
        assert!(load(&path).is_empty());
    }
}
