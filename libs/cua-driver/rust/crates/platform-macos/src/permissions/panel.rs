//! Native NSPanel onboarding UI — live status rows, dynamic heading,
//! auto-dismiss on all-green.
//!
//! Replaces the terminal banner when the daemon is launched from the
//! bundled `/Applications/CuaDriver.app`. Mirrors the Swift gate at
//! `libs/cua-driver/Sources/CuaDriverCore/Permissions/PermissionsGate.swift`.
//!
//! ## UX
//!
//! On first launch with missing grants the panel shows:
//!
//! ```text
//! ┌─ CuaDriver Permissions ─────────────────────┐
//! │  CuaDriver needs your permission            │
//! │  Grant both so CuaDriver can …              │
//! │                                             │
//! │  ✗ Accessibility                            │
//! │      lets cua-driver read the accessibility │
//! │      tree of running apps …                 │
//! │                                             │
//! │  ✗ Screen Recording                         │
//! │      lets cua-driver capture per-window …   │
//! │                                             │
//! │  ✅ All set. CuaDriver is ready to use.      │
//! │     (hidden until both grants flip green)   │
//! │                                             │
//! │  [ Continue anyway ]  [ Open Settings ]     │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! A 1 Hz `NSTimer` reads [`current_status`] on every tick and updates
//! the row icons / heading / subheading / "All set" strip in place.
//! When both grants flip green and `suppress_auto_close == false` the
//! poll callback calls `[NSApp stopModal]` and `show_modal` returns
//! [`PanelOutcome::AllGranted`] so the caller can skip the trailing
//! `wait_for_grants` loop.
//!
//! ## Threading
//!
//! `show_modal` MUST be called on the main thread. The Serve arm in
//! `cua-driver/src/main.rs` invokes the gate synchronously before any
//! threads spawn or NSApp.run() is called by the cursor overlay, so
//! the gate already runs on the main thread. `panel_enabled()`
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
use crate::permissions::status::{current_status, PermissionsStatus};

// ── Public API ──────────────────────────────────────────────────────────

/// What the user did with the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelOutcome {
    /// User asked us to open System Settings — caller should invoke
    /// `open_system_settings_for(...)` for each missing permission and
    /// then enter the polling phase.
    OpenSettings,
    /// User dismissed (Continue anyway button or red-dot close) — the
    /// caller should enter the polling phase without opening Settings.
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

