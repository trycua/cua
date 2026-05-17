//! macOS app enumeration via NSWorkspace and NSRunningApplication.

use std::process::Command;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub running: bool,
    pub active: bool,
}

/// Enumerate running apps with NSApplicationActivationPolicyRegular.
/// Uses `osascript` to query System Events — no Accessibility permission needed.
pub fn list_running_apps() -> Vec<AppInfo> {
    // Use `ps` + NSWorkspace via a brief Swift one-liner via swift-sh is heavy;
    // instead use the Obj-C bridge via `osascript`.
    // Fallback: parse `ps -axo pid,command` and cross-reference with bundle IDs.
    // Better: use a native call via core-foundation bindings.
    list_running_apps_native()
}

fn list_running_apps_native() -> Vec<AppInfo> {
    // Use `lsappinfo list` which is a macOS system tool available on all versions.
    // Alternatively we can use NSWorkspace via objc2 — but for simplicity and
    // to match the Swift reference, we use a subprocess call to `osascript`.
    let script = r#"
set output to ""
tell application "System Events"
    set appList to every application process whose background only is false
    repeat with proc in appList
        set procName to name of proc
        set procPid to unix id of proc
        set bundleId to bundle identifier of proc
        if bundleId is missing value then set bundleId to ""
        set isFront to "0"
        if frontmost of proc then set isFront to "1"
        set output to output & procName & "|" & procPid & "|" & bundleId & "|" & isFront & linefeed
    end repeat
end tell
return output
"#;

    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output();

    match out {
        Err(_) => vec![],
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            parse_osascript_app_list(&text)
        }
    }
}

fn parse_osascript_app_list(text: &str) -> Vec<AppInfo> {
    let mut apps = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() < 2 { continue; }
        let name = parts[0].trim().to_owned();
        let pid: i32 = parts[1].trim().parse().unwrap_or(0);
        let bundle_id = if parts.len() > 2 && !parts[2].trim().is_empty() {
            Some(parts[2].trim().to_owned())
        } else {
            None
        };
        let active = parts.get(3).map(|s| s.trim() == "1").unwrap_or(false);
        if name.is_empty() || pid == 0 { continue; }
        apps.push(AppInfo { name, pid, bundle_id, running: true, active });
    }
    apps
}

/// Launch an app by bundle ID using `open -g -b` (background, no activation).
/// Returns the pid on success.
pub fn launch_app(bundle_id: &str) -> anyhow::Result<i32> {
    let status = Command::new("open")
        .args(["-g", "-b", bundle_id])
        .status()?;
    if !status.success() {
        anyhow::bail!("Failed to launch {bundle_id}");
    }
    // Give the app a moment to start.
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Find its pid.
    let apps = list_running_apps();
    for app in &apps {
        if app.bundle_id.as_deref() == Some(bundle_id) {
            return Ok(app.pid);
        }
    }
    anyhow::bail!("Launched {bundle_id} but could not find its pid")
}

