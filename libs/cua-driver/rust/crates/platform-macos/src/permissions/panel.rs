// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The permission guide window: one step at a time, in plain words.
//!
//! Shown by the startup gate when the bundled app is missing a permission.
//! It follows the guide window in Cua Spaces (a step, a reason, a button to
//! the right System Settings pane, and an app tile to drag into the list),
//! drawn natively with AppKit through raw `objc2::msg_send!`, like
//! `cursor/overlay.rs`.
//!
//! ```text
//! ┌─ Set up Cua Driver ─────────────────────────────┐
//! │ Step 1 of 2                                     │
//! │ Let Cua Driver use your mouse and keyboard      │
//! │ This lets Cua Driver click and type in your     │
//! │ apps when an agent asks it to.                  │
//! │ In System Settings, turn on the switch next to  │
//! │ Cua Driver.                                     │
//! │ ◌ Waiting for the switch to turn on…            │
//! │ [ Open System Settings ]                        │
//! │                                                 │
//! │ If Cua Driver isn't in the list, drag this in:  │
//! │ ┌─────────────────────────────────────────────┐ │
//! │ │ [icon]  Cua Driver                    ✋ Drag │ │
//! │ └─────────────────────────────────────────────┘ │
//! │                                     [ Not Now ] │
//! └─────────────────────────────────────────────────┘
//! ```
//!
//! A 1 Hz timer reads a fresh status (from a short-lived helper process,
//! see `gate::fresh_status`) and feeds it to [`GuideMachine`], which moves
//! to the next step by itself, asks macOS for Screen Recording only once
//! Accessibility is on, and opens the next pane. When both are on it shows
//! "All set. Cua Driver can now use this Mac." and closes itself.
//!
//! ## Threading
//!
//! Everything here runs on the main thread: `show_modal` is called by the
//! gate before the cursor overlay starts `NSApp.run()`, and `NSTimer`
//! callbacks fire on the main run loop.

use std::cell::RefCell;
use std::ffi::c_void;

use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{class, msg_send};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

use crate::permissions::gate::MissingPermission;
use crate::permissions::guide::{self, GuideEffect, GuideMachine, GuideStep};
use crate::permissions::status::PermissionsStatus;

// ── Public API ──────────────────────────────────────────────────────────

/// How the guide ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelOutcome {
    /// The person clicked "Not Now" or closed the window. The caller keeps
    /// waiting in the background so a later grant still unblocks.
    Dismissed,
    /// Both switches are on; the window showed "All set" and closed.
    AllGranted,
}

/// What the window does when it needs a fresh status, a permission
/// request, or a System Settings pane. The gate passes the real ones; the
/// preview example passes fixed ones.
pub struct PanelHooks {
    pub status: Box<dyn FnMut() -> PermissionsStatus>,
    pub request: Box<dyn FnMut(MissingPermission)>,
    pub open_settings: Box<dyn FnMut(MissingPermission)>,
}

/// Inputs for [`show_modal`].
pub struct PanelOpts {
    /// The status the gate already read, for the first render.
    pub initial_status: PermissionsStatus,
    /// Permissions the gate already asked macOS for before the window
    /// opened (so they aren't asked twice).
    pub already_requested: Vec<MissingPermission>,
    pub hooks: PanelHooks,
}

/// Whether the guide window can be shown: on by default for the bundled
/// app on the main thread. `CUA_DRIVER_RS_PERMISSIONS_PANEL=0` (or
/// `false` / `no` / `off`) turns it off and keeps the terminal text only.
/// Bare binaries outside an `.app` always use the terminal text: macOS has
/// no app identity to list for them.
pub fn panel_enabled() -> bool {
    if env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL") {
        return false;
    }
    if MainThreadMarker::new().is_none() {
        return false;
    }
    running_inside_app_bundle()
}

/// Show the guide modally. Blocks the main thread until both switches are
/// on, or the person clicks "Not Now" / closes the window. `None` when no
/// window could be shown (no display), so the caller falls back to the
/// terminal text.
pub fn show_modal(opts: PanelOpts) -> Option<PanelOutcome> {
    let _mtm = MainThreadMarker::new().expect("show_modal must be called from the main thread");
    let machine = GuideMachine::new(opts.initial_status, &opts.already_requested);
    unsafe { run_window(machine, opts.hooks) }
}

