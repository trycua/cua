// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Background input for apps that drop events while they believe they are
//! inactive: GTK 3 on Quartz (GnuCash, GIMP, Inkscape builds).
//!
//! GDK's Quartz backend checks `[NSApp isActive]` when a mouse-down arrives.
//! While the app is inactive it calls `[NSApp activateIgnoringOtherApps:YES]`
//! and drops the event (`gdk/quartz/gdkevents-quartz.c`, gtk-3-24), so a
//! background click raises the app and takes the user's focus instead of
//! landing. Its keys are routed through `[nsevent window]`, which an inactive
//! app does not have.
//!
//! AppKit flips `NSApp.active` when it receives an `NSEventTypeAppKitDefined`
//! event with subtype `NSEventSubtypeApplicationActivated`. Posting that event
//! to the target pid alone makes the app believe it is active while
//! WindowServer's front process stays the user's: the target's `AXFrontmost`
//! turns true, the user's window keeps key, and nothing is raised. Subtype
//! `NSEventSubtypeApplicationDeactivated` undoes it.
//!
//! While active, GDK takes mouse events built as
//! `NSEvent mouseEventWithType:…windowNumber:` and converted with `.CGEvent`.
//! Plain `CGEventCreateMouseEvent` events are still dropped: GDK looks up the
//! event's window through the NSEvent's window number, which only the NSEvent
//! constructor fills in.
//!
//! [`ActivationLease`] holds the activated state for one app across the
//! actions of a burst and posts the deactivation when the burst goes quiet,
//! so the app is never left believing it is active.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// `NSEventTypeAppKitDefined`.
const APPKIT_DEFINED: u64 = 13;
/// `NSEventSubtypeApplicationActivated`.
const SUBTYPE_ACTIVATED: i16 = 1;
/// `NSEventSubtypeApplicationDeactivated`.
const SUBTYPE_DEACTIVATED: i16 = 2;
/// How long the activated state outlives the last action of a burst. A
/// sequence of clicks and keys within this window shares one activation; the
/// deactivation goes out once the target has been idle this long.
const IDLE_DEACTIVATE: Duration = Duration::from_millis(1500);
/// Settle after the activation event, so AppKit has handled it before the
/// first input event arrives.
const ACTIVATION_SETTLE: Duration = Duration::from_millis(120);
/// How long to wait for `AXFrontmost` to report the activation.
const ACTIVATION_CONFIRM: Duration = Duration::from_millis(400);

/// One pid's activation as the table tracks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// No pid holds an activation.
    Idle,
    /// The activation event went out and is being confirmed; nobody may send
    /// input to that pid yet.
    Activating,
    /// Confirmed. `holders` actions are using it; at zero the idle timer
    /// deactivates it.
    Active,
}

/// What an acquirer does next, from the table as it stands. Pure, so the
/// lifecycle is testable without a window server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    /// The pid is already confirmed active: join as another holder.
    Join,
    /// Another acquirer is confirming this pid, or another pid has actions in
    /// flight: wait and look again.
    Wait,
    /// Post the activation (after deactivating `previous` when it was active).
    Activate { deactivate_previous: bool },
}

fn decide(table_pid: Option<i32>, phase: Phase, holders: usize, pid: i32) -> Decision {
    match (table_pid, phase) {
        (_, Phase::Idle) | (None, _) => Decision::Activate {
            deactivate_previous: false,
        },
        (Some(p), Phase::Active) if p == pid => Decision::Join,
        (Some(p), Phase::Activating) if p == pid => Decision::Wait,
        (Some(_), Phase::Activating) => Decision::Wait,
        (Some(_), Phase::Active) if holders > 0 => Decision::Wait,
        (Some(_), Phase::Active) => Decision::Activate {
            deactivate_previous: true,
        },
    }
}

/// The one activation this process holds. Only one pid is AppKit-active
/// through this route at a time.
struct Table {
    pid: Option<i32>,
    window_id: u32,
    phase: Phase,
    holders: usize,
    /// Bumped on every acquire and release, so a pending idle timer can tell
    /// whether anything happened since it was armed.
    generation: u64,
}

