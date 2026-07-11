//! Native NSPanel onboarding UI with a two-step permission state machine.
//!
//! Replaces the terminal banner when the daemon is launched from the
//! bundled `/Applications/CuaDriver.app`. Mirrors the Swift gate at
//! `libs/cua-driver/Sources/CuaDriverCore/Permissions/PermissionsGate.swift`.
//!
//! ## UX
//!
//! The branded explainer is shown before any system prompt. Each missing row
//! has its own Allow action, and only one permission request can be active at
//! a time. After a request is offered, its action becomes “Complete in System
//! Settings” so a denied or previously-dismissed TCC prompt has a clear
//! recovery path.
//!
//! On first launch with missing grants the panel resembles:
//!
//! ```text
//! ┌─ cmux cua Permissions ──────────────────────┐
//! │  Set up cmux cua                            │
//! │  Two macOS permissions are required.        │
//! │                                             │
//! │  ○ Accessibility                    [Allow] │
//! │      Read interface text and use controls.  │
//! │                                             │
//! │  ○ Screen Recording                 [Allow] │
//! │      See app windows to place actions.       │
//! │                                             │
//! │  macOS grants access to cmux cua.            │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! A 1 Hz `NSTimer` reads [`current_status`] on every tick and updates
//! the row icons, actions, heading, and footer in place. When both grants
//! flip green the poll callback calls `[NSApp stopModal]` and `show_modal` returns
//! [`PanelOutcome::AllGranted`] so the caller can skip the trailing
//! `wait_for_grants` loop.
//!
//! ## Threading
//!
//! `show_modal` MUST be called on the main thread. The Serve arm in
//! `cua-driver/src/main.rs` starts its socket worker, then invokes the gate on
//! the main thread before `NSApp.run()` is called by the cursor overlay.
//! `panel_enabled()`
//! double-checks and falls back to the terminal banner if anything
//! is off (not main thread, headless, opt-out env var, non-`.app`
//! invocation).
//!
//! All UI updates run on the main thread because `NSTimer` callbacks
//! dispatch through the main run loop. We do not touch UI from any
//! other thread.
//!
//! ## Library choice
//!
//! Raw `objc2::msg_send!` macros against AppKit classes — same style
//! as `cursor/overlay.rs`. Avoids pulling additional `objc2-app-kit`
//! feature flags or adding new crate dependencies.

use std::cell::RefCell;
use std::ffi::c_void;

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

use crate::permissions::gate::MissingPermission;
use crate::permissions::status::{
    current_status, request_accessibility, request_screen_recording, PermissionGrantState,
    PermissionsStatus,
};

// ── Public API ──────────────────────────────────────────────────────────

/// What the user did with the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelOutcome {
    /// AppKit could not construct a useful panel. The caller should use its
    /// terminal fallback instead.
    Unavailable,
    /// User closed the window. The daemon remains available and individual
    /// tools can report that permission is not granted.
    Dismissed,
    /// Both grants flipped green while the panel was up; the poll
    /// callback auto-dismissed. Caller can skip the polling phase.
    AllGranted,
}

/// Inputs for [`show_modal`].
#[derive(Debug, Clone)]
pub struct PanelOpts {
    /// Used for the initial render only — the poll loop re-derives the
    /// full live status on every tick and re-renders icons / heading
    /// accordingly. Caller passes the snapshot it already has so we
    /// don't double-probe TCC at startup.
    pub initial_status: PermissionsStatus,
}

/// Decide whether the panel can be shown right now. Bundled app launches use
/// the panel by default. Bare binaries retain the terminal flow.
///
///   * `CUA_DRIVER_RS_PERMISSIONS_PANEL` is not set to an explicit off
///     sentinel (`0` / `false` / `no` / `off`, case-insensitive); AND
///   * the process is running on the main thread (a hard requirement
///     for AppKit calls); AND
///   * the binary lives inside an `.app` bundle (bare-binary
///     invocations have no Info.plist / bundle id for TCC to attribute
///     the window to, so they always use the terminal flow).
///
/// The app-bundle requirement preserves correct TCC attribution: System
/// Settings grants the signed helper identity, never the terminal that
/// happened to launch it.
pub fn panel_enabled() -> bool {
    if cua_driver_core::embedded_mode() {
        return false;
    }
    if env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL") {
        return false;
    }
    if MainThreadMarker::new().is_none() {
        return false;
    }
    if !running_inside_app_bundle() {
        return false;
    }
    true
}

