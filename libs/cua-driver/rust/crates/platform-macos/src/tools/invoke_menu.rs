use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_contract::InvokeMenuInput;
use cua_driver_core::{
    action_record::{
        ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
        EvidenceKind, RequestedDelivery,
    },
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    time::Duration,
};

use crate::ax::bindings::{
    ax_get_window_id, copy_action_names, copy_ax_windows, copy_bool_attr, copy_children,
    copy_element_attr, copy_number_attr, copy_string_attr, kAXErrorSuccess, perform_action,
    set_bool_attr_true, AXUIElementCreateApplication, AXUIElementRef,
    AXUIElementSetMessagingTimeout,
};

pub struct InvokeMenuTool;

const AX_MESSAGING_TIMEOUT_SECONDS: f32 = 2.0;
const MAIN_QUEUE_TIMEOUT: Duration = Duration::from_secs(5);

unsafe fn set_messaging_timeout(element: AXUIElementRef) {
    let _ = AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECONDS);
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| {
        let contract =
            cua_driver_contract::tool_contract("invoke_menu").expect("invoke_menu contract");
        ToolDef {
            name: contract.name,
            description: contract.description,
            input_schema: contract.input_schema,
            read_only: contract.annotations.read_only,
            destructive: contract.annotations.destructive,
            idempotent: contract.annotations.idempotent,
            open_world: contract.annotations.open_world,
        }
    })
}

fn normalized_path(path: Vec<String>) -> Result<Vec<String>, String> {
    if path.is_empty() || path.len() > 16 {
        return Err("invoke_menu: path must contain between 1 and 16 segments".into());
    }
    path.into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let segment = segment.trim();
            if segment.is_empty() {
                Err(format!("invoke_menu: path segment {index} is empty"))
            } else {
                Ok(segment.to_owned())
            }
        })
        .collect()
}

#[derive(Debug, Clone, serde::Serialize)]
struct MenuItem {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    has_submenu: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    shortcut: Option<String>,
}

#[derive(Debug)]
struct MenuRefusal {
    message: String,
    failed_segment: Option<usize>,
    items: Vec<MenuItem>,
}

impl MenuRefusal {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            failed_segment: None,
            items: Vec::new(),
        }
    }

    fn at_segment(depth: usize, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            failed_segment: Some(depth),
            items: Vec::new(),
        }
    }

    fn with_items(mut self, items: Vec<MenuItem>) -> Self {
        self.items = items;
        self
    }
}

