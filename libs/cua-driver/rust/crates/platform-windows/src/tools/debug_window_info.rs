use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use windows::core::Interface as _;

// ── debug_window_info ─────────────────────────────────────────────────────────
//
// Diagnostic tool: dump everything the daemon (Session 2 when launched via
// `cua-driver autostart kick`) sees about a given pid's top-level windows.
// Includes Win32 class names, owning .exe basename + full path, and — when
// CUIAutomation is available — the currently-focused UIA element with the
// list of patterns it supports.
//
// Built for CUA-543 investigation: the routing between PostMessage-based
// input and UIA-pattern-based input depends on what the OS actually reports
// for modern XAML / UWP / WinUI3 targets. SSH-side PowerShell probes from
// Session 0 see nothing (cross-session HWNDs return empty class names);
// this tool gives the same view from inside the daemon process.

pub struct DebugWindowInfoTool;
static DEBUG_WI_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for DebugWindowInfoTool {
    fn def(&self) -> &ToolDef {
        DEBUG_WI_DEF.get_or_init(|| ToolDef {
            name: "debug_window_info".into(),
            description: "Diagnostic: dump everything cua-driver sees about a pid's \
                top-level windows from the daemon's session perspective. Returns \
                window class names, owning .exe basename + path, and — when \
                CUIAutomation succeeds — the focused UIA element with the list of \
                patterns it supports (ValuePattern, InvokePattern, TextPattern, \
                TogglePattern, etc.). Used to design / debug input routing for \
                XAML / UWP / WinUI3 targets; see CUA-543."
                .into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer","description":"PID of the process to inspect."}
            },"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid_v: u32 = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(p) if p > 0 && p <= u32::MAX as u64 => p as u32,
            _ => {
                return ToolResult::error("debug_window_info: missing or invalid `pid`".to_string())
            }
        };

        let outcome = tokio::task::spawn_blocking(move || -> serde_json::Value {
            use std::collections::HashSet;
            use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, BOOL, TRUE};
            use windows::Win32::System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize,
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            };
            use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
            use windows::Win32::System::Threading::{
                GetCurrentProcessId, OpenProcess,
                QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
                PROCESS_QUERY_LIMITED_INFORMATION,
            };
            use windows::Win32::UI::Accessibility::{
                CUIAutomation, IUIAutomation,
                UIA_InvokePatternId, UIA_TextPatternId, UIA_TogglePatternId,
                UIA_ValuePatternId, UIA_LegacyIAccessiblePatternId,
                UIA_SelectionItemPatternId, UIA_ExpandCollapsePatternId,
            };
            use windows::Win32::UI::WindowsAndMessaging::{
                EnumWindows, GetClassNameW, GetWindow, GetWindowTextW,
                GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
            };

            // Helper: read class name + title
            fn class_name(h: HWND) -> Option<String> {
                let mut buf = [0u16; 256];
                let n = unsafe { GetClassNameW(h, &mut buf) };
                if n <= 0 { None } else { Some(String::from_utf16_lossy(&buf[..n as usize])) }
            }
            fn window_title(h: HWND) -> Option<String> {
                let mut buf = [0u16; 512];
                let n = unsafe { GetWindowTextW(h, &mut buf) };
                if n <= 0 { None } else { Some(String::from_utf16_lossy(&buf[..n as usize])) }
            }

            // ── Process info: session id + exe path
            let mut session_id: u32 = 0;
            let _ = unsafe { ProcessIdToSessionId(pid_v, &mut session_id) };

            let (exe_path, exe_basename) = unsafe {
                match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid_v) {
                    Ok(h) => {
                        let mut buf = [0u16; 1024];
                        let mut len: u32 = buf.len() as u32;
                        let ok = QueryFullProcessImageNameW(
                            h, PROCESS_NAME_FORMAT(0),
                            windows::core::PWSTR(buf.as_mut_ptr()),
                            &mut len,
                        );
                        let _ = CloseHandle(h);
                        if ok.is_ok() && len > 0 {
                            let path = String::from_utf16_lossy(&buf[..len as usize]);
                            let base = path
                                .rsplit(|c: char| c == '\\' || c == '/')
                                .next()
                                .unwrap_or(&path)
                                .to_ascii_lowercase();
                            (Some(path), Some(base))
                        } else {
                            (None, None)
                        }
                    }
                    Err(_) => (None, None),
                }
            };

            // ── Enumerate top-level visible windows owned by this pid
            struct Ctx { target_pid: u32, hwnds: Vec<isize> }
            extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
                unsafe {
                    let ctx = &mut *(lparam.0 as *mut Ctx);
                    let mut p: u32 = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut p));
                    if p == ctx.target_pid && IsWindowVisible(hwnd).as_bool() {
                        // Only true top-level (no owner) windows.
                        let owner = GetWindow(hwnd, GW_OWNER);
                        if owner.unwrap_or_default().is_invalid() {
                            ctx.hwnds.push(hwnd.0 as isize);
                        }
                    }
                    TRUE
                }
            }
            let mut ctx = Ctx { target_pid: pid_v, hwnds: vec![] };
            let _ = unsafe {
                EnumWindows(Some(enum_cb), LPARAM(&mut ctx as *mut Ctx as isize))
            };

            // ── Init UIA (best-effort; non-fatal if it fails)
            let mut focused_info: Option<serde_json::Value> = None;
            let coinit_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
            if let Ok(uia) = unsafe {
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            } {
                if let Ok(focused) = unsafe { uia.GetFocusedElement() } {
                    // Patterns we care about for input routing
                    let pattern_ids = [
                        (UIA_ValuePatternId.0, "Value"),
                        (UIA_InvokePatternId.0, "Invoke"),
                        (UIA_TextPatternId.0, "Text"),
                        (UIA_TogglePatternId.0, "Toggle"),
                        (UIA_ExpandCollapsePatternId.0, "ExpandCollapse"),
                        (UIA_SelectionItemPatternId.0, "SelectionItem"),
                        (UIA_LegacyIAccessiblePatternId.0, "LegacyIAccessible"),
                    ];
                    let mut patterns: Vec<&str> = vec![];
                    for (id, name) in &pattern_ids {
                        let pid_struct = windows::Win32::UI::Accessibility::UIA_PATTERN_ID(*id);
                        if unsafe { focused.GetCurrentPattern(pid_struct) }.is_ok() {
                            patterns.push(name);
                        }
                    }
                    let name = unsafe { focused.CurrentName() }
                        .ok()
                        .map(|b| b.to_string())
                        .unwrap_or_default();
                    let class = unsafe { focused.CurrentClassName() }
                        .ok()
                        .map(|b| b.to_string())
                        .unwrap_or_default();
                    let ctype = unsafe { focused.CurrentControlType() }
                        .map(|t| t.0)
                        .unwrap_or(0);
                    let value_ro = if patterns.contains(&"Value") {
                        unsafe {
                            focused.GetCurrentPattern(UIA_ValuePatternId)
                                .and_then(|p| p.cast::<windows::Win32::UI::Accessibility::IUIAutomationValuePattern>())
                                .and_then(|vp| vp.CurrentIsReadOnly())
                                .map(|b| b.as_bool())
                                .ok()
                        }
                    } else { None };
                    focused_info = Some(serde_json::json!({
                        "name": name,
                        "class_name": class,
                        "control_type_id": ctype,
                        "supported_patterns": patterns,
                        "value_pattern_is_read_only": value_ro,
                    }));
                }
            }
            if coinit_ok {
                unsafe { CoUninitialize(); }
            }

            // ── Build per-window JSON
            let xaml_classes: HashSet<&str> = [
                "ApplicationFrameWindow",
                "WinUIDesktopWin32WindowClass",
                "Windows.UI.Core.CoreWindow",
                "Microsoft.UI.Content.DesktopChildSiteBridge",
            ].iter().copied().collect();
            let xaml_exes: HashSet<&str> = [
                "notepad.exe", "calculatorapp.exe", "calc.exe",
                "applicationframehost.exe", "photos.exe", "systemsettings.exe",
            ].iter().copied().collect();

            let windows_json: Vec<serde_json::Value> = ctx.hwnds.iter()
                .map(|&h| {
                    let hwnd = HWND(h as *mut _);
                    let cls = class_name(hwnd);
                    let title = window_title(hwnd);
                    let xaml_class_match = cls.as_deref()
                        .map(|c| xaml_classes.contains(c))
                        .unwrap_or(false);
                    serde_json::json!({
                        "hwnd": h,
                        "class_name": cls,
                        "title": title,
                        "xaml_class_match": xaml_class_match,
                    })
                })
                .collect();

            let exe_xaml_match = exe_basename.as_deref()
                .map(|e| xaml_exes.contains(e))
                .unwrap_or(false);

            let daemon_pid = unsafe { GetCurrentProcessId() };
            let mut daemon_session: u32 = 0;
            let _ = unsafe { ProcessIdToSessionId(daemon_pid, &mut daemon_session) };

            serde_json::json!({
                "pid": pid_v,
                "session_id": session_id,
                "daemon_pid": daemon_pid,
                "daemon_session_id": daemon_session,
                "exe_path": exe_path,
                "exe_basename": exe_basename,
                "exe_xaml_match": exe_xaml_match,
                "windows": windows_json,
                "focused_element": focused_info,
                "xaml_routing_recommended": exe_xaml_match
                    || windows_json.iter()
                        .any(|w| w.get("xaml_class_match").and_then(|v| v.as_bool()).unwrap_or(false)),
            })
        }).await;

        match outcome {
            Ok(j) => {
                let pretty = serde_json::to_string_pretty(&j).unwrap_or_else(|_| j.to_string());
                ToolResult::text(pretty).with_structured(j)
            }
            Err(e) => ToolResult::error(format!("debug_window_info: join error: {e}")),
        }
    }
}
