//! macOS app enumeration via NSWorkspace and NSRunningApplication.

pub mod nsworkspace;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void,
    process::Command,
    sync::mpsc::{self, SyncSender},
    time::Duration,
};

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

/// Enumerate running, user-facing apps (the `NSApplicationActivationPolicyRegular`
/// set) from the process table.
///
/// Uses only the process table (libproc) and each bundle's own `Info.plist`, so
/// listing or classifying an application still never triggers the macOS
/// Automation permission for System Events — and, unlike AppKit's cached
/// `runningApplications` array, cannot go stale in a daemon.
pub fn list_running_apps() -> Vec<AppInfo> {
    // KERNEL TRUTH FIRST. `NSWorkspace.runningApplications` is a CACHE that
    // AppKit refreshes from workspace notifications delivered on a main run
    // loop. A long-lived daemon (rcdpd, cua-driver serve) has no such loop
    // servicing it, so the array freezes at the snapshot taken when it was
    // first read: measured in a Space after 32 minutes of uptime, `list_apps`
    // reported 2 running apps -- Finder and `Unity Hub (pid 3082)` -- while the
    // kernel had Finder 391, Unity Hub 3690 (two launches later), Google Chrome
    // 4062 and Blender 5539. A dead pid presented as running is worse than a
    // slow call, and an app that is plainly on screen missing from the list
    // sends an agent off to launch it again.
    //
    // The cache is not made correct by invalidating it -- nothing here owns it,
    // and the notifications that would refresh it are never delivered. So the
    // running set comes from the process table, which cannot go stale, and
    // AppKit is kept only as a fallback for the case where the process table is
    // unreadable.
    let apps = list_running_apps_from_process_table();
    if apps.is_empty() {
        return list_running_apps_native();
    }
    apps
}

/// Every pid the kernel currently has, via `proc_listallpids` (libproc, in
/// libSystem — no entitlement, no extra dependency).
fn all_pids() -> Vec<i32> {
    extern "C" {
        fn proc_listallpids(buffer: *mut c_void, buffersize: i32) -> i32;
    }
    unsafe {
        // Size probe (a null buffer returns the count), then read with headroom
        // so a process spawned between the two calls cannot overflow us.
        let count = proc_listallpids(std::ptr::null_mut(), 0);
        if count <= 0 {
            return Vec::new();
        }
        let capacity = (count as usize) + 64;
        let mut pids = vec![0i32; capacity];
        let bytes = (capacity * std::mem::size_of::<i32>()) as i32;
        let written = proc_listallpids(pids.as_mut_ptr() as *mut c_void, bytes);
        if written <= 0 {
            return Vec::new();
        }
        pids.truncate(written as usize);
        pids.retain(|p| *p > 0);
        pids
    }
}

/// The executable path of one live pid, via `proc_pidpath`. `None` when the
/// process has already exited or its path is not readable.
fn path_of_pid(pid: i32) -> Option<String> {
    extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
    }
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4 * 1024;
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let len = unsafe {
        proc_pidpath(
            pid,
            buf.as_mut_ptr() as *mut c_void,
            PROC_PIDPATHINFO_MAXSIZE as u32,
        )
    };
    if len <= 0 {
        return None;
    }
    buf.truncate(len as usize);
    String::from_utf8(buf).ok()
}

/// The `.app` bundle a running executable belongs to, if any.
///
/// The INNERMOST bundle is the right answer: `Google Chrome Helper.app` lives
/// inside `Google Chrome.app`, and it is the helper's own Info.plist
/// (`LSUIElement`) that says it is not a user-facing app. Taking the outermost
/// bundle would list every helper as "Google Chrome".
fn bundle_root_of_executable(path: &str) -> Option<&str> {
    let marker = ".app/Contents/MacOS/";
    let at = path.rfind(marker)?;
    Some(&path[..at + ".app".len()])
}