/// Show only the finished state ("All set…") for a moment, then close.
/// Used when macOS's "Quit & Reopen" restarted the app mid-guide.
pub fn show_finished(hooks: PanelHooks) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let done = PermissionsStatus {
        accessibility: true,
        screen_recording: true,
    };
    let machine = GuideMachine::new(done, &[]);
    unsafe {
        let _ = run_window(machine, hooks);
    }
}

/// Block the current (main) thread for `seconds` while letting the
/// AppKit run loop process events. Falls back to `thread::sleep` off the
/// main thread.
pub fn pump_run_loop_briefly(seconds: f64) {
    if MainThreadMarker::new().is_none() {
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
        return;
    }
    unsafe {
        let run_loop: *mut AnyObject = msg_send![class!(NSRunLoop), currentRunLoop];
        if run_loop.is_null() {
            std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
            return;
        }
        let date: *mut AnyObject = msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: seconds];
        if date.is_null() {
            std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
            return;
        }
        let default_mode = ns_string("NSDefaultRunLoopMode");
        let _: bool = msg_send![run_loop, runMode: default_mode beforeDate: date];
    }
}

// ── Environment ─────────────────────────────────────────────────────────

fn env_off(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let lower = v.to_ascii_lowercase();
            matches!(lower.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(false)
}

/// The `.app` bundle this executable lives in, if any.
fn app_bundle_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.ancestors()
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
        .map(std::path::Path::to_path_buf)
}

fn running_inside_app_bundle() -> bool {
    app_bundle_path().is_some()
}

// ── Layout ──────────────────────────────────────────────────────────────
//
// Fixed layout, measured from the top of the content view (AppKit's origin
// is bottom-left, so `top()` converts).

const WIN_W: f64 = 480.0;
const WIN_H: f64 = 420.0;
const PAD: f64 = 24.0;
const CONTENT_W: f64 = WIN_W - 2.0 * PAD;
const TILE_H: f64 = 60.0;
const ICON: f64 = 36.0;
/// Ticks (1 s each) the finished state stays up before the window closes.
const DONE_TICKS: u32 = 2;

fn top(y_from_top: f64, height: f64) -> f64 {
    WIN_H - y_from_top - height
}

fn rect(x: f64, y_from_top: f64, w: f64, h: f64) -> NSRect {
    NSRect {
        origin: NSPoint {
            x,
            y: top(y_from_top, h),
        },
        size: NSSize {
            width: w,
            height: h,
        },
    }
}

// ── Window ──────────────────────────────────────────────────────────────

/// Raw view pointers, kept as `usize` (AppKit views are `!Send`; they are
/// only touched on the main thread).
struct Views {
    window: usize,
    step_view: usize,
    done_view: usize,
    counter: usize,
    title: usize,
    why: usize,
    how: usize,
    note: usize,
}

struct Session {
    machine: GuideMachine,
    hooks: PanelHooks,
    views: Views,
    done_ticks: u32,
}

thread_local! {
    static OUTCOME: RefCell<Option<PanelOutcome>> = const { RefCell::new(None) };
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    static TILE_BUNDLE: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
}

fn ptr(u: usize) -> *mut AnyObject {
    u as *mut AnyObject
}

