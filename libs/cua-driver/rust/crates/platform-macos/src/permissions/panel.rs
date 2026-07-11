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
//! ┌─ Cua Driver Permissions ────────────────────┐
//! │  Enable Cua Driver Computer Use             │
//! │  Two macOS permissions are required.        │
//! │                                             │
//! │  ○ Accessibility                    [Allow] │
//! │      Read interface text and use controls.  │
//! │                                             │
//! │  ○ Screen Recording                 [Allow] │
//! │      See app windows to place actions.       │
//! │                                             │
//! │  Connected clients can use these grants.     │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! A 1 Hz `NSTimer` reads [`current_status`] as an opportunistic live
//! update. Permission-dialog and System Settings focus transitions are the
//! authoritative progress signal: when the user returns, the panel asks the
//! gate to re-exec and verify the result in a fresh process. This avoids
//! waiting forever on macOS versions that cache a false TCC probe.
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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

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
    /// The user returned from a macOS permission decision or System Settings.
    /// The current process may still cache a false probe, so the caller must
    /// re-exec and verify without claiming that the grant succeeded.
    RefreshRequired,
}

const OUTCOME_NONE: u8 = 0;
const OUTCOME_DISMISSED: u8 = 1;
const OUTCOME_ALL_GRANTED: u8 = 2;
const OUTCOME_REFRESH_REQUIRED: u8 = 3;
const OUTCOME_UNAVAILABLE: u8 = 4;

static PANEL_VISIBLE: AtomicBool = AtomicBool::new(false);
static PANEL_COMPLETION: AtomicU64 = AtomicU64::new(0);
static PANEL_LAST_OUTCOME: AtomicU8 = AtomicU8::new(OUTCOME_NONE);

struct PanelLifecycleGuard {
    finished: bool,
}

impl PanelLifecycleGuard {
    fn begin() -> Self {
        PANEL_LAST_OUTCOME.store(OUTCOME_NONE, Ordering::Release);
        PANEL_VISIBLE.store(true, Ordering::Release);
        Self { finished: false }
    }

    fn finish(mut self, outcome: PanelOutcome) {
        record_panel_completion(outcome);
        self.finished = true;
    }
}

impl Drop for PanelLifecycleGuard {
    fn drop(&mut self) {
        if !self.finished {
            record_panel_completion(PanelOutcome::Unavailable);
        }
    }
}

fn record_panel_completion(outcome: PanelOutcome) {
    let encoded = match outcome {
        PanelOutcome::Unavailable => OUTCOME_UNAVAILABLE,
        PanelOutcome::Dismissed => OUTCOME_DISMISSED,
        PanelOutcome::AllGranted => OUTCOME_ALL_GRANTED,
        PanelOutcome::RefreshRequired => OUTCOME_REFRESH_REQUIRED,
    };
    PANEL_LAST_OUTCOME.store(encoded, Ordering::Release);
    PANEL_VISIBLE.store(false, Ordering::Release);
    PANEL_COMPLETION.fetch_add(1, Ordering::AcqRel);
}

/// Process-local lifecycle snapshot for the daemon's private permission
/// control channel. This is intentionally separate from the MCP registry: a
/// CLI can tell whether the already-running branded panel is still active (or
/// was dismissed) without adding a public computer-use tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelLifecycle {
    pub visible: bool,
    pub completion: u64,
    pub last_outcome: Option<PanelOutcome>,
}

pub fn lifecycle() -> PanelLifecycle {
    let last_outcome = match PANEL_LAST_OUTCOME.load(Ordering::Acquire) {
        OUTCOME_DISMISSED => Some(PanelOutcome::Dismissed),
        OUTCOME_ALL_GRANTED => Some(PanelOutcome::AllGranted),
        OUTCOME_REFRESH_REQUIRED => Some(PanelOutcome::RefreshRequired),
        OUTCOME_UNAVAILABLE => Some(PanelOutcome::Unavailable),
        _ => None,
    };
    PanelLifecycle {
        visible: PANEL_VISIBLE.load(Ordering::Acquire),
        completion: PANEL_COMPLETION.load(Ordering::Acquire),
        last_outcome,
    }
}

