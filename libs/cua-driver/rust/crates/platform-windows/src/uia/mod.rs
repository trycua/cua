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
    UIA_ProcessIdPropertyId, UIA_ValueValuePropertyId,
    UIA_InvokePatternId, UIA_SelectionItemPatternId,
    UIA_TogglePatternId, UIA_ExpandCollapsePatternId, UIA_TextPatternId,
    UIA_ValuePatternId, UIA_ScrollPatternId,
    TreeScope_Children, TreeScope_Subtree,
};

pub mod cache;
pub mod fg_bypass;
pub mod windows_enum;
pub use cache::ElementCache;
pub use windows_enum::enumerate_top_level_windows;

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

    // SAL (LibreOffice / OpenOffice) fast-path: VCL's UIA provider hangs
    // on `BuildUpdatedCache(TreeScope.Subtree)` for transient dialogs
    // (SALSUBFRAME class — Confirmation, Warning, modal Yes/No). Both
    // the atomic `ElementFromHandleBuildCache` and the split
    // `ElementFromHandle + BuildUpdatedCache(Subtree)` paths block
    // indefinitely under the daemon's MTA thread pool (the same code
    // runs fine via CLI in-process, suggesting a COM apartment /
    // concurrent-access interaction with SAL's provider). The 4 s
    // outer timeout fires and the caller gets the structured
    // diagnostic, but that's a wasted 4 s every call.
    //
    // Short-circuit by class detection: if the target's class starts
    // with "SAL", skip the bulk-cache walk entirely and return the
    // same synthetic stub the post-walk SAL-skip would emit, with the
    // three actionable fallback options. Saves the 4 s wait and the
    // ambiguity of "did the walk run? did it find nothing? did it
    // hang?" The caller sees the stub immediately and routes to the
    // appropriate workaround.
    // Class lookup once — used both for the fast-path skip decision
    // and the diagnostic stub. SALSUBFRAME is the empirically-confirmed
    // hang class (modal Confirmation / Warning dialogs). SALFRAME
    // (the document window, the Recovery dialog) walks fine, so don't
    // include it in the skip list — pixel + element_index against
    // Recovery's Discard All button worked end-to-end through the
    // primary path. SALMENU / SALTMPSUBFRAME are transient menu hosts
    // that the user typically doesn't get_window_state on; default to
    // skipping out of caution. If a real use-case emerges, narrow.
    let sal_class: Option<String> = {
        use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd_win, &mut buf);
        if n > 0 {
            let s = String::from_utf16_lossy(&buf[..n as usize]);
            if s == "SALSUBFRAME" || s == "SALMENU" || s == "SALTMPSUBFRAME" {
                Some(s)
            } else { None }
        } else { None }
    };
    if let Some(class) = sal_class {
        let stub = format!(
            "- Window <{class} — SAL/VCL target, UIA walk skipped>\n\
             (SAL's UIA provider hangs on TreeScope.Subtree BuildUpdatedCache \
             when the daemon is running in MTA; the cua-driver fast-path \
             returns immediately rather than wait 4 s. Use one of: \
             (a) `screenshot(pid, window_id)` + pixel `click(x, y)`; \
             (b) `press_key` with `dispatch:\"foreground\"` (Esc / Enter / Y / N); \
             (c) `get_window_state` with `capture_mode:\"vision\"`.)\n"
        );
        return UiaTreeResult { tree_markdown: stub, nodes: Vec::new() };
    }

    // Two-call sequence (ElementFromHandle + BuildUpdatedCache) instead of
    // the atomic ElementFromHandleBuildCache. Empirically, the single-call
    // batched variant hangs against SAL even in CLI in-process tests;
    // the two-call path works in-process. Kept for non-SAL targets where
    // the difference matters for performance-sensitive providers.
    let uncached = match automation.ElementFromHandle(hwnd_win) {
        Ok(e) => e,
        Err(e) => return UiaTreeResult {
            tree_markdown: format!("ElementFromHandle failed: {e}"),
            nodes: Vec::new(),
        },
    };
    let root_elem = match uncached.BuildUpdatedCache(&cache_req) {
        Ok(e) => e,
        Err(e) => return UiaTreeResult {
            tree_markdown: format!("BuildUpdatedCache failed: {e}"),
            nodes: Vec::new(),
        },
    };

    let mut nodes: Vec<UiaNode> = Vec::new();
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut counter = 0usize;
    let mut total = 0usize;

    walk_cached(&root_elem, 0, &mut nodes, &mut lines, &mut counter, &mut total);

    // Fallback for CoreWindow-class apps (Calculator, Settings, older UWPs).
    // `ElementFromHandle(hwnd)` on a `Windows.UI.Core.CoreWindow` HWND returns
    // an empty wrapper — the actual XAML tree is registered at the desktop
    // root as a separate UIA element with the same ProcessId, not as a child
    // of the hwnd's UIA element. inspect.exe walks from root for these apps;
    // we mirror that here when the primary path yields nothing actionable.
    //
    // Trigger: walk produced zero actionable nodes (the primary path may have
    // still pushed a wrapper-only node — that's why we filter on
    // `element_index.is_some()`).
    //
    // Stage the fallback walk into fresh accumulators and only swap them in
    // if the fallback actually finds actionable elements. Otherwise the
    // wrapper-only node from the primary walk stays the result — better than
    // erasing it AND leaving the consumed `MAX_TOTAL_ELEMENTS` budget intact
    // for the fallback (which would then truncate large trees prematurely).
    if nodes.iter().filter(|n| n.element_index.is_some()).count() == 0 {
        // Skip the desktop-root walk-by-pid fallback for VCL / SAL
        // targets (LibreOffice, OpenOffice). The fallback does its own
        // `BuildUpdatedCache(TreeScope.Subtree)` per matched top-level
        // window — which is exactly the bulk-cache RPC shape that SAL
        // hangs on. The primary two-call path (ElementFromHandle +
        // BuildUpdatedCache on the dialog's own HWND) ALREADY dodged the
        // hang on that one specific call, but the fallback re-introduces
        // it. Returning the empty tree here lets the outer
        // get_window_state timeout fire its structured diagnostic
        // promptly instead of stalling 4 s on the fallback's hang.
        //
        // The diagnostic tells callers exactly how to drive the SAL
        // dialog without the tree: pixel click via screenshot,
        // capture_mode:"vision", or press_key with dispatch:"foreground"
        // for accelerator-style dismissal. That's enough for the
        // common modal-dismissal case (Yes/No/Esc on a Confirmation),
        // which is what SAL dialogs almost always need.
        let is_sal = {
            use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
            let mut buf = [0u16; 64];
            let n = GetClassNameW(hwnd_win, &mut buf);
            n > 0 && {
                let class = String::from_utf16_lossy(&buf[..n as usize]);
                class.starts_with("SAL")
            }
        };
        if !is_sal {
            if let Some(target_pid) = pid_from_hwnd(hwnd_win) {
                let mut fallback_nodes: Vec<UiaNode> = Vec::new();
                let mut fallback_lines: Vec<(usize, String)> = Vec::new();
                let mut fallback_counter = 0usize;
                let mut fallback_total = 0usize;

                tracing::debug!(
                    target: "uia",
                    "ElementFromHandle returned empty tree for hwnd 0x{hwnd:x}; \
                     falling back to GetRootElement + filter ProcessId={target_pid}"
                );
                walk_root_by_pid(
                    &automation,
                    &cache_req,
                    target_pid,
                    &mut fallback_nodes,
                    &mut fallback_lines,
                    &mut fallback_counter,
                    &mut fallback_total,
                );

                if fallback_nodes.iter().any(|n| n.element_index.is_some()) {
                    nodes = fallback_nodes;
                    lines = fallback_lines;
                    // counter/total aren't read after this point — they're
                    // only used by walk_cached's &mut params for element
                    // indexing inside that call.
                }
            }
        } else {
            tracing::debug!(
                target: "uia",
                "SAL target hwnd 0x{hwnd:x} returned empty primary tree; \
                 skipping walk_root_by_pid fallback (known to re-hang on SAL Subtree fetch). \
                 Caller should use press_key/screenshot fallbacks per the get_window_state diagnostic."
            );
            // Return a tree_markdown that mirrors the get_window_state
            // timeout diagnostic so callers get the same actionable
            // fallback options even though the walk itself didn't hit
            // the 4 s outer timeout (because we skipped the hang-prone
            // fallback). Without this the caller sees an empty tree
            // and no error, which is less actionable.
            let stub = format!(
                "- Window <SAL class — empty primary UIA tree>\n\
                 (SAL providers don't expose modal-dialog children via \
                 ElementFromHandle, and the desktop-root fallback walk that \
                 would normally find them is known to hang on SAL Subtree \
                 BuildUpdatedCache. Use one of: \
                 (a) `screenshot(pid, window_id)` + pixel `click(x, y)`; \
                 (b) `press_key` with `dispatch:\"foreground\"` (Esc / Enter / Y / N); \
                 (c) `get_window_state` with `capture_mode:\"vision\"`.)\n"
            );
            return UiaTreeResult { tree_markdown: stub, nodes: Vec::new() };
        }
    }

    let raw_md = render_lines(&lines);
    let tree_markdown = if let Some(q) = query {
        filter_tree(&raw_md, q)
    } else {
        raw_md
    };

    UiaTreeResult { tree_markdown, nodes }
}

