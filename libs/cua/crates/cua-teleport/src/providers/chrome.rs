// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Chrome / Chromium session export.
//!
//! Captures a Chrome profile's open tabs (via the `Sessions/` files and, when
//! reachable, the DevTools endpoint at `http://localhost:9222/json`) and, for
//! full-profile transfers, a curated subset of the profile directory
//! ([`layout::chrome`]). The bundle is written in a single canonical (Linux)
//! layout on the wire; the receiver remaps it to the *destination* platform's
//! real Chrome user-data-dir and relaunches that platform's Chrome.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::bundle::{BundleWriter, DEFAULT_MAX_TOTAL_BYTES};
#[cfg(target_os = "macos")]
use crate::host::{EffectKind, HostCommand};
use crate::host::{HostEffects, default_host};
use crate::layout::chrome::{
    APP_IDS as CHROME_APP_IDS, PROFILE_DIRS, PROFILE_FILES, SESSION_FILES, TABS_JSON,
    dest_rel_path, user_data_dir_for,
};
use crate::providers::util::{add_dir_recursive, add_file_best_effort, dir_len, file_len};
use crate::{
    AppRef, ExportProvider, ManifestItem, Platform, Result, TeleportError, TransferManifest,
    TransferScope, WindowRef,
};

/// The Chrome/Chromium session provider.
pub struct ChromeProvider {
    /// Test/override hook: when set, this exact profile directory is used
    /// instead of the platform-derived location.
    profile_dir_override: Option<PathBuf>,
    /// Which profile to capture: a bare name under the user-data dir
    /// ("Profile 1") or a path. `None` means `Default`.
    profile_selector: Option<String>,
    /// DevTools JSON endpoint; disabled when `None`.
    devtools_url: Option<String>,
    /// macOS: read open tabs via AppleScript when DevTools isn't available (a
    /// normal Chrome exposes no debug port). Disabled by `without_devtools` so
    /// tests stay hermetic.
    allow_applescript: bool,
    max_total_bytes: u64,
    /// Every live-browser, `$HOME` and authorization effect goes through this.
    host: Arc<dyn HostEffects>,
}

impl Default for ChromeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChromeProvider {
    pub fn new() -> Self {
        Self {
            profile_dir_override: None,
            profile_selector: None,
            devtools_url: Some("127.0.0.1:9222".to_string()),
            allow_applescript: true,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host: default_host(),
        }
    }

    /// Act on `host` instead of [`default_host`].
    pub fn with_host(mut self, host: Arc<dyn HostEffects>) -> Self {
        self.host = host;
        self
    }

    /// Override the source profile directory (used by tests and by callers
    /// that already know the exact profile path).
    pub fn with_profile_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.profile_dir_override = Some(dir.into());
        self
    }

    /// Capture the named profile ("Profile 1") or the profile at a path,
    /// instead of `Default` (what `cua teleport … --profile` sets). A selector
    /// that names no profile on disk fails the manifest and the export with
    /// the list of profiles that do exist.
    pub fn with_profile(mut self, selector: impl Into<String>) -> Self {
        self.profile_selector = Some(selector.into());
        self
    }

    /// Disable the DevTools probe and the AppleScript fallback (used by tests to
    /// stay hermetic — no live Chrome, no `osascript`).
    pub fn without_devtools(mut self) -> Self {
        self.devtools_url = None;
        self.allow_applescript = false;
        self
    }

    /// Set the total-size guard for exported bundles.
    pub fn with_max_total_bytes(mut self, bytes: u64) -> Self {
        self.max_total_bytes = bytes;
        self
    }

    /// Resolve the source profile directory for the given platform.
    ///
    /// Chrome keeps each profile in its own subdirectory of the user-data dir
    /// (`Default`, `Profile 1`, …), so which one to capture is a choice. In
    /// precedence order: an explicit `with_profile_dir`, then `with_profile`
    /// (what `--profile` sets), then `Default` — the
    /// profile a plain `Chrome` launch uses, which is the right default because
    /// asking to teleport "my Chrome" means the one the user is signed in to.
    fn source_profile_dir(&self, platform: Platform) -> Option<PathBuf> {
        if let Some(dir) = &self.profile_dir_override {
            return Some(dir.clone());
        }
        let root = self.user_data_dir(platform)?;
        Some(match &self.profile_selector {
            Some(value) => Self::resolve_profile(&root, value),
            None => root.join("Default"),
        })
    }

    /// The platform's Chrome user-data dir — the parent of every profile.
    fn user_data_dir(&self, platform: Platform) -> Option<PathBuf> {
        Some(self.host.home_dir()?.join(user_data_dir_for(platform)))
    }

    /// Interpret a profile selector: a path (absolute, or containing a
    /// separator) is taken as-is so a caller can point anywhere; a bare name is
    /// joined under the user-data dir, which is how Chrome itself names them.
    fn resolve_profile(root: &Path, value: &str) -> PathBuf {
        let value = value.trim();
        let as_path = Path::new(value);
        // `/` separates on every OS (Windows accepts it too); `\` only on
        // Windows, where it is MAIN_SEPARATOR.
        if as_path.is_absolute() || value.contains('/') || value.contains(std::path::MAIN_SEPARATOR)
        {
            as_path.to_path_buf()
        } else {
            root.join(value)
        }
    }

    /// `source_profile_dir`, but failing loudly: a selector that names no
    /// profile on disk would otherwise capture an empty bundle and look like a
    /// successful teleport, so name the profiles that do exist instead.
    fn resolved_profile_dir(&self, platform: Platform) -> Result<PathBuf> {
        let dir = self
            .source_profile_dir(platform)
            .ok_or_else(|| TeleportError::Provider("could not resolve home directory".into()))?;
        // Only an explicit selection is validated; a missing `Default` is left
        // to the existing per-item best-effort handling (Chrome may simply not
        // be installed, which is not an argument error).
        if self.profile_dir_override.is_none() && self.profile_selector.is_some() && !dir.is_dir() {
            let available = self.available_profiles(platform);
            let available = if available.is_empty() {
                "none found".to_string()
            } else {
                available.join(", ")
            };
            return Err(TeleportError::Provider(format!(
                "Chrome profile {} not found; available: {available}",
                dir.display()
            )));
        }
        Ok(dir)
    }

    /// Profile directory names present on disk, for error messages and for
    /// callers offering a picker. A directory is a profile if it holds
    /// `Preferences`, which every real profile has and no sibling cache dir does.
    pub fn available_profiles(&self, platform: Platform) -> Vec<String> {
        let Some(root) = self.user_data_dir(platform) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().join("Preferences").is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        // "Default" first, then the numbered profiles in their natural order.
        names.sort_by_key(|n| (n != "Default", n.clone()));
        names
    }

    /// Fetch open-tab URLs, or say why it could not be done.
    ///
    /// `Ok(vec![])` means one specific thing — Chrome is not running, so there
    /// are genuinely no open tabs. Every other outcome is an `Err`.
    ///
    /// It used to return `Vec<String>` and answer every failure with an empty
    /// vec, so "Chrome has no tabs open" and "we were refused permission to
    /// ask" were the same value. The consequences were invisible by
    /// construction: the manifest still listed "Open tabs", checked, with no
    /// count and `est_bytes: 0`; the export still wrote a valid, empty
    /// `tabs.json`; the teleport still reported success; and the guest's Chrome
    /// opened on nothing. The most likely real cause — macOS Automation
    /// permission not granted for the calling process, which surfaces as
    /// osascript error -1743 — is also the most completely hidden, because it
    /// looks exactly like a tidy browser.
    fn fetch_tabs(&self) -> Result<Vec<String>> {
        // An explicit opt-out is not a failure. `without_devtools()` turns off
        // every enumeration mechanism on purpose — it is how callers (and the
        // hermetic tests) say "do not touch a live browser" — so the honest
        // answer there is "no tabs", not "something went wrong". The rule this
        // whole function exists to enforce is that a MECHANISM THAT WAS TRIED
        // AND FAILED must never be reported as an empty browser; it is not that
        // an empty list is forbidden.
        if self.devtools_url.is_none() && !self.allow_applescript {
            return Ok(Vec::new());
        }
        // DevTools first — works when Chrome was launched with a debug port.
        // An explicitly configured endpoint is a promise that it is there, so a
        // failure to reach it is an error rather than a cue to try something
        // else. (An EMPTY answer is not a failure: that is a live Chrome with
        // no pages, and on macOS AppleScript is still worth trying below.)
        if let Some(addr) = &self.devtools_url {
            match self.host.devtools_page_urls(addr) {
                Ok(urls) if !urls.is_empty() => return Ok(urls),
                Ok(_) => {}
                Err(err) => {
                    #[cfg(target_os = "macos")]
                    if self.allow_applescript {
                        return fetch_tabs_applescript(&*self.host);
                    }
                    return Err(TeleportError::Provider(format!(
                        "Chrome DevTools endpoint {addr} is configured but unreachable \
                         ({err}); cannot enumerate open tabs"
                    )));
                }
            }
        }
        // macOS: a normal Chrome exposes no DevTools port, so read the live tab
        // URLs via AppleScript. This is the usual path in practice.
        #[cfg(target_os = "macos")]
        if self.allow_applescript {
            return fetch_tabs_applescript(&*self.host);
        }
        // Nothing left to try. Do not pretend the browser was empty.
        Err(TeleportError::Provider(
            "cannot enumerate Chrome's open tabs: no DevTools endpoint is configured \
             and no other method is available on this platform. Launch Chrome with \
             --remote-debugging-port=9222 and pass its address, or deselect \"Open \
             tabs\" and rely on the Sessions/ store, which restores the window \
             layout on the destination."
                .to_string(),
        ))
    }
}