pub fn outcome_name(outcome: PanelOutcome) -> &'static str {
    match outcome {
        PanelOutcome::Unavailable => "unavailable",
        PanelOutcome::Dismissed => "dismissed",
        PanelOutcome::AllGranted => "all-granted",
        PanelOutcome::RefreshRequired => "refresh-required",
    }
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
    let _mtm = MainThreadMarker::new().expect("show_modal must be called from the main thread");
    let lifecycle = PanelLifecycleGuard::begin();
    let outcome = unsafe { show_modal_unsafe(&opts) };
    lifecycle.finish(outcome);
    outcome
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
        let run_loop: *mut AnyObject = msg_send![class!(NSRunLoop), currentRunLoop];
        if run_loop.is_null() {
            std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
            return;
        }
        // `[NSDate dateWithTimeIntervalSinceNow:seconds]` builds the
        // deadline. `[NSRunLoop runMode:beforeDate:]` blocks up to
        // that deadline while dispatching pending input sources +
        // timers — including TCC notification taps.
        let date: *mut AnyObject = msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: seconds];
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

const WIN_W: f64 = 640.0;
const WIN_H: f64 = 560.0;
const PAD: f64 = 36.0;
const ROW_H: f64 = 112.0;
const FOOTER_H: f64 = 32.0;

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
    // Keep native traffic-light controls while matching the focused,
    // title-free onboarding presentation used by Codex.
    let _: () = msg_send![window, setTitleVisibility: 1i64];
    let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
    let window_delegate = panel_target_instance();
    let _: () = msg_send![window, setDelegate: window_delegate];

    let content_view: *mut AnyObject = msg_send![window, contentView];

    // ---- Centered product identity, heading, and explainer ----
    let app_icon = build_application_icon(
        app,
        NSPoint {
            x: (WIN_W - 72.0) / 2.0,
            y: 456.0,
        },
        NSSize {
            width: 72.0,
            height: 72.0,
        },
    );
    let _: () = msg_send![content_view, addSubview: app_icon];

    let heading = build_label(
        &heading_for(opts.initial_status, &product_name),
        24.0,
        /*bold=*/ true,
        NSPoint { x: PAD, y: 408.0 },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: 34.0,
        },
    );
    let _: () = msg_send![heading, setAlignment: 1i64];
    let _: () = msg_send![content_view, addSubview: heading];

    let subheading = build_label(
        &subheading_for(opts.initial_status, &product_name),
        13.0,
        /*bold=*/ false,
        NSPoint {
            x: PAD + 28.0,
            y: 344.0,
        },
        NSSize {
            width: WIN_W - 2.0 * (PAD + 28.0),
            height: 50.0,
        },
    );
    let _: () = msg_send![subheading, setAlignment: 1i64];
    let _: () = msg_send![content_view, addSubview: subheading];
    // Subheading is secondary text — apply secondaryLabelColor.
    let secondary_color: *mut AnyObject = msg_send![class!(NSColor), secondaryLabelColor];
    let _: () = msg_send![subheading, setTextColor: secondary_color];

    // ---- Permission rows ----
    let request_flow = PermissionRequestFlow::from_environment();
    let ax_row = build_row(
        MissingPermission::Accessibility,
        opts.initial_status,
        &request_flow,
        NSPoint { x: PAD, y: 210.0 },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: ROW_H,
        },
    );
    let _: () = msg_send![content_view, addSubview: ax_row.container];

    let sr_row = build_row(
        MissingPermission::ScreenRecording,
        opts.initial_status,
        &request_flow,
        NSPoint { x: PAD, y: 82.0 },
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
        NSPoint { x: PAD, y: 24.0 },
        NSSize {
            width: WIN_W - 2.0 * PAD,
            height: FOOTER_H,
        },
    );
    let _: () = msg_send![footer, setAlignment: 1i64];
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
            request_flow,
            product_name,
        });
    });

    // ---- Show window + start poll timer + run modal ----
    // Observe both app activation and window-key transitions. TCC dialogs can
    // use either shape depending on the permission and macOS release. A
    // departure followed by a return is our signal to verify in a fresh
    // process; we never infer that the permission was granted.
    let notification_center: *mut AnyObject =
        msg_send![class!(NSNotificationCenter), defaultCenter];
    let app_resigned = ns_string("NSApplicationDidResignActiveNotification");
    let app_activated = ns_string("NSApplicationDidBecomeActiveNotification");
    let _: () = msg_send![notification_center,
        addObserver: window_delegate
        selector: objc2::sel!(applicationDidResignActive:)
        name: app_resigned
        object: app
    ];
    let _: () = msg_send![notification_center,
        addObserver: window_delegate
        selector: objc2::sel!(applicationDidBecomeActive:)
        name: app_activated
        object: app
    ];
    let _: () = msg_send![window, center];
    let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
    let _: () = msg_send![app, activateIgnoringOtherApps: true];

    let timer = schedule_poll_timer();
    let _: i64 = msg_send![app, runModalForWindow: window];
    // Stop timer first so any in-flight tick can't race with teardown.
    let _: () = msg_send![timer, invalidate];
    let _: () = msg_send![notification_center, removeObserver: window_delegate];

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

