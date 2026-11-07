//! Hold the hardware mouse button while the pointer stays put, so a native
//! macOS drag-and-drop session can be driven by window-routed drag events.
//!
//! A drag-and-drop session (list reordering, file drags, `NSDraggingSession`)
//! ends the moment the window server reports the hardware button as up. A
//! press posted to one process does not change that state (it is the same
//! `CGSInputButtonState` that makes Chromium lose pointer capture, see
//! [`super::electron_drag`]), so the session ended about 0.6 s after it
//! started and the drop never landed.
//!
//! The hold makes the session believe the button is down without moving the
//! pointer:
//!
//! 1. A transparent, non-activating shield window is placed under the pointer,
//!    above every other window, so the one real press and release land on the
//!    driver's own window and never reach the app under the pointer.
//! 2. One hardware `leftMouseDown` is posted at the current pointer position.
//!    The pointer does not move.
//! 3. The caller keeps posting the gesture's `leftMouseDragged` events to the
//!    source process. The session follows their positions (the drag image and
//!    the destination's `draggingUpdated` track them).
//! 4. Before the window-routed release the hold posts the hardware
//!    `leftMouseUp` at the same position, then the caller posts its release at
//!    the drop point. The order matters: the other way round the session never
//!    sees the release.
//!
//! Costs and limits, all reported in the tool result: two hardware events are
//! posted (a down and an up, absorbed by the shield); the user's own pointer
//! movement or click during the gesture (about 2 s) interferes with the drag;
//! the shield needs the daemon's AppKit main loop (`serve`).

use std::ffi::c_void;
use std::sync::mpsc;
use std::time::Duration;

use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

/// How the gesture drives the hold. Real code uses [`PointerHold`]; tests use
/// a recording fake.
pub trait DragHold {
    /// Press the hardware button at the stationary pointer. `false` means the
    /// hold could not be made and nothing was posted.
    fn engage(&mut self) -> bool;
    /// Release the hardware button at the same position and remove the shield.
    fn release(&mut self);
    /// What the hold saw, for the tool result.
    fn report(&self) -> HoldReport;
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HoldReport {
    pub engaged: bool,
    /// The pointer position at the press.
    pub pointer: Option<(f64, f64)>,
    /// The pointer moved more than a few points between the press and the
    /// release, so the user was probably moving it and the drop may be off.
    pub pointer_moved: bool,
}

/// Pointer deviation (points) from the press position that counts as the user
/// moving the pointer.
const POINTER_MOVED_PX: f64 = 3.0;
/// Longest wait for the main queue to create or close the shield.
const MAIN_QUEUE_WAIT: Duration = Duration::from_millis(750);
/// Pause between the hardware release and the window-routed release.
pub const RELEASE_GAP: Duration = Duration::from_millis(100);

pub struct PointerHold {
    shield: Option<usize>,
    report: HoldReport,
    down: bool,
}

impl PointerHold {
    pub fn new() -> Self {
        Self {
            shield: None,
            report: HoldReport::default(),
            down: false,
        }
    }
}

impl Default for PointerHold {
    fn default() -> Self {
        Self::new()
    }
}

fn pointer_position() -> Option<CGPoint> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    Some(CGEvent::new(source).ok()?.location())
}

/// Whether the left button is down in hardware (the user is pressing).
fn left_button_down() -> bool {
    // kCGEventSourceStateHIDSystemState = 1, kCGMouseButtonLeft = 0.
    unsafe { CGEventSourceButtonState(1, 0) }
}

fn post_hardware(kind: CGEventType, at: CGPoint) -> bool {
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        return false;
    };
    let Ok(event) = CGEvent::new_mouse_event(source, kind, at, CGMouseButton::Left) else {
        return false;
    };
    event.post(CGEventTapLocation::HID);
    true
}

impl DragHold for PointerHold {
    fn engage(&mut self) -> bool {
        if self.down {
            return true;
        }
        // Never press over the user's own press.
        if left_button_down() {
            tracing::warn!("pointer hold: the left button is already down; not holding");
            return false;
        }
        let Some(at) = pointer_position() else {
            tracing::warn!("pointer hold: pointer position unavailable");
            return false;
        };
        // The shield first: the press must never reach the app under the pointer.
        let Some(shield) = open_shield(at) else {
            tracing::warn!(
                "pointer hold: the shield window could not be created on the main queue"
            );
            return false;
        };
        self.shield = Some(shield);
        if !post_hardware(CGEventType::LeftMouseDown, at) {
            self.release();
            return false;
        }
        self.down = true;
        self.report.engaged = true;
        self.report.pointer = Some((at.x, at.y));
        true
    }