/// Show the panel modally. Blocks the calling (main) thread until the
/// user clicks one of the buttons, closes the window, or all grants
/// flip green. Returns the outcome so the caller can decide how to
/// proceed.
///
/// Safety / threading: panics if invoked off the main thread. Callers
/// should check [`panel_enabled`] first to avoid the panic.
pub fn show_modal(opts: PanelOpts) -> PanelOutcome {
    let _mtm = MainThreadMarker::new()
        .expect("show_modal must be called from the main thread");
    unsafe { show_modal_unsafe(&opts) }
}

/// Block the current (main) thread for `seconds` while letting the
/// AppKit run loop process events.
///
/// The terminal-flow gate (`gate::wait_for_grants`) used to use
/// `std::thread::sleep` between TCC probes. That works on every other
/// platform but on macOS it starves the AppKit run loop, which is
/// where `AXIsProcessTrusted()`'s per-process trust cache gets
/// invalidated when tccd posts notifications about grant changes.
/// Net effect: a daemon that called `AXIsProcessTrusted()` once at
/// startup and got `false` would keep getting `false` forever even
/// after the user granted accessibility in System Settings — the
/// poll loop never saw the grant flip.
///
/// Pumping the run loop here (instead of `nanosleep`) gives AppKit a
/// chance to handle the TCC change-notification XPC message, which
/// invalidates the trust cache. The next iteration's
/// `AXIsProcessTrusted()` call then sees the fresh truth.
///
/// Falls back to `thread::sleep` if anything in the AppKit setup
/// looks off (not main thread, NSRunLoop class unavailable) so this
/// function is safe to call from any context — it just degrades to
/// the old behaviour in those cases.
pub fn pump_run_loop_briefly(seconds: f64) {
    if MainThreadMarker::new().is_none() {
        // Not the main thread — pumping someone else's run loop is
        // wrong. Just sleep.
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
        return;
    }
    unsafe {
        let run_loop: *mut AnyObject =
            msg_send![class!(NSRunLoop), currentRunLoop];
        if run_loop.is_null() {
            std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
            return;
        }
        // `[NSDate dateWithTimeIntervalSinceNow:seconds]` builds the
        // deadline. `[NSRunLoop runMode:beforeDate:]` blocks up to
        // that deadline while dispatching pending input sources +
        // timers — including TCC notification taps.
        let date: *mut AnyObject =
            msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: seconds];
        if date.is_null() {
            std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
            return;
        }
        let default_mode = ns_string("NSDefaultRunLoopMode");
        let _: bool = msg_send![run_loop, runMode: default_mode beforeDate: date];
    }
}

// ── Implementation ──────────────────────────────────────────────────────

fn env_off(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let lower = v.to_ascii_lowercase();
            matches!(lower.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(false)
}

/// Detect whether the current executable lives under a `.app` bundle.
fn running_inside_app_bundle() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    exe.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| s.ends_with(".app"))
            .unwrap_or(false)
    })
}

// ── Layout constants ────────────────────────────────────────────────────
//
// Window geometry fits a heading, concise explainer, two permission rows with
// independent actions, and an attribution footer.

const WIN_W: f64 = 520.0;
const WIN_H: f64 = 360.0;
const PAD: f64 = 20.0;
const ROW_H: f64 = 84.0;
const FOOTER_H: f64 = 36.0;