/// Launch an app by display name using `open -g -a AppName` (background, no activation).
/// Returns the pid on success.
///
/// Tries `locate_app_by_name` first so callers can pass bundle IDs
/// (`com.apple.calculator`), localized display names (`計算機`), or
/// case-insensitive variants (`CALCULATOR`). Falls back to a raw
/// `open -g -a <name>` if the resolver finds nothing, preserving the
/// previous behavior for inputs LaunchServices already recognizes.
pub fn launch_app_by_name(name: &str) -> anyhow::Result<i32> {
    // Try the 3-pass resolver first (mirrors Swift's AppLauncher.locate).
    if let Some(resolved) = locate_app_by_name(name) {
        // Prefer launching by bundle ID when we have one — it's unambiguous
        // and matches what the Swift implementation does via
        // NSWorkspace.open(URL, configuration:).
        if let Some(bundle_id) = resolved.bundle_id.as_deref() {
            return launch_app(bundle_id);
        }
        // Fall back to launching by path if no bundle ID is available.
        let status = Command::new("open")
            .args(["-g", &resolved.path])
            .status()?;
        if !status.success() {
            anyhow::bail!("Failed to launch app at '{}'", resolved.path);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let apps = list_running_apps();
        for app in &apps {
            // Bundle-ID match must be a *both-Some* equality — comparing
            // two `None`s would silently match an unrelated running app
            // whose bundle_id we also failed to resolve, returning a wrong
            // pid for the launch we just performed.
            let bundle_id_match = matches!(
                (app.bundle_id.as_deref(), resolved.bundle_id.as_deref()),
                (Some(a), Some(b)) if a == b
            );
            if app.name.eq_ignore_ascii_case(&resolved.display_name) || bundle_id_match {
                return Ok(app.pid);
            }
        }
        anyhow::bail!("Launched '{name}' but could not find its pid");
    }

    // Resolver came up empty — let `open -g -a` make its own attempt.
    // It can succeed for installed apps whose Info.plist metadata wasn't
    // visible to our scan (e.g. inside Chrome Apps.localized when the
    // bundle layout is non-standard).
    let status = Command::new("open")
        .args(["-g", "-a", name])
        .status()?;
    if !status.success() {
        anyhow::bail!("Could not locate app (name '{name}')");
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let apps = list_running_apps();
    for app in &apps {
        if app.name.eq_ignore_ascii_case(name) {
            return Ok(app.pid);
        }
    }
    anyhow::bail!("Launched '{name}' but could not find its pid")
}

/// Result of resolving a user-supplied app `name` to a concrete bundle.
#[derive(Debug, Clone)]
pub struct ResolvedApp {
    /// Absolute filesystem path to the `.app` bundle.
    pub path: String,
    /// Bundle identifier (e.g. `com.apple.calculator`) when discoverable.
    pub bundle_id: Option<String>,
    /// Best-effort display name (running localized name, CFBundleDisplayName,
    /// CFBundleName, or stem — in that priority order).
    pub display_name: String,
}

/// Resolve a user-supplied app `name` to an installed bundle using the
/// same three-pass chain as Swift `AppLauncher.locate`:
///
///   1. Filesystem `<name>.app` lookup in the canonical roots
///      (fast, locale-independent for English app names).
///   2. LaunchServices bundle-ID lookup — lets callers pass
///      `com.apple.calculator` as `name` without switching parameters.
///   3. Fuzzy scan:
///      a) Locale-aware `localizedName` from `NSRunningApplication`
///         (covers e.g. `計算機` on JP macOS for Calculator).
///      b) `CFBundleDisplayName` / `CFBundleName` from Info.plist
///         (case-insensitive English variants like `CALCULATOR`).
///      c) Bundle URL stem (filename minus `.app`).
///
/// Matching is case-insensitive throughout. Returns `None` if every pass
/// fails — callers should treat that as "not found" and surface an error.
///
/// Ports `libs/cua-driver/Sources/CuaDriverCore/Apps/AppLauncher.swift`
/// `AppLauncher.locate(bundleId:name:)` (PR trycua/cua#1492).
pub fn locate_app_by_name(name: &str) -> Option<ResolvedApp> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    // ── Pass 1 — Filesystem lookup by `<name>.app` ────────────────────────
    let app_filename = if name.to_lowercase().ends_with(".app") {
        name.to_string()
    } else {
        format!("{name}.app")
    };
    for root in app_search_roots().iter() {
        let path = format!("{root}/{app_filename}");
        if std::path::Path::new(&path).is_dir() {
            let (display_name, bundle_id) = read_bundle_metadata(&path);
            return Some(ResolvedApp {
                path,
                bundle_id,
                display_name,
            });
        }
    }

    // ── Pass 2 — LaunchServices bundle-ID lookup ──────────────────────────
    if let Some(path) = url_for_application_with_bundle_identifier(name) {
        let (display_name, bundle_id) = read_bundle_metadata(&path);
        // If we asked for `name` as a bundle ID and the bundle has no
        // CFBundleIdentifier in its Info.plist (unlikely but possible),
        // fall back to the queried string.
        let bundle_id = bundle_id.or_else(|| Some(name.to_string()));
        return Some(ResolvedApp {
            path,
            bundle_id,
            display_name,
        });
    }

    // ── Pass 3 — Fuzzy scan ──────────────────────────────────────────────
    let needle = name.to_lowercase();

    // 3a) Running apps: locale-aware `localizedName` from
    //     `NSRunningApplication`. Covers non-English locales without
    //     touching disk.
    if let Some(resolved) = find_running_app_by_localized_name(&needle) {
        return Some(resolved);
    }

    // 3b/c) Scan installed bundles in the same roots; match against
    //       CFBundleDisplayName → CFBundleName → stem (case-insensitive).
    for root in app_search_roots().iter() {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            let (display_name, bundle_id) = read_bundle_metadata(&path_str);
            if display_name.to_lowercase() == needle {
                return Some(ResolvedApp {
                    path: path_str,
                    bundle_id,
                    display_name,
                });
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if stem.to_lowercase() == needle {
                return Some(ResolvedApp {
                    path: path_str,
                    bundle_id,
                    display_name,
                });
            }
        }
    }

    None
}

/// The same set of canonical directories Swift's `AppLauncher.locate`
/// searches: system roots first (so `/Applications` wins over a same-name
/// copy in `~/Applications`), then user-local paths, then the
/// Chrome-PWA subfolder. No recursion.
fn app_search_roots() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        "/Applications".to_string(),
        "/System/Applications".to_string(),
        "/System/Applications/Utilities".to_string(),
        "/Applications/Utilities".to_string(),
        format!("{home}/Applications"),
        format!("{home}/Applications/Chrome Apps.localized"),
    ]
}