#[derive(Debug)]
enum MenuOutcome {
    Invoked,
    Listed(Vec<MenuItem>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hop {
    List,
    Press(&'static str),
}

const MENU_MODIFIER_SHIFT: i64 = 1;
const MENU_MODIFIER_OPTION: i64 = 1 << 1;
const MENU_MODIFIER_CONTROL: i64 = 1 << 2;
const MENU_MODIFIER_NO_COMMAND: i64 = 1 << 3;

fn menu_shortcut(cmd_char: Option<&str>, modifiers: Option<f64>) -> Option<String> {
    let key = cmd_char?.trim();
    if key.is_empty() {
        return None;
    }
    let mask = modifiers.unwrap_or_default() as i64;
    let mut rendered = String::new();
    if mask & MENU_MODIFIER_CONTROL != 0 {
        rendered.push('⌃');
    }
    if mask & MENU_MODIFIER_OPTION != 0 {
        rendered.push('⌥');
    }
    if mask & MENU_MODIFIER_SHIFT != 0 {
        rendered.push('⇧');
    }
    if mask & MENU_MODIFIER_NO_COMMAND == 0 {
        rendered.push('⌘');
    }
    rendered.push_str(&key.to_uppercase());
    Some(rendered)
}

fn listable_title(raw: Option<String>) -> Option<String> {
    let title = raw?.trim().to_owned();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

/// Return the semantic menu children of an AX node. AppKit inserts an
/// untitled `AXMenu` container between a menu-bar/submenu item and its items;
/// callers express paths in visible labels, so that container is transparent.
unsafe fn semantic_children(parent: AXUIElementRef) -> Vec<AXUIElementRef> {
    let mut out = Vec::new();
    for child in copy_children(parent) {
        if copy_string_attr(child, "AXRole").as_deref() == Some("AXMenu") {
            out.extend(copy_children(child));
            CFRelease(child as CFTypeRef);
        } else {
            out.push(child);
        }
    }
    out
}

unsafe fn has_submenu(item: AXUIElementRef) -> bool {
    let children = copy_children(item);
    let present = !children.is_empty();
    for child in children {
        CFRelease(child as CFTypeRef);
    }
    present
}

unsafe fn describe_item(item: AXUIElementRef) -> Option<MenuItem> {
    Some(MenuItem {
        title: listable_title(copy_string_attr(item, "AXTitle"))?,
        enabled: copy_bool_attr(item, "AXEnabled"),
        has_submenu: has_submenu(item),
        shortcut: menu_shortcut(
            copy_string_attr(item, "AXMenuItemCmdChar").as_deref(),
            copy_number_attr(item, "AXMenuItemCmdModifiers"),
        ),
    })
}

unsafe fn describe_items(parent: AXUIElementRef) -> Vec<MenuItem> {
    let children = semantic_children(parent);
    let mut items = Vec::with_capacity(children.len());
    for child in children {
        if let Some(item) = describe_item(child) {
            items.push(item);
        }
        CFRelease(child as CFTypeRef);
    }
    items
}

unsafe fn resolve_exact_prefix(
    menu_bar: AXUIElementRef,
    prefix: &[String],
) -> Result<AXUIElementRef, MenuRefusal> {
    let mut current = menu_bar;
    let mut owns_current = false;

    for (depth, segment) in prefix.iter().enumerate() {
        let children = semantic_children(current);
        if owns_current {
            CFRelease(current as CFTypeRef);
        }

        let matched: Vec<usize> = children
            .iter()
            .enumerate()
            .filter(|(_, child)| {
                copy_string_attr(**child, "AXTitle")
                    .unwrap_or_default()
                    .trim()
                    == segment
            })
            .map(|(index, _)| index)
            .collect();

        if matched.len() != 1 {
            let message = if matched.is_empty() {
                format!("invoke_menu: path segment {depth} was not found")
            } else {
                format!("invoke_menu: path segment {depth} is ambiguous")
            };
            let items = children
                .iter()
                .filter_map(|child| describe_item(*child))
                .collect();
            for child in children {
                CFRelease(child as CFTypeRef);
            }
            return Err(MenuRefusal::at_segment(depth, message).with_items(items));
        }

        let keep = matched[0];
        for (index, child) in children.iter().enumerate() {
            if index != keep {
                CFRelease(*child as CFTypeRef);
            }
        }
        current = children[keep];
        owns_current = true;
    }

    if owns_current {
        Ok(current)
    } else {
        Err(MenuRefusal::plain("invoke_menu: path is empty"))
    }
}

fn choose_action(actions: &[String], final_segment: bool) -> Option<&'static str> {
    let supports = |name: &str| actions.iter().any(|action| action == name);
    let order: &[&str] = if final_segment {
        &["AXPress", "AXPick", "AXConfirm"]
    } else {
        &["AXPress", "AXPick", "AXShowMenu", "AXOpen"]
    };
    order.iter().copied().find(|action| supports(action))
}

fn plan_hop(actions: &[String], final_segment: bool, has_submenu: bool) -> Option<Hop> {
    if final_segment && has_submenu {
        return Some(Hop::List);
    }
    choose_action(actions, final_segment).map(Hop::Press)
}

unsafe fn menu_bar_of(app: AXUIElementRef) -> Option<AXUIElementRef> {
    let menu_bar = copy_element_attr(app, "AXMenuBar")?;
    set_messaging_timeout(menu_bar);
    Some(menu_bar)
}

unsafe fn peek_submenu(app: AXUIElementRef, path: &[String]) -> Option<Vec<MenuItem>> {
    let menu_bar = menu_bar_of(app)?;
    let resolved = resolve_exact_prefix(menu_bar, path);
    CFRelease(menu_bar as CFTypeRef);
    let target = resolved.ok()?;
    set_messaging_timeout(target);
    let items = has_submenu(target)
        .then(|| describe_items(target))
        .filter(|items| !items.is_empty());
    CFRelease(target as CFTypeRef);
    items
}

unsafe fn cancel_menu(item: AXUIElementRef) {
    if copy_action_names(item)
        .iter()
        .any(|name| name == "AXCancel")
    {
        let _ = perform_action(item, "AXCancel");
        return;
    }
    for child in copy_children(item) {
        if copy_string_attr(child, "AXRole").as_deref() == Some("AXMenu")
            && copy_action_names(child)
                .iter()
                .any(|name| name == "AXCancel")
        {
            let _ = perform_action(child, "AXCancel");
        }
        CFRelease(child as CFTypeRef);
    }
}

unsafe fn dismiss_opened_menus(app: AXUIElementRef, root: &[String]) {
    let Some(menu_bar) = menu_bar_of(app) else {
        return;
    };
    let resolved = resolve_exact_prefix(menu_bar, root);
    CFRelease(menu_bar as CFTypeRef);
    let Ok(item) = resolved else {
        return;
    };
    set_messaging_timeout(item);
    cancel_menu(item);
    CFRelease(item as CFTypeRef);
}

unsafe fn press_each_segment(
    app: AXUIElementRef,
    path: &[String],
) -> Result<MenuOutcome, MenuRefusal> {
    for depth in 0..path.len() {
        let final_segment = depth + 1 == path.len();
        // Resolve from the live app root for every hop. Opening a menu can
        // replace its AX objects and reorder unrelated snapshot indices.
        let menu_bar = menu_bar_of(app)
            .ok_or_else(|| MenuRefusal::plain("invoke_menu: target exposes no AXMenuBar"))?;
        let target = resolve_exact_prefix(menu_bar, &path[..=depth]);
        CFRelease(menu_bar as CFTypeRef);
        let target = target?;
        set_messaging_timeout(target);

        if copy_bool_attr(target, "AXEnabled") == Some(false) {
            CFRelease(target as CFTypeRef);
            return Err(MenuRefusal::at_segment(
                depth,
                format!("invoke_menu: path segment {depth} is disabled"),
            ));
        }

        let actions = copy_action_names(target);
        let action = match plan_hop(
            &actions,
            final_segment,
            final_segment && has_submenu(target),
        ) {
            Some(Hop::List) => {
                let items = describe_items(target);
                CFRelease(target as CFTypeRef);
                if items.is_empty() {
                    return Err(MenuRefusal::at_segment(
                        depth,
                        format!(
                            "invoke_menu: path segment {depth} opens a submenu whose items accessibility does not expose"
                        ),
                    ));
                }
                return Ok(MenuOutcome::Listed(items));
            }
            Some(Hop::Press(action)) => action,
            None => {
                CFRelease(target as CFTypeRef);
                return Err(MenuRefusal::at_segment(
                    depth,
                    format!("invoke_menu: path segment {depth} has no usable native menu action"),
                ));
            }
        };

        let error = perform_action(target, action);
        CFRelease(target as CFTypeRef);
        if error != kAXErrorSuccess {
            return Err(MenuRefusal::at_segment(
                depth,
                format!(
                    "invoke_menu: native action for path segment {depth} failed with AX error {error}"
                ),
            ));
        }
        if !final_segment {
            std::thread::sleep(Duration::from_millis(80));
        }
    }
    Ok(MenuOutcome::Invoked)
}

unsafe fn invoke_path(pid: i32, path: &[String]) -> Result<MenuOutcome, MenuRefusal> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(MenuRefusal::plain(
            "invoke_menu: target application is unavailable",
        ));
    }
    set_messaging_timeout(app);

    let result = match peek_submenu(app, path) {
        Some(items) => Ok(MenuOutcome::Listed(items)),
        None => {
            let outcome = press_each_segment(app, path);
            if !matches!(outcome, Ok(MenuOutcome::Invoked)) && path.len() > 1 {
                dismiss_opened_menus(app, &path[..1]);
            }
            outcome
        }
    };

    CFRelease(app as CFTypeRef);
    result
}

/// Make one exact application window key before resolving focus-sensitive
/// native menu state.
///
/// `SLPSSetFrontProcessWithOptions(..., kCPSNoWindows)` makes the application
/// active without broadly raising its windows. That is the right default for
/// input delivery, but it can leave the requested window non-key. macOS then
/// exposes contextual Window-menu commands (including Move & Resize) as
/// disabled even though the application itself is frontmost. Raise and mark
/// only the requested AX window, then require an exact focused-window readback
/// before menu resolution proceeds.
fn focus_ax_window(pid: i32, window_id: u32) -> Result<(), String> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err("invoke_menu: target application is unavailable".into());
        }
        set_messaging_timeout(app);

        let mut target = None;
        for window in copy_ax_windows(app) {
            if target.is_none() && ax_get_window_id(window) == Some(window_id) {
                target = Some(window);
            } else {
                CFRelease(window as CFTypeRef);
            }
        }
        CFRelease(app as CFTypeRef);

        let Some(target) = target else {
            return Err("invoke_menu: target accessibility window is unavailable".into());
        };
        set_messaging_timeout(target);

        // SkyLight has requested native key status for this exact window. AX
        // raise/main/focus completes the corresponding visible and semantic
        // state; these writes remain best-effort for applications that expose
        // only a subset of the attributes.
        let _ = perform_action(target, "AXRaise");
        let _ = set_bool_attr_true(target, "AXMain");
        let _ = set_bool_attr_true(target, "AXFocused");
        CFRelease(target as CFTypeRef);
    }
    Ok(())
}

