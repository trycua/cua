// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! AX tree walker: produces the treeMarkdown string and element cache.
//!
//! Format (matching libs/cua-driver exactly):
//!   `INDENT- [N] AXRole "Title" [value="..." actions=[...]]`
//!   `INDENT- AXStaticText = "value"`  (non-indexed)
//!
//! Rules (from cua-driver reference):
//! - An element is addressable (gets an index) when it has ≥1 action name or
//!   exposes a writable AXValue control surface.
//! - Non-actionable leaf nodes with a value are rendered as `AXRole = "value"`.
//! - AXStaticText with no title/value is omitted.
//! - Tree is walked depth-first; element_index is assigned in DFS order.

use super::batch::{NodeAttr, NodeAttributes};
use super::bindings::*;
use super::launch::{is_still_launching, LaunchStep, LaunchWait};
use super::window_scope::{decide_window_scope, ScopeDecision, TopLevelCandidate, WindowScope};
use core_foundation::base::{CFEqual, CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::walk_budget::{WalkBudget, WalkOutcome};
use std::time::Instant;

/// Default maximum depth for AX tree walks. Deep menus and complex web views
/// can nest deeply; 25 covers realistic app chrome without exploding on
/// pathological trees (mirrors Swift reference implementation).
///
/// Callers can override per-call via `walk_tree`'s `max_depth` parameter to
/// trade fidelity for context-window budget on AX-heavy apps (Electron,
/// Obsidian and other large web apps).
pub const DEFAULT_MAX_DEPTH: usize = 25;

/// Default maximum total nodes visited during a single AX walk. Chromium-family
/// apps (Arc, VS Code, Chrome) can expose thousands of nodes; capping at 2 000
/// keeps the walk bounded while still covering realistic app chrome.
/// When the cap is hit the walk stops early and the partial tree is returned
/// with a warning line appended (mirrors Swift reference implementation).
///
/// Callers can override per-call via `walk_tree`'s `max_elements` parameter
/// for large accessibility trees.
pub const DEFAULT_MAX_ELEMENTS: usize = 2_000;

/// Bound each native AX request. Tokio cannot cancel a blocked
/// `AXUIElementCopyAttributeValue` after `spawn_blocking` starts, so the native
/// messaging timeout is what keeps an unresponsive app from retaining a worker
/// indefinitely.
const AX_MESSAGING_TIMEOUT_SECONDS: f32 = 2.0;

unsafe fn set_messaging_timeout(element: AXUIElementRef) {
    let _ = AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECONDS);
}

/// A single node in the AX tree.
#[derive(Debug, Clone)]
pub struct AXNode {
    /// 0-based index (Some = actionable, None = non-actionable display-only node)
    pub element_index: Option<usize>,
    pub role: String,
    /// AXTitle — shown as `"title"` in the tree line.
    pub title: Option<String>,
    /// AXValue — shown as `= "value"` in the tree line.
    pub value: Option<String>,
    /// AXDescription — shown as `(description)` in the tree line.
    /// Kept separate from `title` so `_find_calc_button("2")` can find
    /// Calculator buttons where AXTitle="" but AXDescription="2".
    pub description: Option<String>,
    /// Raw AXPlaceholderValue, kept separate from the value it may also label.
    pub placeholder: Option<String>,
    /// Raw AXSubrole, kept separate from the element's role.
    pub subrole: Option<String>,
    pub identifier: Option<String>,
    pub help: Option<String>,
    pub actions: Vec<String>,
    /// The raw AXUIElementRef pointer value, for caching.
    pub element_ptr: usize,
    /// Depth in the rendered markdown tree (matches the indent level used in
    /// `tree_markdown`). Layout containers AXScrollArea/AXGroup collapse so
    /// children share the parent's depth.
    pub depth: usize,
    /// `element_index` of the nearest actionable ancestor, if any. Walks the
    /// rendered tree (so it skips collapsed layout containers).
    pub parent_element_index: Option<usize>,
    /// Screen-coordinate bounding rect `[x, y, width, height]` captured at
    /// walk time. `None` when AX didn't report a usable position+size.
    pub frame: Option<[f64; 4]>,
    /// AXValue coerced to a string for ALL CF types (CFNumber → "8",
    /// CFBoolean → "1"/"0", CFString as-is). Kept separate from `value`
    /// (string-only) so tree_markdown and the has_content gate — both of
    /// which read `value` — stay byte-identical; only the structured
    /// `elements` array consumes this.
    pub value_state: Option<String>,
    /// AXValueDescription — human-readable value form (e.g. "8 dB").
    pub value_description: Option<String>,
    /// AXMinValue / AXMaxValue for range controls (sliders, steppers).
    pub min_value: Option<f64>,
    pub max_value: Option<f64>,
    /// AXEnabled. `None` when the app doesn't report the attribute.
    pub enabled: Option<bool>,
    /// AXSelected. `None` when the app doesn't report the attribute.
    pub selected: Option<bool>,
    /// True when this node is an AX web-document root or descends from one.
    /// This trust marker is independent of actionable ancestry because
    /// AXWebArea is commonly non-actionable and therefore has no element index.
    pub in_web_content: bool,
}

