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
    /// Per-platform "how launch_app would consume this entry".
    ///
    /// On macOS: filesystem path to the `.app` bundle (e.g. `/Applications/Safari.app`)
    /// when known. `None` for entries that came only from `NSWorkspace`'s
    /// runtime list and whose bundle path could not be resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_path: Option<String>,
    /// Kind discriminator. macOS reports `"desktop"` for every `.app` bundle.
    /// Reserved for future use on platforms with packaged-app distinctions
    /// (e.g. Windows UWP packages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// RFC3339 timestamp of the launcher's filesystem `LastAccessTime` /
    /// `mtime`, when available. `None` when the field could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
}

/// Enumerate running apps with `NSApplicationActivationPolicyRegular`.
///
/// This stays in-process through `NSWorkspace`. Querying System Events through
/// AppleScript adds an unrelated Automation permission, can block on a prompt,
/// and turns a denied prompt into an empty app list.
pub fn list_running_apps() -> Vec<AppInfo> {
    use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};

    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let applications = workspace.runningApplications();
        (0..applications.len())
            .filter_map(|index| applications.get(index))
            .filter(|app| {
                !app.isTerminated()
                    && app.activationPolicy() == NSApplicationActivationPolicy::Regular
            })
            .filter_map(|app| {
                let pid = app.processIdentifier();
                if pid <= 0 {
                    return None;
                }

                let bundle_id = app.bundleIdentifier().map(|value| value.to_string());
                let name = app
                    .localizedName()
                    .map(|value| value.to_string())
                    .or_else(|| bundle_id.clone())
                    .unwrap_or_else(|| format!("pid {pid}"));
                let launch_path = app
                    .bundleURL()
                    .and_then(|url| url.path())
                    .map(|path| path.to_string());

                Some(AppInfo {
                    name,
                    pid,
                    bundle_id,
                    running: true,
                    active: app.isActive(),
                    launch_path,
                    kind: Some("desktop".to_owned()),
                    last_used: None,
                })
            })
            .collect()
    }
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
pub(crate) fn locate_by_name(name: &str) -> Option<AppLocator> {
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
/// NSBundle reads both XML and binary property lists in-process.
fn bundle_id_for_app_path(app_path: &str) -> Option<String> {
    let bundle = app_bundle(std::path::Path::new(app_path))?;
    let bundle_id = unsafe { bundle.bundleIdentifier()? }.to_string();
    nonempty_string(bundle_id)
}

/// Return all apps: running apps merged with installed-but-not-running apps.
///
/// Single flat array. Each entry carries:
///   * `running` (true for currently-live processes, false for installed-only),
///   * `pid` (live pid when running, `0` otherwise),
///   * `launch_path` (filesystem `.app` path when known, else `None`),
///   * `kind` (`"desktop"` on macOS).
pub fn list_all_apps() -> Vec<AppInfo> {
    let mut running = list_running_apps();
    let installed = scan_installed_apps();
    // Lookup: bundle_id → (launch_path, last_used) from the installed scan.
    let installed_by_bundle: std::collections::HashMap<String, (Option<String>, Option<String>)> =
        installed.iter()
            .filter_map(|a| a.bundle_id.clone().map(|b| (b, (a.launch_path.clone(), a.last_used.clone()))))
            .collect();
    // Backfill running entries with the launch_path the installed scan resolved.
    for app in running.iter_mut() {
        if let Some(bid) = &app.bundle_id {
            if let Some((path, last_used)) = installed_by_bundle.get(bid) {
                if app.launch_path.is_none() {
                    app.launch_path = path.clone();
                }
                if app.last_used.is_none() {
                    app.last_used = last_used.clone();
                }
            }
        }
    }

    let running_bundles: std::collections::HashSet<String> = running.iter()
        .filter_map(|a| a.bundle_id.clone())
        .collect();

    let mut installed = installed;
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
            if let Some(mut info) = read_app_plist(&plist_path) {
                info.launch_path = path.to_str().map(str::to_owned);
                info.kind = Some("desktop".to_owned());
                info.last_used = fs_last_used(&path);
                result.push(info);
            }
        }
    }
    result
}

