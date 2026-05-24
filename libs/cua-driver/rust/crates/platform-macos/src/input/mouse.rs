//! Background mouse event synthesis via SLEventPostToPid (SkyLight SPI),
//! with fallback to the public CGEvent::post_to_pid for older OS releases.
//!
//! SLEventPostToPid goes through the IOHIDPostEvent path which:
//! - Triggers CGSTickleActivityMonitor (required for Catalyst / Chromium)
//! - Reaches Mac Catalyst windows that CGEventPostToPid misses
//!
//! Mouse events do NOT attach the SLSEventAuthenticationMessage envelope
//! (Swift reference: `attachAuthMessage: false`) — the auth envelope routes
//! events through a direct Mach delivery path that bypasses
//! cgAnnotatedSessionEventTap, which Chromium's window handler subscribes to.

use core_graphics::{
    event::{CGEvent, CGEventFlags, CGEventType, CGMouseButton},
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use foreign_types::ForeignType;

/// Left-click at `(x, y)` screen coordinates, posted to `pid`.
///
/// Window-local coordinates for backgrounded targets: if `window_local` is
/// `Some((wx, wy))`, stamps a window-local point via `CGEventSetWindowLocation`
/// SPI so WindowServer's hit-test uses the local point directly.
pub fn click_at_xy(pid: i32, x: f64, y: f64, count: usize, modifiers: &[&str]) -> anyhow::Result<()> {
    click_at_xy_inner(pid, x, y, None, None, count, modifiers)
}

/// Like `click_at_xy` but also stamps window-local `(wx, wy)` onto the event.
/// When `wid` is provided, additionally stamps Chromium routing fields
/// (f51 / f58 / f91 / f92) for better backgrounded-target delivery.
pub fn click_at_xy_with_window_local(
    pid: i32,
    x: f64, y: f64,
    wx: f64, wy: f64,
    wid: u32,
    count: usize,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    click_at_xy_inner(pid, x, y, Some((wx, wy)), Some(wid), count, modifiers)
}

fn click_at_xy_inner(
    pid: i32,
    x: f64, y: f64,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    count: usize,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let point = CGPoint::new(x, y);
    let flags = parse_modifier_flags(modifiers);

    // Shared click-group ID (f58) when window_id is known: keeps all pairs
    // in one gesture so WindowServer / Chromium coalesce them correctly.
    let click_group_id: Option<i64> = wid.map(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as i64
    });

    for pair_index in 0..count {
        let click_state = (pair_index + 1) as i64;

        let down = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseDown,
            point,
            CGMouseButton::Left,
        ).map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(down) failed"))?;
        if flags != CGEventFlags::CGEventFlagNull {
            down.set_flags(flags);
        }

        post_mouse_event(pid, &down, window_local, wid, click_group_id, click_state);
        std::thread::sleep(std::time::Duration::from_millis(16));

        let up = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseUp,
            point,
            CGMouseButton::Left,
        ).map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(up) failed"))?;
        if flags != CGEventFlags::CGEventFlagNull {
            up.set_flags(flags);
        }

        post_mouse_event(pid, &up, window_local, wid, click_group_id, click_state);

        if count > 1 {
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }
    Ok(())
}

