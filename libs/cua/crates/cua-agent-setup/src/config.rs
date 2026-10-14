//! `~/.cua/config` (TOML): user preferences shared by the CLI, the SDK and
//! the Spaces app. Edited with toml_edit so other keys and comments stay.
//!
//! ```toml
//! [onboarding]
//! agents = "never"   # never offer agent setup after `cua auth login`
//! ```

use crate::{Error, HostEnv, Result, fsutil};
use std::path::PathBuf;

/// The config file.
pub fn path(env: &HostEnv) -> PathBuf {
    env.cua_home().join("config")
}

fn load(env: &HostEnv) -> Result<toml_edit::DocumentMut> {
    let p = path(env);
    let text = match std::fs::read_to_string(&p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(&p, e)),
    };
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|e| Error::Malformed {
            path: p,
            detail: format!("TOML parse error: {}", e.to_string().trim()),
        })
}

/// Whether the user answered "never" to the agent onboarding prompt.
pub fn onboarding_declined(env: &HostEnv) -> bool {
    load(env)
        .ok()
        .and_then(|d| {
            d.get("onboarding")
                .and_then(|t| t.get("agents"))
                .and_then(|v| v.as_str())
                .map(|s| s == "never")
        })
        .unwrap_or(false)
}

/// Persists (`true`) or clears (`false`) the "never" answer.
pub fn set_onboarding_declined(env: &HostEnv, declined: bool) -> Result<()> {
    let mut d = load(env)?;
    if declined {
        let t = d
            .entry("onboarding")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        let t = t.as_table_like_mut().ok_or_else(|| Error::Malformed {
            path: path(env),
            detail: "\"onboarding\" is not a table".into(),
        })?;
        t.insert("agents", toml_edit::value("never"));
    } else if let Some(t) = d.get_mut("onboarding").and_then(|t| t.as_table_like_mut()) {
        t.remove("agents");
    }
    fsutil::atomic_write(&path(env), d.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_round_trips_and_keeps_other_keys() {
        let d = tempfile::tempdir().unwrap();
        let env = HostEnv::isolated(d.path());
        assert!(!onboarding_declined(&env));
        std::fs::create_dir_all(env.cua_home()).unwrap();
        std::fs::write(path(&env), "# prefs\ntheme = \"dark\"\n").unwrap();
        set_onboarding_declined(&env, true).unwrap();
        assert!(onboarding_declined(&env));
        let text = std::fs::read_to_string(path(&env)).unwrap();
        assert!(
            text.contains("# prefs") && text.contains("theme = \"dark\""),
            "{text}"
        );
        assert!(text.contains("[onboarding]\nagents = \"never\""), "{text}");
        set_onboarding_declined(&env, false).unwrap();
        assert!(!onboarding_declined(&env));
    }
}
