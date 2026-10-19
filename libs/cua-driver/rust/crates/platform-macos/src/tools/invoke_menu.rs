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
    copy_element_attr, copy_string_attr, kAXErrorSuccess, perform_action, set_bool_attr_true,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetMessagingTimeout,
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

/// A menu title as paths compare it: trimmed, with three periods read as
/// the ellipsis character macOS menu titles use ("Save As..." finds
/// "Save As…").
fn menu_title_key(title: &str) -> String {
    title.trim().replace("...", "\u{2026}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitleMatch {
    One(usize),
    Ambiguous,
    None,
}

/// Find a path segment among the titles of one menu level: an exact title
/// first, then a title that differs only in letter case. Anything looser
/// (word overlap, prefixes) is left to the caller, who sees the titles in
/// the error, because a wrong guess would run a different command.
fn pick_title(titles: &[String], segment: &str) -> TitleMatch {
    let wanted = menu_title_key(segment);
    let pick = |same: &dyn Fn(&str) -> bool| {
        let mut hits = titles
            .iter()
            .enumerate()
            .filter(|(_, title)| same(&menu_title_key(title)))
            .map(|(index, _)| index);
        match (hits.next(), hits.next()) {
            (Some(index), None) => TitleMatch::One(index),
            (Some(_), Some(_)) => TitleMatch::Ambiguous,
            _ => TitleMatch::None,
        }
    };
    match pick(&|title| title == wanted) {
        TitleMatch::None => {
            let wanted = wanted.to_lowercase();
            pick(&|title| title.to_lowercase() == wanted)
        }
        found => found,
    }
}

/// Most items a missed segment lists, so a long menu cannot flood the reply.
const MAX_LISTED_TITLES: usize = 40;

/// A missed segment names what was asked for, where, and the items that are
/// there, so the next call can use the real title instead of a full tree
/// read (LibreOffice calls it "To Next Sheet", not "Next Sheet").
fn missing_segment_error(depth: usize, prefix: &[String], titles: &[String]) -> String {
    let wanted = &prefix[depth];
    let location = if depth == 0 {
        "in the menu bar".to_owned()
    } else {
        format!("under \"{}\"", prefix[..depth].join(" > "))
    };
    let mut items: Vec<&str> = titles
        .iter()
        .map(|title| title.trim())
        .filter(|title| !title.is_empty())
        .collect();
    items.dedup();
    if items.is_empty() {
        return format!(
            "invoke_menu: path segment {depth} \"{wanted}\" was not found {location}, which lists no titled items (the app may fill this menu only while it is open)"
        );
    }
    let more = items.len().saturating_sub(MAX_LISTED_TITLES);
    let listed = items
        .iter()
        .take(MAX_LISTED_TITLES)
        .map(|title| format!("\"{title}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    format!(
        "invoke_menu: path segment {depth} \"{wanted}\" was not found {location}. Items there: {listed}{more}"
    )
}

unsafe fn resolve_exact_prefix(
    menu_bar: AXUIElementRef,
    prefix: &[String],
) -> Result<AXUIElementRef, String> {
    let mut current = menu_bar;
    let mut owns_current = false;

    for (depth, segment) in prefix.iter().enumerate() {
        let children = semantic_children(current);
        if owns_current {
            CFRelease(current as CFTypeRef);
        }

        let titles: Vec<String> = children
            .iter()
            .map(|&child| copy_string_attr(child, "AXTitle").unwrap_or_default())
            .collect();
        let picked = pick_title(&titles, segment);
        let mut chosen = None;
        for (index, child) in children.into_iter().enumerate() {
            if matches!(picked, TitleMatch::One(i) if i == index) {
                chosen = Some(child);
            } else {
                CFRelease(child as CFTypeRef);
            }
        }

        match (picked, chosen) {
            (TitleMatch::One(_), Some(child)) => current = child,
            (TitleMatch::Ambiguous, _) => {
                return Err(format!("invoke_menu: path segment {depth} is ambiguous"));
            }
            _ => return Err(missing_segment_error(depth, prefix, &titles)),
        }
        owns_current = true;
    }

    if owns_current {
        Ok(current)
    } else {
        Err("invoke_menu: path is empty".into())
    }
}

/// Close the menu a failed path opened from the menu bar item `top` with
/// AXCancel on that item's menu (which also ends its submenus), and read the
/// result back from WindowServer's window list. Menu windows in
/// `menus_before` were on screen before this call and do not count.
unsafe fn close_opened_menu(
    app: AXUIElementRef,
    pid: i32,
    top: &str,
    menus_before: &[u32],
) -> Option<bool> {
    // An action that reported an error can still open its menu a moment
    // later; give it the same settle time as a hop before looking.
    std::thread::sleep(Duration::from_millis(80));
    if crate::windows::new_menu_windows(pid, menus_before)? == 0 {
        return Some(true);
    }
    let item = copy_element_attr(app, "AXMenuBar").and_then(|bar| {
        set_messaging_timeout(bar);
        let item = resolve_exact_prefix(bar, std::slice::from_ref(&top.to_owned())).ok();
        CFRelease(bar as CFTypeRef);
        item
    });
    let Some(item) = item else {
        return Some(false);
    };
    set_messaging_timeout(item);
    for child in copy_children(item) {
        set_messaging_timeout(child);
        if copy_string_attr(child, "AXRole").as_deref() == Some("AXMenu") {
            let _ = perform_action(child, "AXCancel");
        }
        CFRelease(child as CFTypeRef);
    }
    CFRelease(item as CFTypeRef);
    crate::windows::wait_for_new_menus_closed(pid, menus_before)
}

/// The refusal for a path that failed after pressing a menu item, saying
/// whether the menu it opened was seen to close.
fn failure_after_press(error: String, top: &str, closed: Option<bool>) -> String {
    match closed {
        Some(true) => format!("{error}. No menu window this call opened is still on screen."),
        Some(false) | None => format!(
            "{error}. The {top} menu this call opened may still be open: press escape on the window before other input."
        ),
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

unsafe fn invoke_path(pid: i32, path: &[String]) -> Result<(), String> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err("invoke_menu: target application is unavailable".into());
    }
    set_messaging_timeout(app);

    // Whether this call pressed a menu item (a press that reports an error
    // can still open its menu), so a failure must close what it opened.
    let mut pressed = false;
    // Unreadable now counts every menu window later as this call's, which can
    // only make the closed claim more cautious.
    let menus_before = crate::windows::menu_window_ids(pid).unwrap_or_default();
    let result = (|| {
        for depth in 0..path.len() {
            // Resolve from the live app root for every hop. Opening a menu can
            // replace its AX objects and reorder unrelated snapshot indices.
            let menu_bar = copy_element_attr(app, "AXMenuBar")
                .ok_or_else(|| "invoke_menu: target exposes no AXMenuBar".to_owned())?;
            set_messaging_timeout(menu_bar);
            let target = resolve_exact_prefix(menu_bar, &path[..=depth]);
            CFRelease(menu_bar as CFTypeRef);
            let target = target?;
            set_messaging_timeout(target);

            if copy_bool_attr(target, "AXEnabled") == Some(false) {
                CFRelease(target as CFTypeRef);
                return Err(format!("invoke_menu: path segment {depth} is disabled"));
            }
            let actions = copy_action_names(target);
            let action = choose_action(&actions, depth + 1 == path.len()).ok_or_else(|| {
                format!("invoke_menu: path segment {depth} has no usable native menu action")
            });
            let action = match action {
                Ok(action) => action,
                Err(error) => {
                    CFRelease(target as CFTypeRef);
                    return Err(error);
                }
            };
            pressed = true;
            let error = perform_action(target, action);
            CFRelease(target as CFTypeRef);
            if error != kAXErrorSuccess {
                return Err(format!(
                    "invoke_menu: native action for path segment {depth} failed with AX error {error}"
                ));
            }
            if depth + 1 != path.len() {
                std::thread::sleep(Duration::from_millis(80));
            }
        }
        Ok(())
    })();

    // A failure after a hop opened a menu: close it, so the app is not left
    // tracking a menu (where the next menu command reports success and does
    // nothing).
    let result = result.map_err(|error| {
        if !pressed {
            return error;
        }
        let closed = close_opened_menu(app, pid, &path[0], &menus_before);
        failure_after_press(error, &path[0], closed)
    });

    CFRelease(app as CFTypeRef);
    result
}

fn select_frontmost_pid(
    front_process_matches: Option<bool>,
    workspace_fallback: impl FnOnce() -> Option<i32>,
    pid: i32,
) -> Option<i32> {
    match front_process_matches {
        Some(true) => Some(pid),
        Some(false) => None,
        None => workspace_fallback(),
    }
}

fn live_frontmost_pid(pid: i32, window_id: u32) -> Option<i32> {
    select_frontmost_pid(
        crate::input::skylight::front_process_matches(pid, window_id),
        crate::apps::frontmost_pid,
        pid,
    )
}

fn live_frontmost_app() -> Option<i32> {
    crate::windows::visible_windows()
        .into_iter()
        .find(|window| {
            crate::input::skylight::front_process_matches(window.pid, window.window_id)
                == Some(true)
        })
        .map(|window| window.pid)
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
        && live_frontmost_pid(pid, window_id) != Some(pid)
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
            live_frontmost_pid(pid, window_id),
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

/// The item could not be invoked without activating its app or raising its
/// window. Either would take the user's front window, so refuse and name the
/// route.
fn background_unavailable(reason: String, request: &MenuRequest) -> ToolResult {
    let message = format!(
        "{reason}. {}. If the user allows it, call bring_to_front for this window first, then \
         invoke_menu, or send the app's keyboard shortcut with hotkey and \
         delivery_mode:\"foreground\". A background hotkey does not help: the disabled item \
         ignores its shortcut too. Otherwise use the control in the window.",
        request.unavailable_because()
    );
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "code": "background_unavailable",
        "refusal": { "code": "background_unavailable", "message": message }
    }))
}

