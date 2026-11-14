// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What an input action visibly did, returned with the action.
//!
//! Two checks wrap the pointer and keyboard tools and invoke_menu:
//!
//! - New windows: the target app's on-screen windows are listed before and
//!   after the action; windows that appeared (a dialog, a sheet, a child
//!   window) are named in the result with their id, title and bounds, so the
//!   caller can read them without a `list_windows` round trip.
//! - Text changes in windows missing from the accessibility tree (see
//!   [`cua_driver_core::opaque_text`]): such a window has no tree to read an
//!   effect back from, so the action recaptures it after a short settle,
//!   reads its text and lists the rows that differ from the window's last
//!   text read, in that read's pixel frame. A window that changed is
//!   `confirmed` with `window_change` evidence; one whose rows are all the
//!   same is `suspected_noop`.
//!
//! - Cell changes in spreadsheet grids (see
//!   [`cua_driver_core::sheet_grid`]): typing into a sheet gives no element to
//!   read the entry back from, so for a window whose last read showed a grid
//!   the grid is read again after the action and the result lists the cells
//!   whose values changed, the active cell and its value, and a switch to
//!   another sheet. A change confirms the action.
//!
//! The text check runs only for windows the last read found opaque, and is
//! bounded: past [`EVIDENCE_BUDGET`] the result is returned as the action
//! produced it, with `ocr_status`. The grid check runs only for windows whose
//! last read or action showed a grid.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::opaque_text::{self, RowChange};
use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::{
    ProtectedResourceOwnership, ProtectedResourceScopeError, Tool, ToolDef,
};
use serde_json::{json, Value};

use super::ToolState;
use crate::ocr::RowFrame;

/// Time the target app gets to repaint before the window is read again.
const SETTLE: Duration = Duration::from_millis(250);
/// Longest the after-action read may take before the action returns without it.
const EVIDENCE_BUDGET: Duration = Duration::from_millis(2500);
/// Longest the before-action read may take (only when no earlier read exists).
const BASELINE_BUDGET: Duration = Duration::from_millis(2000);
/// Extra time a sheet gets to apply queued input when the first read after an
/// action shows no change (a long `type_text` is still being processed).
const GRID_RETRY: Duration = Duration::from_millis(400);
/// Longest one grid read may take, off the async runtime.
const GRID_READ_BUDGET: Duration = Duration::from_millis(3000);
/// At most this many text rows are shown for a new opaque window.
const NEW_WINDOW_ROWS: usize = 12;
/// Windows smaller than this (either edge, points) are not reported as new.
const MIN_WINDOW_EDGE: f64 = 20.0;

/// Wraps an input tool with the checks described in the module docs.
pub(super) struct ActionEvidence<T> {
    inner: T,
    state: Arc<ToolState>,
}

impl<T> ActionEvidence<T> {
    pub(super) fn new(inner: T, state: Arc<ToolState>) -> Self {
        Self { inner, state }
    }
}

/// A window that appeared during an action.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NewWindow {
    pub window_id: u32,
    pub title: String,
    pub bounds: [i64; 4],
}

/// The windows in `after` that were not in `before`, large enough to be a
/// dialog or a document (not a tooltip), front to back.
pub(crate) fn new_windows(
    before: &HashSet<u32>,
    after: &[crate::windows::WindowInfo],
) -> Vec<NewWindow> {
    let mut windows: Vec<&crate::windows::WindowInfo> = after
        .iter()
        .filter(|w| !before.contains(&w.window_id))
        .filter(|w| w.bounds.width >= MIN_WINDOW_EDGE && w.bounds.height >= MIN_WINDOW_EDGE)
        .collect();
    windows.sort_by_key(|w| w.z_index);
    windows
        .into_iter()
        .map(|w| NewWindow {
            window_id: w.window_id,
            title: w.title.clone(),
            bounds: [
                w.bounds.x.round() as i64,
                w.bounds.y.round() as i64,
                w.bounds.width.round() as i64,
                w.bounds.height.round() as i64,
            ],
        })
        .collect()
}