unsafe fn show_modal_unsafe(opts: &PanelOpts) -> PanelOutcome {
    // ---- NSApplication setup ----
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    let _: bool = msg_send![app, setActivationPolicy: 1i64];
    let _: () = msg_send![app, finishLaunching];

    let main_screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if main_screen.is_null() {
        return PanelOutcome::Unavailable;
    }

    // ---- Window ----
    let content_rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: WIN_W,
            height: WIN_H,
        },
    };
    // NSWindowStyleMaskTitled (1) | NSWindowStyleMaskClosable (2) — matches
    // PermissionsGate.swift line 97: `[.titled, .closable]`.
    let style_mask: u64 = 1 | 2;
    // NSBackingStoreBuffered = 2.
    let backing: u64 = 2;

    let window: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSPanel), alloc];
        msg_send![allocated,
            initWithContentRect: content_rect
            styleMask: style_mask
            backing: backing
            defer: false
        ]
    };
    if window.is_null() {
        return PanelOutcome::Unavailable;
    }
    let _: () = msg_send![window, setReleasedWhenClosed: false];
    let _: () = msg_send![window, setHidesOnDeactivate: false];
    // NSFloatingWindowLevel (3) so it stays on top of System Settings
    // when the user navigates over to grant permissions.
    let _: () = msg_send![window, setLevel: 3i64];
    let product_name = product_name();
    let title = ns_string(&format!("{product_name} Permissions"));
    let _: () = msg_send![window, setTitle: title];
    let window_delegate = panel_target_instance();
    let _: () = msg_send![window, setDelegate: window_delegate];

    let content_view: *mut AnyObject = msg_send![window, contentView];

    // ---- Heading + subheading ----
    //
    // Y coordinates use AppKit's bottom-left origin. We lay out top-
    // down: subtract row heights from `WIN_H - PAD` as we go.
    let mut y = WIN_H - PAD - 24.0;

    let heading = build_label(
        &heading_for(opts.initial_status, &product_name),
        17.0,
        /*bold=*/ true,
        NSPoint { x: PAD, y },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: 24.0,
        },
    );
    let _: () = msg_send![content_view, addSubview: heading];

    y -= 8.0 + 40.0; // gap + subheading height

    let subheading = build_label(
        &subheading_for(opts.initial_status, &product_name),
        12.0,
        /*bold=*/ false,
        NSPoint { x: PAD, y },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: 40.0,
        },
    );
    let _: () = msg_send![content_view, addSubview: subheading];
    // Subheading is secondary text — apply secondaryLabelColor.
    let secondary_color: *mut AnyObject =
        msg_send![class!(NSColor), secondaryLabelColor];
    let _: () = msg_send![subheading, setTextColor: secondary_color];

    y -= 16.0; // gap before first row

    // ---- Permission rows ----
    let ax_row = build_row(
        MissingPermission::Accessibility,
        opts.initial_status,
        None,
        NSPoint { x: PAD, y: y - ROW_H },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: ROW_H,
        },
    );
    let _: () = msg_send![content_view, addSubview: ax_row.container];
    y -= ROW_H + 8.0;

    let sr_row = build_row(
        MissingPermission::ScreenRecording,
        opts.initial_status,
        None,
        NSPoint { x: PAD, y: y - ROW_H },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: ROW_H,
        },
    );
    let _: () = msg_send![content_view, addSubview: sr_row.container];

    // ---- Attribution / completion footer ----
    let footer = build_label(
        &footer_for(opts.initial_status, &product_name),
        11.0,
        /*bold=*/ false,
        NSPoint { x: PAD, y: 20.0 },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: FOOTER_H,
        },
    );
    let footer_color: *mut AnyObject = msg_send![class!(NSColor), tertiaryLabelColor];
    let _: () = msg_send![footer, setTextColor: footer_color];
    let _: () = msg_send![content_view, addSubview: footer];

    OUTCOME.with(|cell| *cell.borrow_mut() = None);

    // ---- Stash handles for the timer callback ----
    HANDLES.with(|cell| {
        *cell.borrow_mut() = Some(PanelHandles {
            heading: ptr_to_usize(heading),
            subheading: ptr_to_usize(subheading),
            footer: ptr_to_usize(footer),
            ax_row,
            sr_row,
            last_status: opts.initial_status,
            in_progress: None,
            product_name,
        });
    });

    // ---- Show window + start poll timer + run modal ----
    let _: () = msg_send![window, center];
    let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
    let _: () = msg_send![app, activateIgnoringOtherApps: true];

    let timer = schedule_poll_timer();
    let _: i64 = msg_send![app, runModalForWindow: window];
    // Stop timer first so any in-flight tick can't race with teardown.
    let _: () = msg_send![timer, invalidate];

    // Resolve final outcome. The OUTCOME thread-local has the value
    // most actions set; if neither button nor the poll fired, treat
    // it as Dismissed (red-dot close).
    let outcome = OUTCOME
        .with(|cell| *cell.borrow())
        .unwrap_or(PanelOutcome::Dismissed);

    // Tear down the handles before any later panel invocation can
    // observe stale pointers. We can't rely on Drop because everything
    // here is raw pointer Objective-C.
    HANDLES.with(|cell| *cell.borrow_mut() = None);

    let _: () = msg_send![window, orderOut: std::ptr::null::<AnyObject>()];
    outcome
}

// ── Live-update plumbing ────────────────────────────────────────────────
//
// AppKit timers fire on the main run loop's thread, so the callback
// runs on the same thread that built the panel — pointers we stashed
// in a thread-local are safe to dereference. We use raw `usize` to
// sidestep Send/Sync requirements (NSWindow / NSView are !Send).