/// Why a background menu item could not be pressed.
#[derive(Debug)]
enum BackgroundPressError {
    /// macOS keeps the final item (path segment `segment`) disabled on this
    /// route; the caller words why ([`MenuRequest::disabled_text`]).
    Disabled {
        segment: usize,
        is_save: bool,
    },
    Other(String),
    UnboundWindow {
        requested: u32,
        focused: Option<u32>,
        main: Option<u32>,
    },
}

/// What the background route knows about the pressed item.
#[derive(Debug, Clone, Copy)]
struct PressedItem {
    is_save: bool,
}

/// Whether a menu item's key equivalent is plain Command-S, the Save
/// shortcut in every language.
unsafe fn item_is_save(item: AXUIElementRef) -> bool {
    copy_string_attr(item, "AXMenuItemCmdChar").is_some_and(|ch| ch.eq_ignore_ascii_case("s"))
        && crate::ax::bindings::copy_number_attr(item, "AXMenuItemCmdModifiers").unwrap_or(0.0)
            == 0.0
}

/// How long a disabled item gets to enable after its window becomes key, and
/// how often it is re-read.
const ENABLE_WAIT: Duration = Duration::from_millis(600);
const ENABLE_POLL: Duration = Duration::from_millis(40);

/// AppKit resolves document menus against its key/main window, not the ID
/// supplied by a caller. Unknown or sibling context must fail before lookup.
fn require_menu_window(
    requested: u32,
    focused: Option<u32>,
    main: Option<u32>,
) -> Result<(), BackgroundPressError> {
    if focused == Some(requested) && main == Some(requested) {
        Ok(())
    } else {
        Err(BackgroundPressError::UnboundWindow {
            requested,
            focused,
            main,
        })
    }
}