fn app_windows(pid: i32) -> Vec<crate::windows::WindowInfo> {
    crate::windows::visible_windows_without_spaces()
        .into_iter()
        .filter(|w| w.pid == pid)
        .collect()
}

/// The (pid, window) an action addresses, after the window-target guard
/// resolved a pid-only call.
fn action_target(state: &ToolState, args: &Value) -> (Option<i32>, Option<u32>) {
    if let Some((pid, window)) = state.snapshots.window_for_token(args) {
        return (Some(pid), u32::try_from(window).ok());
    }
    let pid = args
        .get("pid")
        .and_then(Value::as_i64)
        .and_then(|pid| i32::try_from(pid).ok());
    let window = args
        .get("window_id")
        .and_then(Value::as_u64)
        .and_then(|window| u32::try_from(window).ok());
    (pid, window)
}

/// Whether the action ran (a refusal or an error has nothing to report).
fn action_ran(result: &ToolResult) -> bool {
    result.is_error != Some(true)
        && result
            .structured_content
            .as_ref()
            .and_then(|s| s.get("effect"))
            .and_then(Value::as_str)
            != Some("refused")
}

/// What the after-action checks found.
#[derive(Debug, Default)]
pub(crate) struct Findings {
    /// `None` when the window's text was not read.
    pub changes: Option<Vec<RowChange>>,
    pub new_windows: Vec<(NewWindow, Option<String>)>,
    /// Why the text was not read, when it should have been.
    pub ocr_status: Option<String>,
    /// What the action did to a spreadsheet grid in the window.
    pub sheet: Option<SheetFinding>,
}

/// A spreadsheet grid read back after an action.
#[derive(Debug)]
pub(crate) struct SheetFinding {
    pub text: String,
    /// Whether any cell, the active cell, the sheet or the view changed.
    pub changed: bool,
}

impl Findings {
    fn is_empty(&self) -> bool {
        self.changes.is_none()
            && self.new_windows.is_empty()
            && self.ocr_status.is_none()
            && self.sheet.is_none()
    }

    /// The text appended to the action's result.
    pub(crate) fn render(&self, window_id: Option<u32>) -> String {
        let mut out = Vec::new();
        match &self.changes {
            Some(changes) if changes.is_empty() => out.push(format!(
                "No visible change: window {} text rows are the same as before the action.",
                window_id.unwrap_or_default()
            )),
            Some(changes) => out.push(format!(
                "Window {} rows that changed ({}):\n{}",
                window_id.unwrap_or_default(),
                changes.len(),
                opaque_text::render_changes(changes)
            )),
            None => {}
        }
        if let Some(sheet) = &self.sheet {
            out.push(sheet.text.clone());
        }
        for (window, rows) in &self.new_windows {
            let [x, y, w, h] = window.bounds;
            match rows {
                // The window was captured for these rows and that capture is
                // now its screenshot frame: pixels off the rows are accepted.
                Some(rows) => {
                    out.push(format!(
                        "New window {} \"{}\" at ({x},{y}) {w}x{h} pt; this capture is now its \
                         screenshot, so pass window_id {} with x,y from the rows below (no read \
                         needed first).",
                        window.window_id,
                        window.title.replace('"', "'"),
                        window.window_id
                    ));
                    if !rows.is_empty() {
                        out.push(format!("Its text (x,y in its screenshot pixels):\n{rows}"));
                    }
                }
                None => out.push(format!(
                    "New window {} \"{}\" at ({x},{y}) {w}x{h} pt; read it with get_window_state \
                     (window_id {}) before pixel actions on it.",
                    window.window_id,
                    window.title.replace('"', "'"),
                    window.window_id
                )),
            }
        }
        if let Some(status) = &self.ocr_status {
            out.push(format!(
                "Window text after the action was not read: {status}."
            ));
        }
        out.join("\n")
    }

