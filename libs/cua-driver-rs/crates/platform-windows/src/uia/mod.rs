//! UI Automation (UIA) tree walking for Windows.
//!
//! Produces the same Markdown format as the macOS AX tree:
//!   `INDENT- [N] ControlType "Name" [value="..." id=... actions=[...]]`
//!   `INDENT- ControlType = "Value"` (non-indexed read-only elements)
//!
//! Uses IUIAutomation COM interface (available Windows 7+).
//! Uses IUIAutomationCacheRequest to batch-fetch all properties in one RPC
//! (avoids per-property cross-process calls that make Chrome's 5000-node tree
//!  take >4s when reading each property individually).

use windows::core::{Interface, BSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ControlTypePropertyId, UIA_HelpTextPropertyId,
    UIA_IsEnabledPropertyId, UIA_IsOffscreenPropertyId, UIA_NamePropertyId,
    UIA_ValueValuePropertyId, UIA_InvokePatternId, UIA_SelectionItemPatternId,
    UIA_TogglePatternId, UIA_ExpandCollapsePatternId, UIA_TextPatternId,
    UIA_ValuePatternId, UIA_ScrollPatternId,
    TreeScope_Subtree,
};

pub mod cache;
pub use cache::ElementCache;

const MAX_DEPTH: usize = 25;
const MAX_TOTAL_ELEMENTS: usize = 5000;

/// A single node in the UIA tree.
#[derive(Clone)]
pub struct UiaNode {
    pub element_index: Option<usize>,
    pub control_type: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub automation_id: Option<String>,
    pub help_text: Option<String>,
    pub actions: Vec<String>,
    /// Raw IUIAutomationElement pointer as usize (retained for cache).
    pub element_ptr: usize,
    /// Screen-coordinate center, captured at walk time to avoid later COM calls.
    pub center_x: i32,
    pub center_y: i32,
}

pub struct UiaTreeResult {
    pub tree_markdown: String,
    pub nodes: Vec<UiaNode>,
}

/// Walk the UIA tree for the window with the given HWND.
pub fn walk_tree(hwnd: u64, query: Option<&str>) -> UiaTreeResult {
    unsafe { walk_tree_unsafe(hwnd, query) }
}

unsafe fn walk_tree_unsafe(hwnd: u64, query: Option<&str>) -> UiaTreeResult {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

    let automation: IUIAutomation = match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
        Ok(a) => a,
        Err(e) => return UiaTreeResult {
            tree_markdown: format!("UIA init failed: {e}"),
            nodes: Vec::new(),
        },
    };

    // Build a cache request that fetches everything we need in ONE bulk RPC.
    let cache_req: IUIAutomationCacheRequest = match automation.CreateCacheRequest() {
        Ok(r) => r,
        Err(e) => return UiaTreeResult {
            tree_markdown: format!("CreateCacheRequest failed: {e}"),
            nodes: Vec::new(),
        },
    };

    // Properties to pre-fetch.
    for prop in &[
        UIA_ControlTypePropertyId,
        UIA_NamePropertyId,
        UIA_ValueValuePropertyId,
        UIA_AutomationIdPropertyId,
        UIA_HelpTextPropertyId,
        UIA_IsEnabledPropertyId,
        UIA_IsOffscreenPropertyId,
        UIA_BoundingRectanglePropertyId,
    ] {
        let _ = cache_req.AddProperty(*prop);
    }

    // Patterns to pre-fetch (for action detection).
    for pat in &[
        UIA_InvokePatternId,
        UIA_TogglePatternId,
        UIA_SelectionItemPatternId,
        UIA_ExpandCollapsePatternId,
        UIA_ValuePatternId,
        UIA_TextPatternId,
        UIA_ScrollPatternId,
    ] {
        let _ = cache_req.AddPattern(*pat);
    }

    // Fetch entire subtree in one call.
    let _ = cache_req.SetTreeScope(TreeScope_Subtree);

    // Apply control-view filter (same as ControlViewWalker).
    if let Ok(ctrl_cond) = automation.ControlViewCondition() {
        let _ = cache_req.SetTreeFilter(&ctrl_cond);
    }

    let hwnd_win = windows::Win32::Foundation::HWND(hwnd as *mut _);
    let root_elem = match automation.ElementFromHandleBuildCache(hwnd_win, &cache_req) {
        Ok(e) => e,
        Err(e) => return UiaTreeResult {
            tree_markdown: format!("ElementFromHandleBuildCache failed: {e}"),
            nodes: Vec::new(),
        },
    };

    let mut nodes: Vec<UiaNode> = Vec::new();
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut counter = 0usize;
    let mut total = 0usize;

    walk_cached(&root_elem, 0, &mut nodes, &mut lines, &mut counter, &mut total);

    let raw_md = render_lines(&lines);
    let tree_markdown = if let Some(q) = query {
        filter_tree(&raw_md, q)
    } else {
        raw_md
    };

    UiaTreeResult { tree_markdown, nodes }
}