#[derive(Debug, Clone, Copy)]
struct RowHandles {
    container: *mut AnyObject,
    icon: *mut AnyObject,
    title: *mut AnyObject,
    button: *mut AnyObject,
}

struct PanelHandles {
    heading: usize,
    subheading: usize,
    footer: usize,
    ax_row: RowHandles,
    sr_row: RowHandles,
    last_status: PermissionsStatus,
    in_progress: Option<MissingPermission>,
    product_name: String,
}

thread_local! {
    static OUTCOME: RefCell<Option<PanelOutcome>> = const { RefCell::new(None) };
    static HANDLES: RefCell<Option<PanelHandles>> = const { RefCell::new(None) };
}

fn ptr_to_usize(p: *mut AnyObject) -> usize {
    p as usize
}

fn usize_to_ptr(u: usize) -> *mut AnyObject {
    u as *mut AnyObject
}

unsafe fn schedule_poll_timer() -> *mut AnyObject {
    // 1 second cadence — matches Swift's
    // `Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true)`.
    //
    // Critical: `[NSApp runModalForWindow:]` runs the main run loop in
    // `NSModalPanelRunLoopMode`, which is NOT one of the standard
    // "common" modes by default. A timer added only to
    // `NSRunLoopCommonModes` (or unmodified, which lands it in
    // `NSDefaultRunLoopMode`) won't fire while the modal is up — the
    // panel renders fine but its icons never update.
    //
    // We construct a non-scheduled timer with `+timerWithTimeInterval:…`
    // and then explicitly add it to the modal-panel mode. We do not
    // also add it to NSDefaultRunLoopMode because the timer's lifetime
    // is bounded by the modal session: as soon as the modal stops
    // (button click or auto-dismiss) we `invalidate` it.
    let target = panel_target_instance();
    let sel = objc2::sel!(pollTick:);
    let timer: *mut AnyObject = msg_send![class!(NSTimer),
        timerWithTimeInterval: 1.0_f64
        target: target
        selector: sel
        userInfo: std::ptr::null::<AnyObject>()
        repeats: true
    ];
    let run_loop: *mut AnyObject = msg_send![class!(NSRunLoop), currentRunLoop];
    let modal_mode = ns_string("NSModalPanelRunLoopMode");
    let _: () = msg_send![run_loop, addTimer: timer forMode: modal_mode];
    timer
}

extern "C" fn on_poll_tick(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _timer: *mut AnyObject,
) {
    // Wrap the whole thing in a panic guard — if anything goes wrong
    // we want to stop modal rather than have the panel hang forever.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        poll_tick_inner();
    }));
    if let Err(e) = result {
        eprintln!("[cua-driver] permissions panel poll tick panicked: {e:?}");
        OUTCOME.with(|cell| *cell.borrow_mut() = Some(PanelOutcome::Dismissed));
        stop_modal();
    }
}

unsafe fn poll_tick_inner() {
    let status = current_status();
    let mut should_stop = false;
    HANDLES.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(handles) = guard.as_mut() else { return };
        if status == handles.last_status {
            // No change — skip the redraw. Avoids needlessly thrashing
            // the layout engine every second on a steady-state panel.
            return;
        }
        if handles
            .in_progress
            .is_some_and(|permission| permission_is_granted(status, permission))
        {
            handles.in_progress = None;
        }
        render_panel(handles, status);
        handles.last_status = status;
        if status.all_granted() {
            should_stop = true;
        }
    });
    if should_stop {
        OUTCOME.with(|cell| *cell.borrow_mut() = Some(PanelOutcome::AllGranted));
        stop_modal();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowAction {
    Allow,
    CompleteInSystemSettings,
    WaitingForOtherPermission,
    Allowed,
}

impl RowAction {
    fn title(self) -> &'static str {
        match self {
            Self::Allow => "Allow",
            Self::CompleteInSystemSettings => "Complete in System Settings",
            Self::WaitingForOtherPermission => "Waiting",
            Self::Allowed => "Allowed",
        }
    }

    fn enabled(self) -> bool {
        matches!(self, Self::Allow | Self::CompleteInSystemSettings)
    }
}

fn permission_is_granted(status: PermissionsStatus, permission: MissingPermission) -> bool {
    match permission {
        MissingPermission::Accessibility => status.accessibility,
        MissingPermission::ScreenRecording => status.screen_recording,
    }
}

