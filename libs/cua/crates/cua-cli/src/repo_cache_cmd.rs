//! `cua host config --repo-cache` / `--repo-cache-size` and `cua host cache`:
//! the repo cache's settings, what it holds, and clearing it.

use crate::util::line;
use cua_repo_cache::{Config, RepoCache};
use cua_sandbox_core::settings::{Settings, key};
use cua_sdk::CuaError;
use std::io::Write;
use std::path::Path;

/// The cache under `home` with the settings in `home`'s config file (and
/// the environment).
pub fn cache_for(home: &Path) -> RepoCache {
    let (enabled, cap_bytes) =
        Settings::load_with(home.join("config.toml"), |k| std::env::var(k).ok())
            .map(|s| s.repo_cache())
            .unwrap_or((true, None));
    RepoCache::new(
        home.join("job-cache").join("repos"),
        Config { enabled, cap_bytes },
    )
}

/// Saves `--repo-cache` and `--repo-cache-size` (a missing one keeps its
/// value).
pub fn set(home: &Path, on: Option<&str>, size: Option<&str>) -> Result<(), CuaError> {
    let mut s = Settings::load_with(home.join("config.toml"), |k| std::env::var(k).ok())
        .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
    for (name, v) in [("cache.repos", on), ("cache.repos_size_gb", size)] {
        if let Some(v) = v {
            let k = key(name).map_err(|e| CuaError::Internal(e.to_string()))?;
            s.set(k, v)
                .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
        }
    }
    Ok(())
}

/// What the cache holds, in plain words.
pub fn lines(c: &RepoCache) -> Vec<String> {
    let st = c.status();
    if !st.enabled {
        return vec![
            "repo cache: off".to_string(),
            format!("  {}", st.size_line()),
        ];
    }
    vec![
        "repo cache: on".to_string(),
        format!("  {}", st.size_line()),
        format!("  {}", st.week_line()),
    ]
}

/// The status as JSON.
pub fn json(c: &RepoCache) -> serde_json::Value {
    let s = c.status();
    serde_json::json!({
        "enabled": s.enabled,
        "repos": s.repos,
        "bytes": s.bytes,
        "cap_bytes": s.cap_bytes,
        "hits_last_week": s.hits,
        "misses_last_week": s.misses,
        "seconds_saved_last_week": s.seconds_saved,
        "bytes_saved_last_week": s.bytes_saved,
    })
}

/// `cua host cache status`.
pub fn status(home: &Path, as_json: bool, out: &mut dyn Write) {
    let c = cache_for(home);
    if as_json {
        line(out, json(&c).to_string());
    } else {
        for l in lines(&c) {
            line(out, l);
        }
    }
}

/// `cua host cache clear`.
pub fn clear(home: &Path, as_json: bool, out: &mut dyn Write) {
    let c = cache_for(home);
    let before = c.status();
    c.clear();
    if as_json {
        line(
            out,
            serde_json::json!({"cleared": before.repos, "freed_bytes": before.bytes}).to_string(),
        );
    } else if before.repos == 0 {
        line(out, "The repo cache is already empty.");
    } else {
        line(
            out,
            format!(
                "Cleared the repo cache: {} repo{}, {}.",
                before.repos,
                if before.repos == 1 { "" } else { "s" },
                cua_repo_cache::size_words(before.bytes)
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn the_flags_save_to_the_config_file_and_show_in_the_status() {
        let home = tempfile::tempdir().unwrap();
        let on = text(|o| status(home.path(), false, o));
        assert!(on.starts_with("repo cache: on\n"), "{on}");
        assert!(on.contains("Repos: 0, 0 MB of "), "{on}");
        set(home.path(), Some("off"), Some("2")).unwrap();
        let toml = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(
            toml.contains("repos = \"off\"") && toml.contains("repos_size_gb = \"2\""),
            "{toml}"
        );
        let off = text(|o| status(home.path(), false, o));
        assert_eq!(off, "repo cache: off\n  Repos: 0, 0 MB of 2 GB\n");
        let j: serde_json::Value =
            serde_json::from_str(&text(|o| status(home.path(), true, o))).unwrap();
        assert_eq!(
            (j["enabled"].as_bool(), j["cap_bytes"].as_u64()),
            (Some(false), Some(2 << 30))
        );
        // A bad value is refused with the setting's name, and nothing changes.
        let e = set(home.path(), None, Some("lots")).unwrap_err();
        assert!(e.to_string().contains("cache.repos_size_gb"), "{e}");
    }

    #[test]
    fn clear_says_what_it_removed() {
        let home = tempfile::tempdir().unwrap();
        let first = text(|o| clear(home.path(), false, o));
        assert_eq!(first, "The repo cache is already empty.\n");
        let j: serde_json::Value =
            serde_json::from_str(&text(|o| clear(home.path(), true, o))).unwrap();
        assert_eq!(j["cleared"], 0);
    }
}