unsafe fn run_window(machine: GuideMachine, hooks: PanelHooks) -> Option<PanelOutcome> {
    // AppKit uses Text Input Sources on this thread from here on.
    crate::input::input_source::main_thread_runs_event_loop();
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    let _: bool = msg_send![app, setActivationPolicy: 1i64];
    let _: () = msg_send![app, finishLaunching];

    let main_screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if main_screen.is_null() {
        return None;
    }

    let content_rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: WIN_W,
            height: WIN_H,
        },
    };
    // Titled | Closable.
    let style_mask: u64 = 1 | 2;
    let window: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSPanel), alloc];
        msg_send![allocated,
            initWithContentRect: content_rect
            styleMask: style_mask
            backing: 2u64
            defer: false
        ]
    };
    if window.is_null() {
        return None;
    }
    let _: () = msg_send![window, setReleasedWhenClosed: false];
    let _: () = msg_send![window, setHidesOnDeactivate: false];
    // Floating, so it stays above System Settings while the person works
    // there.
    let _: () = msg_send![window, setLevel: 3i64];
    let _: () = msg_send![window, setTitle: ns_string(guide::WINDOW_TITLE)];
    let target = target_instance();
    let _: () = msg_send![window, setDelegate: target];

    let content: *mut AnyObject = msg_send![window, contentView];

    // ---- Step view ----
    let step_view = new_view(rect(0.0, 0.0, WIN_W, WIN_H));
    let _: () = msg_send![content, addSubview: step_view];

    let counter = label("", 12.0, false, rect(PAD, 20.0, CONTENT_W, 16.0));
    set_secondary(counter);
    let _: () = msg_send![step_view, addSubview: counter];

    let title = label("", 18.0, true, rect(PAD, 40.0, CONTENT_W, 26.0));
    let _: () = msg_send![step_view, addSubview: title];

    let why = label("", 13.0, false, rect(PAD, 72.0, CONTENT_W, 36.0));
    set_secondary(why);
    let _: () = msg_send![step_view, addSubview: why];

    let how = label("", 13.0, false, rect(PAD, 114.0, CONTENT_W, 18.0));
    let _: () = msg_send![step_view, addSubview: how];

    // Live status: a small spinner while waiting for the switch.
    let spinner: *mut AnyObject = msg_send![class!(NSProgressIndicator), alloc];
    let spinner: *mut AnyObject = msg_send![spinner, initWithFrame: rect(PAD, 144.0, 16.0, 16.0)];
    let _: () = msg_send![spinner, setStyle: 1u64]; // spinning
    let _: () = msg_send![spinner, setControlSize: 1u64]; // small
    let _: () = msg_send![spinner, setIndeterminate: true];
    let _: () = msg_send![spinner, startAnimation: std::ptr::null::<AnyObject>()];
    let _: () = msg_send![step_view, addSubview: spinner];
    let waiting = label(
        guide::WAITING_LINE,
        13.0,
        false,
        rect(PAD + 24.0, 142.0, CONTENT_W - 24.0, 18.0),
    );
    set_secondary(waiting);
    let _: () = msg_send![step_view, addSubview: waiting];

    let open_btn = button(
        guide::OPEN_SETTINGS_BUTTON,
        rect(PAD, 172.0, 200.0, 32.0),
        objc2::sel!(openSettings:),
        target,
    );
    let _: () = msg_send![open_btn, setKeyEquivalent: ns_string("\r")];
    let _: () = msg_send![step_view, addSubview: open_btn];

    let hint = label(
        guide::DRAG_HINT,
        12.0,
        false,
        rect(PAD, 228.0, CONTENT_W, 18.0),
    );
    set_secondary(hint);
    let _: () = msg_send![step_view, addSubview: hint];

    let bundle = app_bundle_path();
    TILE_BUNDLE.with(|cell| *cell.borrow_mut() = bundle.clone());
    let tile = drag_tile(rect(PAD, 250.0, CONTENT_W, TILE_H), bundle.as_deref());
    let _: () = msg_send![step_view, addSubview: tile];

    let note = label("", 11.0, false, rect(PAD, 320.0, CONTENT_W, 32.0));
    set_secondary(note);
    let _: () = msg_send![step_view, addSubview: note];

    let not_now = button(
        guide::NOT_NOW_BUTTON,
        rect(WIN_W - PAD - 110.0, WIN_H - 16.0 - 32.0, 110.0, 32.0),
        objc2::sel!(notNow:),
        target,
    );
    let _: () = msg_send![not_now, setKeyEquivalent: ns_string("\u{1b}")];
    let _: () = msg_send![step_view, addSubview: not_now];

    // ---- Finished view ----
    let done_view = new_view(rect(0.0, 0.0, WIN_W, WIN_H));
    let check: *mut AnyObject = msg_send![class!(NSImageView), alloc];
    let check: *mut AnyObject =
        msg_send![check, initWithFrame: rect((WIN_W - 56.0) / 2.0, 110.0, 56.0, 56.0)];
    let check_img = symbol("checkmark.circle.fill");
    if !check_img.is_null() {
        let config: *mut AnyObject = msg_send![
            class!(NSImageSymbolConfiguration),
            configurationWithPointSize: 48.0_f64
            weight: 0.0_f64
        ];
        let sized: *mut AnyObject = if config.is_null() {
            check_img
        } else {
            msg_send![check_img, imageWithSymbolConfiguration: config]
        };
        let _: () = msg_send![check, setImage: sized];
    }
    let green: *mut AnyObject = msg_send![class!(NSColor), systemGreenColor];
    let _: () = msg_send![check, setContentTintColor: green];
    let _: () = msg_send![done_view, addSubview: check];
    let done_title = label(
        guide::DONE_TITLE,
        17.0,
        true,
        rect(PAD, 184.0, CONTENT_W, 24.0),
    );
    center_text(done_title);
    let _: () = msg_send![done_view, addSubview: done_title];
    let done_why = label(
        GuideStep::Done.copy().why,
        13.0,
        false,
        rect(PAD, 214.0, CONTENT_W, 18.0),
    );
    center_text(done_why);
    set_secondary(done_why);
    let _: () = msg_send![done_view, addSubview: done_why];
    let _: () = msg_send![content, addSubview: done_view];

    let views = Views {
        window: window as usize,
        step_view: step_view as usize,
        done_view: done_view as usize,
        counter: counter as usize,
        title: title as usize,
        why: why as usize,
        how: how as usize,
        note: note as usize,
    };
    render(&views, machine.step());
    OUTCOME.with(|cell| *cell.borrow_mut() = None);
    SESSION.with(|cell| {
        *cell.borrow_mut() = Some(Session {
            machine,
            hooks,
            views,
            done_ticks: 0,
        })
    });

    // Top right of the screen, clear of System Settings, which opens in the
    // middle.
    let visible: NSRect = msg_send![main_screen, visibleFrame];
    let origin = NSPoint {
        x: visible.origin.x + visible.size.width - WIN_W - 24.0,
        y: visible.origin.y + visible.size.height - WIN_H - 48.0,
    };
    let _: () = msg_send![window, setFrameOrigin: origin];
    let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
    let _: () = msg_send![window, orderFrontRegardless];
    let _: () = msg_send![app, activateIgnoringOtherApps: true];

    let timer = schedule_tick(target);
    // Run one tick now so a pending request goes out without a 1 s delay.
    tick();
    if OUTCOME.with(|cell| cell.borrow().is_none()) {
        let _: i64 = msg_send![app, runModalForWindow: window];
    }
    let _: () = msg_send![timer, invalidate];
    let _: () = msg_send![window, setDelegate: std::ptr::null::<AnyObject>()];

    let outcome = OUTCOME
        .with(|cell| *cell.borrow())
        .unwrap_or(PanelOutcome::Dismissed);
    SESSION.with(|cell| *cell.borrow_mut() = None);
    TILE_BUNDLE.with(|cell| *cell.borrow_mut() = None);
    let _: () = msg_send![window, orderOut: std::ptr::null::<AnyObject>()];
    Some(outcome)
}

