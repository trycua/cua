//! Keep the user's key window while a background target opens a menu.
//!
//! When an app that is not active opens a menu (a popup button, a `<select>`,
//! a context menu), AppKit's menu tracking calls
//! `SLPSStealKeyFocusReturningID`. WindowServer then tells the app that holds
//! key focus, normally the user's frontmost app, that it lost key focus, and
//! routes the keyboard to the menu until it closes. The user's window resigns
//! key and their typing stops landing in it, even though no app was activated
//! and no window was raised.
//!
//! While the driver acts on a background target, this module holds a
//! per-process event tap on the user's frontmost app. When that app is told it
//! lost key focus to a guarded target (a type-21 event, subtype `0x4000`, whose
//! field 73 is the thief's pid), the tap releases the theft by id with
//! `SLPSReleaseKeyFocusWithID` and drops the event. The user's app never sees
//! the loss; the menu stays open and is still operable through accessibility.
//!
//! Opening a menu also makes WindowServer order the menu's window group to the
//! front, which lifts the target's parent window above the user's window. No
//! client can stop that ordering or reorder another app's windows, so the
//! guard restores the order right after: once the menu window is on screen
//! (and again when the action ends), if a target window sits above the user's
//! front window, the already-active front app is re-activated with all its
//! windows. Key status does not move and the menu stays open; the raise is
//! visible for at most a frame or two.
//!
//! Theft ids come from one session-wide counter that only the thief learns.
//! [`protect`] therefore calibrates before each guarded action: it steals and
//! releases key focus on the driver's own connection (the tap drops that
//! notice too) and expects the target's theft to take the next id. If another
//! app steals in between, the tap tries the next few ids.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::input::skylight;

/// How long a target stays guarded after the action that armed it ends.
/// AppKit opens a pressed popup's menu from a delayed perform that runs just
/// after the accessibility call returns.
const TAIL: Duration = Duration::from_secs(3);
/// Ids tried after the expected one when another app stole in between.
const ID_SLACK: i32 = 3;
/// Longest wait for the thief's menu window before releasing the theft.
const MENU_WAIT: Duration = Duration::from_millis(150);
/// CGWindowLevel of popup menus (`kCGPopUpMenuWindowLevel`).
const POPUP_MENU_LAYER: i32 = 101;

/// Whether a normal-level window of `target` sits above the frontmost
/// normal-level window of `front` in the on-screen order.
fn target_above_front(target: i32, front: i32) -> bool {
    // Every layer, without the Space queries of `visible_windows`: this runs
    // while the raise is on screen, so it has to be quick.
    let windows = crate::windows::all_windows_any_layer();
    target_above_front_in(&windows, target, front)
}

fn target_above_front_in(windows: &[crate::windows::WindowInfo], target: i32, front: i32) -> bool {
    let top = |pid: i32| {
        windows
            .iter()
            .filter(|w| w.pid == pid && w.layer == 0 && w.is_on_screen)
            .filter(|w| w.bounds.width > 1.0 && w.bounds.height > 1.0)
            .map(|w| w.z_index)
            .max()
    };
    match (top(target), top(front)) {
        (Some(t), Some(f)) => t > f,
        _ => false,
    }
}

/// Put the user's front app back above a background target that raised its
/// window (AppKit orders a popup menu's window group to the front, which
/// lifts its parent window). Re-activating the already-active front app with
/// all its windows restores the order without moving key status; the
/// target's menu stays open.
fn restore_front_if_raised(target: i32, front: i32) {
    if crate::apps::frontmost_pid() != Some(front) || !target_above_front(target, front) {
        return;
    }
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    let restored = unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(front).is_some_and(|app| {
            app.activateWithOptions(NSApplicationActivationOptions::NSApplicationActivateAllWindows)
        })
    };
    RESTORED.fetch_add(1, Ordering::Relaxed);
    tracing::debug!(
        target,
        front,
        restored,
        "restored the user's window above a background target"
    );
}