struct AxWindowFocusRequest {
    pid: i32,
    window_id: u32,
    tx: SyncSender<Result<(), String>>,
    cancelled: Arc<AtomicBool>,
}

#[link(name = "System", kind = "framework")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

unsafe extern "C" fn focus_ax_window_on_main(context: *mut c_void) {
    let request = unsafe { Box::from_raw(context.cast::<AxWindowFocusRequest>()) };
    if request.cancelled.load(Ordering::Acquire) {
        return;
    }
    let result = focus_ax_window(request.pid, request.window_id);
    let _ = request.tx.send(result);
}

fn focus_ax_window_with_thread_affinity(pid: i32, window_id: u32) -> Result<(), String> {
    let is_main_thread = objc2_foundation::MainThreadMarker::new().is_some();
    if pid != std::process::id() as i32 || is_main_thread {
        return focus_ax_window(pid, window_id);
    }

    // AX actions against another process execute in that process. An embedded
    // driver targeting its own window is different: AppKit services AXRaise
    // in this process, and window ordering is main-thread-only. Queue just the
    // self-process AX mutation onto AppKit's main queue, then return to the
    // blocking worker for readiness polling and menu traversal.
    let (tx, rx) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let request = Box::new(AxWindowFocusRequest {
        pid,
        window_id,
        tx,
        cancelled: Arc::clone(&cancelled),
    });
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(request).cast::<c_void>(),
            focus_ax_window_on_main,
        );
    }
    match rx.recv_timeout(MAIN_QUEUE_TIMEOUT) {
        Ok(result) => result,
        Err(error) => {
            // If AppKit never serviced the request, prevent a stale focus
            // change from firing after this tool call has already failed.
            cancelled.store(true, Ordering::Release);
            Err(format!(
                "invoke_menu: failed waiting for the embedded host window on the AppKit main queue: {error}"
            ))
        }
    }
}