#[derive(Default)]
struct ControlState {
    value_state: Option<String>,
    value_description: Option<String>,
    min_value: Option<f64>,
    max_value: Option<f64>,
    enabled: Option<bool>,
    selected: Option<bool>,
}

fn read_control_state_if_actionable<F>(is_actionable: bool, read: F) -> ControlState
where
    F: FnOnce() -> ControlState,
{
    if is_actionable {
        read()
    } else {
        ControlState::default()
    }
}

/// Raw AXPlaceholderValue and AXSubrole kept beside the display label.
#[derive(Debug, Default, PartialEq)]
struct IdentityAttrs {
    placeholder: Option<String>,
    subrole: Option<String>,
}

/// When an app refuses the batched read, each attribute is a synchronous AX
/// request against the walk's time budget. Display-only rows never reach the
/// public `elements` array, so they read AXPlaceholderValue only as the
/// empty-AXValue fallback and skip AXSubrole.
fn read_identity_attrs(
    is_actionable: bool,
    value_is_empty: bool,
    mut read: impl FnMut(NodeAttr) -> Option<String>,
) -> IdentityAttrs {
    IdentityAttrs {
        placeholder: if is_actionable || value_is_empty {
            read(NodeAttr::PlaceholderValue)
        } else {
            None
        },
        subrole: if is_actionable {
            read(NodeAttr::Subrole)
        } else {
            None
        },
    }
}

fn role_supports_value_addressing(role: &str) -> bool {
    matches!(
        role,
        "AXTextField"
            | "AXTextArea"
            | "AXComboBox"
            | "AXSlider"
            | "AXStepper"
            | "AXCheckBox"
            | "AXRadioButton"
    )
}

fn is_addressable(actions_present: bool, value_settable: bool) -> bool {
    actions_present || value_settable
}

pub struct TreeWalkResult {
    pub tree_markdown: String,
    pub nodes: Vec<AXNode>,
    /// True when the walk was cut short by its node or time budget.
    pub truncated: bool,
    /// True when nodes past `max_depth` were omitted, independently of budget `truncated`.
    pub depth_truncated: bool,
    /// Why and where the walk stopped (see [`cua_driver_core::walk_budget`]).
    pub walk: WalkOutcome,
    /// Whether the requested `window_id` actually resolved to an AX surface,
    /// and if not, why. `None` when no `window_id` was requested.
    ///
    /// Issue #2237: without this, an unresolvable id was indistinguishable
    /// from a clean snapshot — callers had no way to tell that the tree they
    /// were handed belonged to a different surface. Any variant other than
    /// [`WindowScope::Matched`] comes with an EMPTY walk, so `nodes` never
    /// describes a window other than the requested one.
    pub window_scope: Option<WindowScope>,
}

/// Walk the AX tree of `pid`, optionally filtered to a specific window.
///
/// `window_id` — when Some, only the AXWindow matching that CGWindowID is
/// walked (plus non-window children like the menu bar). When None, all
/// top-level children are walked.
///
/// Key background-app fix: at the application root we union `AXChildren`
/// and `AXWindows`. macOS only puts windows in `AXChildren` when the app
/// is frontmost; `AXWindows` returns the window list regardless of focus
/// state. Without this union, Safari / any backgrounded app returns an
/// empty tree.
///
/// # Safety
/// Calls macOS AX API. Must be called on a thread that has a CF run loop.
pub fn walk_tree(pid: i32, window_id: Option<u32>, query: Option<&str>) -> TreeWalkResult {
    walk_tree_bounded(
        pid,
        window_id,
        query,
        DEFAULT_MAX_ELEMENTS,
        DEFAULT_MAX_DEPTH,
    )
}

/// Walk the AX tree with caller-supplied caps. See [`walk_tree`] for the
/// common case (defaults apply). `max_elements`/`max_depth` clamp the
/// rendered tree breadth-wise (DFS truncated when the element counter hits
/// the cap) and depth-wise (nodes whose markdown indent would exceed the cap
/// are omitted). Markdown and the `nodes` vec are truncated identically.
///
/// These caps protect against Electron / Obsidian / large web apps
/// that produce 10k+ element trees and blow context windows.
pub fn walk_tree_bounded(
    pid: i32,
    window_id: Option<u32>,
    query: Option<&str>,
    max_elements: usize,
    max_depth: usize,
) -> TreeWalkResult {
    walk_tree_budgeted(
        pid,
        window_id,
        query,
        max_depth,
        WalkBudget::nodes_only(max_elements),
    )
}

pub(crate) fn walk_native_chrome_bounded(
    pid: i32,
    window_id: u32,
    max_elements: usize,
) -> TreeWalkResult {
    walk_tree_with_web_content(
        pid,
        Some(window_id),
        None,
        DEFAULT_MAX_DEPTH,
        WalkBudget::nodes_only(max_elements),
        false,
    )
}

