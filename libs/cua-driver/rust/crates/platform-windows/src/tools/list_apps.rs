// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

// ── list_apps ────────────────────────────────────────────────────────────────

pub struct ListAppsTool;

static LIST_APPS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ListAppsTool {
    fn def(&self) -> &ToolDef {
        LIST_APPS_DEF.get_or_init(|| ToolDef {
            name: "list_apps".into(),
            description:
                "List Windows apps — both currently running and installed-but-not-running — \
                with per-app state flags:\n\n\
                - running: is a process for this app live? (pid is 0 when false)\n\
                - active: is it the system-frontmost app? (implies running)\n\
                - kind: `\"desktop\"` for `.exe`-backed apps (resolved from Start-Menu \
                shortcuts) or `\"uwp\"` for packaged store apps.\n\
                - launch_path: what `launch_app` would consume.\n\
                    * desktop: full `.exe` commandline (path + any arguments preserved from \
                      the source `.lnk`; the exe is quoted when it contains whitespace).\n\
                    * uwp: `shell:appsFolder\\{PackageFamilyName}!{AppId}` where `{AppId}` \
                      falls back to `App` when the package manifest does not expose a \
                      specific Application.Id.\n\
                - last_used: RFC3339 mtime of the launcher (`.lnk` for desktop, \
                package install location for UWP), when readable.\n\n\
                Running apps are derived from visible top-level windows + the foreground \
                pid (mirrors NSApplicationActivationPolicyRegular's intent: background \
                services and console processes are excluded). Installed apps are merged \
                from Start-Menu `.lnk` enumeration and the WinRT PackageManager — running \
                processes whose executable matches a Start-Menu target are folded into a \
                single entry with `running: true`.\n\n\
                Use this for \"is X installed?\" as well as \"is X running?\". For per-window \
                state — on-screen, minimized, window titles — call list_windows instead. \
                For just opening an app — running or not — call launch_app({path: ...}) \
                directly; list_apps is not a prerequisite."
                    .into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        // Strategy:
        // 1. Enumerate top-level visible windows → derive the set of running pids
        //    that have a user-facing surface, including the packaged app each
        //    ApplicationFrameHost frame hosts.
        // 2. Enumerate the process table to map pid → executable name and full
        //    path (used to merge against Start-Menu / UWP launch_paths).
        // 3. Enumerate installed apps from Start-Menu shortcuts + WinRT
        //    PackageManager. Each installed entry has a launch_path.
        // 4. Merge: a running packaged pid whose OS-attested package identity
        //    matches an installed UWP entry, or a running pid whose executable
        //    path matches an installed desktop launch_path, becomes a single
        //    entry with running=true and that launch_path. Remaining
        //    installed entries are emitted with running=false, pid=0.
        //    Remaining running pids (no installed-app match) are emitted as
        //    running=true with launch_path=null.
        let apps = tokio::task::spawn_blocking(|| -> Vec<serde_json::Value> {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

            // Per-step timings logged via `tracing::debug!` (silent at default
            // log level — enable with `RUST_LOG=cua_driver=debug` when debugging
            // a slow list_apps invocation).
            let t0 = std::time::Instant::now();
            let wins = crate::win32::list_windows_via_win32(None);
            tracing::debug!(target: "list_apps", "step 1 list_windows: {} wins ({}ms)", wins.len(), t0.elapsed().as_millis());
            let running_pids = running_app_pids(&wins, crate::win32::hosted_app_pid);

            let t1 = std::time::Instant::now();
            // A foreground ApplicationFrameHost frame makes the app it hosts
            // the active one, not the host.
            let foreground_pid = unsafe {
                let fg = GetForegroundWindow();
                let mut pid = 0u32;
                let _ = GetWindowThreadProcessId(fg, Some(&mut pid));
                crate::win32::hosted_app_pid(pid, fg.0 as u64).unwrap_or(pid)
            };
            tracing::debug!(target: "list_apps", "step 2 fg-window: pid={} ({}ms)", foreground_pid, t1.elapsed().as_millis());

            let t2 = std::time::Instant::now();
            let procs = crate::win32::list_processes();
            tracing::debug!(target: "list_apps", "step 3 list_processes: {} procs ({}ms)", procs.len(), t2.elapsed().as_millis());
            let pid_to_name: std::collections::HashMap<u32, String> =
                procs.iter().map(|p| (p.pid, p.name.clone())).collect();

            let t3 = std::time::Instant::now();
            let installed = crate::win32::list_installed_apps();
            tracing::debug!(target: "list_apps", "step 4 list_installed_apps: {} installed ({}ms)", installed.len(), t3.elapsed().as_millis());
            // Lowercase-exe-basename → installed-app indices. We match running
            // processes by basename rather than full path because the process
            // table's executable name is `notepad.exe` while the Start-Menu
            // shortcut target is `C:\Windows\System32\notepad.exe` — both
            // sides need to be normalised to compare. Multiple Start-Menu
            // shortcuts often target the same basename (different `chrome.exe`
            // launchers, multiple `code.exe` profiles, …), so we bucket as
            // `Vec<usize>` and let `disambiguate_installed_match` pick a
            // winner per running pid rather than letting the last write win.
            let mut by_exe: std::collections::HashMap<String, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, app) in installed.iter().enumerate() {
                if app.kind == "desktop" {
                    let basename = std::path::Path::new(&app.launch_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if !basename.is_empty() {
                        by_exe.entry(basename).or_default().push(i);
                    }
                }
            }

            let mut consumed_installed: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            let mut out: Vec<serde_json::Value> = Vec::new();

            for &pid in &running_pids {
                let name = pid_to_name.get(&pid).cloned().unwrap_or_else(|| "<unknown>".into());
                // A packaged process (Store / MSIX app) never matches a
                // Start-Menu `.lnk` by basename, so it would otherwise surface
                // with no identity and no bundle_id grant could ever match it.
                let aumid = crate::win32::process_app_user_model_id(pid);
                let merged = packaged_installed_match(aumid.as_deref(), &installed).or_else(|| {
                    let key = name.to_ascii_lowercase();
                    let candidates = by_exe.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);
                    disambiguate_installed_match(candidates, &installed, &name)
                });
                if let Some(idx) = merged { consumed_installed.insert(idx); }
                let (display_name, bundle_id, launch_path, kind, last_used) = match merged {
                    Some(idx) => {
                        let a = &installed[idx];
                        (
                            a.name.clone(),
                            Some(a.bundle_id.clone()),
                            Some(a.launch_path.clone()),
                            Some(a.kind.clone()),
                            a.last_used.clone(),
                        )
                    }
                    None => (name, None, None, Some("desktop".to_owned()), None),
                };
                out.push(json!({
                    "pid":         pid,
                    "bundle_id":   bundle_id,
                    "name":        display_name,
                    "running":     true,
                    "active":      pid == foreground_pid,
                    "kind":        kind,
                    "launch_path": launch_path,
                    "last_used":   last_used,
                    "windows":     Vec::<serde_json::Value>::new(),
                }));
            }
            for (i, app) in installed.iter().enumerate() {
                if consumed_installed.contains(&i) { continue }
                out.push(json!({
                    "pid":         0,
                    "bundle_id":   app.bundle_id.clone(),
                    "name":        app.name.clone(),
                    "running":     false,
                    "active":      false,
                    "kind":        app.kind.clone(),
                    "launch_path": app.launch_path.clone(),
                    "last_used":   app.last_used.clone(),
                    "windows":     Vec::<serde_json::Value>::new(),
                }));
            }
            out
        }).await.unwrap_or_default();

        let running_count = apps
            .iter()
            .filter(|a| a["running"].as_bool().unwrap_or(false))
            .count();
        let total = apps.len();
        let installed_only = total - running_count;
        let mut lines = vec![format!(
            "✅ Found {total} app(s): {running_count} running, {installed_only} installed-not-running."
        )];
        for app in apps
            .iter()
            .filter(|a| a["running"].as_bool().unwrap_or(false))
        {
            let name = app["name"].as_str().unwrap_or("?");
            let pid = app["pid"].as_u64().unwrap_or(0);
            lines.push(format!("- {name} (pid {pid})"));
        }
        // `apps` is the unified shape; `processes` retained for any pre-existing
        // callers that consumed the old running-only shape.
        let structured = json!({
            "apps": apps,
            "processes": apps.iter().filter(|a| a["running"].as_bool().unwrap_or(false))
                .map(|a| json!({
                    "pid":  a["pid"], "name": a["name"]
                })).collect::<Vec<_>>(),
        });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}

/// Pids that own a visible top-level window, in first-seen order. A UWP app
/// hosted in an `ApplicationFrameHost` frame owns no top-level window of its
/// own, so `hosted` resolves each frame to the app pid behind it, which
/// follows the host's pid. That is the pid bounded mode resolves the frame to
/// before it looks the app up here.
fn running_app_pids(
    windows: &[crate::win32::WindowInfo],
    hosted: impl Fn(u32, u64) -> Option<u32>,
) -> Vec<u32> {
    let mut seen = std::collections::HashSet::new();
    let mut pids = Vec::new();
    for window in windows {
        for pid in std::iter::once(window.pid).chain(hosted(window.pid, window.hwnd)) {
            if seen.insert(pid) {
                pids.push(pid);
            }
        }
    }
    pids
}

/// Pick which of several Start-Menu / UWP-derived installed apps best matches
/// a running pid whose exe basename collided. Precedence:
///   1. exact case-insensitive equality between `proc_exe_basename` and the
///      installed app's launch_path basename (always true here; left for
///      future per-path matches if we ever bucket on something coarser)
///   2. most recently modified launcher (`last_used` desc — RFC3339 strings
///      are lexicographically ordered the same as chronologically)
///   3. first candidate (deterministic by source order)
fn disambiguate_installed_match(
    candidates: &[usize],
    installed: &[crate::win32::InstalledApp],
    _proc_exe_basename: &str,
) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }

    candidates
        .iter()
        .copied()
        .max_by(|&a, &b| {
            installed[a]
                .last_used
                .as_deref()
                .unwrap_or("")
                .cmp(installed[b].last_used.as_deref().unwrap_or(""))
        })
        .or_else(|| candidates.first().copied())
}