/// How long the order stays watched after a guarded action ends. A target
/// can raise a window a moment after the call that caused it returns (a
/// banner, a sheet, or a menu that a renderer process opens later).
const ORDER_WATCH: Duration = Duration::from_millis(2500);
const ORDER_POLL: Duration = Duration::from_millis(20);

/// Keep restoring the user's window above `target` for a short while after
/// the action. One watcher per target; a later action extends it.
fn watch_order_after_action(target: i32, front: i32) {
    static WATCHING: OnceLock<Mutex<HashMap<i32, Instant>>> = OnceLock::new();
    let watching = WATCHING.get_or_init(|| Mutex::new(HashMap::new()));
    let until = Instant::now() + ORDER_WATCH;
    {
        let mut map = watching
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let running = map.contains_key(&target);
        map.insert(target, until);
        if running {
            return;
        }
    }
    let spawned = std::thread::Builder::new()
        .name("cua-order-watch".into())
        .spawn(move || loop {
            std::thread::sleep(ORDER_POLL);
            restore_front_if_raised(target, front);
            let mut map = watching
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if map
                .get(&target)
                .is_none_or(|until| Instant::now() >= *until)
            {
                map.remove(&target);
                return;
            }
        });
    if spawned.is_err() {
        watching
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&target);
    }
}

/// Wait until `pid` shows a menu-level window, or `budget` runs out.
fn wait_for_menu_window(pid: i32, budget: Duration) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        // `visible_windows` lists layer 0 only; a menu is at layer 101.
        if crate::windows::all_windows_any_layer()
            .iter()
            .any(|w| w.pid == pid && w.layer >= POPUP_MENU_LAYER && w.is_on_screen)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// CGEvent type of the WindowServer focus notices an app receives.
const FOCUS_NOTICE_TYPE: u32 = 21;
/// Field holding the notice subtype.
const FIELD_SUBTYPE: u32 = 64;
/// Field holding the pid of the process that took key focus.
const FIELD_THIEF_PID: u32 = 73;
/// Subtype: "you lost key focus".
const SUBTYPE_LOST_KEY_FOCUS: i64 = 0x4000;

const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventTapCreateForPid(
        pid: i32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;
    fn CFMachPortInvalidate(port: *mut c_void);
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRun();
    fn CFRunLoopStop(rl: *mut c_void);
    fn CFRelease(cf: *const c_void);
    static kCFRunLoopCommonModes: *const c_void;
}

/// What the guard did, for tool results and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Thefts by a guarded target that were released.
    pub released: u64,
    /// Thefts by a guarded target that could not be released.
    pub missed: u64,
}

struct Tap {
    front_pid: i32,
    port: usize,
    run_loop: usize,
}

#[derive(Default)]
struct State {
    tap: Option<Tap>,
    /// Guarded pid -> (live leases, end of the tail after the last lease).
    guarded: HashMap<i32, (usize, Instant)>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The id the next theft is expected to take (0 = unknown).
static NEXT_THEFT_ID: AtomicI32 = AtomicI32::new(0);
/// Calibration notices the tap has dropped.
static OWN_NOTICES: AtomicU64 = AtomicU64::new(0);
/// A front app whose tap missed a calibration notice (0 = none).
static UNVERIFIED_FRONT: AtomicI32 = AtomicI32::new(0);
static RELEASED: AtomicU64 = AtomicU64::new(0);
static MISSED: AtomicU64 = AtomicU64::new(0);
static RESTORED: AtomicU64 = AtomicU64::new(0);

/// Counters since the daemon started.
pub fn stats() -> Stats {
    Stats {
        released: RELEASED.load(Ordering::Relaxed),
        missed: MISSED.load(Ordering::Relaxed),
    }
}

/// While alive, key-focus thefts by the guarded pid are handed back to the
/// user's frontmost app. Dropping it starts a short tail (see [`TAIL`]).
pub struct Lease {
    pid: i32,
    front: i32,
    /// A target window already sat above the user's window when the action
    /// started (the user arranged it so); then nothing is restored.
    above_before: bool,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if !self.above_before {
            restore_front_if_raised(self.pid, self.front);
            watch_order_after_action(self.pid, self.front);
        }
        let mut st = lock();
        if let Some((leases, until)) = st.guarded.get_mut(&self.pid) {
            *leases = leases.saturating_sub(1);
            *until = Instant::now() + TAIL;
        }
        drop(st);
        schedule_idle_teardown();
    }
}