/// Persisted across the one-shot verification re-exec so a denied or
/// dismissed prompt comes back as “Complete in System Settings” instead of
/// repeatedly raising the same TCC dialog.
const REQUESTED_PERMISSIONS_ENV: &str = "CUA_DRIVER_RS_PERMISSION_REQUESTED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionRequestPhase {
    NotRequested,
    PendingSystemDecision,
    NotGranted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PermissionRequestFlow {
    accessibility: PermissionRequestPhase,
    screen_recording: PermissionRequestPhase,
    active_prompt: Option<MissingPermission>,
    settings_permission: Option<MissingPermission>,
    system_ui_departed: bool,
}

impl PermissionRequestFlow {
    fn new(accessibility_requested: bool, screen_recording_requested: bool) -> Self {
        Self {
            accessibility: if accessibility_requested {
                PermissionRequestPhase::NotGranted
            } else {
                PermissionRequestPhase::NotRequested
            },
            screen_recording: if screen_recording_requested {
                PermissionRequestPhase::NotGranted
            } else {
                PermissionRequestPhase::NotRequested
            },
            active_prompt: None,
            settings_permission: None,
            system_ui_departed: false,
        }
    }

    fn from_environment() -> Self {
        let requested = std::env::var(REQUESTED_PERMISSIONS_ENV).unwrap_or_default();
        let has = |needle: &str| requested.split(',').any(|value| value == needle);
        Self::new(has("accessibility"), has("screen_recording"))
    }

    fn phase(self, permission: MissingPermission) -> PermissionRequestPhase {
        match permission {
            MissingPermission::Accessibility => self.accessibility,
            MissingPermission::ScreenRecording => self.screen_recording,
        }
    }

    fn set_phase(&mut self, permission: MissingPermission, phase: PermissionRequestPhase) {
        match permission {
            MissingPermission::Accessibility => self.accessibility = phase,
            MissingPermission::ScreenRecording => self.screen_recording = phase,
        }
    }

    /// Begin one TCC request. Returns false if another system prompt is
    /// already active, preserving the one-prompt-at-a-time invariant.
    fn begin_request(&mut self, permission: MissingPermission) -> bool {
        if self.active_prompt.is_some() {
            return false;
        }
        self.set_phase(permission, PermissionRequestPhase::PendingSystemDecision);
        self.active_prompt = Some(permission);
        self.settings_permission = None;
        self.system_ui_departed = false;
        true
    }

    /// Move a requested permission into its recoverable Settings state. This
    /// also releases the other row because the TCC prompt is no longer active.
    fn begin_settings(&mut self, permission: MissingPermission) {
        self.set_phase(permission, PermissionRequestPhase::NotGranted);
        self.active_prompt = None;
        self.settings_permission = Some(permission);
        self.system_ui_departed = false;
    }

    fn settings_open_failed(&mut self, permission: MissingPermission) {
        if self.settings_permission == Some(permission) {
            self.settings_permission = None;
            self.system_ui_departed = false;
        }
    }

    fn note_system_ui_departed(&mut self) {
        if self.active_prompt.is_some() || self.settings_permission.is_some() {
            self.system_ui_departed = true;
        }
    }

    /// A return signal never means “granted”. It only means the user's
    /// decision is complete enough to verify in a fresh process.
    fn note_system_ui_returned(&mut self) -> bool {
        if !self.system_ui_departed {
            return false;
        }
        let mut refresh_required = false;
        if let Some(permission) = self.active_prompt.take() {
            self.set_phase(permission, PermissionRequestPhase::NotGranted);
            refresh_required = true;
        }
        if self.settings_permission.take().is_some() {
            refresh_required = true;
        }
        self.system_ui_departed = false;
        refresh_required
    }