/// Read the bundle's filesystem `mtime` and serialize as RFC3339.
/// Used as `last_used` heuristic — macOS doesn't reliably surface
/// LaunchServices' true "last launched" timestamp without entitlements,
/// so we approximate with whichever of `atime`/`mtime` the filesystem
/// preserves (mtime is the more portable of the two).
fn fs_last_used(path: &std::path::Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(unix_secs_to_rfc3339(duration.as_secs() as i64))
}

/// Format a Unix epoch seconds value as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
/// Hand-rolled to avoid pulling in a date/time crate just for this.
pub(crate) fn unix_secs_to_rfc3339(secs: i64) -> String {
    // Days since 1970-01-01 + civil date breakdown per Howard Hinnant's algorithm.
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hour, minute, second
    )
}

fn read_app_plist(plist_path: &std::path::Path) -> Option<AppInfo> {
    use objc2_foundation::ns_string;

    let app_path = app_path_for_info_plist(plist_path)?;
    let bundle = app_bundle(app_path)?;
    let bundle_id = nonempty_string(unsafe { bundle.bundleIdentifier()? }.to_string())?;
    let name = bundle_info_string(&bundle, ns_string!("CFBundleDisplayName"))
        .or_else(|| bundle_info_string(&bundle, ns_string!("CFBundleName")))
        .or_else(|| app_path.file_stem()?.to_str().map(str::to_owned))
        .and_then(nonempty_string)?;

    Some(AppInfo {
        name,
        pid: 0,
        bundle_id: Some(bundle_id),
        running: false,
        active: false,
        launch_path: None,
        kind: None,
        last_used: None,
    })
}

/// Return the `.app` directory containing `Contents/Info.plist`.
fn app_path_for_info_plist(plist_path: &std::path::Path) -> Option<&std::path::Path> {
    let contents = plist_path.parent()?;
    if contents.file_name()?.to_str()? != "Contents"
        || plist_path.file_name()?.to_str()? != "Info.plist"
    {
        return None;
    }
    contents.parent()
}

/// Load bundle metadata without spawning one or more `plutil` processes per app.
fn app_bundle(app_path: &std::path::Path) -> Option<objc2::rc::Retained<objc2_foundation::NSBundle>> {
    use objc2_foundation::{NSBundle, NSString};

    let path = NSString::from_str(app_path.to_str()?);
    unsafe { NSBundle::bundleWithPath(&path) }
}

fn bundle_info_string(
    bundle: &objc2_foundation::NSBundle,
    key: &objc2_foundation::NSString,
) -> Option<String> {
    use objc2::{msg_send, rc::Retained, ClassType};
    use objc2_foundation::NSString;

    let value = unsafe { bundle.infoDictionary()?.objectForKey(key)? };
    let is_string: bool = unsafe { msg_send![&*value, isKindOfClass: NSString::class()] };
    if !is_string {
        return None;
    }

    // SAFETY: `isKindOfClass:` confirmed NSString or one of its immutable-compatible
    // subclasses. The property-list dictionary retains the value for this call.
    let value = unsafe { Retained::cast::<NSString>(value) };
    nonempty_string(value.to_string())
}

