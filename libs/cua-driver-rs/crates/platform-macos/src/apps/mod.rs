//! macOS app enumeration via NSWorkspace and NSRunningApplication.

pub mod nsworkspace;

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

/// Launch an app by bundle ID via NSWorkspace, background only (no focus
/// steal). Returns the pid on success.
///
/// Replaces a prior `open -g -b` shell-out. The NSWorkspace path:
///   * honors `activates = false` so LaunchServices doesn't bring the
///     target frontmost,
///   * attaches an `aevt/oapp` AppleEvent descriptor so cold-launched
///     apps (Calculator, etc) get their window-creation handler invoked
///     reliably (the shell-out path silently skipped this for state-
///     restored apps),
///   * returns the actual `NSRunningApplication.processIdentifier`
///     without needing a separate `list_running_apps` lookup, so we
///     can't race a same-bundle-id helper that happens to be running.
pub fn launch_app(bundle_id: &str) -> anyhow::Result<i32> {
    let app_url = resolve_bundle_id_to_path(bundle_id).ok_or_else(|| {
        anyhow::anyhow!("Could not locate app with bundle_id '{bundle_id}'")
    })?;
    let cfg = nsworkspace::OpenConfig {
        apple_event_bundle_id: Some(bundle_id.to_owned()),
        ..Default::default()
    };
    let running = nsworkspace::open_application(&app_url, &cfg)
        .map_err(|e| anyhow::anyhow!("Failed to launch {bundle_id}: {e}"))?;
    let pid: i32 = unsafe { running.processIdentifier() };
    Ok(pid)
}

/// Launch an app by display name via NSWorkspace. Background-only.
/// Returns the pid on success.
///
/// Mirror of Swift `AppLauncher.locate(name:)`: scan the standard
/// roots for `<Name>.app`, then fall back to a LaunchServices lookup
/// in case the caller passed a bundle identifier in the `name` slot.
pub fn launch_app_by_name(name: &str) -> anyhow::Result<i32> {
    let app_url = locate_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("Could not locate app with name '{name}'"))?;
    let bid = bundle_id_for_app_path(&app_url);
    let cfg = nsworkspace::OpenConfig {
        apple_event_bundle_id: bid,
        ..Default::default()
    };
    let running = nsworkspace::open_application(&app_url, &cfg)
        .map_err(|e| anyhow::anyhow!("Failed to launch '{name}': {e}"))?;
    let pid: i32 = unsafe { running.processIdentifier() };
    Ok(pid)
}

/// Launch a bundle with URL handoff. Mirrors Swift's
/// `NSWorkspace.open(urls:withApplicationAt:configuration:)` flow.
///
/// `additional_args` and `env` are merged into the `OpenConfig`.
/// `creates_new_instance` corresponds to AppKit's
/// `createsNewApplicationInstance = true`.
pub fn launch_with_urls_by_bundle(
    bundle_id: &str,
    urls: &[String],
    additional_args: &[String],
    env: &std::collections::HashMap<String, String>,
    creates_new_instance: bool,
) -> anyhow::Result<i32> {
    let app_url = resolve_bundle_id_to_path(bundle_id).ok_or_else(|| {
        anyhow::anyhow!("Could not locate app with bundle_id '{bundle_id}'")
    })?;
    let cfg = nsworkspace::OpenConfig {
        arguments: additional_args.to_vec(),
        environment: env.clone(),
        creates_new_instance,
        apple_event_bundle_id: Some(bundle_id.to_owned()),
    };
    let running = if urls.is_empty() {
        nsworkspace::open_application(&app_url, &cfg)
    } else {
        nsworkspace::open_urls_with_application(urls, &app_url, &cfg)
    }
    .map_err(|e| anyhow::anyhow!("Failed to launch {bundle_id}: {e}"))?;
    let pid: i32 = unsafe { running.processIdentifier() };
    Ok(pid)
}

/// Launch by name with URL handoff. Same contract as
/// `launch_with_urls_by_bundle` but resolves the bundle URL by display
/// name first.
pub fn launch_with_urls_by_name(
    name: &str,
    urls: &[String],
    additional_args: &[String],
    env: &std::collections::HashMap<String, String>,
    creates_new_instance: bool,
) -> anyhow::Result<i32> {
    let app_url = locate_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("Could not locate app with name '{name}'"))?;
    let bid = bundle_id_for_app_path(&app_url);
    let cfg = nsworkspace::OpenConfig {
        arguments: additional_args.to_vec(),
        environment: env.clone(),
        creates_new_instance,
        apple_event_bundle_id: bid,
    };
    let running = if urls.is_empty() {
        nsworkspace::open_application(&app_url, &cfg)
    } else {
        nsworkspace::open_urls_with_application(urls, &app_url, &cfg)
    }
    .map_err(|e| anyhow::anyhow!("Failed to launch '{name}': {e}"))?;
    let pid: i32 = unsafe { running.processIdentifier() };
    Ok(pid)
}

// ── Bundle resolution ────────────────────────────────────────────────────────

/// Resolve a bundle id to an installed `.app` path via NSWorkspace.
/// Returns the absolute filesystem path (no `file://` prefix); callers
/// pass it back through `file_or_app_url` inside `nsworkspace::*`.
fn resolve_bundle_id_to_path(bundle_id: &str) -> Option<String> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let ns = NSString::from_str(bundle_id);
        let url = ws.URLForApplicationWithBundleIdentifier(&ns)?;
        // -[NSURL path] gives us the absolute filesystem path; convert
        // to UTF-8.
        let path = url.path()?;
        Some(path.to_string())
    }
}

/// Mirror of Swift's `AppLauncher.locate(name:)`.
///
/// 1. filesystem lookup by bundle filename in the canonical roots
///    (system first so /Applications wins over ~/Applications);
/// 2. LaunchServices bundle-id lookup, in case the caller passed a
///    bundle identifier in the `name` slot;
/// 3. (skipped) full localized-name scan — not yet needed by current
///    integration tests; can be added if we hit a non-English-name app
///    in the wild.
fn locate_by_name(name: &str) -> Option<String> {
    let app_name = if name.ends_with(".app") {
        name.to_owned()
    } else {
        format!("{name}.app")
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let roots = [
        "/Applications".to_owned(),
        "/System/Applications".to_owned(),
        "/System/Applications/Utilities".to_owned(),
        "/Applications/Utilities".to_owned(),
        format!("{home}/Applications"),
        format!("{home}/Applications/Chrome Apps.localized"),
    ];
    for root in &roots {
        let path = format!("{root}/{app_name}");
        if std::path::Path::new(&path).is_dir() {
            return Some(path);
        }
    }
    // Fallback: maybe caller passed a bundle id as `name`.
    if let Some(p) = resolve_bundle_id_to_path(name) {
        return Some(p);
    }
    None
}

/// Read `CFBundleIdentifier` from an `.app` bundle's `Info.plist`.
/// Falls back to shelling out to `plutil` (already used elsewhere in
/// this file) to avoid pulling in a plist crate just for this.
fn bundle_id_for_app_path(app_path: &str) -> Option<String> {
    let plist = format!("{app_path}/Contents/Info.plist");
    let out = Command::new("plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-", &plist])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let bid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if bid.is_empty() {
        None
    } else {
        Some(bid)
    }
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