/// Does this bundle declare itself a background/agent process — the
/// `NSApplicationActivationPolicyRegular` filter, read from the bundle instead
/// of from AppKit's cache.
fn is_background_bundle(app_path: &str) -> bool {
    let plist = format!("{app_path}/Contents/Info.plist");
    for key in ["LSUIElement", "LSBackgroundOnly"] {
        let out = Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw", "-o", "-", &plist])
            .output();
        if let Ok(out) = out {
            if out.status.success() {
                let raw = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
                // The key is written as both a boolean and a "1"/"0" string in
                // the wild; plutil renders the boolean as true/false.
                if raw == "1" || raw == "true" {
                    return true;
                }
            }
        }
    }
    false
}

/// Where a user-launchable application lives. Mirrors the roots
/// `scan_installed_apps` walks, so "running" and "installed" agree on what
/// counts as an app.
const APP_DIRECTORY_PREFIXES: [&str; 4] = [
    "/Applications/",
    "/System/Applications/",
    "/Users/Shared/Applications/",
    "/opt/homebrew-cask/Caskroom/",
];

/// Running, user-facing apps, derived from the process table.
fn list_running_apps_from_process_table() -> Vec<AppInfo> {
    let pids = all_pids();
    let live: std::collections::HashSet<i32> = pids.iter().copied().collect();
    let processes: Vec<(i32, String)> = pids
        .into_iter()
        // A pid without a path exited between enumeration and lookup, or is
        // unreadable; either way there is nothing to act on.
        .filter_map(|pid| path_of_pid(pid).map(|path| (pid, path)))
        .collect();
    // `active` has the same staleness exposure as the running list:
    // `NSWorkspace.frontmostApplication` is maintained by the same
    // notification machinery. So AppKit's answer is used only when the kernel
    // agrees that pid is still alive, and the fallback is the window server's
    // own front-to-back order — which the Space evidence showed to be accurate
    // when the app list was not (`list_space_windows` over rcdp reported
    // reality while `list_apps` did not).
    let front = frontmost_pid().filter(|p| live.contains(p)).or_else(|| {
        crate::windows::visible_windows()
            .into_iter()
            .map(|w| w.pid)
            .find(|p| live.contains(p))
    });
    let windowed: std::collections::HashSet<i32> = crate::windows::all_windows()
        .into_iter()
        .map(|w| w.pid)
        .collect();
    let mut app_roots: Vec<String> = APP_DIRECTORY_PREFIXES
        .iter()
        .map(|root| (*root).to_owned())
        .collect();
    app_roots.push(format!(
        "{}/Applications/",
        std::env::var("HOME").unwrap_or_default()
    ));
    running_apps_from_processes(&processes, front, &windowed, &app_roots)
}

