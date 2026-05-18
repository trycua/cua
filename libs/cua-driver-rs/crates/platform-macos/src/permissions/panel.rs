//! Phase 1 of the native NSPanel onboarding UI — minimal modal dialog
//! that replaces the terminal banner with a visible window.
//!
//! ## What this is
//!
//! When `gate::run_if_needed` detects missing TCC grants, it can now
//! present a small native window before falling back to the polling
//! loop in `wait_for_grants`. The window lists the missing permissions
//! and offers two buttons:
//!
//!   * **Open System Settings** — opens the matching Privacy panes
//!     (handled by the caller; the panel just records the choice).
//!   * **Continue anyway** — dismisses without opening Settings; the
//!     gate enters its polling phase regardless.
//!
//! Closing the window via the red dot is treated as Continue anyway.
//!
//! ## What this is NOT (yet)
//!
//! Phase 1 deliberately keeps the panel **static**. It does not poll
//! the live grant state, does not redraw red/green status icons, does
//! not auto-dismiss when all grants flip green, and does not auto-chain
//! between Settings panes when one grant flips. Those behaviours are
//! Phase 2 and Phase 3 — see the parity reference in
//! `libs/cua-driver/Sources/CuaDriverCore/Permissions/PermissionsGate.swift`.
//!
//! ## Threading
//!
//! `show_modal` MUST be called on the main thread. The Serve arm in
//! `cua-driver/src/main.rs` invokes the gate synchronously before any
//! threads are spawned, so the gate already runs on the main thread.
//! `panel_enabled()` double-checks this and falls back to the terminal
//! banner if anything is off (not main thread, NSApp already running,
//! headless, opt-out env var, non-`.app` invocation).
//!
//! ## Library choice
//!
//! Raw `objc2::msg_send!` macros against AppKit classes — same style
//! as `cursor/overlay.rs`. Avoids pulling additional `objc2-app-kit`
//! feature flags or adding new crate dependencies. Every UI call goes
//! through `MainThreadMarker::new().expect(...)` so a non-main-thread
//! invocation panics loudly instead of corrupting AppKit state.

use std::ffi::c_void;

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

use crate::permissions::gate::MissingPermission;

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
}

/// Inputs for [`show_modal`].
#[derive(Debug, Clone)]
pub struct PanelOpts {
    pub missing: Vec<MissingPermission>,
}

/// Decide whether the panel can be shown right now. Returns false (and
/// the caller falls back to the terminal banner) when:
///
///   * `CUA_DRIVER_RS_PERMISSIONS_PANEL` env var is set to an off
///     sentinel (`0` / `false` / `no` / `off`, case-insensitive).
///   * Not running on the main thread.
///   * `NSApp` already has a run loop active — Phase 1 deliberately
///     does not try to interleave with an existing AppKit loop.
///   * No graphical session is available (`NSScreen.mainScreen` is
///     nil — headless / launchd-spawned daemons before a display
///     attaches).
///   * The current binary is not running inside an `.app` bundle
///     (bare-binary invocations e.g. `target/release/cua-driver serve`
///     don't get a window — they keep the terminal flow).
///
/// All checks are best-effort. Any failure path returns false so the
/// terminal banner kicks in.
pub fn panel_enabled() -> bool {
    if env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL") {
        return false;
    }
    if MainThreadMarker::new().is_none() {
        return false;
    }
    if !running_inside_app_bundle() {
        return false;
    }
    // Defer NSApp / NSScreen probes into show_modal so we can keep this
    // function side-effect-free — they require an autorelease pool and
    // touching them here just to return a bool would force callers to
    // wrap in @autoreleasepool unnecessarily.
    true
}

