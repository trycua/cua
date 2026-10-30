use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{bitmap_to_screen, is_chromium_browser_target, ToolState};

#[cfg(test)]
mod get_window_state_actions_tests {
    use super::*;
    use crate::uia::UiaNode;

    fn node(actions: Vec<String>) -> UiaNode {
        UiaNode {
            element_index: Some(1),
            control_type: "Button".to_owned(),
            name: Some("OK".to_owned()),
            value: None,
            automation_id: None,
            help_text: None,
            actions,
            enabled: Some(true),
            selected: None,
            element_ptr: 0,
            center_x: 0,
            center_y: 0,
            rect: None,
            msaa_role: None,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }

    #[test]
    fn element_entry_includes_actions_when_present() {
        let n = node(vec!["invoke".to_owned(), "toggle".to_owned()]);
        let entry = build_element_entry(&n, None).unwrap();
        assert_eq!(entry["actions"], json!(["invoke", "toggle"]));
    }

    #[test]
    fn element_entry_omits_actions_when_empty() {
        let n = node(Vec::new());
        let entry = build_element_entry(&n, None).unwrap();
        assert!(entry.get("actions").is_none());
    }
}

// ── get_window_state ─────────────────────────────────────────────────────────

/// Build a single structured element entry for `get_window_state`.
/// Returns `None` when the node has no `element_index` (non-actionable rows).
fn build_element_entry(
    n: &crate::uia::UiaNode,
    snapshot_id: Option<u32>,
) -> Option<serde_json::Value> {
    let idx = n.element_index?;
    // `label`: name → value → automation_id → help_text.
    let label = n
        .name
        .clone()
        .or_else(|| n.value.clone())
        .or_else(|| n.automation_id.clone())
        .or_else(|| n.help_text.clone());
    let mut entry = json!({
        "element_index": idx,
        "role": n.control_type,
        "depth": n.depth,
    });
    if let Some(snapshot_id) = snapshot_id {
        entry["element_token"] = json!(cua_driver_core::element_token::token_for(snapshot_id, idx));
    }
    if n.in_web_content {
        entry["in_web_content"] = json!(true);
    }
    if let Some(label) = label {
        entry["label"] = json!(label);
    }
    // Surface the element's value separately from `label` (which collapses
    // name→value→automation_id→help): a control with both a name AND text
    // (a ValuePattern edit holding typed content) would otherwise hide the
    // text from a caller reading the structured side. See the macOS
    // get_window_state builder for the rationale.
    if let Some(value) = n.value.clone().filter(|v| !v.is_empty()) {
        entry["value"] = json!(value);
    }
    if let Some(enabled) = n.enabled {
        entry["enabled"] = json!(enabled);
    }
    if let Some(selected) = n.selected {
        entry["selected"] = json!(selected);
    }
    if !n.actions.is_empty() {
        entry["actions"] = json!(n.actions);
    }
    if let Some(parent) = n.parent_element_index {
        entry["parent_index"] = json!(parent);
    }
    if let Some((l, t, r, b)) = n.rect {
        entry["frame"] = json!({
            "x": l,
            "y": t,
            "w": (r - l).max(0),
            "h": (b - t).max(0),
        });
    }
    Some(entry)
}

pub struct GetWindowStateTool {
    pub(crate) state: Arc<ToolState>,
}

static GWS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

/// Slack past `timeout_ms` before the walk task is abandoned.
const UIA_WALK_BACKSTOP_GRACE: std::time::Duration = std::time::Duration::from_millis(500);
/// Deadline for the window capture, separate from the walk budget.
const UIA_SCREENSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

#[async_trait]
impl Tool for GetWindowStateTool {
    fn def(&self) -> &ToolDef {
        GWS_DEF.get_or_init(|| ToolDef {
            name: "get_window_state".into(),
            // Description ported from Swift `GetWindowStateTool.swift` with
            // Windows-specific adaptations (UIA instead of AX; no SkyLight
            // Space validation; no per-call `javascript` field — that's a
            // macOS AppleScript hook).
            description: "Walk a running app's UIA tree and return it ONCE, as compact \
                Markdown by default (`tree_format:\"markdown\"`). Every actionable element is \
                tagged `[N]` (an element_index); its element_token is `<snapshot_id>:N`, where \
                `snapshot_id` is printed in the response header and returned in structuredContent. \
                Pass `tree_format:\"elements\"` for the structured `elements` array instead \
                (explicit `element_token`, `frame`, `parent_index`, `value`, `actions` per row), \
                or `\"both\"` for both (about twice the size).\n\n\
                Size: a read is bounded by default (`max_elements` 250). When the walk stops at \
                that budget the response says `Tree truncated at max_elements=…` and `truncated: \
                true`; pass a larger `max_elements`, or `query` / `max_depth`, to reach the rest. \
                `_note` is omitted unless `verbose:true`. `full_output:true` restores the \
                previous full response: both representations, all metadata, and the platform's \
                walk limits (≤5 000 elements, depth ≤25).\n\n\
                DIFF READS: pass `since: <snapshot_id>` from an earlier read of the same window \
                to get only what changed: `+` added rows, `~` changed rows, `-` removed rows \
                (their ids are the old snapshot's), or `no change since …`. Rows not listed keep \
                their previous `[N]` unless a `reindexed:` line says otherwise. The response \
                carries a NEW snapshot_id: use it in element_tokens. An unknown, expired, \
                other-window or differently-scoped (query/max_elements/max_depth) `since` falls \
                back to a full read and `since_status` says why. The focused element is not \
                reported on Windows yet.\n\n\
                INVARIANT: call `get_window_state` once per turn per (pid, window_id) before \
                any element action against that window. The next snapshot of the same \
                (pid, window_id) replaces this one, stales its element tokens, and lists the \
                replaced ids in `invalidated_snapshot_ids`. A screenshot-only read \
                (`include_accessibility_tree:false`) keeps the current snapshot's rows and \
                tokens valid, and `display_only:true` also leaves its pixel frame alone.\n\n\
                The UIA tree walked is the window's tree (HWND-scoped); the screenshot and \
                window bounds reported come from the same `window_id`. This is the source of \
                truth for which window the caller intends to reason about — the driver never \
                picks a window implicitly.\n\n\
                `window_id` MUST belong to `pid`; the call returns `isError: true` otherwise. \
                The driver does not auto-fall-back to a different window.\n\n\
                Set `query` to a case-insensitive substring to project the tree to matching \
                rows plus their ancestor chain. \
                Original element indices are preserved. `total_element_count` reports the \
                unprojected walked snapshot; `returned_element_count` reports the projection. \
                `elements_complete` describes the underlying walk and is unaffected by query \
                projection.\n\n\
                Always returns BOTH the element tree AND a screenshot — ground on both \
                and cross-check (the tree lies on some surfaces). Choose the modality at \
                ACTION time: an element ax action (element_token → \
                accessibility rung) or an element px action (x,y → pixel rung off this \
                screenshot). capture_mode is deprecated and ignored.\n\n\
                The mirror image: pass `include_accessibility_tree:false` to SKIP the \
                UIA walk entirely and return just the screenshot plus window metadata \
                (window_bounds, app_name, window_title) — the capture-only path for a \
                live window preview / picture-in-picture. Setting BOTH \
                `include_accessibility_tree:false` and `include_screenshot:false` is an \
                error. Optional `max_image_dimension` overrides the configured screenshot \
                long-edge limit for this call; pass 0 for native resolution. Legacy \
                `max_dimension` remains a cap on the configured limit.\n\n\
                Uses `IUIAutomationCacheRequest` to batch-fetch all element properties in a \
                single COM call (Chrome's ~5000-element tree returns in ~2-3s instead of \
                timing out at 4s with per-property RPCs).\n\n\
                Optional `max_elements` / `max_depth` bound the UIA walk (Electron / large web \
                apps produce 10k+ element trees). Markdown and structured elements are \
                truncated identically. `elements_complete` is false when a bound skips a node \
                or subtree; ending exactly at a bound remains complete. Default: \
                `max_elements` 250, depth ≤25.\n\n\
                CHROMIUM COVERAGE: a browser-owned permission bubble can be \
                composited outside the requested native window. Chromium-family \
                snapshots therefore describe this limit in structuredContent.capture_coverage. \
                After a verified ineffective window action, call escalate_session, take a \
                fresh get_desktop_state snapshot, act explicitly in desktop scope if needed, \
                then verify with another fresh desktop snapshot. \
                This is separate from page JavaScript dialogs, which remain on \
                browser_dialog.\n\n\
                Windows requires no special permissions.".into(),
            input_schema: cua_driver_core::window_state_view::extend_input_schema(json!({"type":"object","required":["pid","window_id"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "pid":{"type":"integer","description":"Process ID from `list_apps`."},
                "window_id":{"type":"integer","description":"HWND of the target window. Must belong to `pid`. Enumerate via `list_windows` or read from `launch_app`'s `windows` array."},
                "capture_mode": cua_driver_core::capture_mode::capture_mode_schema(),
                "include_accessibility_tree":{"type":"boolean","description":"Default true — walk the UIA tree and return it (per `tree_format`) alongside the screenshot. Set false to SKIP the UIA walk entirely and return just the screenshot plus window metadata (window_bounds, app_name, window_title) — the capture-only path for a live window preview / picture-in-picture. Mirrors include_screenshot. Setting BOTH include_accessibility_tree:false AND include_screenshot:false is an error (nothing to return)."},
                "include_screenshot":{"type":"boolean","description":"Default true — returns a grounding screenshot alongside the tree. Set false to skip the grab and return tree only (the cheap path for re-indexing before an element ax action)."},
                "screenshot_out_file":{"type":"string","description":"When set, write the PNG to this file path instead of embedding base64 in the response. The structured output will contain `screenshot_file_path` instead."},
                "query":{"type":"string","description":"Optional case-insensitive substring. Projects both tree_markdown and structured elements to matches plus ancestors while preserving original indices. Compare total_element_count with returned_element_count; elements_complete continues to describe the underlying unprojected walk."},
                "max_elements":{"type":"integer","minimum":1,"description":"Cap on the total number of UIA nodes walked. Truncates depth-first; markdown and structured elements truncate together. Default 250 (5 000 with full_output:true); the response states when the tree was cut. Raise it to read further into a large window."},
                "max_depth":{"type":"integer","minimum":1,"description":"Cap on the UIA-tree walk depth. Nodes whose rendered indent would exceed this are omitted. Omit for the default (25). Lower for deep menu / Electron trees."},
                "timeout_ms": cua_driver_core::tool_schema::timeout_ms_schema(),
                "max_image_dimension":{"type":"integer","minimum":0,"description":"Per-call long-edge limit for the returned screenshot, in pixels (aspect ratio preserved). Explicit values override configured behavior; 0 returns native resolution. Omit to preserve the configured default."},
                "max_dimension":{"type":"integer","minimum":1,"description":"Legacy optional cap on the returned screenshot's long edge, in pixels (aspect ratio preserved). Applied on top of the configured max_image_dimension ceiling when max_image_dimension is omitted; the tighter wins."}
            },"additionalProperties":false})),
            // Swift annotation: idempotent: false (each call is a fresh snapshot).
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let display_only = match cua_driver_core::window_state_view::display_only(&args) {
            Ok(display_only) => display_only,
            Err(refusal) => return refusal,
        };
        // Swift error wording 1:1.
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let hwnd =
            match args.get("window_id").and_then(|v| v.as_u64()) {
                Some(v) => v,
                None => return ToolResult::error(
                    "Missing required integer field window_id. Use `list_windows` to enumerate \
                 the target app's windows, or read `launch_app`'s `windows` array.",
                ),
            };
        // Validate window belongs to pid — Swift's hard error. The exact HWND is
        // probed through Win32 first; the desktop-wide UIA union (2 s deadline)
        // is consulted only on a miss (#4416).
        use crate::win32::PidWindowLookup;
        let lookup =
            tokio::task::spawn_blocking(move || crate::win32::lookup_window_for_pid(pid, hwnd))
                .await
                .unwrap_or(PidWindowLookup::Missing);
        let window = match lookup {
            PidWindowLookup::Found(window) => window,
            PidWindowLookup::OtherPid(owner) => {
                return ToolResult::error(format!(
                    "window_id {hwnd} belongs to pid {owner}, not pid {pid}. Call \
                     `list_windows({{\"pid\": {pid}}})` to get this pid's own windows."
                ));
            }
            PidWindowLookup::Missing => {
                return ToolResult::error(format!(
                    "No window with window_id {hwnd} exists. Call `list_windows({{\"pid\": \
                     {pid}}})` for candidates."
                ));
            }
        };
        // Window identity metadata (additive): title + on-screen rectangle from
        // the lookup we already did, plus the owning process's executable
        // name. Names the surface on the capture-only path, where no UIA tree
        // identifies it.
        let win_geom = Some((
            window.title.clone(),
            window.x,
            window.y,
            window.width,
            window.height,
        ));
        let app_name = tokio::task::spawn_blocking(move || {
            crate::win32::list_processes()
                .into_iter()
                .find(|p| p.pid == pid)
                .map(|p| p.name)
        })
        .await
        .ok()
        .flatten();
        use cua_driver_core::tool_args::ArgsExt;
        // The canonical per-call value overrides all configured limits,
        // including `0` for native resolution. The legacy field retains its
        // previous cap-folding behavior when the canonical field is omitted.
        let max_image_dimension = match args.get("max_image_dimension") {
            Some(value) => match value.as_u64().and_then(|v| u32::try_from(v).ok()) {
                Some(value) => Some(value),
                None => {
                    return ToolResult::error(
                        "max_image_dimension must be an integer between 0 and 4294967295.",
                    )
                }
            },
            None => None,
        };
        let max_dimension = args
            .get("max_dimension")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .map(|v| v.max(1));
        let max_dim = {
            let cfg = self.state.config.read().unwrap();
            cua_driver_core::image_utils::ImageDimensionLimits {
                configured: cfg.max_image_dimension,
                legacy_max_dimension: max_dimension,
                max_image_dimension,
            }
            .resolve()
        };
        // `capture_mode` is DEPRECATED and ignored — get_window_state always
        // returns BOTH the UIA tree and a screenshot now, so the agent grounds on
        // both and cross-checks (the UIA tree lies often enough that a grounding
        // screenshot should always be present). The modality is chosen at action
        // time: an element ax action (element_index) or an element px action (x,y).
        // We don't read the arg; it stays in the schema only so old callers don't
        // trip additionalProperties:false.
        let view = match cua_driver_core::window_state_view::ViewOptions::from_args(&args) {
            Ok(view) => view,
            Err(refusal) => return refusal,
        };
        let query = args.opt_str("query");
        let session_id = args.opt_str("_session_id");
        let screenshot_out_file = args.opt_str("screenshot_out_file");
        // `display_only` is the public form of the internal observation-only
        // read: pixels for a preview, with no snapshot or capture change.
        let observation_only = display_only
            || args
                .get("_observation_only")
                .and_then(|value| value.as_bool())
                == Some(true);
        // Optional caps — when omitted, fall back to the walker's built-in
        // defaults (#22865). minimum:1 enforced in the schema, but defend
        // against 0 here too.
        let max_elements = view.max_elements(
            &args,
            crate::uia::DEFAULT_MAX_TOTAL_ELEMENTS,
            observation_only,
        );
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(crate::uia::DEFAULT_MAX_DEPTH);
        let timeout_ms = cua_driver_core::tool_schema::resolve_timeout_ms(args.get("timeout_ms"));

        // Always walk the UIA tree. The screenshot is returned by DEFAULT — the
        // grounding frame the agent cross-checks the (sometimes-lying) tree
        // against — but `include_screenshot:false` opts out of the grab entirely
        // (tree only: the cheap path for re-indexing before an element ax action).
        // `screenshot_out_file` still forces a capture to disk regardless. With
        // `screenshot_out_file` the bytes go to disk and the path is surfaced
        // instead of embedding base64; otherwise the base64 PNG is embedded.
        let include_screenshot = args.get("include_screenshot").and_then(|v| v.as_bool());
        // `include_accessibility_tree` (default true) mirrors include_screenshot:
        // set false to SKIP the UIA walk and return just the screenshot + window
        // metadata (the capture-only / preview path).
        let do_tree = args
            .get("include_accessibility_tree")
            .and_then(|v| v.as_bool())
            != Some(false);
        let do_shot = include_screenshot != Some(false) || screenshot_out_file.is_some();
        if !do_tree && !do_shot {
            return ToolResult::error(
                "Nothing to return: both include_accessibility_tree:false and \
                 include_screenshot:false. Set at least one to true, or pass \
                 screenshot_out_file to force a capture.",
            );
        }

        let state = self.state.clone();
        let q = query.clone();
        let out_file = screenshot_out_file.clone();
        // The tree walk runs under `timeout_ms`. UIA's bulk BuildUpdatedCache
        // cannot be interrupted (Chrome's provider can block indefinitely on
        // property reads), so when the walk task itself outlives the budget
        // the response carries an empty tree flagged `truncated` instead of an
        // error, and the screenshot below still comes back.
        let tree_result = if do_tree {
            let tree_task = tokio::task::spawn_blocking(move || {
                let mut budget =
                    cua_driver_core::walk_budget::WalkBudget::new(timeout_ms, max_elements);
                let iconic_before = crate::input::window_is_iconic(hwnd);
                let tree =
                    crate::uia::walk_tree_budgeted(hwnd, q.as_deref(), max_depth, &mut budget);
                let kind = if tree.nodes.iter().any(|node| node.msaa_role.is_some()) {
                    crate::uia::snapshot::SnapshotKind::Msaa
                } else {
                    crate::uia::snapshot::SnapshotKind::Uia
                };
                let payload = crate::uia::snapshot::UiaSnapshot::from_nodes(&tree.nodes, kind)
                    .with_read_while_minimized(
                        iconic_before || crate::input::window_is_iconic(hwnd),
                    );
                (tree, payload, budget.outcome())
            });
            let started = std::time::Instant::now();
            let deadline = std::time::Duration::from_millis(timeout_ms) + UIA_WALK_BACKSTOP_GRACE;
            match tokio::time::timeout(deadline, tree_task).await {
                Ok(Ok(tree)) => Some(tree),
                Ok(Err(e)) => return ToolResult::error(format!("UIA walk task panicked: {e}")),
                Err(_elapsed) => {
                    let class = crate::input::delivery::read_class_name(hwnd);
                    let walk = cua_driver_core::walk_budget::WalkOutcome::timed_out(
                        timeout_ms,
                        started.elapsed(),
                    );
                    let tree = crate::uia::UiaTreeResult {
                        tree_markdown: format!(
                            "(The UIA provider for hwnd 0x{hwnd:x}, class '{class}', did not \
                             answer. Act by pixel off the screenshot, or send press_key with \
                             delivery_mode:\"foreground\" for a dialog's default accelerator.)"
                        ),
                        nodes: Vec::new(),
                        elements_complete: false,
                    };
                    let payload = crate::uia::snapshot::UiaSnapshot::from_nodes(
                        &[],
                        crate::uia::snapshot::SnapshotKind::Uia,
                    );
                    Some((tree, payload, walk))
                }
            }
        } else {
            None
        };
        let blocking = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            // Capture screenshot AND any error message so the response can
            // surface *why* there's no image (the iconic-window guard from
            // #1973 / PR #1974 is the load-bearing case: minimized windows
            // legitimately can't be captured, and the caller needs to know
            // to call `bring_to_front` instead of retrying).
            // The previous `Err(_) => None` silently dropped the error and
            // upstream agents saw an empty response with no signal.
            let (screenshot, screenshot_err) = if do_shot {
                match crate::capture::screenshot_window_bytes(hwnd) {
                    Ok(raw) => {
                        let (native_w, native_h) = crate::capture::png_dimensions_pub(&raw)?;
                        let png = crate::capture::resize_png_if_needed(&raw, max_dim)?;
                        let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                        // `screenshot_out_file` set (any mode) → write to disk and
                        // surface the path, never embed bytes. Otherwise (vision,
                        // no out_file) → embed base64.
                        if let Some(ref path) = out_file {
                            std::fs::write(path, &png)?;
                            (
                                Some((None, Some(path.clone()), w, h, native_w, native_h, png)),
                                None,
                            )
                        } else {
                            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                            (
                                Some((Some(B64.encode(&png)), None, w, h, native_w, native_h, png)),
                                None,
                            )
                        }
                    }
                    Err(e) => (None, Some(format!("{e}"))),
                }
            } else {
                (None, None)
            };
            Ok((screenshot, screenshot_err))
        });
        let result: anyhow::Result<_> =
            match tokio::time::timeout(UIA_SCREENSHOT_TIMEOUT, blocking).await {
                Ok(join_result) => join_result
                    .map_err(|e| anyhow::anyhow!("task panic: {e}"))
                    .and_then(|r| r),
                Err(_elapsed) => Ok((
                    None,
                    Some(format!(
                        "window capture did not finish within {}s",
                        UIA_SCREENSHOT_TIMEOUT.as_secs()
                    )),
                )),
            };
        let result =
            result.map(|(screenshot, screenshot_err)| (tree_result, screenshot, screenshot_err));

        match result {
            Ok((tree_opt, screenshot_opt, screenshot_err)) => {
                let mut content = Vec::new();
                let mut structured = json!({ "window_id": hwnd, "pid": pid });
                let screenshot_scale = screenshot_opt
                    .as_ref()
                    .map(|(_, _, w, _, native_w, _, _)| *native_w as f64 / *w as f64);
                let mut published_snapshot = false;
                let mut invalidated = Vec::new();

                if let Some((tr, payload, walk)) = tree_opt {
                    let is_msaa = tr.nodes.iter().any(|node| node.msaa_role.is_some());
                    let count = tr
                        .nodes
                        .iter()
                        .filter(|n| n.element_index.is_some())
                        .count();
                    let mut header = format!("window_id={hwnd} pid={pid} elements={count}\n");
                    if let Some(note) = walk.note() {
                        header.push_str(&note);
                        header.push('\n');
                    }
                    header.push('\n');
                    content.push(cua_driver_core::protocol::Content::text(
                        header + &tr.tree_markdown,
                    ));
                    structured["element_count"] = json!(count);
                    // Verification consumes only structured rows, which still
                    // omit non-actionable UIA nodes on Windows. A complete
                    // native walk cannot prove absence in that projection.
                    structured["elements_complete"] =
                        json!(tr.elements_complete && !observation_only);
                    structured["tree_markdown"] = json!(tr.tree_markdown);
                    walk.apply(&mut structured);

                    let (snapshot_id, replaced) = (!observation_only)
                        .then(|| {
                            state.snapshots.publish_for_session(
                                pid as i32,
                                hwnd,
                                payload,
                                session_id.as_deref(),
                                screenshot_scale,
                            )
                        })
                        .flatten()
                        .unzip();
                    published_snapshot = snapshot_id.is_some();
                    invalidated.extend(replaced.into_iter().flatten());

                    // Structured `elements` array — preferred consumption
                    // path. Shape matches the cross-platform spec:
                    // `{element_index, element_token, role, label, depth,
                    // actions?, parent_index?, frame?: {x,y,w,h}}`. Frame is
                    // included when UIA reported a usable BoundingRectangle.
                    let elements: Vec<serde_json::Value> = tr
                        .nodes
                        .iter()
                        .filter_map(|n| build_element_entry(n, snapshot_id))
                        .collect();
                    let elements = cua_driver_core::element_query::project_elements_for_query(
                        elements,
                        query.as_deref(),
                        &tr.tree_markdown,
                    );
                    // Screenshot pixels: the capture's top-left is the DWM
                    // bitmap origin (the inverse of the pixel tools'
                    // `bitmap_to_screen`), downsized by delivered/native.
                    let elements = match screenshot_opt.as_ref() {
                        Some((_, _, w, _, native_w, _, _)) if *native_w > 0 => {
                            let (ox, oy) = bitmap_to_screen(hwnd, 0, 0);
                            cua_driver_core::element_frame::with_screenshot_frames(
                                elements,
                                (f64::from(ox), f64::from(oy)),
                                f64::from(*w) / f64::from(*native_w),
                            )
                        }
                        _ => elements,
                    };
                    structured["total_element_count"] = json!(count);
                    structured["returned_element_count"] = json!(elements.len());
                    structured["elements"] = json!(elements);
                    // Surface 6: snapshot id mirror for debug correlation.
                    if let Some(snapshot_id) = snapshot_id {
                        structured["snapshot_id"] = json!(
                            cua_driver_core::element_token::format_snapshot_id(snapshot_id)
                        );
                    }
                    structured["_note"] = json!(
                        "Prefer `elements` — `tree_markdown` will continue to work \
                         but new fields will only be added to the structured side. \
                         Issue #22865: use `max_elements` / `max_depth` to bound the \
                         UIA walk on apps with very large trees."
                    );
                    // Best-effort-background ladder: a UIA walk that ran but found
                    // zero actionable elements is NOT a clean snapshot — the window
                    // may be a non-UIA surface (canvas/WebGL/custom-drawn) or its
                    // tree wasn't ready (Chromium/Electron need an enable + settle).
                    // Mark it degraded so callers don't read `elements: []` as "this
                    // window has no controls", and point at the next rung. An empty
                    // UIA tree means element_index has nothing to bind to, so the
                    // deliberate move is an element px action — read the screenshot
                    // already in this response and click by pixel (x,y).
                    if is_msaa {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!(
                            "msaa_fallback_partial: the UIA provider was unavailable and \
                             Cua Driver used a partial MSAA tree. Treat it as discovery \
                             evidence only; it cannot prove checked state."
                        );
                    } else if let (0, Some(note)) = (count, walk.note()) {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!(note);
                    } else if count == 0 {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!(
                            "ax_tree_empty: the UIA walk returned no actionable elements. \
                             The window may be a non-UIA surface (canvas/WebGL/custom-drawn) \
                             or its accessibility tree was not ready (Chromium/Electron \
                             require a UIA-enable + settle). Do not treat element data as \
                             authoritative — re-snapshot if the app just launched, otherwise \
                             switch to the visual path."
                        );
                        structured["escalation"] = json!({
                            "recommended": "px",
                            "reason": "non-AX surface — act by pixel (x,y) off the \
                                       screenshot in this response (an element px action)."
                        });
                    }
                }

                // A screenshot-only read adds an image but no element rows: it
                // refreshes the current snapshot's pixel frame in place and
                // keeps its tokens valid. Only a window with no element
                // snapshot yet gets a new, empty one.
                let mut refreshed = None;
                if let (false, false, Some(scale)) =
                    (observation_only, published_snapshot, screenshot_scale)
                {
                    refreshed = state.snapshots.refresh_screenshot_for_session(
                        pid as i32,
                        hwnd,
                        session_id.as_deref(),
                        scale,
                        None,
                    );
                    if refreshed.is_none() {
                        let payload = crate::uia::snapshot::UiaSnapshot::from_nodes(
                            &[],
                            crate::uia::snapshot::SnapshotKind::Uia,
                        );
                        if let Some((_, replaced)) = state.snapshots.publish_for_session(
                            pid as i32,
                            hwnd,
                            payload,
                            session_id.as_deref(),
                            screenshot_scale,
                        ) {
                            invalidated.extend(replaced);
                        }
                    }
                }
                if let Some(id) = refreshed {
                    let sid = cua_driver_core::element_token::format_snapshot_id(id);
                    content.push(cua_driver_core::protocol::Content::text(format!(
                        "Screenshot only: snapshot {sid} keeps its rows, and its element_tokens \
                         ({sid}:N) stay valid."
                    )));
                    structured["snapshot_id"] = json!(sid);
                    structured["screenshot_refreshed_snapshot"] = json!(true);
                }
                if !invalidated.is_empty() {
                    let ids: Vec<String> = invalidated
                        .into_iter()
                        .map(cua_driver_core::element_token::format_snapshot_id)
                        .collect();
                    content.push(cua_driver_core::protocol::Content::text(format!(
                        "Invalidated snapshots {}: their element_tokens are stale.",
                        ids.join(", ")
                    )));
                    structured["invalidated_snapshot_ids"] = json!(ids);
                }

                if let Some((b64_opt, file_path, w, h, native_w, native_h, png)) = screenshot_opt {
                    let capture_id = if !observation_only {
                        if let Some(bridge) = &state.capture_bridge {
                            let geometry = match crate::capture_admission::CaptureGeometry::new(
                                w, h, native_w, native_h,
                            ) {
                                Ok(geometry) => geometry,
                                Err(error) => {
                                    return ToolResult::error(format!(
                                        "Window capture geometry is invalid: {error}"
                                    ))
                                }
                            };
                            match bridge.publish(
                                &args,
                                png,
                                crate::capture_admission::WindowsCaptureTarget::Window {
                                    pid,
                                    window_id: hwnd,
                                },
                                geometry,
                            ) {
                                Ok(capture_id) => capture_id,
                                Err(error) => {
                                    return ToolResult::error(format!(
                                        "Window capture publication failed: {error}"
                                    ))
                                }
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    // base64 is embedded only when no out_file was given (vision
                    // path). With `screenshot_out_file` the bytes went to disk and
                    // we surface the path instead — never both. Keep a text content
                    // part when the image went to disk so the response is never
                    // empty on the capture-only path (which has no tree markdown).
                    if let Some(b64) = b64_opt {
                        content.push(cua_driver_core::protocol::Content::image_png(b64));
                    } else if let Some(fp) = &file_path {
                        content.push(cua_driver_core::protocol::Content::text(format!(
                            "window_id={hwnd} pid={pid} size={w}x{h} screenshot written to {fp}"
                        )));
                    }
                    structured["screenshot_width"] = json!(w);
                    structured["screenshot_height"] = json!(h);
                    // Surface 7: mirror the MCP image part's `mimeType` onto
                    // the structured payload so consumers don't have to sniff
                    // magic bytes off the base64 to know the format.
                    structured["screenshot_mime_type"] = json!("image/png");
                    if let Some(capture_id) = capture_id {
                        structured["capture_id"] = json!(capture_id);
                    }
                    if let Some(fp) = file_path {
                        structured["screenshot_file_path"] = json!(fp);
                    }
                } else if let Some(err) = screenshot_err {
                    // Capture failed (most commonly because the target is
                    // minimized — see #1973). Surface the reason in BOTH the
                    // human-readable content stream (so the model sees it
                    // alongside the UIA tree it does get) AND structuredContent
                    // (so MCP clients with structured-only parsing can detect
                    // and act on it). Without this the caller saw an empty
                    // response with no clue why and burned turns retrying.
                    content.push(cua_driver_core::protocol::Content::text(format!(
                        "screenshot unavailable: {err}"
                    )));
                    structured["screenshot_error"] = json!(err);
                }

                // Window identity metadata (additive): title + on-screen
                // rectangle + owning process name for the requested window_id,
                // useful on the capture-only path where no UIA tree names it.
                if let Some((title, x, y, w, h)) = &win_geom {
                    if !title.is_empty() {
                        structured["window_title"] = json!(title);
                    }
                    structured["window_bounds"] =
                        json!({ "x": x, "y": y, "width": w, "height": h });
                }
                if let Some(name) = app_name.as_deref().filter(|n| !n.is_empty()) {
                    structured["app_name"] = json!(name);
                }

                cua_driver_core::window_inspection::mark_browser_chrome_capture_coverage(
                    &mut structured,
                    is_standalone_chromium_browser_process(pid).then_some(
                        cua_driver_core::window_inspection::BrowserChromeCaptureCoverage::NotObservable,
                    ),
                );

                // The capture-only path (include_accessibility_tree:false) leaves
                // `content` empty if the screenshot was also unavailable. Return a
                // structured error rather than a "successful" response with no
                // content parts (consistent with the tree+screenshot path, which
                // always carries at least the tree markdown).
                if content.is_empty() {
                    return ToolResult::error(format!(
                        "No content produced for window_id {hwnd}: the accessibility tree was \
                         skipped (include_accessibility_tree:false) and no screenshot was returned."
                    ))
                    .with_structured(structured);
                }

                if display_only {
                    cua_driver_core::window_state_view::mark_display_only(&mut structured);
                }
                if !observation_only {
                    cua_driver_core::window_state_view::apply(
                        &view,
                        &cua_driver_core::window_state_view::ViewContext {
                            pid: i64::from(pid),
                            window_id: hwnd,
                            key: cua_driver_core::window_state_view::ViewKey {
                                query: query.clone(),
                                max_elements,
                                max_depth: args
                                    .get("max_depth")
                                    .and_then(|v| v.as_u64())
                                    .map(|v| v as usize),
                            },
                            focus_probe: None,
                        },
                        &mut content,
                        &mut structured,
                    );
                }

                ToolResult {
                    content,
                    is_error: None,
                    structured_content: Some(structured),
                    action_record: None,
                }
            }
            Err(e) => ToolResult::error(format!("Error: {e}")),
        }
    }
}

fn is_standalone_chromium_browser_process(pid: u32) -> bool {
    crate::win32::list_processes()
        .into_iter()
        .find(|process| process.pid == pid)
        .is_some_and(|process| is_chromium_browser_target(&process.name))
}

#[cfg(test)]
mod display_only_tests {
    use super::{GetWindowStateTool, ToolState};
    use cua_driver_core::tool::Tool;

    // T3 Code feature-detects the preview mode from this schema property.
    #[test]
    fn display_only_is_advertised() {
        let tool = GetWindowStateTool {
            state: ToolState::new(None),
        };
        assert_eq!(
            tool.def().input_schema["properties"]["display_only"]["type"],
            "boolean"
        );
    }

    #[tokio::test]
    async fn display_only_refuses_accessibility_snapshot_requests() {
        let tool = GetWindowStateTool {
            state: ToolState::new(None),
        };
        for tree in [None, Some(true)] {
            let mut args = serde_json::json!({"pid": 42, "window_id": 7, "display_only": true});
            if let Some(tree) = tree {
                args["include_accessibility_tree"] = serde_json::json!(tree);
            }
            let result = tool.invoke(args).await;
            assert_eq!(
                serde_json::to_value(result).unwrap(),
                serde_json::to_value(cua_driver_core::protocol::ToolResult::error(
                    "display_only requires include_accessibility_tree:false"
                ))
                .unwrap()
            );
        }
    }
}
