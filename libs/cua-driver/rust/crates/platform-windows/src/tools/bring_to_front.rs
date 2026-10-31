use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

// ── bring_to_front ────────────────────────────────────────────────────────

pub struct BringToFrontTool;
static BTF_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for BringToFrontTool {
    fn def(&self) -> &ToolDef {
        BTF_DEF.get_or_init(|| ToolDef {
            name: "bring_to_front".into(),
            description: "Activate `pid`'s window (or `window_id` if specified) -- bring it to \
                the OS foreground. \n\n\
                **This deliberately breaks the no-foreground contract.** It is not part of \
                the normal input ladder. For an ordinary `background_unavailable` response, \
                retry only the refused action with `delivery_mode:\"foreground\"`; the input \
                tool performs its own activate, act, and restore sequence. Use \
                `bring_to_front` only for a focus-proxy surface that must remain foreground \
                across multiple calls, such as an RDP or Windows App session, or when repeated \
                action-scoped activation prevents the remote surface from accepting input. \n\n\
                Implementation uses the `AttachThreadInput` trick to bypass Windows' \
                foreground-lock when the daemon is not at UIAccess integrity. Returns \
                structured `{previous_fg_hwnd, now_fg_hwnd}` so callers can later restore. \
                Windows only; macOS / Linux return an error pointing at platform-native \
                alternatives.".into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"Optional HWND. Defaults to pid's first visible top-level window."}
            },"additionalProperties":false}),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let hwnd_opt = args.opt_u64("window_id");

        // Resolve hwnd: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "bring_to_front: no windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // Run the foreground swap on a blocking thread. The AttachThreadInput
        // trick mirrors `send_key_synthesized` (input/keyboard.rs:313-345)
        // and is validated by `flash-repro/16-edge-launch-fg.ps1` for the
        // Edge launch focus-steal recovery case.
        let outcome =
            tokio::task::spawn_blocking(move || -> Result<(u64, u64, bool, bool), String> {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::Graphics::Dwm::DwmFlush;
                use windows::Win32::UI::WindowsAndMessaging::{
                    GetForegroundWindow, IsIconic, IsWindow, SetWindowPos, ShowWindowAsync,
                    HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                    SW_RESTORE,
                };

                let target = HWND(hwnd as *mut _);
                if !unsafe { IsWindow(target) }.as_bool() {
                    return Err(format!("hwnd 0x{hwnd:x} is not a valid window"));
                }

                let prev_fg = unsafe { GetForegroundWindow() };
                let prev_fg_addr = prev_fg.0 as u64;

                // Iconic windows have no rendered pixels and live at the sentinel
                // (-32000, -32000) position. Restore before changing z-order so
                // bring_to_front is also the advertised recovery path for capture.
                let was_minimized = unsafe { IsIconic(target) }.as_bool();
                if was_minimized {
                    let _ = unsafe { ShowWindowAsync(target, SW_RESTORE) };
                    for _ in 0..20 {
                        if !unsafe { IsIconic(target) }.as_bool() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    if unsafe { IsIconic(target) }.as_bool() {
                        return Err(format!(
                            "restore request did not complete for minimized hwnd 0x{hwnd:x}"
                        ));
                    }
                }

                // Lock-free z-order raise FIRST: bring the window to the top of the
                // normal band (the HWND_TOPMOST→HWND_NOTOPMOST force-to-front trick)
                // so it's brought to the VISIBLE front even when the foreground-lock
                // denies focus. SWP_NOACTIVATE → no focus steal; SetWindowPos z-order
                // is not gated by the foreground-lock / UIAccess. This is the same
                // technique the delivery_mode:"foreground" pointer path uses.
                let raised = unsafe {
                    let a = SetWindowPos(
                        target,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    )
                    .is_ok();
                    let b = SetWindowPos(
                        target,
                        HWND_NOTOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    )
                    .is_ok();
                    a && b
                };

                // The caller explicitly requested a visible transition, so the
                // shared foreground helper may claim the most-recent-input token.
                let _ = unsafe { crate::input::force_foreground_assisted(target) };
                let now_fg = unsafe { GetForegroundWindow() };

                // A restored HWND can stop reporting iconic before its compositor
                // surface is painted. Flush DWM before returning, but do not make
                // the restore operation depend on any particular capture backend.
                // Capture has its own WGC fallback for freshly restored surfaces.
                if was_minimized {
                    let _ = unsafe { DwmFlush() };
                }
                Ok((prev_fg_addr, now_fg.0 as u64, raised, was_minimized))
            })
            .await;

        match outcome {
            Ok(Ok((prev, now, raised, restored))) => {
                let focused = now == hwnd;
                let msg = if focused {
                    format!("✅ bring_to_front: pid {pid} hwnd 0x{hwnd:x} is now foreground (was 0x{prev:x}).")
                } else if raised {
                    format!(
                        "bring_to_front: exact target hwnd 0x{hwnd:x} was raised in z-order, but \
                         Windows kept foreground on hwnd 0x{now:x}; foreground activation failed."
                    )
                } else {
                    format!(
                        "bring_to_front could neither focus nor raise target 0x{hwnd:x} (current \
                         foreground 0x{now:x})."
                    )
                };
                let structured = if focused {
                    json!({
                        "previous_fg_hwnd": format!("0x{prev:x}"),
                        "now_fg_hwnd": format!("0x{now:x}"),
                        "target_hwnd": format!("0x{hwnd:x}"),
                        "landed_on_target": true,
                        "raised": raised,
                        "restored": restored,
                    })
                } else {
                    json!({
                        "code": "foreground_unavailable",
                        "effect": "refused",
                        "pid": pid,
                        "window_id": hwnd,
                        "actual_foreground_window_id": now,
                        "previous_fg_hwnd": format!("0x{prev:x}"),
                        "now_fg_hwnd": format!("0x{now:x}"),
                        "target_hwnd": format!("0x{hwnd:x}"),
                        "landed_on_target": false,
                        "raised": raised,
                        "restored": restored,
                    })
                };
                if focused {
                    ToolResult::text(msg).with_structured(structured)
                } else {
                    ToolResult::error(msg).with_structured(structured)
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("bring_to_front: {e}")),
            Err(join) => {
                ToolResult::error(format!("bring_to_front: blocking-task join error: {join}"))
            }
        }
    }
}
