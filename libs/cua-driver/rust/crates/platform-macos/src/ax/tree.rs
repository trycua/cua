//! AX tree walker: produces the treeMarkdown string and element cache.
//!
//! Format (matching libs/cua-driver exactly):
//!   `INDENT- [N] AXRole "Title" [value="..." actions=[...]]`
//!   `INDENT- AXStaticText = "value"`  (non-indexed)
//!
//! Rules (from cua-driver reference):
//! - An element is "actionable" (gets an index) when it has ≥1 action name.
//! - Non-actionable leaf nodes with a value are rendered as `AXRole = "value"`.
//! - AXStaticText with no title/value is omitted.
//! - Tree is walked depth-first; element_index is assigned in DFS order.

use super::bindings::*;
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// Default maximum depth for AX tree walks. Deep menus and complex web views
/// can nest deeply; 25 covers realistic app chrome without exploding on
/// pathological trees (mirrors Swift reference implementation).
///
/// Callers can override per-call via `walk_tree`'s `max_depth` parameter to
/// trade fidelity for context-window budget on AX-heavy apps (Electron,
/// Obsidian, large web apps — issue #22865).
pub const DEFAULT_MAX_DEPTH: usize = 25;

/// Default maximum total nodes visited during a single AX walk. Chromium-family
/// apps (Arc, VS Code, Chrome) can expose thousands of nodes; capping at 2 000
/// keeps the walk bounded while still covering realistic app chrome.
/// When the cap is hit the walk stops early and the partial tree is returned
/// with a warning line appended (mirrors Swift reference implementation).
///
/// Callers can override per-call via `walk_tree`'s `max_elements` parameter
/// (issue #22865).
pub const DEFAULT_MAX_ELEMENTS: usize = 2_000;

/// How long to let a freshly-enabled Chromium/Electron app build its
/// web-content AX tree before we read it. The tree is materialized
/// asynchronously over IPC once the app detects an assistive client, so a
/// walk that starts immediately sees only the chrome (title bar, a handful
/// of elements). This settle is paid at most once per pid — see
/// `enabled_pids`.
const CHROMIUM_SETTLE_SECONDS: f64 = 0.5;

/// Pids for which we have already flipped on accessibility and paid the
/// one-time settle delay. Repeat snapshots of the same app skip the settle:
/// the tree is already built and stays built for the life of the process.
fn enabled_pids() -> &'static Mutex<HashSet<i32>> {
    static ENABLED_PIDS: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();
    ENABLED_PIDS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A single node in the AX tree.
#[derive(Debug, Clone)]
pub struct AXNode {
    /// 0-based addressable index. Native walks index actionable nodes; Codex
    /// compatibility walks also index meaningful text and containers.
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
}

pub struct TreeWalkResult {
    pub tree_markdown: String,
    pub nodes: Vec<AXNode>,
    /// True when the walk was cut short by the MAX_ELEMENTS cap.
    pub truncated: bool,
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
    walk_tree_bounded(pid, window_id, query, DEFAULT_MAX_ELEMENTS, DEFAULT_MAX_DEPTH)
}

/// Walk the AX tree with caller-supplied caps. See [`walk_tree`] for the
/// common case (defaults apply). `max_elements`/`max_depth` clamp the
/// rendered tree breadth-wise (DFS truncated when the element counter hits
/// the cap) and depth-wise (nodes whose markdown indent would exceed the cap
/// are omitted). Markdown and the `nodes` vec are truncated identically.
///
/// Issue #22865: caps protect against Electron / Obsidian / large web apps
/// that produce 10k+ element trees and blow context windows.
pub fn walk_tree_bounded(
    pid: i32,
    window_id: Option<u32>,
    query: Option<&str>,
    max_elements: usize,
    max_depth: usize,
) -> TreeWalkResult {
    walk_tree_bounded_with_mode(
        pid,
        window_id,
        query,
        max_elements,
        max_depth,
        WalkMode::Native,
    )
}

