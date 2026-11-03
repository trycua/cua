// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Where the app's webviews keep data: under the app's data directory
//! (`$HOME/Library/Application Support/com.trycua.spaces.prototype` on
//! macOS), never the platform's per-user default.
//!
//! WKWebView cannot be pointed at a directory and ignores `HOME`: with its
//! default store, web storage lands in the real `~/Library/WebKit/<app>`.
//! So every window is built through [`isolate`]: a data directory under the
//! app's data directory on Windows and Linux, a non-persistent store on
//! macOS. The page's small settings (`localStorage` keys in
//! `src/state/settings.ts`) live in [`UiStorage`] instead: injected before
//! the page runs as `window.__CUA_UI_STORAGE__` and written back through
//! [`ui_storage_set`], which also updates every open window.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::{Manager, Runtime};

/// The file, in the app's data directory.
pub const UI_STORAGE_FILE: &str = "ui-storage.json";
/// The page reads the injected values from this global.
pub const UI_STORAGE_GLOBAL: &str = "__CUA_UI_STORAGE__";

/// Keys are the page's (`cua.settings.menuBar`, ...).
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// String values by key, as `localStorage` holds them, in one JSON file.
pub struct UiStorage {
    path: PathBuf,
    values: Mutex<BTreeMap<String, String>>,
}

impl UiStorage {
    /// The storage in `dir`; a missing or damaged file starts empty.
    pub fn in_dir(dir: &Path) -> Self {
        let path = dir.join(UI_STORAGE_FILE);
        let values = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            values: Mutex::new(values),
        }
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Everything stored.
    pub fn snapshot(&self) -> BTreeMap<String, String> {
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Sets (or with `None` removes) one key and writes the file atomically.
    pub fn set(&self, key: &str, value: Option<String>) -> Result<(), String> {
        if !valid_key(key) {
            return Err(format!("not a UI storage key: {key}"));
        }
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        match value {
            Some(v) => values.insert(key.to_string(), v),
            None => values.remove(key),
        };
        let bytes = serde_json::to_vec(&*values).map_err(|e| e.to_string())?;
        let dir = self
            .path
            .parent()
            .ok_or("the UI storage file has no directory")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        std::fs::write(tmp.path(), bytes).map_err(|e| e.to_string())?;
        tmp.persist(&self.path).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The script that hands the stored values to a page before it runs.
    pub fn init_script(&self) -> String {
        let json = serde_json::to_string(&self.snapshot())
            .unwrap_or_else(|_| "{}".into())
            .replace('<', "\\u003c");
        format!("window.{UI_STORAGE_GLOBAL} = {json};")
    }

    /// The script that applies one change in an open page.
    pub fn update_script(key: &str, value: Option<&str>) -> String {
        let key = serde_json::to_string(key).unwrap_or_default();
        let assign = match value {
            Some(v) => format!(
                "s[{key}] = {};",
                serde_json::to_string(v)
                    .unwrap_or_default()
                    .replace('<', "\\u003c")
            ),
            None => format!("delete s[{key}];"),
        };
        format!("(function(){{var s=window.{UI_STORAGE_GLOBAL};if(s){{{assign}}}}})();")
    }
}

/// The webview data directory inside the app's data directory.
pub fn webview_data_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("webview")
}

/// The app's data directory (follows `HOME`).
pub fn app_data_dir<R: Runtime>(app: &impl Manager<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("no app data directory: {e}"))
}

/// Applies the app's webview data policy to a window about to be built.
pub fn isolate<'a, R: Runtime, M: Manager<R>>(
    builder: tauri::WebviewWindowBuilder<'a, R, M>,
    data_dir: &Path,
    storage: &UiStorage,
) -> tauri::WebviewWindowBuilder<'a, R, M> {
    builder
        .data_directory(webview_data_dir(data_dir))
        .incognito(cfg!(target_os = "macos"))
        .initialization_script(storage.init_script())
}

/// [`isolate`] with the managed [`UiStorage`] and the app's data directory,
/// as a builder method: `WebviewWindowBuilder::new(..)...isolated(&app).build()`.
pub trait IsolateWebview<R: Runtime>: Sized {
    /// Applies the app's webview data policy.
    fn isolated(self, manager: &impl Manager<R>) -> Self;
}

impl<'a, R: Runtime, M: Manager<R>> IsolateWebview<R> for tauri::WebviewWindowBuilder<'a, R, M> {
    fn isolated(self, manager: &impl Manager<R>) -> Self {
        let storage = manager.state::<UiStorageState>();
        match app_data_dir(manager) {
            Ok(dir) => isolate(self, &dir, &storage.0),
            // No data directory: still never persist web data.
            Err(_) => self
                .incognito(true)
                .initialization_script(storage.0.init_script()),
        }
    }
}

/// Tauri-managed [`UiStorage`].
pub struct UiStorageState(pub UiStorage);

/// Saves one `localStorage`-style key (`null` removes it) and applies it in
/// every open window.
#[tauri::command]
pub fn ui_storage_set<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, UiStorageState>,
    key: String,
    value: Option<String>,
) -> Result<(), String> {
    state.0.set(&key, value.clone())?;
    let script = UiStorage::update_script(&key, value.as_deref());
    for window in app.webview_windows().values() {
        let _ = window.eval(&script);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_persists_reloads_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let s = UiStorage::in_dir(dir.path());
        s.set("cua.settings.menuBar", Some("true".into())).unwrap();
        let again = UiStorage::in_dir(dir.path());
        assert_eq!(
            again
                .snapshot()
                .get("cua.settings.menuBar")
                .map(String::as_str),
            Some("true")
        );
        again.set("cua.settings.menuBar", None).unwrap();
        assert!(UiStorage::in_dir(dir.path()).snapshot().is_empty());
        assert_eq!(s.path(), dir.path().join(UI_STORAGE_FILE));
        assert!(s.set("../x", Some("1".into())).is_err());
    }

    #[test]
    fn scripts_assign_the_global_and_escape_markup() {
        let dir = tempfile::tempdir().unwrap();
        let s = UiStorage::in_dir(dir.path());
        s.set("k", Some("</script>".into())).unwrap();
        let init = s.init_script();
        assert!(init.starts_with("window.__CUA_UI_STORAGE__ = {"));
        assert!(!init.contains("</script>"));
        let up = UiStorage::update_script("k", Some("</x>"));
        assert!(up.contains("s[\"k\"] = \"\\u003c/x>\";"), "{up}");
        assert!(UiStorage::update_script("k", None).contains("delete s[\"k\"];"));
    }

    #[test]
    fn webview_data_is_inside_the_app_data_dir() {
        let d = Path::new("/h/Library/Application Support/com.trycua.spaces.prototype");
        assert!(webview_data_dir(d).starts_with(d));
    }
}
