use async_trait::async_trait;
use cua_driver_core::{
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct GetWindowStateTool {
    state: Arc<ToolState>,
}

impl GetWindowStateTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_window_state".into(),
        description: "Walk a running app's AX tree and return BOTH a structured \
            `elements` array (preferred) AND a Markdown rendering of the same tree \
            (back-compat). Every actionable element is tagged with [element_index N] \
            in the markdown and as `element_index` in the structured array — pass \
            those indices to click, type_text, press_key, etc.\n\n\
            INVARIANT: call get_window_state once per turn per (pid, window_id) before any \
            element-indexed action. The index map is replaced by the next snapshot.\n\n\
            PREFERRED CONSUMERS read `structuredContent.elements` (one entry per \
            indexed row with `element_index`, `role`, `label`, `value` (the \
            element's text/AXValue when present — use it to verify what a field \
            holds), `frame: {x,y,w,h}`, `parent_index`, `depth`). The markdown \
            `tree_markdown` stays available \
            and unchanged in shape for existing text-parsing callers — but new \
            fields will only be added to the structured side.\n\n\
            Always returns BOTH the element tree AND a screenshot — ground on \
            both and cross-check (the tree lies on some surfaces: Electron \
            echo-confirms, Catalyst null values, virtualized off-viewport rows \
            with `h:1` frames). You choose the modality at ACTION time, not here: \
            an element ax action (pass `element_index`/`element_token` → the \
            accessibility rung) or an element px action (pass `x`,`y` → the pixel \
            rung, read straight off this screenshot). `capture_mode` is deprecated \
            and ignored. Pass `include_screenshot:false` to skip the grab and get \
            the tree only — the cheap path when you're just re-indexing before an \
            element ax action.\n\n\
            The snapshot is SCOPED to `window_id`: a window_id that no longer exists is \
            refused with `window_id_not_found`, and one owned by another process is \
            refused with `window_owner_pid_mismatch` naming the real `owner_pid` to retry \
            with (macOS hosts a sandboxed app's Open/Save panel out-of-process, so its \
            window belongs to the panel service, not the app). If the window is live under \
            this pid but its accessibility surface can't be resolved, the tree comes back \
            EMPTY with `degraded_reason: ax_window_unresolved` and the screenshot of the \
            requested window — act by pixel there. This tool never returns another \
            surface's elements under your window_id. Before exposing a screenshot, \
            its raw dimensions are validated as a coherent 1x/2x representation of \
            the requested WindowServer bounds. `px_frame_mismatch` or \
            `px_capture_unavailable` omits an unprovable screenshot/pixel frame \
            instead of guessing a transform; the truthful AX payload remains available.\n\n\
            Optional `query` projects both tree_markdown and structured `elements` to \
            matching lines plus their ancestor chain (case-insensitive substring). The \
            element_index values are unchanged, the complete snapshot remains actionable, \
            and `element_count` continues to report its total size; \
            `filtered_element_count` reports the projected response size.\n\n\
            Optional `max_elements` / `max_depth` bound the AX walk to mitigate \
            context-window blow-up on Electron / Obsidian / large web apps that \
            produce 10k+ element trees. When applied, BOTH the markdown \
            and the structured elements are truncated identically. Omit both for \
            current default behaviour (≤2 000 elements, depth ≤25).".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "window_id"],
            "properties": {
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
                "pid": { "type": "integer", "description": "Target process ID." },
                "window_id": { "type": "integer", "description": "Target window ID from list_windows." },
                "query": { "type": "string", "description": "Case-insensitive filter for tree_markdown and structured elements. Returns matching actionable rows plus their actionable ancestors without renumbering element_index values." },
                "capture_mode": cua_driver_core::capture_mode::capture_mode_schema(),
                "include_screenshot": {
                    "type": "boolean",
                    "description": "Default true — returns a grounding screenshot alongside the tree. Set false to skip the grab and return the tree only (the cheap path when you're just re-indexing before an element ax action; saves the image tokens + screen-grab latency). screenshot_out_file still forces a capture to disk."
                },
                "screenshot_out_file": {
                    "type": "string",
                    "description": "When set, write the PNG to this file path (~ expanded) instead of embedding base64 in the response. The structured output will contain screenshot_file_path instead."
                },
                "max_elements": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Cap on the total number of AX nodes walked. Truncates depth-first; markdown and structured elements truncate together. Omit for the default (2 000). Lower this for Electron / Obsidian / large web apps that produce 10k+ element trees and blow context windows."
                },
                "max_depth": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Cap on the AX-tree walk depth. Nodes whose rendered indent would exceed this are omitted. Omit for the default (25). Lower this for deep menu/Electron trees."
                }
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: false,
        open_world: false,
    })
}

fn chromium_browser_window(pid: i32) -> bool {
    let identity = format!(
        "{} {}",
        crate::apps::get_app_name_for_pid(pid).unwrap_or_default(),
        crate::apps::bundle_id_for_pid(pid).unwrap_or_default()
    )
    .to_ascii_lowercase();
    identity
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| {
            matches!(
                token,
                "chrome"
                    | "chromium"
                    | "brave"
                    | "edge"
                    | "vivaldi"
                    | "opera"
                    | "arc"
                    | "thorium"
                    | "iridium"
                    | "yandex"
            )
        })
}

#[async_trait]
impl Tool for GetWindowStateTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let window_id = match args.require_u32("window_id") {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Issue #2237: pre-flight the requested window against WindowServer
        // BEFORE the (up to 20 s) AX walk. An id that no window carries, or
        // that another process owns, used to fall through the scoped filter and
        // return the app's MENU BAR as a healthy snapshot of the requested
        // window — with a screenshot of the requested window beside it. macOS
        // hosts every sandboxed app's Open/Save panel in
        // `com.apple.appkit.xpc.openAndSavePanelService`, so the owner-mismatch
        // shape is routine, and the caller must be told the real owner pid.
        {
            let owner = match tokio::task::spawn_blocking(move || {
                crate::windows::resolve_window_owner(pid, window_id)
            })
            .await
            {
                Ok(owner) => owner,
                Err(e) => {
                    return ToolResult::error(format!(
                        "window ownership lookup for window_id {window_id} failed: {e}"
                    ))
                }
            };
            if let Some(scope) = crate::ax::window_scope::scope_from_owner(&owner) {
                if let Some(refusal) = window_scope_refusal(pid, window_id, &scope) {
                    return refusal;
                }
            }
        }