/// Guard the user's key focus while the driver acts on `target_pid` in the
/// background. Returns `None` (no guard) when the target is the frontmost app,
/// when there is no other frontmost app, or when the SPIs or the event tap are
/// unavailable; the action then runs exactly as before.
pub fn protect(target_pid: i32) -> Option<Lease> {
    if std::env::var_os("CUA_DRIVER_DISABLE_KEY_FOCUS_GUARD").is_some() {
        return None;
    }
    let own = std::process::id() as i32;
    if !skylight::key_focus_theft_available() {
        return None;
    }
    // Tap the process that holds key focus now. That is the one WindowServer
    // tells when a menu steals it, and it may be a non-activating panel
    // rather than the frontmost app. Only guard when it is the frontmost app:
    // otherwise a launcher-style panel would see the calibration theft.
    let front = crate::apps::frontmost_pid()?;
    let holder = skylight::key_focus_pid().unwrap_or(front);
    if holder != front || front == target_pid || front == own {
        return None;
    }
    if UNVERIFIED_FRONT.load(Ordering::SeqCst) == front {
        return None;
    }
    // Register first, so an idle teardown cannot remove the tap between
    // ensure_tap and the calibration below.
    {
        let mut st = lock();
        let entry = st.guarded.entry(target_pid).or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now() + TAIL;
    }
    let lease = Lease {
        pid: target_pid,
        front,
        above_before: target_above_front(target_pid, front),
    };
    if !ensure_tap(front) {
        return None;
    }
    if !calibrate() {
        // The tap did not see the calibration notice, so it would not see a
        // menu's either. Stop calibrating against this app.
        UNVERIFIED_FRONT.store(front, Ordering::SeqCst);
        tracing::warn!(
            front,
            "key-focus guard: tap did not observe the calibration notice"
        );
        return None;
    }
    Some(lease)
}