/// Full Chromium-compatible left-click recipe matching Swift's `clickViaAuthSignedPost`.
///
/// Sequence (Swift reference minus the `activate_without_raise` prologue — that step
/// causes window-z side effects under Rust's overlay; it is reserved for a future
/// explicit opt-in once the overlay re-pins after focus changes):
///  1. Stamped `mouseMoved` at target coords (f0=2, cursor-state primer).
///  2. Off-screen primer down/up at (-1, -1) (f0=1/2) — satisfies Chromium's
///     user-activation gate without hitting any DOM element.
///  3. Target down/up pair(s) at real coordinates (f0=3/3), clickState 1→N.
///
/// All events carry:
///  - f0  = gesture phase marker (move=2, primerDown=1, primerUp=2, target=3)
///  - f1  = mouseEventClickState (1 for single, 1→2 for double)
///  - f3  = 0  (left button)
///  - f7  = 3  (NSEventSubtypeTouch)
///  - f40 = target pid   (Chromium synthetic-event filter)
///  - f51 / f91 / f92 = CGWindowID (window routing)
///  - f58 = constant click-group ID across all events (gesture coalescing)
///  - `CGEventSetWindowLocation` per-event (window-local point)
///
/// Uses both SkyLight `SLEventPostToPid` AND `CGEvent::post_to_pid` (belt+suspenders)
/// for AppKit / Catalyst target coverage.
pub fn click_at_xy_chromium(
    pid: i32,
    screen_x: f64,
    screen_y: f64,
    win_local_x: f64,
    win_local_y: f64,
    wid: u32,
    count: usize,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let target     = CGPoint::new(screen_x, screen_y);
    let off_screen = CGPoint::new(-1.0, -1.0);
    let win_local  = (win_local_x, win_local_y);
    let off_local  = (-1.0_f64, -1.0_f64);
    let flags      = parse_modifier_flags(modifiers);
    let click_pairs = count.max(1).min(2);
    let window_id  = wid as i64;

    // All 5 events share the same click-group ID so WindowServer / Chromium
    // treat the sequence as one gesture (Swift: field 58).
    let click_group_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as i64;

    // Stamp required fields onto a CGEvent.  All captured values are Copy so
    // this closure is Fn (callable multiple times).
    let stamp = |event: &CGEvent, local: (f64, f64), click_state: i64, phase: i64| {
        let ptr = event.as_ptr() as *mut std::ffi::c_void;
        let set = |f: u32, v: i64| { crate::input::skylight::set_integer_field(ptr, f, v); };
        set(0,  phase);              // kCGMouseEventNumber (gesture phase)
        set(1,  click_state);        // kCGMouseEventClickState
        set(3,  0);                  // kCGMouseEventButtonNumber (left)
        set(7,  3);                  // kCGMouseEventSubtype (NSEventSubtypeTouch)
        set(40, pid as i64);         // Chromium synthetic-event filter
        if window_id != 0 {
            set(51, window_id);      // windowNumber (NSEvent bridge equivalent)
            set(91, window_id);      // kCGMouseEventWindowUnderMousePointer
            set(92, window_id);      // kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent
        }
        set(58, click_group_id);     // click-group ID (gesture coalescing)
        crate::input::skylight::set_window_location(ptr, local.0, local.1);
        if flags != CGEventFlags::CGEventFlagNull {
            event.set_flags(flags);
        }
    };

    // Belt+suspenders: SkyLight path for Chromium/Catalyst + public API for AppKit.
    let post = |event: &CGEvent| {
        let ptr = event.as_ptr() as *mut std::ffi::c_void;
        crate::input::skylight::post_to_pid(pid as libc::pid_t, ptr, false);
        event.post_to_pid(pid as libc::pid_t);
    };

    // Step 1: mouseMoved at target (phase=2, clickState=0).
    let move_event = CGEvent::new_mouse_event(
        source.clone(), CGEventType::MouseMoved, target, CGMouseButton::Left,
    ).map_err(|_| anyhow::anyhow!("mouseMoved event creation failed"))?;
    stamp(&move_event, win_local, 0, 2);
    post(&move_event);
    std::thread::sleep(std::time::Duration::from_millis(15));

    // Step 2: off-screen primer click — opens Chromium user-activation gate
    // at an off-screen coordinate that can't hit any DOM element.
    let primer_down = CGEvent::new_mouse_event(
        source.clone(), CGEventType::LeftMouseDown, off_screen, CGMouseButton::Left,
    ).map_err(|_| anyhow::anyhow!("primer down event creation failed"))?;
    stamp(&primer_down, off_local, 1, 1);
    post(&primer_down);
    std::thread::sleep(std::time::Duration::from_millis(1));

    let primer_up = CGEvent::new_mouse_event(
        source.clone(), CGEventType::LeftMouseUp, off_screen, CGMouseButton::Left,
    ).map_err(|_| anyhow::anyhow!("primer up event creation failed"))?;
    stamp(&primer_up, off_local, 1, 2);
    post(&primer_up);
    // ≥1 frame so Chromium sees primer + target as separate gestures, not run-on.
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Step 3: target click pair(s) with clickState stepped 1→N for double-click
    // coalescing (Chromium renderer coalesces pairs into dblclick when state=1→2).
    for pair_index in 1..=click_pairs {
        let click_state = pair_index as i64;

        let down = CGEvent::new_mouse_event(
            source.clone(), CGEventType::LeftMouseDown, target, CGMouseButton::Left,
        ).map_err(|_| anyhow::anyhow!("target down event creation failed"))?;
        stamp(&down, win_local, click_state, 3);
        post(&down);
        std::thread::sleep(std::time::Duration::from_millis(1));

        let up = CGEvent::new_mouse_event(
            source.clone(), CGEventType::LeftMouseUp, target, CGMouseButton::Left,
        ).map_err(|_| anyhow::anyhow!("target up event creation failed"))?;
        stamp(&up, win_local, click_state, 3);
        post(&up);

        if pair_index < click_pairs {
            // ~80 ms between pairs — under the system double-click threshold,
            // clear of coalescing back into pair N.
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }

    Ok(())
}

/// Press-drag-release gesture from `(from_x, from_y)` to `(to_x, to_y)` in
/// screen coordinates, posted to `pid`.
///
/// `duration_ms` is the wall-clock budget for the drag path; `steps` is the
/// number of intermediate `leftMouseDragged` events linearly interpolated
/// along the path. `modifiers` are held across the entire gesture.
///
/// Like the Swift reference `MouseInput.drag`, uses the SkyLight path for
/// backgrounded-target delivery.
pub fn drag_at_xy(
    pid: i32,
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    from_local: Option<(f64, f64)>,
    to_local: Option<(f64, f64)>,
    wid: Option<u32>,
    duration_ms: u64,
    steps: usize,
    modifiers: &[&str],
    button: DragButton,
) -> anyhow::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let flags = parse_modifier_flags(modifiers);

    let (cg_button, down_type, dragged_type, up_type) = match button {
        DragButton::Left   => (CGMouseButton::Left,   CGEventType::LeftMouseDown,
                               CGEventType::LeftMouseDragged, CGEventType::LeftMouseUp),
        DragButton::Right  => (CGMouseButton::Right,  CGEventType::RightMouseDown,
                               CGEventType::RightMouseDragged, CGEventType::RightMouseUp),
        DragButton::Middle => (CGMouseButton::Center, CGEventType::OtherMouseDown,
                               CGEventType::OtherMouseDragged, CGEventType::OtherMouseUp),
    };

    let click_group_id: Option<i64> = wid.map(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as i64
    });

    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 { duration_ms / steps as u64 } else { duration_ms };

    // MouseDown at start.
    let from_pt = CGPoint::new(from_x, from_y);
    let down = CGEvent::new_mouse_event(source.clone(), down_type, from_pt, cg_button)
        .map_err(|_| anyhow::anyhow!("drag mouseDown failed"))?;
    if flags != CGEventFlags::CGEventFlagNull { down.set_flags(flags); }
    post_mouse_event(pid, &down, from_local, wid, click_group_id, 1);
    std::thread::sleep(std::time::Duration::from_millis(16));

    // Interpolated drag steps.
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + (to_x - from_x) * t;
        let iy = from_y + (to_y - from_y) * t;
        let il = from_local.zip(to_local).map(|((fx, fy), (tx, ty))| {
            (fx + (tx - fx) * t, fy + (ty - fy) * t)
        });
        let drag_pt = CGPoint::new(ix, iy);
        let drag = CGEvent::new_mouse_event(source.clone(), dragged_type, drag_pt, cg_button)
            .map_err(|_| anyhow::anyhow!("drag mouseDragged failed"))?;
        if flags != CGEventFlags::CGEventFlagNull { drag.set_flags(flags); }
        post_mouse_event(pid, &drag, il, wid, click_group_id, 1);
        if step_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(step_delay_ms));
        }
    }

    // MouseUp at end.
    let to_pt = CGPoint::new(to_x, to_y);
    let up = CGEvent::new_mouse_event(source.clone(), up_type, to_pt, cg_button)
        .map_err(|_| anyhow::anyhow!("drag mouseUp failed"))?;
    if flags != CGEventFlags::CGEventFlagNull { up.set_flags(flags); }
    post_mouse_event(pid, &up, to_local, wid, click_group_id, 1);

    Ok(())
}

