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

use cua_driver_core::walk_budget::WalkBudget;
use windows::core::{Interface, BSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationElementArray, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, ToggleState_Off, ToggleState_On,
    TreeScope, TreeScope_Children, TreeScope_Element, TreeScope_Subtree,
    UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId,
    UIA_ExpandCollapsePatternId, UIA_HelpTextPropertyId, UIA_InvokePatternId,
    UIA_IsEnabledPropertyId, UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
    UIA_NamePropertyId, UIA_ProcessIdPropertyId, UIA_RangeValuePatternId, UIA_ScrollPatternId,
    UIA_SelectionItemIsSelectedPropertyId, UIA_SelectionItemPatternId, UIA_TextPatternId,
    UIA_TogglePatternId, UIA_ToggleToggleStatePropertyId, UIA_ValuePatternId,
    UIA_ValueValuePropertyId,
};

pub mod fg_bypass;
pub(crate) mod focused_document;
pub mod scroll;
pub mod snapshot;
pub mod windows_enum;
pub use snapshot::Snapshots;
pub use windows_enum::enumerate_top_level_windows;

/// Default cap; callers can override via [`walk_tree_bounded`].
pub const DEFAULT_MAX_DEPTH: usize = 25;
/// Default cap; callers can override via [`walk_tree_bounded`].
pub const DEFAULT_MAX_TOTAL_ELEMENTS: usize = 5000;

/// A single node in the accessibility tree.
///
/// Same shape for the UIA primary path AND the MSAA fallback (used for
/// SAL/VCL window classes — see `msaa.rs`). MSAA-only fields use the
/// `_ptr is IAccessible` / `msaa_role = Some(...)` discriminator.
#[derive(Clone)]
pub struct UiaNode {
    pub element_index: Option<usize>,
    pub control_type: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub automation_id: Option<String>,
    pub help_text: Option<String>,
    pub actions: Vec<String>,
    /// Enabled state reported by UIA. `None` is reserved for fallback
    /// backends that cannot establish it.
    pub enabled: Option<bool>,
    /// Toggle/selection state when the element exposes one of those patterns.
    pub selected: Option<bool>,
    /// UIA `IsPassword`: `Some(true)` for a password field, `Some(false)` for
    /// an element that accepts text and is not one, `None` when unknown or
    /// not applicable (non-text elements, MSAA fallback).
    pub is_password: Option<bool>,
    /// Raw COM pointer (IUIAutomationElement for UIA path, IAccessible for
    /// MSAA path) as usize. Retained — `Snapshots` Drop releases it via
    /// the `kind`-appropriate vtable.
    pub element_ptr: usize,
    /// Screen-coordinate center, captured at walk time to avoid later COM calls.
    pub center_x: i32,
    pub center_y: i32,
    /// Full screen-coord rect (left, top, right, bottom). Available for
    /// elements that report a meaningful bounding box. Used by the click
    /// tool when `action:"expand"` needs the right-edge offset.
    pub rect: Option<(i32, i32, i32, i32)>,
    /// MSAA role code (e.g. 0x38 = `ROLE_SYSTEM_BUTTONDROPDOWN`). `Some`
    /// iff this node came from the MSAA walker — the click tool checks
    /// this to route `action:"expand"` to a right-edge SendInput click
    /// instead of an unsupported UIA pattern lookup.
    pub msaa_role: Option<i32>,
    /// Depth in the rendered markdown tree (matches the `lines` indent
    /// level). Defaults to 0 when the node came from a builder that
    /// doesn't track depth.
    pub depth: usize,
    /// `element_index` of the nearest actionable ancestor, if any.
    /// Mirrors the markdown's parent-of-this-row.
    pub parent_element_index: Option<usize>,
    /// True when this node is below a UIA Document control. Browser-owned
    /// consent chrome must never match renderer-controlled descendants.
    pub in_web_content: bool,
}

pub struct UiaTreeResult {
    pub tree_markdown: String,
    pub nodes: Vec<UiaNode>,
    /// True when the unprojected native walk visited every node exposed by
    /// the selected root without a bound or enumeration failure skipping a
    /// node/subtree. A `query` only projects the response and does not change
    /// this underlying snapshot property.
    pub elements_complete: bool,
}

#[derive(Debug)]
struct WalkState<'a> {
    budget: &'a mut WalkBudget,
    max_depth: usize,
    complete: bool,
}

impl<'a> WalkState<'a> {
    fn new(budget: &'a mut WalkBudget, max_depth: usize) -> Self {
        Self {
            budget,
            max_depth,
            complete: true,
        }
    }

    /// Admit one node to the walk. Reaching a bound exactly is not itself
    /// truncation; completeness changes only when a node beyond that bound is
    /// actually encountered and skipped.
    fn enter(&mut self, depth: usize) -> bool {
        if depth > self.max_depth || !self.budget.admit() {
            self.complete = false;
            return false;
        }
        true
    }

    fn mark_incomplete(&mut self) {
        self.complete = false;
    }
}