/// [`walk_tree_bounded`] under a caller's [`WalkBudget`]: the walk also stops
/// when the budget's `timeout_ms` runs out, returning the partial tree. Each
/// AX call is bounded by the per-element messaging timeout, so the walk
/// overruns the deadline by at most one attribute read.
pub fn walk_tree_budgeted(
    pid: i32,
    window_id: Option<u32>,
    query: Option<&str>,
    max_depth: usize,
    budget: WalkBudget,
) -> TreeWalkResult {
    walk_tree_with_web_content(pid, window_id, query, max_depth, budget, true)
}

fn walk_tree_with_web_content(
    pid: i32,
    window_id: Option<u32>,
    query: Option<&str>,
    max_depth: usize,
    mut budget: WalkBudget,
    include_web_content: bool,
) -> TreeWalkResult {
    let mut nodes: Vec<AXNode> = Vec::new();
    let mut lines: Vec<(usize, String)> = Vec::new(); // (depth, line)
    let mut index_counter = 0usize;
    let mut depth_truncated = false;
    let mut window_scope: Option<WindowScope> = None;

    unsafe {
        let app_elem = AXUIElementCreateApplication(pid);
        if app_elem.is_null() {
            return TreeWalkResult {
                tree_markdown: String::new(),
                nodes,
                truncated: false,
                depth_truncated: false,
                walk: budget.outcome(),
                // No application AX element at all, so a requested window
                // certainly did not resolve.
                window_scope: window_id.map(|_| WindowScope::AxUnresolved {
                    ax_window_count: 0,
                    ax_windows_error: None,
                }),
            };
        }
        set_messaging_timeout(app_elem);

        // A window-scoped walk of an app that is still launching waits, within
        // the caller's budget, for the app to answer AX (see `super::launch`).
        let mut launch_wait = LaunchWait::new(budget.time_limit());
        let (top_level, walk_these) = loop {
            // Chromium/Electron apps (Arc, VS Code, Electron shells) ship their
            // web-content AX tree OFF and only build it once an assistive client
            // asks for it. Without this, the first walk of such an app returns an
            // empty/title-bar-only tree (#1616). Flip the enablement attribute,
            // then — only when the flip actually took and only the first time we
            // see this process lifetime — let the asynchronously-built tree settle
            // before we read it. Native Cocoa apps reject the attribute, so they
            // pay no settle cost. This relies on the MAX_ELEMENTS node cap to keep
            // the now-materialized (potentially large) tree bounded. An app that
            // has not started answering AX rejects the flip without caching the
            // refusal, so each launch-wait attempt asks again.
            let attempt_started = Instant::now();
            super::enablement::ensure_chromium_ax_enabled(pid, app_elem);

            let (mut top_level, ax_windows_error) = copy_top_level(app_elem, pid, window_id);

            // Scope: keep non-window children (menu bar) + the target window —
            // but ONLY once the target window has actually been identified. When
            // nothing claims the requested id, `decide_window_scope` reports why
            // and walks nothing; it must never fall back to "everything that isn't
            // a window", which is how issue #2237 returned menu bars as panels.
            let Some(wid) = window_id else {
                let walk = top_level.clone();
                break (top_level, walk);
            };
            let decision = scope_top_level(&mut top_level, pid, wid, ax_windows_error);
            if matches!(decision.scope, WindowScope::AxUnresolved { .. }) {
                match launch_wait.step(attempt_started, Instant::now(), || is_still_launching(pid))
                {
                    LaunchStep::Retry(pause) => {
                        release_all(top_level);
                        std::thread::sleep(pause);
                        continue;
                    }
                    // Still unresolved because the app never finished
                    // launching: say so instead of blaming the window scope.
                    LaunchStep::TimedOut => budget.stop_for_app_lookup_timeout(),
                    LaunchStep::Proceed => {}
                }
            }
            let walk = decision
                .walk
                .iter()
                .map(|&index| top_level[index])
                .collect();
            window_scope = Some(decision.scope);
            break (top_level, walk);
        };

        // Walk each top-level child at depth 0.
        for child in walk_these {
            walk_element(
                child,
                0,
                None,
                false,
                &mut nodes,
                &mut lines,
                &mut index_counter,
                &mut budget,
                &mut depth_truncated,
                max_depth,
                include_web_content,
            );
        }

        // Release all top-level elements (copy_children / copy_ax_windows both retain).
        release_all(top_level);

        CFRelease(app_elem as CFTypeRef);
    }

    let walk = budget.outcome();
    prune_echo_text_rows(&mut lines);
    let raw_markdown = render_lines(&lines);
    let mut tree_markdown = if let Some(q) = query {
        filter_tree(&raw_markdown, q)
    } else {
        raw_markdown
    };

    if let Some(note) = walk.note() {
        tree_markdown.push('\n');
        tree_markdown.push_str(&note);
    }

    TreeWalkResult {
        tree_markdown,
        nodes,
        truncated: walk.truncated(),
        depth_truncated,
        walk,
        window_scope,
    }
}