unsafe fn require_live_menu_window(
    app: AXUIElementRef,
    requested: u32,
) -> Result<(), BackgroundPressError> {
    let read = |name| {
        let window = copy_element_attr(app, name)?;
        let id = ax_get_window_id(window);
        CFRelease(window as CFTypeRef);
        id
    };
    require_menu_window(requested, read("AXFocusedWindow"), read("AXMainWindow"))
}

fn unbound_menu_window(
    pid: i32,
    requested: u32,
    focused: Option<u32>,
    main: Option<u32>,
) -> ToolResult {
    super::background_refusal_result(
        pid,
        requested,
        &cua_driver_core::background_input::BackgroundRefusal {
            code: super::WINDOW_NOT_KEY,
            reason: format!(
                "invoke_menu: requested window {requested} is not the confirmed document menu context \
                 (focused={focused:?}, main={main:?}); no menu action was dispatched"
            ),
            advice: Some("foreground"),
        },
    )
}

/// Resolve the whole path from the app's live menu bar without opening any
/// level, then perform the final item's action. Used for a background app:
/// opening a menu-bar menu would take key focus and draw over the user's
/// front app.
unsafe fn invoke_path_in_background(
    pid: i32,
    window_id: u32,
    path: &[String],
) -> Result<PressedItem, BackgroundPressError> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(BackgroundPressError::Other(
            "invoke_menu: target application is unavailable".into(),
        ));
    }
    set_messaging_timeout(app);
    if let Err(error) = require_live_menu_window(app, window_id) {
        CFRelease(app as CFTypeRef);
        return Err(error);
    }
    let menu_bar = copy_element_attr(app, "AXMenuBar");
    CFRelease(app as CFTypeRef);
    let menu_bar = menu_bar.ok_or_else(|| {
        BackgroundPressError::Other("invoke_menu: target exposes no AXMenuBar".to_owned())
    })?;
    set_messaging_timeout(menu_bar);
    let target = resolve_exact_prefix(menu_bar, path);
    CFRelease(menu_bar as CFTypeRef);
    let target = target.map_err(BackgroundPressError::Other)?;
    set_messaging_timeout(target);
    let is_save = item_is_save(target);
    let result = (|| {
        // Preserve enablement priming for an already-bound document. The
        // context check above prevents this from switching to a sibling after
        // resolving that sibling's menu; the check below revalidates dispatch.
        if copy_bool_attr(target, "AXEnabled") == Some(false)
            && crate::input::skylight::activate_without_raise_with(pid, window_id, false)
        {
            let mut waited = Duration::ZERO;
            while waited < ENABLE_WAIT && copy_bool_attr(target, "AXEnabled") == Some(false) {
                std::thread::sleep(ENABLE_POLL);
                waited += ENABLE_POLL;
            }
        }
        if copy_bool_attr(target, "AXEnabled") == Some(false) {
            return Err(BackgroundPressError::Disabled {
                segment: path.len().saturating_sub(1),
                is_save,
            });
        }
        let actions = copy_action_names(target);
        let action = choose_action(&actions, true).ok_or_else(|| {
            BackgroundPressError::Other(
                "invoke_menu: the final menu item has no usable native menu action".to_owned(),
            )
        })?;
        // Revalidate after path resolution, immediately before the only
        // dispatch. Never fix a mismatch by silently switching documents.
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err(BackgroundPressError::Other(
                "invoke_menu: target application is unavailable before dispatch".into(),
            ));
        }
        set_messaging_timeout(app);
        let bound = require_live_menu_window(app, window_id);
        CFRelease(app as CFTypeRef);
        bound?;
        let error = perform_action(target, action);
        if error != kAXErrorSuccess {
            return Err(BackgroundPressError::Other(format!(
                "invoke_menu: native action for the final menu item failed with AX error {error}"
            )));
        }
        Ok(PressedItem { is_save })
    })();
    CFRelease(target as CFTypeRef);
    result
}

/// The file behind a document window (its `AXDocument` URL), if any.
fn document_path_of_window(pid: i32, window_id: u32) -> Option<std::path::PathBuf> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        set_messaging_timeout(app);
        let mut url = None;
        for window in copy_ax_windows(app) {
            if url.is_none() && ax_get_window_id(window) == Some(window_id) {
                url = copy_string_attr(window, "AXDocument");
            }
            CFRelease(window as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
        url.as_deref()
            .and_then(crate::apple_events::file_url_to_path)
    }
}

