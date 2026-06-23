//! MSAA (Microsoft Active Accessibility) tree walker.
//!
//! Fallback for SAL/VCL window classes (LibreOffice, OpenOffice) where the
//! UIA walker hangs on `BuildUpdatedCache(Subtree)`. MSAA via oleacc.dll's
//! `AccessibleObjectFromWindow` + recursive `accChild` walks these windows
//! cleanly because it doesn't go through the bulk-cache RPC that SAL's UIA
//! provider deadlocks on under the daemon's MTA pool.
//!
//! Bonus payoff: MSAA preserves the `ROLE_SYSTEM_BUTTONDROPDOWN` role (0x38)
//! that Windows' built-in MSAA→UIA proxy collapses to a featureless
//! `SplitButton` (no `ExpandCollapse` pattern, no separable dropdown child).
//! For BUTTONDROPDOWN, this walker emits `actions=["invoke","expand"]` —
//! the `click` tool routes `action:"expand"` to a right-edge SendInput
//! click that opens the dropdown half (e.g. LO Writer "Font Color" → color
//! picker) instead of just re-firing the press half.
//!
//! Discovery context: `flash-repro/ia2_pixel_click.ps1` proved the path —
//! `accLocation("Font Color")` → click `(rect.right - 4, center_y)` →
//! `SALTMPSUBFRAME 'Font Color'` popup appears.

use std::ptr::null_mut;

use windows::core::{Interface, VARIANT};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};

use crate::uia::{UiaNode, UiaTreeResult};

const OBJID_CLIENT: u32 = 0xFFFFFFFC;

// MSAA role codes (subset; full list in oleacc.h).
const ROLE_SYSTEM_TITLEBAR: i32 = 0x01;
const ROLE_SYSTEM_MENUBAR: i32 = 0x02;
const ROLE_SYSTEM_SCROLLBAR: i32 = 0x03;
const ROLE_SYSTEM_WINDOW: i32 = 0x09;
const ROLE_SYSTEM_CLIENT: i32 = 0x0A;
const ROLE_SYSTEM_MENUPOPUP: i32 = 0x0B;
const ROLE_SYSTEM_MENUITEM: i32 = 0x0C;
const ROLE_SYSTEM_TOOLTIP: i32 = 0x0D;
const ROLE_SYSTEM_DIALOG: i32 = 0x12;
const ROLE_SYSTEM_GROUPING: i32 = 0x14;
const ROLE_SYSTEM_TOOLBAR: i32 = 0x16;
const ROLE_SYSTEM_STATUSBAR: i32 = 0x17;
const ROLE_SYSTEM_LINK: i32 = 0x1E;
const ROLE_SYSTEM_LIST: i32 = 0x21;
const ROLE_SYSTEM_LISTITEM: i32 = 0x22;
const ROLE_SYSTEM_PAGETAB: i32 = 0x25;
const ROLE_SYSTEM_GRAPHIC: i32 = 0x28;
const ROLE_SYSTEM_STATICTEXT: i32 = 0x29;
const ROLE_SYSTEM_TEXT: i32 = 0x2A;
const ROLE_SYSTEM_PUSHBUTTON: i32 = 0x2B;
const ROLE_SYSTEM_CHECKBUTTON: i32 = 0x2C;
const ROLE_SYSTEM_RADIOBUTTON: i32 = 0x2D;
const ROLE_SYSTEM_COMBOBOX: i32 = 0x2E;
const ROLE_SYSTEM_PROGRESSBAR: i32 = 0x30;
const ROLE_SYSTEM_SLIDER: i32 = 0x33;
const ROLE_SYSTEM_BUTTONDROPDOWN: i32 = 0x38;
const ROLE_SYSTEM_BUTTONMENU: i32 = 0x39;
const ROLE_SYSTEM_BUTTONDROPDOWNGRID: i32 = 0x3A;
const ROLE_SYSTEM_PAGETABLIST: i32 = 0x3C;
const ROLE_SYSTEM_SPLITBUTTON: i32 = 0x3E;

const MAX_DEPTH: usize = 25;
const MAX_TOTAL_ELEMENTS: usize = 5000;

/// Walk the MSAA tree for the window with the given HWND. Used as fallback
/// for SAL/VCL targets where the UIA walker would hang.
pub fn walk_msaa_tree(hwnd: u64) -> UiaTreeResult {
    unsafe { walk_unsafe(hwnd) }
}