/// The application's top-level AX children: `AXChildren` ∪ `AXWindows`.
///
/// `AXChildren` omits windows when the app isn't frontmost (AppKit
/// limitation); `AXWindows` returns the window list regardless of activation
/// state, so the union is the only way to see background windows. Every
/// returned element is retained; release them with [`release_all`].
unsafe fn copy_top_level(
    app_elem: AXUIElementRef,
    pid: i32,
    window_id: Option<u32>,
) -> (Vec<AXUIElementRef>, Option<AXError>) {
    let mut top_level = copy_children(app_elem);
    // A requested window on another Space is absent from AXWindows; the
    // `_including` variant recovers it by exact CGWindowID.
    let (from_windows, ax_windows_error) = match window_id {
        Some(wid) => copy_ax_windows_including_or_error(app_elem, pid, wid),
        None => (copy_ax_windows(app_elem), None),
    };
    for w in from_windows {
        // AXChildren and AXWindows can return different proxy pointers for
        // the same native window. CFEqual compares their AX identity;
        // pointer equality alone duplicates the whole subtree and can turn
        // one exact dialog action into a false ambiguity.
        if !top_level
            .iter()
            .any(|&e| CFEqual(e as CFTypeRef, w as CFTypeRef) != 0)
        {
            top_level.push(w);
        } else {
            // Already present — release the extra retain from copy_ax_windows.
            CFRelease(w as CFTypeRef);
        }
    }
    (top_level, ax_windows_error)
}

/// Decide which of `top_level` a walk scoped to `wid` covers.
///
/// A sheet's own id is never a top-level window: the sheet hangs off the
/// window it is attached to. When no window claims `wid`, the matching
/// attached sheet is appended to `top_level`, so the walk covers that sheet
/// and not the parent window's own controls.
unsafe fn scope_top_level(
    top_level: &mut Vec<AXUIElementRef>,
    pid: i32,
    wid: u32,
    ax_windows_error: Option<AXError>,
) -> ScopeDecision {
    let mut candidates: Vec<TopLevelCandidate> = top_level
        .iter()
        .map(|&child| top_level_candidate(child))
        .collect();
    if !candidates
        .iter()
        .any(|candidate| candidate.ax_window_id == Some(wid))
    {
        let windows: Vec<AXUIElementRef> = top_level
            .iter()
            .zip(&candidates)
            .filter(|(_, candidate)| candidate.role == "AXWindow")
            .map(|(&window, _)| window)
            .collect();
        for sheet in windows
            .into_iter()
            .flat_map(|window| super::exact_target::attached_sheets(window))
        {
            let candidate = top_level_candidate(sheet);
            if candidate.ax_window_id == Some(wid)
                && !candidates.iter().any(|c| c.ax_window_id == Some(wid))
            {
                top_level.push(sheet);
                candidates.push(candidate);
            } else {
                CFRelease(sheet as CFTypeRef);
            }
        }
    }
    decide_window_scope(&candidates, wid, ax_windows_error, || {
        crate::windows::resolve_window_owner(pid, wid)
    })
}

unsafe fn top_level_candidate(child: AXUIElementRef) -> TopLevelCandidate {
    set_messaging_timeout(child);
    let role = copy_string_attr(child, "AXRole").unwrap_or_default();
    let subrole = copy_string_attr(child, "AXSubrole");
    let identifier = copy_string_attr(child, "AXIdentifier");
    // Match AX window element → CGWindowID via private SPI. Only windows and
    // sheets carry one, so skip the round-trip elsewhere.
    let ax_window_id = if matches!(role.as_str(), "AXWindow" | "AXSheet") {
        ax_get_window_id(child)
    } else {
        None
    };
    TopLevelCandidate {
        role,
        subrole,
        identifier,
        ax_window_id,
    }
}