/// How a background menu command was asked for, so the result can say it.
#[derive(Debug, Clone)]
pub(super) enum MenuRequest {
    /// `invoke_menu` with a path.
    Path,
    /// `invoke_menu` with a path for a window of the frontmost app that is not
    /// its key window: the user is working in another window of that app.
    PathBehindUsersWindow,
    /// `hotkey` with this combo, which is the item's key equivalent.
    KeyEquivalent(String),
}

impl MenuRequest {
    fn preface(&self, path: &[String]) -> String {
        match self {
            MenuRequest::Path | MenuRequest::PathBehindUsersWindow => String::new(),
            MenuRequest::KeyEquivalent(combo) => format!(
                "{combo} is the key equivalent of {}. Key events do not reach a background \
                 app's menus, so no keys were sent. ",
                path.join(" > ")
            ),
        }
    }

    /// Where the item was pressed, for the result text.
    fn placement(&self) -> &'static str {
        match self {
            MenuRequest::Path | MenuRequest::KeyEquivalent(_) => {
                "with the app in the background (not activated)"
            }
            MenuRequest::PathBehindUsersWindow => {
                "in a window behind the user's window (made key for the press without raising \
                 it; key is handed back to the user's window after the press)"
            }
        }
    }

    /// When macOS keeps the item disabled on this route, for refusals.
    fn disabled_while(&self, app: &str) -> String {
        match self {
            MenuRequest::Path | MenuRequest::KeyEquivalent(_) => {
                format!("while {app} is in the background")
            }
            MenuRequest::PathBehindUsersWindow => {
                "while its window is behind the user's window".to_owned()
            }
        }
    }

    /// The refusal text for a final item macOS keeps disabled on this route.
    fn disabled_text(&self, segment: usize) -> String {
        format!(
            "invoke_menu: path segment {segment} is disabled {}",
            self.disabled_while("the app")
        )
    }

    /// Why the call refuses instead of reaching the item, for
    /// `background_unavailable`.
    fn unavailable_because(&self) -> &'static str {
        match self {
            MenuRequest::Path | MenuRequest::KeyEquivalent(_) => {
                "invoke_menu does not activate a background app: macOS keeps this menu item \
                 unavailable while the app is behind the user's window"
            }
            MenuRequest::PathBehindUsersWindow => {
                "invoke_menu does not raise a window that is behind the user's window: macOS \
                 keeps this menu item unavailable there, even with the window made key for the \
                 press"
            }
        }
    }

    /// The result text when a disabled Save went through the app's scripting
    /// interface instead.
    fn scripted_save_text(
        &self,
        path: &[String],
        app: &str,
        file: &std::path::Path,
        file_written: bool,
    ) -> String {
        let untouched = match self {
            MenuRequest::Path | MenuRequest::KeyEquivalent(_) => "not activated",
            MenuRequest::PathBehindUsersWindow => "the window was not raised",
        };
        let disk = if file_written {
            format!(" and {} was written to disk", file.display())
        } else {
            format!(
                "; {} did not change on disk because it already held the document's content",
                file.display()
            )
        };
        format!(
            "{}{} is disabled {}, so the document was saved through {app}'s scripting interface \
             instead ({untouched}). Verified: {app} reports no unsaved changes{disk}.",
            self.preface(path),
            path.join(" > "),
            self.disabled_while(app)
        )
    }
}

