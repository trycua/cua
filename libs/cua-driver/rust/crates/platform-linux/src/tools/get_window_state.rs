use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    framed_elements_first, non_ax_escalation, popup_menu_elements, rects_intersect, ToolState,
    WindowOverlays,
};

// ── get_window_state ─────────────────────────────────────────────────────────

/// The largest window screenshot delivered as-is. The Anthropic API downsizes
/// any image above ~1.15 megapixels before the model sees it, so a 1568x861
/// PNG reached the model as ~1447x795 while the driver still mapped its x/y
/// as 1568x861: every pixel click landed a uniform 0.94x short. Capping here
/// keeps "pixels of THIS screenshot" true for the image the model reads.
const WINDOW_SCREENSHOT_MAX_PIXELS: u64 = 1_150_000;

/// The long edge a `w`x`h` image must shrink to so that it holds at most
/// `max_pixels` pixels; `None` when it already fits.
fn megapixel_long_edge_cap(w: u32, h: u32, max_pixels: u64) -> Option<u32> {
    let pixels = u64::from(w) * u64::from(h);
    if w == 0 || h == 0 || pixels <= max_pixels {
        return None;
    }
    let long = w.max(h) as f64;
    let short = w.min(h) as f64;
    let mut edge = (long * (max_pixels as f64 / pixels as f64).sqrt()).floor() as u32;
    // The resizer rounds both sides; step down until the rounded image fits.
    while edge > 1 {
        let scale = edge as f64 / long;
        let fits = ((long * scale).round() as u64) * ((short * scale).round() as u64) <= max_pixels;
        if fits {
            break;
        }
        edge -= 1;
    }
    Some(edge)
}

#[cfg(test)]
mod megapixel_cap_tests {
    use super::{megapixel_long_edge_cap, WINDOW_SCREENSHOT_MAX_PIXELS};

    #[test]
    fn a_window_shot_above_the_api_threshold_is_capped_below_it() {
        let edge =
            megapixel_long_edge_cap(1568, 861, WINDOW_SCREENSHOT_MAX_PIXELS).expect("capped");
        let scale = edge as f64 / 1568.0;
        let (w, h) = (
            (1568.0 * scale).round() as u64,
            (861.0 * scale).round() as u64,
        );
        assert!(w * h <= WINDOW_SCREENSHOT_MAX_PIXELS, "{w}x{h}");
        assert!((1440..1568).contains(&edge), "{edge}");
    }

    #[test]
    fn a_shot_within_the_threshold_is_left_alone() {
        assert_eq!(
            megapixel_long_edge_cap(1280, 800, WINDOW_SCREENSHOT_MAX_PIXELS),
            None
        );
        assert_eq!(
            megapixel_long_edge_cap(0, 800, WINDOW_SCREENSHOT_MAX_PIXELS),
            None
        );
    }
}