        let query = args.opt_str("query");
        let screenshot_out_file = args.opt_str("screenshot_out_file").map(|s| {
            // Expand ~ prefix.
            if let Some(relative) = s.strip_prefix("~/") {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/{relative}")
            } else {
                s
            }
        });
        // Effective config resolves call-arg > session-override > global. The
        // daemon injects `_session_id` for named MCP sessions; absent => global.
        let session_id = args.opt_str("_session_id");
        let effective_max_dim = {
            let cfg = self.state.config.read().unwrap();
            self.state
                .session_config
                .effective_max_image_dimension(session_id.as_deref(), &cfg)
        };
        // `capture_mode` is DEPRECATED and ignored — get_window_state always
        // returns BOTH the tree and a screenshot now, so the agent grounds on
        // both and cross-checks (the AX tree lies often enough that a grounding
        // screenshot should always be present). The modality is chosen at action
        // time: an element ax action (element_index) or element px action (x,y).
        // We don't even read the arg; it stays in the schema only so old callers
        // don't trip additionalProperties:false.
        //
        // `include_screenshot` (default true) is the perf opt-out: set false to
        // skip the grab and return the tree only — the cheap path when you're
        // just re-indexing before an element ax action. `screenshot_out_file`
        // still forces a capture (an explicit "write the frame to disk").
        let include_screenshot = args.get("include_screenshot").and_then(|v| v.as_bool());
        let should_capture = include_screenshot != Some(false) || screenshot_out_file.is_some();
        // Internal direct-tool mode used by verify_state. Registry ingress
        // strips underscore-prefixed arguments before public dispatch; only
        // a trusted direct in-process invocation can enable this mode.
        let observation_only = args
            .get("_observation_only")
            .and_then(|value| value.as_bool())
            == Some(true);
        // Optional caps — when omitted, fall back to the defaults baked into
        // the AX walker (#22865). minimum:1 keyed in the schema, but defend
        // against 0 here as well so a misbehaving client can't disable the
        // walk entirely.
        let max_elements = args
            .get("max_elements")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(crate::ax::tree::DEFAULT_MAX_ELEMENTS);
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(crate::ax::tree::DEFAULT_MAX_DEPTH);

        // Always walk the AX tree (perception returns both tree + screenshot).
        let tree_result = {
            let q = query.clone();
            // Keep the product deadline below the public client's 25-second
            // deadline so callers receive a structured driver error. The AX
            // walker also applies a native per-element messaging timeout because
            // dropping a spawn_blocking JoinHandle cannot cancel a blocked AX call.
            let walk_future = tokio::task::spawn_blocking(move || {
                crate::ax::tree::walk_tree_bounded(
                    pid,
                    Some(window_id),
                    q.as_deref(),
                    max_elements,
                    max_depth,
                )
            });
            match tokio::time::timeout(std::time::Duration::from_secs(20), walk_future).await {
                Ok(Ok(r)) => Some(r),
                Ok(Err(e)) => return ToolResult::error(format!("AX tree walk failed: {e}")),
                Err(_elapsed) => {
                    return ToolResult::error(format!(
                        "AX tree walk for pid={pid} timed out after 20 s. \
                         The app (likely Arc, Electron, or Safari with many tabs) has a \
                         pathologically large accessibility tree. \
                         Workaround: re-call with a depth-limited scan \
                         (max_elements / max_depth), then act by pixel (x,y) off \
                         the screenshot if the tree stays unusable."
                    ));
                }
            }
        };

        // The window can close, or its CGWindow can be re-parented onto another
        // process, between the pre-flight and the walk. Re-apply the same
        // refusals against what the walk actually observed.
        let window_scope = tree_result.as_ref().and_then(|r| r.window_scope.clone());
        if let Some(ref scope) = window_scope {
            if let Some(refusal) = window_scope_refusal(pid, window_id, scope) {
                return refusal;
            }
        }
        // `window_scope` is None only when no window_id was requested, which
        // this tool never does — so treat that as resolved.
        let scope_matched = window_scope.as_ref().is_none_or(|s| s.is_matched());

        // Update element cache — ONLY for a resolved window scope. Caching an
        // unresolved scope's nodes under (pid, window_id) is what turned a
        // wrong-surface snapshot into a wrong-surface *action*: a follow-up
        // click(element_index=N) picked whatever the walk happened to return.
        // For an unresolved scope, replace any prior entry with an empty
        // snapshot so a stale index map cannot be clicked through either.
        if !observation_only {
            if let Some(ref r) = tree_result {
                if scope_matched {
                    self.state.element_cache.update(pid, window_id, &r.nodes);
                } else {
                    self.state.element_cache.update(pid, window_id, &[]);
                }
            }
        }

