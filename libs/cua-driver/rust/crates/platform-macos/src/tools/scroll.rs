use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;

use super::ToolState;

pub struct ScrollTool {
    state: Arc<ToolState>,
}

impl ScrollTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "scroll".into(),
        description: "Scroll the target pid's focused region by synthesized keystrokes.\n\n\
            Mapping: by='page' → PageDown/PageUp × amount; by='line' → DownArrow/UpArrow × amount. \
            Horizontal variants use Left/Right arrow keys.\n\n\
            Optional element_index + window_id pre-focuses the element before scrolling.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "direction"],
            "properties": {
                "pid": { "type": "integer" },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction."
                },
                "by": {
                    "type": "string",
                    "enum": ["line", "page"],
                    "description": "Scroll granularity. Default: line."
                },
                "amount": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "Number of keystroke repetitions. Default: 3."
                },
                "window_id": { "type": "integer" },
                "element_index": { "type": "integer" }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    })
}

#[async_trait]
impl Tool for ScrollTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use mcp_server::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") { Ok(v) => v, Err(e) => return e };
        let direction = match args.require_str("direction") { Ok(v) => v, Err(e) => return e };
        let by = args.str_or("by", "line");
        let amount = args.u64_or("amount", 3) as usize;
        let element_index = args.opt_u64("element_index").map(|v| v as usize);
        let window_id = args.opt_u64("window_id").map(|v| v as u32);

        // Resolve the pre-focus element pointer (if requested) outside
        // the suppression closure — only the focus_element() write itself
        // needs to run under suppression, the cache lookup does not.
        let pre_focus_ptr: Option<usize> = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            self.state.element_cache.get_element_ptr(pid, wid, idx)
        } else {
            None
        };

        let key = match (by.as_str(), direction.as_str()) {
            ("page", "down")  | (_, "down") if by == "page"  => "pagedown",
            ("page", "up")    | (_, "up")   if by == "page"  => "pageup",
            ("line", "down")  | (_, "down")                  => "down",
            ("line", "up")    | (_, "up")                    => "up",
            (_, "left")                                       => "left",
            (_, "right")                                      => "right",
            _                                                 => "down",
        };
        let key = key.to_owned();

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Scroll keystrokes (PageDown / arrow) into search-box autocomplete
        // can spawn floating helper windows; rare but real. Wrap for parity
        // with the other action tools.
        //
        // The AX focus_element() pre-write also runs inside the closure so
        // any reflex activations it triggers are caught by both the wildcard
        // snapshot suppressor and the targeted FocusGuard lease.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "scroll.CGEvent",
            || async move {
                // Pre-focus the element under suppression so its
                // side-effects are captured by the snapshot + lease.
                if let Some(element_ptr) = pre_focus_ptr {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::input::ax_actions::focus_element(element_ptr)
                    }).await;
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }

                tokio::task::spawn_blocking(move || {
                    for _ in 0..amount {
                        if let Err(e) = crate::input::keyboard::press_key(pid, &key, &[]) {
                            return Err(e);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Ok(())
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Scrolled {direction} by {by} × {amount}.{}",
                changes.result_suffix()
            )),
            Ok(Err(e)) => ToolResult::error(format!("Scroll failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