unsafe fn walk_cached(
    element: &IUIAutomationElement,
    depth: usize,
    nodes: &mut Vec<UiaNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    total: &mut usize,
) {
    if depth > MAX_DEPTH || *total >= MAX_TOTAL_ELEMENTS {
        return;
    }
    *total += 1;

    let control_type = read_cached_control_type(element);
    let name = read_cached_bstr_name(element);
    let value = read_cached_bstr_value(element);
    let automation_id = read_cached_bstr(element, UIA_AutomationIdPropertyId);
    let help_text = read_cached_bstr(element, UIA_HelpTextPropertyId);
    let is_enabled = read_cached_bool(element, UIA_IsEnabledPropertyId).unwrap_or(true);
    let is_offscreen = read_cached_bool(element, UIA_IsOffscreenPropertyId).unwrap_or(false);

    let actions = detect_cached_actions(element, is_enabled);
    let is_actionable = !actions.is_empty() && is_enabled && !is_offscreen;
    let has_content = name.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || value.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);

    if is_actionable || has_content {
        let retained: IUIAutomationElement = element.clone();
        let ptr = retained.as_raw() as usize;
        std::mem::forget(retained);

        let node = if is_actionable {
            let idx = *counter;
            *counter += 1;
            let (center_x, center_y) = read_cached_bounding_rect(element);
            UiaNode {
                element_index: Some(idx),
                control_type: control_type.clone(),
                name: name.clone(),
                value: value.clone(),
                automation_id: automation_id.clone(),
                help_text: help_text.clone(),
                actions: actions.clone(),
                element_ptr: ptr,
                center_x,
                center_y,
            }
        } else {
            UiaNode {
                element_index: None,
                control_type: control_type.clone(),
                name: name.clone(),
                value: value.clone(),
                automation_id: automation_id.clone(),
                help_text: help_text.clone(),
                actions: vec![],
                element_ptr: ptr,
                center_x: 0,
                center_y: 0,
            }
        };

        lines.push((depth, format_node_line(&node)));
        nodes.push(node);
    }

    // Recurse using cached children (no additional RPC).
    if let Ok(children) = element.GetCachedChildren() {
        let len = children.Length().unwrap_or(0);
        for i in 0..len {
            if let Ok(child) = children.GetElement(i) {
                walk_cached(&child, depth + 1, nodes, lines, counter, total);
            }
        }
    }
}

fn read_cached_control_type(element: &IUIAutomationElement) -> String {
    unsafe {
        element.CachedControlType().ok()
            .map(|ct| control_type_name(ct.0))
            .unwrap_or_else(|| "Unknown".into())
    }
}

fn read_cached_bstr_name(element: &IUIAutomationElement) -> Option<String> {
    unsafe {
        let bstr = element.CachedName().ok()?;
        let s = bstr.to_string();
        if s.trim().is_empty() { None } else { Some(s) }
    }
}

fn read_cached_bstr_value(element: &IUIAutomationElement) -> Option<String> {
    read_cached_bstr(element, UIA_ValueValuePropertyId)
}

fn read_cached_bstr(element: &IUIAutomationElement, property_id: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID) -> Option<String> {
    unsafe {
        let variant = element.GetCachedPropertyValue(property_id).ok()?;
        if variant.as_raw().Anonymous.Anonymous.vt == 8 {
            let bstr = BSTR::from_raw(variant.as_raw().Anonymous.Anonymous.Anonymous.bstrVal);
            let s = bstr.to_string();
            std::mem::forget(bstr);
            if s.trim().is_empty() { None } else { Some(s) }
        } else {
            None
        }
    }
}

