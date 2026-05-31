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

/// Maximum depth for AX tree walks. Deep menus and complex web views can
/// nest deeply; 25 covers realistic app chrome without exploding on
/// pathological trees (mirrors Swift reference implementation).
const MAX_DEPTH: usize = 25;

/// Maximum total nodes visited during a single AX walk. Chromium-family apps
/// (Arc, VS Code, Chrome) can expose thousands of nodes; capping at 2 000
/// keeps the walk bounded while still covering realistic app chrome.
/// When the cap is hit the walk stops early and the partial tree is returned
/// with a warning line appended (mirrors Swift reference implementation).
const MAX_ELEMENTS: usize = 2_000;

/// Per-node cap on how many children we fetch via the ranged-children API.
/// Distinct from MAX_ELEMENTS (a total-walk cap): this bounds a single
/// pathological node (e.g. a virtualized list/grid exposing thousands of
/// rows) so one node can't dominate the walk, while the total cap bounds the
/// walk as a whole. When a node's children are clipped here a per-node note
/// is appended to the tree.
const MAX_CHILDREN: core_foundation::array::CFIndex = 200;

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
    pub identifier: Option<String>,
    pub help: Option<String>,
    pub actions: Vec<String>,
    /// The raw AXUIElementRef pointer value, for caching.
    pub element_ptr: usize,
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
    let mut nodes: Vec<AXNode> = Vec::new();
    let mut lines: Vec<(usize, String)> = Vec::new(); // (depth, line)
    let mut index_counter = 0usize;
    // Shared visited-node counter passed into walk_element to enforce MAX_ELEMENTS.
    let mut visited_count = 0usize;
    // Set to true only when walk_element actually stops early due to the cap —
    // avoids a false-positive when the tree naturally ends on exactly MAX_ELEMENTS.
    let mut truncated = false;

    unsafe {
        let app_elem = AXUIElementCreateApplication(pid);
        if app_elem.is_null() {
            return TreeWalkResult { tree_markdown: String::new(), nodes, truncated: false };
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
            walk_element(child, 0, &mut nodes, &mut lines, &mut index_counter, &mut visited_count, &mut truncated);
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
            "\n⚠️  AX tree truncated at {MAX_ELEMENTS} nodes \
             (app has a very large accessibility tree — Arc, Electron, or similar). \
             Element indices above are still valid. Use pixel clicks for elements \
             not visible in this partial tree."
        ));
    }

    TreeWalkResult { tree_markdown, nodes, truncated: truncated_flag }
}

unsafe fn walk_element(
    element: AXUIElementRef,
    depth: usize,
    nodes: &mut Vec<AXNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    visited_count: &mut usize,
    truncated: &mut bool,
) {
    if depth > MAX_DEPTH { return; }
    // Enforce total-node cap — mirrors Swift's maxElements guard.
    // Set the truncated flag only when we actually stop early.
    if *visited_count >= MAX_ELEMENTS {
        *truncated = true;
        return;
    }
    *visited_count += 1;

    // Batch every string attribute we care about into ONE IPC round-trip
    // instead of ~7 separate AXUIElementCopyAttributeValue calls. Order is
    // load-bearing — each slot is indexed by position below.
    const ATTRS: [&str; 7] = [
        "AXRole",            // 0
        "AXTitle",           // 1
        "AXValue",           // 2
        "AXPlaceholderValue",// 3 — fallback for empty text fields
        "AXDescription",     // 4
        "AXIdentifier",      // 5
        "AXHelp",            // 6
    ];
    let attrs = copy_multiple_attrs(element, &ATTRS);

    let role = attrs[0].clone().unwrap_or_else(|| "AXUnknown".into());

    // Skip pure layout containers that have no interesting content.
    if role == "AXScrollArea" || role == "AXGroup" {
        // Still recurse — children may be interesting.
        let (children, child_clipped) = copy_children_ranged(element, MAX_CHILDREN);
        for child in children {
            walk_element(child, depth, nodes, lines, counter, visited_count, truncated);
            CFRelease(child as CFTypeRef);
        }
        if child_clipped {
            note_clipped_children(lines, depth);
        }
        return;
    }

    // Keep AXTitle and AXDescription SEPARATE so that the tree format matches
    // the Swift reference: title → "title", description → (description).
    // This is critical for Calculator where AXTitle="" but AXDescription="2"
    // (digit buttons). Merging them would produce "2" (quoted) instead of (2)
    // (parens), breaking _find_calc_button which searches for "(2)".
    //
    // None-vs-empty matters here: an attribute that is present-but-empty ("")
    // must stay distinct from an absent attribute. copy_multiple_attrs preserves
    // that — a present empty slot is Some("") and an unsupported slot is None —
    // so the AXTitle="" / AXDescription="2" Calculator case is unaffected by
    // batching.
    let title = attrs[1].clone();
    // AXValue, with AXPlaceholderValue as fallback when AXValue is missing/blank.
    let value = attrs[2].clone()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| attrs[3].clone());
    let description = attrs[4].clone();
    let identifier = attrs[5].clone();
    let help = attrs[6].clone().filter(|h| !h.trim().is_empty());
    // Action names remain a separate call — they come from a different AX API
    // (AXUIElementCopyActionNames) and can't ride the attribute batch.
    let actions = copy_action_names(element);

    let visible_title = title.as_deref().unwrap_or("").trim().to_owned();
    let visible_description = description.as_deref().unwrap_or("").trim().to_owned();
    let visible_value = value.as_deref().unwrap_or("").trim().to_owned();

    let has_content = !visible_title.is_empty()
        || !visible_description.is_empty()
        || !visible_value.is_empty();
    let is_actionable = !actions.is_empty();

    if !is_actionable && !has_content && role != "AXWindow" && role != "AXSheet" {
        let (children, child_clipped) = copy_children_ranged(element, MAX_CHILDREN);
        for child in children {
            walk_element(child, depth + 1, nodes, lines, counter, visited_count, truncated);
            CFRelease(child as CFTypeRef);
        }
        if child_clipped {
            note_clipped_children(lines, depth + 1);
        }
        return;
    }

    let element_ptr = element as usize;
    let node = if is_actionable {
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
        }
    };

    let line = format_node_line(&node);
    lines.push((depth, line));
    nodes.push(node);

    let (children, child_clipped) = copy_children_ranged(element, MAX_CHILDREN);
    for child in children {
        walk_element(child, depth + 1, nodes, lines, counter, visited_count, truncated);
        CFRelease(child as CFTypeRef);
    }
    if child_clipped {
        note_clipped_children(lines, depth + 1);
    }
}

/// Append a per-node truncation note when a node's children were clipped at the
/// per-node `MAX_CHILDREN` cap. This is a *different concern* from the total
/// MAX_ELEMENTS node cap: a single node with thousands of children (a giant
/// list/grid) is capped locally without aborting the whole walk.
fn note_clipped_children(lines: &mut Vec<(usize, String)>, depth: usize) {
    lines.push((
        depth,
        format!("- … (children clipped at {MAX_CHILDREN})"),
    ));
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