    /// Fold the findings into the action's legacy structured payload: a
    /// visible change is `window_change` evidence (and confirms an action
    /// nothing else confirmed); unchanged rows make it `suspected_noop`.
    pub(crate) fn apply(&self, structured: &mut serde_json::Map<String, Value>, text: &str) {
        let changed = self.changes.as_ref().is_some_and(|c| !c.is_empty());
        let unchanged = self.changes.as_ref().is_some_and(Vec::is_empty);
        let appeared = !self.new_windows.is_empty();
        let sheet_changed = self.sheet.as_ref().is_some_and(|s| s.changed);
        let effect = structured
            .get("effect")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let unconfirmed = matches!(effect.as_deref(), None | Some("unverifiable"));
        if sheet_changed {
            // The cells were read back: no read is needed to check the entry.
            structured.remove("next_call");
            structured.remove("hint");
        }
        if changed || appeared || sheet_changed {
            let evidence = structured
                .entry("evidence")
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(items) = evidence {
                items.push(json!({ "kind": "window_change", "detail": text }));
            }
            if unconfirmed {
                structured.insert("effect".into(), json!("confirmed"));
                structured.insert("verified".into(), json!(true));
            }
        } else if unchanged && unconfirmed {
            structured.insert("effect".into(), json!("suspected_noop"));
        }
        if let Some(changes) = &self.changes {
            structured.insert(
                "changed_rows".into(),
                json!(changes
                    .iter()
                    .take(opaque_text::MAX_CHANGES)
                    .map(opaque_text::render_change)
                    .collect::<Vec<_>>()),
            );
            structured.insert("changed_row_count".into(), json!(changes.len()));
        }
        if appeared {
            structured.insert(
                "new_windows".into(),
                json!(self
                    .new_windows
                    .iter()
                    .map(|(w, _)| json!({
                        "window_id": w.window_id,
                        "title": w.title,
                        "bounds": w.bounds,
                    }))
                    .collect::<Vec<_>>()),
            );
        }
        if let Some(status) = &self.ocr_status {
            structured.insert("ocr_status".into(), json!(status));
        }
    }
}

/// Add `text` to the result's first text block: clients that read only
/// `structuredContent` get that block as the action's `summary`.
fn append_text(result: &mut ToolResult, text: String) {
    for content in &mut result.content {
        if let Content::Text { text: existing, .. } = content {
            if !existing.trim().is_empty() {
                existing.push('\n');
                existing.push_str(&text);
                return;
            }
        }
    }
    result.content.insert(0, Content::text(text));
}

/// Read `window_id`'s text in `frame`, off the async runtime and
/// within `budget`.
/// The element of `window_id` holding keyboard focus, then up to three of its
/// ancestors, as role and name (title, else description).
/// `None` when the app's focus is not in that window.
fn focus_chain(pid: i32, window_id: u32) -> Option<cua_driver_core::sheet_grid::FocusChain> {
    use crate::ax::bindings::{copy_element_attr, copy_string_attr};
    use core_foundation::base::{CFRelease, CFTypeRef};
    unsafe {
        let mut element = crate::ax::exact_target::focused_element_in_window(pid, window_id)?;
        let mut chain = Vec::new();
        for level in 0..4 {
            let attr = |name: &str| copy_string_attr(element, name).filter(|v| !v.is_empty());
            chain.push(cua_driver_core::sheet_grid::FocusElement {
                role: attr("AXRole").unwrap_or_default(),
                name: attr("AXTitle")
                    .or_else(|| attr("AXDescription"))
                    .unwrap_or_default(),
            });
            let parent = if level < 3 {
                copy_element_attr(element, "AXParent")
            } else {
                None
            };
            CFRelease(element as CFTypeRef);
            match parent {
                Some(parent) => element = parent,
                None => break,
            }
        }
        Some(chain)
    }
}

/// Read `window_id`'s spreadsheet grid now, off the async runtime.
async fn read_grid(pid: i32, window_id: u32) -> Option<cua_driver_core::sheet_grid::Grid> {
    let task = tokio::task::spawn_blocking(move || crate::sheet::read_grid(pid, window_id));
    tokio::time::timeout(GRID_READ_BUDGET, task)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
}

