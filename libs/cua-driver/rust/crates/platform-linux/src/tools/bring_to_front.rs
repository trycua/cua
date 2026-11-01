use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

use super::impl_::{foreground_hyprland_action, foreground_hyprland_refusal};

// ── bring_to_front (Linux) ───────────────────────────────────────────────────

pub struct BringToFrontTool;

static BTF_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for BringToFrontTool {
    fn def(&self) -> &ToolDef {
        BTF_DEF.get_or_init(|| ToolDef {
            name: "bring_to_front".into(),
            description:
                "Persistently activate a window so subsequent input lands on it. \
                 This deliberately breaks the no-foreground contract and is not part \
                 of the normal input ladder. For an ordinary `background_unavailable` \
                 response, retry only the refused action with \
                 `delivery_mode:\"foreground\"`; the input tool performs its own \
                 activate, act, and restore sequence. Use `bring_to_front` only for a \
                 focus-proxy surface that must remain foreground across multiple calls, \
                 such as a remote desktop session, or when repeated action-scoped \
                 activation prevents the remote surface from accepting input. \
                 X11: EWMH _NET_ACTIVE_WINDOW activation (the `wmctrl -a` equivalent, \
                 with proper timestamp handling to beat focus-stealing prevention). \
                 Wayland: activates through a target-addressable compositor adapter \
                 (wlroots foreign-toplevel or the GNOME Shell helper) and refuses \
                 when the compositor offers no safe adapter. Matches the macOS / \
                 Windows bring_to_front rung."
                .into(),
            input_schema: serde_json::json!({
                "type":"object","required":["pid"],"properties":{
                    "pid":{"type":"integer","description":"Process ID of the app to activate."},
                    "window_id":{"type":"integer","description":"X11 window id (xid) to activate. If omitted, the first window of `pid` is used."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        if crate::wayland::is_wayland() {
            let window_id = match args.opt_u64("window_id") {
                Some(window_id) => window_id,
                None => match crate::wayland::list_windows_dispatch(Some(pid)).first() {
                    Some(window) => window.xid,
                    None => {
                        return ToolResult::error(format!(
                            "bring_to_front: no window_id given and no Wayland windows found for pid {pid}."
                        ))
                    }
                },
            };
            if crate::wayland::wayland_input_enabled() && crate::wayland::hyprland::is_session() {
                if args.opt_u64("window_id").is_none() {
                    return foreground_hyprland_refusal("an exact window_id is required");
                }
                return foreground_hyprland_action(
                    &args,
                    pid,
                    window_id,
                    crate::wayland::hyprland_input::Action::Activate,
                )
                .await;
            }
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::activate_window_for_input_target(window_id, Some(pid))
            })
            .await;
            return match result {
                Ok(Ok(())) => {
                    ToolResult::text(format!("Brought Wayland window {window_id} to front."))
                        .with_structured(serde_json::json!({
                            "window_id": window_id,
                            "platform": "linux",
                            "session": "wayland",
                        }))
                }
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }

        // X11: resolve the target xid (window_id, else first window for pid).
        let xid = match args.opt_u64("window_id") {
            Some(x) => x,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => {
                        return ToolResult::error(format!(
                        "bring_to_front: no window_id given and no windows found for pid {pid}."
                    ))
                    }
                }
            }
        };
        let r =
            tokio::task::spawn_blocking(move || crate::input::x11_activate_window_persistent(xid))
                .await;
        match r {
            Ok(Ok(prior)) => ToolResult::text(format!(
                "✅ Brought window {xid} to front (X11 _NET_ACTIVE_WINDOW)."
            ))
            .with_structured(serde_json::json!({
                "window_id": xid,
                "prior_active": prior,
                "platform": "linux",
            })),
            Ok(Err(e)) => ToolResult::error(format!("bring_to_front failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
