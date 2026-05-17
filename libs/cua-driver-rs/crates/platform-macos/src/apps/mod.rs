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
    // Pass the bundle id straight through — `nsworkspace::resolve_application_url`
    // calls `URLForApplicationWithBundleIdentifier` and uses the resulting
    // NSURL verbatim. Going via a `path` string and back loses the
    // alias/cryptex metadata Safari (and other Cryptex-installed apps)
    // need to relaunch from `/System/Cryptexes/App/...`.
    let cfg = nsworkspace::OpenConfig {
        apple_event_bundle_id: Some(bundle_id.to_owned()),
        ..Default::default()
    };
    let running = nsworkspace::open_application(bundle_id, &cfg)
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
    let located = locate_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("Could not locate app with name '{name}'"))?;
    let (app_ref, bid) = located.app_ref_and_bundle_id();
    let cfg = nsworkspace::OpenConfig {
        apple_event_bundle_id: bid,
        ..Default::default()
    };
    let running = nsworkspace::open_application(&app_ref, &cfg)
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
    // Pass the bundle id directly — see `launch_app` rationale above
    // (Cryptex-installed apps).
    //
    // Only attach the `oapp` AppleEvent on the no-URL path. With URLs
    // present, the URL-handoff path delivers its own `aevt/odoc` to
    // the target and attaching `oapp` on top causes
    // openURLs:withApplicationAtURL: to bail with "application not
    // found" for Cryptex-installed apps (Safari). Verified empirically.
    let cfg = nsworkspace::OpenConfig {
        arguments: additional_args.to_vec(),
        environment: env.clone(),
        creates_new_instance,
        apple_event_bundle_id: if urls.is_empty() {
            Some(bundle_id.to_owned())
        } else {
            None
        },
    };
    let running = if urls.is_empty() {
        nsworkspace::open_application(bundle_id, &cfg)
    } else {
        nsworkspace::open_urls_with_application(urls, bundle_id, &cfg)
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
    let located = locate_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("Could not locate app with name '{name}'"))?;
    let (app_ref, bid) = located.app_ref_and_bundle_id();
    // See `launch_with_urls_by_bundle` — skip `oapp` AppleEvent on
    // the URL-handoff path.
    let cfg = nsworkspace::OpenConfig {
        arguments: additional_args.to_vec(),
        environment: env.clone(),
        creates_new_instance,
        apple_event_bundle_id: if urls.is_empty() { bid } else { None },
    };
    let running = if urls.is_empty() {
        nsworkspace::open_application(&app_ref, &cfg)
    } else {
        nsworkspace::open_urls_with_application(urls, &app_ref, &cfg)
    }
    .map_err(|e| anyhow::anyhow!("Failed to launch '{name}': {e}"))?;
    let pid: i32 = unsafe { running.processIdentifier() };
    Ok(pid)
}

// ── Bundle resolution ────────────────────────────────────────────────────────

/// What `locate_by_name` resolved a display name into.
///
/// Two shapes because Cryptex-installed apps (Safari on macOS Sonoma+,
/// and a growing set of other system apps) live under
/// `/System/Cryptexes/App/...` — flattening their LaunchServices NSURL
/// to a filesystem path via `-[NSURL path]` loses the
/// alias/cryptex metadata LaunchServices needs to relaunch the bundle.
/// The fix: when the resolver went through LaunchServices, hand the
/// bundle id back to the launch helpers verbatim — they pass it
/// straight through to `URLForApplicationWithBundleIdentifier` and
/// use the resulting NSURL unmodified.
pub(crate) enum AppLocator {
    /// Found by filesystem scan (`/Applications/...`, `~/Applications/...`).
    /// Path-based launch is safe here — these apps aren't Cryptex-installed.
    Path(String),
    /// Found via LaunchServices bundle-id lookup. Carry the bundle id
    /// (NOT the lossy `url.path()`) so the launch helpers re-resolve
    /// the live NSURL on demand and preserve cryptex metadata.
    BundleId(String),
}