/// Walk the UIA tree for the window with the given HWND.
pub fn walk_tree(hwnd: u64, query: Option<&str>) -> UiaTreeResult {
    walk_tree_bounded(hwnd, query, DEFAULT_MAX_TOTAL_ELEMENTS, DEFAULT_MAX_DEPTH)
}

fn is_menu_control(control_type: &str) -> bool {
    matches!(control_type, "Menu" | "MenuBar" | "MenuItem")
}

fn exact_menu_path_matches(nodes: &[UiaNode], path: &[String]) -> Vec<usize> {
    let by_element_index: std::collections::HashMap<usize, usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(node_index, node)| node.element_index.map(|index| (index, node_index)))
        .collect();

    nodes
        .iter()
        .enumerate()
        .filter_map(|(node_index, node)| {
            if !is_menu_control(&node.control_type)
                || node.enabled == Some(false)
                || node.name.as_deref().map(str::trim) != path.last().map(String::as_str)
            {
                return None;
            }
            let mut lineage = Vec::new();
            let mut cursor = Some(node_index);
            while let Some(current) = cursor {
                let ancestor = &nodes[current];
                if is_menu_control(&ancestor.control_type) {
                    if let Some(name) = ancestor.name.as_deref().map(str::trim) {
                        if !name.is_empty() {
                            lineage.push(name);
                        }
                    }
                }
                cursor = ancestor
                    .parent_element_index
                    .and_then(|index| by_element_index.get(&index).copied());
            }
            lineage.reverse();
            lineage.dedup();
            (lineage == path.iter().map(String::as_str).collect::<Vec<_>>()).then_some(node_index)
        })
        .collect()
}

unsafe fn release_walk_nodes(nodes: Vec<UiaNode>) {
    use windows::Win32::UI::Accessibility::IAccessible;
    for node in nodes {
        if node.element_ptr == 0 {
            continue;
        }
        if node.msaa_role.is_some() {
            drop(IAccessible::from_raw(node.element_ptr as *mut _));
        } else {
            drop(IUIAutomationElement::from_raw(node.element_ptr as *mut _));
        }
    }
}

unsafe fn invoke_menu_element(element_ptr: usize, final_segment: bool) -> Result<(), String> {
    let element =
        std::mem::ManuallyDrop::new(IUIAutomationElement::from_raw(element_ptr as *mut _));
    if !final_segment {
        if let Ok(pattern) = element.GetCurrentPattern(UIA_ExpandCollapsePatternId) {
            if let Ok(expand) = pattern.cast::<IUIAutomationExpandCollapsePattern>() {
                return expand
                    .Expand()
                    .map_err(|error| format!("ExpandCollapse.Expand failed: {error}"));
            }
        }
    }
    if let Ok(pattern) = element.GetCurrentPattern(UIA_InvokePatternId) {
        if let Ok(invoke) = pattern.cast::<IUIAutomationInvokePattern>() {
            return invoke
                .Invoke()
                .map_err(|error| format!("InvokePattern.Invoke failed: {error}"));
        }
    }
    if final_segment {
        if let Ok(pattern) = element.GetCurrentPattern(UIA_SelectionItemPatternId) {
            if let Ok(selection) = pattern.cast::<IUIAutomationSelectionItemPattern>() {
                return selection
                    .Select()
                    .map_err(|error| format!("SelectionItem.Select failed: {error}"));
            }
        }
    }
    Err("element exposes no usable native menu pattern".into())
}

