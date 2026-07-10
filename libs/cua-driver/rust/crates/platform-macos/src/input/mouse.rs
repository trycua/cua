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
pub fn click_at_xy(
    pid: i32,
    x: f64,
    y: f64,
    count: usize,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    click_at_xy_inner(pid, x, y, None, None, count, modifiers)
}

/// Screen-absolute click posted to the GLOBAL HID tap (`CGEventTapLocation::HID`),
/// NOT routed to any pid — the OS delivers it to whichever window owns the
/// screen point, the macOS analogue of Windows' `WindowFromPoint` + `SendInput`
/// desktop-scope click. This backs the `capture_scope="desktop"`, window-less
/// (no pid/window_id) branch of the `click` tool: the agent has located the
/// target by vision in `get_desktop_state` and clicks true screen pixels.
///
/// Unlike the pid-routed `click_at_xy`, this honors the real foreground/Z order
/// (it lands on whatever is visually on top at the point) — exactly the
/// foreground, vision-driven model that complements the background contract.
pub fn click_at_xy_desktop(x: f64, y: f64, count: usize, button: &str) -> anyhow::Result<()> {
    use core_graphics::display::CGDisplay;
    use core_graphics::event::CGEventTapLocation;
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let point = CGPoint::new(x, y);
    let (down_ty, up_ty, btn) = match button {
        "right" => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        "middle" => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        ),
        _ => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
    };
    // Warp the REAL cursor to the point first (the macOS peer of Linux XTest's
    // pointer warp). A synthetic MouseMoved event does not relocate the hardware
    // cursor, and AppKit hit-tests some clicks against the actual cursor
    // position, so without the warp the down/up can miss the target. Desktop
    // scope is the foreground modality, so moving the visible cursor is expected.
    let _ = CGDisplay::warp_mouse_cursor_position(point);
    // Re-couple cursor + mouse-delta so the synthesized click hit-tests at the
    // warped point, not the pre-warp one.
    unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
    std::thread::sleep(std::time::Duration::from_millis(40));
    for _ in 0..count.max(1) {
        let down = CGEvent::new_mouse_event(source.clone(), down_ty, point, btn)
            .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(down) failed"))?;
        down.post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let up = CGEvent::new_mouse_event(source.clone(), up_ty, point, btn)
            .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(up) failed"))?;
        up.post(CGEventTapLocation::HID);
    }
    Ok(())
}

extern "C" {
    /// Reconnect the mouse-delta stream to the (just-warped) cursor position so a
    /// synthesized click hit-tests at the new location, not the pre-warp one.
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
}

/// Like `click_at_xy` but also stamps window-local `(wx, wy)` onto the event.
/// When `wid` is provided, additionally stamps Chromium routing fields
/// (f51 / f58 / f91 / f92) for better backgrounded-target delivery.
pub fn click_at_xy_with_window_local(
    pid: i32,
    x: f64,
    y: f64,
    wx: f64,
    wy: f64,
    wid: u32,
    count: usize,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    click_at_xy_inner(pid, x, y, Some((wx, wy)), Some(wid), count, modifiers)
}