/// Decide whether the panel can be shown right now. The panel is
/// **opt-in**: the default behaviour is the terminal banner, and the
/// panel only appears when:
///
///   * `CUA_DRIVER_RS_PERMISSIONS_PANEL` env var is set to an on
///     sentinel (`1` / `true` / `yes` / `on`, case-insensitive); AND
///   * the process is running on the main thread (a hard requirement
///     for AppKit calls); AND
///   * the binary lives inside an `.app` bundle (bare-binary
///     invocations have no Info.plist / bundle id for TCC to attribute
///     the window to, so they always use the terminal flow).
///
/// Rationale for opt-in: the panel relies on a chain of macOS
/// behaviours that are easy to break in practice — TCC responsible-
/// process attribution, session-level TCC caches that survive
/// `tccutil reset`, ad-hoc codesign identity mismatches between dev
/// builds and the bundle id, and AppKit modal-run-loop modes. When
/// any link is off, the panel can present a confusing UX (e.g. shows
/// "missing" rows because TCC returns stale cache, or auto-dismisses
/// against the user's intent). The terminal flow has none of those
/// failure modes, so it's the safer default until we have signals
/// that the UI is consistently helpful across users.
pub fn panel_enabled() -> bool {
    if !env_on("CUA_DRIVER_RS_PERMISSIONS_PANEL") {
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

fn env_on(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let lower = v.to_ascii_lowercase();
            matches!(lower.as_str(), "1" | "true" | "yes" | "on")
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
// Window geometry mirrors Swift's `.frame(width: 460)` plus heights
// chosen to fit: heading (24) + subheading (40) + 2 rows (76 each) +
// "All set" strip (32) + button row (48) + paddings (20·5). Keeping
// the numbers as a block here so the math is auditable.

const WIN_W: f64 = 460.0;
const WIN_H: f64 = 360.0;
const PAD: f64 = 20.0;
const ROW_H: f64 = 76.0;
const BUTTON_BAR_H: f64 = 56.0;
const READY_STRIP_H: f64 = 36.0;

unsafe fn show_modal_unsafe(opts: &PanelOpts) -> PanelOutcome {
    // ---- NSApplication setup ----
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    let _: bool = msg_send![app, setActivationPolicy: 1i64];
    let _: () = msg_send![app, finishLaunching];

    let main_screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if main_screen.is_null() {
        return PanelOutcome::Dismissed;
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
        return PanelOutcome::Dismissed;
    }
    let _: () = msg_send![window, setReleasedWhenClosed: false];
    let _: () = msg_send![window, setHidesOnDeactivate: false];
    // NSFloatingWindowLevel (3) so it stays on top of System Settings
    // when the user navigates over to grant permissions.
    let _: () = msg_send![window, setLevel: 3i64];
    let title = ns_string("CuaDriver Permissions");
    let _: () = msg_send![window, setTitle: title];

    let content_view: *mut AnyObject = msg_send![window, contentView];

    // ---- Heading + subheading ----
    //
    // Y coordinates use AppKit's bottom-left origin. We lay out top-
    // down: subtract row heights from `WIN_H - PAD` as we go.
    let mut y = WIN_H - PAD - 24.0;

    let heading = build_label(
        heading_for(opts.initial_status),
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
        subheading_for(opts.initial_status),
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
        opts.initial_status.accessibility,
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
        opts.initial_status.screen_recording,
        NSPoint { x: PAD, y: y - ROW_H },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: ROW_H,
        },
    );
    let _: () = msg_send![content_view, addSubview: sr_row.container];
    y -= ROW_H + 8.0;

    // ---- Ready strip (initially hidden; shown by poll on all-green) ----
    let ready_strip = build_ready_strip(NSPoint { x: PAD, y: y - READY_STRIP_H }, WIN_W - 2.0 * PAD);
    let _: () = msg_send![content_view, addSubview: ready_strip];
    let initially_all_green =
        opts.initial_status.accessibility && opts.initial_status.screen_recording;
    let _: () = msg_send![ready_strip, setHidden: !initially_all_green];

    // ---- Buttons across the bottom ----
    OUTCOME.with(|cell| *cell.borrow_mut() = None);

    let continue_btn = build_button(
        "Continue anyway",
        NSPoint { x: PAD, y: 16.0 },
        NSSize {
            width: 160.0,
            height: 32.0,
        },
        ButtonAction::Dismiss,
    );
    let _: () = msg_send![content_view, addSubview: continue_btn];

    let open_settings_btn = build_button(
        "Open System Settings",
        NSPoint {
            x: WIN_W - PAD - 180.0,
            y: 16.0,
        },
        NSSize {
            width: 180.0,
            height: 32.0,
        },
        ButtonAction::OpenSettings,
    );
    let _: () = msg_send![content_view, addSubview: open_settings_btn];
    let _: () = msg_send![open_settings_btn, setKeyEquivalent: ns_string("\r")];

    // ---- Stash handles for the timer callback ----
    HANDLES.with(|cell| {
        *cell.borrow_mut() = Some(PanelHandles {
            window: ptr_to_usize(window),
            heading: ptr_to_usize(heading),
            subheading: ptr_to_usize(subheading),
            ready_strip: ptr_to_usize(ready_strip),
            ax_row,
            sr_row,
            last_status: opts.initial_status,
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
}

struct PanelHandles {
    window: usize,
    heading: usize,
    subheading: usize,
    ready_strip: usize,
    ax_row: RowHandles,
    sr_row: RowHandles,
    last_status: PermissionsStatus,
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
        // Update row icons + label color tints.
        update_row(handles.ax_row, status.accessibility);
        update_row(handles.sr_row, status.screen_recording);
        // Update heading + subheading copy.
        let new_heading = ns_string(heading_for(status));
        let _: () = msg_send![usize_to_ptr(handles.heading), setStringValue: new_heading];
        let new_subheading = ns_string(subheading_for(status));
        let _: () = msg_send![usize_to_ptr(handles.subheading), setStringValue: new_subheading];
        // Toggle the ready strip.
        let all_green = status.accessibility && status.screen_recording;
        let _: () = msg_send![usize_to_ptr(handles.ready_strip), setHidden: !all_green];
        handles.last_status = status;
        if all_green {
            should_stop = true;
        }
    });
    if should_stop {
        OUTCOME.with(|cell| *cell.borrow_mut() = Some(PanelOutcome::AllGranted));
        stop_modal();
    }
}

unsafe fn update_row(row: RowHandles, granted: bool) {
    // Swap icon symbol and tint.
    let symbol = if granted {
        "checkmark.circle.fill"
    } else {
        "xmark.circle.fill"
    };
    let img = ns_image_for_symbol(symbol);
    let _: () = msg_send![row.icon, setImage: img];
    let tint: *mut AnyObject = if granted {
        msg_send![class!(NSColor), systemGreenColor]
    } else {
        msg_send![class!(NSColor), systemRedColor]
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
}

// ── Heading / subheading copy (parity with Swift) ────────────────────────

fn heading_for(s: PermissionsStatus) -> &'static str {
    match (s.accessibility, s.screen_recording) {
        (true, true) => "CuaDriver is ready",
        (true, false) | (false, true) => "One more permission",
        (false, false) => "CuaDriver needs your permission",
    }
}

fn subheading_for(s: PermissionsStatus) -> &'static str {
    match (s.accessibility, s.screen_recording) {
        (true, true) =>
            "Both permissions are granted. You can close this window whenever you're ready.",
        (true, false) =>
            "Accessibility is granted. Now grant Screen Recording in the System Settings window that just opened.",
        (false, true) =>
            "Screen Recording is granted. Now grant Accessibility in the System Settings window that just opened.",
        (false, false) =>
            "Grant both so CuaDriver can inspect and drive native apps on your behalf. This window closes on its own once each item turns green.",
    }
}

// ── Helpers: NSString, labels, buttons, icons, rows ──────────────────────

/// Bridge `&str` → autoreleased `NSString` via `stringWithUTF8String:`.
unsafe fn ns_string(text: &str) -> *mut AnyObject {
    let owned = std::ffi::CString::new(text).unwrap_or_default();
    msg_send![class!(NSString), stringWithUTF8String: owned.as_ptr() as *const c_void]
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
    OpenSettings,
    Dismiss,
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

/// Build one permission row: container NSView with three subviews
/// (icon, title, subtitle). Returns handles so the poll timer can
/// update the icon + title tint live.
unsafe fn build_row(
    permission: MissingPermission,
    granted: bool,
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
    let symbol = if granted {
        "checkmark.circle.fill"
    } else {
        "xmark.circle.fill"
    };
    let img = ns_image_for_symbol(symbol);
    let _: () = msg_send![icon, setImage: img];
    let tint: *mut AnyObject = if granted {
        msg_send![class!(NSColor), systemGreenColor]
    } else {
        msg_send![class!(NSColor), systemRedColor]
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
            width: size.width - 60.0,
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
        permission.rationale(),
        11.0,
        /*bold=*/ false,
        NSPoint { x: 44.0, y: 8.0 },
        NSSize {
            width: size.width - 60.0,
            height: 40.0,
        },
    );
    let secondary: *mut AnyObject = msg_send![class!(NSColor), secondaryLabelColor];
    let _: () = msg_send![subtitle, setTextColor: secondary];
    let _: () = msg_send![container, addSubview: subtitle];

    RowHandles {
        container,
        icon,
        title,
    }
}

unsafe fn build_ready_strip(origin: NSPoint, width: f64) -> *mut AnyObject {
    let strip = build_label(
        "✅  All set. CuaDriver is ready to use.",
        12.0,
        /*bold=*/ false,
        origin,
        NSSize {
            width,
            height: READY_STRIP_H,
        },
    );
    let green: *mut AnyObject = msg_send![class!(NSColor), systemGreenColor];
    let _: () = msg_send![strip, setTextColor: green];
    strip
}

// ── Button target/action plumbing + poll timer target ────────────────────

unsafe fn install_target(button: *mut AnyObject, action: ButtonAction) {
    let target = panel_target_instance();
    let _: () = msg_send![button, setTarget: target];
    let sel = match action {
        ButtonAction::OpenSettings => objc2::sel!(openSettings:),
        ButtonAction::Dismiss => objc2::sel!(dismiss:),
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
                objc2::sel!(openSettings:),
                on_open_settings as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(dismiss:),
                on_dismiss as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(pollTick:),
                on_poll_tick as extern "C" fn(_, _, _),
            );
        }
        builder.register()
    })
}

extern "C" fn on_open_settings(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _sender: *mut AnyObject,
) {
    OUTCOME.with(|cell| *cell.borrow_mut() = Some(PanelOutcome::OpenSettings));
    stop_modal();
}

extern "C" fn on_dismiss(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _sender: *mut AnyObject,
) {
    OUTCOME.with(|cell| *cell.borrow_mut() = Some(PanelOutcome::Dismissed));
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
    use std::sync::{Mutex, OnceLock};

    static PANEL_ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        PANEL_ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn env_on_recognises_documented_sentinels() {
        let _guard = env_lock();
        for v in &["1", "true", "TRUE", "True", "yes", "YES", "on", "ON"] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_PANEL", v);
            assert!(
                env_on("CUA_DRIVER_RS_PERMISSIONS_PANEL"),
                "value {v:?} must be treated as on",
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
    }

    #[test]
    fn env_on_ignores_falsy_and_unknown() {
        let _guard = env_lock();
        for v in &["0", "false", "no", "off", "garbage", ""] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_PANEL", v);
            assert!(
                !env_on("CUA_DRIVER_RS_PERMISSIONS_PANEL"),
                "value {v:?} must not be treated as on",
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
    }

    #[test]
    fn unset_env_keeps_panel_disabled_by_default() {
        // Opt-in default: with no env var set, the panel does not show
        // and the gate falls back to the terminal banner.
        let _guard = env_lock();
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
        assert!(!env_on("CUA_DRIVER_RS_PERMISSIONS_PANEL"));
    }

    #[test]
    fn heading_parity_with_swift() {
        // Verbatim parity with PermissionsGate.swift lines 239-244.
        let st = |a: bool, sr: bool| PermissionsStatus {
            accessibility: a,
            screen_recording: sr,
        };
        assert_eq!(heading_for(st(true, true)), "CuaDriver is ready");
        assert_eq!(heading_for(st(true, false)), "One more permission");
        assert_eq!(heading_for(st(false, true)), "One more permission");
        assert_eq!(
            heading_for(st(false, false)),
            "CuaDriver needs your permission"
        );
    }

    #[test]
    fn subheading_mentions_remaining_permission() {
        let st = |a: bool, sr: bool| PermissionsStatus {
            accessibility: a,
            screen_recording: sr,
        };
        // Only Accessibility granted → mentions Screen Recording next.
        assert!(subheading_for(st(true, false)).contains("Screen Recording"));
        // Only Screen Recording granted → mentions Accessibility next.
        assert!(subheading_for(st(false, true)).contains("Accessibility"));
    }
}