/// Resolve and invoke an exact application menu path from fresh UIA state at
/// every hop. No cached element index survives a menu mutation.
pub fn invoke_menu_path(hwnd: u64, path: &[String]) -> Result<(), String> {
    for depth in 0..path.len() {
        let result = walk_tree(hwnd, None);
        let matches = exact_menu_path_matches(&result.nodes, &path[..=depth]);
        let target_index = match matches.as_slice() {
            [index] => *index,
            [] => {
                unsafe { release_walk_nodes(result.nodes) };
                return Err(format!("menu path segment {depth} was not found"));
            }
            _ => {
                unsafe { release_walk_nodes(result.nodes) };
                return Err(format!("menu path segment {depth} is ambiguous"));
            }
        };
        let target = &result.nodes[target_index];
        let error = if target.enabled == Some(false) {
            Some(format!("menu path segment {depth} is disabled"))
        } else if target.msaa_role.is_some() {
            Some(format!(
                "menu path segment {depth} is exposed only through MSAA, not UI Automation"
            ))
        } else {
            unsafe { invoke_menu_element(target.element_ptr, depth + 1 == path.len()) }.err()
        };
        unsafe { release_walk_nodes(result.nodes) };
        if let Some(error) = error {
            return Err(format!("menu path segment {depth}: {error}"));
        }
        if depth + 1 != path.len() {
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }
    Ok(())
}

/// Walk the UIA tree with caller-supplied caps. `max_elements`/`max_depth`
/// truncate the walk and the rendered markdown identically. Issue #22865:
/// caps protect against Electron / large web apps that produce 10k+
/// element trees and blow context windows.
pub fn walk_tree_bounded(
    hwnd: u64,
    query: Option<&str>,
    max_elements: usize,
    max_depth: usize,
) -> UiaTreeResult {
    let mut budget = WalkBudget::nodes_only(max_elements);
    walk_tree_budgeted(hwnd, query, max_depth, &mut budget)
}

/// [`walk_tree_bounded`] under a caller's [`WalkBudget`], checked per node of
/// the cached (and MSAA) walk. The bulk `BuildUpdatedCache` fetch before it is
/// setup the budget does not cover; the caller's backstop bounds it. Read the
/// outcome from `budget` afterwards.
pub fn walk_tree_budgeted(
    hwnd: u64,
    query: Option<&str>,
    max_depth: usize,
    budget: &mut WalkBudget,
) -> UiaTreeResult {
    unsafe { walk_tree_unsafe(hwnd, query, max_depth, budget) }
}

unsafe fn walk_tree_unsafe(
    hwnd: u64,
    query: Option<&str>,
    max_depth: usize,
    budget: &mut WalkBudget,
) -> UiaTreeResult {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

    let automation: IUIAutomation =
        match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
            Ok(a) => a,
            Err(e) => {
                return UiaTreeResult {
                    tree_markdown: format!("UIA init failed: {e}"),
                    nodes: Vec::new(),
                    elements_complete: false,
                }
            }
        };

    // Build a cache request that fetches everything we need in ONE bulk RPC.
    let (cache_req, mut cache_request_complete) = match node_cache_request(&automation) {
        Ok(r) => r,
        Err(e) => {
            return UiaTreeResult {
                tree_markdown: format!("CreateCacheRequest failed: {e}"),
                nodes: Vec::new(),
                elements_complete: false,
            }
        }
    };

    // Fetch entire subtree in one call.
    cache_request_complete &= cache_req.SetTreeScope(TreeScope_Subtree).is_ok();

    // Apply control-view filter (same as ControlViewWalker).
    cache_request_complete &= automation
        .ControlViewCondition()
        .and_then(|condition| cache_req.SetTreeFilter(&condition))
        .is_ok();

    let hwnd_win = windows::Win32::Foundation::HWND(hwnd as *mut _);

    // SAL (LibreOffice / OpenOffice) fallback: ALL SAL-class windows go
    // through the MSAA walker.
    //
    // Two reasons:
    //   1. **Hang avoidance** for SALSUBFRAME / SALMENU / SALTMPSUBFRAME —
    //      VCL's UIA provider hangs on `BuildUpdatedCache(TreeScope.Subtree)`
    //      under the daemon's MTA pool, wasting the timeout_ms walk budget.
    //   2. **Role fidelity** for SALFRAME (main document window, Recovery
    //      dialog) — UIA technically walks fine, but the built-in
    //      MSAA→UIA proxy collapses `ROLE_SYSTEM_BUTTONDROPDOWN` (0x38) to
    //      a featureless `SplitButton` with no separable dropdown
    //      affordance. MSAA via oleacc.dll preserves the role, letting
    //      the click tool route `action:"expand"` to a right-edge
    //      SendInput click that opens the dropdown half (e.g. LO Writer
    //      "Font Color" → SALTMPSUBFRAME color picker).
    //
    // The UIA pattern dispatches we lose for SALFRAME (Toggle / Select /
    // ExpandCollapse) only matter for WinUI3 controls, which VCL doesn't
    // host. Net win.
    let sal_class: bool = {
        use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd_win, &mut buf);
        n > 0 && String::from_utf16_lossy(&buf[..n as usize]).starts_with("SAL")
    };
    if sal_class {
        return crate::msaa::walk_msaa_tree_budgeted(hwnd, budget);
    }

    // Two-call sequence (ElementFromHandle + BuildUpdatedCache) instead of
    // the atomic ElementFromHandleBuildCache. Empirically, the single-call
    // batched variant hangs against SAL even in CLI in-process tests;
    // the two-call path works in-process. Kept for non-SAL targets where
    // the difference matters for performance-sensitive providers.
    let uncached = match automation.ElementFromHandle(hwnd_win) {
        Ok(e) => e,
        Err(e) => {
            return UiaTreeResult {
                tree_markdown: format!("ElementFromHandle failed: {e}"),
                nodes: Vec::new(),
                elements_complete: false,
            }
        }
    };
    // A single transient provider error (commonly E_FAIL / 0x80004005 from a
    // control rebuilding its automation subtree mid-walk) must not take down
    // the whole snapshot — the same call usually succeeds a beat later. Retry
    // the bulk cache build a few times with a short backoff before giving up,
    // so one misbehaving provider doesn't force the agent down to pixel mode.
    // See #1881. (A per-node partial-tree fallback — returning elements=N for
    // the subtrees that did resolve — remains a larger follow-up.)
    let root_elem = {
        const MAX_ATTEMPTS: u32 = 3;
        let mut attempt = 0u32;
        loop {
            match uncached.BuildUpdatedCache(&cache_req) {
                Ok(e) => break e,
                Err(e) => {
                    attempt += 1;
                    if attempt >= MAX_ATTEMPTS {
                        return UiaTreeResult {
                            tree_markdown: format!(
                                "BuildUpdatedCache failed after {attempt} attempts: {e}"
                            ),
                            nodes: Vec::new(),
                            elements_complete: false,
                        };
                    }
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            }
        }
    };

    let mut nodes: Vec<UiaNode> = Vec::new();
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut counter = 0usize;
    let mut walk_state = WalkState::new(budget, max_depth);
    if !cache_request_complete {
        walk_state.mark_incomplete();
    }

    walk_cached_bounded(
        &root_elem,
        0,
        None,
        false,
        &mut nodes,
        &mut lines,
        &mut counter,
        &mut walk_state,
    );

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
    // wrapper-only node from the primary walk stays the result. Both paths
    // share the caller's time and node budget.
    if nodes.iter().filter(|n| n.element_index.is_some()).count() == 0 {
        // An empty HWND wrapper cannot prove a complete window snapshot. The
        // fallback may discover controls in other windows of the process, so
        // it remains discovery evidence rather than an exact-window proof.
        walk_state.mark_incomplete();
        // SAL windows already returned through the MSAA path above.
        if !walk_state.budget.expired() {
            if let Some(target_pid) = pid_from_hwnd(hwnd_win) {
                let mut fallback_nodes: Vec<UiaNode> = Vec::new();
                let mut fallback_lines: Vec<(usize, String)> = Vec::new();
                let mut fallback_counter = 0usize;

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
                    &mut walk_state,
                );

                if fallback_nodes.iter().any(|n| n.element_index.is_some()) {
                    nodes = fallback_nodes;
                    lines = fallback_lines;
                }
            }
        }
    }

    let raw_md = render_lines(&lines);
    let tree_markdown = if let Some(q) = query {
        filter_tree(&raw_md, q)
    } else {
        raw_md
    };

    UiaTreeResult {
        tree_markdown,
        nodes,
        elements_complete: walk_state.complete,
    }
}