/// Draw `step`: the step's words, or the finished view.
unsafe fn render(views: &Views, step: GuideStep) {
    let done = step == GuideStep::Done;
    let _: () = msg_send![ptr(views.step_view), setHidden: done];
    let _: () = msg_send![ptr(views.done_view), setHidden: !done];
    if done {
        return;
    }
    let copy = step.copy();
    let _: () = msg_send![ptr(views.counter), setStringValue: ns_string(copy.counter)];
    let _: () = msg_send![ptr(views.title), setStringValue: ns_string(copy.title)];
    let _: () = msg_send![ptr(views.why), setStringValue: ns_string(copy.why)];
    let _: () = msg_send![ptr(views.how), setStringValue: ns_string(copy.how)];
    let _: () = msg_send![ptr(views.note), setStringValue: ns_string(copy.note)];
    let _: () = msg_send![ptr(views.window), setTitle: ns_string(guide::WINDOW_TITLE)];
}

// ── Timer, buttons, window close ─────────────────────────────────────────

unsafe fn schedule_tick(target: *mut AnyObject) -> *mut AnyObject {
    // `runModalForWindow:` runs the loop in NSModalPanelRunLoopMode, which a
    // default-scheduled timer never fires in, so add it to that mode.
    let timer: *mut AnyObject = msg_send![class!(NSTimer),
        timerWithTimeInterval: 1.0_f64
        target: target
        selector: objc2::sel!(pollTick:)
        userInfo: std::ptr::null::<AnyObject>()
        repeats: true
    ];
    let run_loop: *mut AnyObject = msg_send![class!(NSRunLoop), currentRunLoop];
    let _: () = msg_send![run_loop, addTimer: timer forMode: ns_string("NSModalPanelRunLoopMode")];
    timer
}