/// Classify one process-table snapshot into running, user-facing apps.
///
/// `processes` is every live `(pid, executable path)`; `front` is the
/// frontmost pid already checked against that snapshot; `windowed` is every
/// pid that owns a window; `app_roots` are the directories a user launches
/// apps from. Every reported pid comes from `processes`, so the answer can
/// never be staler than the snapshot it was given.
fn running_apps_from_processes(
    processes: &[(i32, String)],
    front: Option<i32>,
    windowed: &std::collections::HashSet<i32>,
    app_roots: &[String],
) -> Vec<AppInfo> {
    let mut background: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut meta: std::collections::HashMap<String, Option<AppInfo>> =
        std::collections::HashMap::new();
    // bundle path -> the pids of its live, user-facing processes.
    let mut by_bundle: std::collections::HashMap<String, Vec<i32>> =
        std::collections::HashMap::new();
    let mut apps: Vec<AppInfo> = Vec::new();

    for (pid, exec_path) in processes {
        let Some(app_path) = bundle_root_of_executable(exec_path) else {
            continue; // not an app bundle: a daemon, a CLI, a helper tool
        };
        let app_path = app_path.to_owned();
        if *background
            .entry(app_path.clone())
            .or_insert_with(|| is_background_bundle(&app_path))
        {
            continue;
        }
        meta.entry(app_path.clone()).or_insert_with(|| {
            let plist = format!("{app_path}/Contents/Info.plist");
            read_app_plist(std::path::Path::new(&plist))
        });
        by_bundle.entry(app_path).or_default().push(*pid);
    }

    // Apps a user could have launched live in the app directories. Everything
    // else that happens to be bundled -- `sociallayerd.app`,
    // `UserNotificationCenter.app`, a framework's `Python.app` -- is a system
    // service that sets its activation policy in code rather than in its
    // Info.plist, so the plist filter alone cannot see it. Admit those only
    // when they actually own a window, which is the same evidence a user has
    // for calling something a running app. Finder is the standing exception:
    // it lives in CoreServices and may legitimately have no window open.
    for (app_path, mut pids) in by_bundle {
        let Some(Some(info)) = meta.get(&app_path).cloned() else {
            continue; // no bundle id / name: not a launchable app
        };
        let in_app_dir = app_roots.iter().any(|root| app_path.starts_with(root));
        let is_finder = info.bundle_id.as_deref() == Some("com.apple.finder");
        if !in_app_dir && !is_finder && !pids.iter().any(|p| windowed.contains(p)) {
            continue;
        }
        // NSWorkspace lists every running instance of an app, not one per
        // bundle: two copies of one bundle (`open -n`, a second Electron
        // window process, a test fixture next to a sentinel built from the
        // same app) are separate apps with separate pids an agent acts on.
        // So report every process of the bundle that owns a window, plus the
        // frontmost one. A bundle none of whose processes qualifies is still
        // running (Finder with no window, an app between windows): report its
        // lowest pid -- the parent, since children are spawned later. Never
        // report a pid the caller cannot act on.
        pids.sort_unstable();
        pids.dedup();
        let mut selected: Vec<i32> = pids
            .iter()
            .copied()
            .filter(|p| windowed.contains(p) || front == Some(*p))
            .collect();
        if selected.is_empty() {
            selected.push(pids[0]);
        }
        for pid in selected {
            apps.push(AppInfo {
                name: info.name.clone(),
                pid,
                bundle_id: info.bundle_id.clone(),
                running: true,
                active: front == Some(pid),
                launch_path: Some(app_path.clone()),
                kind: Some("desktop".to_owned()),
                last_used: None,
            });
        }
    }
    apps.sort_by(|a, b| a.name.cmp(&b.name).then(a.pid.cmp(&b.pid)));
    apps
}

/// AppKit's view of the running apps: the fallback when the process table
/// is unreadable.
fn list_running_apps_native() -> Vec<AppInfo> {
    enumerate_running_apps().0
}

/// Live `(pid, active, bundle path)` entries per bundle identifier.
type RunningAppStates = std::collections::HashMap<String, Vec<(i32, bool, Option<String>)>>;

/// Single walk over `NSWorkspace.runningApplications` producing both views the
/// app listing needs from ONE snapshot:
///
/// * standalone entries — `Regular`-policy apps only, so background helpers
///   and system UI agents stay out of the list. This stays entirely inside
///   AppKit so listing or classifying an application never triggers the
///   macOS Automation permission for System Events.
/// * live `(pid, active)` state for every process that reports a bundle
///   identifier, across ALL activation policies. The `Regular`-only filter
///   above must not decide whether an *installed* app is running: bundles
///   shipped with `LSUIElement = true` (Cua Driver itself, many menu-bar
///   apps) run as `Accessory`, never enter the standalone list, and would
///   otherwise surface as `running = false / pid = 0` while windows and the
///   accessibility tree see the live process (#3060).
fn enumerate_running_apps() -> (Vec<AppInfo>, RunningAppStates) {
    use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};

    let mut standalone = Vec::new();
    let mut states = std::collections::HashMap::new();
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let running = workspace.runningApplications();
        for index in 0..running.count() {
            let app = running.objectAtIndex(index);
            if app.isTerminated() {
                continue;
            }
            let pid = app.processIdentifier();
            if pid <= 0 {
                continue;
            }
            let bundle_id = app.bundleIdentifier().map(|value| value.to_string());
            let launch_path = app
                .bundleURL()
                .and_then(|url| url.path())
                .map(|path| path.to_string());
            if let Some(bid) = bundle_id.as_deref() {
                if !bid.is_empty() {
                    states.entry(bid.to_owned()).or_insert_with(Vec::new).push((
                        pid,
                        app.isActive(),
                        launch_path.clone(),
                    ));
                }
            }
            if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
                continue;
            }
            let Some(name) = app.localizedName().map(|value| value.to_string()) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            standalone.push(AppInfo {
                name,
                pid,
                bundle_id,
                running: true,
                active: app.isActive(),
                launch_path,
                kind: Some("desktop".to_owned()),
                last_used: None,
            });
        }
    }
    (standalone, states)
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
        .with_context(|| format!("Failed to launch {bundle_id}"))?;
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
        .with_context(|| format!("Failed to launch '{name}'"))?;
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
    if additional_args.is_empty()
        && env.is_empty()
        && !creates_new_instance
        && finder_folder_handoff(bundle_id, urls)
    {
        return open_finder_folders(urls);
    }

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
    .with_context(|| format!("Failed to launch {bundle_id}"))?;
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
    if additional_args.is_empty()
        && env.is_empty()
        && !creates_new_instance
        && bid
            .as_deref()
            .is_some_and(|bundle_id| finder_folder_handoff(bundle_id, urls))
    {
        return open_finder_folders(urls);
    }
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
    .with_context(|| format!("Failed to launch '{name}'"))?;
    let pid: i32 = unsafe { running.processIdentifier() };
    Ok(pid)
}