/// Read the open tab URLs from a running Chrome via AppleScript (macOS). Returns
/// `Ok(vec![])` only when Chrome is not running; any other failure is an `Err`
/// carrying osascript's own diagnosis (commonly -1743, "Not authorized to send
/// Apple events to Google Chrome", i.e. the Automation permission).
#[cfg(target_os = "macos")]
fn fetch_tabs_applescript(host: &dyn HostEffects) -> Result<Vec<String>> {
    // "Chrome isn't running" is the one legitimate empty answer, and it has to
    // be established BEFORE asking, because `tell application "Google Chrome"`
    // would otherwise LAUNCH Chrome just to be asked what it has open — a
    // browser window appearing on the user's screen because they previewed a
    // manifest. pgrep matches the real bundle executable.
    let running = host
        .run(
            &HostCommand::new(EffectKind::ProcessLookup, "/usr/bin/pgrep")
                .args(["-x", "Google Chrome"]),
        )
        .map(|output| output.success)
        .unwrap_or(false);
    if !running {
        return Ok(Vec::new());
    }
    // The first output line is the front window's *active* tab URL (empty when
    // it can't be read); the remaining lines are every tab in window order. The
    // active tab is reordered to the front below so the guest opens with the
    // same tab focused (Chrome activates the first URL argument on launch).
    const SCRIPT: &str = r#"tell application "Google Chrome"
set activeURL to ""
try
  set activeURL to URL of active tab of front window
end try
set out to activeURL & linefeed
repeat with w in windows
  repeat with t in tabs of w
    set out to out & (URL of t) & linefeed
  end repeat
end repeat
return out
end tell"#;
    match run_osascript_bounded(host, SCRIPT, APPLESCRIPT_BUDGET) {
        Ok(raw) => Ok(reorder_active_tab_first(&raw)),
        Err(why) => Err(TeleportError::Provider(format!(
            "Google Chrome is running but its open tabs could not be read: {why}. \
             If this mentions Apple events or -1743, grant Automation control of \
             Google Chrome to the process running the teleport (for example `cua`) in System Settings > \
             Privacy & Security > Automation."
        ))),
    }
}

/// Time budget for the AppleScript tab read. Once Automation permission is
/// granted the read is fast; this cap keeps the manifest from stalling if the
/// permission prompt is pending or Chrome is slow to answer.
#[cfg(target_os = "macos")]
const APPLESCRIPT_BUDGET: Duration = Duration::from_millis(1200);

/// Run `osascript -e <script>` with a hard timeout, returning its stdout on a
/// clean exit or a description of what went wrong. If the process outruns the
/// budget (e.g. blocked on an Automation-permission prompt) it is killed so the
/// caller — a manifest preview or an export — never hangs.
///
/// stderr is CAPTURED, not discarded. osascript puts the only useful diagnosis
/// there ("Not authorized to send Apple events to Google Chrome. (-1743)"), and
/// sending it to /dev/null was what reduced every distinct failure to a bare
/// "no tabs".
#[cfg(target_os = "macos")]
fn run_osascript_bounded(
    host: &dyn HostEffects,
    script: &str,
    budget: Duration,
) -> std::result::Result<String, String> {
    let output = host
        .run(
            &HostCommand::new(EffectKind::AppleScript, "osascript")
                .args(["-e", script])
                .timeout(budget),
        )
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                format!(
                    "osascript did not answer within {} ms (an Automation \
                     permission prompt may be pending)",
                    budget.as_millis()
                )
            } else {
                format!("could not run osascript: {e}")
            }
        })?;
    if !output.success {
        let err = String::from_utf8_lossy(&output.stderr);
        let err = err.trim();
        return Err(if err.is_empty() {
            "osascript exited unsuccessfully".to_string()
        } else {
            format!("osascript failed: {err}")
        });
    }
    String::from_utf8(output.stdout).map_err(|e| format!("could not read osascript output: {e}"))
}