    /// Accept a live green probe as progress, while treating a live red probe
    /// as inconclusive because macOS may cache it for the process lifetime.
    fn reconcile_grants(&mut self, status: PermissionsStatus) -> bool {
        let mut changed = false;
        for permission in [
            MissingPermission::Accessibility,
            MissingPermission::ScreenRecording,
        ] {
            if permission_is_granted(status, permission) {
                if self.active_prompt == Some(permission) {
                    self.active_prompt = None;
                    changed = true;
                }
                if self.settings_permission == Some(permission) {
                    self.settings_permission = None;
                    changed = true;
                }
            }
        }
        if self.active_prompt.is_none() && self.settings_permission.is_none() {
            self.system_ui_departed = false;
        }
        changed
    }
}

fn remember_requested(permission: MissingPermission) {
    let mut flow = PermissionRequestFlow::from_environment();
    flow.set_phase(permission, PermissionRequestPhase::NotGranted);
    let mut requested = Vec::new();
    if flow.accessibility != PermissionRequestPhase::NotRequested {
        requested.push("accessibility");
    }
    if flow.screen_recording != PermissionRequestPhase::NotRequested {
        requested.push("screen_recording");
    }
    std::env::set_var(REQUESTED_PERMISSIONS_ENV, requested.join(","));
}

pub(crate) fn clear_request_history() {
    std::env::remove_var(REQUESTED_PERMISSIONS_ENV);
}