/// Run a menu command in a background app without activating it, or in a
/// window behind the user's window without raising it (`request` says which,
/// and its texts say so). Presses the item through accessibility when macOS
/// keeps it enabled; a disabled Save of a document window goes through the
/// app's scripting interface instead (TextEdit disables Save while it is
/// behind the user's window). A Save is verified on disk. Anything else that
/// is disabled is refused with `background_unavailable`.
pub(super) async fn run_in_background(
    pid: i32,
    window_id: u32,
    path: Vec<String>,
    request: MenuRequest,
) -> ToolResult {
    let _mutation = super::acquire_background_mutation(pid).await;
    let document = tokio::task::spawn_blocking(move || document_path_of_window(pid, window_id))
        .await
        .ok()
        .flatten();
    let before = document
        .as_deref()
        .and_then(crate::apple_events::modified_time);
    let press_path = path.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let _key_focus = crate::key_focus_guard::protect(pid);
        unsafe { invoke_path_in_background(pid, window_id, &press_path) }
    })
    .await;
    let preface = request.preface(&path);
    match outcome {
        Ok(Ok(PressedItem { is_save: true })) if document.is_some() => {
            let file = document.clone().expect("checked");
            let written = tokio::task::spawn_blocking(move || {
                crate::apple_events::wait_for_new_mtime(&file, before, Duration::from_secs(2))
            })
            .await
            .unwrap_or(false);
            let file = document.expect("checked");
            if written {
                background_success(
                    format!(
                        "{preface}Pressed {} through accessibility {}. Verified: {} was \
                         written to disk.",
                        path.join(" > "),
                        request.placement(),
                        file.display()
                    ),
                    ActionEffect::Confirmed,
                    ActionTransport::MacosAxAction,
                    "The document's file modification time moved after the press",
                )
            } else {
                background_success(
                    format!(
                        "{preface}Pressed {} through accessibility {}, but {} did not change \
                         on disk within 2 s. The document may have had no unsaved changes, or \
                         the save did not run; verify from fresh state.",
                        path.join(" > "),
                        request.placement(),
                        file.display()
                    ),
                    ActionEffect::Unverifiable,
                    ActionTransport::MacosAxAction,
                    "AX accepted the press; the file modification time did not move",
                )
            }
        }
        Ok(Ok(_)) => background_success(
            format!(
                "{preface}Invoked {} through accessibility {}; verify the command's semantic \
                 effect from fresh state.",
                path.join(" > "),
                request.placement()
            ),
            ActionEffect::Unverifiable,
            ActionTransport::MacosAxAction,
            "Every menu hop resolved uniquely without opening it and AX accepted the final action",
        ),
        Ok(Err(BackgroundPressError::Disabled {
            is_save: true,
            segment,
        })) if document.is_some() => {
            let file = document.expect("checked");
            let bundle_id = crate::apps::bundle_id_for_pid(pid).unwrap_or_default();
            let app_name =
                crate::apps::get_app_name_for_pid(pid).unwrap_or_else(|| "the app".into());
            let save_file = file.clone();
            let save_app = app_name.clone();
            let saved = tokio::task::spawn_blocking(move || {
                crate::apple_events::save_document(&bundle_id, &save_app, &save_file)
            })
            .await;
            match saved {
                Ok(Ok(saved)) => background_success(
                    request.scripted_save_text(&path, &app_name, &file, saved.file_written),
                    ActionEffect::Confirmed,
                    ActionTransport::MacosAppleEvent,
                    "The app's scripting interface reported the document saved",
                ),
                Ok(Err(refusal)) => background_unavailable(
                    format!(
                        "{preface}{}, and saving through {app_name}'s scripting interface did \
                         not complete ({}: {}). Nothing was saved",
                        request.disabled_text(segment),
                        refusal.code,
                        refusal.reason
                    ),
                    &request,
                ),
                Err(error) => refusal(format!("invoke_menu: blocking task failed: {error}")),
            }
        }
        Ok(Err(BackgroundPressError::Disabled { segment, .. })) => background_unavailable(
            format!("{preface}{}", request.disabled_text(segment)),
            &request,
        ),
        Ok(Err(BackgroundPressError::UnboundWindow {
            requested,
            focused,
            main,
        })) => unbound_menu_window(pid, requested, focused, main),
        Ok(Err(BackgroundPressError::Other(error))) => refusal(error),
        Err(error) => refusal(format!("invoke_menu: blocking task failed: {error}")),
    }
}

fn background_success(
    text: String,
    effect: ActionEffect,
    transport: ActionTransport,
    detail: &str,
) -> ToolResult {
    // A confirmed result rests on a readback of the effect (the file on disk
    // or the app's own saved state), not on the AX return code.
    let kind = if matches!(effect, ActionEffect::Confirmed) {
        EvidenceKind::ValueReadback
    } else {
        EvidenceKind::NativeApiResult
    };
    ToolResult::text(text).with_action_record(
        ActionExecutionRecord::builder(effect, transport, RequestedDelivery::Background)
            .actual_delivery(ActualDelivery::Background)
            .evidence(ActionEvidence {
                kind,
                detail: detail.into(),
            })
            .build()
            .expect("invoke_menu record is valid"),
    )
}

/// How a menu command reaches its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuRoute {
    /// The app is behind the user's window: press the item without
    /// activating the app or opening the menu.
    Background,
    /// The app is frontmost but the target is not its key window, so the user
    /// is working in another window of the same app. Raising the target would
    /// take that window from them: give the target key status for the press
    /// only (in-app key-window record, no raise, no activation), as keys sent
    /// to such a window are, then hand key back.
    BehindUsersWindow,
    /// The target is the frontmost app's key window, or, when that cannot be
    /// read, its frontmost window on screen. Either way it is the user's own
    /// window: the menu bar on screen is already its own, and raising it
    /// changes nothing.
    KeyWindowInFront,
    /// The frontmost app's key window cannot be read, and the target is not
    /// its frontmost window on screen (`order_read`) or that order cannot be
    /// read either. The target may be behind the user's window, and the
    /// key-window record cannot be confirmed without a readable key window:
    /// refuse rather than raise it.
    KeyWindowUnknown { order_read: bool },
}

/// `key_window` is the frontmost app's `AXFocusedWindow`. `front_window` is
/// its frontmost on-screen window, consulted only when the key window cannot
/// be read; `None` when the window order cannot be read.
fn menu_route(
    target_in_front: bool,
    key_window: Option<u32>,
    front_window: Option<u32>,
    window_id: u32,
) -> MenuRoute {
    if !target_in_front {
        return MenuRoute::Background;
    }
    match key_window {
        Some(key) if key == window_id => MenuRoute::KeyWindowInFront,
        Some(_) => MenuRoute::BehindUsersWindow,
        None if front_window == Some(window_id) => MenuRoute::KeyWindowInFront,
        None => MenuRoute::KeyWindowUnknown {
            order_read: front_window.is_some(),
        },
    }
}

/// Refusal for a window of the app the user is using that cannot be pressed
/// without raising it. Nothing is invoked: the item would act on the user's
/// window. `why` completes "invoke_menu: window N …".
fn behind_users_window_refusal(pid: i32, window_id: u32, why: &str) -> ToolResult {
    super::background_refusal_result(
        pid,
        window_id,
        &cua_driver_core::background_input::BackgroundRefusal {
            code: super::WINDOW_NOT_KEY,
            reason: format!(
                "invoke_menu: window {window_id} {why}. Nothing was invoked. If the user allows \
                 it, call bring_to_front for this window first, then invoke_menu"
            ),
            advice: Some("bring_to_front"),
        },
    )
}