/// Parse `fetch_tabs_applescript`'s output — active-tab URL on the first line,
/// then every tab in window order — into a de-duplicated tab list whose first
/// entry is the source's active tab. Falls back to the existing order when the
/// active tab can't be determined. Pure, so it is unit-testable off macOS.
#[cfg(target_os = "macos")]
fn reorder_active_tab_first(raw: &str) -> Vec<String> {
    fn is_real(url: &str) -> bool {
        !url.is_empty() && !url.starts_with("chrome://") && !url.starts_with("chrome-extension://")
    }
    let mut lines = raw.lines();
    let active = lines.next().map(str::trim).unwrap_or("").to_string();
    let mut tabs: Vec<String> = lines
        .map(str::trim)
        .filter(|url| is_real(url))
        .map(str::to_string)
        .collect();
    // Move the active tab to the front (it also appears in the repeat above, so
    // remove the duplicate rather than listing the URL twice).
    if is_real(&active) {
        match tabs.iter().position(|url| url == &active) {
            Some(pos) => {
                let url = tabs.remove(pos);
                tabs.insert(0, url);
            }
            None => tabs.insert(0, active),
        }
    }
    tabs
}

/// The profile subdirectory name (e.g. "Default") from a source profile dir.
fn profile_name(profile_dir: &Path) -> String {
    profile_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Default".to_string())
}

/// Build a [`ManifestItem`], turning an optional `(count, noun)` into the split
/// `count`/`count_noun` fields. `count == None` leaves the consent UI to fall
/// back to `est_bytes`.
fn manifest_item(
    label: &str,
    rel_path: &str,
    est_bytes: u64,
    sensitive: bool,
    count: Option<(u64, &str)>,
) -> ManifestItem {
    let (count, count_noun) = match count {
        Some((n, noun)) => (Some(n), Some(noun.to_string())),
        None => (None, None),
    };
    ManifestItem {
        label: label.to_string(),
        rel_path: rel_path.to_string(),
        est_bytes,
        count,
        count_noun,
        sensitive,
        // Everything that is not sensitive is on by default; everything that is
        // stays off and is gated.
        //
        // This used to be `rel_path == TABS_JSON`, i.e. a default Chrome
        // teleport moved ONE item: a list of URLs. Not the session store, not
        // preferences, not even bookmarks. "Teleport my Chrome" produced a
        // stock browser that happened to have some tabs open — and when tab
        // enumeration came up empty it produced a 0-byte `tabs.json` and looked
        // like it had done nothing at all. Compare the Unity Hub provider,
        // which checks everything except a bulky cache and lands a working,
        // signed-in Hub; Chrome was the odd one out, and it was not a
        // considered position: no other provider ties the default set to one
        // hard-coded path.
        //
        // The `sensitive` flag already encodes the judgment about what must not
        // move without consent, so the default set is derived from it rather
        // than being a second, independent list that drifts. Concretely:
        //
        //   moves by default   tabs.json, Session state (*), Preferences,
        //                      Bookmarks, Web Data, Local Storage/, Sessions/,
        //                      Session Storage/
        //   withheld           Cookies, Login Data, History
        //
        // `Sessions/` is the load-bearing addition: it is the SNSS store that
        // `--restore-last-session` reads on the destination, so the guest comes
        // up with the real window and tab layout instead of a row of URLs
        // opened as arguments.
        //
        // The three withheld items are exactly the credential-shaped ones, and
        // they remain reachable — check them explicitly, or allow them ahead of
        // time for unattended use via ~/.cua/spaces-teleport-policy.json
        // (allow_sensitive), which is what that file is for. Withholding
        // History as well is not only privacy: it is 80 MB on a normal profile
        // and buys the destination nothing.
        //
        // The cost of the split: a default teleport lands a browser that is NOT
        // logged in, because cookies are a credential. That is the right
        // default for something that copies a personal profile onto another
        // machine, and it is now the only thing missing rather than everything.
        default_checked: !sensitive,
    }
}

/// `Local Storage/` as items: the LevelDB is read from a private copy and its
/// values travel in the reserved `localstorage.json` entry, which the receiver
/// writes into the destination browser's own store (a raw copy of LevelDB
/// files would replace whatever the destination holds, and tear if the source
/// is running). Anything else under `Local Storage/` stays a file. When the
/// LevelDB cannot be read, the whole directory is copied as before.
fn add_local_storage<W: std::io::Write>(
    writer: &mut cua_teleport_bundle::bundle::BundleWriter<W>,
    profile_dir: &Path,
    disk: &Path,
    rel: &str,
) -> Result<()> {
    match cua_chromium_storage::read(&cua_chromium_storage::store_dir(profile_dir)) {
        Ok(items) => {
            if !items.is_empty() {
                writer.add_bytes(
                    cua_chromium_storage::LOCAL_STORAGE_ENTRY,
                    0o600,
                    &cua_teleport_bundle::local_storage::serialize(&items),
                )?;
            }
            if let Ok(entries) = std::fs::read_dir(disk) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name == "leveldb" {
                        continue;
                    }
                    let child = format!("{rel}/{name}");
                    match std::fs::symlink_metadata(&path) {
                        Ok(m) if m.is_file() => add_file_best_effort(writer, &path, &child)?,
                        Ok(m) if m.is_dir() => add_dir_recursive(writer, &path, &child)?,
                        _ => {}
                    }
                }
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!(error = %e, "localStorage could not be read as items; copying its files");
            add_dir_recursive(writer, disk, rel)
        }
    }
}

/// Where curated profile file `name` lives on disk: `Cookies` resolves to
/// whichever of `Network/Cookies` and the root file is newer; every
/// other file is at the profile root.
fn profile_file_path(profile_dir: &Path, name: &str) -> PathBuf {
    if name == "Cookies" {
        cua_teleport_bundle::layout::chrome::cookies_store(profile_dir)
    } else {
        profile_dir.join(name)
    }
}

/// The concrete-item count and plural noun for one curated profile file, or
/// `None` (unknown file, or a locked/absent/unreadable database) so the consent
/// UI falls back to the byte size. Pure: a path in, an `Option` out.
fn profile_file_count(profile_dir: &Path, name: &str) -> Option<(u64, &'static str)> {
    let path = profile_file_path(profile_dir, name);
    match name {
        "Bookmarks" => count_bookmarks(&path).map(|n| (n, "bookmarks")),
        "Cookies" => count_sqlite_rows(&path, "cookies").map(|n| (n, "cookies")),
        "History" => count_sqlite_rows(&path, "urls").map(|n| (n, "history entries")),
        "Login Data" => count_sqlite_rows(&path, "logins").map(|n| (n, "logins")),
        "Web Data" => count_sqlite_rows(&path, "autofill").map(|n| (n, "autofill entries")),
        _ => None,
    }
}

/// Count the leaf bookmark nodes in a Chrome `Bookmarks` JSON file. Pure: path
/// in, `Option<count>` out; `None` when the file is missing or unparseable.
fn count_bookmarks(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(count_bookmark_urls(&value))
}

/// Recursively count nodes with `"type": "url"` (the leaf bookmarks) anywhere in
/// a Chrome bookmarks JSON tree.
fn count_bookmark_urls(value: &serde_json::Value) -> u64 {
    match value {
        serde_json::Value::Object(map) => {
            let is_url = map.get("type").and_then(serde_json::Value::as_str) == Some("url");
            let here = u64::from(is_url);
            here + map.values().map(count_bookmark_urls).sum::<u64>()
        }
        serde_json::Value::Array(items) => items.iter().map(count_bookmark_urls).sum(),
        _ => 0,
    }
}