/// Read CFBundleDisplayName / CFBundleName / CFBundleIdentifier from a
/// bundle's Info.plist. Returns `(display_name, bundle_id)` where
/// `display_name` falls back to the on-disk stem when no plist keys exist.
fn read_bundle_metadata(app_path: &str) -> (String, Option<String>) {
    let plist_path = format!("{app_path}/Contents/Info.plist");
    let display_name = plutil_extract(&plist_path, "CFBundleDisplayName")
        .or_else(|| plutil_extract(&plist_path, "CFBundleName"))
        .unwrap_or_else(|| {
            std::path::Path::new(app_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string()
        });
    let bundle_id = plutil_extract(&plist_path, "CFBundleIdentifier");
    (display_name, bundle_id)
}

fn plutil_extract(plist_path: &str, key: &str) -> Option<String> {
    let out = Command::new("plutil")
        .args(["-extract", key, "raw", "-o", "-", plist_path])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Ask LaunchServices (via `NSWorkspace.URLForApplicationWithBundleIdentifier`)
/// for the path to the app registered for `bundle_id`. Returns `None` if
/// no such bundle is registered.
#[cfg(target_os = "macos")]
fn url_for_application_with_bundle_identifier(bundle_id: &str) -> Option<String> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    // SAFETY: `NSWorkspace.sharedWorkspace()` and
    // `URLForApplicationWithBundleIdentifier:` are thread-safe AppKit
    // APIs documented as callable from any thread.
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let ns_id = NSString::from_str(bundle_id);
        let url = workspace.URLForApplicationWithBundleIdentifier(&ns_id)?;
        let ns_path = url.path()?;
        Some(ns_path.to_string())
    }
}

/// Iterate running applications and return one whose
/// `localizedName` (locale-aware) matches `needle` case-insensitively.
/// `needle` must already be lowercased by the caller.
#[cfg(target_os = "macos")]
fn find_running_app_by_localized_name(needle: &str) -> Option<ResolvedApp> {
    use objc2_app_kit::NSWorkspace;

    // SAFETY: NSWorkspace.runningApplications snapshots the array on call;
    // accessing element properties is safe from any thread.
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let running = workspace.runningApplications();
        for i in 0..running.count() {
            let app = running.objectAtIndex(i);
            let Some(url) = app.bundleURL() else { continue };
            let Some(path) = url.path() else { continue };
            let Some(localized) = app.localizedName() else { continue };
            if localized.to_string().to_lowercase() != needle {
                continue;
            }
            let path_str = path.to_string();
            let (display_name, bundle_id) = read_bundle_metadata(&path_str);
            // Prefer the running app's bundleIdentifier when available —
            // it's authoritative and doesn't require a plist read.
            let bundle_id = app
                .bundleIdentifier()
                .map(|s| s.to_string())
                .or(bundle_id);
            return Some(ResolvedApp {
                path: path_str,
                bundle_id,
                display_name,
            });
        }
    }
    None
}