/// One status check: feed the machine, apply its effects, and close a
/// moment after the finished state shows.
fn tick() {
    let mut finish = false;
    SESSION.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(session) = guard.as_mut() else {
            return;
        };
        if session.machine.step() == GuideStep::Done {
            session.done_ticks += 1;
            finish = session.done_ticks >= DONE_TICKS;
            return;
        }
        let status = (session.hooks.status)();
        for effect in session.machine.on_status(status) {
            match effect {
                GuideEffect::Show(step) => unsafe {
                    if step == GuideStep::ScreenRecording {
                        guide::write_resume_marker();
                    }
                    render(&session.views, step);
                },
                GuideEffect::Request(permission) => {
                    if permission == MissingPermission::ScreenRecording {
                        guide::write_resume_marker();
                    }
                    (session.hooks.request)(permission);
                }
                GuideEffect::OpenSettings(permission) => {
                    (session.hooks.open_settings)(permission);
                }
            }
        }
    });
    if finish {
        OUTCOME.with(|cell| *cell.borrow_mut() = Some(PanelOutcome::AllGranted));
        stop_modal();
    }
}

extern "C" fn on_poll_tick(_this: *mut AnyObject, _cmd: Sel, _timer: *mut AnyObject) {
    // Never let a panic hang the window: close it instead.
    if std::panic::catch_unwind(tick).is_err() {
        eprintln!("[cua-driver] permission guide check failed; closing the guide");
        OUTCOME.with(|cell| *cell.borrow_mut() = Some(PanelOutcome::Dismissed));
        stop_modal();
    }
}

extern "C" fn on_open_settings(_this: *mut AnyObject, _cmd: Sel, _sender: *mut AnyObject) {
    SESSION.with(|cell| {
        if let Some(session) = cell.borrow_mut().as_mut() {
            if let Some(permission) = session.machine.step().permission() {
                (session.hooks.open_settings)(permission);
            }
        }
    });
}

extern "C" fn on_not_now(_this: *mut AnyObject, _cmd: Sel, _sender: *mut AnyObject) {
    OUTCOME.with(|cell| *cell.borrow_mut() = Some(PanelOutcome::Dismissed));
    stop_modal();
}

extern "C" fn on_window_will_close(
    _this: *mut AnyObject,
    _cmd: Sel,
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

/// End the modal loop. `stopModal` from a timer only takes effect on the
/// next event, so post an empty one to wake the loop.
fn stop_modal() {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, stopModal];
        // NSEventTypeApplicationDefined = 15.
        let event: *mut AnyObject = msg_send![class!(NSEvent),
            otherEventWithType: 15u64
            location: NSPoint { x: 0.0, y: 0.0 }
            modifierFlags: 0u64
            timestamp: 0.0_f64
            windowNumber: 0i64
            context: std::ptr::null::<AnyObject>()
            subtype: 0i16
            data1: 0i64
            data2: 0i64
        ];
        if !event.is_null() {
            let _: () = msg_send![app, postEvent: event atStart: true];
        }
    }
}

unsafe fn target_instance() -> *mut AnyObject {
    let cls = target_class();
    let inst: *mut AnyObject = msg_send![cls, alloc];
    msg_send![inst, init]
}