        // Capture the screenshot and deliver it alongside the tree — the
        // grounding frame the agent cross-checks the (sometimes-lying) tree
        // against. Skipped only when `include_screenshot:false` (and no
        // screenshot_out_file). With `screenshot_out_file` set, write to disk and
        // surface the path instead of embedding base64; otherwise embed base64.
        let max_dim = effective_max_dim;
        // Returns the encoded/file capture, delivered dimensions, optional
        // downscale source width, the WindowServer bounds it was validated
        // against, and the raw capture's backing scale.
        let mut screenshot_frame_error = None;
        let screenshot = if should_capture {
            let out_file = screenshot_out_file.clone();
            let res = tokio::task::spawn_blocking(move || -> Result<
                (
                    Option<String>,
                    Option<String>,
                    u32,
                    u32,
                    Option<u32>,
                    crate::windows::WindowBounds,
                    f64,
                ),
                super::px_frame::PxFrameError,
            > {
                use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                let bounds = crate::windows::window_bounds_by_id(window_id)
                    .filter(|b| b.width > 0.0 && b.height > 0.0)
                    .ok_or(super::px_frame::PxFrameError::WindowNotFound { window_id })?;
                let raw = crate::capture::screenshot_window_bytes(window_id).map_err(|e| {
                    super::px_frame::PxFrameError::CaptureUnavailable {
                        window_id,
                        reason: e.to_string(),
                    }
                })?;
                let (orig_w, orig_h) = crate::capture::png_dimensions(&raw).map_err(|e| {
                    super::px_frame::PxFrameError::CaptureUnavailable {
                        window_id,
                        reason: e.to_string(),
                    }
                })?;
                let scale =
                    super::px_frame::validate_capture_frame(window_id, &bounds, orig_w, orig_h)?;
                let png = crate::capture::resize_png_if_needed(&raw, max_dim).map_err(|e| {
                    super::px_frame::PxFrameError::CaptureUnavailable {
                        window_id,
                        reason: e.to_string(),
                    }
                })?;
                let (w, h) = crate::capture::png_dimensions(&png).map_err(|e| {
                    super::px_frame::PxFrameError::CaptureUnavailable {
                        window_id,
                        reason: e.to_string(),
                    }
                })?;
                let original_w = if w < orig_w { Some(orig_w) } else { None };
                if let Some(ref path) = out_file {
                    std::fs::write(path, &png).map_err(|e| {
                        super::px_frame::PxFrameError::CaptureUnavailable {
                            window_id,
                            reason: e.to_string(),
                        }
                    })?;
                    Ok((
                        None,
                        Some(path.clone()),
                        w,
                        h,
                        original_w,
                        bounds,
                        scale,
                    ))
                } else {
                    Ok((
                        Some(BASE64.encode(&png)),
                        None,
                        w,
                        h,
                        original_w,
                        bounds,
                        scale,
                    ))
                }
            }).await;
            match res {
                Ok(Ok((b64, file_path, w, h, orig_w, bounds, scale))) => {
                    // Record resize ratio so ClickTool can scale coordinates back
                    // up. Keyed per window: two windows of one pid can carry
                    // different ratios (only the large one downscales), and a
                    // pid-only key leaked one window's ratio into the other's
                    // pixel clicks.
                    if !observation_only {
                        if let Some(ow) = orig_w {
                            if w > 0 {
                                self.state.resize_registry.set_ratio(
                                    pid,
                                    window_id,
                                    ow as f64 / w as f64,
                                );
                            }
                        } else {
                            self.state.resize_registry.clear_ratio(pid, window_id);
                        }
                    }
                    Some((b64, file_path, w, h, bounds, scale))
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        "Screenshot frame could not be verified for window {window_id}: {e:?}"
                    );
                    if !observation_only {
                        self.state.resize_registry.clear_ratio(pid, window_id);
                    }
                    screenshot_frame_error = Some(e);
                    None
                }
                Err(e) => {
                    tracing::warn!("Screenshot task error for window {window_id}: {e}");
                    None
                }
            }
        } else {
            None
        };

        // Capture screenshot dimensions before consuming.
        let screenshot_dims = screenshot.as_ref().map(|(_, _, w, h, _, _)| (*w, *h));
        let screenshot_file_path = screenshot
            .as_ref()
            .and_then(|(_, fp, _, _, _, _)| fp.clone());
        let screenshot_frame = screenshot
            .as_ref()
            .map(|(_, _, _, _, bounds, scale)| (bounds.clone(), *scale));

        // Build response.
        let mut content: Vec<Content> = Vec::new();

        if let Some((b64_opt, _file_path, w, h, _bounds, _scale)) = screenshot {
            if let Some(b64) = b64_opt {
                content.push(Content::image_png(b64));
            }

            // Summary text line (matching Swift reference format).
            let element_count = tree_result
                .as_ref()
                .map(|r| r.nodes.iter().filter(|n| n.element_index.is_some()).count())
                .unwrap_or(0);
            let summary = if let Some(ref r) = tree_result {
                format!(
                    "window_id={window_id} pid={pid} size={}x{} elements={element_count}\n\n{}",
                    w, h, r.tree_markdown
                )
            } else {
                format!("window_id={window_id} pid={pid} size={}x{}", w, h)
            };
            content.push(Content::text(summary));
        } else if let Some(ref r) = tree_result {
            let element_count = r.nodes.iter().filter(|n| n.element_index.is_some()).count();
            content.push(Content::text(format!(
                "window_id={window_id} pid={pid} elements={element_count}\n\n{}",
                r.tree_markdown
            )));
        }

        if content.is_empty() {
            return ToolResult::error(
                "No content produced (neither AX tree nor screenshot succeeded)",
            );
        }

        let element_count = tree_result
            .as_ref()
            .map(|r| r.nodes.iter().filter(|n| n.element_index.is_some()).count())
            .unwrap_or(0);
        let tree_md = tree_result
            .as_ref()
            .map(|r| r.tree_markdown.clone())
            .unwrap_or_default();

        // Surface 6: register a snapshot in the global token registry so
        // every actionable element gets an opaque `element_token` keyed
        // to (pid, this snapshot id). The integer `element_index` stays
        // alongside unchanged — the token is additive. Snapshot id is
        // generated even when the walk returned no elements so consumers
        // calling `get_window_state` and then immediately re-snapshotting
        // get a clean LRU step every time.
        //
        // Skipped entirely for an unresolved window scope: an element_token is
        // a promise that index N addresses a row of THIS window, and there is
        // no such row to promise (issue #2237).
        let elem_count_for_snapshot = tree_result
            .as_ref()
            .map(|r| r.nodes.iter().filter(|n| n.element_index.is_some()).count())
            .unwrap_or(0);
        let snapshot_id = if scope_matched && !observation_only {
            Some(cua_driver_core::element_token::global().register_snapshot(
                pid,
                window_id,
                elem_count_for_snapshot,
            ))
        } else {
            None
        };

        // Build the structured `elements` array — one entry per actionable
        // node, matching the order (and indices) of the markdown rendering.
        // This is the preferred consumption path; `tree_markdown` is kept
        // alongside for back-compat with existing text-parsing callers
        // (Hermes' regex parser, Codex, Claude Code) and is signalled as
        // preferred-for-back-compat-only via the `_note` field below.
        let elements_json: Vec<serde_json::Value> = match (snapshot_id, tree_result.as_ref()) {
            (Some(sid), Some(r)) => build_elements_array_with_token(&r.nodes, sid),
            (None, Some(r)) if scope_matched => build_elements_array(&r.nodes),
            _ => Vec::new(),
        };
        let elements_json = cua_driver_core::element_query::project_elements_for_query(
            elements_json,
            query.as_deref(),
            &tree_md,
        );
        let filtered_element_count = elements_json.len();
        // The structured array intentionally contains only actionable nodes,
        // and AX child reads can fail independently of the element/depth caps.
        // Until the walker exposes a proof over the projected search domain,
        // absence must remain unknown rather than being claimed complete.
        let elements_complete = false;

        let mut structured = serde_json::json!({
            "window_id": window_id,
            "pid": pid,
            "element_count": element_count,
            "total_element_count": element_count,
            "returned_element_count": filtered_element_count,
            "elements_complete": elements_complete,
            "tree_markdown": tree_md,
            "elements": elements_json,
            "_note": "Prefer `elements` — `tree_markdown` will continue to work \
                but new fields will only be added to the structured side. \
                Issue #22865: use `max_elements` / `max_depth` to bound the \
                AX walk on apps with very large trees."
        });
        if query.is_some() {
            structured["filtered_element_count"] = serde_json::json!(filtered_element_count);
        }
        // Surface 6: an opaque snapshot identifier consumers can log
        // alongside the per-element tokens for debug correlation. Same value
        // embedded in every `element_token` emitted in `elements[]` above.
        // Additive — old consumers ignore it. Absent when no snapshot was
        // registered (unresolved window scope).
        if let Some(sid) = snapshot_id {
            structured["snapshot_id"] =
                serde_json::json!(cua_driver_core::element_token::token_for(sid, 0)
                    .trim_end_matches(":0")
                    .to_string());
        }
        // Best-effort-background ladder, rung (2). Both rungs point the agent at
        // the same next move: an empty AX tree means element_index has nothing
        // to bind to, so the deliberate action is an element px action — read
        // the screenshot already in this response and click by pixel (x,y).
        // macOS can pixel-target in the background, so the recommendation is
        // `px`, not `foreground`.
        match degradation_for(tree_result.is_some(), element_count, window_scope.as_ref()) {
            Degradation::None => {}
            Degradation::AxTreeEmpty => {
                structured["degraded"] = serde_json::json!(true);
                structured["degraded_reason"] = serde_json::json!(
                    "ax_tree_empty: the AX walk returned no actionable elements. The \
                     window may be a non-AX surface (canvas/WebGL/custom-drawn) or its \
                     accessibility tree was not ready (Chromium/Electron require an \
                     AX-enable + settle). Do not treat element data as authoritative — \
                     re-snapshot if the app just launched, otherwise switch to the \
                     visual path."
                );
                structured["escalation"] = serde_json::json!({
                    "recommended": "px",
                    "reason": "non-AX surface — act by pixel (x,y) off the screenshot \
                               in this response (an element px action)."
                });
            }
            Degradation::AxWindowUnresolved { ax_window_count } => {
                structured["degraded"] = serde_json::json!(true);
                structured["degraded_reason"] = serde_json::json!(format!(
                    "ax_window_unresolved: window_id {window_id} exists and is owned by \
                     pid {pid}, but none of the {ax_window_count} AXWindow element(s) \
                     under that pid reports this CGWindowID. The tree is returned EMPTY \
                     on purpose: the accessibility elements reachable under this pid \
                     belong to other surfaces (the menu bar, other windows), not to the \
                     requested window, so presenting them would misground the next \
                     action."
                ));
                structured["escalation"] = serde_json::json!({
                    "recommended": "px",
                    "reason": "act by pixel (x,y) off the screenshot in this response — \
                               the frame IS the requested window even though its AX \
                               surface is unresolved."
                });
            }
        }
        if let Some((sw, sh)) = screenshot_dims {
            structured["screenshot_width"] = serde_json::json!(sw);
            structured["screenshot_height"] = serde_json::json!(sh);
            // Surface 7: emit an explicit `screenshot_mime_type` on the
            // structured payload so consumers don't have to sniff the magic
            // bytes off the base64 PNG (`iVBOR` = PNG, `/9j/` = JPEG) to
            // know what they're holding. `Content::image_png` already carries
            // `mimeType` on the protocol image part — this mirrors it onto
            // the structured side. Additive: keeps every existing field.
            structured["screenshot_mime_type"] = serde_json::json!("image/png");
        }
        if let Some((bounds, scale)) = screenshot_frame {
            structured["window_bounds"] = serde_json::json!({
                "x": bounds.x,
                "y": bounds.y,
                "width": bounds.width,
                "height": bounds.height
            });
            structured["screenshot_scale"] = serde_json::json!(scale);
            structured["screenshot_frame_valid"] = serde_json::json!(true);
        }
        if let Some(error) = screenshot_frame_error {
            structured["screenshot_frame_valid"] = serde_json::json!(false);
            structured["screenshot_error"] = super::px_frame::error_structured(&error);
        }
        if let Some(ref fp) = screenshot_file_path {
            structured["screenshot_file_path"] = serde_json::json!(fp);
        }
        cua_driver_core::window_inspection::mark_browser_chrome_capture_coverage(
            &mut structured,
            chromium_browser_window(pid),
        );
        ToolResult {
            content,
            is_error: None,
            structured_content: Some(structured),
            action_record: None,
        }
    }
}