/// Resolve the owning process id of `hwnd` via `GetWindowThreadProcessId`.
/// Returns `None` if the API fails or the HWND is invalid.
unsafe fn pid_from_hwnd(hwnd: windows::Win32::Foundation::HWND) -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid: u32 = 0;
    let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if tid == 0 || pid == 0 {
        None
    } else {
        Some(pid)
    }
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
#[allow(clippy::too_many_arguments)]
unsafe fn walk_root_by_pid(
    automation: &IUIAutomation,
    cache_req: &IUIAutomationCacheRequest,
    target_pid: u32,
    nodes: &mut Vec<UiaNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    walk_state: &mut WalkState<'_>,
) {
    let root = match automation.GetRootElement() {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(target: "uia", "GetRootElement failed: {e}");
            walk_state.mark_incomplete();
            return;
        }
    };
    let true_cond = match automation.CreateTrueCondition() {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(target: "uia", "CreateTrueCondition failed: {e}");
            walk_state.mark_incomplete();
            return;
        }
    };
    let kids = match root.FindAll(TreeScope_Children, &true_cond) {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!(target: "uia", "root.FindAll(Children) failed: {e}");
            walk_state.mark_incomplete();
            return;
        }
    };
    let count = match kids.Length() {
        Ok(count) => count,
        Err(e) => {
            tracing::debug!(target: "uia", "root child count failed: {e}");
            walk_state.mark_incomplete();
            return;
        }
    };
    for i in 0..count {
        let elem = match kids.GetElement(i) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(target: "uia", "root child {i} fetch failed: {e}");
                walk_state.mark_incomplete();
                continue;
            }
        };
        // Read ProcessId without a cache — root.FindAll didn't use one.
        // VARIANT for VT_I4 (UIA's ProcessId type) puts the int at
        // Anonymous.Anonymous.Anonymous.lVal; mirrors the read_cached_bool
        // pattern below for VT_BOOL.
        let pid: u32 = match elem.GetCurrentPropertyValue(UIA_ProcessIdPropertyId) {
            Ok(v) => {
                let raw = v.as_raw();
                if raw.Anonymous.Anonymous.vt != 3
                /* VT_I4 */
                {
                    walk_state.mark_incomplete();
                    continue;
                }
                raw.Anonymous.Anonymous.Anonymous.lVal as u32
            }
            Err(e) => {
                tracing::debug!(target: "uia", "root child {i} ProcessId read failed: {e}");
                walk_state.mark_incomplete();
                continue;
            }
        };
        if pid != target_pid {
            continue;
        }
        // Match — pull a cached subtree from this element using the same
        // cache_req shape as the primary path so walk_cached_bounded sees the same
        // properties + patterns.
        let cached = match elem.BuildUpdatedCache(cache_req) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(target: "uia", "BuildUpdatedCache on pid={target_pid} match failed: {e}");
                walk_state.mark_incomplete();
                continue;
            }
        };
        walk_cached_bounded(&cached, 0, None, false, nodes, lines, counter, walk_state);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn walk_cached_bounded(
    element: &IUIAutomationElement,
    depth: usize,
    parent_index: Option<usize>,
    in_web_content: bool,
    nodes: &mut Vec<UiaNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    walk_state: &mut WalkState<'_>,
) {
    if !walk_state.enter(depth) {
        return;
    }

    let (control_type, emitted_parent) = emit_cached_node(
        element,
        depth,
        parent_index,
        in_web_content,
        nodes,
        lines,
        counter,
    );

    let children = match cached_children(element) {
        Ok(Some(children)) => children,
        Ok(None) => return,
        Err(_) => {
            walk_state.mark_incomplete();
            return;
        }
    };
    let len = match children.Length() {
        Ok(len) => len,
        Err(_) => {
            walk_state.mark_incomplete();
            return;
        }
    };
    for i in 0..len {
        match children.GetElement(i) {
            Ok(child) => walk_cached_bounded(
                &child,
                depth + 1,
                emitted_parent,
                in_web_content || control_type.eq_ignore_ascii_case("Document"),
                nodes,
                lines,
                counter,
                walk_state,
            ),
            Err(_) => walk_state.mark_incomplete(),
        }
    }
}