/// Resolve the owning process id of `hwnd` via `GetWindowThreadProcessId`.
/// Returns `None` if the API fails or the HWND is invalid.
unsafe fn pid_from_hwnd(hwnd: windows::Win32::Foundation::HWND) -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid: u32 = 0;
    let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if tid == 0 || pid == 0 { None } else { Some(pid) }
}

/// Root-walk UIA fallback for apps whose top-level window is a
/// `Windows.UI.Core.CoreWindow` (Calculator, Settings, older UWPs).
///
/// `ElementFromHandle(CoreWindow_hwnd)` returns an empty wrapper for these —
/// the actual XAML tree is registered at the desktop root as a sibling
/// element with the same `ProcessId`. We enumerate the root's children
/// (filtered by `ProcessId == target_pid`) and walk descendants from there,
/// reusing the caller's `cache_req` so the same properties + patterns get
/// pre-fetched as the primary path.
unsafe fn walk_root_by_pid(
    automation: &IUIAutomation,
    cache_req: &IUIAutomationCacheRequest,
    target_pid: u32,
    nodes: &mut Vec<UiaNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    total: &mut usize,
) {
    let root = match automation.GetRootElement() {
        Ok(r) => r,
        Err(e) => { tracing::debug!(target: "uia", "GetRootElement failed: {e}"); return; }
    };
    let true_cond = match automation.CreateTrueCondition() {
        Ok(c) => c,
        Err(e) => { tracing::debug!(target: "uia", "CreateTrueCondition failed: {e}"); return; }
    };
    let kids = match root.FindAll(TreeScope_Children, &true_cond) {
        Ok(a) => a,
        Err(e) => { tracing::debug!(target: "uia", "root.FindAll(Children) failed: {e}"); return; }
    };
    let count = kids.Length().unwrap_or(0);
    for i in 0..count {
        let elem = match kids.GetElement(i) { Ok(e) => e, Err(_) => continue };
        // Read ProcessId without a cache — root.FindAll didn't use one.
        // VARIANT for VT_I4 (UIA's ProcessId type) puts the int at
        // Anonymous.Anonymous.Anonymous.lVal; mirrors the read_cached_bool
        // pattern below for VT_BOOL.
        let pid: u32 = match elem.GetCurrentPropertyValue(UIA_ProcessIdPropertyId) {
            Ok(v) => {
                let raw = v.as_raw();
                if raw.Anonymous.Anonymous.vt != 3 /* VT_I4 */ { continue; }
                raw.Anonymous.Anonymous.Anonymous.lVal as u32
            }
            Err(_) => continue,
        };
        if pid != target_pid { continue; }
        // Match — pull a cached subtree from this element using the same
        // cache_req shape as the primary path so walk_cached sees the same
        // properties + patterns.
        let cached = match elem.BuildUpdatedCache(cache_req) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(target: "uia", "BuildUpdatedCache on pid={target_pid} match failed: {e}");
                continue;
            }
        };
        walk_cached(&cached, 0, nodes, lines, counter, total);
    }
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