fn read_cached_bool(element: &IUIAutomationElement, property_id: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID) -> Option<bool> {
    unsafe {
        let variant = element.GetCachedPropertyValue(property_id).ok()?;
        if variant.as_raw().Anonymous.Anonymous.vt == 11 {
            Some(variant.as_raw().Anonymous.Anonymous.Anonymous.boolVal != 0)
        } else {
            None
        }
    }
}

fn read_cached_bounding_rect(element: &IUIAutomationElement) -> (i32, i32) {
    unsafe {
        element.CachedBoundingRectangle()
            .map(|r| ((r.left + r.right) / 2, (r.top + r.bottom) / 2))
            .unwrap_or((0, 0))
    }
}

fn detect_cached_actions(element: &IUIAutomationElement, is_enabled: bool) -> Vec<String> {
    if !is_enabled { return vec![]; }
    let mut actions = Vec::new();
    unsafe {
        if element.GetCachedPattern(UIA_InvokePatternId).is_ok() {
            actions.push("invoke".into());
        }
        if element.GetCachedPattern(UIA_TogglePatternId).is_ok() {
            actions.push("toggle".into());
        }
        if element.GetCachedPattern(UIA_SelectionItemPatternId).is_ok() {
            actions.push("select".into());
        }
        if element.GetCachedPattern(UIA_ExpandCollapsePatternId).is_ok() {
            actions.push("expand".into());
        }
        if element.GetCachedPattern(UIA_ValuePatternId).is_ok() {
            actions.push("set_value".into());
        }
        if element.GetCachedPattern(UIA_TextPatternId).is_ok() {
            actions.push("text".into());
        }
        if element.GetCachedPattern(UIA_ScrollPatternId).is_ok() {
            actions.push("scroll".into());
        }
    }
    actions
}

fn control_type_name(id: i32) -> String {
    match id {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50025 => "Custom",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        50039 => "SemanticZoom",
        50040 => "AppBar",
        _ => "Unknown",
    }.into()
}

fn format_node_line(node: &UiaNode) -> String {
    let mut s = String::new();
    if let Some(idx) = node.element_index {
        s.push_str(&format!("- [{}] {}", idx, node.control_type));
        if let Some(n) = &node.name {
            s.push_str(&format!(" \"{}\"", n));
        }
        let mut attrs = Vec::new();
        if let Some(v) = &node.value { attrs.push(format!("value=\"{}\"", v)); }
        if let Some(id) = &node.automation_id { attrs.push(format!("id={}", id)); }
        if let Some(h) = &node.help_text { attrs.push(format!("help=\"{}\"", h)); }
        if !node.actions.is_empty() {
            attrs.push(format!("actions=[{}]", node.actions.join(",")));
        }
        if !attrs.is_empty() {
            s.push_str(&format!(" [{}]", attrs.join(" ")));
        }
    } else {
        s.push_str(&format!("- {}", node.control_type));
        if let Some(n) = &node.name { s.push_str(&format!(" \"{}\"", n)); }
        if let Some(v) = &node.value { s.push_str(&format!(" = \"{}\"", v)); }
    }
    s
}

fn render_lines(lines: &[(usize, String)]) -> String {
    let mut out = String::new();
    for (depth, line) in lines {
        for _ in 0..*depth { out.push_str("  "); }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn filter_tree(markdown: &str, query: &str) -> String {
    let needle = query.to_lowercase();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut ancestors: Vec<&str> = Vec::new();
    let mut last_emitted: Vec<Option<&str>> = Vec::new();
    let mut output: Vec<&str> = Vec::new();

    for line in &lines {
        let depth = line.chars().take_while(|c| *c == ' ').count() / 2;
        while ancestors.len() <= depth {
            ancestors.push("");
            last_emitted.push(None);
        }
        for d in (depth + 1)..ancestors.len() { last_emitted[d] = None; }
        ancestors[depth] = line;

        if line.to_lowercase().contains(&needle) {
            for d in 0..depth {
                if ancestors[d].is_empty() { continue; }
                if last_emitted[d] == Some(ancestors[d]) { continue; }
                last_emitted[d] = Some(ancestors[d]);
                output.push(ancestors[d]);
            }
            last_emitted[depth] = Some(line);
            output.push(line);
        }
    }

    if output.is_empty() { return String::new(); }
    let mut r = output.join("\n");
    r.push('\n');
    r
}