/// Learn the id the next theft will take: steal and release key focus on the
/// driver's own connection. The tap drops the notice this sends to the user's
/// app, so it never sees the round trip. Returns whether the tap saw it.
fn calibrate() -> bool {
    let seen_before = OWN_NOTICES.load(Ordering::SeqCst);
    let Some(id) = skylight::steal_key_focus() else {
        return false;
    };
    skylight::release_key_focus(id);
    NEXT_THEFT_ID.store(id.wrapping_add(1), Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_millis(150);
    while OWN_NOTICES.load(Ordering::SeqCst) == seen_before {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    true
}

fn is_guarded(pid: i32) -> bool {
    let st = lock();
    st.guarded
        .get(&pid)
        .is_some_and(|(leases, until)| *leases > 0 || Instant::now() < *until)
}

/// Release the theft a guarded target just made. Tries the expected id first,
/// then a few later ones in case another app stole in between.
fn release_target_theft() -> bool {
    let expected = NEXT_THEFT_ID.load(Ordering::SeqCst);
    if expected == 0 {
        return false;
    }
    for id in expected..=expected.wrapping_add(ID_SLACK) {
        if skylight::release_key_focus(id) == 0 {
            NEXT_THEFT_ID.store(id.wrapping_add(1), Ordering::SeqCst);
            return true;
        }
    }
    false
}

/// What the tap does with one focus notice. Pure, for tests.
#[derive(Debug, PartialEq, Eq)]
enum NoticeAction {
    Pass,
    /// The driver's own calibration theft: swallow it.
    DropOwn,
    /// A guarded target stole key focus: release it, then swallow the notice.
    ReleaseAndDrop,
}

fn classify_notice(
    event_type: u32,
    subtype: i64,
    thief_pid: i64,
    own_pid: i32,
    guarded: impl Fn(i32) -> bool,
) -> NoticeAction {
    if event_type != FOCUS_NOTICE_TYPE || subtype != SUBTYPE_LOST_KEY_FOCUS {
        return NoticeAction::Pass;
    }
    let Ok(thief) = i32::try_from(thief_pid) else {
        return NoticeAction::Pass;
    };
    if thief == own_pid {
        NoticeAction::DropOwn
    } else if guarded(thief) {
        NoticeAction::ReleaseAndDrop
    } else {
        NoticeAction::Pass
    }
}

extern "C" fn tap_callback(
    _proxy: *mut c_void,
    event_type: u32,
    event: *mut c_void,
    _user_info: *mut c_void,
) -> *mut c_void {
    if event_type == TAP_DISABLED_BY_TIMEOUT || event_type == TAP_DISABLED_BY_USER_INPUT {
        if let Some(port) = lock().tap.as_ref().map(|tap| tap.port) {
            unsafe { CGEventTapEnable(port as *mut c_void, true) };
        }
        return event;
    }
    if event.is_null() {
        return event;
    }
    let (subtype, thief) = unsafe {
        (
            CGEventGetIntegerValueField(event, FIELD_SUBTYPE),
            CGEventGetIntegerValueField(event, FIELD_THIEF_PID),
        )
    };
    match classify_notice(
        event_type,
        subtype,
        thief,
        std::process::id() as i32,
        is_guarded,
    ) {
        NoticeAction::Pass => event,
        NoticeAction::DropOwn => {
            OWN_NOTICES.fetch_add(1, Ordering::SeqCst);
            std::ptr::null_mut()
        }
        NoticeAction::ReleaseAndDrop => {
            // Swallow the notice now, so the user's window never resigns key,
            // but release the theft only once the menu window is on screen.
            // Released earlier, AppKit orders the menu's window group to the
            // front, which raises the target's window over the user's.
            let thief_pid = thief as i32;
            let spawned = std::thread::Builder::new()
                .name("cua-key-focus-release".into())
                .spawn(move || {
                    wait_for_menu_window(thief_pid, MENU_WAIT);
                    let front = lock().tap.as_ref().map(|tap| tap.front_pid);
                    if let Some(front) = front {
                        restore_front_if_raised(thief_pid, front);
                    }
                    if release_target_theft() {
                        RELEASED.fetch_add(1, Ordering::Relaxed);
                        tracing::debug!(thief_pid, "released a background key-focus theft");
                    } else {
                        MISSED.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(thief_pid, "could not release a background key-focus theft");
                    }
                });
            if spawned.is_ok() {
                std::ptr::null_mut()
            } else {
                event
            }
        }
    }
}

/// Make sure the tap sits on `front_pid`, the app that holds key focus.
fn ensure_tap(front_pid: i32) -> bool {
    {
        let mut st = lock();
        if st
            .tap
            .as_ref()
            .is_some_and(|tap| tap.front_pid == front_pid)
        {
            return true;
        }
        if let Some(old) = st.tap.take() {
            stop_tap(old);
        }
    }
    let (tx, rx) = std::sync::mpsc::channel::<Option<(usize, usize)>>();
    let spawned = std::thread::Builder::new()
        .name("cua-key-focus-guard".into())
        .spawn(move || unsafe {
            let mask = 1u64 << FOCUS_NOTICE_TYPE;
            // kCGHeadInsertEventTap = 0, kCGEventTapOptionDefault = 0.
            let port =
                CGEventTapCreateForPid(front_pid, 0, 0, mask, tap_callback, std::ptr::null_mut());
            if port.is_null() {
                let _ = tx.send(None);
                return;
            }
            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
            if source.is_null() {
                CFMachPortInvalidate(port);
                CFRelease(port);
                let _ = tx.send(None);
                return;
            }
            let run_loop = CFRunLoopGetCurrent();
            CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
            CGEventTapEnable(port, true);
            let _ = tx.send(Some((port as usize, run_loop as usize)));
            CFRunLoopRun();
            CFMachPortInvalidate(port);
            CFRelease(source);
            CFRelease(port);
        });
    if spawned.is_err() {
        return false;
    }
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Some((port, run_loop))) => {
            let mut st = lock();
            if let Some(old) = st.tap.replace(Tap {
                front_pid,
                port,
                run_loop,
            }) {
                stop_tap(old);
            }
            true
        }
        _ => {
            tracing::warn!(front_pid, "key-focus guard: event tap unavailable");
            false
        }
    }
}