async fn read_rows(
    window_id: u32,
    frame: RowFrame,
    budget: Duration,
) -> Result<crate::ocr::TextBaseline, String> {
    let task = tokio::task::spawn_blocking(move || crate::ocr::read_window_rows(window_id, frame));
    match tokio::time::timeout(budget, task).await {
        Ok(Ok(Ok(rows))) => Ok(rows),
        Ok(Ok(Err(e))) => Err(format!("failed: {e}")),
        Ok(Err(e)) => Err(format!("failed: {e}")),
        Err(_) => Err("timed out".to_owned()),
    }
}

#[async_trait]
impl<T: Tool + 'static> Tool for ActionEvidence<T> {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    fn has_independent_input_lane(&self, args: &Value) -> bool {
        self.inner.has_independent_input_lane(args)
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        self.inner
            .protected_resource_ownership(adapter_id, args)
            .await
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, ProtectedResourceScopeError> {
        self.inner.protected_resource_scope(adapter_id, args).await
    }

    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), ProtectedResourceScopeError> {
        self.inner
            .validate_protected_resource_scope(adapter_id, args, approved_scope)
            .await
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let (pid, window_id) = action_target(&self.state, &args);
        let session = args
            .get("_session_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(pid) = pid else {
            return self.inner.invoke(args).await;
        };
        let opaque_window = window_id.filter(|&w| crate::ocr::known_opaque(pid, w));

        // Before: the app's windows, and the opaque window's text if no read
        // left one to compare with.
        let before: HashSet<u32> = tokio::task::spawn_blocking(move || app_windows(pid))
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|w| w.window_id)
            .collect();
        let mut ocr_status = None;
        let mut baseline = None;
        if let Some(window) = opaque_window {
            baseline = crate::ocr::baseline(pid, window);
            if baseline.is_none() {
                match self.action_frame(&args, pid, window) {
                    Some(frame) => match read_rows(window, frame, BASELINE_BUDGET).await {
                        Ok(rows) => baseline = Some(rows),
                        Err(status) => ocr_status = Some(status),
                    },
                    None => ocr_status = Some("no screenshot of the window to compare with".into()),
                }
            }
        }

        // A sheet's grid before the action: the last read's, when it saw the
        // whole window, else read now.
        let grid_window = window_id.filter(|&w| crate::sheet::last_grid(pid, w).is_some());
        let mut grid_before = None;
        if let Some(window) = grid_window {
            grid_before = crate::sheet::last_grid(pid, window).filter(|g| g.complete);
            if grid_before.is_none() {
                grid_before = read_grid(pid, window).await;
            }
        }

        // A Return/Tab (or text ending in one) is expected to commit an entry
        // or move the active cell; the result says so when it did not.
        let expected_move = {
            let modifiers: Vec<&str> = args
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|m| m.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let name = self.inner.def().name.as_str();
            cua_driver_core::sheet_grid::expects_cell_move(
                (name == "press_key")
                    .then(|| args.get("key").and_then(Value::as_str))
                    .flatten(),
                &modifiers,
                (name == "type_text")
                    .then(|| args.get("text").and_then(Value::as_str))
                    .flatten(),
            )
        };

        let mut result = self.inner.invoke(args).await;
        if !action_ran(&result) {
            return result;
        }
        // Only a text or grid check waits for a repaint; new windows are
        // checked right away, so other apps' actions return as fast as before.
        if opaque_window.is_some() || grid_before.is_some() {
            tokio::time::sleep(SETTLE).await;
        }

        let mut findings = Findings {
            ocr_status,
            ..Findings::default()
        };
        if let (Some(window), Some(before)) = (grid_window, grid_before) {
            let mut after = read_grid(pid, window).await;
            let unchanged = |after: &Option<cua_driver_core::sheet_grid::Grid>| {
                after
                    .as_ref()
                    .is_some_and(|a| !cua_driver_core::sheet_grid::diff(&before, a).is_change())
            };
            if unchanged(&after) {
                tokio::time::sleep(GRID_RETRY).await;
                after = read_grid(pid, window).await;
            }
            // No grid read in time (or the window closed): nothing is
            // reported, and the next read of the window decides whether it
            // still shows one.
            if let Some(mut after) = after {
                let focus = tokio::task::spawn_blocking(move || focus_chain(pid, window))
                    .await
                    .ok()
                    .flatten();
                cua_driver_core::sheet_grid::hold_active_while_combo_focused(
                    &mut after,
                    &before,
                    focus.as_deref(),
                );
                let change = cua_driver_core::sheet_grid::diff(&before, &after);
                let mut text = cua_driver_core::sheet_grid::render_change(&change, &after);
                if let Some(note) = cua_driver_core::sheet_grid::render_focus_note(
                    &change,
                    &after,
                    focus.as_deref(),
                    expected_move,
                ) {
                    text.push(' ');
                    text.push_str(&note);
                }
                findings.sheet = Some(SheetFinding {
                    text,
                    changed: change.is_change(),
                });
                crate::sheet::remember(pid, window, after);
            }
        }
        if let Some((window, baseline)) = opaque_window.zip(baseline) {
            let frame = RowFrame::MaxEdge(baseline.width.max(baseline.height));
            match read_rows(window, frame, EVIDENCE_BUDGET).await {
                Ok(after) => {
                    findings.changes = Some(opaque_text::diff_rows(&baseline.rows, &after.rows));
                    crate::ocr::remember_rows(pid, window, after);
                }
                Err(status) => findings.ocr_status = Some(status),
            }
        }
        let after = tokio::task::spawn_blocking(move || app_windows(pid))
            .await
            .unwrap_or_default();
        let default_edge = self
            .state
            .config
            .read()
            .map(|c| {
                self.state
                    .session_config
                    .effective_max_image_dimension(session.as_deref(), &c)
            })
            .unwrap_or(super::DEFAULT_MAX_IMAGE_DIMENSION);
        for window in new_windows(&before, &after) {
            // A window an opaque window opened is drawn by the same toolkit:
            // read its text now (it has no tree to read either).
            let rows = if opaque_window.is_some() {
                crate::ocr::assume_opaque(pid, window.window_id);
                read_rows(
                    window.window_id,
                    RowFrame::MaxEdge(default_edge),
                    EVIDENCE_BUDGET,
                )
                .await
                .ok()
                .map(|text| {
                    self.adopt_frame(pid, &window, &text, session.as_deref());
                    let shown: Vec<_> = text.rows.iter().take(NEW_WINDOW_ROWS).cloned().collect();
                    let mut block: String = shown
                        .iter()
                        .map(|row| format!("  {}\n", opaque_text::render_row(row)))
                        .collect();
                    if text.rows.len() > NEW_WINDOW_ROWS {
                        block.push_str(&format!(
                            "  ({} more rows)\n",
                            text.rows.len() - NEW_WINDOW_ROWS
                        ));
                    }
                    block.trim_end().to_owned()
                })
            } else {
                None
            };
            findings.new_windows.push((window, rows));
        }
        if findings.is_empty() {
            return result;
        }
        let text = findings.render(opaque_window.or(window_id));
        let structured = result
            .structured_content
            .get_or_insert_with(|| Value::Object(Default::default()));
        if let Value::Object(map) = structured {
            findings.apply(map, &text);
        }
        append_text(&mut result, text);
        result
    }
}