fn click_at_xy_inner(
    pid: i32,
    x: f64,
    y: f64,
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

    // Prime the target window's cursor-tracking state with a leading mouseMoved
    // so an AppKit NSButton / NSView hit-tests the down at the right point
    // (Swift recipe Step 3). Without it the synthetic mouseDown on a
    // backgrounded AppKit control is silently ignored.
    post_mouse_moved_primer(pid, &source, point, window_local, wid, click_group_id);
    std::thread::sleep(std::time::Duration::from_millis(12));

    for pair_index in 0..count {
        let click_state = (pair_index + 1) as i64;

        let down = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseDown,
            point,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(down) failed"))?;
        if flags != CGEventFlags::CGEventFlagNull {
            down.set_flags(flags);
        }

        post_mouse_event(
            pid,
            &down,
            window_local,
            wid,
            click_group_id,
            click_state,
            0,
            3,
        );
        // 28 ms down→up gap: an NSButton's mouseDown enters a modal tracking
        // loop that polls for the matching mouseUp; too tight a gap can race the
        // loop's first poll and the click is dropped. 16 ms was under that
        // threshold for some controls.
        std::thread::sleep(std::time::Duration::from_millis(28));

        let up = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseUp,
            point,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(up) failed"))?;
        if flags != CGEventFlags::CGEventFlagNull {
            up.set_flags(flags);
        }

        post_mouse_event(
            pid,
            &up,
            window_local,
            wid,
            click_group_id,
            click_state,
            0,
            3,
        );

        if count > 1 {
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }
    Ok(())
}

/// Full Chromium-compatible left-click recipe matching Swift's `clickViaAuthSignedPost`.
///
/// The focus-without-raise prologue makes the target window key without changing
/// its z-order, which Chromium requires before it accepts a background pixel
/// mouseDown. The cursor overlay is re-pinned by the click tool after dispatch.
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

    // Chromium's first-mouse handling rejects a background click delivered to
    // a non-key window. This SkyLight focus record keys the requested window
    // without raising it or moving the user's cursor.
    if crate::input::skylight::activate_without_raise(pid as libc::pid_t, wid) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let target = CGPoint::new(screen_x, screen_y);
    let off_screen = CGPoint::new(-1.0, -1.0);
    let win_local = (win_local_x, win_local_y);
    let off_local = (-1.0_f64, -1.0_f64);
    let flags = parse_modifier_flags(modifiers);
    let click_pairs = count.max(1).min(2);
    let window_id = wid as i64;

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
        let set = |f: u32, v: i64| {
            crate::input::skylight::set_integer_field(ptr, f, v);
        };
        set(0, phase); // kCGMouseEventNumber (gesture phase)
        set(1, click_state); // kCGMouseEventClickState
        set(3, 0); // kCGMouseEventButtonNumber (left)
        set(7, 3); // kCGMouseEventSubtype (NSEventSubtypeTouch)
        set(40, pid as i64); // Chromium synthetic-event filter
        if window_id != 0 {
            set(51, window_id); // windowNumber (NSEvent bridge equivalent)
            set(91, window_id); // kCGMouseEventWindowUnderMousePointer
            set(92, window_id); // kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent
        }
        set(58, click_group_id); // click-group ID (gesture coalescing)
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
        source.clone(),
        CGEventType::MouseMoved,
        target,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("mouseMoved event creation failed"))?;
    stamp(&move_event, win_local, 0, 2);
    post(&move_event);
    std::thread::sleep(std::time::Duration::from_millis(15));

    // Step 2: off-screen primer click — opens Chromium user-activation gate
    // at an off-screen coordinate that can't hit any DOM element.
    let primer_down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        off_screen,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("primer down event creation failed"))?;
    stamp(&primer_down, off_local, 1, 1);
    post(&primer_down);
    std::thread::sleep(std::time::Duration::from_millis(1));

    let primer_up = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseUp,
        off_screen,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("primer up event creation failed"))?;
    stamp(&primer_up, off_local, 1, 2);
    post(&primer_up);
    // ≥1 frame so Chromium sees primer + target as separate gestures, not run-on.
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Step 3: target click pair(s) with clickState stepped 1→N for double-click
    // coalescing (Chromium renderer coalesces pairs into dblclick when state=1→2).
    for pair_index in 1..=click_pairs {
        let click_state = pair_index as i64;

        let down = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseDown,
            target,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("target down event creation failed"))?;
        stamp(&down, win_local, click_state, 3);
        post(&down);
        std::thread::sleep(std::time::Duration::from_millis(1));

        let up = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseUp,
            target,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("target up event creation failed"))?;
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
    foreground_release: bool,
) -> anyhow::Result<()> {
    use core_graphics::event::CGEventTapLocation;
    use std::time::{SystemTime, UNIX_EPOCH};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let flags = parse_modifier_flags(modifiers);

    let (cg_button, down_type, dragged_type, up_type) = match button {
        DragButton::Left => (
            CGMouseButton::Left,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ),
        DragButton::Right => (
            CGMouseButton::Right,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseDragged,
            CGEventType::RightMouseUp,
        ),
        DragButton::Middle => (
            CGMouseButton::Center,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseDragged,
            CGEventType::OtherMouseUp,
        ),
    };
    // f3 button number must match the dragged button (0=left, 1=right, 2=middle).
    let button_number: i64 = match button {
        DragButton::Left => 0,
        DragButton::Right => 1,
        DragButton::Middle => 2,
    };

    let click_group_id: Option<i64> = wid.map(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as i64
    });

    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 {
        duration_ms / steps as u64
    } else {
        duration_ms
    };

    // MouseDown at start.
    let from_pt = CGPoint::new(from_x, from_y);
    let down = CGEvent::new_mouse_event(source.clone(), down_type, from_pt, cg_button)
        .map_err(|_| anyhow::anyhow!("drag mouseDown failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        down.set_flags(flags);
    }
    post_mouse_event(
        pid,
        &down,
        from_local,
        wid,
        click_group_id,
        1,
        button_number,
        0,
    );
    std::thread::sleep(std::time::Duration::from_millis(16));

    // Interpolated drag steps.
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + (to_x - from_x) * t;
        let iy = from_y + (to_y - from_y) * t;
        let il = from_local
            .zip(to_local)
            .map(|((fx, fy), (tx, ty))| (fx + (tx - fx) * t, fy + (ty - fy) * t));
        let drag_pt = CGPoint::new(ix, iy);
        let drag = CGEvent::new_mouse_event(source.clone(), dragged_type, drag_pt, cg_button)
            .map_err(|_| anyhow::anyhow!("drag mouseDragged failed"))?;
        if flags != CGEventFlags::CGEventFlagNull {
            drag.set_flags(flags);
        }
        post_mouse_event(pid, &drag, il, wid, click_group_id, 1, button_number, 0);
        if step_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(step_delay_ms));
        }
    }

    // MouseUp at end.
    // Give Chromium one run-loop turn to process the final dragged event at
    // the drop point before releasing pointer capture.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let to_pt = CGPoint::new(to_x, to_y);
    let up = CGEvent::new_mouse_event(source.clone(), up_type, to_pt, cg_button)
        .map_err(|_| anyhow::anyhow!("drag mouseUp failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        up.set_flags(flags);
    }
    post_mouse_event(pid, &up, to_local, wid, click_group_id, 1, button_number, 0);
    if foreground_release {
        // A frontmost Chromium surface can consume PID-routed down/move
        // events yet filter the synthetic release. Re-post only the release
        // through the HID tap while the foreground assist still holds focus.
        up.post(CGEventTapLocation::HID);
    }
    // Chromium may process the final pointerup on the next run-loop turn. In
    // the foreground rung the caller restores the previous app immediately
    // after this function returns, so let the target consume the release and
    // complete pointer capture before that restore.
    std::thread::sleep(std::time::Duration::from_millis(100));

    Ok(())
}