/// Return all apps: running apps merged with installed-but-not-running apps.
pub fn list_all_apps() -> Vec<AppInfo> {
    let running = list_running_apps();
    let running_bundles: std::collections::HashSet<String> = running.iter()
        .filter_map(|a| a.bundle_id.clone())
        .collect();

    let mut installed = scan_installed_apps();
    // Remove apps already in running list.
    installed.retain(|a| !a.bundle_id.as_ref().map_or(false, |b| running_bundles.contains(b)));

    let mut all = running;
    all.extend(installed);
    all
}

fn scan_installed_apps() -> Vec<AppInfo> {
    let dirs = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
        "/System/Applications/Utilities",
    ];
    let home = std::env::var("HOME").unwrap_or_default();
    let user_apps = format!("{home}/Applications");

    let mut result = Vec::new();
    let mut all_dirs: Vec<&str> = dirs.to_vec();
    let user_apps_str: &str = user_apps.as_str();
    all_dirs.push(user_apps_str);

    for dir in all_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") { continue }
            let plist_path = path.join("Contents/Info.plist");
            if let Some(info) = read_app_plist(&plist_path) {
                result.push(info);
            }
        }
    }
    result
}

fn read_app_plist(plist_path: &std::path::Path) -> Option<AppInfo> {
    let bundle_id_out = Command::new("plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-",
               plist_path.to_str()?])
        .output().ok()?;
    if !bundle_id_out.status.success() { return None; }
    let bundle_id = String::from_utf8_lossy(&bundle_id_out.stdout).trim().to_string();
    if bundle_id.is_empty() { return None; }

    let name_out = Command::new("plutil")
        .args(["-extract", "CFBundleDisplayName", "raw", "-o", "-",
               plist_path.to_str()?])
        .output().ok();
    let name = name_out
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            // Fallback: CFBundleName.
            Command::new("plutil")
                .args(["-extract", "CFBundleName", "raw", "-o", "-",
                       plist_path.to_str().unwrap_or("")])
                .output().ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    plist_path.parent()
                        .and_then(|p| p.parent())
                        .and_then(|p| p.file_stem())
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string()
                })
        });

    if name.is_empty() { return None; }

    Some(AppInfo {
        name,
        pid: 0,
        bundle_id: Some(bundle_id),
        running: false,
        active: false,
    })
}

/// Return the localized application name for a running process by PID.
/// Uses `ps -p {pid} -o comm=` which gives the command name without path.
/// Returns `None` if the PID is unknown or the command fails.
pub fn get_app_name_for_pid(pid: i32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // Strip path prefix: "/Applications/Safari.app/Contents/MacOS/Safari" → "Safari"
    Some(
        std::path::Path::new(&raw)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&raw)
            .to_string(),
    )
}

/// Format the app list in the same text style as libs/cua-driver.
pub fn format_app_list(apps: &[AppInfo]) -> String {
    let running: Vec<&AppInfo> = apps.iter().filter(|a| a.running).collect();
    let total = apps.len();
    // Match Swift `ListAppsTool.swift` `summary(_:)` text format 1:1.
    let mut lines = vec![format!(
        "✅ Found {} app(s): {} running, {} installed-not-running.",
        total,
        running.len(),
        total - running.len()
    )];
    for app in &running {
        let bundle = app.bundle_id.as_deref().map(|b| format!(" [{b}]")).unwrap_or_default();
        lines.push(format!("- {} (pid {}){}", app.name, app.pid, bundle));
    }
    lines.join("\n")
}