/// Pick the installed UWP entry whose launch_path is exactly
/// `shell:appsFolder\{AUMID}` for the AUMID Windows attests for a running
/// process.
///
/// A packaged process with no AUMID gets no match: the installed catalog
/// keeps one entry per package, so its family alone cannot say which app the
/// process belongs to. A process whose AUMID names a different app of the
/// same package gets no match either, so it never borrows a sibling app's
/// launch_path.
fn packaged_installed_match(
    aumid: Option<&str>,
    installed: &[crate::win32::InstalledApp],
) -> Option<usize> {
    let launch_path = format!("shell:appsFolder\\{}", aumid?);
    installed
        .iter()
        .position(|app| app.kind == "uwp" && app.launch_path.eq_ignore_ascii_case(&launch_path))
}

#[cfg(test)]
mod list_apps_packaged_identity_tests {
    use super::packaged_installed_match;
    use crate::win32::InstalledApp;

    fn app(kind: &str, bundle_id: &str, launch_path: &str) -> InstalledApp {
        InstalledApp {
            name: bundle_id.to_owned(),
            bundle_id: bundle_id.to_owned(),
            kind: kind.to_owned(),
            launch_path: launch_path.to_owned(),
            last_used: None,
        }
    }

    const NOTEPAD: &str = "Microsoft.WindowsNotepad_8wekyb3d8bbwe";