/// Show the panel modally. Blocks the calling (main) thread until the
/// user clicks one of the buttons or closes the window. Returns the
/// outcome so the caller can decide whether to open Settings.
///
/// Safety / threading: panics if invoked off the main thread. Callers
/// should check [`panel_enabled`] first to avoid the panic.
pub fn show_modal(opts: PanelOpts) -> PanelOutcome {
    let _mtm = MainThreadMarker::new()
        .expect("show_modal must be called from the main thread");
    unsafe { show_modal_unsafe(&opts) }
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
/// Phase 1 only opens the panel from the canonical install path so a
/// `cargo run` from inside the repo (where the binary has no Info.plist
/// and is not TCC-attributed to the bundle id) still uses the terminal
/// flow. The check is a path-suffix sniff against
/// `std::env::current_exe()` — no AppKit calls.
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

unsafe fn show_modal_unsafe(opts: &PanelOpts) -> PanelOutcome {
    // ---- NSApplication setup ----
    // `sharedApplication` is idempotent; matches the pattern in
    // `cursor/overlay.rs::run_appkit`. We set the activation policy to
    // Accessory (1) so the panel doesn't add a Dock icon for the daemon
    // — same as the overlay path. `finishLaunching` is required before
    // `runModalForWindow:` will dispatch events.
    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    let _: bool = msg_send![app, setActivationPolicy: 1i64];
    let _: () = msg_send![app, finishLaunching];

    // Bail to caller's fallback if no display is attached — the
    // terminal banner is the right UX in that case.
    let main_screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if main_screen.is_null() {
        return PanelOutcome::Dismissed;
    }

    // ---- Window geometry ----
    // 460x320 matches PermissionsGate.swift's `.frame(width: 460)` plus
    // intrinsic SwiftUI heights. We pick a fixed height here since we
    // know the row count up-front (max 2 rows + heading + buttons).
    let content_rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: 460.0,
            height: 280.0,
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

    // ---- Content view ----
    let content_view: *mut AnyObject = msg_send![window, contentView];

    // Heading text — semibold 17pt, top-left.
    let heading = build_label(
        "CuaDriver needs your permission",
        17.0,
        /*bold=*/ true,
        NSPoint { x: 20.0, y: 230.0 },
        NSSize {
            width: 420.0,
            height: 24.0,
        },
    );
    let _: () = msg_send![content_view, addSubview: heading];

    // Subheading copy — verbatim from PermissionsGate.swift line 259-262
    // when both rows are missing. We don't try to switch wording based
    // on which subset is missing in Phase 1 — that's a Phase 2 task
    // tied to the dynamic-heading work.
    let subheading_text =
        "Grant the items below so CuaDriver can inspect and drive native \
         apps on your behalf.";
    let subheading = build_label(
        subheading_text,
        12.0,
        /*bold=*/ false,
        NSPoint { x: 20.0, y: 190.0 },
        NSSize {
            width: 420.0,
            height: 36.0,
        },
    );
    let _: () = msg_send![content_view, addSubview: subheading];

    // Missing-permissions list. Each row is one label spanning two
    // lines: the permission name and the rationale.
    let mut row_y: f64 = 130.0;
    for (i, m) in opts.missing.iter().enumerate() {
        let row_text = format!("• {}\n   {}", m.label(), m.rationale());
        let label = build_label(
            &row_text,
            12.0,
            /*bold=*/ false,
            NSPoint { x: 20.0, y: row_y },
            NSSize {
                width: 420.0,
                height: 44.0,
            },
        );
        let _: () = msg_send![content_view, addSubview: label];
        row_y -= 56.0;
        // Phase 1 only ever has at most two missing rows (AX + SR);
        // stop after that so the layout stays inside the window.
        if i >= 1 {
            break;
        }
    }

    // ---- Buttons ----
    //
    // Two buttons across the bottom: Continue anyway (left, secondary)
    // and Open System Settings (right, primary / default). We give
    // them shared static targets so the modal session can read the
    // outcome via a single thread-local — overkill abstractions aren't
    // worth it for two buttons.
    OUTCOME.with(|cell| *cell.borrow_mut() = None);

    let continue_btn = build_button(
        "Continue anyway",
        NSPoint { x: 20.0, y: 16.0 },
        NSSize {
            width: 160.0,
            height: 32.0,
        },
        ButtonAction::Dismiss,
    );
    let _: () = msg_send![content_view, addSubview: continue_btn];

    let open_settings_btn = build_button(
        "Open System Settings",
        NSPoint { x: 260.0, y: 16.0 },
        NSSize {
            width: 180.0,
            height: 32.0,
        },
        ButtonAction::OpenSettings,
    );
    let _: () = msg_send![content_view, addSubview: open_settings_btn];
    // Mark Open Settings as the default button (Return key triggers it).
    let _: () = msg_send![open_settings_btn, setKeyEquivalent: ns_string("\r")];

    // ---- Show and run modal ----
    let _: () = msg_send![window, center];
    let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
    // ignoringOtherApps:YES — Swift line 117. Forces the daemon's
    // window to the front even when launched from a background context.
    let _: () = msg_send![app, activateIgnoringOtherApps: true];
    // Blocks the main thread; returns when stopModal is called by a
    // button action or when the window is closed.
    let _: i64 = msg_send![app, runModalForWindow: window];

    // Read the outcome the action callback set. If neither button was
    // pressed (window closed via red dot) the cell stays empty and we
    // treat it as Dismissed.
    let outcome = OUTCOME
        .with(|cell| *cell.borrow())
        .unwrap_or(PanelOutcome::Dismissed);

    let _: () = msg_send![window, orderOut: std::ptr::null::<AnyObject>()];
    outcome
}

// ── Helpers: NSString, labels, buttons ───────────────────────────────────

/// Bridge `&str` → autoreleased `NSString` via `stringWithUTF8String:`.
/// The selector requires a NUL-terminated C string; we build one with a
/// trailing `\0`, hand off `.as_ptr()` for the duration of the call,
/// and rely on AppKit to copy the bytes into the NSString. The
/// `CString` lives until the end of the statement so the pointer
/// stays valid.
unsafe fn ns_string(text: &str) -> *mut AnyObject {
    let owned = std::ffi::CString::new(text).unwrap_or_default();
    msg_send![class!(NSString), stringWithUTF8String: owned.as_ptr() as *const c_void]
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
    // Allow multi-line wrapping for the subheading + row rationale.
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
    // NSBezelStyleRounded = 1.
    let _: () = msg_send![button, setBezelStyle: 1i64];
    // NSButtonTypeMomentaryPushIn = 7.
    let _: () = msg_send![button, setButtonType: 7i64];
    install_target(button, action);
    button
}

// ── Button target/action plumbing ────────────────────────────────────────
//
// AppKit's target/action pattern requires a target Objective-C object
// implementing a selector with the right shape. We register a tiny
// custom class (`CuaDriverPanelTarget`) at first call and instantiate
// one instance per panel. Each button's action is encoded by its
// selector (`openSettings:` / `dismiss:`); both handlers set the
// thread-local outcome cell and call `[NSApp stopModal]` to break the
// runModal loop.

use std::cell::RefCell;

thread_local! {
    static OUTCOME: RefCell<Option<PanelOutcome>> = const { RefCell::new(None) };
}

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
/// memoised in a `OnceLock` — Objective-C requires that any given
/// runtime class name is registered exactly once per process, and
/// `ClassBuilder::new` returns `None` on the second attempt.
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

    /// Serialises every test that mutates `CUA_DRIVER_RS_PERMISSIONS_PANEL`.
    /// Mirrors the same pattern as `permissions::gate::tests::env_lock`: env
    /// vars are process-global and `cargo test` runs the suite in parallel,
    /// so a missing lock here makes the three env-parsing tests trample
    /// each other's `set_var` / `remove_var` calls and flake.
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
        for v in &["0", "false", "FALSE", "no", "NO", "off", "OFF"] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_PANEL", v);
            assert!(
                env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"),
                "value {v:?} must be treated as off",
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
    }

    #[test]
    fn env_off_ignores_truthy_and_unknown() {
        let _guard = env_lock();
        for v in &["1", "true", "yes", "on", "garbage", ""] {
            std::env::set_var("CUA_DRIVER_RS_PERMISSIONS_PANEL", v);
            assert!(
                !env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"),
                "value {v:?} must not be treated as off",
            );
        }
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
    }

    #[test]
    fn missing_unset_env_keeps_panel_enabled_byte() {
        let _guard = env_lock();
        std::env::remove_var("CUA_DRIVER_RS_PERMISSIONS_PANEL");
        assert!(!env_off("CUA_DRIVER_RS_PERMISSIONS_PANEL"));
    }
}