fn row_action(
    status: PermissionsStatus,
    in_progress: Option<MissingPermission>,
    permission: MissingPermission,
) -> RowAction {
    if permission_is_granted(status, permission) {
        return RowAction::Allowed;
    }
    match in_progress {
        None => RowAction::Allow,
        Some(active) if active == permission => RowAction::CompleteInSystemSettings,
        Some(_) => RowAction::WaitingForOtherPermission,
    }
}

unsafe fn render_panel(handles: &PanelHandles, status: PermissionsStatus) {
    update_row(
        handles.ax_row,
        MissingPermission::Accessibility,
        status,
        handles.in_progress,
    );
    update_row(
        handles.sr_row,
        MissingPermission::ScreenRecording,
        status,
        handles.in_progress,
    );

    let heading = ns_string(&heading_for(status, &handles.product_name));
    let _: () = msg_send![usize_to_ptr(handles.heading), setStringValue: heading];
    let subheading = ns_string(&subheading_for(status, &handles.product_name));
    let _: () = msg_send![usize_to_ptr(handles.subheading), setStringValue: subheading];
    let footer = ns_string(&footer_for(status, &handles.product_name));
    let _: () = msg_send![usize_to_ptr(handles.footer), setStringValue: footer];
}

unsafe fn update_row(
    row: RowHandles,
    permission: MissingPermission,
    status: PermissionsStatus,
    in_progress: Option<MissingPermission>,
) {
    let granted = permission_is_granted(status, permission);
    // Swap icon symbol and tint.
    let symbol = if granted {
        "checkmark.circle.fill"
    } else {
        "circle"
    };
    let img = ns_image_for_symbol(symbol);
    let _: () = msg_send![row.icon, setImage: img];
    let tint: *mut AnyObject = if granted {
        msg_send![class!(NSColor), systemGreenColor]
    } else {
        msg_send![class!(NSColor), secondaryLabelColor]
    };
    let _: () = msg_send![row.icon, setContentTintColor: tint];
    // Title bold turns from default → secondary on grant (subtle visual
    // de-emphasis once granted).
    let title_color: *mut AnyObject = if granted {
        msg_send![class!(NSColor), secondaryLabelColor]
    } else {
        msg_send![class!(NSColor), labelColor]
    };
    let _: () = msg_send![row.title, setTextColor: title_color];

    let action = row_action(status, in_progress, permission);
    let title = ns_string(action.title());
    let _: () = msg_send![row.button, setTitle: title];
    let _: () = msg_send![row.button, setEnabled: action.enabled()];
}

// ── User-facing copy ────────────────────────────────────────────────────

fn heading_for(status: PermissionsStatus, product_name: &str) -> String {
    match status.grant_state() {
        PermissionGrantState::BothGranted => format!("{product_name} is ready"),
        PermissionGrantState::AccessibilityGranted
        | PermissionGrantState::ScreenRecordingGranted => "One more permission".into(),
        PermissionGrantState::NoneGranted => format!("Set up {product_name}"),
    }
}

fn subheading_for(status: PermissionsStatus, product_name: &str) -> String {
    match status.grant_state() {
        PermissionGrantState::BothGranted =>
            "Both macOS permissions are active. Computer control can start now.".into(),
        PermissionGrantState::AccessibilityGranted =>
            "Accessibility is allowed. Allow Screen Recording to finish setup.".into(),
        PermissionGrantState::ScreenRecordingGranted =>
            "Screen Recording is allowed. Allow Accessibility to finish setup.".into(),
        PermissionGrantState::NoneGranted => format!(
            "{product_name} needs two macOS permissions to see and interact with apps on your behalf."
        ),
    }
}

fn footer_for(status: PermissionsStatus, product_name: &str) -> String {
    if status.all_granted() {
        return format!("All set. {product_name} can now use apps on your Mac.");
    }
    format!(
        "macOS grants access to {product_name}, not your terminal. Revoke it anytime in Privacy & Security."
    )
}

// ── Helpers: NSString, labels, buttons, icons, rows ──────────────────────

/// Bridge `&str` → autoreleased `NSString` via `stringWithUTF8String:`.
unsafe fn ns_string(text: &str) -> *mut AnyObject {
    let owned = std::ffi::CString::new(text).unwrap_or_default();
    msg_send![class!(NSString), stringWithUTF8String: owned.as_ptr() as *const c_void]
}