/// Turn an unresolvable window scope into a structured refusal, or `None` when
/// the scope is one the caller can still be served (issue #2237).
///
/// Refusing is the point: the pre-fix behaviour returned the app's menu bar
/// under the requested `window_id`, which reads as a healthy snapshot and gets
/// clicked by `element_index`. Both refusals name the exact retry, matching the
/// remedy-in-the-refusal shape the rest of the driver uses.
///
/// The owner pid is REPORTED, not followed: `element_cache`, the element-token
/// registry and `ResizeRegistry` are all keyed on the caller-supplied pid, so
/// walking under `owner_pid` while echoing the requested pid would hand back
/// indices the caller replays against the wrong key. One retry with the named
/// pid is correct and cheap.
fn window_scope_refusal(
    pid: i32,
    window_id: u32,
    scope: &crate::ax::WindowScope,
) -> Option<ToolResult> {
    use crate::ax::WindowScope;
    match scope {
        // Resolved, or resolvable-as-degraded — the caller gets a response.
        WindowScope::Matched | WindowScope::AxUnresolved { .. } => None,
        WindowScope::NotFound => Some(
            ToolResult::error(format!(
                "window_id {window_id} is not a live window (closed, or the id is stale). \
                 Refusing to return an accessibility tree, because the elements reachable \
                 under pid {pid} belong to other surfaces — not to the window you asked \
                 for. Call list_windows for current window_ids."
            ))
            .with_structured(serde_json::json!({
                "code": "window_id_not_found",
                "pid": pid,
                "window_id": window_id,
                "suggestion": "call list_windows for current window_ids; the window may have closed"
            })),
        ),
        WindowScope::OwnerPidMismatch {
            owner_pid,
            owner_app_name,
        } => Some(
            ToolResult::error(format!(
                "window_id {window_id} is owned by pid {owner_pid} (\"{owner_app_name}\"), \
                 not pid {pid}. macOS hosts a sandboxed app's Open/Save panel \
                 out-of-process, so the panel's CGWindowID belongs to the panel service \
                 rather than the app that opened it. Refusing to return pid {pid}'s \
                 accessibility tree for it. Re-call get_window_state with pid={owner_pid} \
                 and the same window_id."
            ))
            .with_structured(serde_json::json!({
                "code": "window_owner_pid_mismatch",
                "pid": pid,
                "window_id": window_id,
                "owner_pid": owner_pid,
                "owner_app_name": owner_app_name,
                "suggestion": format!(
                    "window_id {window_id} is owned by pid {owner_pid}, not pid {pid} \
                     (macOS hosts sandboxed Open/Save panels out-of-process). Re-call \
                     get_window_state with pid={owner_pid} and the same window_id."
                )
            })),
        ),
    }
}