/// Build a single structured element entry for `get_window_state`.
/// Returns `None` when the node has no `element_index` (non-actionable rows).
fn build_element_entry(
    n: &crate::atspi::AtspiNode,
    snapshot_id: Option<u32>,
    bounds: Option<(i32, i32, u32, u32)>,
) -> Option<serde_json::Value> {
    let idx = n.element_index?;
    // `label` mirrors what a human reading the markdown row would call this
    // element: its name, else its description (Qt keeps a button's tooltip
    // there: "Play"/"Pause"). Never its value: four spin buttons all labelled
    // "0.0" cannot be told apart — such a control is `unlabelled` instead,
    // with its place among its siblings in `description`.
    let unlabelled = n.name.is_none()
        && n.description
            .as_deref()
            .is_some_and(|d| d.starts_with(crate::atspi::native::UNLABELLED_NOTE_PREFIX));
    let label = n
        .name
        .clone()
        .or_else(|| n.description.clone().filter(|_| !unlabelled));
    let mut entry = json!({
        "element_index": idx,
        "role": n.role,
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
    if unlabelled {
        entry["unlabelled"] = json!(true);
    }
    // Surface the element's value separately from `label` (which collapses
    // name→value→description): a field with both a name AND typed text would
    // otherwise hide the text from a caller reading the structured side,
    // leaving it only in tree_markdown. See the macOS get_window_state builder
    // for the rationale.
    if let Some(value) = n.value.clone().filter(|v| !v.is_empty()) {
        entry["value"] = json!(value);
    }
    if let Some(enabled) = n.enabled {
        entry["enabled"] = json!(enabled);
    }
    if let Some(selected) = n.selected {
        entry["selected"] = json!(selected);
    }
    let actions: Vec<String> = n
        .actions
        .iter()
        .filter(|a| !a.trim().is_empty())
        .cloned()
        .collect();
    if !actions.is_empty() {
        entry["actions"] = json!(actions);
    }
    if let Some(parent) = n.parent_element_index {
        entry["parent_index"] = json!(parent);
    }
    if let Some((x, y, w, h)) = bounds {
        entry["frame"] = json!({ "x": x, "y": y, "w": w, "h": h });
    }
    if let Some(description) = n.description.clone().filter(|d| !d.is_empty()) {
        entry["description"] = json!(description);
    }
    Some(entry)
}

fn window_overlays(pid: u32, xid: u64) -> WindowOverlays {
    let mut out = WindowOverlays {
        window_rect: crate::x11::window_info(xid).map(|w| (w.x, w.y, w.width, w.height)),
        ..Default::default()
    };
    let over = |rect: (i32, i32, u32, u32)| {
        out.window_rect
            .is_some_and(|target| rects_intersect(rect, target))
    };
    let mut covers = false;
    for window in crate::x11::list_windows(Some(pid)) {
        if window.xid == xid || !window.is_on_screen || window.width == 0 || window.height == 0 {
            continue;
        }
        let Some(owner) = crate::x11::transient_for(window.xid) else {
            continue;
        };
        covers |= over((window.x, window.y, window.width, window.height));
        out.dialogs.push(json!({
            "window_id": window.xid,
            "title": window.title,
            "transient_for": owner,
            "modal": crate::x11::window_is_modal(window.xid),
            "bounds": { "x": window.x, "y": window.y, "width": window.width, "height": window.height },
        }));
    }
    // A dialog/plugin window can legitimately run as a DIFFERENT process than
    // the application it belongs to (GIMP's separate-process export dialogs,
    // LibreOffice's Document Recovery dialog under a distinct soffice.bin).
    // Exact-pid matching above would make such a window invisible even though
    // it is unambiguously this application's own popup. WM_TRANSIENT_FOR
    // resolving to one of pid's own windows is the correlation signal (see
    // `list_cross_pid_transient_windows`); such entries are tagged with
    // `owning_pid` so callers can tell it apart from a same-process dialog.
    for window in crate::x11::list_cross_pid_transient_windows(pid) {
        if window.xid == xid || !window.is_on_screen || window.width == 0 || window.height == 0 {
            continue;
        }
        let Some(owner) = crate::x11::transient_for(window.xid) else {
            continue;
        };
        covers |= over((window.x, window.y, window.width, window.height));
        out.dialogs.push(json!({
            "window_id": window.xid,
            "title": window.title,
            "transient_for": owner,
            "modal": crate::x11::window_is_modal(window.xid),
            "bounds": { "x": window.x, "y": window.y, "width": window.width, "height": window.height },
            "owning_pid": window.pid,
        }));
    }
    for popup in crate::input::mapped_popup_windows() {
        if popup.window == xid || popup.pid.is_some_and(|owner| owner != pid) {
            continue;
        }
        // Desktop-wide override-redirect windows that are nobody's menu:
        // mutter's guard window and the driver's own cursor overlay.
        if popup.pid.is_none()
            && (popup.title.contains("guard window") || popup.title.starts_with("Cua."))
        {
            continue;
        }
        covers |= over((popup.x, popup.y, popup.width, popup.height));
        out.popups.push(popup.to_json());
    }
    out.covers_window = covers;
    out
}

pub struct GetWindowStateTool {
    pub(crate) state: Arc<ToolState>,
}

const COLD_START_WALK_TIMEOUT_MS: u64 = 2_000;

/// The AT-SPI walker's own node budget when the caller gives none.
const LINUX_DEFAULT_MAX_ELEMENTS: usize = 5_000;

fn linux_snapshot_timeout_ms(timeout: Option<&Value>, has_prior_snapshot: bool) -> u64 {
    cua_driver_core::tool_schema::resolve_timeout_ms_with_first_snapshot_grace(
        timeout,
        has_prior_snapshot,
        COLD_START_WALK_TIMEOUT_MS,
    )
}

static GWS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetWindowStateTool {
    fn def(&self) -> &ToolDef {
        GWS_DEF.get_or_init(|| ToolDef {
            name: "get_window_state".into(),
            description: "Walk a running app's AT-SPI tree and return it ONCE, as \
                compact Markdown by default (`tree_format:\"markdown\"`). Every actionable \
                element is tagged `[N]` (an element_index); its element_token is \
                `<snapshot_id>:N`, where `snapshot_id` is printed in the response header and \
                returned in structuredContent. Pass `tree_format:\"elements\"` for the \
                structured `elements` array instead (explicit `element_token`, `role`, \
                `label`, `value`, `enabled`, `selected`, `actions`, `frame: {x,y,w,h}` when \
                AT-SPI reports usable bounds, `parent_index`, `depth`), or `\"both\"` for \
                both (about twice the size). Set `query` to project the tree to matching rows \
                plus their ancestor chain while preserving original indices; \
                `total_element_count` reports the complete snapshot.\n\n\
                Size: a read is bounded by default (`max_elements` 250 actionable nodes). \
                When the walk stops at that budget the response says `Tree truncated at \
                max_elements=…` and `truncated: true`; pass a larger `max_elements`, or \
                `query` / `max_depth`, to reach the rest. `_note` is omitted unless \
                `verbose:true`. `full_output:true` restores the previous full response: both \
                representations, all metadata, and the walker's own limits (≤5 000 nodes).\n\n\
                DIFF READS: pass `since: <snapshot_id>` from an earlier read of the same window \
                to get only what changed: `+` added rows, `~` changed rows, `-` removed rows \
                (their ids are the old snapshot's), or `no change since …`. Rows not listed keep \
                their previous `[N]` unless a `reindexed:` line says otherwise. The response \
                carries a NEW snapshot_id: use it in element_tokens. An unknown, expired, \
                other-window or differently-scoped (query/max_elements/max_depth) `since` falls \
                back to a full read and `since_status` says why. The focused element is not \
                reported on Linux yet.\n\n\
                Always returns BOTH the element tree AND a screenshot — ground on \
                both and cross-check (the tree lies on some surfaces). Choose the \
                modality at ACTION time: an element ax action \
                (element_token → accessibility rung) or an element px \
                action (x,y → pixel rung off this screenshot). capture_mode is \
                deprecated and ignored. On Wayland, where output capture cannot prove \
                the requested surface's identity, the truthful tree is returned without \
                a screenshot and `screenshot_error.code` is \
                `surface_identity_unproven`.\n\n\
                The mirror image: pass `include_accessibility_tree:false` to SKIP \
                the AT-SPI walk entirely and return just the screenshot plus \
                window metadata (window_bounds, app_name, window_title) — the \
                capture-only path for a live window preview / picture-in-picture. \
                Setting BOTH `include_accessibility_tree:false` and \
                `include_screenshot:false` is an error. Optional `max_dimension` \
                caps the returned screenshot's long edge in pixels for a cheap \
                thumbnail.\n\n\
                Optional `max_elements` / `max_depth` bound the AT-SPI walk \
                (Electron / large web apps produce 10k+ element trees). Markdown and \
                structured elements are truncated identically. Default: `max_elements` 250.\n\n\
                TIME BUDGET: `timeout_ms` (default 1000) bounds the whole AT-SPI \
                walk. When omitted, a window's first snapshot gets a 2000 ms \
                cold-start budget so Chromium/Electron can finish publishing its \
                initial tree. Explicit values are always honored. Large apps \
                (LibreOffice, GIMP, file managers) can exceed the budget; \
                the call then returns the PARTIAL tree with `truncated: true`, \
                `truncation_reason`, `nodes_visited`/`nodes_pending` and \
                `elements_complete: false`. Every element listed is real and \
                clickable; elements after the cut are simply missing. Retry with a \
                larger `timeout_ms` (e.g. 5000) or narrow with `query`/`max_depth` \
                when the element you need is absent.\n\n\
                SCREENSHOT SCALE: the screenshot is delivered at or below 1.15 megapixels \
                (long edge <= max_image_dimension, 1568 by default), because larger images \
                are downsized before a model reads them and its pixel coordinates would then \
                be uniformly short. Element `screenshot_frame`s and x/y for the pointer tools \
                are pixels of the delivered screenshot (`frame` stays in screen coordinates, \
                as on every platform); `frame_scale` < 1 reports the downsizing. An \
                explicit per-call `max_image_dimension` (0 = native) replaces this cap.\n\n\
                POPUP MENUS: a context menu / popover / combo list is an \
                override-redirect window that list_windows never shows. A click or \
                right_click that opened one names it in its result (`popup: \
                {window_id, bounds, title}`); pass that window_id here to walk the \
                popup's own AT-SPI toplevel so its menu items get element indices \
                (then click them by element_token). Omitting window_id while a popup \
                of this pid is open walks that popup.".into(),
            input_schema: cua_driver_core::window_state_view::extend_input_schema(json!({"type":"object","required":["pid"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "pid":{"type":"integer","description":"Process ID that owns the window."},
                "window_id":{"type":"integer","description":"Native window identifier from list_windows, or the `popup.window_id` a click / right_click result named (an open context menu / popover; its menu items then get element indices). Omitted: the pid's open popup menu when one is mapped, else its focused / active / largest window."},
                "capture_mode": cua_driver_core::capture_mode::capture_mode_schema(),
                "include_accessibility_tree":{"type":"boolean",
                    "description":"Default true — walk the AT-SPI tree and return it (per `tree_format`) alongside the screenshot. Set false to SKIP the AT-SPI walk entirely and return just the screenshot plus window metadata (window_bounds, app_name, window_title) — the capture-only path for a live window preview / picture-in-picture. Mirrors include_screenshot. Setting BOTH include_accessibility_tree:false AND include_screenshot:false is an error (nothing to return)."},
                "include_screenshot":{"type":"boolean",
                    "description":"Default true — returns a grounding screenshot alongside the tree. Set false to skip the grab and return tree only (the cheap path for re-indexing before an element ax action)."},
                "screenshot_out_file":{"type":"string",
                    "description":"When set, write the PNG to this file path (~ expanded) instead of embedding base64 in the response. The structured output carries screenshot_file_path instead."},
                "query":{"type":"string","description":"Optional case-insensitive substring. Projects both tree_markdown and structured elements to matches plus ancestors while preserving original indices. Compare total_element_count with returned_element_count."},
                "max_elements":{"type":"integer","minimum":1,"description":"Cap on total AT-SPI nodes walked. Default 250 (5 000 with full_output:true); the response states when the tree was cut. Raise it to read further into a large window."},
                "max_depth":{"type":"integer","minimum":1,"description":"Cap on the AT-SPI tree walk depth. Omit for the default (uncapped). Lower for deeply nested apps."},
                "timeout_ms": cua_driver_core::tool_schema::timeout_ms_schema(),
                "max_dimension":{"type":"integer","minimum":1,"description":"Legacy optional cap on the returned screenshot's long edge. Applied on top of the configured max_image_dimension ceiling when max_image_dimension is omitted."},
                "max_image_dimension":{"type":"integer","minimum":0,"description":"Per-call long-edge override. This value wins over configured and legacy limits; 0 returns native-resolution PNG bytes. Omit to preserve configured behavior."}
            },"additionalProperties":false})),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let display_only = match cua_driver_core::window_state_view::display_only(&args) {
            Ok(display_only) => display_only,
            Err(refusal) => return refusal,
        };
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // `window_id` omitted: the open popup menu of this pid when one is
        // mapped (a context menu the caller wants to read), else the pid's
        // focused / dialog / active / largest window.
        let xid = match args.opt_u64("window_id") {
            Some(v) => v,
            None => {
                let chosen = tokio::task::spawn_blocking(move || {
                    if let Some(popup) = crate::input::mapped_popup_windows()
                        .into_iter()
                        .rev()
                        .find(|p| p.pid == Some(pid))
                    {
                        return Some(popup.window);
                    }
                    let windows = crate::x11::list_windows(Some(pid));
                    let candidates: Vec<u64> = windows.iter().map(|w| w.xid).collect();
                    crate::x11::pick_pid_window(
                        &windows,
                        crate::x11::focused_window_among(&candidates),
                        crate::x11::transient_for,
                        crate::x11::active_window(),
                    )
                })
                .await
                .ok()
                .flatten();
                match chosen {
                    Some(v) => v,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };
        // Retain the legacy additive cap, but let the canonical per-call field
        // replace the configured ceiling entirely (including 0 = native size).
        let max_dimension = args
            .get("max_dimension")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as u32);
        let max_image_dimension = match args.get("max_image_dimension") {
            None => None,
            Some(value) => match value.as_u64().and_then(|value| u32::try_from(value).ok()) {
                Some(value) => Some(value),
                None => {
                    return ToolResult::error(
                        "get_window_state.max_image_dimension must be an integer from 0 through 4294967295.",
                    )
                    .with_structured(json!({ "code": "invalid_arguments" }))
                }
            },
        };
        let explicit_max_image_dimension = max_image_dimension.is_some();
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
        // returns BOTH the AT-SPI tree and a screenshot now, so the agent grounds
        // on both and cross-checks (the tree lies often enough that a grounding
        // screenshot should always be present). The modality is chosen at action
        // time: an element ax action (element_token) or element px action (x,y).
        // We don't even read the arg; it stays in the schema only so old callers
        // don't trip additionalProperties:false.
        let view = match cua_driver_core::window_state_view::ViewOptions::from_args(&args) {
            Ok(view) => view,
            Err(refusal) => return refusal,
        };
        let query = args.opt_str("query");
        let session_id = args.opt_str("_session_id");
        // include_screenshot (default true) — the perf opt-out. The tree+screenshot
        // pair is the default; `include_screenshot:false` skips the grab and returns
        // tree only (the cheap re-index path before an element ax action). A
        // screenshot_out_file still forces a capture (to disk), regardless.
        let include_screenshot = args.get("include_screenshot").and_then(|v| v.as_bool());
        // `include_accessibility_tree` (default true) mirrors include_screenshot:
        // set false to SKIP the AT-SPI walk and return just the screenshot +
        // window metadata (the capture-only / preview path).
        let want_tree = args
            .get("include_accessibility_tree")
            .and_then(|v| v.as_bool())
            != Some(false);
        // screenshot_out_file: when set, write the PNG to disk and surface the
        // path instead of embedding base64 in the response. `~` expands.
        let screenshot_out_file = args.opt_str("screenshot_out_file").map(|s| {
            if let Some(rest) = s.strip_prefix("~/") {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/{rest}")
            } else {
                s
            }
        });
        // Optional caps — when omitted, the AT-SPI walker uses its built-in
        // defaults (#22865).
        let max_elements = Some(
            view.max_elements(
                &args,
                LINUX_DEFAULT_MAX_ELEMENTS,
                display_only
                    || args
                        .get("_observation_only")
                        .and_then(|value| value.as_bool())
                        == Some(true),
            ),
        );
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize);
        let timeout_ms = linux_snapshot_timeout_ms(
            args.get("timeout_ms"),
            self.state
                .snapshots
                .contains_semantic_window(pid as i32, xid),
        );
        let walk_timeout = std::time::Duration::from_millis(timeout_ms);

        let process_is_live = crate::proc_fs::is_process_live(pid);
        // Enumerate the pid's windows ONCE and reuse the result for both the
        // window-ownership check (Wayland) and the additive window metadata
        // below, instead of paying for the compositor/X11 enumeration twice.
        // `window_meta` also names the surface + its on-screen rectangle on the
        // capture-only path, where no AT-SPI tree identifies it.
        let popup_meta = (!crate::wayland::is_wayland())
            .then(|| crate::input::popup_window_info(xid))
            .flatten();
        let window_meta = crate::wayland::list_windows_dispatch(Some(pid))
            .into_iter()
            .find(|w| w.xid == xid)
            .or_else(|| {
                popup_meta.as_ref().map(|popup| crate::x11::WindowInfo {
                    xid: popup.window,
                    pid: popup.pid,
                    app_name: String::new(),
                    title: popup.title.clone(),
                    is_on_screen: true,
                    z_index: None,
                    x: popup.x,
                    y: popup.y,
                    width: popup.width,
                    height: popup.height,
                })
            });
        let window_matches = if crate::wayland::is_wayland() {
            window_meta.as_ref().is_some_and(|w| w.pid == Some(pid))
                || crate::wayland::window_was_listed_for_pid(pid, xid)
        } else {
            crate::x11::window_belongs_to_pid(xid, pid)
                || crate::input::popup_window_info(xid)
                    .is_some_and(|popup| popup.pid.is_none_or(|owner| owner == pid))
                // A cross-process dialog/plugin window transient-for one of
                // pid's own windows (GIMP export dialogs, LibreOffice's
                // Document Recovery) legitimately belongs to a different pid
                // than the caller's target; accept it here too so the
                // get_window_state(window_id=...) follow-up named in the
                // dialogs[] overlay actually works.
                || crate::x11::list_cross_pid_transient_windows(pid)
                    .iter()
                    .any(|w| w.xid == xid)
        };
        if !process_is_live || !window_matches {
            return ToolResult::error(format!(
                "Window target pid {pid}, window_id {xid} is stale or no longer running; refresh list_windows."
            ));
        }

        // Always walk the AT-SPI tree; capture the screenshot by default. The
        // tree+screenshot pair is the default so the agent grounds on both and
        // cross-checks the (sometimes-lying) tree against the frame. An explicit
        // `include_screenshot:false` skips the grab; an unproven Wayland surface
        // returns the tree with a typed screenshot error instead of unrelated pixels.
        let should_capture = include_screenshot != Some(false) || screenshot_out_file.is_some();
        if !want_tree && !should_capture {
            return ToolResult::error(
                "Nothing to return: both include_accessibility_tree:false and \
                 include_screenshot:false. Set at least one to true, or pass \
                 screenshot_out_file to force a capture.",
            );
        }
        // `display_only` is the public form of the internal observation-only
        // read: pixels for a preview, with no snapshot or capture change.
        let observation_only = display_only
            || args
                .get("_observation_only")
                .and_then(|value| value.as_bool())
                == Some(true);
        let state = self.state.clone();
        let state_for_capture = state.clone();
        let query_for_walk = query.clone();

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            // Skip the AT-SPI walk on the capture-only path
            // (include_accessibility_tree:false).
            let tree_result = if want_tree {
                Some(crate::atspi::walk_tree_bounded_within(
                    pid,
                    xid,
                    query_for_walk.as_deref(),
                    max_elements,
                    max_depth,
                    walk_timeout,
                ))
            } else {
                None
            };
            let binds_window = tree_result.as_ref().is_some_and(|tree| {
                crate::atspi::snapshot_binds_window(pid, xid, tree.window_scoped, tree.trusted)
            });
            // Bounds and element indices come from the same captured AT-SPI
            // traversal. Joining two live walks by ordinal mis-associated
            // Chromium controls when its lazy subtree changed between walks.
            let bounds = tree_result
                .as_ref()
                .map(|tree| tree.bounds.clone())
                .unwrap_or_default();
            // Capture and DELIVER the screenshot alongside the tree by default — the
            // grounding frame the agent cross-checks the tree against. With
            // screenshot_out_file set, write to disk and surface the path instead
            // of embedding base64; otherwise embed base64. Skipped only when
            // include_screenshot:false and no disk path was requested.
            // Keep the exact delivered PNG for capture publication. The action
            // binding must identify the bytes returned to the caller, including
            // any max-dimension resize, rather than the backend's raw frame.
            let mut screenshot_error = None;
            // Same-pid popups (menus, combo lists) and transient dialogs mapped
            // over this window: listed for the caller, and when one overlaps
            // the window the screenshot is taken from the screen, since the
            // window's own drawable never shows them.
            let overlays = if crate::wayland::is_wayland() {
                WindowOverlays::default()
            } else {
                window_overlays(pid, xid)
            };
            let screenshot = if should_capture {
                // A screenshot-only read with a size cap (a live preview) asks
                // a GNOME helper with API 10 to downscale in the compositor,
                // and keeps the window's full size for frame_scale.
                let preview = if !want_tree && max_dim > 0 && crate::wayland::is_wayland() {
                    crate::wayland::shell_helper::screenshot_window_preview(xid, pid, max_dim)
                } else {
                    None
                };
                let captured = if let Some((png, width, height)) = preview {
                    Ok((png, Some((width, height))))
                } else if overlays.covers_window {
                    overlays
                        .window_rect
                        .ok_or_else(|| anyhow::anyhow!("window geometry unavailable"))
                        .and_then(|(x, y, w, h)| {
                            crate::capture::screenshot_root_region_png(x, y, w, h)
                        })
                        .or_else(|_| crate::wayland::screenshot_dispatch_with_pid(xid, pid))
                        .map(|png| (png, None))
                } else {
                    crate::wayland::screenshot_dispatch_with_pid(xid, pid).map(|png| (png, None))
                };
                match captured {
                    Ok((raw, original_size)) => {
                        let (orig_w, orig_h) = match original_size {
                            Some(size) => size,
                            None => crate::capture::png_dimensions_pub(&raw)?,
                        };
                        let png = crate::capture::resize_png_if_needed(&raw, max_dim)?;
                        let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                        // An explicit per-call max_image_dimension (0 = native)
                        // is authoritative; the model-safe cap applies otherwise.
                        let cap = if explicit_max_image_dimension {
                            None
                        } else {
                            megapixel_long_edge_cap(w, h, WINDOW_SCREENSHOT_MAX_PIXELS)
                        };
                        let png = match cap {
                            Some(edge) => crate::capture::resize_png_if_needed(&png, edge)?,
                            None => png,
                        };
                        let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                        let original_w = if w < orig_w { Some(orig_w) } else { None };
                        let (b64, file_path) = if let Some(ref path) = screenshot_out_file {
                            std::fs::write(path, &png)?;
                            (None, Some(path.clone()))
                        } else {
                            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                            (Some(B64.encode(&png)), None)
                        };
                        let capture_id = if observation_only {
                            None
                        } else {
                            Some(crate::capture_action_frame::publish_window(
                                &state_for_capture.capture_service,
                                &args,
                                &png,
                                pid,
                                xid,
                                (w, h),
                                (orig_w, orig_h),
                            )?)
                        };
                        Some((b64, file_path, w, h, original_w, capture_id))
                    }
                    Err(error) if crate::wayland::is_surface_identity_unproven(&error) => {
                        screenshot_error = Some(error.to_string());
                        None
                    }
                    // The window passed the ownership check above and went
                    // away during the AT-SPI walk (a document closed by the
                    // action being observed): a stale target, not a capture
                    // failure.
                    Err(_) if !crate::wayland::is_wayland() && !crate::x11::window_exists(xid) => {
                        return Err(anyhow::anyhow!(
                            "Window target pid {pid}, window_id {xid} is stale or no longer running; refresh list_windows."
                        ));
                    }
                    Err(error) => {
                        return Err(anyhow::anyhow!(
                            "window screenshot failed for window {xid}: {error}"
                        ));
                    }
                }
            } else {
                None
            };
            Ok((
                tree_result,
                screenshot,
                bounds,
                screenshot_error,
                overlays,
                binds_window,
            ))
        })
        .await;

        match result {
            Ok(Ok((tree_opt, shot_opt, bounds, screenshot_error, overlays, binds_window))) => {
                let mut content = Vec::new();
                let mut structured = json!({ "window_id": xid, "pid": pid });
                let screenshot_scale = shot_opt.as_ref().map(|(_, _, w, _, original_w, _)| {
                    original_w.map_or(1.0, |ow| ow as f64 / *w as f64)
                });
                let mut published_snapshot = false;
                let mut invalidated = Vec::new();

                if let Some(tr) = tree_opt {
                    let source_trusted = tr.trusted;
                    let source_degraded_reason = tr.degraded_reason.clone();
                    let count = tr
                        .nodes
                        .iter()
                        .filter(|n| n.element_index.is_some())
                        .count();
                    let mut header = format!(
                        "window_id={xid} pid={pid} elements={count} walk_ms={}\n",
                        tr.elapsed_ms
                    );
                    if tr.truncated {
                        header.push_str(&cua_driver_core::walk_budget::truncation_note(
                            tr.truncation_reason.as_deref(),
                            timeout_ms,
                            tr.nodes_visited,
                            tr.nodes_pending,
                        ));
                        header.push('\n');
                    } else if !tr.bounds_complete {
                        header.push_str(
                            "⚠️ bounds phase ran out of time: some elements have no frame \
                             (element_token clicks still work; pixel targeting may not). \
                             Retry with a larger timeout_ms if you need frames.\n",
                        );
                    }
                    header.push('\n');
                    content.push(cua_driver_core::protocol::Content::text(
                        header + &tr.tree_markdown,
                    ));
                    structured["element_count"] = json!(count);
                    // `elements_complete` is a real claim now: a native walk that
                    // finished without hitting the deadline / node budget saw
                    // every node the application publishes. Truncated or
                    // fallback trees keep negative existence unknown.
                    structured["elements_complete"] = json!(tr.trusted && !tr.truncated);
                    structured["truncated"] = json!(tr.truncated);
                    if let Some(reason) = &tr.truncation_reason {
                        structured["truncation_reason"] = json!(reason);
                    }
                    structured["nodes_visited"] = json!(tr.nodes_visited);
                    structured["nodes_pending"] = json!(tr.nodes_pending);
                    structured["bounds_complete"] = json!(tr.bounds_complete);
                    structured["walk_elapsed_ms"] = json!(tr.elapsed_ms as u64);
                    structured["timeout_ms"] = json!(timeout_ms);
                    structured["tree_markdown"] = json!(tr.tree_markdown);

                    // Elements bind to this window only when the walk proved
                    // the window's scope (`snapshot_binds_window`).
                    let target_scoped = binds_window;
                    if !observation_only && !target_scoped {
                        invalidated.extend(state.snapshots.remove(pid as i32, xid));
                        crate::atspi::snapshot::forget_window(pid, xid);
                    }
                    let (snapshot_id, replaced) = (!observation_only && target_scoped)
                        .then(|| {
                            state.snapshots.publish_for_session(
                                pid as i32,
                                xid,
                                crate::atspi::snapshot::update_snapshot(
                                    pid, xid, &tr.nodes, &tr.bounds,
                                ),
                                session_id.as_deref(),
                                screenshot_scale,
                            )
                        })
                        .flatten()
                        .unzip();
                    published_snapshot = snapshot_id.is_some();
                    invalidated.extend(replaced.into_iter().flatten());

                    // Structured `elements` array: one entry per actionable node.
                    // Shape: `{element_index, element_token, role, label,
                    // depth, actions?, parent_index?, frame?: {x,y,w,h}}`. Frame is
                    // included whenever AT-SPI Component.GetExtents(Screen)
                    // reported usable bounds; omitted otherwise (some
                    // toolkits leave bounds unset on hidden / virtual
                    // elements).
                    use std::collections::HashMap;
                    // The walk produces screen extents (what the element
                    // cache and hit-tests compare against). The published
                    // `frame` is the element's screen rectangle, as on macOS and
                    // Windows; `screenshot_frame` (added below) is the same
                    // rectangle in pixels of the screenshot in this response,
                    // the space of the pointer tools' window-local x/y. The
                    // window-local origin is the X11 window's root-relative
                    // origin, the one `window_bounds` and
                    // `coordinate_frame:"desktop"` translation use. A popup
                    // (override-redirect) window is not in the WM's client
                    // list; when its origin cannot be read, the popup's own
                    // screen rectangle is the origin.
                    let local_origin = (!crate::wayland::is_wayland())
                        .then(|| crate::atspi::native::x11_window_origin(xid))
                        .flatten()
                        .or_else(|| popup_meta.as_ref().map(|popup| (popup.x, popup.y)));
                    let bounds_by_idx: HashMap<usize, (i32, i32, u32, u32)> = bounds
                        .into_iter()
                        .map(|(i, x, y, w, h)| (i, (x, y, w, h)))
                        .collect();
                    let elements: Vec<serde_json::Value> = tr
                        .nodes
                        .iter()
                        .filter_map(|n| {
                            build_element_entry(
                                n,
                                snapshot_id,
                                bounds_by_idx.get(&n.element_index?).copied(),
                            )
                        })
                        .collect();
                    let elements = cua_driver_core::element_query::project_elements_for_query(
                        elements,
                        query.as_deref(),
                        &tr.tree_markdown,
                    );
                    let elements = framed_elements_first(elements);
                    // Screenshot pixels: the capture is the window's own X11
                    // drawable (screen pixels), downsized by delivered/native.
                    let elements = match (local_origin, shot_opt.as_ref()) {
                        (Some((ox, oy)), Some((_, _, w, _, orig_w, _))) => {
                            let scale = orig_w.map_or(1.0, |ow| *w as f64 / ow as f64);
                            cua_driver_core::element_frame::with_screenshot_frames(
                                elements,
                                (f64::from(ox), f64::from(oy)),
                                scale,
                            )
                        }
                        _ => elements,
                    };
                    // A popup that has no AT-SPI frame of its own (LibreOffice
                    // VCL menus live under the menubar's `menu` node): return
                    // the open menu's items, the ones drawn inside the popup,
                    // instead of the whole application.
                    let elements = match (&popup_meta, tr.window_scoped) {
                        (Some(popup), false) => popup_menu_elements(
                            elements,
                            (popup.x, popup.y),
                            popup.width,
                            popup.height,
                        ),
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
                         Use `max_elements` / `max_depth` to bound the \
                         AT-SPI walk on apps with very large trees."
                    );
                    // Best-effort-background ladder parity with macOS/Windows: an
                    // AT-SPI walk that ran but found zero actionable elements is
                    // NOT a clean "this window has no controls" — far more often
                    // the bridge wasn't ready (toolkit-accessibility off, or the
                    // daemon isn't on the desktop session bus so the registry is
                    // empty), or it's a non-AX surface (canvas/WebGL). Mark it
                    // degraded so callers don't read `elements: []` as authoritative.
                    if !target_scoped {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!("accessibility_window_identity_unproven: tree is application-scoped; exact-window element tokens and bounds are unavailable");
                    } else if !source_trusted {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!(source_degraded_reason
                            .unwrap_or_else(|| {
                                "x11_property_fallback_partial: AT-SPI was unavailable and \
                                 Cua Driver only recovered window metadata. Treat it as \
                                 discovery evidence; it cannot prove checked state."
                                    .to_owned()
                            }));
                    } else if count == 0 && tr.truncated {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] =
                            json!(cua_driver_core::walk_budget::truncation_note(
                                tr.truncation_reason.as_deref(),
                                timeout_ms,
                                tr.nodes_visited,
                                tr.nodes_pending,
                            ));
                    } else if count == 0 {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!(
                            "atspi_tree_empty: the AT-SPI walk returned no actionable \
                             elements. Common causes: the a11y bridge is off (enable \
                             `gsettings set org.gnome.desktop.interface \
                             toolkit-accessibility true`), the daemon is not on the \
                             desktop session bus (DBUS_SESSION_BUS_ADDRESS unreachable — \
                             run `cua-driver doctor`), or the window is a non-AX surface \
                             (canvas/WebGL/custom-drawn). Do not treat element data as \
                             authoritative — verify via the screenshot, and re-snapshot \
                             after enabling a11y or if the app just launched."
                        );
                        // Point the agent at the next rung explicitly: an empty
                        // tree means element_index has nothing to bind to. The
                        // recommendation is session-dependent (X11 → px,
                        // Wayland → foreground) — see non_ax_escalation.
                        structured["escalation"] = non_ax_escalation();
                    }
                }

                // A screenshot-only read adds an image but no element rows: it
                // refreshes the current snapshot's pixel frame in place and
                // keeps its tokens valid. Only a window with no element
                // snapshot yet gets a new capture-only one.
                let mut refreshed = None;
                if let (false, false, Some(scale)) =
                    (observation_only, published_snapshot, screenshot_scale)
                {
                    refreshed = state.snapshots.refresh_screenshot_for_session(
                        pid as i32,
                        xid,
                        session_id.as_deref(),
                        scale,
                        None,
                    );
                    if refreshed.is_none() {
                        if let Some((_, replaced)) = state.snapshots.publish_capture_for_session(
                            pid as i32,
                            xid,
                            crate::atspi::snapshot::AtspiSnapshot::from_nodes(&[]),
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

                if let Some((b64_opt, file_path, w, h, orig_w, capture_id)) = shot_opt {
                    // ax mode + screenshot_out_file writes the PNG to disk and
                    // returns b64=None — never embed the image bytes in that case.
                    // Keep a text content part when the image went to disk so the
                    // response is never empty on the capture-only path (which has
                    // no tree markdown either).
                    if let Some(b64) = b64_opt {
                        content.push(cua_driver_core::protocol::Content::image_png(b64));
                    } else if let Some(fp) = &file_path {
                        content.push(cua_driver_core::protocol::Content::text(format!(
                            "window_id={xid} pid={pid} size={w}x{h} screenshot written to {fp}"
                        )));
                    }
                    structured["screenshot_width"] = json!(w);
                    structured["screenshot_height"] = json!(h);
                    structured["screenshot_frame_valid"] = json!(true);
                    if let Some(ow) = orig_w {
                        if ow > 0 {
                            structured["frame_scale"] = json!(w as f64 / ow as f64);
                            structured["screenshot_original_width"] = json!(ow);
                        }
                    }
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
                }
                if let Some(reason) = &screenshot_error {
                    structured["screenshot_frame_valid"] = json!(false);
                    structured["screenshot_error"] =
                        surface_identity_unproven_error(xid, reason.clone());
                }
                // Window identity metadata (additive): app + title + on-screen
                // rectangle for the requested window_id, useful on the
                // capture-only path where no AT-SPI tree names the surface.
                if popup_meta.is_some() {
                    structured["popup"] = json!(true);
                }
                if let Some(meta) = &window_meta {
                    if !meta.app_name.is_empty() {
                        structured["app_name"] = json!(meta.app_name);
                    }
                    if !meta.title.is_empty() {
                        structured["window_title"] = json!(meta.title);
                    }
                    structured["window_bounds"] = json!({
                        "x": meta.x, "y": meta.y, "width": meta.width, "height": meta.height
                    });
                }
                // Transient dialogs and popups of this pid that are open over
                // the window, each with the call that targets it.
                if !overlays.dialogs.is_empty() {
                    structured["dialogs"] = json!(overlays.dialogs);
                }
                if !overlays.popups.is_empty() {
                    structured["popups"] = json!(overlays.popups);
                }
                if overlays.covers_window {
                    structured["screenshot_composited"] = json!(true);
                }
                if let Some(note) = overlays.follow_up(pid) {
                    structured["follow_up"] = json!(note);
                    content.push(cua_driver_core::protocol::Content::text(note));
                }
                structured["coordinate_frame"] = json!("window");
                structured["frame_note"] = json!(
                    "x/y for click / double_click / right_click / drag / scroll on this \
                     window are pixels of THIS screenshot (window-local, 0..screenshot_width \
                     x 0..screenshot_height), as are the element frames; the screenshot is \
                     kept at or below 1.15 megapixels so the image you read is the image these \
                     pixels index (frame_scale < 1 says the window was downsized to it). \
                     window_bounds is where it sits on the screen. Pass scope:\"desktop\" only \
                     for get_desktop_state pixels."
                );
                if display_only {
                    cua_driver_core::window_state_view::mark_display_only(&mut structured);
                }

                // The capture-only path (include_accessibility_tree:false) leaves
                // `content` empty when the screenshot was also unavailable — most
                // often on Wayland, where per-window capture cannot prove surface
                // identity. Return a structured error rather than a "successful"
                // response with no content parts.
                if content.is_empty() {
                    let reason_note = match &screenshot_error {
                        Some(reason) => format!(" and no screenshot could be captured ({reason})"),
                        None => " and no screenshot was returned".to_string(),
                    };
                    return ToolResult::error(format!(
                        "No content produced for window_id {xid}: the accessibility tree was \
                         skipped (include_accessibility_tree:false){reason_note}."
                    ))
                    .with_structured(structured);
                }

                if !observation_only {
                    cua_driver_core::window_state_view::apply(
                        &view,
                        &cua_driver_core::window_state_view::ViewContext {
                            pid: i64::from(pid),
                            window_id: xid,
                            key: cua_driver_core::window_state_view::ViewKey {
                                query: query.clone(),
                                max_elements: max_elements.unwrap_or(LINUX_DEFAULT_MAX_ELEMENTS),
                                max_depth,
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
            Ok(Err(e)) => ToolResult::error(format!("Capture error: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// One-line, model-facing explanation of a partial tree and what to do about
/// it. Shared by the text header and the structured `degraded_reason`.
fn surface_identity_unproven_error(xid: u64, reason: String) -> Value {
    json!({
        "code": "surface_identity_unproven",
        "window_id": xid,
        "reason": reason,
        "suggestion": "capture the full output explicitly, or retry on a compositor backend that supports identified per-window capture"
    })
}

#[cfg(test)]
mod get_window_state_actions_tests {
    use super::*;
    use crate::atspi::AtspiNode;

    fn node(actions: Vec<String>) -> AtspiNode {
        AtspiNode {
            element_index: Some(1),
            role: "button".to_owned(),
            name: Some("ok".to_owned()),
            value: None,
            checked: None,
            enabled: Some(true),
            selected: None,
            description: None,
            actions,
            element_key: 1,
            identity: None,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
            object_ref: None,
        }
    }

    #[test]
    fn element_entry_omits_actions_when_empty() {
        let n = node(Vec::new());
        let entry = build_element_entry(&n, None, None).unwrap();
        assert!(entry.get("actions").is_none());
    }

    #[test]
    fn element_entry_filters_blank_action_names() {
        let n = node(vec![
            "Press".to_owned(),
            "".to_owned(),
            "   ".to_owned(),
            "Open".to_owned(),
        ]);
        let entry = build_element_entry(&n, None, None).unwrap();
        assert_eq!(entry["actions"], json!(["Press", "Open"]));
    }
}

#[cfg(test)]
mod display_only_tests {
    use super::{GetWindowStateTool, ToolState};
    use cua_driver_core::tool::Tool;

    // T3 Code feature-detects the preview mode from this schema property.
    #[test]
    fn display_only_is_advertised() {
        let tool = GetWindowStateTool {
            state: ToolState::new(),
        };
        assert_eq!(
            tool.def().input_schema["properties"]["display_only"]["type"],
            "boolean"
        );
    }

    #[tokio::test]
    async fn display_only_refuses_accessibility_snapshot_requests() {
        let tool = GetWindowStateTool {
            state: ToolState::new(),
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
#[cfg(test)]
mod window_capture_dimension_tests {
    use super::{GetWindowStateTool, ToolState};
    use cua_driver_core::tool::Tool;

    #[test]
    fn schema_exposes_zero_as_native_resolution() {
        let tool = GetWindowStateTool {
            state: ToolState::new(),
        };
        assert_eq!(
            tool.def().input_schema["properties"]["max_image_dimension"]["minimum"],
            0
        );
    }
}