/// Release elements retained by [`copy_top_level`].
unsafe fn release_all(elements: Vec<AXUIElementRef>) {
    for element in elements {
        CFRelease(element as CFTypeRef);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn walk_element(
    element: AXUIElementRef,
    depth: usize,
    parent_index: Option<usize>,
    in_web_content: bool,
    nodes: &mut Vec<AXNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    budget: &mut WalkBudget,
    depth_truncated: &mut bool,
    max_depth: usize,
    include_web_content: bool,
) {
    if depth > max_depth {
        *depth_truncated = true;
        return;
    }
    // Node and time budget: a refused node is counted as discovered but not
    // visited, and the walk unwinds with the partial tree it has.
    if !budget.admit() {
        return;
    }

    // Messaging timeouts are per AX object, not inherited from the application
    // element, so every descendant must be bounded before any attribute read.
    set_messaging_timeout(element);

    // One round trip for every attribute this node can need (see
    // `super::batch`); the reads below only convert the reply.
    let attrs = NodeAttributes::read(element);
    let role = attrs
        .string(NodeAttr::Role)
        .unwrap_or_else(|| "AXUnknown".into());

    let in_web_content = in_web_content || is_web_content_role(&role);
    if in_web_content && !include_web_content {
        return;
    }

    // Skip pure layout containers that have no interesting content.
    if role == "AXScrollArea" || role == "AXGroup" {
        // Still recurse — children may be interesting. Layout containers
        // collapse, so children inherit the parent's depth AND the same
        // parent_index (no actionable node was emitted here).
        let children = attrs.children();
        drop(attrs);
        for child in children {
            walk_element(
                child,
                depth,
                parent_index,
                in_web_content,
                nodes,
                lines,
                counter,
                budget,
                depth_truncated,
                max_depth,
                include_web_content,
            );
            CFRelease(child as CFTypeRef);
        }
        return;
    }

    // Keep AXTitle and AXDescription SEPARATE so that the tree format matches
    // the Swift reference: title → "title", description → (description).
    // This is critical for Calculator where AXTitle="" but AXDescription="2"
    // (digit buttons). Merging them would produce "2" (quoted) instead of (2)
    // (parens), breaking _find_calc_button which searches for "(2)".
    let title = attrs.string(NodeAttr::Title);
    // Read AXValue once with enough type information to preserve the existing
    // string-only markdown while also exposing numeric/boolean control state.
    let copied_value = attrs.stringish(NodeAttr::Value);
    let value = copied_value
        .as_ref()
        .and_then(|copied| copied.string_value.clone());
    let value = value.filter(|v| !v.trim().is_empty());
    let description = attrs.string(NodeAttr::Description);
    let identifier = attrs.string(NodeAttr::Identifier);
    let help = attrs
        .string(NodeAttr::Help)
        .filter(|h| !h.trim().is_empty());
    let actions = copy_action_names(element);

    // Some native controls expose no AX action names but do expose a writable
    // AXValue. Finder's transient inline-rename field is the important case:
    // rendering it without an element_index leaves an agent able to see the
    // field but unable to call set_value on it. Probe writability only for the
    // small family of value controls so arbitrary display nodes do not pay an
    // extra AX round trip.
    let value_settable = actions.is_empty()
        && role_supports_value_addressing(&role)
        && is_attribute_settable(element, "AXValue");
    // A disabled control is still indexed and published with `enabled: false`:
    // enablement is state the caller needs to read (a form's Save button, a
    // toolbar item in a window that is not key), not a reason to hide the row.
    // click, right_click, double_click, set_value and type_text re-read
    // AXEnabled before dispatch and refuse a disabled token target with
    // `element_disabled` (`tools::refuse_if_disabled`).
    let enabled = if !actions.is_empty() || value_settable {
        attrs.bool(NodeAttr::Enabled)
    } else {
        None
    };
    let is_actionable = is_addressable(!actions.is_empty(), value_settable);
    let identity = read_identity_attrs(is_actionable, value.is_none(), |attr| attrs.string(attr));
    let value = value.or_else(|| identity.placeholder.clone());

    let visible_title = title.as_deref().unwrap_or("").trim().to_owned();
    let visible_description = description.as_deref().unwrap_or("").trim().to_owned();
    let visible_value = value.as_deref().unwrap_or("").trim().to_owned();

    let has_content =
        !visible_title.is_empty() || !visible_description.is_empty() || !visible_value.is_empty();

    if !is_actionable && !has_content && role != "AXWindow" && role != "AXSheet" {
        let children = attrs.children();
        drop(attrs);
        for child in children {
            walk_element(
                child,
                depth + 1,
                parent_index,
                in_web_content,
                nodes,
                lines,
                counter,
                budget,
                depth_truncated,
                max_depth,
                include_web_content,
            );
            CFRelease(child as CFTypeRef);
        }
        return;
    }

    let element_ptr = element as usize;
    let frame = attrs.screen_rect();
    // Structured `elements` only contains actionable nodes. Keep all new AX
    // round-trips behind that same gate so display-only rows pay no cost.
    let control_state = read_control_state_if_actionable(is_actionable, || ControlState {
        value_state: copied_value
            .map(|copied| copied.state_value)
            .filter(|v| !v.trim().is_empty())
            .or_else(|| value.clone())
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty()),
        value_description: attrs
            .string(NodeAttr::ValueDescription)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty()),
        min_value: attrs.number(NodeAttr::MinValue),
        max_value: attrs.number(NodeAttr::MaxValue),
        enabled,
        selected: attrs.bool(NodeAttr::Selected),
    });
    let node = if is_actionable {
        let idx = *counter;
        *counter += 1;
        // Retain so the element stays alive in the cache after `copy_children`
        // releases the per-child ref at the end of the caller's loop.
        CFRetain(element as CFTypeRef);
        AXNode {
            element_index: Some(idx),
            role: role.clone(),
            title: if visible_title.is_empty() {
                None
            } else {
                Some(visible_title.clone())
            },
            value: if visible_value.is_empty() {
                None
            } else {
                Some(visible_value.clone())
            },
            description: if visible_description.is_empty() {
                None
            } else {
                Some(visible_description.clone())
            },
            placeholder: identity.placeholder,
            subrole: identity.subrole,
            identifier: identifier.clone(),
            help: help.clone(),
            actions: actions.clone(),
            element_ptr,
            depth,
            parent_element_index: parent_index,
            frame,
            value_state: control_state.value_state.clone(),
            value_description: control_state.value_description.clone(),
            min_value: control_state.min_value,
            max_value: control_state.max_value,
            enabled: control_state.enabled,
            selected: control_state.selected,
            in_web_content,
        }
    } else {
        AXNode {
            element_index: None,
            role: role.clone(),
            title: if visible_title.is_empty() {
                None
            } else {
                Some(visible_title.clone())
            },
            value: if visible_value.is_empty() {
                None
            } else {
                Some(visible_value.clone())
            },
            description: if visible_description.is_empty() {
                None
            } else {
                Some(visible_description.clone())
            },
            placeholder: identity.placeholder,
            subrole: identity.subrole,
            identifier: identifier.clone(),
            help: help.clone(),
            actions: vec![],
            element_ptr,
            depth,
            parent_element_index: parent_index,
            frame,
            value_state: control_state.value_state.clone(),
            value_description: control_state.value_description.clone(),
            min_value: control_state.min_value,
            max_value: control_state.max_value,
            enabled: control_state.enabled,
            selected: control_state.selected,
            in_web_content,
        }
    };

    // Track this node as the parent for its descendants only when it was
    // assigned an element_index (mirrors what the markdown shows: only
    // indexed rows are addressable in click(element_index=N)).
    let next_parent = node.element_index.or(parent_index);

    // A closed menu of the menu bar stays collapsed when the caller asked for
    // it: its items are reachable through invoke_menu or a query, and walking
    // them cost most of a read (94% of a Chrome window read in a VM check)
    // and ate the node budget before the window content.
    let collapse_here = role == "AXMenuBarItem"
        && MENUS_COLLAPSED.with(std::cell::Cell::get)
        && attrs.bool(NodeAttr::Selected) != Some(true);
    let line = if role == "AXMenuBar" && MENUS_COLLAPSED.with(std::cell::Cell::get) {
        format!("{} {COLLAPSED_MENU_NOTE}", format_node_line(&node))
    } else {
        format_node_line(&node)
    };
    lines.push((depth, line));
    nodes.push(node);
    if collapse_here {
        return;
    }

    let children = attrs.children();
    drop(attrs);
    for child in children {
        walk_element(
            child,
            depth + 1,
            next_parent,
            in_web_content,
            nodes,
            lines,
            counter,
            budget,
            depth_truncated,
            max_depth,
            include_web_content,
        );
        CFRelease(child as CFTypeRef);
    }
}