pub(crate) fn finder_folder_handoff(bundle_id: &str, urls: &[String]) -> bool {
    bundle_id == "com.apple.finder"
        && !urls.is_empty()
        && urls.iter().all(|url| std::path::Path::new(url).is_dir())
}

fn open_finder_folders(urls: &[String]) -> anyhow::Result<i32> {
    const MAIN_QUEUE_TIMEOUT: Duration = Duration::from_secs(5);

    if objc2_foundation::MainThreadMarker::new().is_some() {
        select_finder_folders(urls)?;
    } else {
        let (tx, rx) = mpsc::sync_channel(1);
        let request = Box::new(FinderFolderRequest {
            folders: urls.to_vec(),
            tx,
        });
        unsafe {
            let main_queue = &raw const _dispatch_main_q as *const c_void;
            dispatch_async_f(
                main_queue,
                Box::into_raw(request) as *mut c_void,
                open_finder_folders_on_main,
            );
        }
        rx.recv_timeout(MAIN_QUEUE_TIMEOUT)
            .context("Timed out waiting for Finder folder handoff on the AppKit main queue")?
            .map_err(anyhow::Error::msg)?;
    }

    list_running_apps()
        .into_iter()
        .find(|app| app.bundle_id.as_deref() == Some("com.apple.finder") && app.pid > 0)
        .map(|app| app.pid)
        .ok_or_else(|| {
            anyhow::anyhow!("Finder accepted the folder open request but is not running")
        })
}

fn select_finder_folders(urls: &[String]) -> anyhow::Result<()> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    for folder in urls {
        let folder = NSString::from_str(folder);
        if !unsafe { workspace.selectFile_inFileViewerRootedAtPath(None, &folder) } {
            anyhow::bail!("Finder refused to open folder: {folder}");
        }
    }
    Ok(())
}

struct FinderFolderRequest {
    folders: Vec<String>,
    tx: SyncSender<Result<(), String>>,
}

#[link(name = "System", kind = "framework")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