/// Why a window behind the user's window was not pressed: it could not be
/// made key for the press.
fn not_made_key(reason: &str) -> String {
    format!(
        "is behind the user's window in the app they are using and could not be made key \
         without raising it ({reason})"
    )
}

/// Why a window of the frontmost app was not pressed when that app's key
/// window cannot be read ([`MenuRoute::KeyWindowUnknown`]).
fn key_window_unknown(order_read: bool) -> &'static str {
    if order_read {
        "is not the frontmost window of the app the user is using, and that app's key window \
         could not be read, so it may be behind the user's window"
    } else {
        "belongs to the app the user is using, whose key window and on-screen window order \
         could not be read, so it may be behind the user's window"
    }
}

/// Make the target the app's key window for one press, without raising it.
/// The returned scope hands key back to the user's window when dropped.
async fn key_scope_behind_users_window(
    pid: i32,
    window_id: u32,
) -> Result<crate::input::skylight::WindowKeyScope, ToolResult> {
    match tokio::task::spawn_blocking(move || super::key_scope(pid, Some(window_id))).await {
        Ok(Ok(Some(scope))) => Ok(scope),
        Ok(Ok(None)) => Err(behind_users_window_refusal(
            pid,
            window_id,
            &not_made_key("focus without raise is unavailable"),
        )),
        Ok(Err(_)) if crate::input::skylight::windowserver_connection_invalid() => {
            Err(super::windowserver_connection_refusal(pid, Some(window_id)))
        }
        Ok(Err(reason)) => Err(behind_users_window_refusal(
            pid,
            window_id,
            &not_made_key(&reason),
        )),
        Err(error) => Err(refusal(format!(
            "invoke_menu: blocking task failed: {error}"
        ))),
    }
}

fn refusal(message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": { "code": "menu_path_unavailable", "message": message }
    }))
}