    fn release(&mut self) {
        if self.down {
            if let Some((x, y)) = self.report.pointer {
                let at = CGPoint::new(x, y);
                if let Some(now) = pointer_position() {
                    self.report.pointer_moved = (now.x - x).hypot(now.y - y) > POINTER_MOVED_PX;
                }
                post_hardware(CGEventType::LeftMouseUp, at);
            }
            self.down = false;
        }
        // Keep the shield a moment so the release also lands on it.
        if let Some(shield) = self.shield.take() {
            std::thread::sleep(RELEASE_GAP);
            close_shield(shield);
        }
    }

    fn report(&self) -> HoldReport {
        self.report
    }
}

impl Drop for PointerHold {
    fn drop(&mut self) {
        // A failed gesture must not leave the hardware button down.
        self.release();
    }
}

// ── Shield window (main queue) ────────────────────────────────────────────

#[link(name = "System", kind = "framework")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGShieldingWindowLevel() -> i32;
    fn CGEventSourceButtonState(state_id: i32, button: u32) -> bool;
}

struct OpenJob {
    at: CGPoint,
    reply: mpsc::Sender<usize>,
}

struct CloseJob {
    window: usize,
    reply: mpsc::Sender<()>,
}

fn on_main<T: Send + 'static>(payload: T, cb: unsafe extern "C" fn(*mut c_void)) {
    let boxed = Box::new(payload);
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(main_queue, Box::into_raw(boxed) as *mut c_void, cb);
    }
}

/// Create the shield under `at` (global top-left display points) and return
/// its window pointer. `None` when the main queue is not running (no AppKit
/// loop in this process) or the window could not be made.
fn open_shield(at: CGPoint) -> Option<usize> {
    let (reply, rx) = mpsc::channel();
    on_main(OpenJob { at, reply }, open_shield_cb);
    match rx.recv_timeout(MAIN_QUEUE_WAIT) {
        Ok(0) | Err(_) => None,
        Ok(window) => Some(window),
    }
}

fn close_shield(window: usize) {
    let (reply, rx) = mpsc::channel();
    on_main(CloseJob { window, reply }, close_shield_cb);
    let _ = rx.recv_timeout(MAIN_QUEUE_WAIT);
}

unsafe extern "C" fn open_shield_cb(ctx: *mut c_void) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let job: OpenJob = *Box::from_raw(ctx as *mut OpenJob);
    // Display points are top-left; AppKit wants bottom-left of the main screen.
    let screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if screen.is_null() {
        let _ = job.reply.send(0);
        return;
    }
    let frame: NSRect = msg_send![screen, frame];
    let side = 40.0_f64;
    let rect = NSRect::new(
        NSPoint::new(
            job.at.x - side / 2.0,
            frame.size.height - job.at.y - side / 2.0,
        ),
        NSSize::new(side, side),
    );
    // NSWindowStyleMaskBorderless (0) | NSWindowStyleMaskNonactivatingPanel (1<<7)
    let style_mask: u64 = 1 << 7;
    let backing_store_buffered: u64 = 2;
    let alloc: *mut AnyObject = msg_send![class!(NSPanel), alloc];
    let win: *mut AnyObject = msg_send![
        alloc,
        initWithContentRect: rect
        styleMask: style_mask
        backing: backing_store_buffered
        defer: false
    ];
    if win.is_null() {
        let _ = job.reply.send(0);
        return;
    }
    // Almost clear but not fully: a fully transparent window is click-through,
    // and the shield exists to take the click.
    let tint: *mut AnyObject = msg_send![class!(NSColor), colorWithWhite: 0.0_f64 alpha: 0.01_f64];
    let _: () = msg_send![win, setBackgroundColor: tint];
    let _: () = msg_send![win, setOpaque: false];
    let _: () = msg_send![win, setHasShadow: false];
    let _: () = msg_send![win, setIgnoresMouseEvents: false];
    let _: () = msg_send![win, setBecomesKeyOnlyIfNeeded: true];
    let _: () = msg_send![win, setHidesOnDeactivate: false];
    let _: () = msg_send![win, setReleasedWhenClosed: false];
    let _: () = msg_send![win, setLevel: (CGShieldingWindowLevel() as i64) + 1];
    // 1<<0 CanJoinAllSpaces | 1<<4 Stationary | 1<<6 Transient | 1<<7 IgnoresCycle | 1<<8 FullScreenAuxiliary
    let behavior: u64 = (1 << 0) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 8);
    let _: () = msg_send![win, setCollectionBehavior: behavior];
    let _: () = msg_send![win, orderFrontRegardless];
    let _ = job.reply.send(win as usize);
}

unsafe extern "C" fn close_shield_cb(ctx: *mut c_void) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    let job: CloseJob = *Box::from_raw(ctx as *mut CloseJob);
    let win = job.window as *mut AnyObject;
    let _: () = msg_send![win, orderOut: std::ptr::null_mut::<AnyObject>()];
    let _: () = msg_send![win, close];
    // `releasedWhenClosed` is off and `init` returned a +1 reference.
    let _: () = msg_send![win, release];
    let _ = job.reply.send(());
}