/// Foreground drag through the global HID event tap.
///
/// A frontmost Chromium/WebKit surface expects a real HID-origin gesture for
/// pointer capture and drag tracking. PID-routed `post_to_pid` events are
/// suitable for background delivery, but they can be silently filtered by the
/// renderer even when the target is frontmost.
pub fn drag_at_xy_foreground(
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    duration_ms: u64,
    steps: usize,
    modifiers: &[&str],
    button: DragButton,
) -> anyhow::Result<()> {
    use core_graphics::display::CGDisplay;
    use core_graphics::event::CGEventTapLocation;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let flags = parse_modifier_flags(modifiers);
    let (cg_button, down_type, dragged_type, up_type) = match button {
        DragButton::Left => (
            CGMouseButton::Left,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ),
        DragButton::Right => (
            CGMouseButton::Right,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseDragged,
            CGEventType::RightMouseUp,
        ),
        DragButton::Middle => (
            CGMouseButton::Center,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseDragged,
            CGEventType::OtherMouseUp,
        ),
    };
    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 {
        duration_ms / steps as u64
    } else {
        duration_ms
    };

    let post = |event: &CGEvent| event.post(CGEventTapLocation::HID);

    // Keep WindowServer's hardware cursor and event stream coupled. AppKit
    // hit-tests some pointer-capture surfaces against the actual cursor even
    // when the HID event carries an explicit location.
    let _ = CGDisplay::warp_mouse_cursor_position(CGPoint::new(from_x, from_y));
    unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
    std::thread::sleep(std::time::Duration::from_millis(40));

    // Prime the renderer's tracking state with a genuine HID mouse move.
    if let Ok(move_event) = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::MouseMoved,
        CGPoint::new(from_x, from_y),
        cg_button,
    ) {
        post(&move_event);
    }
    std::thread::sleep(std::time::Duration::from_millis(30));

    let down = CGEvent::new_mouse_event(
        source.clone(),
        down_type,
        CGPoint::new(from_x, from_y),
        cg_button,
    )
    .map_err(|_| anyhow::anyhow!("foreground drag mouseDown failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        down.set_flags(flags);
    }
    down.set_integer_value_field(core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE, 1);
    down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(16));

    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let event = CGEvent::new_mouse_event(
            source.clone(),
            dragged_type,
            CGPoint::new(from_x + (to_x - from_x) * t, from_y + (to_y - from_y) * t),
            cg_button,
        )
        .map_err(|_| anyhow::anyhow!("foreground drag mouseDragged failed"))?;
        if flags != CGEventFlags::CGEventFlagNull {
            event.set_flags(flags);
        }
        event.set_integer_value_field(core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE, 1);
        post(&event);
        if step_delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(step_delay_ms));
        }
    }

    let up = CGEvent::new_mouse_event(source, up_type, CGPoint::new(to_x, to_y), cg_button)
        .map_err(|_| anyhow::anyhow!("foreground drag mouseUp failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        up.set_flags(flags);
    }
    up.set_integer_value_field(core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE, 1);
    post(&up);
    // The foreground wrapper restores the previous app immediately after this
    // function returns. Let the target's run loop consume the queued HID
    // gesture, including pointer-capture release, before that restore happens.
    std::thread::sleep(std::time::Duration::from_millis(100));
    Ok(())
}