fn is_web_content_role(role: &str) -> bool {
    let normalized = role
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("webarea") || normalized.contains("documentweb") || normalized == "document"
}

#[cfg(test)]
mod web_content_role_tests {
    use super::is_web_content_role;

    #[test]
    fn recognizes_native_web_document_roles_without_marking_app_chrome() {
        for role in ["AXWebArea", "AXDocumentWeb", "document"] {
            assert!(is_web_content_role(role), "{role} must start web trust");
        }
        for role in ["AXWindow", "AXButton", "AXToolbar"] {
            assert!(!is_web_content_role(role), "{role} stays native");
        }
    }
}

fn format_node_line(node: &AXNode) -> String {
    let mut parts = String::new();

    // Common prefix (with or without index).
    if let Some(idx) = node.element_index {
        parts.push_str(&format!("- [{}] {}", idx, node.role));
    } else {
        parts.push_str(&format!("- {}", node.role));
    }

    // AXTitle → "title"
    if let Some(t) = &node.title {
        parts.push_str(&format!(" \"{}\"", t));
    }
    // AXValue → = "value"
    if let Some(v) = &node.value {
        parts.push_str(&format!(" = \"{}\"", v));
    }
    // AXDescription → (description) — critical for Calculator digit buttons
    // where AXTitle="" but AXDescription="2". A description that only repeats
    // the title (Chrome's `"Back" (Back)`) is not shown twice.
    if let Some(d) = node
        .description
        .as_ref()
        .filter(|d| node.title.as_ref() != Some(*d))
    {
        parts.push_str(&format!(" ({})", d));
    }

    // Bracketed metadata block (identifier, help, actions).
    if node.element_index.is_some() {
        let mut attrs: Vec<String> = Vec::new();
        if let Some(id) = node
            .identifier
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            attrs.push(format!("id={}", id));
        }
        if let Some(h) = &node.help {
            attrs.push(format!("help=\"{}\"", h));
        }
        if node.enabled == Some(false) {
            attrs.push("enabled=false".to_owned());
        }
        let shown: Vec<String> = node
            .actions
            .iter()
            .filter(|a| !UNLISTED_ACTIONS.contains(&a.as_str()))
            .map(|a| a.strip_prefix("AX").unwrap_or(a).to_lowercase())
            .collect();
        if !shown.is_empty() {
            attrs.push(format!("actions=[{}]", shown.join(",")));
        }
        if !attrs.is_empty() {
            parts.push_str(" [");
            parts.push_str(&attrs.join(" "));
            parts.push(']');
        }
    }

    parts
}