/// `CuaDriverPermissionGuideTarget`: button actions, the timer, and the
/// window delegate. Registered once per process.
fn target_class() -> &'static objc2::runtime::AnyClass {
    use objc2::declare::ClassBuilder;
    use std::sync::OnceLock;

    static CLASS: OnceLock<&'static objc2::runtime::AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut builder = ClassBuilder::new("CuaDriverPermissionGuideTarget", class!(NSObject))
            .expect("CuaDriverPermissionGuideTarget already registered");
        unsafe {
            builder.add_method(
                objc2::sel!(openSettings:),
                on_open_settings as extern "C" fn(_, _, _),
            );
            builder.add_method(objc2::sel!(notNow:), on_not_now as extern "C" fn(_, _, _));
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

// ── The draggable app tile ───────────────────────────────────────────────
//
// A plain card (same border all round) with the app's icon and name. A
// drag puts the app bundle's file URL on the pasteboard, which System
// Settings' privacy lists accept, the same as dragging the app from Finder.

unsafe fn drag_tile(frame: NSRect, bundle: Option<&std::path::Path>) -> *mut AnyObject {
    let cls = tile_class();
    let tile: *mut AnyObject = msg_send![cls, alloc];
    let tile: *mut AnyObject = msg_send![tile, initWithFrame: frame];
    let _: () = msg_send![tile, setWantsLayer: true];
    let layer: *mut AnyObject = msg_send![tile, layer];
    if !layer.is_null() {
        let _: () = msg_send![layer, setCornerRadius: 10.0_f64];
        let _: () = msg_send![layer, setBorderWidth: 1.0_f64];
        let border: *mut AnyObject = msg_send![class!(NSColor), separatorColor];
        let border_cg: CGColorRef = msg_send![border, CGColor];
        let _: () = msg_send![layer, setBorderColor: border_cg];
        let fill: *mut AnyObject = msg_send![class!(NSColor), controlBackgroundColor];
        let fill_cg: CGColorRef = msg_send![fill, CGColor];
        let _: () = msg_send![layer, setBackgroundColor: fill_cg];
    }
    let height = frame.size.height;
    let width = frame.size.width;

    let icon_view: *mut AnyObject = msg_send![class!(NSImageView), alloc];
    let icon_frame = NSRect {
        origin: NSPoint {
            x: 12.0,
            y: (height - ICON) / 2.0,
        },
        size: NSSize {
            width: ICON,
            height: ICON,
        },
    };
    let icon_view: *mut AnyObject = msg_send![icon_view, initWithFrame: icon_frame];
    let icon = app_icon(bundle);
    if !icon.is_null() {
        let _: () = msg_send![icon_view, setImage: icon];
    }
    let _: () = msg_send![tile, addSubview: icon_view];

    let name = label(
        guide::TILE_NAME,
        15.0,
        false,
        NSRect {
            origin: NSPoint {
                x: 12.0 + ICON + 12.0,
                y: (height - 20.0) / 2.0,
            },
            size: NSSize {
                width: width - ICON - 120.0,
                height: 20.0,
            },
        },
    );
    let _: () = msg_send![tile, addSubview: name];

    let hand: *mut AnyObject = msg_send![class!(NSImageView), alloc];
    let hand: *mut AnyObject = msg_send![hand, initWithFrame: NSRect {
        origin: NSPoint { x: width - 46.0, y: height / 2.0 - 2.0 },
        size: NSSize { width: 20.0, height: 20.0 },
    }];
    let hand_img = symbol("hand.draw");
    if !hand_img.is_null() {
        let _: () = msg_send![hand, setImage: hand_img];
    }
    let secondary: *mut AnyObject = msg_send![class!(NSColor), secondaryLabelColor];
    let _: () = msg_send![hand, setContentTintColor: secondary];
    let _: () = msg_send![tile, addSubview: hand];
    let drag = label(
        guide::TILE_DRAG_LABEL,
        10.0,
        false,
        NSRect {
            origin: NSPoint {
                x: width - 60.0,
                y: height / 2.0 - 18.0,
            },
            size: NSSize {
                width: 48.0,
                height: 14.0,
            },
        },
    );
    set_secondary(drag);
    center_text(drag);
    let _: () = msg_send![tile, addSubview: drag];

    // Tooltip, and the role VoiceOver reads.
    let _: () = msg_send![tile, setToolTip: ns_string(guide::DRAG_HINT)];
    tile
}

unsafe fn app_icon(bundle: Option<&std::path::Path>) -> *mut AnyObject {
    let Some(path) = bundle.and_then(|p| p.to_str()) else {
        return std::ptr::null_mut();
    };
    let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
    let icon: *mut AnyObject = msg_send![workspace, iconForFile: ns_string(path)];
    if !icon.is_null() {
        let _: () = msg_send![icon, setSize: NSSize { width: ICON, height: ICON }];
    }
    icon
}

/// `CuaDriverDragTile`: an NSView that starts a file drag of the app
/// bundle on mouse down. It takes every click inside it (including on its
/// icon and labels) and accepts the first click even while System Settings
/// is the active app.
fn tile_class() -> &'static objc2::runtime::AnyClass {
    use objc2::declare::ClassBuilder;
    use std::sync::OnceLock;

    static CLASS: OnceLock<&'static objc2::runtime::AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut builder = ClassBuilder::new("CuaDriverDragTile", class!(NSView))
            .expect("CuaDriverDragTile already registered");
        if let Some(protocol) = objc2::runtime::AnyProtocol::get("NSDraggingSource") {
            builder.add_protocol(protocol);
        }
        unsafe {
            builder.add_method(
                objc2::sel!(mouseDown:),
                tile_mouse_down as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(hitTest:),
                tile_hit_test as extern "C" fn(_, _, _) -> _,
            );
            builder.add_method(
                objc2::sel!(acceptsFirstMouse:),
                tile_accepts_first_mouse as extern "C" fn(_, _, _) -> _,
            );
            builder.add_method(
                objc2::sel!(mouseDownCanMoveWindow),
                tile_cannot_move_window as extern "C" fn(_, _) -> _,
            );
            builder.add_method(
                objc2::sel!(draggingSession:sourceOperationMaskForDraggingContext:),
                tile_drag_operations as extern "C" fn(_, _, _, _) -> _,
            );
        }
        builder.register()
    })
}