/// The name macOS will show in Privacy & Security. Reading it from the main
/// bundle keeps the explainer aligned with the actual TCC identity after a
/// downstream app rebrands the driver.
fn product_name() -> String {
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        if !bundle.is_null() {
            for key in ["CFBundleDisplayName", "CFBundleName"] {
                let value: *mut AnyObject = msg_send![
                    bundle,
                    objectForInfoDictionaryKey: ns_string(key)
                ];
                if value.is_null() {
                    continue;
                }
                let utf8: *const std::ffi::c_char = msg_send![value, UTF8String];
                if utf8.is_null() {
                    continue;
                }
                let value = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
                if !value.trim().is_empty() {
                    return value.into_owned();
                }
            }
        }
    }
    "cmux cua".into()
}

unsafe fn ns_image_for_symbol(symbol: &str) -> *mut AnyObject {
    let name = ns_string(symbol);
    let desc = ns_string("permission status");
    let img: *mut AnyObject = msg_send![class!(NSImage),
        imageWithSystemSymbolName: name
        accessibilityDescription: desc
    ];
    img
}

unsafe fn build_label(
    text: &str,
    font_size: f64,
    bold: bool,
    origin: NSPoint,
    size: NSSize,
) -> *mut AnyObject {
    let frame = NSRect { origin, size };
    let label: *mut AnyObject = msg_send![class!(NSTextField), alloc];
    let label: *mut AnyObject = msg_send![label, initWithFrame: frame];
    let ns_text = ns_string(text);
    let _: () = msg_send![label, setStringValue: ns_text];
    let _: () = msg_send![label, setEditable: false];
    let _: () = msg_send![label, setBordered: false];
    let _: () = msg_send![label, setDrawsBackground: false];
    let _: () = msg_send![label, setSelectable: false];
    let _: () = msg_send![label, setUsesSingleLineMode: false];
    let cell: *mut AnyObject = msg_send![label, cell];
    if !cell.is_null() {
        // NSLineBreakByWordWrapping = 0.
        let _: () = msg_send![cell, setLineBreakMode: 0i64];
        let _: () = msg_send![cell, setWraps: true];
    }
    let font: *mut AnyObject = if bold {
        msg_send![class!(NSFont), boldSystemFontOfSize: font_size]
    } else {
        msg_send![class!(NSFont), systemFontOfSize: font_size]
    };
    let _: () = msg_send![label, setFont: font];
    label
}

#[derive(Debug, Clone, Copy)]
enum ButtonAction {
    Accessibility,
    ScreenRecording,
}

unsafe fn build_button(
    title: &str,
    origin: NSPoint,
    size: NSSize,
    action: ButtonAction,
) -> *mut AnyObject {
    let frame = NSRect { origin, size };
    let button: *mut AnyObject = msg_send![class!(NSButton), alloc];
    let button: *mut AnyObject = msg_send![button, initWithFrame: frame];
    let ns_title = ns_string(title);
    let _: () = msg_send![button, setTitle: ns_title];
    let _: () = msg_send![button, setBezelStyle: 1i64];
    let _: () = msg_send![button, setButtonType: 7i64];
    install_target(button, action);
    button
}