fn nonempty_string(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
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

/// Return the bundle identifier of the running process for `pid`, via
/// `NSRunningApplication.runningApplicationWithProcessIdentifier(pid)?.bundleIdentifier`.
///
/// Returns `None` when:
/// - the pid is unknown to NSWorkspace (non-AppKit processes, e.g. raw
///   command-line tools)
/// - the running app exposes no bundle id (rare: unbundled `.app`-less
///   processes).
///
/// Used by [`crate::terminal::is_terminal_pid`] to route `type_text` past
/// the AX path when the target window belongs to a terminal emulator.
pub fn bundle_id_for_pid(pid: i32) -> Option<String> {
    use objc2_app_kit::NSRunningApplication;
    unsafe {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
        let ns = app.bundleIdentifier()?;
        Some(ns.to_string())
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

#[cfg(test)]
mod tests {
    use super::{app_path_for_info_plist, read_app_plist, unix_secs_to_rfc3339};

    #[test]
    fn rfc3339_epoch() {
        assert_eq!(unix_secs_to_rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_negative_pre_epoch() {
        // 1969-12-31T23:59:59Z = epoch - 1 second.
        assert_eq!(unix_secs_to_rfc3339(-1), "1969-12-31T23:59:59Z");
        // 1969-01-01T00:00:00Z = epoch - 365 days.
        assert_eq!(unix_secs_to_rfc3339(-365 * 86_400), "1969-01-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_leap_day_in_leap_year() {
        // 2020-02-29T12:00:00Z. Days from 1970-01-01:
        //   50 years * 365 + 13 leap days (1972..=2020 inclusive of 13) - 1
        //   (Feb 29 is the 60th day of 2020, so 59 prior days in 2020).
        // Use the known timestamp instead of recomputing.
        // `date -d "2020-02-29T12:00:00Z" +%s` = 1582977600.
        assert_eq!(unix_secs_to_rfc3339(1_582_977_600), "2020-02-29T12:00:00Z");
    }

    #[test]
    fn rfc3339_feb_28_non_leap_year() {
        // 2019-02-28T00:00:00Z → 1551312000.
        assert_eq!(unix_secs_to_rfc3339(1_551_312_000), "2019-02-28T00:00:00Z");
        // The very next second is Mar 1, not Feb 29.
        assert_eq!(unix_secs_to_rfc3339(1_551_312_000 + 86_400), "2019-03-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_end_of_year_wrap() {
        // 2023-12-31T23:59:59Z = 1704067199; +1 second wraps to 2024-01-01.
        assert_eq!(unix_secs_to_rfc3339(1_704_067_199), "2023-12-31T23:59:59Z");
        assert_eq!(unix_secs_to_rfc3339(1_704_067_200), "2024-01-01T00:00:00Z");
    }

    #[test]
    fn info_plist_path_resolves_its_app_bundle() {
        let path = std::path::Path::new("/Applications/Example.app/Contents/Info.plist");
        assert_eq!(
            app_path_for_info_plist(path),
            Some(std::path::Path::new("/Applications/Example.app"))
        );
        assert_eq!(app_path_for_info_plist(std::path::Path::new("/tmp/Info.plist")), None);
    }

    #[test]
    fn reads_bundle_id_and_display_name_in_process() {
        let root = std::env::temp_dir().join(format!(
            "cua-driver-app-plist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let app = root.join("Fixture.app");
        let contents = app.join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        let plist = contents.join("Info.plist");
        std::fs::write(
            &plist,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.trycua.fixture</string>
<key>CFBundleDisplayName</key><string>Fixture Display Name</string>
<key>CFBundleName</key><string>Ignored Name</string>
</dict></plist>"#,
        )
        .unwrap();

        let info = read_app_plist(&plist).expect("fixture bundle metadata");
        assert_eq!(info.bundle_id.as_deref(), Some("dev.trycua.fixture"));
        assert_eq!(info.name, "Fixture Display Name");
        assert!(!info.running);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rfc3339_recent_arbitrary_timestamp() {
        // 2024-06-15T13:45:30Z → 1718459130.
        assert_eq!(unix_secs_to_rfc3339(1_718_459_130), "2024-06-15T13:45:30Z");
    }

    #[test]
    fn rfc3339_known_pre_2000_timestamp() {
        // 1990-07-04T15:30:00Z → 647105400.
        assert_eq!(unix_secs_to_rfc3339(647_105_400), "1990-07-04T15:30:00Z");
    }
}