unsafe fn cached_children(
    element: &IUIAutomationElement,
) -> windows::core::Result<Option<IUIAutomationElementArray>> {
    let mut raw = std::ptr::null_mut();
    let result =
        (Interface::vtable(element).GetCachedChildren)(Interface::as_raw(element), &mut raw);
    result.ok()?;
    if raw.is_null() {
        Ok(None)
    } else {
        Ok(Some(IUIAutomationElementArray::from_raw(raw)))
    }
}

/// Cache request carrying every property and pattern the node projection
/// reads. Callers choose the tree scope and filter.
unsafe fn node_cache_request(
    automation: &IUIAutomation,
) -> windows::core::Result<(IUIAutomationCacheRequest, bool)> {
    let cache_req = automation.CreateCacheRequest()?;
    let mut complete = true;
    for prop in &[
        UIA_ControlTypePropertyId,
        UIA_NamePropertyId,
        UIA_ValueValuePropertyId,
        UIA_AutomationIdPropertyId,
        UIA_HelpTextPropertyId,
        UIA_IsEnabledPropertyId,
        UIA_IsOffscreenPropertyId,
        UIA_IsPasswordPropertyId,
        UIA_BoundingRectanglePropertyId,
        UIA_ToggleToggleStatePropertyId,
        UIA_SelectionItemIsSelectedPropertyId,
    ] {
        complete &= cache_req.AddProperty(*prop).is_ok();
    }
    for pat in &[
        UIA_InvokePatternId,
        UIA_TogglePatternId,
        UIA_SelectionItemPatternId,
        UIA_ExpandCollapsePatternId,
        UIA_ValuePatternId,
        UIA_RangeValuePatternId,
        UIA_TextPatternId,
        UIA_ScrollPatternId,
    ] {
        complete &= cache_req.AddPattern(*pat).is_ok();
    }
    Ok((cache_req, complete))
}