impl AppLocator {
    /// `(app_ref_for_nsworkspace, optional_bundle_id_for_oapp_event)`.
    ///
    /// `app_ref` is the string the `nsworkspace::*` helpers consume —
    /// a filesystem path or a bundle id; either flows through
    /// `resolve_application_url` correctly. `bundle_id` is `Some(...)`
    /// when known (either because LaunchServices gave it to us or
    /// because we read it from the bundle's Info.plist) and `None`
    /// when it couldn't be determined — callers use it for the `oapp`
    /// AppleEvent attachment on the no-URL launch path.
    pub(crate) fn app_ref_and_bundle_id(self) -> (String, Option<String>) {
        match self {
            AppLocator::Path(p) => {
                let bid = bundle_id_for_app_path(&p);
                (p, bid)
            }
            AppLocator::BundleId(bid) => (bid.clone(), Some(bid)),
        }
    }
}

/// Resolve a bundle id to its `AppLocator::BundleId` form.
///
/// Returns `None` if LaunchServices can't find an app for the given
/// bundle id. Note: we deliberately do NOT call `-[NSURL path]` on
/// the resolved URL — that's the fix for CodeRabbit #3 (Cryptex
/// relaunch). Callers should pass the bundle id back to the
/// `nsworkspace::*` helpers, which re-resolve the live NSURL.
pub(crate) fn resolve_bundle_id_to_locator(bundle_id: &str) -> Option<AppLocator> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let ns = NSString::from_str(bundle_id);
        // We only care about presence here — the live NSURL is
        // re-fetched inside `nsworkspace::resolve_application_url`
        // when the launch actually fires. Returning the bundle id
        // (not a flattened `url.path()`) preserves the alias/cryptex
        // metadata Safari needs to relaunch from `/System/Cryptexes/App/...`.
        let _url = ws.URLForApplicationWithBundleIdentifier(&ns)?;
        Some(AppLocator::BundleId(bundle_id.to_owned()))
    }
}

/// Mirror of Swift's `AppLauncher.locate(name:)`.
///
/// 1. filesystem lookup by bundle filename in the canonical roots
///    (system first so /Applications wins over ~/Applications);
/// 2. LaunchServices bundle-id lookup, in case the caller passed a
///    bundle identifier in the `name` slot — preserved as
///    `AppLocator::BundleId` so the launch path uses the live NSURL
///    (Cryptex-safe — see CodeRabbit #3);
/// 3. (skipped) full localized-name scan — not yet needed by current
///    integration tests; can be added if we hit a non-English-name app
///    in the wild.
fn locate_by_name(name: &str) -> Option<AppLocator> {
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
            return Some(AppLocator::Path(path));
        }
    }
    // Fallback: maybe caller passed a bundle id as `name`. Use the
    // Cryptex-safe locator (carries the bundle id, never the lossy
    // url.path()).
    resolve_bundle_id_to_locator(name)
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

/// Return the pid of the current frontmost application via
/// `NSWorkspace.shared.frontmostApplication`. `None` if there isn't one
/// (rare — e.g. screensaver).
pub fn frontmost_pid() -> Option<i32> {
    use objc2_app_kit::NSWorkspace;
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let app = ws.frontmostApplication()?;
        let pid: i32 = app.processIdentifier();
        Some(pid)
    }
}

/// Re-activate the app with `pid` via
/// `NSRunningApplication.runningApplicationWithProcessIdentifier(pid)?.activateWithOptions([])`.
/// Returns `true` if the app was found and activate was attempted.
/// Used as the belt-and-braces step in `LaunchAppTool` when the target
/// has self-activated despite the focus-steal observer.
pub fn activate_pid(pid: i32) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    unsafe {
        match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            Some(app) => app.activateWithOptions(NSApplicationActivationOptions(0)),
            None => false,
        }
    }
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