unsafe extern "C" fn open_finder_folders_on_main(context: *mut c_void) {
    let request = unsafe { Box::from_raw(context.cast::<FinderFolderRequest>()) };
    let result = select_finder_folders(&request.folders).map_err(|error| error.to_string());
    let _ = request.tx.send(result);
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
/// Falls back to shelling out to `plutil` (already used elsewhere in
/// this file) to avoid pulling in a plist crate just for this.
fn bundle_id_for_app_path(app_path: &str) -> Option<String> {
    let plist = format!("{app_path}/Contents/Info.plist");
    let out = Command::new("/usr/bin/plutil")
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
///
/// Single flat array. Each entry carries:
///   * `running` (true for currently-live processes, false for installed-only),
///   * `pid` (live pid when running, `0` otherwise),
///   * `launch_path` (filesystem `.app` path when known, else `None`),
///   * `kind` (`"desktop"` on macOS).
pub fn list_all_apps() -> Vec<AppInfo> {
    // The running list comes from the process table (kernel truth). AppKit
    // supplies the all-policies state for installed apps that run as
    // accessories (#3060), kept only for pids the kernel still has, since
    // its cache can go stale in a long-lived daemon.
    let running = list_running_apps();
    let live: std::collections::HashSet<i32> = all_pids().into_iter().collect();
    let (_, mut running_states) = enumerate_running_apps();
    if !live.is_empty() {
        for entries in running_states.values_mut() {
            entries.retain(|(pid, _, _)| live.contains(pid));
        }
        running_states.retain(|_, entries| !entries.is_empty());
    }
    let installed = scan_installed_apps();
    merge_app_lists(running, installed, &running_states)
}

/// Pure merge behind [`list_all_apps`] — extracted so the identity rules
/// are testable without a live NSWorkspace:
///
/// * standalone entries keep the `Regular`-only contract of
///   [`list_running_apps`] (helpers and UI agents stay out of the list);
/// * installed entries resolve `running` / `pid` / `active` against the
///   all-policies running-state map by bundle id and bundle path, so an installed `.app`
///   whose process runs as an accessory reports its live state instead
///   of the `pid = 0` scan defaults (#3060). Exact bundle paths distinguish
///   installed copies that share an identifier. Bundle-id-only fallback is
///   allowed only when the installed copy is unambiguous;
/// * installed entries already covered by a standalone running entry are
///   dropped — the standalone entry wins and is backfilled with the
///   `launch_path` / `last_used` the installed scan resolved.
pub(crate) fn merge_app_lists(
    mut running: Vec<AppInfo>,
    mut installed: Vec<AppInfo>,
    running_states: &RunningAppStates,
) -> Vec<AppInfo> {
    // Lookup: bundle_id → (launch_path, last_used) from the installed scan.
    let installed_by_bundle: std::collections::HashMap<String, (Option<String>, Option<String>)> =
        installed
            .iter()
            .filter_map(|a| {
                a.bundle_id
                    .clone()
                    .map(|b| (b, (a.launch_path.clone(), a.last_used.clone())))
            })
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

    let installed_bundle_counts = installed
        .iter()
        .filter_map(|app| app.bundle_id.clone())
        .fold(
            std::collections::HashMap::<String, usize>::new(),
            |mut counts, bundle| {
                *counts.entry(bundle).or_default() += 1;
                counts
            },
        );

    // Upgrade installed entries whose live process runs outside the Regular
    // list. Prefer the exact bundle path so duplicate debug/release copies do
    // not all inherit one process's live state.
    for app in installed.iter_mut() {
        let Some(bundle_id) = app.bundle_id.as_deref() else {
            continue;
        };
        let Some(candidates) = running_states.get(bundle_id) else {
            continue;
        };
        let exact = app.launch_path.as_deref().and_then(|installed_path| {
            candidates
                .iter()
                .find(|(_, _, live_path)| live_path.as_deref() == Some(installed_path))
        });
        let selected = exact.or_else(|| {
            (installed_bundle_counts.get(bundle_id) == Some(&1))
                .then(|| candidates.last())
                .flatten()
        });
        if let Some(&(pid, active, _)) = selected {
            app.running = true;
            app.pid = pid;
            app.active = active;
        }
    }

    let running_bundles: std::collections::HashSet<String> =
        running.iter().filter_map(|a| a.bundle_id.clone()).collect();

    // Remove apps already in running list.
    installed.retain(|a| {
        !a.bundle_id
            .as_ref()
            .is_some_and(|b| running_bundles.contains(b))
    });

    running.extend(installed);
    running
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
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("app") {
                continue;
            }
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
    cua_driver_core::timestamp::unix_secs_to_rfc3339(duration.as_secs() as i64)
}

fn read_app_plist(plist_path: &std::path::Path) -> Option<AppInfo> {
    let bundle_id_out = Command::new("plutil")
        .args([
            "-extract",
            "CFBundleIdentifier",
            "raw",
            "-o",
            "-",
            plist_path.to_str()?,
        ])
        .output()
        .ok()?;
    if !bundle_id_out.status.success() {
        return None;
    }
    let bundle_id = String::from_utf8_lossy(&bundle_id_out.stdout)
        .trim()
        .to_string();
    if bundle_id.is_empty() {
        return None;
    }

    let name_out = Command::new("plutil")
        .args([
            "-extract",
            "CFBundleDisplayName",
            "raw",
            "-o",
            "-",
            plist_path.to_str()?,
        ])
        .output()
        .ok();
    let name = name_out
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            // Fallback: CFBundleName.
            Command::new("plutil")
                .args([
                    "-extract",
                    "CFBundleName",
                    "raw",
                    "-o",
                    "-",
                    plist_path.to_str().unwrap_or(""),
                ])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    plist_path
                        .parent()
                        .and_then(|p| p.parent())
                        .and_then(|p| p.file_stem())
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string()
                })
        });

    if name.is_empty() {
        return None;
    }

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
        let bundle = app
            .bundle_id
            .as_deref()
            .map(|b| format!(" [{b}]"))
            .unwrap_or_default();
        lines.push(format!("- {} (pid {}){}", app.name, app.pid, bundle));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        all_pids, bundle_root_of_executable, finder_folder_handoff, merge_app_lists, path_of_pid,
        running_apps_from_processes, AppInfo,
    };
    use std::collections::HashSet;

    fn app(name: &str, pid: i32, bundle: Option<&str>, running: bool) -> AppInfo {
        AppInfo {
            name: name.to_owned(),
            pid,
            bundle_id: bundle.map(str::to_owned),
            running,
            active: false,
            launch_path: None,
            kind: None,
            last_used: None,
        }
    }

    fn states(pairs: &[(&str, i32, bool, Option<&str>)]) -> super::RunningAppStates {
        pairs.iter().fold(
            super::RunningAppStates::new(),
            |mut states, (bundle, pid, active, path)| {
                states.entry((*bundle).to_owned()).or_default().push((
                    *pid,
                    *active,
                    path.map(str::to_owned),
                ));
                states
            },
        )
    }

    #[test]
    fn installed_app_running_as_accessory_reports_live_state() {
        // The #3060 shape: CuaDriver.app ships LSUIElement=true, so its live
        // process runs as Accessory and never enters the Regular list.
        let mut entry = app("Cua Driver", 0, Some("com.trycua.driver"), false);
        entry.launch_path = Some("/Applications/CuaDriver.app".to_owned());
        let merged = merge_app_lists(
            vec![],
            vec![entry],
            &states(&[(
                "com.trycua.driver",
                31438,
                false,
                Some("/Applications/CuaDriver.app"),
            )]),
        );
        assert_eq!(merged.len(), 1);
        assert!(merged[0].running);
        assert_eq!(merged[0].pid, 31438);
        // The upgrade must not clobber the fields the installed scan owns.
        assert_eq!(
            merged[0].launch_path.as_deref(),
            Some("/Applications/CuaDriver.app")
        );
    }

    #[test]
    fn installed_app_not_running_keeps_scan_defaults() {
        let merged = merge_app_lists(
            vec![],
            vec![app("TextEdit", 0, Some("com.apple.TextEdit"), false)],
            &states(&[]),
        );
        assert_eq!(merged.len(), 1);
        assert!(!merged[0].running);
        assert_eq!(merged[0].pid, 0);
    }

    #[test]
    fn regular_running_app_wins_over_installed_entry() {
        let running = vec![app("Safari", 100, Some("com.apple.Safari"), true)];
        let installed = vec![{
            let mut entry = app("Safari", 0, Some("com.apple.Safari"), false);
            entry.launch_path = Some("/Applications/Safari.app".to_owned());
            entry
        }];
        let merged = merge_app_lists(
            running,
            installed,
            &states(&[(
                "com.apple.Safari",
                100,
                true,
                Some("/Applications/Safari.app"),
            )]),
        );
        assert_eq!(merged.len(), 1);
        assert!(merged[0].running);
        assert_eq!(merged[0].pid, 100);
        assert_eq!(
            merged[0].launch_path.as_deref(),
            Some("/Applications/Safari.app")
        );
    }

    #[test]
    fn accessory_process_without_installed_bundle_adds_no_entry() {
        // A background helper with a bundle id but no installed .app must not
        // materialize a row just because it appears in the states map.
        let merged = merge_app_lists(
            vec![],
            vec![],
            &states(&[("dev.helper.agent", 9, false, None)]),
        );
        assert!(merged.is_empty());
    }

    #[test]
    fn duplicate_bundle_ids_upgrade_only_the_live_bundle_path() {
        let mut release = app("Cua Driver", 0, Some("com.trycua.driver"), false);
        release.launch_path = Some("/Applications/CuaDriver.app".to_owned());
        let mut debug = app("Cua Driver", 0, Some("com.trycua.driver"), false);
        debug.launch_path = Some("/Users/test/CuaDriver.app".to_owned());

        let merged = merge_app_lists(
            vec![],
            vec![release, debug],
            &states(&[(
                "com.trycua.driver",
                42,
                false,
                Some("/Users/test/CuaDriver.app"),
            )]),
        );

        assert!(!merged[0].running);
        assert_eq!(merged[0].pid, 0);
        assert!(merged[1].running);
        assert_eq!(merged[1].pid, 42);
    }

    #[test]
    fn finder_folder_handoff_is_narrowly_selected() {
        let folder = std::env::temp_dir().to_string_lossy().into_owned();

        assert!(finder_folder_handoff(
            "com.apple.finder",
            std::slice::from_ref(&folder)
        ));
        assert!(!finder_folder_handoff("com.apple.TextEdit", &[folder]));
        assert!(!finder_folder_handoff(
            "com.apple.finder",
            &["https://example.com".to_owned()]
        ));
    }

    /// Write a minimal `.app` bundle under `root` and return its executable
    /// path. Nothing is executed: the classifier only reads the bundle's
    /// Info.plist, so no process, window, or LaunchServices record appears.
    fn synthetic_bundle(root: &std::path::Path, name: &str, extra_plist: &str) -> String {
        let bundle = root.join(format!("{name}.app"));
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).expect("create bundle");
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.trycua.test.{name}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundleExecutable</key><string>{name}</string>
{extra_plist}
</dict></plist>
"#
            ),
        )
        .expect("write Info.plist");
        macos.join(name).to_str().expect("utf-8 path").to_owned()
    }

    fn pid_of(apps: &[AppInfo], name: &str) -> Option<i32> {
        let bundle_id = format!("com.trycua.test.{name}");
        apps.iter()
            .find(|app| app.bundle_id.as_deref() == Some(bundle_id.as_str()))
            .map(|app| app.pid)
    }

    /// The regression this file exists for: the running list is derived from
    /// the process-table snapshot it is given, never from a cache, so an app
    /// appears with the pid that snapshot holds and disappears as soon as a
    /// snapshot no longer holds it (the stale-pid bug).
    #[test]
    fn running_apps_follow_the_process_snapshot() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = format!("{}/", temp.path().display());
        let regular = synthetic_bundle(temp.path(), "CuaAppProbe", "");
        let roots = [root];
        let nothing = HashSet::new();

        let launched = [(4242, regular.clone()), (4243, "/usr/bin/ssh".to_owned())];
        let apps = running_apps_from_processes(&launched, None, &nothing, &roots);
        assert_eq!(pid_of(&apps, "CuaAppProbe"), Some(4242));
        assert_eq!(
            apps.len(),
            1,
            "a non-bundled process is not an app: {apps:?}"
        );
        assert!(!apps[0].active);

        let exited = [(4243, "/usr/bin/ssh".to_owned())];
        let apps = running_apps_from_processes(&exited, None, &nothing, &roots);
        assert_eq!(
            pid_of(&apps, "CuaAppProbe"),
            None,
            "an app missing from the snapshot must not be reported (stale pid)"
        );
    }

    /// A bundle with no windowed process is one entry: the frontmost pid when
    /// it belongs to the bundle, otherwise the lowest (parent) pid. Every
    /// reported pid is one the snapshot holds.
    #[test]
    fn windowless_bundle_is_one_entry_preferring_the_front_pid() {
        let temp = tempfile::tempdir().expect("temp dir");
        let exe = synthetic_bundle(temp.path(), "CuaMulti", "");
        let roots = [format!("{}/", temp.path().display())];
        let nothing = HashSet::new();
        let processes = [(900, exe.clone()), (700, exe.clone()), (800, exe)];

        let apps = running_apps_from_processes(&processes, None, &nothing, &roots);
        assert_eq!(apps.len(), 1, "{apps:?}");
        assert_eq!((apps[0].pid, apps[0].active), (700, false));

        let apps = running_apps_from_processes(&processes, Some(800), &nothing, &roots);
        assert_eq!(apps.len(), 1, "{apps:?}");
        assert_eq!((apps[0].pid, apps[0].active), (800, true));

        let apps = running_apps_from_processes(&processes, Some(1), &nothing, &roots);
        assert_eq!(apps.len(), 1, "{apps:?}");
        assert_eq!((apps[0].pid, apps[0].active), (700, false));
    }

    /// Two windowed instances of one bundle are two apps, as NSWorkspace
    /// reports them. A frontmost second instance must not hide the first
    /// (the E2E fixture vs. foreground-sentinel regression: both are
    /// CuaTestHarness.Electron.app, the sentinel is frontmost).
    #[test]
    fn every_windowed_instance_of_a_bundle_is_listed() {
        let temp = tempfile::tempdir().expect("temp dir");
        let exe = synthetic_bundle(temp.path(), "CuaTwin", "");
        // Outside the app roots, so only windowed processes qualify.
        let roots = ["/nonexistent-app-root/".to_owned()];
        let windowed: HashSet<i32> = [700, 800].into_iter().collect();
        let processes = [(700, exe.clone()), (800, exe.clone()), (900, exe)];

        let apps = running_apps_from_processes(&processes, Some(800), &windowed, &roots);
        let listed: Vec<(i32, bool)> = apps.iter().map(|a| (a.pid, a.active)).collect();
        assert_eq!(listed, vec![(700, false), (800, true)], "{apps:?}");
    }

    /// Agent and background bundles are never apps; a bundle outside the app
    /// directories counts only once it owns a window.
    #[test]
    fn background_and_windowless_system_bundles_are_filtered() {
        let temp = tempfile::tempdir().expect("temp dir");
        let agent = synthetic_bundle(temp.path(), "CuaAgent", "<key>LSUIElement</key><true/>");
        let background = synthetic_bundle(
            temp.path(),
            "CuaBackground",
            "<key>LSBackgroundOnly</key><string>1</string>",
        );
        let service = synthetic_bundle(temp.path(), "CuaService", "");
        let processes = [(10, agent), (11, background), (12, service)];
        let in_roots = [format!("{}/", temp.path().display())];

        let apps = running_apps_from_processes(&processes, None, &HashSet::new(), &in_roots);
        assert_eq!(
            apps.iter().map(|app| app.pid).collect::<Vec<_>>(),
            vec![12],
            "agent and background-only bundles must be filtered: {apps:?}"
        );

        let elsewhere = ["/Applications/".to_owned()];
        let apps = running_apps_from_processes(&processes, None, &HashSet::new(), &elsewhere);
        assert!(
            apps.is_empty(),
            "windowless system bundle admitted: {apps:?}"
        );
        let apps = running_apps_from_processes(&processes, None, &HashSet::from([12]), &elsewhere);
        assert_eq!(pid_of(&apps, "CuaService"), Some(12));
    }

    /// The process-table readers see this very process at its own path. This
    /// reads only the kernel's record of the test process: no window server,
    /// no LaunchServices, no other application.
    #[test]
    fn process_table_reports_this_process() {
        let own = std::process::id() as i32;
        assert!(all_pids().contains(&own));
        let path = path_of_pid(own).expect("own executable path");
        let expected = std::fs::canonicalize(std::env::current_exe().expect("current exe"))
            .expect("canonical exe");
        assert_eq!(std::path::Path::new(&path), expected);
    }

    #[test]
    fn bundle_root_takes_the_innermost_bundle() {
        assert_eq!(
            bundle_root_of_executable(
                "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/1/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper"
            ),
            Some("/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/1/Helpers/Google Chrome Helper.app")
        );
        assert_eq!(
            bundle_root_of_executable("/Applications/Blender.app/Contents/MacOS/Blender"),
            Some("/Applications/Blender.app")
        );
        assert_eq!(bundle_root_of_executable("/usr/bin/ssh"), None);
    }
}