fn focus_exact_window(pid: i32, window_id: u32) -> Result<(), String> {
    let native_key_requested = crate::input::skylight::make_exact_window_key(pid, window_id);
    if !native_key_requested
        && crate::apps::frontmost_pid() != Some(pid)
        && !crate::apps::activate_pid(pid)
    {
        return Err("invoke_menu: target application could not be activated".into());
    }
    if !native_key_requested {
        std::thread::sleep(Duration::from_millis(120));
    }

    focus_ax_window_with_thread_affinity(pid, window_id)?;

    // AXFocusedWindow can lead AppKit's native `isKeyWindow` state while the
    // WindowServer activation is still settling. Menu validation observes the
    // latter. Require the exact app/window pair to remain stable briefly so a
    // loaded desktop cannot expose a transiently disabled contextual item.
    let deadline = std::time::Instant::now() + Duration::from_millis(800);
    let mut stable_since = None;
    loop {
        let now = std::time::Instant::now();
        if exact_window_is_ready(
            crate::apps::frontmost_pid(),
            pid,
            crate::ax::bindings::focused_window_id_of_pid(pid),
            window_id,
        ) {
            let since = stable_since.get_or_insert(now);
            if now.duration_since(*since) >= Duration::from_millis(120) {
                return Ok(());
            }
        } else {
            stable_since = None;
        }
        if now >= deadline {
            return Err(format!(
                "invoke_menu: target window {window_id} did not become stably key and frontmost"
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn exact_window_is_ready(
    frontmost_pid: Option<i32>,
    target_pid: i32,
    focused_window_id: Option<u32>,
    target_window_id: u32,
) -> bool {
    frontmost_pid == Some(target_pid) && focused_window_id == Some(target_window_id)
}

fn refusal(details: MenuRefusal) -> ToolResult {
    let mut refused = serde_json::json!({
        "code": "menu_path_unavailable",
        "message": details.message.clone(),
    });
    if let Some(segment) = details.failed_segment {
        refused["failed_segment"] = serde_json::json!(segment);
    }
    if !details.items.is_empty() {
        refused["items"] = serde_json::json!(details.items);
    }
    ToolResult::error(details.message).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": refused,
    }))
}

fn listed(resolved_path: Vec<String>, items: Vec<MenuItem>) -> ToolResult {
    ToolResult::text(format!(
        "Resolved the live native menu path to a submenu and listed its {} items without dispatching anything; extend the path with one of those titles to invoke a command.",
        items.len()
    ))
    .with_structured(serde_json::json!({
        "status": cua_driver_contract::LISTING_STATUS,
        "resolved_path": resolved_path,
        "items": items,
    }))
}

fn invoked() -> ToolResult {
    ToolResult::text(
        "Resolved the live native menu path and dispatched its final accessibility action; verify the command's semantic effect from fresh state.",
    )
    .with_action_record(
        ActionExecutionRecord::builder(
            ActionEffect::Unverifiable,
            ActionTransport::MacosAxAction,
            RequestedDelivery::Foreground,
        )
        .actual_delivery(ActualDelivery::Foreground)
        .evidence(ActionEvidence {
            kind: EvidenceKind::NativeApiResult,
            detail: "Every menu hop resolved uniquely and AX accepted the final action".into(),
        })
        .build()
        .expect("invoke_menu record is valid"),
    )
}

#[async_trait]
impl Tool for InvokeMenuTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input: InvokeMenuInput =
            match cua_driver_core::tool_args::parse_typed_input("invoke_menu", args) {
                Ok(input) => input,
                Err(result) => return result,
            };
        let path = match normalized_path(input.path) {
            Ok(path) => path,
            Err(error) => return refusal(MenuRefusal::plain(error)),
        };
        let pid = match i32::try_from(input.pid) {
            Ok(pid) => pid,
            Err(_) => return refusal(MenuRefusal::plain("invoke_menu: pid is out of range")),
        };
        let window_id = match u32::try_from(input.window_id) {
            Ok(window_id) => window_id,
            Err(_) => return refusal(MenuRefusal::plain("invoke_menu: window_id is out of range")),
        };
        if !crate::windows::all_windows()
            .iter()
            .any(|window| window.pid == pid && window.window_id == window_id)
        {
            return refusal(MenuRefusal::plain(
                "invoke_menu: window_id does not belong to pid",
            ));
        }
        let resolved_path = path.clone();

        let outcome = tokio::task::spawn_blocking(move || {
            let prior_frontmost = crate::apps::frontmost_pid();
            let prior_frontmost_window =
                prior_frontmost.and_then(crate::ax::bindings::focused_window_id_of_pid);
            let needs_activation = prior_frontmost != Some(pid);

            let result = match focus_exact_window(pid, window_id) {
                Ok(()) => unsafe { invoke_path(pid, &path) },
                Err(error) => Err(MenuRefusal::plain(error)),
            };

            // Restore the exact prior key window when one was observable,
            // including across applications. Falling back to app activation
            // preserves the previous behavior for apps without an AX window.
            if let Some(prior_pid) = prior_frontmost {
                let already_restored =
                    prior_pid == pid && prior_frontmost_window == Some(window_id);
                if !already_restored {
                    let restored_exact = prior_frontmost_window.is_some_and(|prior_window_id| {
                        focus_exact_window(prior_pid, prior_window_id).is_ok()
                    });
                    if needs_activation && !restored_exact {
                        let _ = crate::apps::activate_pid(prior_pid);
                    }
                }
            }
            result
        })
        .await;

        match outcome {
            Ok(Ok(MenuOutcome::Invoked)) => invoked(),
            Ok(Ok(MenuOutcome::Listed(items))) => listed(resolved_path, items),
            Ok(Err(refused)) => refusal(refused),
            Err(error) => refusal(MenuRefusal::plain(format!(
                "invoke_menu: blocking task failed: {error}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_normalization_rejects_empty_segments() {
        assert_eq!(
            normalized_path(vec![" Window ".into(), " Left ".into()]).unwrap(),
            vec!["Window", "Left"]
        );
        assert!(normalized_path(vec!["Window".into(), "  ".into()]).is_err());
    }

    #[test]
    fn action_selection_is_explicit_and_ordered() {
        let actions = vec!["AXShowMenu".into(), "AXPress".into()];
        assert_eq!(choose_action(&actions, false), Some("AXPress"));
        assert_eq!(choose_action(&actions, true), Some("AXPress"));
        assert_eq!(choose_action(&["AXShowMenu".into()], true), None);
    }

    #[test]
    fn a_final_segment_that_opens_a_submenu_is_listed_instead_of_pressed() {
        let pressable = vec!["AXPress".to_owned()];
        assert_eq!(plan_hop(&pressable, true, true), Some(Hop::List));
        assert_eq!(plan_hop(&[], true, true), Some(Hop::List));
        assert_eq!(
            plan_hop(&pressable, true, false),
            Some(Hop::Press("AXPress"))
        );
        assert_eq!(
            plan_hop(&pressable, false, true),
            Some(Hop::Press("AXPress"))
        );
        assert_eq!(plan_hop(&["AXShowMenu".to_owned()], true, false), None);
    }

    #[test]
    fn a_menu_shortcut_renders_the_modifiers_macos_reports() {
        assert_eq!(menu_shortcut(Some("s"), Some(0.0)).as_deref(), Some("⌘S"));
        assert_eq!(menu_shortcut(Some("s"), None).as_deref(), Some("⌘S"));
        assert_eq!(menu_shortcut(Some("s"), Some(1.0)).as_deref(), Some("⇧⌘S"));
        assert_eq!(menu_shortcut(Some("["), Some(2.0)).as_deref(), Some("⌥⌘["));
        assert_eq!(
            menu_shortcut(Some("d"), Some(7.0)).as_deref(),
            Some("⌃⌥⇧⌘D")
        );
        assert_eq!(menu_shortcut(Some("f"), Some(8.0)).as_deref(), Some("F"));
        assert_eq!(menu_shortcut(Some("f"), Some(12.0)).as_deref(), Some("⌃F"));
        assert_eq!(menu_shortcut(Some("  "), Some(0.0)), None);
        assert_eq!(menu_shortcut(None, Some(0.0)), None);
    }

    #[test]
    fn an_untitled_row_is_not_a_listable_item() {
        assert_eq!(
            listable_title(Some("  Move Down ".into())).as_deref(),
            Some("Move Down")
        );
        assert_eq!(listable_title(Some(String::new())), None);
        assert_eq!(listable_title(Some("   ".into())), None);
        assert_eq!(listable_title(None), None);
    }

    #[test]
    fn a_listed_submenu_publishes_its_items_as_a_success() {
        let result = listed(
            vec!["View".into(), "Code Folding".into()],
            vec![
                MenuItem {
                    title: "Fold".into(),
                    enabled: Some(true),
                    has_submenu: false,
                    shortcut: Some("⌥⌘F".into()),
                },
                MenuItem {
                    title: "More".into(),
                    enabled: None,
                    has_submenu: true,
                    shortcut: None,
                },
            ],
        );
        assert_eq!(result.is_error, None);
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["status"], "listed");
        assert_eq!(
            structured["resolved_path"],
            serde_json::json!(["View", "Code Folding"])
        );
        assert_eq!(structured["items"][0]["title"], "Fold");
        assert_eq!(structured["items"][0]["enabled"], true);
        assert_eq!(structured["items"][0]["has_submenu"], false);
        assert_eq!(structured["items"][0]["shortcut"], "⌥⌘F");
        assert_eq!(structured["items"][1]["has_submenu"], true);
        assert!(structured["items"][1].get("enabled").is_none());
        assert!(structured["items"][1].get("shortcut").is_none());
    }

    #[test]
    fn a_listed_submenu_is_a_non_action_outcome_the_runtime_accepts() {
        let structured = listed(
            vec!["Card".into(), "Add Field".into()],
            vec![MenuItem {
                title: "Phone".into(),
                enabled: Some(true),
                has_submenu: false,
                shortcut: None,
            }],
        )
        .structured_content
        .expect("structured content");
        assert!(cua_driver_contract::is_action_listing_output(
            "invoke_menu",
            &structured
        ));
        assert_eq!(
            cua_driver_contract::validate_success_output("invoke_menu", structured),
            Ok(false)
        );
    }

    #[test]
    fn a_pressed_menu_item_still_carries_its_execution_record() {
        let pressed = invoked();
        assert_eq!(pressed.is_error, None);
        assert!(pressed.structured_content.is_none());
        let public = serde_json::to_value(
            pressed
                .action_record
                .expect("execution record")
                .public_result()
                .expect("public projection"),
        )
        .expect("projection serializes");
        assert_eq!(public["effect"], "unverifiable");
        assert_eq!(public["route"], "accessibility");
        assert_eq!(public["delivery"]["mode"], "foreground");
    }

    #[test]
    fn an_unresolved_segment_names_the_level_it_failed_at() {
        let result = refusal(
            MenuRefusal::at_segment(1, "invoke_menu: path segment 1 was not found").with_items(
                vec![MenuItem {
                    title: "Bold".into(),
                    enabled: Some(false),
                    has_submenu: false,
                    shortcut: None,
                }],
            ),
        );
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["status"], "refused");
        assert_eq!(structured["refusal"]["code"], "menu_path_unavailable");
        assert_eq!(structured["refusal"]["failed_segment"], 1);
        assert_eq!(structured["refusal"]["items"][0]["title"], "Bold");
        assert_eq!(structured["refusal"]["items"][0]["enabled"], false);

        let plain = refusal(MenuRefusal::plain("invoke_menu: pid is out of range"))
            .structured_content
            .expect("structured content");
        assert_eq!(
            plain["refusal"]["message"],
            "invoke_menu: pid is out of range"
        );
        assert!(plain["refusal"].get("failed_segment").is_none());
        assert!(plain["refusal"].get("items").is_none());
    }

    #[test]
    fn menu_focus_requires_the_exact_frontmost_app_and_window() {
        assert!(exact_window_is_ready(Some(7), 7, Some(42), 42));
        assert!(!exact_window_is_ready(Some(8), 7, Some(42), 42));
        assert!(!exact_window_is_ready(Some(7), 7, Some(41), 42));
        assert!(!exact_window_is_ready(Some(7), 7, None, 42));
    }
}
