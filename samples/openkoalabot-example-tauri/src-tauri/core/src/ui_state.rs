// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The UI's saved state (the Bots, the selected Space and Bot, threads,
//! dropped files, panel toggles) as one JSON file in the app's data
//! directory.
//!
//! The webview runs with a non-persistent (incognito) data store on macOS
//! and a data directory under the app's data directory elsewhere, because
//! WKWebView keeps web storage under the real `~/Library/WebKit/<app>` no
//! matter what `HOME` says. So the page never persists through
//! `localStorage`: the shell injects this file's contents before the page
//! loads ([`UiStateStore::init_script`]) and the page writes each key back
//! through a command ([`UiStateStore::set`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::{Error, Result};

/// The file name, in the app's data directory.
pub const UI_STATE_FILE: &str = "ui-state.json";

/// The page reads the injected state from this global.
pub const UI_STATE_GLOBAL: &str = "__OKB_UI_STATE__";

/// Keys are the page's (`okb.bots`, ...): short, printable, no path parts.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// One JSON object on disk; every write replaces the file atomically.
pub struct UiStateStore {
    path: PathBuf,
    state: Mutex<Map<String, Value>>,
}

impl UiStateStore {
    /// The store in `dir` (created on the first write). An unreadable or
    /// malformed file starts empty: saved UI state is a convenience.
    pub fn in_dir(dir: &Path) -> Self {
        let path = dir.join(UI_STATE_FILE);
        let state = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Everything saved.
    pub fn snapshot(&self) -> Map<String, Value> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Saves one key (`null` removes it) and writes the file.
    pub fn set(&self, key: &str, value: Value) -> Result<()> {
        if !valid_key(key) {
            return Err(Error::Invalid(format!("not a UI state key: {key}")));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if value.is_null() {
            state.remove(key);
        } else {
            state.insert(key.to_string(), value);
        }
        let bytes = serde_json::to_vec(&*state).map_err(|e| Error::Invalid(e.to_string()))?;
        let dir = self
            .path
            .parent()
            .ok_or_else(|| Error::Invalid("the UI state file has no directory".into()))?;
        std::fs::create_dir_all(dir).map_err(|e| Error::Invalid(e.to_string()))?;
        let tmp =
            tempfile::NamedTempFile::new_in(dir).map_err(|e| Error::Invalid(e.to_string()))?;
        std::fs::write(tmp.path(), bytes).map_err(|e| Error::Invalid(e.to_string()))?;
        tmp.persist(&self.path)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(())
    }

    /// The initialization script that hands the saved state to the page
    /// before any of its code runs.
    pub fn init_script(&self) -> String {
        // JSON is a JS expression; `<` is escaped so the text can never close
        // a script element.
        let json = Value::Object(self.snapshot())
            .to_string()
            .replace('<', "\\u003c");
        format!("window.{UI_STATE_GLOBAL} = {json};")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let store = UiStateStore::in_dir(dir.path());
        store.set("okb.bots", json!([{"id": "b1"}])).unwrap();
        store.set("okb.bot", json!("b1")).unwrap();
        let again = UiStateStore::in_dir(dir.path());
        assert_eq!(again.snapshot()["okb.bots"], json!([{"id": "b1"}]));
        again.set("okb.bot", Value::Null).unwrap();
        assert!(
            !UiStateStore::in_dir(dir.path())
                .snapshot()
                .contains_key("okb.bot")
        );
        assert_eq!(store.path(), dir.path().join(UI_STATE_FILE));
    }

    #[test]
    fn refuses_odd_keys_and_survives_a_bad_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(UI_STATE_FILE), "not json").unwrap();
        let store = UiStateStore::in_dir(dir.path());
        assert!(store.snapshot().is_empty());
        assert!(store.set("../x", json!(1)).is_err());
        assert!(store.set("", json!(1)).is_err());
    }

    #[test]
    fn init_script_assigns_the_global_and_escapes_markup() {
        let dir = tempfile::tempdir().unwrap();
        let store = UiStateStore::in_dir(dir.path());
        store.set("okb.space", json!("</script>")).unwrap();
        let script = store.init_script();
        assert!(script.starts_with("window.__OKB_UI_STATE__ = {"));
        assert!(!script.contains("</script>"));
        assert!(script.contains("\\u003c/script>"));
    }
}