unsafe fn walk_unsafe(hwnd: u64) -> UiaTreeResult {
    let hwnd_win = HWND(hwnd as *mut _);
    let mut raw_root: *mut std::ffi::c_void = null_mut();
    // AccessibleObjectFromWindow returns IAccessible (as `*mut c_void` typed
    // via the IID we pass). We pass IID_IAccessible.
    let iid_iaccessible = IAccessible::IID;
    let hr = AccessibleObjectFromWindow(
        hwnd_win,
        OBJID_CLIENT,
        &iid_iaccessible,
        &mut raw_root as *mut _ as *mut _,
    );
    if hr.is_err() || raw_root.is_null() {
        return UiaTreeResult {
            tree_markdown: format!(
                "- Window <SAL/VCL — MSAA fallback failed (AccessibleObjectFromWindow hr={hr:?})>\n"
            ),
            nodes: Vec::new(),
        };
    }
    let root: IAccessible = IAccessible::from_raw(raw_root);

    let mut nodes: Vec<UiaNode> = Vec::new();
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut counter = 0usize;
    let mut total = 0usize;

    walk(&root, 0, None, &mut nodes, &mut lines, &mut counter, &mut total);

    let tree_markdown = render_lines(&lines);
    UiaTreeResult { tree_markdown, nodes }
}

#[allow(clippy::too_many_arguments)]
unsafe fn walk(
    acc: &IAccessible,
    depth: usize,
    parent_index: Option<usize>,
    nodes: &mut Vec<UiaNode>,
    lines: &mut Vec<(usize, String)>,
    counter: &mut usize,
    total: &mut usize,
) {
    if depth >= MAX_DEPTH || *total >= MAX_TOTAL_ELEMENTS {
        return;
    }
    *total += 1;

    let self_var: VARIANT = VARIANT::from(0i32); // CHILDID_SELF

    // Properties — each call wrapped to swallow per-element COM errors.
    let role_int: Option<i32> = acc
        .get_accRole(&self_var)
        .ok()
        .and_then(|v| variant_to_i32(&v));
    let name: Option<String> = acc
        .get_accName(&self_var)
        .ok()
        .map(|b| b.to_string())
        .filter(|s| !s.trim().is_empty());
    let default_action: Option<String> = acc
        .get_accDefaultAction(&self_var)
        .ok()
        .map(|b| b.to_string())
        .filter(|s| !s.trim().is_empty());

    // accLocation: out left, top, width, height (screen coords).
    let rect: Option<(i32, i32, i32, i32)> = {
        let mut l = 0i32;
        let mut t = 0i32;
        let mut w = 0i32;
        let mut h = 0i32;
        if acc
            .accLocation(&mut l, &mut t, &mut w, &mut h, &self_var)
            .is_ok()
            && w > 0
            && h > 0
        {
            Some((l, t, l + w, t + h))
        } else {
            None
        }
    };

    let control_type = role_to_control_type(role_int.unwrap_or(0));
    let actions = actions_for(role_int.unwrap_or(0), default_action.as_deref());
    let is_actionable = !actions.is_empty();
    let has_content = name.is_some();

    if is_actionable || has_content {
        // Retain the IAccessible pointer for the cache. Mirror what the UIA
        // walker does: clone, get raw, forget the local — the cache's Drop
        // releases via IUnknown.
        let retained: IAccessible = acc.clone();
        let ptr = retained.as_raw() as usize;
        std::mem::forget(retained);

        let (center_x, center_y) = rect
            .map(|(l, t, r, b)| ((l + r) / 2, (t + b) / 2))
            .unwrap_or((0, 0));

        let node = if is_actionable {
            let idx = *counter;
            *counter += 1;
            UiaNode {
                element_index: Some(idx),
                control_type: control_type.clone(),
                name: name.clone(),
                value: None,
                automation_id: None,
                class_name: Some("MSAA".into()),
                help_text: None,
                enabled: true,
                visible: rect.is_some(),
                selected: None,
                focused: None,
                actions: actions.clone(),
                element_ptr: ptr,
                center_x,
                center_y,
                rect,
                msaa_role: role_int,
                depth,
                parent_element_index: parent_index,
            }
        } else {
            UiaNode {
                element_index: None,
                control_type: control_type.clone(),
                name: name.clone(),
                value: None,
                automation_id: None,
                class_name: Some("MSAA".into()),
                help_text: None,
                enabled: true,
                visible: rect.is_some(),
                selected: None,
                focused: None,
                actions: Vec::new(),
                element_ptr: ptr,
                center_x: 0,
                center_y: 0,
                rect,
                msaa_role: role_int,
                depth,
                parent_element_index: parent_index,
            }
        };
        // Track this node as the parent_index for its descendants only when
        // it received an element_index (mirrors what the markdown shows:
        // only indexed rows are addressable).
        let next_parent = node.element_index.or(parent_index);
        lines.push((depth, crate::uia::format_node_line(&node)));
        nodes.push(node);

        // Recurse via accChildCount + get_accChild.
        let child_count: i32 = acc.accChildCount().unwrap_or(0);
        for i in 1..=child_count {
            let child_var = VARIANT::from(i);
            // accChild returns IDispatch — query for IAccessible.
            if let Ok(child_disp) = acc.get_accChild(&child_var) {
                if let Ok(child_acc) = child_disp.cast::<IAccessible>() {
                    walk(&child_acc, depth + 1, next_parent, nodes, lines, counter, total);
                }
            }
        }
        return;
    }

    // Non-emitting path (filtered out by !is_actionable && !has_content):
    // still recurse, propagating the same parent_index.
    let child_count: i32 = acc.accChildCount().unwrap_or(0);
    for i in 1..=child_count {
        let child_var = VARIANT::from(i);
        if let Ok(child_disp) = acc.get_accChild(&child_var) {
            if let Ok(child_acc) = child_disp.cast::<IAccessible>() {
                walk(&child_acc, depth + 1, parent_index, nodes, lines, counter, total);
            }
        }
    }
}

