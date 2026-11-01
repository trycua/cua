use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    window_target::z_index_from_front_to_back,
};
use serde_json::{json, Value};

#[cfg(test)]
use super::impl_::exact_window_ownership_result;

// ── list_windows ─────────────────────────────────────────────────────────────

pub struct ListWindowsTool;

static LIST_WINDOWS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ListWindowsTool {
    fn def(&self) -> &ToolDef {
        LIST_WINDOWS_DEF.get_or_init(|| ToolDef {
            name: "list_windows".into(),
            // Description matches Swift `ListWindowsTool.swift` semantics —
            // Windows-specific caveat: no Spaces concept, so on_current_space
            // / space_ids are always omitted and current_space_id is null.
            description: "List every top-level window currently known to the window manager. \
                Each record self-contains its owning app identity so the caller never has to \
                join back against list_apps.\n\n\
                Use this — not list_apps — for any window-level reasoning: \"does this app \
                have a visible window right now?\", \"which of this pid's windows is the main \
                one?\".\n\n\
                Per-record fields: window_id (HWND), pid + app_name, title, \
                bounds {x, y, width, height}, layer (always 0), z_index (integer or null; higher \
                values are closer to the front; null means stacking order is unavailable and \
                callers must not infer one), is_on_screen, minimized. To select a frontmost \
                candidate, take the maximum integer z_index; if every value is null, use an \
                explicit fallback instead of relying on array order. The macOS-specific \
                on_current_space / space_ids fields are \
                omitted on Windows; current_space_id is null.\n\n\
                Inputs: pid (optional pid filter), app (app-name filter), on_screen_only (bool, \
                default false). Filter with app or pid: an unfiltered call lists every window. \
                The driver's own overlay is omitted.".into(),
            input_schema: json!({"type":"object","properties":{
                "pid":{"type":"integer","description":"Optional pid filter. When set, only this pid's windows are returned."},
                "app":{"type":"string","description":cua_driver_core::tool_args::LIST_WINDOWS_APP_FILTER_DESCRIPTION},
                "on_screen_only":{"type":"boolean","description":"When true, drop windows that aren't currently on-screen. Default false."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let filter_pid = args.opt_u64("pid").map(|v| v as u32);
        let on_screen_only = args.bool_or("on_screen_only", false);
        let (mut windows, pid_to_name) = tokio::task::spawn_blocking(move || {
            let wins = crate::win32::list_windows(filter_pid);
            let procs = crate::win32::list_processes();
            let map: std::collections::HashMap<u32, String> =
                procs.into_iter().map(|p| (p.pid, p.name)).collect();
            (wins, map)
        })
        .await
        .unwrap_or_default();
        if on_screen_only {
            windows.retain(|w| w.is_on_screen);
        }
        drop_own_windows(&mut windows, std::process::id());
        if let Some(filter) = args.opt_str("app") {
            windows.retain(|w| {
                let app_name = pid_to_name.get(&w.pid).map(String::as_str).unwrap_or("");
                cua_driver_core::tool_args::app_filter_matches(&filter, app_name, None)
            });
        }

        // Swift surfaces a warning when a pid filter matches nothing.
        let missing_pid_warning = if let Some(fp) = filter_pid {
            if windows.is_empty() {
                use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
                use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
                let fg_pid = unsafe {
                    let fg = GetForegroundWindow();
                    let mut p = 0u32;
                    let _ = GetWindowThreadProcessId(fg, Some(&mut p));
                    p
                };
                let fg_name = pid_to_name.get(&fg_pid).map(|s| s.as_str()).unwrap_or("?");
                Some(format!(
                    "⚠️ No windows found for pid {fp}. The pid may be wrong or the app may not \
                     have created a window yet. The current frontmost app appears to be \
                     \"{fg_name}\" (pid {fg_pid})."
                ))
            } else {
                None
            }
        } else {
            None
        };

        // z_index: list_windows merges EnumWindows first (which the Win32
        // window manager returns in canonical top-to-bottom z-order), then
        // appends any UIA-only HWNDs UIA found that EnumWindows missed
        // (UIA-only entries land at the bottom of the z-stack — they have
        // no canonical Win32 ordering). Higher list index = farther from
        // front. Swift convention: higher z_index = closer to front.
        // Invert via `(len - 1 - i)` so the front-most window gets the
        // largest z.
        let n = windows.len();
        let records: Vec<serde_json::Value> = windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let app_name = pid_to_name.get(&w.pid).cloned().unwrap_or_default();
                let z_index = z_index_from_front_to_back(n, i) as i64;
                json!({
                    "window_id":  w.hwnd,
                    "pid":        w.pid,
                    "app_name":   app_name,
                    "title":      w.title,
                    "bounds":     { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
                    "layer":      0,
                    "z_index":    z_index,
                    "is_on_screen": w.is_on_screen,
                    "minimized":    w.minimized,
                })
            })
            .collect();

        let total = records.len();
        let pid_count = windows
            .iter()
            .map(|w| w.pid)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let on_screen = records
            .iter()
            .filter(|r| r["is_on_screen"].as_bool().unwrap_or(false))
            .count();
        // Swift text format 1:1 — append the SPI-unavailable note since
        // Windows has no equivalent of macOS Spaces.
        let header = format!(
            "✅ Found {total} window(s) across {pid_count} app(s); {on_screen} on-screen. \
             (SkyLight Space SPIs unavailable — on_current_space / space_ids omitted.)"
        );
        let mut lines = vec![header];
        if let Some(warning) = missing_pid_warning {
            lines.push(warning);
        }
        for r in &records {
            let app = r["app_name"].as_str().unwrap_or("?");
            let pid = r["pid"].as_u64().unwrap_or(0);
            let tt = r["title"].as_str().unwrap_or("");
            let wid = r["window_id"].as_u64().unwrap_or(0);
            let title_disp = if tt.is_empty() {
                "(no title)".to_owned()
            } else {
                format!("\"{tt}\"")
            };
            let tag = if r["is_on_screen"].as_bool().unwrap_or(false) {
                ""
            } else {
                " [off-screen]"
            };
            lines.push(format!(
                "- {app} (pid {pid}) {title_disp} [window_id: {wid}]{tag}"
            ));
        }

        // Legacy alias: keep the old flat fields under each record for any
        // pre-existing Rust callers, but they read from the same source.
        let legacy_windows: Vec<serde_json::Value> = windows
            .iter()
            .map(|w| {
                json!({
                    "window_id": w.hwnd, "pid": w.pid, "title": w.title,
                    "x": w.x, "y": w.y, "width": w.width, "height": w.height,
                    "is_on_screen": w.is_on_screen, "minimized": w.minimized,
                })
            })
            .collect();
        let structured = json!({
            "windows":          records,
            "current_space_id": serde_json::Value::Null,  // Windows has no Spaces
            "_legacy_windows":  legacy_windows,
        });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}

fn drop_own_windows(windows: &mut Vec<crate::win32::WindowInfo>, own_pid: u32) {
    windows.retain(|w| w.pid != own_pid);
}

#[cfg(test)]
mod list_windows_z_index_tests {
    use super::{drop_own_windows, exact_window_ownership_result, z_index_from_front_to_back};
    use crate::win32::WindowInfo;

    #[test]
    fn explicit_pid_hwnd_guard_refuses_wrong_or_stale_owners() {
        assert!(exact_window_ownership_result(42, 7, Some(42)).is_ok());

        let wrong = exact_window_ownership_result(42, 7, Some(99)).unwrap_err();
        assert_eq!(wrong.is_error, Some(true));
        assert_eq!(
            wrong.structured_content.as_ref().unwrap()["code"],
            "window_target_mismatch"
        );
        assert_eq!(wrong.structured_content.as_ref().unwrap()["owner_pid"], 99);

        let stale = exact_window_ownership_result(42, 7, None).unwrap_err();
        assert_eq!(
            stale.structured_content.as_ref().unwrap()["code"],
            "window_target_not_found"
        );
    }

    fn window(hwnd: u64, pid: u32) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid,
            title: format!("window {hwnd}"),
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
            is_on_screen: true,
            minimized: false,
        }
    }

    #[test]
    fn the_drivers_own_windows_are_dropped_and_the_rest_keep_contiguous_z() {
        let mut windows = vec![window(1, 7), window(2, 42), window(3, 7), window(4, 9)];
        drop_own_windows(&mut windows, 7);
        assert_eq!(windows.iter().map(|w| w.hwnd).collect::<Vec<_>>(), [2, 4]);
        let z: Vec<_> = (0..windows.len())
            .map(|i| z_index_from_front_to_back(windows.len(), i))
            .collect();
        assert_eq!(z, [1, 0]);
    }
}