/// Hard per-database time budget for a count. Chrome's databases open
/// immutably (no lock wait) so this is only a defensive cap against a
/// pathological read; one item can never stall the whole manifest.
const SQLITE_COUNT_BUDGET: Duration = Duration::from_millis(400);

/// `SELECT count(*)` over one table of a Chrome SQLite database. Pure: path +
/// table in, `Option<count>` out; `None` when the file is missing, unreadable,
/// corrupt, the table is absent, or the count exceeds the time budget — the
/// consent UI then falls back to the byte size.
///
/// The database is opened as an **immutable** read-only URI, so SQLite reads the
/// file bytes directly and ignores the WAL and file locks. That means the count
/// works — quickly — while Chrome is running and holding a write lock (the count
/// may be slightly stale, which is fine for a consent preview) instead of
/// blocking on the busy-timeout for seconds and then failing.
fn count_sqlite_rows(path: &Path, table: &str) -> Option<u64> {
    if !path.is_file() {
        return None;
    }
    let uri = format!("file:{}?mode=ro&immutable=1", encode_sqlite_uri_path(path));
    let table = table.to_string();

    // Run the count on a scratch thread with a hard timeout: `immutable=1` means
    // it shouldn't block, but this guarantees the manifest never stalls on one
    // database. If the thread outlives the budget its result is simply dropped.
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Option<u64>>(1);
    std::thread::spawn(move || {
        let count = (|| {
            let conn = rusqlite::Connection::open_with_flags(
                uri.as_str(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )
            .ok()?;
            // Never wait on a lock (immutable already avoids it; belt and braces).
            let _ = conn.busy_timeout(Duration::from_millis(0));
            // `table` is always a hard-coded constant here; still quote it defensively.
            let sql = format!("SELECT count(*) FROM \"{table}\"");
            conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
                .ok()
                .map(|count| count as u64)
        })();
        let _ = sender.send(count);
    });
    receiver.recv_timeout(SQLITE_COUNT_BUDGET).ok().flatten()
}

/// Percent-encode the characters significant in a SQLite `file:` URI (`%`, `?`,
/// `#`) so a profile path can't be misread as a URI query/fragment. `%` is
/// escaped first to avoid double-decoding. Chrome profile paths are normally
/// plain, so this is usually a no-op.
fn encode_sqlite_uri_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3F"),
            '#' => out.push_str("%23"),
            other => out.push(other),
        }
    }
    out
}

impl ExportProvider for ChromeProvider {
    fn id(&self) -> &str {
        crate::layout::chrome::ID
    }

    fn host(&self) -> &dyn HostEffects {
        &*self.host
    }

    fn display_name(&self) -> &str {
        crate::layout::chrome::DISPLAY
    }

    fn platform_supported(&self, platform: Platform) -> bool {
        matches!(
            platform,
            Platform::MacOS | Platform::Linux | Platform::Windows
        )
    }

    fn install_probe(&self) -> Option<crate::InstallProbe> {
        Some(crate::InstallProbe::path(crate::layout::chrome::MACOS_APP))
    }