thread_local! {
    static MENUS_COLLAPSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Shown on the menu bar row of a read whose menus were not walked.
pub(crate) const COLLAPSED_MENU_NOTE: &str =
    "(menus collapsed: run an item with invoke_menu path, or pass query to search menu items)";

/// Run `walk` with the menu bar's closed menus collapsed (or not) on this
/// thread. Only `get_window_state` collapses; every other walker keeps the
/// full menu tree.
pub fn with_menus_collapsed<T>(collapse: bool, walk: impl FnOnce() -> T) -> T {
    let previous = MENUS_COLLAPSED.with(|flag| flag.replace(collapse));
    let result = walk();
    MENUS_COLLAPSED.with(|flag| flag.set(previous));
    result
}

/// Actions every web-content element advertises. Listing them on each row
/// cost about a third of a web tree's text and told the caller nothing; the
/// structured `elements` still carry the full list.
const UNLISTED_ACTIONS: &[&str] = &["AXShowMenu", "AXScrollToVisible"];

/// Drop static-text leaf rows that only repeat text already shown on their
/// parent row (a web button "Save case" with a child text "Save case"). The
/// rows keep their element indices; only the rendering omits them, and a
/// `query` for the text still matches the parent.
fn prune_echo_text_rows(lines: &mut Vec<(usize, String)>) {
    let mut keep = vec![true; lines.len()];
    for i in 0..lines.len() {
        let (depth, line) = &lines[i];
        let Some(text) = echo_text_of(line) else {
            continue;
        };
        let is_leaf = lines.get(i + 1).is_none_or(|(next, _)| next <= depth);
        if !is_leaf || *depth == 0 {
            continue;
        }
        let parent = lines[..i].iter().rev().find(|(d, _)| *d + 1 == *depth);
        if parent.is_some_and(|(_, parent_line)| quoted_text_contains(parent_line, text)) {
            keep[i] = false;
        }
    }
    let mut index = 0;
    lines.retain(|_| {
        let kept = keep[index];
        index += 1;
        kept
    });
}

/// The text of a bare static-text row: `- [N] AXStaticText = "text"` with
/// nothing else on the row.
fn echo_text_of(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("- ")?;
    let rest = match rest.strip_prefix('[') {
        Some(after) => after.split_once("] ")?.1,
        None => rest,
    };
    let text = rest.strip_prefix("AXStaticText = \"")?.strip_suffix('"')?;
    (!text.is_empty() && !text.contains('"')).then_some(text)
}

/// Whether one of the double-quoted strings on `line` contains `text`.
fn quoted_text_contains(line: &str, text: &str) -> bool {
    line.split('"')
        .skip(1)
        .step_by(2)
        .any(|quoted| quoted.contains(text))
}

fn render_lines(lines: &[(usize, String)]) -> String {
    let mut out = String::new();
    for (depth, line) in lines {
        for _ in 0..*depth {
            out.push_str("  ");
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Filter the tree markdown to lines matching `query` plus their ancestor chain.
fn filter_tree(markdown: &str, query: &str) -> String {
    let needle = query.to_lowercase();
    let lines: Vec<&str> = markdown.lines().collect();

    let mut current_ancestor: Vec<&str> = Vec::new();
    let mut last_emitted_at: Vec<Option<&str>> = Vec::new();
    let mut output: Vec<&str> = Vec::new();

    for line in &lines {
        let depth = leading_indent_depth(line);

        while current_ancestor.len() <= depth {
            current_ancestor.push("");
            last_emitted_at.push(None);
        }
        for emitted_at in last_emitted_at.iter_mut().skip(depth + 1) {
            *emitted_at = None;
        }
        current_ancestor[depth] = line;

        if line.to_lowercase().contains(&needle) {
            for ancestor_depth in 0..depth {
                let ancestor = current_ancestor[ancestor_depth];
                if ancestor.is_empty() {
                    continue;
                }
                if last_emitted_at[ancestor_depth] == Some(ancestor) {
                    continue;
                }
                last_emitted_at[ancestor_depth] = Some(ancestor);
                output.push(ancestor);
            }
            last_emitted_at[depth] = Some(line);
            output.push(line);
        }
    }

    if output.is_empty() {
        return String::new();
    }
    let mut result = output.join("\n");
    result.push('\n');
    result
}

fn leading_indent_depth(line: &str) -> usize {
    let mut count = 0;
    for ch in line.chars() {
        if ch == ' ' {
            count += 1;
        } else {
            break;
        }
    }
    count / 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_disabled_control_row_says_so() {
        let node = AXNode {
            element_index: Some(7),
            role: "AXCheckBox".into(),
            title: Some("Reset".into()),
            value: None,
            description: None,
            placeholder: None,
            subrole: None,
            identifier: None,
            help: None,
            actions: vec!["AXPress".into()],
            element_ptr: 0,
            depth: 2,
            parent_element_index: None,
            frame: None,
            value_state: None,
            value_description: None,
            min_value: None,
            max_value: None,
            enabled: Some(false),
            selected: None,
            in_web_content: false,
        };
        assert_eq!(
            format_node_line(&node),
            "- [7] AXCheckBox \"Reset\" [enabled=false actions=[press]]"
        );
    }

    #[test]
    fn writable_value_controls_are_addressable_without_actions() {
        assert!(is_addressable(false, true));
        assert!(is_addressable(true, false));
        assert!(!is_addressable(false, false));

        for role in [
            "AXTextField",
            "AXTextArea",
            "AXComboBox",
            "AXSlider",
            "AXStepper",
            "AXCheckBox",
            "AXRadioButton",
        ] {
            assert!(role_supports_value_addressing(role), "{role}");
        }
        for role in ["AXStaticText", "AXImage", "AXWindow", "AXGroup"] {
            assert!(!role_supports_value_addressing(role), "{role}");
        }
    }

    #[test]
    fn control_state_reads_are_gated_by_actionability() {
        let reads = Cell::new(0);
        let display_only = read_control_state_if_actionable(false, || {
            reads.set(reads.get() + 1);
            ControlState {
                enabled: Some(true),
                ..ControlState::default()
            }
        });
        assert_eq!(reads.get(), 0, "display-only nodes must not read state");
        assert_eq!(display_only.enabled, None);

        let actionable = read_control_state_if_actionable(true, || {
            reads.set(reads.get() + 1);
            ControlState {
                enabled: Some(true),
                ..ControlState::default()
            }
        });
        assert_eq!(reads.get(), 1, "actionable nodes must read state once");
        assert_eq!(actionable.enabled, Some(true));
    }

    fn identity_reads(is_actionable: bool, value_is_empty: bool) -> (Vec<NodeAttr>, IdentityAttrs) {
        let mut reads = Vec::new();
        let identity = read_identity_attrs(is_actionable, value_is_empty, |attr| {
            reads.push(attr);
            Some(format!("raw {}", attr.name()))
        });
        (reads, identity)
    }

    #[test]
    fn display_only_nodes_skip_identity_reads_unless_placeholder_is_the_value() {
        let (reads, identity) = identity_reads(false, false);
        assert!(reads.is_empty(), "display row with a value read {reads:?}");
        assert_eq!(identity, IdentityAttrs::default());

        let (reads, identity) = identity_reads(false, true);
        assert_eq!(reads, [NodeAttr::PlaceholderValue]);
        assert_eq!(
            identity.placeholder.as_deref(),
            Some("raw AXPlaceholderValue")
        );
        assert_eq!(identity.subrole, None);
    }

    #[test]
    fn actionable_nodes_read_placeholder_and_subrole_once() {
        for value_is_empty in [false, true] {
            let (reads, identity) = identity_reads(true, value_is_empty);
            assert_eq!(reads, [NodeAttr::PlaceholderValue, NodeAttr::Subrole]);
            assert_eq!(
                identity.placeholder.as_deref(),
                Some("raw AXPlaceholderValue")
            );
            assert_eq!(identity.subrole.as_deref(), Some("raw AXSubrole"));
        }
    }
}

#[cfg(test)]
mod lean_row_tests {
    use super::{echo_text_of, prune_echo_text_rows};

    fn rows(lines: &[(usize, &str)]) -> Vec<(usize, String)> {
        lines.iter().map(|(d, l)| (*d, l.to_string())).collect()
    }

    #[test]
    fn echo_text_rows_under_a_labelled_parent_are_dropped() {
        let mut lines = rows(&[
            (0, "- [0] AXWindow \"Mail\" [actions=[raise]]"),
            (
                1,
                "- [8] AXButton \"Atrium plant service CM-199\" [actions=[press]]",
            ),
            (2, "- [9] AXStaticText = \"Atrium plant service\""),
            (2, "- [10] AXStaticText = \"CM-199\""),
            (1, "- [11] AXHeading \"Messages\""),
            (2, "- [12] AXStaticText = \"Inbox\""),
            (1, "- [13] AXStaticText = \"Mail\""),
        ]);
        prune_echo_text_rows(&mut lines);
        let kept: Vec<&str> = lines.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            kept,
            [
                "- [0] AXWindow \"Mail\" [actions=[raise]]",
                "- [8] AXButton \"Atrium plant service CM-199\" [actions=[press]]",
                "- [11] AXHeading \"Messages\"",
                "- [12] AXStaticText = \"Inbox\"",
            ]
        );
    }

    #[test]
    fn menu_collapsing_is_scoped_to_the_wrapped_walk() {
        use super::{with_menus_collapsed, MENUS_COLLAPSED};
        assert!(!MENUS_COLLAPSED.with(std::cell::Cell::get));
        let inside = with_menus_collapsed(true, || {
            let nested = with_menus_collapsed(false, || MENUS_COLLAPSED.with(std::cell::Cell::get));
            (MENUS_COLLAPSED.with(std::cell::Cell::get), nested)
        });
        assert_eq!(inside, (true, false));
        assert!(!MENUS_COLLAPSED.with(std::cell::Cell::get));
    }

    #[test]
    fn rows_with_children_or_other_fields_are_kept() {
        let mut lines = rows(&[
            (0, "- [1] AXLink \"Docs\""),
            (1, "- [2] AXStaticText = \"Docs\" [actions=[press]]"),
            (1, "- [3] AXStaticText = \"Docs\""),
            (2, "- [4] AXStaticText = \"child\""),
        ]);
        prune_echo_text_rows(&mut lines);
        assert_eq!(lines.len(), 4);
        assert_eq!(echo_text_of("- AXStaticText = \"x\""), Some("x"));
        assert_eq!(echo_text_of("- [2] AXStaticText = \"\""), None);
    }
}