struct PanelHandles {
    heading: usize,
    subheading: usize,
    footer: usize,
    ax_row: RowHandles,
    sr_row: RowHandles,
    last_status: PermissionsStatus,
    request_flow: PermissionRequestFlow,
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
        let Some(handles) = guard.as_mut() else {
            return;
        };
        let flow_changed = handles.request_flow.reconcile_grants(status);
        if status == handles.last_status && !flow_changed {
            // No change — skip the redraw. Avoids needlessly thrashing
            // the layout engine every second on a steady-state panel.
            return;
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

fn permission_symbol(permission: MissingPermission, granted: bool) -> &'static str {
    if granted {
        return "checkmark.circle.fill";
    }
    match permission {
        MissingPermission::Accessibility => "accessibility",
        MissingPermission::ScreenRecording => "camera.viewfinder",
    }
}

fn row_action(
    status: PermissionsStatus,
    flow: &PermissionRequestFlow,
    permission: MissingPermission,
) -> RowAction {
    if permission_is_granted(status, permission) {
        return RowAction::Allowed;
    }
    match flow.active_prompt {
        Some(active) if active == permission => RowAction::CompleteInSystemSettings,
        Some(_) => RowAction::WaitingForOtherPermission,
        None => match flow.phase(permission) {
            PermissionRequestPhase::NotRequested => RowAction::Allow,
            PermissionRequestPhase::PendingSystemDecision | PermissionRequestPhase::NotGranted => {
                RowAction::CompleteInSystemSettings
            }
        },
    }
}

fn primary_allow_permission(
    status: PermissionsStatus,
    flow: &PermissionRequestFlow,
) -> Option<MissingPermission> {
    if flow.active_prompt.is_some() {
        return None;
    }
    [
        MissingPermission::Accessibility,
        MissingPermission::ScreenRecording,
    ]
    .into_iter()
    .find(|permission| {
        !permission_is_granted(status, *permission)
            && flow.phase(*permission) == PermissionRequestPhase::NotRequested
    })
}

fn is_primary_allow(
    status: PermissionsStatus,
    flow: &PermissionRequestFlow,
    permission: MissingPermission,
    action: RowAction,
) -> bool {
    action == RowAction::Allow && primary_allow_permission(status, flow) == Some(permission)
}

unsafe fn render_panel(handles: &PanelHandles, status: PermissionsStatus) {
    update_row(
        handles.ax_row,
        MissingPermission::Accessibility,
        status,
        &handles.request_flow,
    );
    update_row(
        handles.sr_row,
        MissingPermission::ScreenRecording,
        status,
        &handles.request_flow,
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
    flow: &PermissionRequestFlow,
) {
    let granted = permission_is_granted(status, permission);
    // Swap icon symbol and tint.
    let symbol = permission_symbol(permission, granted);
    let img = ns_image_for_symbol(symbol);
    let _: () = msg_send![row.icon, setImage: img];
    let tint: *mut AnyObject = if granted {
        msg_send![class!(NSColor), systemGreenColor]
    } else {
        msg_send![class!(NSColor), systemBlueColor]
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

    let action = row_action(status, flow, permission);
    let title = ns_string(action.title());
    let _: () = msg_send![row.button, setTitle: title];
    style_button(
        row.button,
        action,
        is_primary_allow(status, flow, permission, action),
    );
}

// ── User-facing copy ────────────────────────────────────────────────────

fn heading_for(status: PermissionsStatus, product_name: &str) -> String {
    let feature_name = computer_use_feature_name(product_name);
    match status.grant_state() {
        PermissionGrantState::BothGranted => format!("{feature_name} is ready"),
        PermissionGrantState::AccessibilityGranted
        | PermissionGrantState::ScreenRecordingGranted => {
            format!("Finish enabling {feature_name}")
        }
        PermissionGrantState::NoneGranted => format!("Enable {feature_name}"),
    }
}

fn computer_use_feature_name(product_name: &str) -> String {
    if product_name.to_ascii_lowercase().contains("computer use") {
        product_name.to_owned()
    } else {
        format!("{product_name} Computer Use")
    }
}

fn subheading_for(status: PermissionsStatus, product_name: &str) -> String {
    match status.grant_state() {
        PermissionGrantState::BothGranted => {
            "Both macOS permissions are active. Computer control can start now.".into()
        }
        PermissionGrantState::AccessibilityGranted => {
            "Accessibility is allowed. Allow Screen Recording to finish setup.".into()
        }
        PermissionGrantState::ScreenRecordingGranted => {
            "Screen Recording is allowed. Allow Accessibility to finish setup.".into()
        }
        PermissionGrantState::NoneGranted => format!(
            "{product_name} needs these permissions to use apps on your Mac.\n\
             Continue only if you trust the app or client that started {product_name}."
        ),
    }
}

fn footer_for(status: PermissionsStatus, product_name: &str) -> String {
    if status.all_granted() {
        return format!("All set. {product_name} can now use apps on your Mac.");
    }
    format!(
        "macOS grants access to {product_name}, not your terminal. Connected clients can use these grants; revoke them anytime in Privacy & Security."
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
    "Cua Driver".into()
}

unsafe fn build_application_icon(
    app: *mut AnyObject,
    origin: NSPoint,
    size: NSSize,
) -> *mut AnyObject {
    let view: *mut AnyObject = msg_send![class!(NSImageView), alloc];
    let view: *mut AnyObject = msg_send![view, initWithFrame: NSRect { origin, size }];
    let mut image: *mut AnyObject = msg_send![app, applicationIconImage];
    if image.is_null() {
        image = ns_image_for_symbol("cursorarrow.rays");
    }
    let _: () = msg_send![view, setImage: image];
    // NSImageScaleProportionallyUpOrDown.
    let _: () = msg_send![view, setImageScaling: 3i64];
    view
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

unsafe fn style_button(button: *mut AnyObject, action: RowAction, primary: bool) {
    let _: () = msg_send![button, setEnabled: action.enabled()];
    if primary {
        let blue: *mut AnyObject = msg_send![class!(NSColor), systemBlueColor];
        let white: *mut AnyObject = msg_send![class!(NSColor), whiteColor];
        let _: () = msg_send![button, setBezelColor: blue];
        let _: () = msg_send![button, setContentTintColor: white];
        let _: () = msg_send![button, setKeyEquivalent: ns_string("\r")];
    } else {
        let tint: *mut AnyObject = if action.enabled() {
            msg_send![class!(NSColor), labelColor]
        } else {
            msg_send![class!(NSColor), secondaryLabelColor]
        };
        let _: () = msg_send![button, setBezelColor: std::ptr::null::<AnyObject>()];
        let _: () = msg_send![button, setContentTintColor: tint];
        let _: () = msg_send![button, setKeyEquivalent: ns_string("")];
    }
}

/// Build one permission row: status icon, title, explanation, and action.
unsafe fn build_row(
    permission: MissingPermission,
    status: PermissionsStatus,
    flow: &PermissionRequestFlow,
    origin: NSPoint,
    size: NSSize,
) -> RowHandles {
    let container: *mut AnyObject = msg_send![class!(NSView), alloc];
    let container: *mut AnyObject = msg_send![container, initWithFrame: NSRect {
        origin,
        size,
    }];
    let _: () = msg_send![container, setWantsLayer: true];
    let layer: *mut AnyObject = msg_send![container, layer];
    if !layer.is_null() {
        let fill: *mut AnyObject = msg_send![class!(NSColor), controlBackgroundColor];
        let cg_color: *mut c_void = msg_send![fill, CGColor];
        let _: () = msg_send![layer, setBackgroundColor: cg_color];
        let _: () = msg_send![layer, setCornerRadius: 18.0_f64];
        let _: () = msg_send![layer, setShadowOpacity: 0.14_f32];
        let _: () = msg_send![layer, setShadowRadius: 12.0_f64];
        let _: () = msg_send![layer, setShadowOffset: NSSize {
            width: 0.0,
            height: -3.0,
        }];
    }

    // Icon
    let icon_frame = NSRect {
        origin: NSPoint { x: 22.0, y: 32.0 },
        size: NSSize {
            width: 48.0,
            height: 48.0,
        },
    };
    let icon: *mut AnyObject = msg_send![class!(NSImageView), alloc];
    let icon: *mut AnyObject = msg_send![icon, initWithFrame: icon_frame];
    let granted = permission_is_granted(status, permission);
    let symbol = permission_symbol(permission, granted);
    let img = ns_image_for_symbol(symbol);
    let _: () = msg_send![icon, setImage: img];
    let _: () = msg_send![icon, setImageScaling: 3i64];
    let tint: *mut AnyObject = if granted {
        msg_send![class!(NSColor), systemGreenColor]
    } else {
        msg_send![class!(NSColor), systemBlueColor]
    };
    let _: () = msg_send![icon, setContentTintColor: tint];
    let _: () = msg_send![container, addSubview: icon];

    // Title
    let title = build_label(
        permission.label(),
        15.0,
        /*bold=*/ true,
        NSPoint { x: 88.0, y: 61.0 },
        NSSize {
            width: size.width - 330.0,
            height: 24.0,
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
        12.0,
        /*bold=*/ false,
        NSPoint { x: 88.0, y: 29.0 },
        NSSize {
            width: size.width - 330.0,
            height: 28.0,
        },
    );
    let secondary: *mut AnyObject = msg_send![class!(NSColor), secondaryLabelColor];
    let _: () = msg_send![subtitle, setTextColor: secondary];
    let _: () = msg_send![container, addSubview: subtitle];

    let action = row_action(status, flow, permission);
    let button_action = match permission {
        MissingPermission::Accessibility => ButtonAction::Accessibility,
        MissingPermission::ScreenRecording => ButtonAction::ScreenRecording,
    };
    let button = build_button(
        action.title(),
        NSPoint {
            x: size.width - 224.0,
            y: 38.0,
        },
        NSSize {
            width: 202.0,
            height: 36.0,
        },
        button_action,
    );
    style_button(
        button,
        action,
        is_primary_allow(status, flow, permission, action),
    );
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
        MissingPermission::Accessibility => "Access app interfaces to read and use controls.",
        MissingPermission::ScreenRecording => "Capture app windows to know where to click.",
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
            builder.add_method(
                objc2::sel!(windowDidResignKey:),
                on_system_ui_departed as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(windowDidBecomeKey:),
                on_system_ui_returned as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(applicationDidResignActive:),
                on_system_ui_departed as extern "C" fn(_, _, _),
            );
            builder.add_method(
                objc2::sel!(applicationDidBecomeActive:),
                on_system_ui_returned as extern "C" fn(_, _, _),
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
                .map(|handles| row_action(handles.last_status, &handles.request_flow, permission))
        });

        match action {
            Some(RowAction::Allow) => {
                // Paint the in-progress state before invoking the TCC API. The
                // system prompt may immediately move focus to another app.
                HANDLES.with(|cell| unsafe {
                    let mut guard = cell.borrow_mut();
                    let Some(handles) = guard.as_mut() else {
                        return;
                    };
                    if !handles.request_flow.begin_request(permission) {
                        return;
                    }
                    let status = handles.last_status;
                    render_panel(handles, status);
                });
                remember_requested(permission);

                let granted_signal = match permission {
                    MissingPermission::Accessibility => request_accessibility(),
                    MissingPermission::ScreenRecording => request_screen_recording(),
                };
                if granted_signal {
                    // A true request result is authoritative, but still do not
                    // paint the row green from that return value. Re-exec and
                    // let a fresh preflight prove the grant.
                    request_refresh();
                } else {
                    unsafe { poll_tick_inner() };
                }
            }
            Some(RowAction::CompleteInSystemSettings) => {
                HANDLES.with(|cell| unsafe {
                    let mut guard = cell.borrow_mut();
                    let Some(handles) = guard.as_mut() else {
                        return;
                    };
                    handles.request_flow.begin_settings(permission);
                    let status = handles.last_status;
                    render_panel(handles, status);
                });
                remember_requested(permission);
                if let Err(error) = crate::permissions::gate::open_system_settings_for(permission) {
                    eprintln!(
                        "[cua-driver] could not open System Settings for {}: {error}",
                        permission.label()
                    );
                    HANDLES.with(|cell| unsafe {
                        let mut guard = cell.borrow_mut();
                        let Some(handles) = guard.as_mut() else {
                            return;
                        };
                        handles.request_flow.settings_open_failed(permission);
                        let status = handles.last_status;
                        render_panel(handles, status);
                    });
                }
            }
            Some(RowAction::WaitingForOtherPermission | RowAction::Allowed) | None => {}
        }
    }));
    if let Err(error) = result {
        eprintln!("[cua-driver] permissions panel action panicked: {error:?}");
    }
}

extern "C" fn on_system_ui_departed(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _notification: *mut AnyObject,
) {
    HANDLES.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(handles) = guard.as_mut() else {
            return;
        };
        handles.request_flow.note_system_ui_departed();
    });
}