/// Mouse button for drag gestures.
#[derive(Clone, Copy, Debug)]
pub enum DragButton {
    Left,
    Right,
    Middle,
}

/// Right-click at `(x, y)` with optional modifier keys.
pub fn right_click_at_xy(pid: i32, x: f64, y: f64, modifiers: &[&str]) -> anyhow::Result<()> {
    right_click_at_xy_inner(pid, x, y, None, modifiers)
}

/// Like `right_click_at_xy` but also stamps `CGEventSetWindowLocation` with
/// the window-local `(wx, wy)` point for better backgrounded-target delivery.
pub fn right_click_at_xy_with_window_local(
    pid: i32,
    x: f64, y: f64,
    wx: f64, wy: f64,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    right_click_at_xy_inner(pid, x, y, Some((wx, wy)), modifiers)
}

fn right_click_at_xy_inner(
    pid: i32,
    x: f64, y: f64,
    window_local: Option<(f64, f64)>,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let point = CGPoint::new(x, y);
    let flags = parse_modifier_flags(modifiers);

    let down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::RightMouseDown,
        point,
        CGMouseButton::Right,
    ).map_err(|_| anyhow::anyhow!("right mouse down failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        down.set_flags(flags);
    }
    post_mouse_event(pid, &down, window_local, None, None, 1);
    std::thread::sleep(std::time::Duration::from_millis(16));

    let up = CGEvent::new_mouse_event(
        source,
        CGEventType::RightMouseUp,
        point,
        CGMouseButton::Right,
    ).map_err(|_| anyhow::anyhow!("right mouse up failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        up.set_flags(flags);
    }
    post_mouse_event(pid, &up, window_local, None, None, 1);

    Ok(())
}

/// Post a mouse event to `pid`.
///
/// Matches Swift's `MouseInput.postBoth(_:toPid:)`:
/// - Fires `SLEventPostToPid` (SkyLight path — reaches backgrounded Chromium/Catalyst).
/// - Also fires `CGEvent::post_to_pid` (public path — lands on AppKit targets where
///   SkyLight mouse delivery drops).
/// Both are always posted in sequence regardless of whether the other succeeded.
///
/// Field stamps applied (always):
/// - f40 = `pid`  (Chromium's synthetic-event filter)
/// - `CGEventSetWindowLocation` = window-local point (if provided)
///
/// Additional stamps when `wid` is provided (Chromium window-routing fields):
/// - f1  = `click_state`  (kCGMouseEventClickState)
/// - f3  = 0              (kCGMouseEventButtonNumber = left)
/// - f7  = 3              (kCGMouseEventSubtype = NSEventSubtypeTouch)
/// - f51 = window_id      (windowNumber, NSEvent bridge equivalent)
/// - f58 = click_group_id (gesture coalescing across pairs)
/// - f91 = window_id      (kCGMouseEventWindowUnderMousePointer)
/// - f92 = window_id      (kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent)
fn post_mouse_event(
    pid: i32,
    event: &CGEvent,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    click_group_id: Option<i64>,
    click_state: i64,
) {
    let event_ptr = event.as_ptr() as *mut std::ffi::c_void;

    // Stamp window-local point for backgrounded window targeting.
    if let Some((wx, wy)) = window_local {
        crate::input::skylight::set_window_location(event_ptr, wx, wy);
    }

    // Chromium window-routing fields — stamp when window_id is known.
    if let (Some(wid), Some(cgid)) = (wid, click_group_id) {
        let window_id = wid as i64;
        let set = |f: u32, v: i64| { crate::input::skylight::set_integer_field(event_ptr, f, v); };
        set(1,  click_state);  // kCGMouseEventClickState
        set(3,  0);            // kCGMouseEventButtonNumber (left)
        set(7,  3);            // kCGMouseEventSubtype (NSEventSubtypeTouch)
        set(51, window_id);    // windowNumber
        set(58, cgid);         // click-group ID (gesture coalescing)
        set(91, window_id);    // kCGMouseEventWindowUnderMousePointer
        set(92, window_id);    // kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent
    }

    // Always stamp f40 = target pid (Chromium synthetic-event filter).
    crate::input::skylight::set_integer_field(event_ptr, 40, pid as i64);

    // SkyLight path: activity-monitor tickle → reaches Catalyst/Chromium.
    // Mouse events skip the auth-message envelope (Swift: attachAuthMessage: false).
    crate::input::skylight::post_to_pid(pid as libc::pid_t, event_ptr, false);

    // Public path: delivers to AppKit targets where SkyLight mouse drops.
    // Belt+suspenders — both fire unconditionally (matches Swift `postBoth`).
    event.post_to_pid(pid as libc::pid_t);
}

fn parse_modifier_flags(modifiers: &[&str]) -> CGEventFlags {
    let mut flags = CGEventFlags::CGEventFlagNull;
    for m in modifiers {
        match m.to_lowercase().as_str() {
            "cmd" | "command" => flags |= CGEventFlags::CGEventFlagCommand,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "option" | "alt" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            _ => {}
        }
    }
    flags
}