/// Which degradation rung a snapshot lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Degradation {
    /// Clean snapshot — no `degraded` field is emitted.
    None,
    /// A walk ran and produced no actionable elements.
    AxTreeEmpty,
    /// The requested window is live and owned by this pid, but no AXWindow
    /// claims its CGWindowID, so the walk deliberately covered nothing.
    AxWindowUnresolved { ax_window_count: usize },
}

/// Decide the degradation rung. Pure: `walk_attempted` is false in the
/// screenshot-only path (an empty tree is expected there, not degraded), and
/// the unresolved-scope rung outranks the generic empty-tree rung because it
/// explains *why* the tree is empty.
fn degradation_for(
    walk_attempted: bool,
    element_count: usize,
    scope: Option<&crate::ax::WindowScope>,
) -> Degradation {
    if !walk_attempted {
        return Degradation::None;
    }
    if let Some(crate::ax::WindowScope::AxUnresolved { ax_window_count }) = scope {
        return Degradation::AxWindowUnresolved {
            ax_window_count: *ax_window_count,
        };
    }
    if element_count == 0 {
        return Degradation::AxTreeEmpty;
    }
    Degradation::None
}

/// Render the actionable nodes from the AX walk into the
/// `structuredContent.elements` array shape described on the tool: one entry
/// per node with an `element_index`, carrying role, label (built from
/// title/description/value/identifier), frame, parent_index, depth, and —
/// Surface 6 — an opaque `element_token` for the same row.
///
/// Order matches the markdown rendering exactly (DFS, same indices). Only
/// nodes that received an `element_index` (i.e. are addressable via
/// click(element_index=N)) appear — non-actionable display-only rows are
/// omitted to match the contract on the tool description.
pub(crate) fn build_elements_array_with_token(
    nodes: &[crate::ax::tree::AXNode],
    snapshot_id: u32,
) -> Vec<serde_json::Value> {
    nodes
        .iter()
        .filter_map(|node| {
            let idx = node.element_index?;
            // `label` is a best-effort human-readable string: title first,
            // then description, then value, then identifier. Mirrors what
            // a human reading the markdown row would call this element.
            let label = node
                .title
                .clone()
                .or_else(|| node.description.clone())
                .or_else(|| node.value.clone())
                .or_else(|| node.identifier.clone());
            let frame = node
                .frame
                .map(|[x, y, w, h]| serde_json::json!({ "x": x, "y": y, "w": w, "h": h }));
            let mut entry = serde_json::json!({
                "element_index": idx,
                // Surface 6: opaque token paired to the integer index.
                // Tools accept either; the token has explicit validity
                // (invalidated when the next snapshot supersedes this
                // one in the per-pid LRU). See cua-driver-core's
                // `element_token` module.
                "element_token": cua_driver_core::element_token::token_for(snapshot_id, idx),
                "role": node.role,
                "depth": node.depth,
            });
            if let Some(label) = label {
                entry["label"] = serde_json::Value::String(label);
            }
            // Surface the element's AXValue separately from `label`. `label`
            // collapses title→description→value→identifier into one display
            // string, so on a control that has BOTH a title/description AND a
            // value (e.g. a "Compose message" text field holding typed text),
            // the value is shadowed and invisible to a caller reading the
            // structured side — it only showed up in `tree_markdown`, forcing a
            // markdown grep to verify what landed. Emit it explicitly so the
            // verify-then-escalate loop can read the typed text structurally.
            // `value_state` widens the string-only AXValue read to all CF
            // types (CFNumber sliders → "8", CFBoolean checkboxes/radios →
            // "1"/"0") — controls whose state was previously invisible here.
            // Falls back to `value` so the field never regresses for
            // string-valued elements.
            if let Some(value) = node
                .value_state
                .clone()
                .or_else(|| node.value.clone())
                .filter(|v| !v.is_empty())
            {
                entry["value"] = serde_json::Value::String(value);
            }
            if let Some(desc) = node.value_description.clone() {
                entry["value_description"] = serde_json::Value::String(desc);
            }
            // Only surface a real range: WebKit reports AXMinValue/AXMaxValue
            // as 0.0/0.0 on non-range controls (checkboxes, radios), which
            // would be pure noise on every two-state element.
            if let (Some(min), Some(max)) = (node.min_value, node.max_value) {
                if max > min {
                    entry["min"] = serde_json::json!(min);
                    entry["max"] = serde_json::json!(max);
                }
            }
            if let Some(enabled) = node.enabled {
                entry["enabled"] = serde_json::Value::Bool(enabled);
            }
            let selected = node.selected.or_else(|| {
                let role = node.role.to_ascii_lowercase();
                if role.contains("checkbox") || role.contains("radiobutton") {
                    node.value_state.as_deref().and_then(|value| match value {
                        "1" | "true" | "on" => Some(true),
                        "0" | "false" | "off" => Some(false),
                        _ => None,
                    })
                } else {
                    None
                }
            });
            if let Some(selected) = selected {
                entry["selected"] = serde_json::Value::Bool(selected);
            }
            if node.in_web_content {
                entry["in_web_content"] = serde_json::Value::Bool(true);
            }
            if let Some(frame) = frame {
                entry["frame"] = frame;
            }
            if let Some(parent) = node.parent_element_index {
                entry["parent_index"] = serde_json::json!(parent);
            }
            Some(entry)
        })
        .collect()
}