fn table() -> &'static Mutex<Table> {
    static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(Table {
            pid: None,
            window_id: 0,
            phase: Phase::Idle,
            holders: 0,
            generation: 0,
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, Table> {
    table().lock().unwrap_or_else(|e| e.into_inner())
}

/// How long an acquirer waits for another pid's actions or another
/// acquirer's confirmation before refusing.
const ACQUIRE_WAIT: Duration = Duration::from_secs(10);

/// Holds `pid` AppKit-active (without raising it or taking the user's focus)
/// for the duration of one action. When the last holder drops, the
/// deactivation goes out after [`IDLE_DEACTIVATE`] unless another action
/// joins first.
pub struct ActivationLease {
    pid: i32,
}

impl ActivationLease {
    /// Activate `pid` through `window_id`, or join its confirmed activation.
    /// Returns `Err` when the activation could not be confirmed: input must
    /// not be sent then, because GDK would activate itself and raise the app.
    pub fn acquire(pid: i32, window_id: u32) -> Result<Self, String> {
        let deadline = Instant::now() + ACQUIRE_WAIT;
        let previous = loop {
            let mut t = lock();
            match decide(t.pid, t.phase, t.holders, pid) {
                Decision::Join => {
                    t.holders += 1;
                    t.generation += 1;
                    return Ok(Self { pid });
                }
                Decision::Wait => {
                    drop(t);
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "pid {pid} could not be made active: another app's background input \
                             through the AppKit activation route did not finish in time"
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Decision::Activate {
                    deactivate_previous,
                } => {
                    let previous = deactivate_previous
                        .then_some(t.pid.zip(Some(t.window_id)))
                        .flatten();
                    t.pid = Some(pid);
                    t.window_id = window_id;
                    t.phase = Phase::Activating;
                    t.holders = 1;
                    t.generation += 1;
                    break previous;
                }
            }
        };
        if let Some((old_pid, old_wid)) = previous {
            post_appkit_event(old_pid, old_wid, SUBTYPE_DEACTIVATED);
        }
        post_appkit_event(pid, window_id, SUBTYPE_ACTIVATED);
        std::thread::sleep(ACTIVATION_SETTLE);
        let confirmed = wait_frontmost(pid, true, ACTIVATION_CONFIRM);
        let mut t = lock();
        if confirmed {
            t.phase = Phase::Active;
            return Ok(Self { pid });
        }
        t.pid = None;
        t.phase = Phase::Idle;
        t.holders = 0;
        t.generation += 1;
        drop(t);
        post_appkit_event(pid, window_id, SUBTYPE_DEACTIVATED);
        Err(format!(
            "pid {pid} did not report itself active after the AppKit activation event \
             (AXFrontmost stayed false), so no input was sent: its toolkit would have raised \
             the app instead"
        ))
    }
}

impl Drop for ActivationLease {
    fn drop(&mut self) {
        let generation = {
            let mut t = lock();
            if t.pid != Some(self.pid) || t.holders == 0 {
                return;
            }
            t.holders -= 1;
            t.generation += 1;
            if t.holders > 0 {
                return;
            }
            t.generation
        };
        let pid = self.pid;
        std::thread::spawn(move || {
            std::thread::sleep(IDLE_DEACTIVATE);
            let window_id = {
                let mut t = lock();
                if t.pid != Some(pid)
                    || t.generation != generation
                    || t.holders > 0
                    || t.phase != Phase::Active
                {
                    return;
                }
                t.pid = None;
                t.phase = Phase::Idle;
                t.generation += 1;
                t.window_id
            };
            post_appkit_event(pid, window_id, SUBTYPE_DEACTIVATED);
        });
    }
}

/// Post the AppKit activation or deactivation event to `pid` only.
#[cfg(target_os = "macos")]
fn post_appkit_event(pid: i32, window_id: u32, subtype: i16) {
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::NSPoint;
    unsafe {
        objc2::rc::autoreleasepool(|_| {
            let event: *mut AnyObject = msg_send![class!(NSEvent),
                otherEventWithType: APPKIT_DEFINED
                location: NSPoint { x: 0.0, y: 0.0 }
                modifierFlags: 0u64
                timestamp: 0.0_f64
                windowNumber: window_id as isize
                context: std::ptr::null::<AnyObject>()
                subtype: subtype
                data1: 0isize
                data2: 0isize
            ];
            if event.is_null() {
                return;
            }
            let cg: *mut std::ffi::c_void = msg_send![event, CGEvent];
            if !cg.is_null() {
                CGEventPostToPid(pid, cg);
            }
        });
    }
}

#[cfg(not(target_os = "macos"))]
fn post_appkit_event(_pid: i32, _window_id: u32, _subtype: i16) {}

/// Wait until `pid`'s `AXFrontmost` reads `want`.
fn wait_frontmost(pid: i32, want: bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if ax_frontmost(pid) == Some(want) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn ax_frontmost(pid: i32) -> Option<bool> {
    use crate::ax::bindings::{copy_bool_attr, AXUIElementCreateApplication};
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let value = copy_bool_attr(app, "AXFrontmost");
        core_foundation::base::CFRelease(app as core_foundation::base::CFTypeRef);
        value
    }
}

/// Mouse event kinds the GDK route builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Moved,
    LeftDown,
    LeftUp,
}

impl MouseKind {
    /// `NSEventType` raw value.
    fn ns_type(self) -> u64 {
        match self {
            Self::LeftDown => 1,
            Self::LeftUp => 2,
            Self::Moved => 5,
        }
    }
}

/// One event of a click as the GDK route posts it: the window-local point in
/// AppKit's bottom-left window coordinates, the click count and the pressure.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PlannedMouseEvent {
    kind: MouseKind,
    /// AppKit window coordinates (bottom-left origin), for the NSEvent.
    window_point: (f64, f64),
    /// The same point with a top-left origin, for `CGEventSetWindowLocation`.
    local: (f64, f64),
    click_count: i64,
    pressure: f32,
}

/// The click as events: a move to the point, then `count` down/up pairs.
/// `local` is the top-left window-local point and `window_height` the window
/// frame's height, both in points.
fn click_plan(local: (f64, f64), window_height: f64, count: usize) -> Vec<PlannedMouseEvent> {
    let window_point = (local.0, window_height - local.1);
    let mut plan = vec![PlannedMouseEvent {
        kind: MouseKind::Moved,
        window_point,
        local,
        click_count: 0,
        pressure: 0.0,
    }];
    for n in 1..=count.max(1) as i64 {
        plan.push(PlannedMouseEvent {
            kind: MouseKind::LeftDown,
            window_point,
            local,
            click_count: n,
            pressure: 1.0,
        });
        plan.push(PlannedMouseEvent {
            kind: MouseKind::LeftUp,
            window_point,
            local,
            click_count: n,
            pressure: 0.0,
        });
    }
    plan
}

/// Gap after each posted event; GDK coalesces closer events.
const EVENT_GAP: Duration = Duration::from_millis(60);

/// Click `count` times at window-local `local` (top-left origin, points) of
/// `window_id`, with `screen` the same point in screen coordinates. The
/// caller holds an [`ActivationLease`] for `pid`.
#[cfg(target_os = "macos")]
pub fn click(
    pid: i32,
    window_id: u32,
    screen: (f64, f64),
    local: (f64, f64),
    window_height: f64,
    count: usize,
) -> anyhow::Result<()> {
    for event in click_plan(local, window_height, count) {
        post_ns_mouse_event(pid, window_id, screen, event)?;
        std::thread::sleep(EVENT_GAP);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn post_ns_mouse_event(
    pid: i32,
    window_id: u32,
    screen: (f64, f64),
    event: PlannedMouseEvent,
) -> anyhow::Result<()> {
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::NSPoint;
    let posted = unsafe {
        objc2::rc::autoreleasepool(|_| {
            let process_info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            let uptime: f64 = msg_send![process_info, systemUptime];
            let ns_event: *mut AnyObject = msg_send![class!(NSEvent),
                mouseEventWithType: event.kind.ns_type()
                location: NSPoint { x: event.window_point.0, y: event.window_point.1 }
                modifierFlags: 0u64
                timestamp: uptime
                windowNumber: window_id as isize
                context: std::ptr::null::<AnyObject>()
                eventNumber: 0isize
                clickCount: event.click_count as isize
                pressure: event.pressure
            ];
            if ns_event.is_null() {
                return false;
            }
            let cg: *mut std::ffi::c_void = msg_send![ns_event, CGEvent];
            if cg.is_null() {
                return false;
            }
            CGEventSetLocation(cg, screen.0, screen.1);
            crate::input::skylight::set_window_location(cg, event.local.0, event.local.1);
            CGEventSetIntegerValueField(cg, WINDOW_UNDER_POINTER, window_id as i64);
            CGEventSetIntegerValueField(cg, WINDOW_UNDER_POINTER_THAT_CAN_HANDLE, window_id as i64);
            CGEventPostToPid(pid, cg);
            true
        })
    };
    if posted {
        Ok(())
    } else {
        anyhow::bail!("NSEvent could not build the {:?} event", event.kind)
    }
}

/// `kCGMouseEventWindowUnderMousePointer`.
const WINDOW_UNDER_POINTER: u32 = 91;
/// `kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent`.
const WINDOW_UNDER_POINTER_THAT_CAN_HANDLE: u32 = 92;

#[cfg(target_os = "macos")]
extern "C" {
    fn CGEventPostToPid(pid: i32, event: *mut std::ffi::c_void);
    fn CGEventSetLocation(event: *mut std::ffi::c_void, x: f64, y: f64);
    fn CGEventSetIntegerValueField(event: *mut std::ffi::c_void, field: u32, value: i64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquirers_join_a_confirmed_activation_and_wait_out_an_unconfirmed_one() {
        let first = Decision::Activate {
            deactivate_previous: false,
        };
        assert_eq!(decide(None, Phase::Idle, 0, 7), first);
        assert_eq!(decide(Some(7), Phase::Active, 1, 7), Decision::Join);
        assert_eq!(decide(Some(7), Phase::Active, 0, 7), Decision::Join);
        assert_eq!(
            decide(Some(7), Phase::Activating, 1, 7),
            Decision::Wait,
            "no input before the activation is confirmed"
        );
    }

    #[test]
    fn another_pid_waits_while_actions_are_in_flight_and_takes_over_an_idle_activation() {
        assert_eq!(decide(Some(7), Phase::Active, 1, 9), Decision::Wait);
        assert_eq!(decide(Some(7), Phase::Activating, 1, 9), Decision::Wait);
        assert_eq!(
            decide(Some(7), Phase::Active, 0, 9),
            Decision::Activate {
                deactivate_previous: true
            }
        );
    }

    #[test]
    fn a_click_is_a_move_then_counted_down_up_pairs_in_appkit_window_coordinates() {
        let plan = click_plan((90.0, 166.0), 800.0, 2);
        let kinds: Vec<_> = plan.iter().map(|e| (e.kind, e.click_count)).collect();
        assert_eq!(
            kinds,
            vec![
                (MouseKind::Moved, 0),
                (MouseKind::LeftDown, 1),
                (MouseKind::LeftUp, 1),
                (MouseKind::LeftDown, 2),
                (MouseKind::LeftUp, 2),
            ]
        );
        assert!(plan
            .iter()
            .all(|e| e.window_point == (90.0, 634.0) && e.local == (90.0, 166.0)));
        assert_eq!(plan[1].pressure, 1.0);
        assert_eq!(plan[2].pressure, 0.0);
    }

    #[test]
    fn a_zero_count_still_clicks_once() {
        assert_eq!(click_plan((1.0, 1.0), 10.0, 0).len(), 3);
    }
}