extern "C" fn on_system_ui_returned(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _notification: *mut AnyObject,
) {
    let refresh_required = HANDLES.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(handles) = guard.as_mut() else {
            return false;
        };
        handles.request_flow.note_system_ui_returned()
    });
    if refresh_required {
        request_refresh();
    }
}

fn request_refresh() {
    OUTCOME.with(|cell| {
        let mut outcome = cell.borrow_mut();
        if outcome.is_none() {
            *outcome = Some(PanelOutcome::RefreshRequired);
        }
    });
    stop_modal();
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
    fn requested_history_survives_verification_reexec() {
        let _guard = env_lock();
        clear_request_history();
        remember_requested(MissingPermission::Accessibility);
        let flow = PermissionRequestFlow::from_environment();
        assert_eq!(
            flow.phase(MissingPermission::Accessibility),
            PermissionRequestPhase::NotGranted
        );
        assert_eq!(
            flow.phase(MissingPermission::ScreenRecording),
            PermissionRequestPhase::NotRequested
        );
        clear_request_history();
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
        assert_eq!(
            heading_for(st(false, false), "Cua Driver"),
            "Enable Cua Driver Computer Use"
        );
        assert_eq!(
            heading_for(st(true, false), "Cua Driver"),
            "Finish enabling Cua Driver Computer Use"
        );
        assert_eq!(
            heading_for(st(false, true), "Cua Driver"),
            "Finish enabling Cua Driver Computer Use"
        );
        assert_eq!(
            heading_for(st(true, true), "Cua Driver"),
            "Cua Driver Computer Use is ready"
        );
        assert_eq!(
            heading_for(st(false, false), "Codex Computer Use"),
            "Enable Codex Computer Use"
        );
        assert!(subheading_for(st(true, false), "Cua Driver").contains("Screen Recording"));
        assert!(subheading_for(st(false, true), "Cua Driver").contains("Accessibility"));
        assert_eq!(
            subheading_for(st(false, false), "Cua Driver"),
            "Cua Driver needs these permissions to use apps on your Mac.\n\
             Continue only if you trust the app or client that started Cua Driver."
        );
        assert!(footer_for(st(false, false), "Cua Driver").contains("not your terminal"));
        assert!(footer_for(st(false, false), "Cua Driver").contains("Connected clients"));
    }

    #[test]
    fn only_one_permission_request_is_active() {
        let neither = PermissionsStatus {
            accessibility: false,
            screen_recording: false,
        };
        let mut flow = PermissionRequestFlow::new(false, false);
        assert_eq!(
            row_action(neither, &flow, MissingPermission::Accessibility),
            RowAction::Allow
        );
        assert_eq!(
            row_action(neither, &flow, MissingPermission::ScreenRecording),
            RowAction::Allow
        );

        assert!(flow.begin_request(MissingPermission::Accessibility));
        assert!(!flow.begin_request(MissingPermission::ScreenRecording));
        assert_eq!(
            row_action(neither, &flow, MissingPermission::Accessibility,),
            RowAction::CompleteInSystemSettings
        );
        assert_eq!(
            row_action(neither, &flow, MissingPermission::ScreenRecording,),
            RowAction::WaitingForOtherPermission
        );
    }

    #[test]
    fn only_one_allow_button_is_primary() {
        let neither = PermissionsStatus {
            accessibility: false,
            screen_recording: false,
        };
        let flow = PermissionRequestFlow::new(false, false);
        assert!(is_primary_allow(
            neither,
            &flow,
            MissingPermission::Accessibility,
            RowAction::Allow,
        ));
        assert!(!is_primary_allow(
            neither,
            &flow,
            MissingPermission::ScreenRecording,
            RowAction::Allow,
        ));

        let accessibility_allowed = PermissionsStatus {
            accessibility: true,
            screen_recording: false,
        };
        assert!(is_primary_allow(
            accessibility_allowed,
            &flow,
            MissingPermission::ScreenRecording,
            RowAction::Allow,
        ));
    }

    #[test]
    fn denied_or_dismissed_prompt_recovers_both_rows_after_return_signal() {
        let neither = PermissionsStatus {
            accessibility: false,
            screen_recording: false,
        };
        let mut flow = PermissionRequestFlow::new(false, false);
        assert!(flow.begin_request(MissingPermission::Accessibility));

        // A stray activation notification cannot complete a request. The
        // panel waits for an actual departure/return pair from system UI.
        assert!(!flow.note_system_ui_returned());
        flow.note_system_ui_departed();
        assert!(flow.note_system_ui_returned());

        // Cached false remains false: the return asks the gate to re-exec but
        // never paints a false green grant. The denied row gets Settings and
        // the untouched row is immediately usable.
        assert_eq!(
            flow.phase(MissingPermission::Accessibility),
            PermissionRequestPhase::NotGranted
        );
        assert_eq!(
            row_action(neither, &flow, MissingPermission::Accessibility),
            RowAction::CompleteInSystemSettings
        );
        assert_eq!(
            row_action(neither, &flow, MissingPermission::ScreenRecording),
            RowAction::Allow
        );
    }

    #[test]
    fn settings_recovery_releases_a_pending_prompt() {
        let neither = PermissionsStatus {
            accessibility: false,
            screen_recording: false,
        };
        let mut flow = PermissionRequestFlow::new(false, false);
        assert!(flow.begin_request(MissingPermission::Accessibility));
        flow.begin_settings(MissingPermission::Accessibility);

        assert_eq!(flow.active_prompt, None);
        assert_eq!(
            row_action(neither, &flow, MissingPermission::Accessibility),
            RowAction::CompleteInSystemSettings
        );
        assert_eq!(
            row_action(neither, &flow, MissingPermission::ScreenRecording),
            RowAction::Allow
        );
        flow.note_system_ui_departed();
        assert!(
            flow.note_system_ui_returned(),
            "returning from Settings must request a fresh-process verification"
        );
    }

    #[test]
    fn live_green_probe_advances_without_waiting_for_return() {
        let accessibility_only = PermissionsStatus {
            accessibility: true,
            screen_recording: false,
        };
        let mut flow = PermissionRequestFlow::new(false, false);
        assert!(flow.begin_request(MissingPermission::Accessibility));
        flow.note_system_ui_departed();
        assert!(flow.reconcile_grants(accessibility_only));
        assert_eq!(
            row_action(accessibility_only, &flow, MissingPermission::Accessibility,),
            RowAction::Allowed
        );
        assert_eq!(
            row_action(
                accessibility_only,
                &flow,
                MissingPermission::ScreenRecording,
            ),
            RowAction::Allow
        );
        assert!(!flow.note_system_ui_returned());
    }

    #[test]
    fn panel_lifecycle_guard_records_unwind_as_unavailable() {
        let _guard = env_lock();
        let before = lifecycle().completion;
        {
            let _lifecycle = PanelLifecycleGuard::begin();
            assert!(lifecycle().visible);
        }
        let after = lifecycle();
        assert!(!after.visible);
        assert_eq!(after.completion, before + 1);
        assert_eq!(after.last_outcome, Some(PanelOutcome::Unavailable));
    }
}