impl<T> ActionEvidence<T> {
    /// Make the capture a new window's rows were read off that window's
    /// screenshot frame, and its rows the baseline the next action diffs
    /// against. The rows' x,y are shown to the caller, so a pixel action on
    /// the window must accept them without another read. A window that
    /// already has a snapshot keeps it (its tokens stay valid).
    fn adopt_frame(
        &self,
        pid: i32,
        window: &NewWindow,
        text: &crate::ocr::TextBaseline,
        session: Option<&str>,
    ) {
        let id = u64::from(window.window_id);
        if text.capture_scale <= 0.0 || self.state.snapshots.contains_window(pid, id) {
            return;
        }
        self.state.snapshots.publish_capture_for_session(
            pid,
            id,
            crate::ax::snapshot::AxSnapshot::from_nodes(&[]),
            session,
            Some(text.capture_scale),
        );
        crate::ocr::remember_rows(pid, window.window_id, text.clone());
    }

    /// The frame the caller's pixel actions on `window` use: its latest
    /// screenshot's.
    fn action_frame(&self, args: &Value, pid: i32, window: u32) -> Option<RowFrame> {
        self.state
            .snapshots
            .screenshot_scale(pid, Some(u64::from(window)), args)
            .ok()
            .filter(|scale| *scale > 0.0)
            .map(RowFrame::Downscale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(window_id: u32, z_index: usize, width: f64) -> crate::windows::WindowInfo {
        crate::windows::WindowInfo {
            window_id,
            pid: 42,
            app_name: "App".into(),
            title: format!("w{window_id}"),
            bounds: crate::windows::WindowBounds {
                x: 10.0,
                y: 20.0,
                width,
                height: 200.0,
            },
            layer: 0,
            z_index,
            is_on_screen: true,
            current_space_id: None,
            on_current_space: None,
            space_ids: None,
        }
    }

    #[test]
    fn only_windows_that_appeared_are_new() {
        let before: HashSet<u32> = [1, 2].into_iter().collect();
        let after = [
            window(1, 2, 300.0),
            window(7, 1, 300.0),
            window(9, 0, 300.0),
        ];
        let new = new_windows(&before, &after);
        assert_eq!(
            new.iter().map(|w| w.window_id).collect::<Vec<_>>(),
            vec![9, 7]
        );
        assert_eq!(new[0].bounds, [10, 20, 300, 200]);
        assert_eq!(new[0].title, "w9");
    }

    #[test]
    fn slivers_are_not_new_windows() {
        let new = new_windows(&HashSet::new(), &[window(3, 0, 4.0)]);
        assert!(new.is_empty());
    }

    fn change() -> RowChange {
        RowChange {
            y: 165,
            before: Some("a | n".into()),
            after: Some("a @(10,165) | c @(80,165)".into()),
            ink_only: false,
        }
    }

    #[test]
    fn a_visible_change_confirms_an_unverified_action() {
        let findings = Findings {
            changes: Some(vec![change()]),
            ..Findings::default()
        };
        let text = findings.render(Some(5));
        assert!(text.starts_with("Window 5 rows that changed (1):\ny=165: \"a | n\" -> "));
        let mut map = json!({"effect": "unverifiable"})
            .as_object()
            .cloned()
            .unwrap();
        findings.apply(&mut map, &text);
        assert_eq!(map["effect"], "confirmed");
        assert_eq!(map["evidence"][0]["kind"], "window_change");
        assert_eq!(map["changed_row_count"], 1);
    }

    #[test]
    fn unchanged_rows_mark_a_suspected_noop() {
        let findings = Findings {
            changes: Some(Vec::new()),
            ..Findings::default()
        };
        let text = findings.render(Some(5));
        assert!(text.starts_with("No visible change"));
        let mut map = json!({"effect": "unverifiable"})
            .as_object()
            .cloned()
            .unwrap();
        findings.apply(&mut map, &text);
        assert_eq!(map["effect"], "suspected_noop");
        assert!(map.get("evidence").is_none());
    }

    #[test]
    fn a_stronger_effect_is_kept() {
        let findings = Findings {
            changes: Some(Vec::new()),
            ..Findings::default()
        };
        let mut map = json!({"effect": "confirmed"}).as_object().cloned().unwrap();
        findings.apply(&mut map, "x");
        assert_eq!(map["effect"], "confirmed");
    }

    #[test]
    fn a_new_window_is_named_with_its_rows() {
        let findings = Findings {
            new_windows: vec![(
                NewWindow {
                    window_id: 77,
                    title: "Save \"changes\"".into(),
                    bounds: [1, 2, 300, 120],
                },
                Some("  - OCRRow \"y=10\" = \"Save @(40,10)\"".into()),
            )],
            ..Findings::default()
        };
        let text = findings.render(Some(5));
        assert!(text.contains("New window 77 \"Save 'changes'\" at (1,2) 300x120 pt"));
        assert!(text.contains("OCRRow \"y=10\""));
        assert!(text.contains("pass window_id 77 with x,y from the rows below"));
        assert!(!text.contains("before pixel actions"));
        let mut map = serde_json::Map::new();
        findings.apply(&mut map, &text);
        assert_eq!(map["effect"], "confirmed");
        assert_eq!(map["new_windows"][0]["window_id"], 77);
    }

    #[test]
    fn a_new_window_without_rows_asks_for_a_read() {
        let findings = Findings {
            new_windows: vec![(
                NewWindow {
                    window_id: 78,
                    title: "Open".into(),
                    bounds: [1, 2, 300, 120],
                },
                None,
            )],
            ..Findings::default()
        };
        let text = findings.render(Some(5));
        assert!(text.contains("read it with get_window_state (window_id 78) before pixel actions"));
    }

    #[test]
    fn an_unread_window_reports_why() {
        let findings = Findings {
            ocr_status: Some("timed out".into()),
            ..Findings::default()
        };
        let text = findings.render(Some(5));
        assert_eq!(
            text,
            "Window text after the action was not read: timed out."
        );
        let mut map = json!({"effect": "unverifiable"})
            .as_object()
            .cloned()
            .unwrap();
        findings.apply(&mut map, &text);
        assert_eq!(map["effect"], "unverifiable");
        assert_eq!(map["ocr_status"], "timed out");
    }

    #[test]
    fn a_changed_sheet_confirms_and_drops_the_reread_hint() {
        let findings = Findings {
            sheet: Some(SheetFinding {
                text: "Sheet table \"Sheet Data\" read back after the action: 1 cell(s) \
                       changed: B2 \"\" -> \"12.5\". Active cell B3, empty."
                    .into(),
                changed: true,
            }),
            ..Findings::default()
        };
        let text = findings.render(Some(5));
        assert!(text.starts_with("Sheet table \"Sheet Data\" read back"));
        let mut map = json!({
            "effect": "unverifiable",
            "hint": "Read the field before typing again",
            "next_call": {"tool": "get_window_state"}
        })
        .as_object()
        .cloned()
        .unwrap();
        findings.apply(&mut map, &text);
        assert_eq!(map["effect"], "confirmed");
        assert_eq!(map["evidence"][0]["kind"], "window_change");
        assert!(map.get("hint").is_none());
        assert!(map.get("next_call").is_none());
    }

    #[test]
    fn an_unchanged_sheet_is_reported_without_a_verdict() {
        let findings = Findings {
            sheet: Some(SheetFinding {
                text:
                    "Sheet table \"S\" read back after the action: no visible cell changed value."
                        .into(),
                changed: false,
            }),
            ..Findings::default()
        };
        let text = findings.render(Some(5));
        let mut map = json!({"effect": "unverifiable", "hint": "h"})
            .as_object()
            .cloned()
            .unwrap();
        findings.apply(&mut map, &text);
        assert_eq!(map["effect"], "unverifiable");
        assert_eq!(map["hint"], "h");
        assert!(text.contains("no visible cell changed value"));
    }

    #[test]
    fn findings_join_the_first_text_block() {
        let mut result = ToolResult::text("Clicked at (10,20).");
        append_text(&mut result, "No visible change.".into());
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            Content::Text { text, .. } => {
                assert_eq!(text, "Clicked at (10,20).\nNo visible change.")
            }
            _ => panic!("text"),
        }
        let mut empty = ToolResult::default();
        append_text(&mut empty, "x".into());
        assert_eq!(empty.content.len(), 1);
    }
}