#[async_trait]
impl Tool for InvokeMenuTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // The menu bar belongs to the app: with only a pid, use its focused
        // window.
        let mut args = args;
        super::default_window_for_pid(&mut args);
        let input: InvokeMenuInput =
            match cua_driver_core::tool_args::parse_typed_input("invoke_menu", args) {
                Ok(input) => input,
                Err(result) => return result,
            };
        let path = match normalized_path(input.path) {
            Ok(path) => path,
            Err(error) => return refusal(error),
        };
        let pid = match i32::try_from(input.pid) {
            Ok(pid) => pid,
            Err(_) => return refusal("invoke_menu: pid is out of range".into()),
        };
        let window_id = match u32::try_from(input.window_id) {
            Ok(window_id) => window_id,
            Err(_) => return refusal("invoke_menu: window_id is out of range".into()),
        };
        if !crate::windows::all_windows()
            .iter()
            .any(|window| window.pid == pid && window.window_id == window_id)
        {
            return refusal("invoke_menu: window_id does not belong to pid".into());
        }

        // A background target: invoke the item without activating the app or
        // raising its window. When macOS keeps the item unavailable to a
        // background app, refuse and say so instead of activating it. A window
        // behind the user's window in the app they are using is not raised
        // either (MenuRoute::BehindUsersWindow, MenuRoute::KeyWindowUnknown).
        let (target_in_front, key_window, front_window) = tokio::task::spawn_blocking(move || {
            if live_frontmost_app().or_else(crate::apps::frontmost_pid) != Some(pid) {
                return (false, None, None);
            }
            let key_window = crate::ax::bindings::focused_window_id_of_pid(pid);
            // The window order stands in only when the key window cannot be read.
            let front_window = key_window
                .is_none()
                .then(|| {
                    crate::input::skylight::app_window_order(pid)
                        .first()
                        .copied()
                })
                .flatten();
            (true, key_window, front_window)
        })
        .await
        .unwrap_or((false, None, None));
        match menu_route(target_in_front, key_window, front_window, window_id) {
            MenuRoute::Background => {
                return run_in_background(pid, window_id, path, MenuRequest::Path).await;
            }
            MenuRoute::KeyWindowUnknown { order_read } => {
                return behind_users_window_refusal(pid, window_id, key_window_unknown(order_read));
            }
            MenuRoute::BehindUsersWindow => {
                let scope = match key_scope_behind_users_window(pid, window_id).await {
                    Ok(scope) => scope,
                    Err(refused) => return refused,
                };
                let result =
                    run_in_background(pid, window_id, path, MenuRequest::PathBehindUsersWindow)
                        .await;
                drop(scope);
                return result;
            }
            MenuRoute::KeyWindowInFront => {}
        }

        let outcome = tokio::task::spawn_blocking(move || {
            let prior_frontmost = live_frontmost_app().or_else(crate::apps::frontmost_pid);
            let prior_frontmost_window =
                prior_frontmost.and_then(crate::ax::bindings::focused_window_id_of_pid);
            let needs_activation = prior_frontmost != Some(pid);

            let result = focus_exact_window(pid, window_id)
                .and_then(|()| unsafe { invoke_path(pid, &path) });

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
            Ok(Ok(())) => ToolResult::text(
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
            ),
            Ok(Err(error)) => refusal(error),
            Err(error) => refusal(format!("invoke_menu: blocking task failed: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_menu_requires_both_exact_document_contexts() {
        assert!(require_menu_window(7, Some(7), Some(7)).is_ok());
        for (focused, main) in [
            (Some(8), Some(8)),
            (Some(7), Some(8)),
            (Some(8), Some(7)),
            (None, Some(7)),
            (Some(7), None),
            (None, None),
        ] {
            assert!(matches!(
                require_menu_window(7, focused, main),
                Err(BackgroundPressError::UnboundWindow { requested: 7, focused: f, main: m })
                    if f == focused && m == main
            ));
        }
    }

    #[test]
    fn unbound_menu_refuses_before_dispatch_with_the_exact_target() {
        let result = unbound_menu_window(42, 7, Some(8), None);
        assert_eq!(result.is_error, Some(true));
        let data = result.structured_content.unwrap();
        assert_eq!(data["code"], "window_not_key");
        assert_eq!(data["effect"], "refused");
        assert_eq!(data["pid"], 42);
        assert_eq!(data["window_id"], 7);
        assert!(data["reason"]
            .as_str()
            .unwrap()
            .contains("no menu action was dispatched"));
    }

    #[test]
    fn a_background_menu_refusal_names_the_code_and_the_route() {
        let message = |request: &MenuRequest| {
            let result = background_unavailable(request.disabled_text(1), request);
            assert_eq!(result.is_error, Some(true));
            let structured = result.structured_content.expect("structured refusal");
            assert_eq!(structured["code"], "background_unavailable");
            let message = structured["refusal"]["message"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(message.contains("bring_to_front"));
            assert!(message.contains("delivery_mode:\"foreground\""));
            assert!(message.contains("A background hotkey does not help"));
            message
        };
        for request in [MenuRequest::Path, MenuRequest::KeyEquivalent("⌘B".into())] {
            let background = message(&request);
            assert!(background.starts_with(
                "invoke_menu: path segment 1 is disabled while the app is in the background. \
                 invoke_menu does not activate a background app"
            ));
        }
        let behind = message(&MenuRequest::PathBehindUsersWindow);
        assert!(behind.starts_with(
            "invoke_menu: path segment 1 is disabled while its window is behind the user's \
             window. invoke_menu does not raise a window that is behind the user's window"
        ));
        assert!(!behind.contains("in the background"));
        assert!(!behind.contains("background app"));
    }

    #[test]
    fn a_scripted_save_says_where_the_window_was() {
        let path = vec!["File".to_owned(), "Save".to_owned()];
        let file = std::path::Path::new("/tmp/note.txt");
        let background = MenuRequest::Path.scripted_save_text(&path, "TextEdit", file, true);
        assert_eq!(
            background,
            "File > Save is disabled while TextEdit is in the background, so the document was \
             saved through TextEdit's scripting interface instead (not activated). Verified: \
             TextEdit reports no unsaved changes and /tmp/note.txt was written to disk."
        );
        let behind =
            MenuRequest::PathBehindUsersWindow.scripted_save_text(&path, "TextEdit", file, false);
        assert_eq!(
            behind,
            "File > Save is disabled while its window is behind the user's window, so the \
             document was saved through TextEdit's scripting interface instead (the window was \
             not raised). Verified: TextEdit reports no unsaved changes; /tmp/note.txt did not \
             change on disk because it already held the document's content."
        );
        assert!(MenuRequest::KeyEquivalent("⌘S".into())
            .scripted_save_text(&path, "TextEdit", file, true)
            .starts_with("⌘S is the key equivalent of File > Save."));
    }

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
    fn a_failed_path_says_whether_its_menu_was_closed() {
        let error = || "invoke_menu: path segment 1 was not found".to_owned();
        assert_eq!(
            failure_after_press(error(), "File", Some(true)),
            "invoke_menu: path segment 1 was not found. No menu window this call opened is still on screen."
        );
        for unconfirmed in [Some(false), None] {
            assert!(failure_after_press(error(), "File", unconfirmed)
                .ends_with("The File menu this call opened may still be open: press escape on the window before other input."));
        }
    }

    #[test]
    fn segments_match_exactly_then_ignoring_case() {
        let titles: Vec<String> = ["To Next Sheet", "To Previous Sheet", "", "Go to Sheet..."]
            .map(String::from)
            .to_vec();
        assert_eq!(pick_title(&titles, "To Next Sheet"), TitleMatch::One(0));
        assert_eq!(pick_title(&titles, "to next sheet"), TitleMatch::One(0));
        assert_eq!(pick_title(&titles, "Go to Sheet…"), TitleMatch::One(3));
        assert_eq!(pick_title(&titles, "Next Sheet"), TitleMatch::None);
        let twins: Vec<String> = ["Copy", "copy"].map(String::from).to_vec();
        assert_eq!(pick_title(&twins, "Copy"), TitleMatch::One(0));
        assert_eq!(pick_title(&twins, "COPY"), TitleMatch::Ambiguous);
    }

    #[test]
    fn a_missed_segment_lists_the_items_at_its_level() {
        let path: Vec<String> = ["Sheet", "Navigate", "Next Sheet"]
            .map(String::from)
            .to_vec();
        let titles: Vec<String> = ["To Next Sheet", "", "To Previous Sheet"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            missing_segment_error(2, &path, &titles),
            "invoke_menu: path segment 2 \"Next Sheet\" was not found under \"Sheet > Navigate\". Items there: \"To Next Sheet\", \"To Previous Sheet\""
        );
        assert!(missing_segment_error(0, &path, &["File".into()]).contains("in the menu bar"));
        assert!(missing_segment_error(1, &path, &[]).contains("lists no titled items"));
        let many: Vec<String> = (0..45).map(|i| format!("Item {i}")).collect();
        assert!(missing_segment_error(1, &path, &many).ends_with("and 5 more"));
    }

    #[test]
    fn three_periods_match_the_ellipsis_in_menu_titles() {
        assert_eq!(
            menu_title_key(" Save As... "),
            menu_title_key("Save As\u{2026}")
        );
        assert_eq!(menu_title_key("Save As..."), menu_title_key("Save As..."));
        assert_ne!(menu_title_key("Save As"), menu_title_key("Save As\u{2026}"));
        assert_ne!(
            menu_title_key("save as..."),
            menu_title_key("Save As\u{2026}")
        );
    }

    #[test]
    fn menu_focus_requires_the_exact_frontmost_app_and_window() {
        assert!(exact_window_is_ready(Some(7), 7, Some(42), 42));
        assert!(!exact_window_is_ready(Some(8), 7, Some(42), 42));
        assert!(!exact_window_is_ready(Some(7), 7, Some(41), 42));
        assert!(!exact_window_is_ready(Some(7), 7, None, 42));
    }

    #[test]
    fn a_window_behind_the_users_window_in_the_front_app_is_never_raised() {
        for front_window in [None, Some(41), Some(42)] {
            assert_eq!(
                menu_route(false, None, front_window, 42),
                MenuRoute::Background
            );
            assert_eq!(
                menu_route(false, Some(42), front_window, 42),
                MenuRoute::Background
            );
            // A readable key window decides; the window order does not.
            assert_eq!(
                menu_route(true, Some(42), front_window, 42),
                MenuRoute::KeyWindowInFront
            );
            assert_eq!(
                menu_route(true, Some(41), front_window, 42),
                MenuRoute::BehindUsersWindow
            );
        }
    }

    #[test]
    fn an_unknown_key_window_takes_the_old_path_only_for_the_apps_front_window() {
        // The app's frontmost window is the user's own: raising it changes nothing.
        assert_eq!(
            menu_route(true, None, Some(42), 42),
            MenuRoute::KeyWindowInFront
        );
        // Any other window may be behind theirs, and the key-window record
        // cannot be confirmed without a readable key window.
        assert_eq!(
            menu_route(true, None, Some(41), 42),
            MenuRoute::KeyWindowUnknown { order_read: true }
        );
        // Nothing is known about the order: refuse too.
        assert_eq!(
            menu_route(true, None, None, 42),
            MenuRoute::KeyWindowUnknown { order_read: false }
        );
    }

    fn assert_refused_with_bring_to_front(result: ToolResult) -> String {
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured refusal");
        assert_eq!(structured["code"], "window_not_key");
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["pid"], 7);
        assert_eq!(structured["window_id"], 42);
        assert_eq!(structured["escalation"]["recommended"], "bring_to_front");
        let reason = structured["reason"].as_str().unwrap().to_owned();
        assert!(reason.starts_with("invoke_menu: window 42 "));
        assert!(reason.contains("Nothing was invoked"));
        reason
    }

    #[test]
    fn a_window_that_cannot_take_key_is_refused_with_bring_to_front() {
        let reason = assert_refused_with_bring_to_front(behind_users_window_refusal(
            7,
            42,
            &not_made_key("focus without raise is unavailable"),
        ));
        assert!(reason.contains(
            "is behind the user's window in the app they are using and could not be made key \
             without raising it (focus without raise is unavailable)"
        ));
    }

    #[test]
    fn an_unknown_key_window_is_refused_with_bring_to_front() {
        let not_front = assert_refused_with_bring_to_front(behind_users_window_refusal(
            7,
            42,
            key_window_unknown(true),
        ));
        assert!(not_front.contains("is not the frontmost window of the app the user is using"));
        assert!(not_front.contains("may be behind the user's window"));
        let unread = assert_refused_with_bring_to_front(behind_users_window_refusal(
            7,
            42,
            key_window_unknown(false),
        ));
        assert!(unread.contains("whose key window and on-screen window order could not be read"));
        assert!(unread.contains("may be behind the user's window"));
    }

    #[test]
    fn results_say_where_the_item_was_pressed() {
        assert!(MenuRequest::Path
            .placement()
            .contains("in the background (not activated)"));
        assert!(MenuRequest::KeyEquivalent("⌘S".into())
            .placement()
            .contains("not activated"));
        let behind = MenuRequest::PathBehindUsersWindow.placement();
        assert!(behind.contains("without raising"));
        assert!(behind.contains("key is handed back to the user's window after the press"));
        assert!(!behind.contains("background"));
        assert_eq!(
            MenuRequest::PathBehindUsersWindow.preface(&["File".into()]),
            ""
        );
    }

    fn workspace_frontmost_must_not_be_read() -> Option<i32> {
        panic!("stale workspace state must not be consulted");
    }

    #[test]
    fn windowserver_mismatch_never_falls_back_to_stale_workspace_state() {
        assert_eq!(
            select_frontmost_pid(Some(true), workspace_frontmost_must_not_be_read, 7),
            Some(7)
        );
        assert_eq!(
            select_frontmost_pid(Some(false), workspace_frontmost_must_not_be_read, 7),
            None
        );
        assert_eq!(select_frontmost_pid(None, || Some(8), 7), Some(8));
        assert_eq!(select_frontmost_pid(None, || None, 7), None);
    }
}