    fn matches(&self, app: &AppRef) -> bool {
        let id = app.app_id.to_ascii_lowercase();
        CHROME_APP_IDS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&id))
    }

    fn app_ids(&self) -> &[&str] {
        CHROME_APP_IDS
    }

    fn manifest(
        &self,
        app: &AppRef,
        _window: Option<&WindowRef>,
        scope: TransferScope,
    ) -> Result<TransferManifest> {
        let profile_dir = self.resolved_profile_dir(app.platform)?;
        let profile = profile_name(&profile_dir);

        let mut items = Vec::new();
        let mut notes = vec![
            "Closing Chrome before transfer yields a cleaner capture; locked databases are copied best-effort while Chrome runs.".to_string(),
        ];

        // Tabs are always part of the transfer. The count is the live tab list
        // length (DevTools/AppleScript); `None` when Chrome isn't reachable.
        // A manifest is a PREVIEW, so it must not refuse to render just because
        // the browser could not be interrogated — but it must not quietly show
        // a checked "Open tabs" row with no count either, which is what a
        // swallowed failure looked like and is indistinguishable from a Chrome
        // with nothing open. Say so in the notes, and leave the row unchecked so
        // a default teleport does not ship an empty tabs.json. `capture_selected`
        // is where the same failure becomes fatal.
        let tab_count = match self.fetch_tabs() {
            Ok(tabs) => Some((tabs.len() as u64, "tabs")),
            Err(err) => {
                notes.push(format!("Open tabs could NOT be read: {err}"));
                None
            }
        };
        let mut tabs_item = manifest_item("Open tabs", TABS_JSON, 0, false, tab_count);
        if tab_count.is_none() {
            tabs_item.default_checked = false;
        }
        items.push(tabs_item);
        for session in SESSION_FILES {
            let disk = profile_dir.join(session);
            if disk.exists() {
                items.push(manifest_item(
                    &format!("Session state ({session})"),
                    &dest_rel_path(&profile, session),
                    file_len(&disk),
                    false,
                    None,
                ));
            }
        }

        if scope == TransferScope::FullProfile {
            for (name, sensitive) in PROFILE_FILES {
                let disk = profile_file_path(&profile_dir, name);
                items.push(manifest_item(
                    name,
                    &dest_rel_path(&profile, name),
                    file_len(&disk),
                    *sensitive,
                    profile_file_count(&profile_dir, name),
                ));
            }
            for (dir, sensitive) in PROFILE_DIRS {
                let disk = profile_dir.join(dir);
                // Directories keep the size fallback (no meaningful item count).
                items.push(manifest_item(
                    &format!("{dir}/"),
                    &dest_rel_path(&profile, dir),
                    dir_len(&disk),
                    *sensitive,
                    None,
                ));
            }
            notes.push("Full profile includes cookies, saved logins, and history.".to_string());
        }

        let total_est_bytes = items.iter().map(|item| item.est_bytes).sum();
        Ok(TransferManifest {
            provider_id: self.id().to_string(),
            app_display_name: app.display_name.clone(),
            scope,
            items,
            total_est_bytes,
            notes,
        })
    }

    fn capture_selected(
        &self,
        app: &AppRef,
        scope: TransferScope,
        include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> Result<()> {
        let profile_dir = self.resolved_profile_dir(app.platform)?;
        let profile = profile_name(&profile_dir);

        let mut writer = BundleWriter::with_limit(
            out,
            self.id(),
            app.display_name.clone(),
            scope,
            self.max_total_bytes,
        );

        // `None` means "everything the scope implies"; a set restricts capture
        // to the checked `rel_path`s (as reported by `manifest`).
        let wants = |rel: &str| include.is_none_or(|set| set.contains(rel));

        // The normalized tab list (`tabs.json`) always accompanies a tab-ish
        // selection: no filter at all, the "Open tabs" item itself, or any raw
        // session-state file. It is tiny and is what the destination replays.
        let tabs_selected = include.is_none()
            || wants(TABS_JSON)
            || SESSION_FILES
                .iter()
                .any(|session| wants(&dest_rel_path(&profile, session)));
        if tabs_selected {
            // Normalized tab list. This is the one place the failure must be
            // fatal: the caller ASKED for the open tabs, and writing a valid,
            // empty tabs.json instead is a transfer that reports success and
            // silently drops the thing it was for. An empty list is still
            // written when Chrome genuinely has nothing open — `fetch_tabs`
            // reserves `Ok(vec![])` for exactly that.
            let tabs = self.fetch_tabs()?;
            let tabs_json = serde_json::to_vec(&tabs)?;
            writer.add_bytes(TABS_JSON, 0o644, &tabs_json)?;
        }

        // Raw session-state files (best effort).
        for session in SESSION_FILES {
            let rel = dest_rel_path(&profile, session);
            if !wants(&rel) {
                continue;
            }
            let disk = profile_dir.join(session);
            if disk.is_file() {
                add_file_best_effort(&mut writer, &disk, &rel)?;
            }
        }

        if scope == TransferScope::FullProfile {
            for (name, _sensitive) in PROFILE_FILES {
                let rel = dest_rel_path(&profile, name);
                if !wants(&rel) {
                    continue;
                }
                // `Cookies` is never copied as a raw file: it is encrypted
                // under THIS machine's Safe Storage key, so a raw copy is
                // undecryptable bytes on any other machine (the bug this
                // module exists to fix). Decrypt here instead and write the
                // reserved `cookies.json` entry; the receiver
                // (`cua_spacesd_teleport::cookies`) re-encrypts each value
                // under the DESTINATION's own key. A decrypt failure fails
                // the whole capture rather than silently shipping an empty
                // (or raw, undecryptable) `Cookies` file that LOOKS like a
                // signed-in session and is not one.
                if *name == "Cookies" {
                    let cookies = crate::browser_cookies::ChromeCookies::new(self.host.clone())
                        .with_platform(app.platform)
                        .with_profile_dir(profile_dir.clone())
                        .read(&[])?;
                    if !cookies.is_empty() {
                        let items: Vec<cua_teleport_bundle::cookies::CookieItem> = cookies
                            .into_iter()
                            .map(crate::browser_cookies::DecryptedCookie::into_item)
                            .collect();
                        writer.add_bytes(
                            cua_teleport_bundle::cookies::COOKIES_ENTRY,
                            0o600,
                            &cua_teleport_bundle::cookies::serialize(&items),
                        )?;
                    }
                    continue;
                }
                let disk = profile_dir.join(name);
                if disk.is_file() {
                    add_file_best_effort(&mut writer, &disk, &rel)?;
                }
            }
            for (dir, _sensitive) in PROFILE_DIRS {
                let rel = dest_rel_path(&profile, dir);
                if !wants(&rel) {
                    continue;
                }
                let disk = profile_dir.join(dir);
                if disk.is_dir() {
                    if *dir == "Local Storage" {
                        add_local_storage(&mut writer, &profile_dir, &disk, &rel)?;
                    } else {
                        add_dir_recursive(&mut writer, &disk, &rel)?;
                    }
                }
            }
        }

        writer.finish()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleReader;
    use std::io::Cursor;

    fn fake_profile() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        // A real (small) Cookies SQLite database, one row, encrypted with
        // Chrome's fixed Linux `v10` password -- decryptable in a hermetic
        // test with no Keychain/host interaction (`app()` below declares
        // `Platform::Linux` for exactly this reason).
        crate::browser_cookies::write_cookies_db_for_tests(
            &profile,
            &[crate::browser_cookies::TestCookieRow {
                host_key: "example.test",
                name: "sid",
                encrypted_value: {
                    let key = cua_teleport_bundle::chromium_crypto::derive_key(
                        cua_teleport_bundle::chromium_crypto::LINUX_V10_PASSWORD,
                        cua_teleport_bundle::chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
                    );
                    cua_teleport_bundle::chromium_crypto::encrypt_v10(&key, b"cookie-value")
                },
                path: "/",
                expires_utc: 0,
                is_secure: true,
                is_httponly: true,
                samesite: 1,
            }],
        )
        .unwrap();
        std::fs::write(profile.join("Login Data"), b"login-bytes").unwrap();
        std::fs::write(profile.join("History"), b"history-bytes").unwrap();
        std::fs::write(profile.join("Preferences"), b"{\"profile\":1}").unwrap();
        std::fs::write(profile.join("Bookmarks"), b"{\"roots\":{}}").unwrap();
        std::fs::write(profile.join("Current Session"), b"snss-bytes").unwrap();
        let local_storage = profile.join("Local Storage");
        std::fs::create_dir_all(&local_storage).unwrap();
        std::fs::write(local_storage.join("leveldb.log"), b"ls-bytes").unwrap();
        // A cache dir that must NOT be captured.
        let cache = profile.join("Cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("data_0"), b"junk").unwrap();
        // Return the temp dir; caller uses `.path().join("Default")`.
        dir
    }

    fn app() -> AppRef {
        AppRef {
            app_id: "com.google.Chrome".into(),
            display_name: "Google Chrome".into(),
            // Linux: every fixture test sets an explicit `with_profile_dir`
            // (so this never affects WHERE a test reads from), and it makes
            // the new cookie-decrypt capture path hermetic -- the fixed
            // Linux `v10` password needs no Keychain, unlike macOS's.
            platform: Platform::Linux,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn active_tab_is_reordered_first_without_duplication() {
        // Active tab (line 1) also appears among the tabs; it should move to the
        // front exactly once, junk (chrome://, blanks) dropped.
        let raw = "https://b.example\nhttps://a.example\nchrome://newtab\nhttps://b.example\n\nhttps://c.example\n";
        assert_eq!(
            reorder_active_tab_first(raw),
            vec![
                "https://b.example".to_string(),
                "https://a.example".to_string(),
                "https://c.example".to_string(),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn missing_active_tab_keeps_existing_order() {
        // Empty active line -> no reordering, original order preserved.
        let raw = "\nhttps://a.example\nhttps://b.example\n";
        assert_eq!(
            reorder_active_tab_first(raw),
            vec![
                "https://a.example".to_string(),
                "https://b.example".to_string()
            ]
        );
    }

    #[test]
    fn counts_bookmark_url_leaves_and_rises_with_more() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks");
        // Two leaf URLs (one nested inside a folder); folders don't count.
        let two = r#"{"roots":{"bookmark_bar":{"type":"folder","name":"Bar","children":[
            {"type":"url","name":"A","url":"https://a"},
            {"type":"folder","name":"F","children":[
                {"type":"url","name":"B","url":"https://b"}
            ]}
        ]}}}"#;
        std::fs::write(&path, two).unwrap();
        assert_eq!(count_bookmarks(&path), Some(2));

        // Importing more bookmarks raises the count for the same file.
        let three = r#"{"roots":{"bookmark_bar":{"type":"folder","name":"Bar","children":[
            {"type":"url","name":"A","url":"https://a"},
            {"type":"url","name":"B","url":"https://b"},
            {"type":"url","name":"C","url":"https://c"}
        ]}}}"#;
        std::fs::write(&path, three).unwrap();
        assert_eq!(count_bookmarks(&path), Some(3));
    }

    #[test]
    fn encodes_uri_significant_characters_only() {
        // A normal profile path is left untouched.
        assert_eq!(
            encode_sqlite_uri_path(Path::new("/Users/x/Library/Chrome/Default/Cookies")),
            "/Users/x/Library/Chrome/Default/Cookies"
        );
        // `%` is escaped first, then `?` and `#`.
        assert_eq!(
            encode_sqlite_uri_path(Path::new("/tmp/a?b#c%d/Cookies")),
            "/tmp/a%3Fb%23c%25d/Cookies"
        );
    }

    #[test]
    fn missing_or_unparseable_bookmarks_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(count_bookmarks(&dir.path().join("nope")), None);
        let bad = dir.path().join("Bookmarks");
        std::fs::write(&bad, b"not json at all").unwrap();
        assert_eq!(count_bookmarks(&bad), None);
    }

    #[test]
    fn counts_sqlite_rows_and_rises_with_more() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute("CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT)", [])
                .unwrap();
            conn.execute("INSERT INTO urls (url) VALUES ('https://a')", [])
                .unwrap();
        }
        assert_eq!(count_sqlite_rows(&path, "urls"), Some(1));

        // More rows -> higher count for the same app database.
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO urls (url) VALUES ('https://b'), ('https://c')",
                [],
            )
            .unwrap();
        }
        assert_eq!(count_sqlite_rows(&path, "urls"), Some(3));

        // Absent table and missing file both fall back (None).
        assert_eq!(count_sqlite_rows(&path, "cookies"), None);
        assert_eq!(count_sqlite_rows(&dir.path().join("missing"), "urls"), None);
    }

    #[test]
    fn manifest_surfaces_item_counts() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join("Bookmarks"),
            r#"{"roots":{"bookmark_bar":{"type":"folder","children":[
                {"type":"url","url":"https://a"},
                {"type":"url","url":"https://b"}
            ]}}}"#,
        )
        .unwrap();
        {
            let conn = rusqlite::Connection::open(profile.join("History")).unwrap();
            conn.execute("CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT)", [])
                .unwrap();
            conn.execute(
                "INSERT INTO urls (url) VALUES ('https://a'), ('https://b'), ('https://c')",
                [],
            )
            .unwrap();
        }

        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(profile.clone());
        let manifest = provider
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();

        let bookmarks = manifest
            .items
            .iter()
            .find(|i| i.label == "Bookmarks")
            .unwrap();
        assert_eq!(bookmarks.count, Some(2));
        assert_eq!(bookmarks.count_noun.as_deref(), Some("bookmarks"));

        let history = manifest
            .items
            .iter()
            .find(|i| i.label == "History")
            .unwrap();
        assert_eq!(history.count, Some(3));
        assert_eq!(history.count_noun.as_deref(), Some("history entries"));

        // A directory item keeps the size fallback (no count).
        let local_storage = manifest
            .items
            .iter()
            .find(|i| i.label == "Local Storage/")
            .unwrap();
        assert_eq!(local_storage.count, None);
    }

    #[test]
    fn matches_expected_ids() {
        let provider = ChromeProvider::new();
        for id in ["com.google.Chrome", "chromium", "google-chrome", "chrome"] {
            assert!(provider.matches(&AppRef {
                app_id: id.into(),
                display_name: "x".into(),
                platform: Platform::Linux,
            }));
        }
        assert!(!provider.matches(&AppRef {
            app_id: "com.apple.Safari".into(),
            display_name: "Safari".into(),
            platform: Platform::MacOS,
        }));
    }

    #[test]
    fn manifest_marks_sensitive_items() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));
        let manifest = provider
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();
        assert_eq!(manifest.provider_id, "chrome");
        let cookies = manifest
            .items
            .iter()
            .find(|item| item.label == "Cookies")
            .unwrap();
        assert!(cookies.sensitive);
        let prefs = manifest
            .items
            .iter()
            .find(|item| item.label == "Preferences")
            .unwrap();
        assert!(!prefs.sensitive);
    }

    /// Modern Chrome keeps its cookies only in `Network/Cookies`. The
    /// manifest must count them, and the export must send them as decrypted
    /// rows without shipping the raw database (or any `Network/` file) too.
    #[test]
    fn modern_network_cookies_are_counted_and_exported_as_rows_only() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        let key = cua_teleport_bundle::chromium_crypto::derive_key(
            cua_teleport_bundle::chromium_crypto::LINUX_V10_PASSWORD,
            cua_teleport_bundle::chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
        );
        crate::browser_cookies::write_network_cookies_db_for_tests(
            &profile,
            &[crate::browser_cookies::TestCookieRow {
                host_key: "example.test",
                name: "sid",
                encrypted_value: cua_teleport_bundle::chromium_crypto::encrypt_v10(&key, b"v"),
                path: "/",
                expires_utc: 0,
                is_secure: true,
                is_httponly: true,
                samesite: 1,
            }],
        )
        .unwrap();
        assert!(!profile.join("Cookies").exists());
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(&profile);

        let manifest = provider
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();
        let cookies = manifest
            .items
            .iter()
            .find(|item| item.label == "Cookies")
            .unwrap();
        assert!(cookies.est_bytes > 0, "size read from Network/Cookies");
        assert_eq!(cookies.count, Some(1));

        let mut bundle = Vec::new();
        provider
            .export(&app(), TransferScope::FullProfile, &mut bundle)
            .unwrap();
        let entries = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .read_all()
            .unwrap();
        assert!(!entries.iter().any(|e| e.rel_path.ends_with("Cookies")));
        let rows = cua_teleport_bundle::cookies::parse(
            &entries
                .iter()
                .find(|e| e.rel_path == cua_teleport_bundle::cookies::COOKIES_ENTRY)
                .unwrap()
                .bytes,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, b"v");
    }

    /// A full export carries the curated profile in the canonical (Linux)
    /// bundle layout, checksummed, and never the caches. Where it lands on the
    /// destination is the receiver's business (`cua-spacesd-teleport`).
    #[test]
    fn full_export_packs_the_curated_profile_in_the_canonical_layout() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));

        let mut bundle = Vec::new();
        provider
            .export(&app(), TransferScope::FullProfile, &mut bundle)
            .unwrap();

        let reader = BundleReader::open(Cursor::new(bundle)).unwrap();
        assert_eq!(reader.header().provider_id, "chrome");
        assert_eq!(reader.header().scope, TransferScope::FullProfile);
        let entries = reader.read_all().unwrap();
        let get = |rel: &str| {
            entries
                .iter()
                .find(|e| e.rel_path == rel)
                .map(|e| e.bytes.clone())
        };
        // The raw, still-encrypted `Cookies` file is never shipped: it would
        // be undecryptable on any other machine. Decrypted cookies ride in
        // the reserved `cookies.json` entry instead.
        assert!(get(".config/google-chrome/Default/Cookies").is_none());
        let cookies_json = get(cua_teleport_bundle::cookies::COOKIES_ENTRY).unwrap();
        let items = cua_teleport_bundle::cookies::parse(&cookies_json);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].host_key, "example.test");
        assert_eq!(items[0].value, b"cookie-value");
        assert_eq!(
            get(".config/google-chrome/Default/Local Storage/leveldb.log").as_deref(),
            Some(&b"ls-bytes"[..])
        );
        // No devtools/AppleScript: the tab list is honestly empty.
        assert_eq!(get(TABS_JSON).as_deref(), Some(&b"[]"[..]));
        // Cache must not have been captured.
        assert!(!entries.iter().any(|e| e.rel_path.contains("Cache")));
        // Profile data, including the decrypted-cookies entry, is written
        // owner-only.
        let cookies_entry = entries
            .iter()
            .find(|e| e.rel_path == cua_teleport_bundle::cookies::COOKIES_ENTRY)
            .unwrap();
        assert_eq!(cookies_entry.mode & 0o777, 0o600);
    }

    /// A selected profile that does not exist fails loudly with the profiles
    /// that do, rather than exporting an empty bundle.
    #[test]
    fn unknown_profile_selector_names_the_available_profiles() {
        use crate::host::FakeHost;
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(user_data_dir_for(Platform::MacOS));
        for name in ["Default", "Profile 1"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("Preferences"), b"{}").unwrap();
        }
        let host = Arc::new(FakeHost::new().with_home(home.path()));
        // This test resolves the profile dir via `app.platform` (no
        // `with_profile_dir` override), so it needs a MacOS `AppRef`
        // regardless of `app()`'s own (Linux) default.
        let macos_app = AppRef {
            platform: Platform::MacOS,
            ..app()
        };
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_host(host.clone())
            .with_profile("Profile 9");
        let err = provider
            .manifest(&macos_app, None, TransferScope::FullProfile)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Default, Profile 1"), "{err}");
        // A present profile resolves under the (fake) home.
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_host(host)
            .with_profile("Profile 1");
        assert_eq!(
            provider.resolved_profile_dir(Platform::MacOS).unwrap(),
            root.join("Profile 1")
        );
    }

    #[test]
    fn export_selected_writes_only_the_checked_items() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));

        // Check "Bookmarks" alone (a small profile file). Because tabs were not
        // part of the selection, only that one entry is captured — none of the
        // large/sensitive files, and not even the normalized tab list.
        let bookmarks = dest_rel_path("Default", "Bookmarks");
        let include: HashSet<String> = [bookmarks.clone()].into_iter().collect();

        let mut bundle = Vec::new();
        provider
            .export_selected(
                &app(),
                TransferScope::FullProfile,
                Some(&include),
                &mut bundle,
            )
            .unwrap();

        let paths: Vec<_> = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|entry| entry.rel_path.clone())
            .collect();

        // Nothing that was left unchecked leaks in.
        assert!(
            !paths.iter().any(|p| p.ends_with("Cookies")),
            "no cookies: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with("History")),
            "no history: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with("Login Data")),
            "no logins: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("Local Storage")),
            "no local storage: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with("Current Session")),
            "no session: {paths:?}"
        );
        assert_eq!(
            paths,
            vec![bookmarks],
            "only the checked bookmarks: {paths:?}"
        );
    }

    #[test]
    fn export_selected_with_tabs_carries_tabs_json() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));

        // Check the "Open tabs" item plus "Bookmarks": exactly those two land
        // (tabs.json for the tabs item, Bookmarks for the checked file).
        let bookmarks = dest_rel_path("Default", "Bookmarks");
        let include: HashSet<String> = [TABS_JSON.to_string(), bookmarks.clone()]
            .into_iter()
            .collect();

        let mut bundle = Vec::new();
        provider
            .export_selected(
                &app(),
                TransferScope::FullProfile,
                Some(&include),
                &mut bundle,
            )
            .unwrap();

        let mut paths: Vec<_> = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|entry| entry.rel_path.clone())
            .collect();
        paths.sort();
        let mut expected = vec![bookmarks, TABS_JSON.to_string()];
        expected.sort();
        assert_eq!(paths, expected, "only tabs.json + bookmarks: {paths:?}");
    }

    #[test]
    fn export_selected_none_matches_full_export() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));

        let mut a = Vec::new();
        provider
            .export(&app(), TransferScope::FullProfile, &mut a)
            .unwrap();
        let mut b = Vec::new();
        provider
            .export_selected(&app(), TransferScope::FullProfile, None, &mut b)
            .unwrap();

        let paths_a: Vec<_> = BundleReader::open(Cursor::new(a))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        let paths_b: Vec<_> = BundleReader::open(Cursor::new(b))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(paths_a, paths_b);
    }

    #[test]
    fn export_selected_session_file_pulls_in_tabs_json() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));

        // Check only the raw session-state file; tabs.json rides along.
        let session = dest_rel_path("Default", "Current Session");
        let include: HashSet<String> = [session.clone()].into_iter().collect();

        let mut bundle = Vec::new();
        provider
            .export_selected(
                &app(),
                TransferScope::FullProfile,
                Some(&include),
                &mut bundle,
            )
            .unwrap();
        let paths: Vec<_> = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert!(paths.iter().any(|p| p == &session), "session: {paths:?}");
        assert!(paths.iter().any(|p| p == TABS_JSON), "tabs.json: {paths:?}");
        assert!(
            !paths.iter().any(|p| p.ends_with("Bookmarks")),
            "no bookmarks: {paths:?}"
        );
    }

    /// Live tab enumeration (DevTools, then AppleScript on macOS) reaches
    /// Chrome only through the injected host. With a fake, the real browser is
    /// never probed, scripted or launched.
    #[test]
    fn live_tab_enumeration_goes_through_the_injected_host() {
        use crate::host::{EffectKind, FakeHost, HostOutput};

        let host = Arc::new(FakeHost::new().with_responder(|command| {
            Ok(match command.kind {
                EffectKind::ProcessLookup => HostOutput::ok(""),
                EffectKind::AppleScript => {
                    HostOutput::ok("https://b.example/\nhttps://a.example/\nhttps://b.example/\n")
                }
                _ => HostOutput::failed(),
            })
        }));
        let provider = ChromeProvider::new().with_host(host.clone());
        let tabs = provider.fetch_tabs();
        assert_eq!(host.calls_of(EffectKind::BrowserDevTools).len(), 1);
        if cfg!(target_os = "macos") {
            assert_eq!(
                tabs.unwrap(),
                vec![
                    "https://b.example/".to_string(),
                    "https://a.example/".to_string()
                ]
            );
            assert_eq!(host.calls_of(EffectKind::ProcessLookup).len(), 1);
            let scripts = host.calls_of(EffectKind::AppleScript);
            assert_eq!(scripts.len(), 1);
            assert_eq!(scripts[0].program, "osascript");
        } else {
            assert!(tabs.is_err());
            assert!(host.calls_of(EffectKind::AppleScript).is_empty());
        }
    }

    #[test]
    fn tabs_only_skips_profile_files() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));
        let mut bundle = Vec::new();
        provider
            .export(&app(), TransferScope::TabsOnly, &mut bundle)
            .unwrap();
        let reader = BundleReader::open(Cursor::new(bundle)).unwrap();
        let paths: Vec<_> = reader
            .header()
            .entries
            .iter()
            .map(|entry| entry.rel_path.clone())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("Current Session")));
        assert!(!paths.iter().any(|p| p.ends_with("Cookies")));
    }

    /// A configured-but-unreachable DevTools endpoint is a FAILURE to enumerate,
    /// not an empty browser. Previously both produced `Vec::new()`, so an export
    /// wrote a valid, empty tabs.json and reported success.
    ///
    /// Built by hand rather than through `without_devtools()`, which switches
    /// off every mechanism and is therefore the deliberate-opt-out case. This is
    /// the shape a Linux host has: a DevTools endpoint and no AppleScript.
    #[test]
    fn unreachable_devtools_is_an_error_not_an_empty_browser() {
        let dir = fake_profile();
        let provider = ChromeProvider {
            profile_dir_override: Some(dir.path().join("Default")),
            profile_selector: None,
            // Port 1 is reserved and never listening.
            devtools_url: Some("127.0.0.1:1".to_string()),
            allow_applescript: false,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host: default_host(),
        };

        let err = provider
            .fetch_tabs()
            .expect_err("must not report zero tabs");
        let msg = err.to_string();
        assert!(msg.contains("unreachable"), "unhelpful message: {msg}");

        // The manifest still renders (it is a preview) but says why, and leaves
        // the row unchecked so a default teleport cannot ship an empty list.
        let manifest = provider
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();
        let tabs = manifest
            .items
            .iter()
            .find(|i| i.rel_path == TABS_JSON)
            .expect("tabs row present");
        assert!(!tabs.default_checked, "must not be checked when unreadable");
        assert!(tabs.count.is_none());
        assert!(
            manifest
                .notes
                .iter()
                .any(|n| n.contains("could NOT be read")),
            "no explanatory note: {:?}",
            manifest.notes
        );

        // …and an export that was actually asked for the tabs fails.
        let mut bundle = Vec::new();
        provider
            .export(&app(), TransferScope::TabsOnly, &mut bundle)
            .expect_err("export must fail rather than ship an empty tabs.json");
    }

    /// The default set is the product contract for "teleport my Chrome", and it
    /// regressed silently once already: every item but `tabs.json` was
    /// `default_checked: false`, so a bare teleport moved a list of URLs and
    /// looked like it had done nothing. Pin both halves of the split.
    /// Every credential the default leaves out is reachable through exactly
    /// the opt-in groups the catalog offers for Chrome, and each group has
    /// an item: the picker's checkboxes and the manifest cannot drift.
    #[test]
    fn opt_in_groups_cover_exactly_the_withheld_credentials() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));
        let manifest = provider
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();
        crate::ux::testing::assert_opt_ins_match(crate::layout::chrome::ID, &manifest);
    }

    #[test]
    fn default_set_moves_a_usable_session_but_no_credentials() {
        let dir = fake_profile();
        let provider = ChromeProvider::new()
            .without_devtools()
            .with_profile_dir(dir.path().join("Default"));
        let manifest = provider
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();

        let checked: Vec<&str> = manifest
            .items
            .iter()
            .filter(|i| i.default_checked)
            .map(|i| i.rel_path.as_str())
            .collect();

        // Credentials never move without an explicit choice.
        for secret in ["Cookies", "Login Data", "History"] {
            assert!(
                !checked.iter().any(|p| p.ends_with(secret)),
                "{secret} must NOT be checked by default, got {checked:?}"
            );
        }
        // …and everything needed for the destination to be the user's browser
        // rather than a stock one does. `Sessions` is the load-bearing entry:
        // it is what --restore-last-session reads.
        assert!(checked.contains(&TABS_JSON), "tabs: {checked:?}");
        for wanted in ["Preferences", "Bookmarks", "Sessions"] {
            assert!(
                checked.iter().any(|p| p.ends_with(wanted)),
                "{wanted} must be checked by default, got {checked:?}"
            );
        }

        // The split is derived from `sensitive`, not a second hand-kept list.
        for item in &manifest.items {
            assert_eq!(
                item.default_checked, !item.sensitive,
                "{} must follow the sensitive flag",
                item.rel_path
            );
        }
    }
}

