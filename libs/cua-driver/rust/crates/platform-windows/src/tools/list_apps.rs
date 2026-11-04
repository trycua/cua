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
        //    that have a user-facing surface.
        // 2. Enumerate the process table to map pid → executable name and full
        //    path (used to merge against Start-Menu / UWP launch_paths).
        // 3. Enumerate installed apps from Start-Menu shortcuts + WinRT
        //    PackageManager. Each installed entry has a launch_path.
        // 4. Merge: a running pid whose executable path matches an installed
        //    desktop launch_path becomes a single entry with running=true and
        //    that launch_path. Remaining installed entries are emitted with
        //    running=false, pid=0. Remaining running pids (no installed-app
        //    match) are emitted as running=true with launch_path=null.
        let apps = tokio::task::spawn_blocking(|| -> Vec<serde_json::Value> {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

            // Per-step timings logged via `tracing::debug!` (silent at default
            // log level — enable with `RUST_LOG=cua_driver=debug` when debugging
            // a slow list_apps invocation).
            let t0 = std::time::Instant::now();
            let wins = crate::win32::list_windows_via_win32(None);
            tracing::debug!(target: "list_apps", "step 1 list_windows: {} wins ({}ms)", wins.len(), t0.elapsed().as_millis());
            let mut seen_pids = std::collections::HashSet::new();
            let mut running_pids: Vec<u32> = Vec::new();
            for w in &wins {
                if seen_pids.insert(w.pid) { running_pids.push(w.pid); }
            }

            let t1 = std::time::Instant::now();
            let foreground_pid = unsafe {
                let fg = GetForegroundWindow();
                let mut pid = 0u32;
                let _ = GetWindowThreadProcessId(fg, Some(&mut pid));
                pid
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
                let key = name.to_ascii_lowercase();
                let candidates = by_exe.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);
                let merged = disambiguate_installed_match(candidates, &installed, &name);
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