/// Mouse button for drag gestures.
#[derive(Clone, Copy, Debug)]
pub enum DragButton {
    Left,
    Right,
    Middle,
}

/// Middle-click at `(x, y)` with optional modifier keys.
///
/// Posts an `OtherMouseDown` / `OtherMouseUp` pair with `CGMouseButton::Center`
/// to the target pid through the same SkyLight + public-API postBoth path the
/// left- and right-click primitives use. Window-local stamping mirrors
/// `right_click_at_xy_with_window_local`.
pub fn middle_click_at_xy(pid: i32, x: f64, y: f64, modifiers: &[&str]) -> anyhow::Result<()> {
    middle_click_at_xy_inner(pid, x, y, None, modifiers)
}

/// Like `middle_click_at_xy` but stamps the window-local `(wx, wy)` point.
pub fn middle_click_at_xy_with_window_local(
    pid: i32,
    x: f64,
    y: f64,
    wx: f64,
    wy: f64,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    middle_click_at_xy_inner(pid, x, y, Some((wx, wy)), modifiers)
}

fn middle_click_at_xy_inner(
    pid: i32,
    x: f64,
    y: f64,
    window_local: Option<(f64, f64)>,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let point = CGPoint::new(x, y);
    let flags = parse_modifier_flags(modifiers);

    let down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::OtherMouseDown,
        point,
        CGMouseButton::Center,
    )
    .map_err(|_| anyhow::anyhow!("middle mouse down failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        down.set_flags(flags);
    }
    post_mouse_event(pid, &down, window_local, None, None, 1, 2, 3);
    std::thread::sleep(std::time::Duration::from_millis(16));

    let up = CGEvent::new_mouse_event(
        source,
        CGEventType::OtherMouseUp,
        point,
        CGMouseButton::Center,
    )
    .map_err(|_| anyhow::anyhow!("middle mouse up failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        up.set_flags(flags);
    }
    post_mouse_event(pid, &up, window_local, None, None, 1, 2, 3);

    Ok(())
}

/// Right-click at `(x, y)` with optional modifier keys (no window routing).
pub fn right_click_at_xy(pid: i32, x: f64, y: f64, modifiers: &[&str]) -> anyhow::Result<()> {
    right_click_at_xy_inner(pid, x, y, None, None, modifiers)
}

/// Like `right_click_at_xy` but stamps `CGEventSetWindowLocation` with the
/// window-local `(wx, wy)` point AND the window-routing fields (f51/f91/f92) so
/// the `rightMouseDown` reaches a backgrounded `NSView`.
///
/// `wid` is required for the routing fields: without a window number stamped,
/// WindowServer falls back to a screen-location hit-test that skips non-key
/// (backgrounded) windows, so the right-down never reached the NSView — the
/// reported "right-click does not fire rightMouseDown" bug. The left-click path
/// already threaded `wid`; right-click did not, which is why it broke.
pub fn right_click_at_xy_with_window_local(
    pid: i32,
    x: f64,
    y: f64,
    wx: f64,
    wy: f64,
    wid: u32,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    right_click_at_xy_inner(pid, x, y, Some((wx, wy)), Some(wid), modifiers)
}

fn right_click_at_xy_inner(
    pid: i32,
    x: f64,
    y: f64,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let point = CGPoint::new(x, y);
    let flags = parse_modifier_flags(modifiers);

    let click_group_id: Option<i64> = wid.map(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as i64
    });

    // Prime cursor-tracking state at the target so AppKit hit-tests the
    // right-down at the right NSView (same rationale as the left path).
    post_mouse_moved_primer(pid, &source, point, window_local, wid, click_group_id);
    std::thread::sleep(std::time::Duration::from_millis(12));

    let down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::RightMouseDown,
        point,
        CGMouseButton::Right,
    )
    .map_err(|_| anyhow::anyhow!("right mouse down failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        down.set_flags(flags);
    }
    // button_number = 1 (right). Stamping 0 here routes the event as a left
    // button-number on the receiving side even though the type is rightMouseDown.
    post_mouse_event(pid, &down, window_local, wid, click_group_id, 1, 1, 3);
    std::thread::sleep(std::time::Duration::from_millis(28));

    let up = CGEvent::new_mouse_event(
        source,
        CGEventType::RightMouseUp,
        point,
        CGMouseButton::Right,
    )
    .map_err(|_| anyhow::anyhow!("right mouse up failed"))?;
    if flags != CGEventFlags::CGEventFlagNull {
        up.set_flags(flags);
    }
    post_mouse_event(pid, &up, window_local, wid, click_group_id, 1, 1, 3);

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
/// - f1  = `click_state`    (kCGMouseEventClickState)
/// - f3  = `button_number`  (kCGMouseEventButtonNumber: 0=left, 1=right, 2=middle)
/// - f7  = 3                (kCGMouseEventSubtype = NSEventSubtypeTouch)
/// - f51 = window_id        (windowNumber, NSEvent bridge equivalent)
/// - f58 = click_group_id   (gesture coalescing across pairs)
/// - f91 = window_id        (kCGMouseEventWindowUnderMousePointer)
/// - f92 = window_id        (kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent)
///
/// `button_number` MUST match the button encoded in the event type (right-down
/// stamped with f3=0 routes as a left-click on the receiving side — this was the
/// right-click-lands-as-nothing bug). Left=0, Right=1, Middle=2.
fn post_mouse_event(
    pid: i32,
    event: &CGEvent,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    click_group_id: Option<i64>,
    click_state: i64,
    button_number: i64,
    subtype: i64,
) {
    let event_ptr = event.as_ptr() as *mut std::ffi::c_void;

    // Stamp window-local point for backgrounded window targeting.
    if let Some((wx, wy)) = window_local {
        crate::input::skylight::set_window_location(event_ptr, wx, wy);
    }

    // Chromium / AppKit window-routing fields — stamp when window_id is known.
    if let (Some(wid), Some(cgid)) = (wid, click_group_id) {
        let window_id = wid as i64;
        let set = |f: u32, v: i64| {
            crate::input::skylight::set_integer_field(event_ptr, f, v);
        };
        set(1, click_state); // kCGMouseEventClickState
        set(3, button_number); // kCGMouseEventButtonNumber (0=left, 1=right, 2=middle)
        set(7, subtype); // kCGMouseEventSubtype (touch for clicks, normal for drags)
        set(51, window_id); // windowNumber
        set(58, cgid); // click-group ID (gesture coalescing)
        set(91, window_id); // kCGMouseEventWindowUnderMousePointer
        set(92, window_id); // kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent
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

/// Post a stamped `mouseMoved` to `pid` at `point` before a down/up pair.
///
/// AppKit hit-tests a button/`NSView` against the cursor-tracking state the
/// window last saw; a backgrounded window that never received a move event has
/// stale tracking state, so the synthetic `mouseDown` lands "outside" the
/// control and `-mouseDown:` never fires (the synthetic-NSButton-ignored bug).
/// A leading `mouseMoved` at the target primes that state — this is exactly the
/// Step-3 `mouseMoved` of the Swift `clickViaAuthSignedPost` recipe, which the
/// default Rust pixel path had dropped. Click-state 0 / no button (move events
/// carry no button); window-routing fields still stamped so the move reaches the
/// right backgrounded window.
fn post_mouse_moved_primer(
    pid: i32,
    source: &CGEventSource,
    point: CGPoint,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    click_group_id: Option<i64>,
) {
    if let Ok(mv) = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::MouseMoved,
        point,
        CGMouseButton::Left,
    ) {
        post_mouse_event(pid, &mv, window_local, wid, click_group_id, 0, 0, 3);
    }
}

/// Synthesize a **pixel-unit mouse-wheel** scroll at `(screen_x, screen_y)`,
/// posted to `pid`.
///
/// This is the pixel-wheel scroll path. Unlike the keystroke path (PageDown /
/// arrow keys), which only ever drives the *focused* / page scroller, a real
/// wheel event is hit-tested by the renderer at the cursor point: whatever
/// element sits under `(screen_x, screen_y)` receives the scroll. That is the
/// only way to scroll a nested `overflow:auto` div that has no `tabindex` and
/// therefore can never take keyboard focus (verified no-op via keystrokes on
/// WKWebView's inner `scroll-tall` and WebView2).
///
/// `CGEventCreateScrollWheelEvent2` builds the event in `kCGScrollEventUnitPixel`
/// units; we then anchor it at the target point with `CGEventSetLocation` so the
/// renderer routes it correctly, and stamp the same background-delivery fields
/// the click primitives use (window-local point + f40 pid filter + window-routing
/// fields) so it reaches backgrounded Chromium/Catalyst/WKWebView targets.
///
/// Sign convention (macOS): a POSITIVE `delta_y_per_tick` scrolls the content
/// toward the top (reveals content ABOVE); NEGATIVE reveals content BELOW.
/// POSITIVE `delta_x_per_tick` reveals content to the LEFT; NEGATIVE to the
/// RIGHT. The direction→delta mapping lives in the `scroll` tool; this primitive
/// stays sign-agnostic (if a live target scrolls inverted, flip there).
///
/// `ticks` discrete wheel events are posted (one per notch), mirroring the
/// keystroke path's `amount` repetitions, each separated by a short gap so the
/// renderer animates per-notch instead of coalescing into a single jump.
///
/// `window_local`/`wid`: when known, stamp the window-local point and the
/// Chromium window-routing fields (f51/f91/f92) for backgrounded delivery —
/// identical in spirit to `post_mouse_event`.
pub fn scroll_wheel_at_xy(
    pid: i32,
    screen_x: f64,
    screen_y: f64,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    delta_y_per_tick: i32,
    delta_x_per_tick: i32,
    ticks: usize,
) -> anyhow::Result<()> {
    use core_graphics::event::ScrollEventUnit;

    for _ in 0..ticks.max(1) {
        // Fresh source per event, matching the click primitives.
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
        // wheel_count = 2 → both axes carried (vertical = wheel1/axis-1,
        // horizontal = wheel2/axis-2). PIXEL units so the deltas are device
        // pixels, matching a trackpad/precise wheel rather than coarse notches.
        let event = CGEvent::new_scroll_event(
            source,
            ScrollEventUnit::PIXEL,
            2,
            delta_y_per_tick,
            delta_x_per_tick,
            0,
        )
        .map_err(|_| anyhow::anyhow!("CGEvent::new_scroll_event failed"))?;

        let event_ptr = event.as_ptr() as *mut std::ffi::c_void;

        // Anchor the event at the target screen point so the renderer's wheel
        // hit-test routes the scroll to the element under the cursor.
        unsafe { CGEventSetLocation(event_ptr, screen_x, screen_y) };

        // Background-delivery stamps (mirror post_mouse_event).
        if let Some((wx, wy)) = window_local {
            crate::input::skylight::set_window_location(event_ptr, wx, wy);
        }
        if let Some(wid) = wid {
            let window_id = wid as i64;
            let set = |f: u32, v: i64| {
                crate::input::skylight::set_integer_field(event_ptr, f, v);
            };
            set(51, window_id); // windowNumber
            set(91, window_id); // kCGMouseEventWindowUnderMousePointer
            set(92, window_id); // ...ThatCanHandleThisEvent
        }
        // f40 = target pid (Chromium synthetic-event filter).
        crate::input::skylight::set_integer_field(event_ptr, 40, pid as i64);

        // Belt+suspenders post: SkyLight reaches backgrounded Chromium/Catalyst;
        // the public path lands on AppKit/WKWebView. Mouse-class → no auth envelope.
        crate::input::skylight::post_to_pid(pid as libc::pid_t, event_ptr, false);
        event.post_to_pid(pid as libc::pid_t);

        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    Ok(())
}

extern "C" {
    /// `void CGEventSetLocation(CGEventRef event, CGPoint location)`.
    ///
    /// `CGPoint { double x, double y }` is classified as two FP eightbytes on
    /// arm64 / x86-64, so passing the two doubles as separate args is
    /// ABI-identical to passing the struct by value — the same trick the
    /// SkyLight bridge uses for `CGEventSetWindowLocation`.
    fn CGEventSetLocation(event: *mut std::ffi::c_void, x: f64, y: f64);
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