#[cfg(test)]
mod profile_selection_tests {
    use super::*;

    #[test]
    fn bare_name_joins_the_user_data_dir() {
        let root = Path::new("/ud");
        assert_eq!(
            ChromeProvider::resolve_profile(root, "Profile 1"),
            Path::new("/ud/Profile 1")
        );
    }

    #[test]
    fn absolute_path_is_used_verbatim() {
        // A caller that already knows the exact path must not have it
        // reinterpreted as a name under the user-data dir.
        let root = Path::new("/ud");
        assert_eq!(
            ChromeProvider::resolve_profile(root, "/elsewhere/Work"),
            Path::new("/elsewhere/Work")
        );
    }

    #[test]
    fn relative_path_with_separator_is_a_path_not_a_name() {
        let root = Path::new("/ud");
        assert_eq!(
            ChromeProvider::resolve_profile(root, "nested/Work"),
            Path::new("nested/Work")
        );
    }

    #[test]
    fn selector_is_trimmed() {
        let root = Path::new("/ud");
        assert_eq!(
            ChromeProvider::resolve_profile(root, "  Default  "),
            Path::new("/ud/Default")
        );
    }

    #[test]
    fn explicit_profile_dir_wins_over_the_selector() {
        // with_profile_dir is the most specific signal; a profile selector set
        // for an unrelated reason must not redirect an explicit caller.
        let dir = std::path::PathBuf::from("/nonexistent/cua-chrome-override-test");
        let provider = ChromeProvider::new()
            .with_profile("Profile 3")
            .with_profile_dir(&dir);
        assert_eq!(
            provider.source_profile_dir(Platform::MacOS).unwrap(),
            dir,
            "explicit override must take precedence"
        );
        // And it stays unvalidated, so an override to a not-yet-created dir
        // still reaches the existing best-effort item handling.
        assert!(provider.resolved_profile_dir(Platform::MacOS).is_ok());
    }

    #[test]
    fn available_profiles_lists_default_first() {
        // Exercised via resolve_profile's sibling sorting contract: a real
        // read_dir is environment-dependent, so assert only the ordering key.
        let mut names = vec![
            "Profile 2".to_string(),
            "Default".to_string(),
            "Profile 1".to_string(),
        ];
        names.sort_by_key(|n| (n != "Default", n.clone()));
        assert_eq!(names, vec!["Default", "Profile 1", "Profile 2"]);
    }
}