/// Back-compat wrapper for callers that don't yet have a snapshot id
/// to pass through. Emits the same fields as the token-aware builder
/// minus `element_token`. New call sites should prefer
/// `build_elements_array_with_token`.
#[allow(dead_code)]
pub(crate) fn build_elements_array(nodes: &[crate::ax::tree::AXNode]) -> Vec<serde_json::Value> {
    // Use a snapshot_id of 0 only to satisfy the signature; tokens
    // built from id=0 are not registered and would fail the registry's
    // stale check — but since this entry point is only kept for
    // pre-existing callers (none in production after Surface 6), it
    // strips the token field after rendering.
    let mut out = build_elements_array_with_token(nodes, 0);
    for entry in &mut out {
        if let Some(obj) = entry.as_object_mut() {
            obj.remove("element_token");
        }
    }
    out
}

/// Keep the structured response aligned with a query-filtered markdown tree.
///
/// The AX walker deliberately keeps the complete node/cache snapshot so the
/// original element indices remain valid. The rendered markdown already holds
/// the exact matching rows and ancestor chain, so use its indices as the
/// projection source of truth instead of duplicating query matching over the
/// structured fields.
#[cfg(test)]
mod window_scope_contract_tests {
    use super::*;
    use crate::ax::WindowScope;

    fn panel_mismatch() -> WindowScope {
        WindowScope::OwnerPidMismatch {
            owner_pid: 900,
            owner_app_name: "Open and Save Panel Service".into(),
        }
    }

    fn structured(result: ToolResult) -> serde_json::Value {
        assert_eq!(result.is_error, Some(true), "must be an error result");
        result
            .structured_content
            .expect("refusals carry structured content")
    }

    #[test]
    fn stale_window_id_is_a_structured_not_found() {
        let s = structured(
            window_scope_refusal(800, 67340, &WindowScope::NotFound).expect("must refuse"),
        );
        assert_eq!(s["code"], "window_id_not_found");
        assert_eq!(s["pid"], 800);
        assert_eq!(s["window_id"], 67340);
        assert!(s["suggestion"].as_str().unwrap().contains("list_windows"));
    }

    /// Issue #2237's reported case: TextEdit's Open panel window belongs to the
    /// out-of-process panel service. The refusal must name the real owner pid
    /// so the caller can retry, and must NOT redirect on its own (the element
    /// caches are keyed on the caller-supplied pid).
    #[test]
    fn owner_pid_mismatch_names_the_owner_and_the_retry() {
        let refusal = window_scope_refusal(800, 67340, &panel_mismatch()).expect("must refuse");
        let text = format!("{:?}", refusal.content);
        let s = structured(refusal);
        assert_eq!(s["code"], "window_owner_pid_mismatch");
        assert_eq!(s["owner_pid"], 900);
        assert_eq!(s["owner_app_name"], "Open and Save Panel Service");
        assert_eq!(s["pid"], 800, "the requested pid is echoed, not replaced");
        assert!(
            s["suggestion"].as_str().unwrap().contains("pid=900"),
            "the retry must name the owner pid: {}",
            s["suggestion"]
        );
        assert!(
            !text.contains("AXMenuBar"),
            "the refusal must never carry menu-bar content"
        );
    }

    #[test]
    fn resolvable_scopes_are_not_refused() {
        assert!(window_scope_refusal(800, 11, &WindowScope::Matched).is_none());
        assert!(
            window_scope_refusal(800, 11, &WindowScope::AxUnresolved { ax_window_count: 2 })
                .is_none(),
            "a live same-pid window degrades; it does not error"
        );
    }

    /// The reported failure signature: a wrong-surface walk returns a healthy
    /// non-zero element count, so the pre-fix `element_count == 0` rung stayed
    /// silent. An unresolved scope now degrades on its own evidence.
    #[test]
    fn unresolved_scope_degrades_with_its_own_reason() {
        assert_eq!(
            degradation_for(
                true,
                0,
                Some(&WindowScope::AxUnresolved { ax_window_count: 3 })
            ),
            Degradation::AxWindowUnresolved { ax_window_count: 3 }
        );
    }

    #[test]
    fn empty_tree_still_degrades_as_ax_tree_empty() {
        // Back-compat with the pre-existing rung.
        assert_eq!(
            degradation_for(true, 0, Some(&WindowScope::Matched)),
            Degradation::AxTreeEmpty
        );
    }

    #[test]
    fn resolved_window_with_elements_is_not_degraded() {
        assert_eq!(
            degradation_for(true, 42, Some(&WindowScope::Matched)),
            Degradation::None
        );
    }

    #[test]
    fn screenshot_only_path_does_not_degrade() {
        assert_eq!(degradation_for(false, 0, None), Degradation::None);
    }