extern "C" fn tile_hit_test(this: *mut AnyObject, _cmd: Sel, point: NSPoint) -> *mut AnyObject {
    // `point` is in the superview's coordinates, like `frame`.
    let frame: NSRect = unsafe { msg_send![this, frame] };
    let inside = point.x >= frame.origin.x
        && point.x < frame.origin.x + frame.size.width
        && point.y >= frame.origin.y
        && point.y < frame.origin.y + frame.size.height;
    if inside {
        this
    } else {
        std::ptr::null_mut()
    }
}

extern "C" fn tile_accepts_first_mouse(
    _this: *mut AnyObject,
    _cmd: Sel,
    _event: *mut AnyObject,
) -> Bool {
    Bool::YES
}

extern "C" fn tile_cannot_move_window(_this: *mut AnyObject, _cmd: Sel) -> Bool {
    Bool::NO
}

extern "C" fn tile_drag_operations(
    _this: *mut AnyObject,
    _cmd: Sel,
    _session: *mut AnyObject,
    _context: isize,
) -> usize {
    // NSDragOperationCopy | Link | Generic.
    1 | 2 | 4
}

extern "C" fn tile_mouse_down(this: *mut AnyObject, _cmd: Sel, event: *mut AnyObject) {
    let Some(bundle) = TILE_BUNDLE.with(|cell| cell.borrow().clone()) else {
        return;
    };
    let Some(path) = bundle.to_str() else {
        return;
    };
    unsafe {
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: ns_string(path)];
        if url.is_null() {
            return;
        }
        let item: *mut AnyObject = msg_send![class!(NSDraggingItem), alloc];
        let item: *mut AnyObject = msg_send![item, initWithPasteboardWriter: url];
        if item.is_null() {
            return;
        }
        let bounds: NSRect = msg_send![this, bounds];
        let drag_frame = NSRect {
            origin: NSPoint {
                x: 12.0,
                y: (bounds.size.height - ICON) / 2.0,
            },
            size: NSSize {
                width: ICON,
                height: ICON,
            },
        };
        let icon = app_icon(Some(&bundle));
        let _: () = msg_send![item, setDraggingFrame: drag_frame contents: icon];
        let items: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: item];
        let _: *mut AnyObject = msg_send![this,
            beginDraggingSessionWithItems: items
            event: event
            source: this
        ];
    }
}

// ── Small AppKit helpers ─────────────────────────────────────────────────

/// A `CGColorRef`, typed so the Objective-C runtime check sees
/// `^{CGColor=}` rather than `void *`.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct CGColorRef(*mut c_void);