/// Walk only a window's native (non-renderer) accessibility tree.
///
/// [`walk_tree`] fetches the whole subtree in one `BuildUpdatedCache` call,
/// which includes the active tab's web document. When a Chromium renderer
/// stops answering UIA, that one call times out and the walk returns no
/// nodes at all, even though the browser's own tab strip, toolbar and
/// omnibox are still live (#4121: Edge 153 after a CDP session, about 10 s
/// per walk). This walker fetches one level per call and never descends
/// below a `Document`, so browser chrome stays observable. The `Document`
/// node itself is still reported, so callers can prove which page is
/// committed without trusting any renderer-owned descendant.
///
/// Returns `Err` when any native level cannot be read. A partial native tree
/// would misstate tab topology, so callers must treat it as unavailable.
pub fn walk_native_tree(hwnd: u64) -> Result<Vec<UiaNode>, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("UIA init failed: {e}"))?;
        let (cache_req, _) = node_cache_request(&automation)
            .map_err(|e| format!("CreateCacheRequest failed: {e}"))?;
        let _ = cache_req.SetTreeScope(TreeScope(TreeScope_Element.0 | TreeScope_Children.0));
        if let Ok(ctrl_cond) = automation.ControlViewCondition() {
            let _ = cache_req.SetTreeFilter(&ctrl_cond);
        }
        let root = automation
            .ElementFromHandle(windows::Win32::Foundation::HWND(hwnd as *mut _))
            .and_then(|element| element.BuildUpdatedCache(&cache_req))
            .map_err(|e| format!("native UIA root could not be read: {e}"))?;
        let mut nodes = Vec::new();
        let mut lines = Vec::new();
        let mut counter = 0usize;
        let mut budget = WalkBudget::nodes_only(DEFAULT_MAX_TOTAL_ELEMENTS);
        let walked = walk_native_level(
            &root,
            &cache_req,
            0,
            None,
            &mut nodes,
            &mut lines,
            &mut counter,
            &mut budget,
        );
        match walked {
            Ok(()) => Ok(nodes),
            Err(error) => {
                release_walk_nodes(nodes);
                Err(error)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn walk_native_level(
    element: &IUIAutomationElement,
    cache_req: &IUIAutomationCacheRequest,
    depth: usize,
    parent_index: Option<usize>,
    nodes: &mut Vec<UiaNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    budget: &mut WalkBudget,
) -> Result<(), String> {
    if depth > DEFAULT_MAX_DEPTH {
        return Ok(());
    }
    if !budget.admit() {
        return Err("the native UIA tree exceeded the element budget".to_owned());
    }
    let (control_type, emitted_parent) =
        emit_cached_node(element, depth, parent_index, false, nodes, lines, counter);
    if control_type.eq_ignore_ascii_case("Document") {
        return Ok(());
    }
    let Ok(children) = element.GetCachedChildren() else {
        return Ok(());
    };
    for i in 0..children.Length().unwrap_or(0) {
        let child = children
            .GetElement(i)
            .and_then(|child| child.BuildUpdatedCache(cache_req))
            .map_err(|e| format!("a native UIA level could not be read: {e}"))?;
        walk_native_level(
            &child,
            cache_req,
            depth + 1,
            emitted_parent,
            nodes,
            lines,
            counter,
            budget,
        )?;
    }
    Ok(())
}

/// Project one cached element into `nodes`/`lines`. Returns its control type
/// and the parent index its descendants should inherit.
fn emit_cached_node(
    element: &IUIAutomationElement,
    depth: usize,
    parent_index: Option<usize>,
    in_web_content: bool,
    nodes: &mut Vec<UiaNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
) -> (String, Option<usize>) {
    let control_type = read_cached_control_type(element);
    let name = read_cached_bstr_name(element);
    let value = read_cached_bstr_value(element);
    let automation_id = read_cached_bstr(element, UIA_AutomationIdPropertyId);
    let help_text = read_cached_bstr(element, UIA_HelpTextPropertyId);
    let enabled = read_cached_bool(element, UIA_IsEnabledPropertyId);
    // Missing UIA state must remain unknown on the structured observation
    // surface. Action discovery keeps its historical best-effort assumption.
    let is_enabled = enabled.unwrap_or(true);
    let selected = read_cached_selected(element);
    let actions = detect_cached_actions(element, &control_type, is_enabled);
    let is_password = text_entry_is_password(
        read_cached_bool(element, UIA_IsPasswordPropertyId),
        &actions,
    );
    // Providers are expected to blank Value on password fields, but that is a
    // convention, not something UIA enforces. Never echo it once the element
    // says it is a password field.
    let value = redact_password_value(value, is_password);
    let is_actionable = !actions.is_empty() && is_enabled;
    let has_content = name
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        || value
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

    let mut emitted_parent: Option<usize> = parent_index;
    if is_actionable || has_content {
        let retained: IUIAutomationElement = element.clone();
        let ptr = retained.as_raw() as usize;
        std::mem::forget(retained);

        let node = if is_actionable {
            let idx = *counter;
            *counter += 1;
            let (center_x, center_y, rect) = read_cached_bounding_rect_full(element);
            emitted_parent = Some(idx);
            UiaNode {
                element_index: Some(idx),
                control_type: control_type.clone(),
                name: name.clone(),
                value: value.clone(),
                automation_id: automation_id.clone(),
                help_text: help_text.clone(),
                actions: actions.clone(),
                enabled,
                selected,
                is_password,
                element_ptr: ptr,
                center_x,
                center_y,
                rect,
                msaa_role: None,
                depth,
                parent_element_index: parent_index,
                in_web_content,
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
                enabled,
                selected,
                is_password,
                element_ptr: ptr,
                center_x: 0,
                center_y: 0,
                rect: None,
                msaa_role: None,
                depth,
                parent_element_index: parent_index,
                in_web_content,
            }
        };

        lines.push((depth, format_node_line(&node)));
        nodes.push(node);
    }
    (control_type, emitted_parent)
}

/// Keep `IsPassword` where it carries information: always when it is true,
/// and as an explicit `false` only on elements that accept text input, so a
/// consumer can tell "not a password field" from "unknown" without every
/// button in the tree carrying the field.
pub(crate) fn text_entry_is_password(
    is_password: Option<bool>,
    actions: &[String],
) -> Option<bool> {
    match is_password {
        Some(true) => Some(true),
        Some(false) if actions.iter().any(|a| a == "set_value" || a == "text") => Some(false),
        _ => None,
    }
}

/// Drop an element's value when it reports itself as a password field, so a
/// provider that does not blank Value cannot leak the secret through `value`,
/// the label fallback, or the Markdown row.
pub(crate) fn redact_password_value(
    value: Option<String>,
    is_password: Option<bool>,
) -> Option<String> {
    if is_password == Some(true) {
        None
    } else {
        value
    }
}

fn read_cached_control_type(element: &IUIAutomationElement) -> String {
    unsafe {
        element
            .CachedControlType()
            .ok()
            .map(|ct| control_type_name(ct.0))
            .unwrap_or_else(|| "Unknown".into())
    }
}

fn read_cached_bstr_name(element: &IUIAutomationElement) -> Option<String> {
    unsafe {
        let bstr = element.CachedName().ok()?;
        let s = bstr.to_string();
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

fn read_cached_bstr_value(element: &IUIAutomationElement) -> Option<String> {
    read_cached_bstr(element, UIA_ValueValuePropertyId)
}

fn read_cached_bstr(
    element: &IUIAutomationElement,
    property_id: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
) -> Option<String> {
    unsafe {
        let variant = element.GetCachedPropertyValue(property_id).ok()?;
        if variant.as_raw().Anonymous.Anonymous.vt == 8 {
            let bstr = BSTR::from_raw(variant.as_raw().Anonymous.Anonymous.Anonymous.bstrVal);
            let s = bstr.to_string();
            std::mem::forget(bstr);
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        } else {
            None
        }
    }
}

fn read_cached_bool(
    element: &IUIAutomationElement,
    property_id: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
) -> Option<bool> {
    unsafe {
        let variant = element.GetCachedPropertyValue(property_id).ok()?;
        if variant.as_raw().Anonymous.Anonymous.vt == 11 {
            Some(variant.as_raw().Anonymous.Anonymous.Anonymous.boolVal != 0)
        } else {
            None
        }
    }
}

/// Read bounding rect as (center_x, center_y, Some((l,t,r,b))). Returns
/// rect=None when the element has no meaningful BoundingRectangle (offscreen
/// containers, structure-only elements).
fn read_cached_bounding_rect_full(
    element: &IUIAutomationElement,
) -> (i32, i32, Option<(i32, i32, i32, i32)>) {
    unsafe {
        match element.CachedBoundingRectangle() {
            Ok(r) if r.right > r.left && r.bottom > r.top => (
                (r.left + r.right) / 2,
                (r.top + r.bottom) / 2,
                Some((r.left, r.top, r.right, r.bottom)),
            ),
            _ => (0, 0, None),
        }
    }
}

fn detect_cached_actions(
    element: &IUIAutomationElement,
    control_type: &str,
    is_enabled: bool,
) -> Vec<String> {
    if !is_enabled {
        return vec![];
    }
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
        if element
            .GetCachedPattern(UIA_ExpandCollapsePatternId)
            .is_ok()
        {
            actions.push("expand".into());
        }
        if element.GetCachedPattern(UIA_ValuePatternId).is_ok() {
            actions.push("set_value".into());
        }
        // RangeValuePattern is exposed by Sliders, ProgressBars, and other
        // numeric-range controls. Without this entry the slider parent
        // gets actions=[] → marked non-actionable → no `[N]` index in the
        // flat tree, making the slider unaddressable by AutomationId.
        if element.GetCachedPattern(UIA_RangeValuePatternId).is_ok() {
            actions.push("set_value".into());
        }
        if element.GetCachedPattern(UIA_TextPatternId).is_ok() {
            actions.push("text".into());
        }
        if element.GetCachedPattern(UIA_ScrollPatternId).is_ok() {
            actions.push("scroll".into());
        }
    }
    if actions.is_empty() && control_type == "MenuItem" {
        // Some WPF MenuItem peers show up in ControlView with a usable name
        // and bounding rectangle, but without cached Invoke/ExpandCollapse
        // patterns. Index them anyway so click can retry live patterns and then
        // fall back to the coordinate injector if UIA still reports no pattern.
        actions.push("invoke".into());
    }
    actions
}

fn read_cached_selected(element: &IUIAutomationElement) -> Option<bool> {
    unsafe {
        if let Ok(pattern) = element.GetCachedPattern(UIA_TogglePatternId) {
            if let Ok(toggle) = pattern.cast::<IUIAutomationTogglePattern>() {
                return match toggle.CachedToggleState() {
                    Ok(state) if state == ToggleState_On => Some(true),
                    Ok(state) if state == ToggleState_Off => Some(false),
                    _ => None,
                };
            }
        }
        if let Ok(pattern) = element.GetCachedPattern(UIA_SelectionItemPatternId) {
            if let Ok(selection) = pattern.cast::<IUIAutomationSelectionItemPattern>() {
                return selection
                    .CachedIsSelected()
                    .ok()
                    .map(|selected| selected.as_bool());
            }
        }
    }
    None
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
    }
    .into()
}

pub(crate) fn format_node_line(node: &UiaNode) -> String {
    let mut s = String::new();
    if let Some(idx) = node.element_index {
        s.push_str(&format!("- [{}] {}", idx, node.control_type));
        if let Some(n) = &node.name {
            s.push_str(&format!(" \"{}\"", n));
        }
        let mut attrs = Vec::new();
        if let Some(v) = &node.value {
            attrs.push(format!("value=\"{}\"", v));
        }
        if let Some(id) = &node.automation_id {
            attrs.push(format!("id={}", id));
        }
        if let Some(h) = &node.help_text {
            attrs.push(format!("help=\"{}\"", h));
        }
        if node.is_password == Some(true) {
            attrs.push("password".to_owned());
        }
        if !node.actions.is_empty() {
            attrs.push(format!("actions=[{}]", node.actions.join(",")));
        }
        if !attrs.is_empty() {
            s.push_str(&format!(" [{}]", attrs.join(" ")));
        }
    } else {
        s.push_str(&format!("- {}", node.control_type));
        if let Some(n) = &node.name {
            s.push_str(&format!(" \"{}\"", n));
        }
        if let Some(v) = &node.value {
            s.push_str(&format!(" = \"{}\"", v));
        }
    }
    s
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
        for d in (depth + 1)..ancestors.len() {
            last_emitted[d] = None;
        }
        ancestors[depth] = line;

        if line.to_lowercase().contains(&needle) {
            for d in 0..depth {
                if ancestors[d].is_empty() {
                    continue;
                }
                if last_emitted[d] == Some(ancestors[d]) {
                    continue;
                }
                last_emitted[d] = Some(ancestors[d]);
                output.push(ancestors[d]);
            }
            last_emitted[depth] = Some(line);
            output.push(line);
        }
    }

    if output.is_empty() {
        return String::new();
    }
    let mut r = output.join("\n");
    r.push('\n');
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn star(child_count: usize) -> Vec<Vec<usize>> {
        let mut children = vec![Vec::new(); child_count + 1];
        children[0] = (1..=child_count).collect();
        children
    }

    fn chain(len: usize) -> Vec<Vec<usize>> {
        let mut children = vec![Vec::new(); len];
        for i in 0..len.saturating_sub(1) {
            children[i] = vec![i + 1];
        }
        children
    }

    fn walk_synthetic_tree(
        children: &[Vec<usize>],
        node: usize,
        depth: usize,
        state: &mut WalkState<'_>,
    ) {
        if !state.enter(depth) {
            return;
        }
        for child in &children[node] {
            walk_synthetic_tree(children, *child, depth + 1, state);
        }
    }

    #[test]
    fn walk_state_is_complete_only_when_no_bound_skips_a_node() {
        struct Case {
            name: &'static str,
            children: Vec<Vec<usize>>,
            max_elements: usize,
            max_depth: usize,
            expected_visited: usize,
            expected_complete: bool,
        }

        let cases = [
            Case {
                name: "default bounds admit a 55-node tree",
                children: star(54),
                max_elements: DEFAULT_MAX_TOTAL_ELEMENTS,
                max_depth: DEFAULT_MAX_DEPTH,
                expected_visited: 55,
                expected_complete: true,
            },
            Case {
                name: "visiting exactly max_elements is still complete",
                children: star(54),
                max_elements: 55,
                max_depth: DEFAULT_MAX_DEPTH,
                expected_visited: 55,
                expected_complete: true,
            },
            Case {
                name: "skipping a node past max_elements is incomplete",
                children: star(54),
                max_elements: 54,
                max_depth: DEFAULT_MAX_DEPTH,
                expected_visited: 54,
                expected_complete: false,
            },
            Case {
                name: "a leaf at exactly max_depth is still complete",
                children: chain(3),
                max_elements: 10,
                max_depth: 2,
                expected_visited: 3,
                expected_complete: true,
            },
            Case {
                name: "skipping a descendant past max_depth is incomplete",
                children: chain(3),
                max_elements: 10,
                max_depth: 1,
                expected_visited: 2,
                expected_complete: false,
            },
        ];

        for case in cases {
            let mut budget = WalkBudget::nodes_only(case.max_elements);
            let mut state = WalkState::new(&mut budget, case.max_depth);
            walk_synthetic_tree(&case.children, 0, 0, &mut state);
            assert_eq!(
                state.budget.outcome().nodes_visited,
                case.expected_visited,
                "{}: visited",
                case.name
            );
            assert_eq!(
                state.complete, case.expected_complete,
                "{}: complete",
                case.name
            );
        }
    }

    #[test]
    fn walk_state_keeps_the_callers_budget_across_walks() {
        let mut budget = WalkBudget::nodes_only(2);
        assert!(budget.admit());
        let mut state = WalkState::new(&mut budget, DEFAULT_MAX_DEPTH);

        walk_synthetic_tree(&star(1), 0, 0, &mut state);

        assert!(!state.complete);
        let outcome = state.budget.outcome();
        assert_eq!((outcome.nodes_visited, outcome.nodes_pending), (2, 1));
        assert_eq!(outcome.reason(), Some("node_budget"));
    }

    #[test]
    fn timed_out_walk_cannot_report_complete() {
        let mut budget = WalkBudget::new(100, 10);
        budget.stop_for_timeout();
        let mut state = WalkState::new(&mut budget, DEFAULT_MAX_DEPTH);

        walk_synthetic_tree(&star(0), 0, 0, &mut state);

        assert!(!state.complete);
        assert_eq!(state.budget.outcome().reason(), Some("timeout"));
        assert_eq!(state.budget.outcome().nodes_visited, 0);
    }
}