    fn installed() -> Vec<InstalledApp> {
        vec![
            app(
                "desktop",
                r"C:\Windows\System32\notepad.exe",
                r"C:\Windows\System32\notepad.exe",
            ),
            app(
                "uwp",
                "Microsoft.WindowsCalculator_8wekyb3d8bbwe",
                r"shell:appsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            ),
            app("uwp", NOTEPAD, &format!(r"shell:appsFolder\{NOTEPAD}!App")),
        ]
    }

    #[test]
    fn running_store_app_matches_its_installed_entry_by_aumid() {
        let aumid = format!("{NOTEPAD}!App");
        assert_eq!(
            packaged_installed_match(Some(&aumid), &installed()),
            Some(2)
        );
    }

    #[test]
    fn aumid_match_ignores_ascii_case() {
        let aumid = "microsoft.windowsnotepad_8wekyb3d8bbwe!app";
        assert_eq!(packaged_installed_match(Some(aumid), &installed()), Some(2));
    }

    #[test]
    fn process_without_aumid_never_matches_a_store_entry() {
        assert_eq!(packaged_installed_match(None, &installed()), None);
    }

    #[test]
    fn sibling_app_of_the_same_package_does_not_borrow_its_launch_path() {
        let aumid = format!("{NOTEPAD}!OtherApp");
        assert_eq!(packaged_installed_match(Some(&aumid), &installed()), None);
    }

    #[test]
    fn desktop_entries_are_never_package_matches() {
        let apps = vec![app(
            "desktop",
            NOTEPAD,
            &format!(r"shell:appsFolder\{NOTEPAD}!App"),
        )];
        let aumid = format!("{NOTEPAD}!App");
        assert_eq!(packaged_installed_match(Some(&aumid), &apps), None);
    }
}

#[cfg(test)]
mod running_app_pids_tests {
    use super::running_app_pids;
    use crate::win32::WindowInfo;

    fn window(pid: u32, hwnd: u64) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid,
            title: String::new(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            is_on_screen: true,
            minimized: false,
        }
    }

    #[test]
    fn hosted_apps_follow_their_frame_host_once_each() {
        // Host pid 100's frames 7 and 8 host app pids 200 and 300; frame 9
        // hosts nothing the platform can attribute. Pid 42 is a plain window.
        let windows = [
            window(42, 1),
            window(100, 7),
            window(100, 8),
            window(100, 9),
            window(42, 2),
            window(100, 7),
        ];
        let hosted = |pid: u32, hwnd: u64| match (pid, hwnd) {
            (100, 7) => Some(200),
            (100, 8) => Some(300),
            _ => None,
        };
        assert_eq!(running_app_pids(&windows, hosted), vec![42, 100, 200, 300]);
    }
}