    #[test]
    fn schema_advertises_the_window_scope_error_codes() {
        let description = def().description.clone();
        for code in [
            "window_id_not_found",
            "window_owner_pid_mismatch",
            "ax_window_unresolved",
        ] {
            assert!(
                description.contains(code),
                "tool description must advertise {code}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax::tree::AXNode;
    use cua_driver_core::element_query::project_elements_for_query;

    fn node(
        idx: Option<usize>,
        role: &str,
        title: Option<&str>,
        depth: usize,
        parent: Option<usize>,
        frame: Option<[f64; 4]>,
    ) -> AXNode {
        AXNode {
            element_index: idx,
            role: role.into(),
            title: title.map(|s| s.to_string()),
            value: None,
            description: None,
            identifier: None,
            help: None,
            actions: vec![],
            element_ptr: 0,
            depth,
            parent_element_index: parent,
            frame,
            value_state: None,
            value_description: None,
            min_value: None,
            max_value: None,
            enabled: None,
            selected: None,
            in_web_content: false,
        }
    }

    #[test]
    fn elements_match_indexed_node_count() {
        // Mix of indexed + non-indexed nodes; only indexed should surface.
        let nodes = vec![
            node(
                Some(0),
                "AXWindow",
                Some("Doc"),
                0,
                None,
                Some([0.0, 0.0, 800.0, 600.0]),
            ),
            node(None, "AXStaticText", Some("hint"), 1, Some(0), None),
            node(
                Some(1),
                "AXButton",
                Some("OK"),
                1,
                Some(0),
                Some([10.0, 20.0, 60.0, 24.0]),
            ),
            node(
                Some(2),
                "AXButton",
                Some("Cancel"),
                1,
                Some(0),
                Some([80.0, 20.0, 60.0, 24.0]),
            ),
        ];
        let elements = build_elements_array(&nodes);
        assert_eq!(
            elements.len(),
            3,
            "non-actionable rows must be filtered out"
        );
        let indices: Vec<u64> = elements
            .iter()
            .map(|e| e["element_index"].as_u64().unwrap())
            .collect();
        assert_eq!(
            indices,
            vec![0, 1, 2],
            "ordering must match DFS / element_index assignment"
        );
    }

    #[test]
    fn query_projection_keeps_only_rendered_actionable_rows() {
        let nodes = vec![
            node(Some(0), "AXWindow", Some("Document"), 0, None, None),
            node(Some(1), "AXMenuItem", Some("Window"), 1, Some(0), None),
            node(
                Some(2),
                "AXMenuItem",
                Some("Move & Resize"),
                2,
                Some(1),
                None,
            ),
            node(Some(3), "AXMenuItem", Some("Left"), 3, Some(2), None),
            node(Some(4), "AXButton", Some("Unrelated"), 1, Some(0), None),
        ];
        let elements = build_elements_array(&nodes);
        let filtered_markdown = concat!(
            "- [0] AXWindow \"Document\"\n",
            "  - [1] AXMenuItem \"Window\"\n",
            "    - [2] AXMenuItem \"Move & Resize\"\n",
            "      - [3] AXMenuItem \"Left\"\n",
        );

        let projected = project_elements_for_query(elements, Some("Left"), filtered_markdown);
        let indices: Vec<u64> = projected
            .iter()
            .map(|entry| entry["element_index"].as_u64().unwrap())
            .collect();

        assert_eq!(indices, vec![0, 1, 2, 3]);
    }

    #[test]
    fn query_projection_returns_no_elements_when_markdown_has_no_match() {
        let nodes = vec![node(Some(0), "AXButton", Some("Unrelated"), 0, None, None)];
        let elements = build_elements_array(&nodes);

        let projected = project_elements_for_query(elements, Some("zoomLeft"), "");

        assert!(projected.is_empty());
    }

    #[test]
    fn unfiltered_projection_preserves_every_element() {
        let nodes = vec![
            node(Some(0), "AXButton", Some("One"), 0, None, None),
            node(Some(1), "AXButton", Some("Two"), 0, None, None),
        ];
        let elements = build_elements_array(&nodes);

        let projected = project_elements_for_query(elements, None, "");

        assert_eq!(projected.len(), 2);
    }

    #[test]
    fn elements_shape_carries_role_label_frame_parent_depth() {
        let nodes = vec![node(
            Some(7),
            "AXButton",
            Some("Go"),
            3,
            Some(2),
            Some([1.5, 2.5, 33.0, 44.0]),
        )];
        let entry = &build_elements_array(&nodes)[0];
        assert_eq!(entry["element_index"], 7);
        assert_eq!(entry["role"], "AXButton");
        assert_eq!(entry["label"], "Go");
        assert_eq!(entry["depth"], 3);
        assert_eq!(entry["parent_index"], 2);
        let frame = &entry["frame"];
        assert_eq!(frame["x"], 1.5);
        assert_eq!(frame["y"], 2.5);
        assert_eq!(frame["w"], 33.0);
        assert_eq!(frame["h"], 44.0);
    }

    #[test]
    fn elements_surface_value_separately_from_label() {
        // A field with BOTH a title and a value (e.g. WhatsApp's "Compose
        // message" box holding typed text): label is the title, but the typed
        // value must ALSO be exposed so the caller can verify what landed.
        let mut nodes = vec![node(
            Some(0),
            "AXTextArea",
            Some("Compose message"),
            1,
            None,
            None,
        )];
        nodes[0].value = Some("i love u".into());
        let entry = &build_elements_array(&nodes)[0];
        assert_eq!(entry["label"], "Compose message", "label stays the title");
        assert_eq!(
            entry["value"], "i love u",
            "value must be surfaced separately"
        );
    }

    #[test]
    fn elements_surface_control_state_fields() {
        // A slider whose AXValue is a CFNumber: `value` comes from the
        // coerced value_state, alongside value_description, min/max,
        // enabled, and selected.
        let mut nodes = vec![node(
            Some(0),
            "AXSlider",
            Some("Stationary noise suppression"),
            1,
            None,
            None,
        )];
        nodes[0].value_state = Some("8".into());
        nodes[0].value_description = Some("8 dB".into());
        nodes[0].min_value = Some(2.0);
        nodes[0].max_value = Some(8.0);
        nodes[0].enabled = Some(true);
        nodes[0].selected = Some(false);
        let entry = &build_elements_array(&nodes)[0];
        assert_eq!(
            entry["value"], "8",
            "numeric AXValue surfaces via value_state"
        );
        assert_eq!(entry["value_description"], "8 dB");
        assert_eq!(entry["min"], 2.0);
        assert_eq!(entry["max"], 8.0);
        assert_eq!(entry["enabled"], true);
        assert_eq!(entry["selected"], false);
    }

    #[test]
    fn elements_surface_inherited_web_content_trust_marker() {
        let mut nodes = vec![node(
            Some(0),
            "AXButton",
            Some("Renderer button"),
            2,
            None,
            None,
        )];
        nodes[0].in_web_content = true;
        let entry = &build_elements_array(&nodes)[0];
        assert_eq!(entry["in_web_content"], true);
    }

    #[test]
    fn checkbox_value_state_normalizes_to_selected() {
        let mut nodes = vec![node(Some(0), "AXCheckBox", Some("I agree"), 0, None, None)];
        nodes[0].value_state = Some("0".into());
        let entry = &build_elements_array(&nodes)[0];
        assert_eq!(entry["selected"], false);
    }

    #[test]
    fn elements_control_state_fields_omitted_when_absent() {
        // Stock behaviour is unchanged for elements without control state.
        let nodes = vec![node(Some(0), "AXButton", Some("OK"), 0, None, None)];
        let entry = &build_elements_array(&nodes)[0];
        for key in ["value_description", "min", "max", "enabled", "selected"] {
            assert!(entry.get(key).is_none(), "{key} must be omitted");
        }
    }

    #[test]
    fn elements_omit_degenerate_min_max_range() {
        // WebKit reports AXMinValue/AXMaxValue as 0.0/0.0 on non-range
        // controls (checkboxes, radios) — a degenerate range is omitted.
        let mut nodes = vec![node(Some(0), "AXCheckBox", Some("On"), 0, None, None)];
        nodes[0].min_value = Some(0.0);
        nodes[0].max_value = Some(0.0);
        let entry = &build_elements_array(&nodes)[0];
        assert!(entry.get("min").is_none(), "degenerate min must be omitted");
        assert!(entry.get("max").is_none(), "degenerate max must be omitted");
    }

    #[test]
    fn elements_value_state_falls_back_to_string_value() {
        // String-valued elements keep their `value` even with no value_state.
        let mut nodes = vec![node(Some(0), "AXComboBox", None, 0, None, None)];
        nodes[0].value = Some("Search".into());
        let entry = &build_elements_array(&nodes)[0];
        assert_eq!(entry["value"], "Search");
    }

    #[test]
    fn elements_omit_empty_value() {
        // An empty AXValue must not emit a `value` field (matches the other
        // optional fields' omit-when-absent contract).
        let mut nodes = vec![node(Some(0), "AXButton", Some("OK"), 0, None, None)];
        nodes[0].value = Some(String::new());
        let entry = &build_elements_array(&nodes)[0];
        assert!(entry.get("value").is_none(), "empty value must be omitted");
    }

    #[test]
    fn elements_omit_optional_fields_when_missing() {
        let nodes = vec![node(Some(0), "AXUnknown", None, 0, None, None)];
        let entry = &build_elements_array(&nodes)[0];
        assert!(
            entry.get("label").is_none(),
            "label must be omitted when title/value/desc/id are all empty"
        );
        assert!(
            entry.get("frame").is_none(),
            "frame must be omitted when no rect was captured"
        );
        assert!(
            entry.get("parent_index").is_none(),
            "parent_index must be omitted at the root"
        );
        assert_eq!(entry["role"], "AXUnknown");
        assert_eq!(entry["depth"], 0);
    }

    #[test]
    fn elements_label_fallback_chain() {
        // title missing → description → value → identifier
        let nodes = vec![
            node(Some(0), "AXButton", None, 0, None, None),
            node(Some(1), "AXButton", None, 0, None, None),
            node(Some(2), "AXButton", None, 0, None, None),
        ];
        let mut nodes = nodes;
        nodes[0].description = Some("from-desc".into());
        nodes[1].value = Some("from-val".into());
        nodes[2].identifier = Some("from-id".into());
        let elements = build_elements_array(&nodes);
        assert_eq!(elements[0]["label"], "from-desc");
        assert_eq!(elements[1]["label"], "from-val");
        assert_eq!(elements[2]["label"], "from-id");
    }

    /// Every element entry carries a non-empty snapshot-bound
    /// `element_token` alongside its numeric `element_index`.
    #[test]
    fn build_elements_array_with_token_emits_element_token_per_row() {
        let reg = cua_driver_core::element_token::global();
        let pid = 0x6abc_0001_i32;
        let sid = reg.register_snapshot(pid, /* window_id = */ 9, 3);
        let nodes = vec![
            node(Some(0), "AXButton", Some("A"), 1, None, None),
            node(Some(1), "AXButton", Some("B"), 1, None, None),
            node(Some(2), "AXButton", Some("C"), 1, None, None),
        ];
        let entries = build_elements_array_with_token(&nodes, sid);
        assert_eq!(entries.len(), 3);
        // Every entry must have BOTH fields (additive contract).
        for e in &entries {
            assert!(
                e.get("element_index").is_some(),
                "element_index must remain"
            );
            let tok = e
                .get("element_token")
                .and_then(|v| v.as_str())
                .expect("element_token must be a string");
            assert!(tok.starts_with('s'), "token must use the 's' prefix: {tok}");
            assert!(tok.contains(':'), "token must be `s{{hex}}:{{idx}}`: {tok}");
        }
        // Each token must resolve through the registry to the same
        // (window_id, element_index) the integer field reports.
        for e in &entries {
            let idx = e["element_index"].as_u64().unwrap() as usize;
            let tok = e["element_token"].as_str().unwrap();
            let (wid, resolved_idx) = reg.resolve(pid, tok).expect("token must resolve");
            assert_eq!(wid, 9);
            assert_eq!(resolved_idx, idx);
        }
    }

    /// Back-compat: `build_elements_array` (the old shim) must NOT emit
    /// `element_token` — older callers that never plumb a snapshot id
    /// through get a clean shape.
    #[test]
    fn build_elements_array_shim_skips_element_token() {
        let nodes = vec![node(Some(0), "AXButton", Some("A"), 1, None, None)];
        let entries = build_elements_array(&nodes);
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].get("element_token").is_none(),
            "back-compat shim must NOT emit element_token; got: {}",
            entries[0]
        );
    }

    #[test]
    fn walk_tree_bounded_signature_accepts_caps_no_panic() {
        // Regression guard for #22865: the bounded variant must accept
        // arbitrary cap values without panicking, even against a pid that
        // has no AX tree to walk. Returns a TreeWalkResult either way.
        // Use pid that won't be a real process. Don't assume tree is empty
        // (CI may have process re-use) — only assert that the call returns
        // and the result struct shape is intact.
        let r1 = crate::ax::tree::walk_tree_bounded(i32::MAX, None, None, 5, 2);
        // Cap of 5 is the contract test from the task: when this many
        // visible nodes existed, the walker must stop early. The dead pid
        // exercises the early-return path; the assertion is that the call
        // honors the cap without overflowing or panicking.
        assert!(r1.nodes.len() <= 5, "max_elements=5 must cap nodes ≤ 5");
        assert!(
            r1.nodes.iter().all(|n| n.depth <= 2),
            "max_depth=2 must cap depth ≤ 2"
        );
        // And the uncapped variant — same dead-pid path, just validating
        // walk_tree(...) (which delegates to walk_tree_bounded with
        // DEFAULT_MAX_*) returns the same empty/safe shape.
        let r2 = crate::ax::tree::walk_tree(i32::MAX, None, None);
        assert_eq!(
            r1.nodes.len(),
            r2.nodes.len(),
            "no-pid case: both bounded and unbounded must agree on the empty result"
        );
    }
}