/// Extract i32 from a VARIANT — accRole returns VT_I4 in practice (sometimes
/// VT_BSTR for custom roles, which we map to 0 = unknown).
unsafe fn variant_to_i32(v: &VARIANT) -> Option<i32> {
    let raw = v.as_raw();
    let vt = raw.Anonymous.Anonymous.vt;
    if vt == 3 {
        // VT_I4
        Some(raw.Anonymous.Anonymous.Anonymous.lVal)
    } else {
        None
    }
}

/// Map MSAA role id → control_type string. For roles not in this list we
/// emit `Role_<hex>` so the agent at least sees something diagnostic.
fn role_to_control_type(role: i32) -> String {
    match role {
        ROLE_SYSTEM_TITLEBAR => "TitleBar",
        ROLE_SYSTEM_MENUBAR => "MenuBar",
        ROLE_SYSTEM_SCROLLBAR => "ScrollBar",
        ROLE_SYSTEM_WINDOW => "Window",
        ROLE_SYSTEM_CLIENT => "Pane",
        ROLE_SYSTEM_MENUPOPUP => "Menu",
        ROLE_SYSTEM_MENUITEM => "MenuItem",
        ROLE_SYSTEM_TOOLTIP => "ToolTip",
        ROLE_SYSTEM_DIALOG => "Window",
        ROLE_SYSTEM_GROUPING => "Group",
        ROLE_SYSTEM_TOOLBAR => "ToolBar",
        ROLE_SYSTEM_STATUSBAR => "StatusBar",
        ROLE_SYSTEM_LINK => "Hyperlink",
        ROLE_SYSTEM_LIST => "List",
        ROLE_SYSTEM_LISTITEM => "ListItem",
        ROLE_SYSTEM_PAGETAB => "TabItem",
        ROLE_SYSTEM_PAGETABLIST => "Tab",
        ROLE_SYSTEM_GRAPHIC => "Image",
        ROLE_SYSTEM_STATICTEXT => "Text",
        ROLE_SYSTEM_TEXT => "Edit",
        ROLE_SYSTEM_PUSHBUTTON => "Button",
        ROLE_SYSTEM_CHECKBUTTON => "CheckBox",
        ROLE_SYSTEM_RADIOBUTTON => "RadioButton",
        ROLE_SYSTEM_COMBOBOX => "ComboBox",
        ROLE_SYSTEM_PROGRESSBAR => "ProgressBar",
        ROLE_SYSTEM_SLIDER => "Slider",
        ROLE_SYSTEM_BUTTONDROPDOWN
        | ROLE_SYSTEM_BUTTONMENU
        | ROLE_SYSTEM_BUTTONDROPDOWNGRID
        | ROLE_SYSTEM_SPLITBUTTON => "SplitButton",
        0 => "Unknown",
        other => return format!("Role_0x{:X}", other),
    }
    .into()
}

/// Compute `actions=[...]` for a MSAA element. Roles with a meaningful
/// default action get `invoke`. Dropdown-flavored roles ALSO get `expand`
/// so callers can address the dropdown half separately — the click tool
/// routes `action:"expand"` to a right-edge SendInput click rather than
/// just calling `accDoDefaultAction` (which fires the press half).
fn actions_for(role: i32, default_action: Option<&str>) -> Vec<String> {
    let has_action = default_action
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let mut actions = Vec::new();

    let is_dropdown = matches!(
        role,
        ROLE_SYSTEM_BUTTONDROPDOWN
            | ROLE_SYSTEM_BUTTONMENU
            | ROLE_SYSTEM_BUTTONDROPDOWNGRID
            | ROLE_SYSTEM_SPLITBUTTON
    );
    let is_clickable = matches!(
        role,
        ROLE_SYSTEM_PUSHBUTTON
            | ROLE_SYSTEM_CHECKBUTTON
            | ROLE_SYSTEM_RADIOBUTTON
            | ROLE_SYSTEM_LINK
            | ROLE_SYSTEM_MENUITEM
            | ROLE_SYSTEM_LISTITEM
            | ROLE_SYSTEM_PAGETAB
            | ROLE_SYSTEM_COMBOBOX
    );

    if has_action || is_dropdown || is_clickable {
        actions.push("invoke".into());
    }
    if is_dropdown {
        actions.push("expand".into());
    }
    actions
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