/// Build one permission row: status icon, title, explanation, and action.
unsafe fn build_row(
    permission: MissingPermission,
    status: PermissionsStatus,
    in_progress: Option<MissingPermission>,
    origin: NSPoint,
    size: NSSize,
) -> RowHandles {
    let container: *mut AnyObject = msg_send![class!(NSView), alloc];
    let container: *mut AnyObject = msg_send![container, initWithFrame: NSRect {
        origin,
        size,
    }];

    // Icon
    let icon_frame = NSRect {
        origin: NSPoint {
            x: 8.0,
            y: size.height - 32.0,
        },
        size: NSSize {
            width: 24.0,
            height: 24.0,
        },
    };
    let icon: *mut AnyObject = msg_send![class!(NSImageView), alloc];
    let icon: *mut AnyObject = msg_send![icon, initWithFrame: icon_frame];
    let granted = permission_is_granted(status, permission);
    let symbol = if granted {
        "checkmark.circle.fill"
    } else {
        "circle"
    };
    let img = ns_image_for_symbol(symbol);
    let _: () = msg_send![icon, setImage: img];
    let tint: *mut AnyObject = if granted {
        msg_send![class!(NSColor), systemGreenColor]
    } else {
        msg_send![class!(NSColor), secondaryLabelColor]
    };
    let _: () = msg_send![icon, setContentTintColor: tint];
    let _: () = msg_send![container, addSubview: icon];

    // Title
    let title = build_label(
        permission.label(),
        13.0,
        /*bold=*/ true,
        NSPoint {
            x: 44.0,
            y: size.height - 28.0,
        },
        NSSize {
            width: size.width - 270.0,
            height: 20.0,
        },
    );
    if granted {
        let c: *mut AnyObject = msg_send![class!(NSColor), secondaryLabelColor];
        let _: () = msg_send![title, setTextColor: c];
    }
    let _: () = msg_send![container, addSubview: title];

    // Subtitle (rationale)
    let subtitle = build_label(
        row_description(permission),
        11.0,
        /*bold=*/ false,
        NSPoint { x: 44.0, y: 8.0 },
        NSSize {
            width: size.width - 270.0,
            height: 40.0,
        },
    );
    let secondary: *mut AnyObject = msg_send![class!(NSColor), secondaryLabelColor];
    let _: () = msg_send![subtitle, setTextColor: secondary];
    let _: () = msg_send![container, addSubview: subtitle];

    let action = row_action(status, in_progress, permission);
    let button_action = match permission {
        MissingPermission::Accessibility => ButtonAction::Accessibility,
        MissingPermission::ScreenRecording => ButtonAction::ScreenRecording,
    };
    let button = build_button(
        action.title(),
        NSPoint {
            x: size.width - 210.0,
            y: 27.0,
        },
        NSSize {
            width: 202.0,
            height: 32.0,
        },
        button_action,
    );
    let _: () = msg_send![button, setEnabled: action.enabled()];
    let _: () = msg_send![container, addSubview: button];

    RowHandles {
        container,
        icon,
        title,
        button,
    }
}

fn row_description(permission: MissingPermission) -> &'static str {
    match permission {
        MissingPermission::Accessibility =>
            "Read interface text and use buttons, fields, menus, and other app controls.",
        MissingPermission::ScreenRecording =>
            "See app windows so actions can be placed accurately and their results verified.",
    }
}

// ── Button target/action plumbing + poll timer target ────────────────────

unsafe fn install_target(button: *mut AnyObject, action: ButtonAction) {
    let target = panel_target_instance();
    let _: () = msg_send![button, setTarget: target];
    let sel = match action {
        ButtonAction::Accessibility => objc2::sel!(accessibilityPermission:),
        ButtonAction::ScreenRecording => objc2::sel!(screenRecordingPermission:),
    };
    let _: () = msg_send![button, setAction: sel];
}

unsafe fn panel_target_instance() -> *mut AnyObject {
    let cls = panel_target_class();
    let inst: *mut AnyObject = msg_send![cls, alloc];
    let inst: *mut AnyObject = msg_send![inst, init];
    inst
}

/// Returns the runtime-registered `CuaDriverPermissionsPanelTarget`
/// class. Registration happens lazily on the first call and is then
/// memoised — Objective-C requires that any given runtime class name
/// is registered exactly once per process.
fn panel_target_class() -> &'static objc2::runtime::AnyClass {
    use objc2::declare::ClassBuilder;
    use std::sync::OnceLock;

    static CLASS: OnceLock<&'static objc2::runtime::AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let superclass = class!(NSObject);
        let mut builder = ClassBuilder::new("CuaDriverPermissionsPanelTarget", superclass)
            .expect("CuaDriverPermissionsPanelTarget already registered");
        unsafe {
            builder.add_method(
                objc2::sel!(accessibilityPermission:),
                on_accessibility_permission as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(screenRecordingPermission:),
                on_screen_recording_permission as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(pollTick:),
                on_poll_tick as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(windowWillClose:),
                on_window_will_close as extern "C" fn(_, _, _),
            );
        }
        builder.register()
    })
}

extern "C" fn on_accessibility_permission(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _sender: *mut AnyObject,
) {
    handle_permission_action(MissingPermission::Accessibility);
}

extern "C" fn on_screen_recording_permission(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _sender: *mut AnyObject,
) {
    handle_permission_action(MissingPermission::ScreenRecording);
}