/// Codex Computer Use compatibility requires a complete addressable map rather
/// than the native action-only map. This variant preserves meaningful layout
/// containers and assigns indices to display text so scroll, selection, and
/// secondary actions can target the same rows the v829 surface exposes.
/// Native callers continue through [`walk_tree_bounded`] unchanged.
pub fn walk_tree_bounded_full_map(
    pid: i32,
    window_id: Option<u32>,
    query: Option<&str>,
    max_elements: usize,
    max_depth: usize,
) -> TreeWalkResult {
    walk_tree_bounded_with_mode(
        pid,
        window_id,
        query,
        max_elements,
        max_depth,
        WalkMode::CodexFull,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WalkMode {
    Native,
    CodexFull,
}

fn walk_tree_bounded_with_mode(
    pid: i32,
    window_id: Option<u32>,
    query: Option<&str>,
    max_elements: usize,
    max_depth: usize,
    mode: WalkMode,
) -> TreeWalkResult {
    let mut nodes: Vec<AXNode> = Vec::new();
    let mut lines: Vec<(usize, String)> = Vec::new(); // (depth, line)
    let mut index_counter = 0usize;
    // Shared visited-node counter passed into walk_element to enforce the cap.
    let mut visited_count = 0usize;
    // Set to true only when walk_element actually stops early due to the cap —
    // avoids a false-positive when the tree naturally ends on exactly the cap.
    let mut truncated = false;

    unsafe {
        let app_elem = AXUIElementCreateApplication(pid);
        if app_elem.is_null() {
            return TreeWalkResult { tree_markdown: String::new(), nodes, truncated: false };
        }

        // Chromium/Electron apps (Arc, VS Code, Electron shells) ship their
        // web-content AX tree OFF and only build it once an assistive client
        // asks for it. Without this, the first walk of such an app returns an
        // empty/title-bar-only tree (#1616). Flip the enablement attribute,
        // then — only when the flip actually took and only the first time we
        // see this pid — let the asynchronously-built tree settle before we
        // read it. Native Cocoa apps reject the attribute, so they pay no
        // settle cost. This relies on the MAX_ELEMENTS node cap to keep the
        // now-materialized (potentially large) tree bounded.
        let already_enabled = enabled_pids().lock().map(|s| s.contains(&pid)).unwrap_or(false);
        if !already_enabled && enable_chromium_accessibility(app_elem) {
            crate::permissions::panel::pump_run_loop_briefly(CHROMIUM_SETTLE_SECONDS);
            if let Ok(mut set) = enabled_pids().lock() {
                set.insert(pid);
            }
        }

        // Union AXChildren + AXWindows — the only way to see background windows.
        // AXChildren omits windows when the app isn't frontmost (AppKit limitation).
        // AXWindows returns the window list regardless of activation state.
        let from_children = copy_children(app_elem);
        let from_windows = copy_ax_windows(app_elem);

        let mut top_level = from_children;
        for w in from_windows {
            // Deduplicate by raw pointer identity.
            if !top_level.iter().any(|&e| e == w) {
                top_level.push(w);
            } else {
                // Already present — release the extra retain from copy_ax_windows.
                CFRelease(w as CFTypeRef);
            }
        }

        // Filter: keep non-window children (menu bar) + the target window.
        let walk_these: Vec<AXUIElementRef> = if let Some(wid) = window_id {
            top_level.iter().copied().filter(|&child| {
                let role = copy_string_attr(child, "AXRole").unwrap_or_default();
                if role != "AXWindow" {
                    return true; // always keep menu bar and other non-window items
                }
                // Match AX window element → CGWindowID via private SPI.
                ax_get_window_id(child) == Some(wid)
            }).collect()
        } else {
            top_level.iter().copied().collect()
        };

        // Walk each top-level child at depth 0.
        for child in walk_these {
            walk_element(
                child,
                0,
                None,
                &mut nodes,
                &mut lines,
                &mut index_counter,
                &mut visited_count,
                &mut truncated,
                max_elements,
                max_depth,
                mode,
            );
        }

        // Release all top-level elements (copy_children / copy_ax_windows both retain).
        for child in top_level {
            CFRelease(child as CFTypeRef);
        }

        CFRelease(app_elem as CFTypeRef);
    }

    let truncated_flag = truncated;
    let raw_markdown = render_lines(&lines);
    let mut tree_markdown = if let Some(q) = query {
        filter_tree(&raw_markdown, q)
    } else {
        raw_markdown
    };

    if truncated_flag {
        tree_markdown.push_str(&format!(
            "\n⚠️  AX tree truncated at {max_elements} nodes \
             (app has a very large accessibility tree — Arc, Electron, or similar). \
             Element indices above are still valid. Use pixel clicks for elements \
             not visible in this partial tree."
        ));
    }

    TreeWalkResult { tree_markdown, nodes, truncated: truncated_flag }
}

fn should_collapse_layout_container(role: &str, mode: WalkMode) -> bool {
    mode == WalkMode::Native && matches!(role, "AXScrollArea" | "AXGroup")
}

fn should_index_node(
    role: &str,
    is_actionable: bool,
    has_content: bool,
    mode: WalkMode,
) -> bool {
    if is_actionable {
        return true;
    }
    mode == WalkMode::CodexFull
        && role != "AXUnknown"
        && (has_content || role.starts_with("AX"))
}

#[allow(clippy::too_many_arguments)]
unsafe fn walk_element(
    element: AXUIElementRef,
    depth: usize,
    parent_index: Option<usize>,
    nodes: &mut Vec<AXNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    visited_count: &mut usize,
    truncated: &mut bool,
    max_elements: usize,
    max_depth: usize,
    mode: WalkMode,
) {
    if depth > max_depth { return; }
    // Enforce total-node cap — mirrors Swift's maxElements guard.
    // Set the truncated flag only when we actually stop early.
    if *visited_count >= max_elements {
        *truncated = true;
        return;
    }
    *visited_count += 1;

    let role = copy_string_attr(element, "AXRole")
        .unwrap_or_else(|| "AXUnknown".into());

    // Skip pure layout containers that have no interesting content.
    if should_collapse_layout_container(&role, mode) {
        // Still recurse — children may be interesting. Layout containers
        // collapse, so children inherit the parent's depth AND the same
        // parent_index (no actionable node was emitted here).
        let children = copy_children(element);
        for child in children {
            walk_element(
                child,
                depth,
                parent_index,
                nodes,
                lines,
                counter,
                visited_count,
                truncated,
                max_elements,
                max_depth,
                mode,
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
    let title = copy_string_attr(element, "AXTitle");
    let value = copy_string_attr(element, "AXValue");
    // AXPlaceholderValue as fallback for empty text fields.
    let value = value.filter(|v| !v.trim().is_empty())
        .or_else(|| copy_string_attr(element, "AXPlaceholderValue"));
    let description = copy_string_attr(element, "AXDescription");
    let identifier = copy_string_attr(element, "AXIdentifier");
    let help = copy_string_attr(element, "AXHelp").filter(|h| !h.trim().is_empty());
    let actions = copy_action_names(element);

    let visible_title = title.as_deref().unwrap_or("").trim().to_owned();
    let visible_description = description.as_deref().unwrap_or("").trim().to_owned();
    let visible_value = value.as_deref().unwrap_or("").trim().to_owned();

    let has_content = !visible_title.is_empty()
        || !visible_description.is_empty()
        || !visible_value.is_empty();
    let is_actionable = !actions.is_empty();
    let is_indexed = should_index_node(&role, is_actionable, has_content, mode);

    if !is_indexed && !has_content && role != "AXWindow" && role != "AXSheet" {
        let children = copy_children(element);
        for child in children {
            walk_element(
                child,
                depth + 1,
                parent_index,
                nodes,
                lines,
                counter,
                visited_count,
                truncated,
                max_elements,
                max_depth,
                mode,
            );
            CFRelease(child as CFTypeRef);
        }
        return;
    }

    let element_ptr = element as usize;
    let frame = element_screen_rect(element);
    let node = if is_indexed {
        let idx = *counter;
        *counter += 1;
        // Retain so the element stays alive in the cache after `copy_children`
        // releases the per-child ref at the end of the caller's loop.
        CFRetain(element as CFTypeRef);
        AXNode {
            element_index: Some(idx),
            role: role.clone(),
            title: if visible_title.is_empty() { None } else { Some(visible_title.clone()) },
            value: if visible_value.is_empty() { None } else { Some(visible_value.clone()) },
            description: if visible_description.is_empty() { None } else { Some(visible_description.clone()) },
            identifier: identifier.clone(),
            help: help.clone(),
            actions: actions.clone(),
            element_ptr,
            depth,
            parent_element_index: parent_index,
            frame,
        }
    } else {
        AXNode {
            element_index: None,
            role: role.clone(),
            title: if visible_title.is_empty() { None } else { Some(visible_title.clone()) },
            value: if visible_value.is_empty() { None } else { Some(visible_value.clone()) },
            description: if visible_description.is_empty() { None } else { Some(visible_description.clone()) },
            identifier: identifier.clone(),
            help: help.clone(),
            actions: vec![],
            element_ptr,
            depth,
            parent_element_index: parent_index,
            frame,
        }
    };

    // Track this node as the parent for its descendants only when it was
    // assigned an element_index (mirrors what the markdown shows: only
    // indexed rows are addressable in click(element_index=N)).
    let next_parent = node.element_index.or(parent_index);

    let line = format_node_line(&node);
    lines.push((depth, line));
    nodes.push(node);

    let children = copy_children(element);
    for child in children {
        walk_element(
            child,
            depth + 1,
            next_parent,
            nodes,
            lines,
            counter,
            visited_count,
            truncated,
            max_elements,
            max_depth,
            mode,
        );
        CFRelease(child as CFTypeRef);
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
    // where AXTitle="" but AXDescription="2".
    if let Some(d) = &node.description {
        parts.push_str(&format!(" ({})", d));
    }

    // Bracketed metadata block (identifier, help, actions).
    if node.element_index.is_some() {
        let mut attrs: Vec<String> = Vec::new();
        if let Some(id) = &node.identifier {
            attrs.push(format!("id={}", id));
        }
        if let Some(h) = &node.help {
            attrs.push(format!("help=\"{}\"", h));
        }
        if !node.actions.is_empty() {
            let action_str = node.actions.iter()
                .map(|a| a.strip_prefix("AX").unwrap_or(a).to_lowercase())
                .collect::<Vec<_>>()
                .join(",");
            attrs.push(format!("actions=[{}]", action_str));
        }
        if !attrs.is_empty() {
            parts.push_str(" [");
            parts.push_str(&attrs.join(" "));
            parts.push(']');
        }
    }

    parts
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
        for deeper in (depth + 1)..current_ancestor.len() {
            last_emitted_at[deeper] = None;
        }
        current_ancestor[depth] = line;

        if line.to_lowercase().contains(&needle) {
            for ancestor_depth in 0..depth {
                let ancestor = current_ancestor[ancestor_depth];
                if ancestor.is_empty() { continue; }
                if last_emitted_at[ancestor_depth] == Some(ancestor) { continue; }
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
        if ch == ' ' { count += 1; } else { break; }
    }
    count / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_mode_remains_action_only_and_collapses_layout() {
        assert!(should_index_node("AXButton", true, true, WalkMode::Native));
        assert!(!should_index_node("AXStaticText", false, true, WalkMode::Native));
        assert!(!should_index_node("AXWindow", false, false, WalkMode::Native));
        assert!(should_collapse_layout_container("AXGroup", WalkMode::Native));
        assert!(should_collapse_layout_container("AXScrollArea", WalkMode::Native));
    }

    #[test]
    fn codex_full_mode_indexes_text_and_preserves_containers() {
        assert!(should_index_node("AXStaticText", false, true, WalkMode::CodexFull));
        assert!(should_index_node("AXWindow", false, false, WalkMode::CodexFull));
        assert!(should_index_node("AXGroup", false, false, WalkMode::CodexFull));
        assert!(should_index_node("AXScrollArea", false, false, WalkMode::CodexFull));
        assert!(!should_index_node("AXUnknown", false, false, WalkMode::CodexFull));
        assert!(!should_collapse_layout_container("AXGroup", WalkMode::CodexFull));
        assert!(!should_collapse_layout_container("AXScrollArea", WalkMode::CodexFull));
    }
}