fn stop_tap(tap: Tap) {
    unsafe {
        CGEventTapEnable(tap.port as *mut c_void, false);
        CFRunLoopStop(tap.run_loop as *mut c_void);
    }
}

/// Remove the tap once nothing has been guarded for a while, so the driver
/// does not keep a tap on the user's app between tasks.
fn schedule_idle_teardown() {
    let _ = std::thread::Builder::new()
        .name("cua-key-focus-guard-idle".into())
        .spawn(|| {
            std::thread::sleep(TAIL + Duration::from_secs(1));
            let mut st = lock();
            let now = Instant::now();
            st.guarded
                .retain(|_, (leases, until)| *leases > 0 || now < *until);
            if st.guarded.is_empty() {
                if let Some(tap) = st.tap.take() {
                    stop_tap(tap);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: i32 = 100;

    #[test]
    fn a_guarded_target_theft_is_released_and_swallowed() {
        let action = classify_notice(21, 0x4000, 42, OWN, |pid| pid == 42);
        assert_eq!(action, NoticeAction::ReleaseAndDrop);
    }

    fn win(pid: i32, layer: i32, z_index: usize) -> crate::windows::WindowInfo {
        crate::windows::WindowInfo {
            window_id: z_index as u32,
            pid,
            app_name: String::new(),
            title: String::new(),
            bounds: crate::windows::WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 300.0,
            },
            layer,
            z_index,
            is_on_screen: true,
            current_space_id: None,
            on_current_space: None,
            space_ids: None,
        }
    }

    #[test]
    fn a_target_window_above_the_users_window_is_detected() {
        // z_index: higher is closer to the front.
        let raised = [win(7, 0, 3), win(42, 0, 5), win(7, 0, 4)];
        assert!(target_above_front_in(&raised, 42, 7));
        let below = [win(7, 0, 5), win(42, 0, 3)];
        assert!(!target_above_front_in(&below, 42, 7));
        // A menu-level window does not count; only normal windows are raised.
        let menu_only = [win(7, 0, 4), win(42, 101, 9), win(42, 0, 2)];
        assert!(!target_above_front_in(&menu_only, 42, 7));
        // Without a window of either app there is nothing to restore.
        assert!(!target_above_front_in(&[win(42, 0, 9)], 42, 7));
    }

    #[test]
    fn the_drivers_own_calibration_theft_is_swallowed() {
        assert_eq!(
            classify_notice(21, 0x4000, OWN as i64, OWN, |_| false),
            NoticeAction::DropOwn
        );
    }

    #[test]
    fn other_apps_and_other_notices_pass_through() {
        // An app the driver is not acting on keeps normal macOS behaviour.
        assert_eq!(
            classify_notice(21, 0x4000, 7, OWN, |pid| pid == 42),
            NoticeAction::Pass
        );
        // "Key focus returned" and other subtypes are never touched.
        assert_eq!(
            classify_notice(21, 0x8000, 42, OWN, |_| true),
            NoticeAction::Pass
        );
        // Other event types are never touched.
        assert_eq!(
            classify_notice(13, 0x4000, 42, OWN, |_| true),
            NoticeAction::Pass
        );
        // A malformed pid passes.
        assert_eq!(
            classify_notice(21, 0x4000, i64::MAX, OWN, |_| true),
            NoticeAction::Pass
        );
    }
}