fn handle_permission_action(permission: MissingPermission) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let action = HANDLES.with(|cell| {
            let guard = cell.borrow();
            guard
                .as_ref()
                .map(|handles| row_action(handles.last_status, handles.in_progress, permission))
        });

        match action {
            Some(RowAction::Allow) => {
                // Paint the in-progress state before invoking the TCC API. The
                // system prompt may immediately move focus to another app.
                HANDLES.with(|cell| unsafe {
                    let mut guard = cell.borrow_mut();
                    let Some(handles) = guard.as_mut() else { return };
                    handles.in_progress = Some(permission);
                    render_panel(handles, handles.last_status);
                });

                match permission {
                    MissingPermission::Accessibility => {
                        let _ = request_accessibility();
                    }
                    MissingPermission::ScreenRecording => {
                        let _ = request_screen_recording();
                    }
                }
                unsafe { poll_tick_inner() };
            }
            Some(RowAction::CompleteInSystemSettings) => {
                if let Err(error) = crate::permissions::gate::open_system_settings_for(permission) {
                    eprintln!(
                        "[cua-driver] could not open System Settings for {}: {error}",
                        permission.label()
                    );
                }
            }
            Some(RowAction::WaitingForOtherPermission | RowAction::Allowed) | None => {}
        }
    }));
    if let Err(error) = result {
        eprintln!("[cua-driver] permissions panel action panicked: {error:?}");
    }
}

extern "C" fn on_window_will_close(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _notification: *mut AnyObject,
) {
    OUTCOME.with(|cell| {
        let mut outcome = cell.borrow_mut();
        if outcome.is_none() {
            *outcome = Some(PanelOutcome::Dismissed);
        }
    });
    stop_modal();
}

fn stop_modal() {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, stopModal];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::permissions::test_env_lock()
    }

    #[test]
    fn explicit_off_values_disable_panel() {
        let _guard = env_lock();
        for v in &["0", "false", "FALSE", "False", "no", "NO", "off", "OFF"] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_PANEL", v);
            assert!(
                env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"),
                "value {v:?} must disable the panel",
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
    }

    #[test]
    fn truthy_and_unknown_values_leave_panel_enabled() {
        let _guard = env_lock();
        for v in &["1", "true", "yes", "on", "garbage", ""] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_PANEL", v);
            assert!(
                !env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"),
                "value {v:?} must not disable the panel",
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
    }

    #[test]
    fn unset_env_uses_bundled_panel_by_default() {
        let _guard = env_lock();
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
        assert!(!env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"));
    }

    #[test]
    fn embedded_host_never_uses_driver_owned_panel() {
        let _guard = env_lock();
        std::env::set_var(cua_driver_core::EMBEDDED_ENV, "1");
        assert!(!panel_enabled());
        std::env::remove_var(cua_driver_core::EMBEDDED_ENV);
    }

    #[test]
    fn copy_tracks_each_grant_state() {
        let st = |a: bool, sr: bool| PermissionsStatus {
            accessibility: a,
            screen_recording: sr,
        };
        assert_eq!(heading_for(st(false, false), "cmux cua"), "Set up cmux cua");
        assert_eq!(
            heading_for(st(true, false), "cmux cua"),
            "One more permission"
        );
        assert_eq!(
            heading_for(st(false, true), "cmux cua"),
            "One more permission"
        );
        assert_eq!(heading_for(st(true, true), "cmux cua"), "cmux cua is ready");
        assert!(subheading_for(st(true, false), "cmux cua").contains("Screen Recording"));
        assert!(subheading_for(st(false, true), "cmux cua").contains("Accessibility"));
        assert!(footer_for(st(false, false), "cmux cua").contains("not your terminal"));
    }

    #[test]
    fn only_one_permission_request_is_active() {
        let neither = PermissionsStatus {
            accessibility: false,
            screen_recording: false,
        };
        assert_eq!(
            row_action(neither, None, MissingPermission::Accessibility),
            RowAction::Allow
        );
        assert_eq!(
            row_action(neither, None, MissingPermission::ScreenRecording),
            RowAction::Allow
        );

        assert_eq!(
            row_action(
                neither,
                Some(MissingPermission::Accessibility),
                MissingPermission::Accessibility,
            ),
            RowAction::CompleteInSystemSettings
        );
        assert_eq!(
            row_action(
                neither,
                Some(MissingPermission::Accessibility),
                MissingPermission::ScreenRecording,
            ),
            RowAction::WaitingForOtherPermission
        );
    }

    #[test]
    fn granting_active_permission_advances_to_remaining_row() {
        let accessibility_only = PermissionsStatus {
            accessibility: true,
            screen_recording: false,
        };
        assert_eq!(
            row_action(accessibility_only, None, MissingPermission::Accessibility,),
            RowAction::Allowed
        );
        assert_eq!(
            row_action(accessibility_only, None, MissingPermission::ScreenRecording,),
            RowAction::Allow
        );
    }
}