unsafe impl objc2::Encode for CGColorRef {
    const ENCODING: objc2::Encoding =
        objc2::Encoding::Pointer(&objc2::Encoding::Struct("CGColor", &[]));
}

/// `&str` → autoreleased `NSString`.
unsafe fn ns_string(text: &str) -> *mut AnyObject {
    let owned = std::ffi::CString::new(text).unwrap_or_default();
    msg_send![class!(NSString), stringWithUTF8String: owned.as_ptr()]
}

unsafe fn symbol(name: &str) -> *mut AnyObject {
    msg_send![class!(NSImage),
        imageWithSystemSymbolName: ns_string(name)
        accessibilityDescription: std::ptr::null::<AnyObject>()
    ]
}

unsafe fn new_view(frame: NSRect) -> *mut AnyObject {
    let view: *mut AnyObject = msg_send![class!(NSView), alloc];
    msg_send![view, initWithFrame: frame]
}

unsafe fn label(text: &str, size: f64, bold: bool, frame: NSRect) -> *mut AnyObject {
    let label: *mut AnyObject = msg_send![class!(NSTextField), alloc];
    let label: *mut AnyObject = msg_send![label, initWithFrame: frame];
    let _: () = msg_send![label, setStringValue: ns_string(text)];
    let _: () = msg_send![label, setEditable: false];
    let _: () = msg_send![label, setBordered: false];
    let _: () = msg_send![label, setDrawsBackground: false];
    let _: () = msg_send![label, setSelectable: false];
    let _: () = msg_send![label, setUsesSingleLineMode: false];
    let cell: *mut AnyObject = msg_send![label, cell];
    if !cell.is_null() {
        // NSLineBreakByWordWrapping = 0.
        let _: () = msg_send![cell, setLineBreakMode: 0u64];
        let _: () = msg_send![cell, setWraps: true];
    }
    let font: *mut AnyObject = if bold {
        msg_send![class!(NSFont), boldSystemFontOfSize: size]
    } else {
        msg_send![class!(NSFont), systemFontOfSize: size]
    };
    let _: () = msg_send![label, setFont: font];
    label
}

unsafe fn set_secondary(label: *mut AnyObject) {
    let color: *mut AnyObject = msg_send![class!(NSColor), secondaryLabelColor];
    let _: () = msg_send![label, setTextColor: color];
}

unsafe fn center_text(label: *mut AnyObject) {
    // NSTextAlignmentCenter = 1.
    let _: () = msg_send![label, setAlignment: 1i64];
}

unsafe fn button(
    title: &str,
    frame: NSRect,
    action: Sel,
    target: *mut AnyObject,
) -> *mut AnyObject {
    let button: *mut AnyObject = msg_send![class!(NSButton), alloc];
    let button: *mut AnyObject = msg_send![button, initWithFrame: frame];
    let _: () = msg_send![button, setTitle: ns_string(title)];
    let _: () = msg_send![button, setBezelStyle: 1u64]; // rounded push button
    let _: () = msg_send![button, setTarget: target];
    let _: () = msg_send![button, setAction: action];
    button
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
    fn env_off_recognises_documented_sentinels() {
        let _guard = env_lock();
        for v in &["0", "false", "FALSE", "no", "NO", "off", "Off"] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_PANEL", v);
            assert!(
                env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"),
                "value {v:?} must turn the guide window off",
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
    }

    #[test]
    fn guide_window_is_on_unless_turned_off() {
        let _guard = env_lock();
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
        assert!(!env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"));
        for v in &["1", "true", "yes", "on", "garbage", ""] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_PANEL", v);
            assert!(
                !env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"),
                "value {v:?} must keep the guide window on",
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
    }

    #[test]
    fn layout_fits_the_window() {
        // Every row from the top stays inside the window.
        for (y, h) in [
            (20.0, 16.0),
            (40.0, 26.0),
            (72.0, 36.0),
            (114.0, 18.0),
            (142.0, 18.0),
            (172.0, 32.0),
            (228.0, 18.0),
            (250.0, TILE_H),
            (320.0, 32.0),
        ] {
            let r = rect(PAD, y, CONTENT_W, h);
            assert!(r.origin.y >= 48.0, "row at {y} overlaps the bottom buttons");
            assert!(r.origin.y + r.size.height <= WIN_H);
        }
    }
}
