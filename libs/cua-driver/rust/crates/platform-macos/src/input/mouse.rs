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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MousePostMode {
    /// Post the same event through SkyLight and again through the public API.
    /// An AppKit target receives both copies, so one logical event arrives
    /// twice. Kept only until the remaining callers move to
    /// [`MousePostMode::SkyLightPreferred`].
    Both,
    /// Post each event exactly once: through SkyLight `SLEventPostToPid`, or
    /// through the public `CGEvent::post_to_pid` only when the SPI is absent.
    SkyLightPreferred,
    PublicOnly,
    HidOnly,
}

/// The transport an event is handed to. [`post_route`] decides it from the
/// post mode, so the "how many times is this event delivered" question has one
/// answer that unit tests can check without a window server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PostRoute {
    SkyLight,
    PublicPid,
    Hid,
    /// Legacy duplicate delivery: SkyLight, then the public API.
    SkyLightAndPublic,
}

fn post_route(mode: MousePostMode, skylight_available: bool) -> PostRoute {
    match mode {
        MousePostMode::Both => PostRoute::SkyLightAndPublic,
        MousePostMode::SkyLightPreferred if skylight_available => PostRoute::SkyLight,
        MousePostMode::SkyLightPreferred | MousePostMode::PublicOnly => PostRoute::PublicPid,
        MousePostMode::HidOnly => PostRoute::Hid,
    }
}

fn skylight_available() -> bool {
    #[cfg(test)]
    if let Some(available) = post_sink::skylight_override() {
        return available;
    }
    crate::input::skylight::is_available()
}

/// Hand one stamped event to its transport.
fn dispatch_mouse_event(route: PostRoute, pid: i32, event: &CGEvent) {
    #[cfg(test)]
    if post_sink::record(route, event) {
        return;
    }
    let event_ptr = event.as_ptr() as *mut std::ffi::c_void;
    match route {
        PostRoute::SkyLight => {
            crate::input::skylight::post_to_pid(pid as libc::pid_t, event_ptr, false);
        }
        PostRoute::PublicPid => event.post_to_pid(pid as libc::pid_t),
        PostRoute::Hid => event.post(core_graphics::event::CGEventTapLocation::HID),
        PostRoute::SkyLightAndPublic => {
            crate::input::skylight::post_to_pid(pid as libc::pid_t, event_ptr, false);
            event.post_to_pid(pid as libc::pid_t);
        }
    }
}

#[derive(Clone, Copy)]
pub enum WindowClickDelivery {
    Background,
    Foreground,
}

impl WindowClickDelivery {
    pub fn from_foreground(foreground: bool) -> Self {
        if foreground {
            Self::Foreground
        } else {
            Self::Background
        }
    }
}

/// Write exactly `flags` on a posted mouse event. An event made from the
/// HID-state source otherwise keeps whatever modifiers the HID state holds,
/// so a held or stuck Control would turn a background click into a
/// Control-click (a context menu instead of a click).
fn set_explicit_flags(event: &CGEvent, flags: CGEventFlags) {
    event.set_flags(flags);
}

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
    click_at_xy_inner(
        pid,
        (x, y),
        None,
        None,
        count,
        modifiers,
        MousePostMode::Both,
    )
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
    click_at_xy_desktop_inner(x, y, count, button, &[], false)
}

/// Global HID click with physical modifier transitions. The caller must guard
/// the exact foreground window because this transport has no pid addressing.
pub fn click_at_xy_desktop_with_modifiers(
    x: f64,
    y: f64,
    count: usize,
    button: &str,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    click_at_xy_desktop_inner(x, y, count, button, modifiers, false)
}

/// Global HID modifier click that restores the user's hardware cursor after
/// the event pair has been queued.
pub fn click_at_xy_desktop_with_modifiers_preserving_cursor(
    x: f64,
    y: f64,
    count: usize,
    button: &str,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    click_at_xy_desktop_inner(x, y, count, button, modifiers, true)
}

fn click_at_xy_desktop_inner(
    x: f64,
    y: f64,
    count: usize,
    button: &str,
    modifiers: &[&str],
    preserve_cursor: bool,
) -> anyhow::Result<()> {
    use core_graphics::display::CGDisplay;
    use core_graphics::event::CGEventTapLocation;
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let prior = if preserve_cursor {
        Some(
            CGEvent::new(source.clone())
                .map_err(|_| anyhow::anyhow!("CGEvent::new failed"))?
                .location(),
        )
    } else {
        None
    };
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
    let result = super::keyboard::with_global_modifier_keys(modifiers, |flags| {
        if !modifiers.is_empty() {
            // Prime AppKit's cursor-tracking state after the physical modifiers
            // are down so the modified mouseDown preserves the existing
            // selection. Leave ordinary clicks on their established event path.
            let moved = CGEvent::new_mouse_event(
                source.clone(),
                CGEventType::MouseMoved,
                point,
                CGMouseButton::Left,
            )
            .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(move) failed"))?;
            moved.set_flags(flags);
            moved.post(CGEventTapLocation::HID);
            std::thread::sleep(std::time::Duration::from_millis(12));
        }

        for pair_index in 0..count.max(1) {
            let down = CGEvent::new_mouse_event(source.clone(), down_ty, point, btn)
                .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(down) failed"))?;
            down.set_flags(flags);
            down.set_integer_value_field(
                core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
                (pair_index + 1) as i64,
            );
            down.post(CGEventTapLocation::HID);
            std::thread::sleep(std::time::Duration::from_millis(28));
            let up = CGEvent::new_mouse_event(source.clone(), up_ty, point, btn)
                .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(up) failed"))?;
            up.set_flags(flags);
            up.set_integer_value_field(
                core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
                (pair_index + 1) as i64,
            );
            up.post(CGEventTapLocation::HID);
            if count > 1 {
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
        }
        Ok(())
    });

    // Let AppKit consume the up event before the pointer is restored. The
    // exact foreground guard remains active around this entire helper.
    std::thread::sleep(std::time::Duration::from_millis(40));
    if let Some(prior) = prior {
        let _ = CGDisplay::warp_mouse_cursor_position(prior);
        unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
    }
    result
}

/// Move the real hardware cursor to a logical desktop point.
///
/// A warp alone posts no mouse event. AppKit then only re-evaluates tracking
/// areas when the pointer crosses into a different window, so a second warp
/// inside the same window never fires `mouseEntered`/`mouseMoved`, and
/// hover-revealed controls stay hidden. Follow the warp with a `MouseMoved`
/// at the same point through the HID tap, as a physical mouse would.
pub fn move_cursor_desktop(x: f64, y: f64) -> anyhow::Result<()> {
    use core_graphics::display::CGDisplay;
    let point = CGPoint::new(x, y);
    CGDisplay::warp_mouse_cursor_position(point)
        .map_err(|error| anyhow::anyhow!("CGWarpMouseCursorPosition failed: {error:?}"))?;
    unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
    // Let WindowServer settle the warp first: a move posted in the same
    // instant was occasionally dropped (1 of 5 in the VM check). A second
    // move a moment later is harmless when the first one landed.
    for _ in 0..2 {
        std::thread::sleep(std::time::Duration::from_millis(15));
        post_pointer_moved(point)?;
    }
    Ok(())
}

/// Post one `MouseMoved` at `point` through the global HID tap, so AppKit
/// re-evaluates tracking areas under the (already warped) pointer.
fn post_pointer_moved(point: CGPoint) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let moved =
        CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(move) failed"))?;
    dispatch_mouse_event(PostRoute::Hid, 0, &moved);
    Ok(())
}

/// Scroll the foreground desktop surface at a logical screen point through the
/// global HID queue. Mirrors computer-server's pynput wheel behavior while
/// preserving cua-driver's explicit direction/amount contract.
pub fn scroll_wheel_desktop(
    x: f64,
    y: f64,
    delta_y_per_tick: i32,
    delta_x_per_tick: i32,
    ticks: usize,
) -> anyhow::Result<()> {
    use core_graphics::event::{CGEventTapLocation, ScrollEventUnit};

    move_cursor_desktop(x, y)?;
    std::thread::sleep(std::time::Duration::from_millis(40));
    for _ in 0..ticks.max(1) {
        // AppKit does not reliably consume synthetic LINE-unit events posted
        // through the global HID queue. pynput's proven macOS desktop path uses
        // PIXEL units, a null source, and scales each logical wheel notch to ten
        // pixels. Keep that exact controller convention instead of attaching
        // synthetic source state unrelated to the physical pointer we warped.
        let wheel_y = (delta_y_per_tick / 12).clamp(-100, 100);
        let wheel_x = (delta_x_per_tick / 12).clamp(-100, 100);
        let event_ref = unsafe {
            CGEventCreateScrollWheelEvent2(
                std::ptr::null_mut(),
                ScrollEventUnit::PIXEL,
                2,
                wheel_y,
                wheel_x,
                0,
            )
        };
        if event_ref.is_null() {
            return Err(anyhow::anyhow!("CGEventCreateScrollWheelEvent2 failed"));
        }
        let event = unsafe { CGEvent::from_ptr(event_ref) };
        event.post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    Ok(())
}

/// Click one exact desktop point, then restore the user's cursor position.
///
/// This is intentionally narrower than the public desktop click path. It is
/// used only by approved, bounded setup flows after an accessibility element
/// has proven the exact target point and window.
pub fn click_at_xy_desktop_preserving_cursor(x: f64, y: f64) -> anyhow::Result<()> {
    click_at_xy_desktop_inner(x, y, 1, "left", &[], true)
}

extern "C" {
    /// Quartz's non-variadic scroll-event constructor. The public desktop
    /// path intentionally passes a null source to match real mouse-controller
    /// libraries such as pynput.
    fn CGEventCreateScrollWheelEvent2(
        source: core_graphics::sys::CGEventSourceRef,
        units: core_graphics::event::CGScrollEventUnit,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> core_graphics::sys::CGEventRef;

    /// Reconnect the mouse-delta stream to the (just-warped) cursor position so a
    /// synthesized click hit-tests at the new location, not the pre-warp one.
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
}

/// Like `click_at_xy` but also targets a specific window. Foreground delivery
/// uses the supplied window-local point with one public pid post. Background
/// delivery stamps the window-local point on macOS 27+, or the screen point
/// before, through SkyLight with a public fallback if the symbol is unavailable.
// The flattened arguments mirror the native event fields used by existing callers.
#[allow(clippy::too_many_arguments)]
pub fn click_at_xy_with_window_local(
    pid: i32,
    x: f64,
    y: f64,
    wx: f64,
    wy: f64,
    wid: u32,
    count: usize,
    modifiers: &[&str],
    delivery: WindowClickDelivery,
) -> anyhow::Result<()> {
    match delivery {
        WindowClickDelivery::Background => {
            click_at_xy_chromium(pid, x, y, wx, wy, wid, count, modifiers)
        }
        WindowClickDelivery::Foreground => click_at_xy_inner(
            pid,
            (x, y),
            Some((wx, wy)),
            Some(wid),
            count,
            modifiers,
            MousePostMode::PublicOnly,
        ),
    }
}

/// `screen` is the click point in screen coordinates.
fn click_at_xy_inner(
    pid: i32,
    screen: (f64, f64),
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    count: usize,
    modifiers: &[&str],
    post_mode: MousePostMode,
) -> anyhow::Result<()> {
    let (x, y) = screen;
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

    super::keyboard::with_pid_modifier_keys(pid, modifiers, || {
        // Prime the target window's cursor-tracking state with a leading
        // mouseMoved so an AppKit NSButton / NSView hit-tests the down at the
        // right point. Without it the synthetic mouseDown on a backgrounded
        // AppKit control is silently ignored.
        post_mouse_moved_primer(
            pid,
            &source,
            point,
            window_local,
            wid,
            click_group_id,
            post_mode,
        );
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
            set_explicit_flags(&down, flags);

            post_mouse_event_with_mode(
                pid,
                &down,
                window_local,
                wid,
                click_group_id,
                click_state,
                0,
                3,
                post_mode,
            );
            // 28 ms down→up gap: an NSButton's mouseDown enters a modal
            // tracking loop that polls for the matching mouseUp; too tight a
            // gap can race the loop's first poll and the click is dropped.
            std::thread::sleep(std::time::Duration::from_millis(28));

            let up = CGEvent::new_mouse_event(
                source.clone(),
                CGEventType::LeftMouseUp,
                point,
                CGMouseButton::Left,
            )
            .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event(up) failed"))?;
            set_explicit_flags(&up, flags);

            post_mouse_event_with_mode(
                pid,
                &up,
                window_local,
                wid,
                click_group_id,
                click_state,
                0,
                3,
                post_mode,
            );

            if count > 1 {
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
        }
        Ok(())
    })
}

/// Prepare a raw background pixel click by making the target window AppKit-key
/// without raising or restacking it and without taking key status from the
/// user's window.
///
/// The Swift implementation ran this immediately before the stamped event
/// stream. The original Rust port retained the SkyLight primitive but omitted
/// this call while cursor-overlay repinning was incomplete. Callers should
/// re-pin their overlay after this returns, then post the click sequence.
///
/// Returns whether the private focus-without-raise recipe succeeded. Event
/// posting remains best-effort when the private APIs are unavailable.
pub fn prepare_background_pixel_click(pid: i32, wid: u32) -> bool {
    // Post only the focus record to the target. The defocus record to the
    // current front process made the user's key window resign key, and
    // re-activating that process afterwards did not make the window key
    // again, so the user's typing was dropped until they clicked it.
    let activated =
        crate::input::skylight::activate_without_raise_with(pid as libc::pid_t, wid, false);
    // Match Swift's settle interval so AppKit updates its active/key-window
    // routing before the mouseMoved + primer + target stream arrives.
    std::thread::sleep(std::time::Duration::from_millis(50));
    activated
}

/// Event kinds in the pure route plan for the Chromium-compatible background
/// left-click recipe.
///
/// The sequence stays PID/window-routed throughout. The caller must first run
/// [`prepare_background_pixel_click`] for background delivery, then re-pin any
/// cursor overlay before entering this event stream.
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
///  - `CGEventSetWindowLocation` per-event (window-local point on macOS 27+,
///    screen point before; see `window_location_is_window_local`)
///
/// Uses the SkyLight route required by Chromium-compatible targets, with the
/// public API only as a fallback when the private symbol is unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChromiumClickEventKind {
    Move,
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ChromiumClickRouteStep {
    kind: ChromiumClickEventKind,
    event_location: (f64, f64),
    skylight_window_location: (f64, f64),
    public_window_location: (f64, f64),
    click_state: i64,
    phase: i64,
    button_number: i64,
    subtype: i64,
    delay_after_ms: u64,
}

/// Whether WindowServer hands the `CGEventSetWindowLocation` stamp of a
/// pid-routed click to the target as its window-local point. macOS 27 does:
/// a screen-point stamp arrives as `locationInWindow == (screen_x, height -
/// screen_y)`, so the click lands shifted by the window's origin (AppKit,
/// Chromium and GPUI windows alike). Earlier releases derived the local point
/// from the screen stamp themselves.
fn window_location_is_window_local() -> bool {
    static WINDOW_LOCAL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WINDOW_LOCAL.get_or_init(|| {
        crate::tools::macos_product_version()
            .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
            .is_some_and(|major| major >= 27)
    })
}

fn chromium_click_route_plan(
    screen_x: f64,
    screen_y: f64,
    win_local_x: f64,
    win_local_y: f64,
    count: usize,
    window_local: bool,
) -> Vec<ChromiumClickRouteStep> {
    let screen_target = (screen_x, screen_y);
    let stamped_target = if window_local {
        (win_local_x, win_local_y)
    } else {
        screen_target
    };
    let off_screen = (-1.0, -1.0);
    let mut steps = vec![
        ChromiumClickRouteStep {
            kind: ChromiumClickEventKind::Move,
            event_location: screen_target,
            skylight_window_location: stamped_target,
            public_window_location: stamped_target,
            click_state: 0,
            phase: 2,
            button_number: 0,
            subtype: 3,
            delay_after_ms: 15,
        },
        ChromiumClickRouteStep {
            kind: ChromiumClickEventKind::Down,
            event_location: off_screen,
            skylight_window_location: off_screen,
            public_window_location: off_screen,
            click_state: 1,
            phase: 1,
            button_number: 0,
            subtype: 3,
            delay_after_ms: 1,
        },
        ChromiumClickRouteStep {
            kind: ChromiumClickEventKind::Up,
            event_location: off_screen,
            skylight_window_location: off_screen,
            public_window_location: off_screen,
            click_state: 1,
            phase: 2,
            button_number: 0,
            subtype: 3,
            delay_after_ms: 100,
        },
    ];

    let click_pairs = count.clamp(1, 2);
    for pair_index in 1..=click_pairs {
        let click_state = pair_index as i64;
        steps.push(ChromiumClickRouteStep {
            kind: ChromiumClickEventKind::Down,
            event_location: screen_target,
            skylight_window_location: stamped_target,
            public_window_location: stamped_target,
            click_state,
            phase: 3,
            button_number: 0,
            subtype: 3,
            delay_after_ms: 1,
        });
        steps.push(ChromiumClickRouteStep {
            kind: ChromiumClickEventKind::Up,
            event_location: screen_target,
            skylight_window_location: stamped_target,
            public_window_location: stamped_target,
            click_state,
            phase: 3,
            button_number: 0,
            subtype: 3,
            delay_after_ms: if pair_index < click_pairs { 80 } else { 0 },
        });
    }
    steps
}

/// Post the route plan for the Chromium-compatible background left-click
/// recipe. `CGEvent::new_mouse_event` starts with the screen-space location.
/// `CGEventSetWindowLocation` carries the window-local point on macOS 27+ and
/// the screen point before, for both SkyLight and the public PID fallback.
#[allow(clippy::too_many_arguments)]
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
    let route_plan = chromium_click_route_plan(
        screen_x,
        screen_y,
        win_local_x,
        win_local_y,
        count,
        window_location_is_window_local(),
    );
    let flags = parse_modifier_flags(modifiers);
    let window_id = wid as i64;

    // All 5 events share the same click-group ID so WindowServer / Chromium
    // treat the sequence as one gesture (Swift: field 58).
    let click_group_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as i64;

    // Stamp required fields onto a CGEvent.  All captured values are Copy so
    // this closure is Fn (callable multiple times).
    let stamp = |event: &CGEvent, step: &ChromiumClickRouteStep| {
        let ptr = event.as_ptr() as *mut std::ffi::c_void;
        let set = |f: u32, v: i64| {
            crate::input::skylight::set_integer_field(ptr, f, v);
        };
        set(0, step.phase); // kCGMouseEventNumber (gesture phase)
        set(1, step.click_state); // kCGMouseEventClickState
        set(3, step.button_number); // kCGMouseEventButtonNumber (left)
        set(7, step.subtype); // kCGMouseEventSubtype (NSEventSubtypeTouch)
        set(40, pid as i64); // Chromium synthetic-event filter
        if window_id != 0 {
            set(51, window_id); // windowNumber (NSEvent bridge equivalent)
            set(91, window_id); // kCGMouseEventWindowUnderMousePointer
            set(92, window_id); // kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent
        }
        set(58, click_group_id); // click-group ID (gesture coalescing)
        crate::input::skylight::set_window_location(
            ptr,
            step.skylight_window_location.0,
            step.skylight_window_location.1,
        );
        set_explicit_flags(event, flags);
    };

    let post = |event: &CGEvent, _step: &ChromiumClickRouteStep| {
        dispatch_mouse_event(
            post_route(MousePostMode::SkyLightPreferred, skylight_available()),
            pid,
            event,
        );
    };

    for step in route_plan {
        let (event_type, error_label) = match step.kind {
            ChromiumClickEventKind::Move => (CGEventType::MouseMoved, "mouseMoved"),
            ChromiumClickEventKind::Down if step.phase == 1 => {
                (CGEventType::LeftMouseDown, "primer down")
            }
            ChromiumClickEventKind::Up if step.phase == 2 => {
                (CGEventType::LeftMouseUp, "primer up")
            }
            ChromiumClickEventKind::Down => (CGEventType::LeftMouseDown, "target down"),
            ChromiumClickEventKind::Up => (CGEventType::LeftMouseUp, "target up"),
        };
        let event = CGEvent::new_mouse_event(
            source.clone(),
            event_type,
            CGPoint::new(step.event_location.0, step.event_location.1),
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("{error_label} event creation failed"))?;
        stamp(&event, &step);
        post(&event, &step);
        if step.delay_after_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(step.delay_after_ms));
        }
    }

    Ok(())
}

/// Window drag with an observer called for every native pointer position.
///
/// Background delivery keeps the established PID-routed sequence. When
/// `foreground_hid` is true, the same stamped gesture is posted once per event
/// through the global HID queue while the hardware cursor follows the path.
///
/// The cursor overlay uses this for the same reason as
/// [`drag_at_xy_foreground_observed`]: the synthetic cursor should follow the
/// actual dispatched path rather than jumping to the endpoint after release.
#[allow(clippy::too_many_arguments)]
pub fn drag_at_xy_observed<F>(
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
    foreground_hid: bool,
    observe: F,
) -> anyhow::Result<()>
where
    F: FnMut(f64, f64),
{
    if foreground_hid {
        return drag_at_xy_window_foreground_observed(
            pid,
            from_x,
            from_y,
            to_x,
            to_y,
            from_local,
            to_local,
            wid,
            duration_ms,
            steps,
            modifiers,
            button,
            observe,
        );
    }

    let wid = wid.ok_or_else(|| {
        anyhow::anyhow!("a background drag needs the exact target window (window_id)")
    })?;
    drag_at_xy_background(
        pid,
        (from_x, from_y),
        (to_x, to_y),
        wid,
        duration_ms,
        steps,
        modifiers,
        button,
        observe,
        || true,
    )
    .map(|_| ())
}

/// How a background drag gesture ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackgroundDragOutcome {
    /// Every planned event was posted and the release landed on the drop point.
    Completed,
    /// The caller's `keep_going` check failed during the path; the release
    /// was posted at once at this screen point instead of the drop point.
    StoppedEarly { at: (f64, f64) },
}

/// Event kinds in the background drag plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackgroundDragEventKind {
    /// Stamped `mouseMoved` at the start point (cursor-tracking primer).
    Move,
    /// Off-screen left down/up at (-1, -1): the same Chromium user-activation
    /// primer the background click recipe sends before its target pair.
    PrimerDown,
    PrimerUp,
    Down,
    Dragged,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BackgroundDragStep {
    kind: BackgroundDragEventKind,
    /// Screen point. It is both the event location and the stamped window
    /// location, as in the background click recipe.
    point: (f64, f64),
    /// Field 0 gesture phase: move 2, primer down 1, primer up 2, gesture 3.
    phase: i64,
    click_state: i64,
    delay_after_ms: u64,
}

/// The complete background drag gesture, as data.
///
/// One stamped move primes the target's cursor-tracking state, an off-screen
/// primer click satisfies Chromium's user-activation gate, and then one
/// down, `steps` interpolated drags and one up make the gesture. Every event
/// is posted exactly once.
fn background_drag_plan(
    from: (f64, f64),
    to: (f64, f64),
    steps: usize,
    duration_ms: u64,
) -> Vec<BackgroundDragStep> {
    use BackgroundDragEventKind::*;
    let steps = steps.max(1);
    let step_delay_ms = duration_ms / steps as u64;
    let off_screen = (-1.0, -1.0);
    let step = |kind, point, phase, click_state, delay_after_ms| BackgroundDragStep {
        kind,
        point,
        phase,
        click_state,
        delay_after_ms,
    };
    let mut plan = Vec::with_capacity(steps + 5);
    plan.push(step(Move, from, 2, 0, 15));
    plan.push(step(PrimerDown, off_screen, 1, 1, 1));
    plan.push(step(PrimerUp, off_screen, 2, 1, 100));
    // A short hold before the first drag lets drag sources (NSTableView,
    // Chromium pointer capture) arm their tracking.
    plan.push(step(Down, from, 3, 1, 30));
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let point = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        // The last drag lands on the drop point; give the target one
        // run-loop turn to process it before the release.
        let delay = if i == steps {
            step_delay_ms.max(50)
        } else {
            step_delay_ms
        };
        plan.push(step(Dragged, point, 3, 1, delay));
    }
    plan.push(step(Up, to, 3, 1, 100));
    plan
}

/// Background press-drag-release addressed to one window of `pid`.
///
/// This is the background click route stretched over a gesture: every event
/// is posted to the process (SkyLight `SLEventPostToPid`, or the public
/// per-pid API only when the SPI is absent), carries the exact window in the
/// routing fields (f51/f91/f92), the target pid (f40) and one gesture ID
/// (f58), and is stamped with its own location. The hardware pointer is never
/// warped and nothing is posted to the global HID tap, so the user's pointer,
/// frontmost app and window order stay as they were. The caller makes the
/// window AppKit-key without raising it first
/// ([`prepare_background_pixel_click`]).
///
/// `keep_going` runs after each drag step; when it returns false the rest of
/// the path is skipped and the release is posted at once where the gesture
/// stands. The drag tool uses it to cut short a gesture that started a system
/// drag-and-drop session, which macOS tracks with the hardware pointer.
#[allow(clippy::too_many_arguments)]
pub fn drag_at_xy_background<F, G>(
    pid: i32,
    from: (f64, f64),
    to: (f64, f64),
    wid: u32,
    duration_ms: u64,
    steps: usize,
    modifiers: &[&str],
    button: DragButton,
    mut observe: F,
    mut keep_going: G,
) -> anyhow::Result<BackgroundDragOutcome>
where
    F: FnMut(f64, f64),
    G: FnMut() -> bool,
{
    use std::time::{SystemTime, UNIX_EPOCH};
    use BackgroundDragEventKind::*;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    let flags = parse_modifier_flags(modifiers);
    let (cg_button, down_type, dragged_type, up_type, button_number) = match button {
        DragButton::Left => (
            CGMouseButton::Left,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
            0,
        ),
        DragButton::Right => (
            CGMouseButton::Right,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseDragged,
            CGEventType::RightMouseUp,
            1,
        ),
        DragButton::Middle => (
            CGMouseButton::Center,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseDragged,
            CGEventType::OtherMouseUp,
            2,
        ),
    };
    let gesture_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as i64;
    let route = post_route(MousePostMode::SkyLightPreferred, skylight_available());
    let window_id = wid as i64;

    let mut stopped_at: Option<(f64, f64)> = None;
    let mut last_point = from;
    for mut step in background_drag_plan(from, to, steps, duration_ms) {
        if let Some(at) = stopped_at {
            match step.kind {
                Dragged => continue,
                Up => step.point = at,
                _ => {}
            }
        }
        // (type, button, button number, subtype). Primers keep the click
        // recipe's left button and touch subtype; the gesture itself is a
        // plain mouse drag (subtype 0), like the foreground drag.
        let (event_type, event_button, event_button_number, subtype) = match step.kind {
            Move => (CGEventType::MouseMoved, CGMouseButton::Left, 0, 3),
            PrimerDown => (CGEventType::LeftMouseDown, CGMouseButton::Left, 0, 3),
            PrimerUp => (CGEventType::LeftMouseUp, CGMouseButton::Left, 0, 3),
            Down => (down_type, cg_button, button_number, 0),
            Dragged => (dragged_type, cg_button, button_number, 0),
            Up => (up_type, cg_button, button_number, 0),
        };
        let point = CGPoint::new(step.point.0, step.point.1);
        let event = CGEvent::new_mouse_event(source.clone(), event_type, point, event_button)
            .map_err(|_| {
                anyhow::anyhow!("background drag {:?} event creation failed", step.kind)
            })?;
        let ptr = event.as_ptr() as *mut std::ffi::c_void;
        let set =
            |field: u32, value: i64| crate::input::skylight::set_integer_field(ptr, field, value);
        set(0, step.phase);
        set(1, step.click_state);
        set(3, event_button_number);
        set(7, subtype);
        set(40, pid as i64);
        set(51, window_id);
        set(91, window_id);
        set(92, window_id);
        set(58, gesture_id);
        crate::input::skylight::set_window_location(ptr, step.point.0, step.point.1);
        set_explicit_flags(&event, flags);
        dispatch_mouse_event(route, pid, &event);
        if matches!(step.kind, Down | Dragged) {
            observe(step.point.0, step.point.1);
            last_point = step.point;
        }
        if step.delay_after_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(step.delay_after_ms));
        }
        if step.kind == Dragged && stopped_at.is_none() && !keep_going() {
            stopped_at = Some(last_point);
        }
    }
    Ok(match stopped_at {
        Some(at) => BackgroundDragOutcome::StoppedEarly { at },
        None => BackgroundDragOutcome::Completed,
    })
}

#[allow(clippy::too_many_arguments)]
fn post_drag_mouse_event(
    pid: i32,
    event_type: CGEventType,
    point: CGPoint,
    button: CGMouseButton,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    click_group_id: Option<i64>,
    click_state: i64,
    button_number: i64,
    subtype: i64,
    mode: MousePostMode,
    flags: CGEventFlags,
    error_context: &str,
) -> anyhow::Result<()> {
    debug_assert_eq!(mode, MousePostMode::HidOnly);
    let event = new_global_mouse_event(event_type, point, button)
        .map_err(|_| anyhow::anyhow!(error_context.to_owned()))?;
    set_explicit_flags(&event, flags);
    post_mouse_event_with_mode(
        pid,
        &event,
        window_local,
        wid,
        click_group_id,
        click_state,
        button_number,
        subtype,
        mode,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn drag_at_xy_window_foreground_observed<F>(
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
    mut observe: F,
) -> anyhow::Result<()>
where
    F: FnMut(f64, f64),
{
    use core_graphics::display::CGDisplay;
    use std::time::{SystemTime, UNIX_EPOCH};

    let (cg_button, down_type, dragged_type, up_type, button_number) = match button {
        DragButton::Left => (
            CGMouseButton::Left,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
            0,
        ),
        DragButton::Right => (
            CGMouseButton::Right,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseDragged,
            CGEventType::RightMouseUp,
            1,
        ),
        DragButton::Middle => (
            CGMouseButton::Center,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseDragged,
            CGEventType::OtherMouseUp,
            2,
        ),
    };
    let click_group_id = wid.map(|_| {
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
    let events = foreground_drag_events(from_x, from_y, to_x, to_y, steps);

    super::keyboard::with_global_modifier_keys(modifiers, |flags| {
        for spec in events {
            let (event_type, click_state, event_button, event_button_number, subtype) = match spec
                .kind
            {
                ForegroundDragEventKind::Move => {
                    (CGEventType::MouseMoved, 0, CGMouseButton::Left, 0, 3)
                }
                ForegroundDragEventKind::Down => (down_type, 1, cg_button, button_number, 0),
                ForegroundDragEventKind::Dragged => (dragged_type, 1, cg_button, button_number, 0),
                ForegroundDragEventKind::Up => (up_type, 1, cg_button, button_number, 0),
            };
            let local = from_local.zip(to_local).map(|((fx, fy), (tx, ty))| {
                (
                    fx + (tx - fx) * spec.progress,
                    fy + (ty - fy) * spec.progress,
                )
            });

            match spec.kind {
                ForegroundDragEventKind::Move => {
                    let _ = CGDisplay::warp_mouse_cursor_position(spec.point);
                    unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                ForegroundDragEventKind::Dragged => {
                    let _ = CGDisplay::warp_mouse_cursor_position(spec.point);
                }
                ForegroundDragEventKind::Up => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                ForegroundDragEventKind::Down => {}
            }

            post_drag_mouse_event(
                pid,
                event_type,
                spec.point,
                event_button,
                local,
                wid,
                click_group_id,
                click_state,
                event_button_number,
                subtype,
                MousePostMode::HidOnly,
                flags,
                "foreground drag event creation failed",
            )?;

            match spec.kind {
                ForegroundDragEventKind::Move => {
                    std::thread::sleep(std::time::Duration::from_millis(12));
                }
                ForegroundDragEventKind::Down => {
                    observe(spec.point.x, spec.point.y);
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                ForegroundDragEventKind::Dragged => {
                    observe(spec.point.x, spec.point.y);
                    if step_delay_ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(step_delay_ms));
                    }
                }
                ForegroundDragEventKind::Up => {}
            }
        }
        Ok(())
    })?;

    std::thread::sleep(std::time::Duration::from_millis(100));
    Ok(())
}

/// Foreground drag with an observer called for every native pointer position.
///
/// The cursor overlay uses this to follow the same interpolated path and
/// cadence as the HID gesture instead of gliding only after the real pointer
/// has already completed the drag.
#[allow(clippy::too_many_arguments)]
pub fn drag_at_xy_foreground_observed<F>(
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    duration_ms: u64,
    steps: usize,
    modifiers: &[&str],
    button: DragButton,
    mut observe: F,
) -> anyhow::Result<()>
where
    F: FnMut(f64, f64),
{
    use core_graphics::display::CGDisplay;
    use core_graphics::event::CGEventTapLocation;

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
    let events = foreground_drag_events(from_x, from_y, to_x, to_y, steps);
    let steps = steps.max(1);
    let step_delay_ms = if steps > 1 {
        duration_ms / steps as u64
    } else {
        duration_ms
    };

    // Keep WindowServer's hardware cursor and event stream coupled. AppKit
    // hit-tests some pointer-capture surfaces against the actual cursor even
    // when the HID event carries an explicit location.
    let _ = CGDisplay::warp_mouse_cursor_position(CGPoint::new(from_x, from_y));
    unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
    observe(from_x, from_y);
    std::thread::sleep(std::time::Duration::from_millis(40));

    super::keyboard::with_global_modifier_keys(modifiers, |flags| {
        for spec in events {
            let event_type = match spec.kind {
                ForegroundDragEventKind::Move => CGEventType::MouseMoved,
                ForegroundDragEventKind::Down => down_type,
                ForegroundDragEventKind::Dragged => dragged_type,
                ForegroundDragEventKind::Up => up_type,
            };
            // A null source matches macOS's established global mouse-controller
            // path. WindowServer then carries the pressed-button state from the
            // down through every dragged event instead of treating each event as
            // an independent HIDSystemState snapshot.
            let event =
                new_global_mouse_event(event_type, spec.point, cg_button).map_err(|_| {
                    anyhow::anyhow!("foreground drag {:?} event creation failed", spec.kind)
                })?;
            event.set_flags(flags);
            event.post(CGEventTapLocation::HID);

            match spec.kind {
                ForegroundDragEventKind::Move => {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                ForegroundDragEventKind::Down => {
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                ForegroundDragEventKind::Dragged => {
                    observe(spec.point.x, spec.point.y);
                    if step_delay_ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(step_delay_ms));
                    }
                }
                ForegroundDragEventKind::Up => {}
            }
        }
        Ok(())
    })?;
    // The foreground wrapper restores the previous app immediately after this
    // function returns. Let the target's run loop consume the queued HID
    // gesture, including pointer-capture release, before that restore happens.
    std::thread::sleep(std::time::Duration::from_millis(100));
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForegroundDragEventKind {
    Move,
    Down,
    Dragged,
    Up,
}

#[derive(Clone, Copy, Debug)]
struct ForegroundDragEvent {
    kind: ForegroundDragEventKind,
    point: CGPoint,
    progress: f64,
}

fn foreground_drag_events(
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    steps: usize,
) -> Vec<ForegroundDragEvent> {
    let steps = steps.max(1);
    let mut events = Vec::with_capacity(steps + 3);
    let from = CGPoint::new(from_x, from_y);
    events.push(ForegroundDragEvent {
        kind: ForegroundDragEventKind::Move,
        point: from,
        progress: 0.0,
    });
    events.push(ForegroundDragEvent {
        kind: ForegroundDragEventKind::Down,
        point: from,
        progress: 0.0,
    });
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        events.push(ForegroundDragEvent {
            kind: ForegroundDragEventKind::Dragged,
            point: CGPoint::new(from_x + (to_x - from_x) * t, from_y + (to_y - from_y) * t),
            progress: t,
        });
    }
    events.push(ForegroundDragEvent {
        kind: ForegroundDragEventKind::Up,
        point: CGPoint::new(to_x, to_y),
        progress: 1.0,
    });
    events
}

fn new_global_mouse_event(
    event_type: CGEventType,
    point: CGPoint,
    button: CGMouseButton,
) -> Result<CGEvent, ()> {
    let event_ref =
        unsafe { CGEventCreateMouseEvent(std::ptr::null_mut(), event_type, point, button) };
    if event_ref.is_null() {
        Err(())
    } else {
        Ok(unsafe { CGEvent::from_ptr(event_ref) })
    }
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
/// to the target pid once through SkyLight, falling back to the public API
/// only when the SPI is absent. Window-local stamping mirrors
/// `right_click_at_xy_with_window_local`.
pub fn middle_click_at_xy(pid: i32, x: f64, y: f64, modifiers: &[&str]) -> anyhow::Result<()> {
    right_click_at_xy_inner(pid, x, y, None, None, modifiers, CGMouseButton::Center)
}

pub fn middle_click_at_xy_with_window_local(
    pid: i32,
    x: f64,
    y: f64,
    wx: f64,
    wy: f64,
    wid: u32,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    right_click_at_xy_inner(
        pid,
        x,
        y,
        Some((wx, wy)),
        Some(wid),
        modifiers,
        CGMouseButton::Center,
    )
}

/// Right-click at `(x, y)` with optional modifier keys (no window routing).
pub fn right_click_at_xy(pid: i32, x: f64, y: f64, modifiers: &[&str]) -> anyhow::Result<()> {
    right_click_at_xy_inner(pid, x, y, None, None, modifiers, CGMouseButton::Right)
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
///
/// The `mouseMoved` primer, `rightMouseDown` and `rightMouseUp` are each posted
/// once, through SkyLight (the public API only when the SPI is absent). Posting
/// the same event through both transports delivered it to an AppKit view twice,
/// so one right click arrived as down, down, up, up (#4679).
pub fn right_click_at_xy_with_window_local(
    pid: i32,
    x: f64,
    y: f64,
    wx: f64,
    wy: f64,
    wid: u32,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    right_click_at_xy_inner(
        pid,
        x,
        y,
        Some((wx, wy)),
        Some(wid),
        modifiers,
        CGMouseButton::Right,
    )
}

fn right_click_at_xy_inner(
    pid: i32,
    x: f64,
    y: f64,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    modifiers: &[&str],
    button: CGMouseButton,
) -> anyhow::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let (down_type, up_type, button_number) = match button {
        CGMouseButton::Center => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, 2),
        _ => (CGEventType::RightMouseDown, CGEventType::RightMouseUp, 1),
    };

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

    post_mouse_moved_primer(
        pid,
        &source,
        point,
        window_local,
        wid,
        click_group_id,
        MousePostMode::SkyLightPreferred,
    );
    std::thread::sleep(std::time::Duration::from_millis(12));

    let down = CGEvent::new_mouse_event(source.clone(), down_type, point, button)
        .map_err(|_| anyhow::anyhow!("mouse down failed"))?;
    set_explicit_flags(&down, flags);
    post_mouse_event_with_mode(
        pid,
        &down,
        window_local,
        wid,
        click_group_id,
        1,
        button_number,
        3,
        MousePostMode::SkyLightPreferred,
    );
    std::thread::sleep(std::time::Duration::from_millis(28));

    let up = CGEvent::new_mouse_event(source, up_type, point, button)
        .map_err(|_| anyhow::anyhow!("mouse up failed"))?;
    set_explicit_flags(&up, flags);
    post_mouse_event_with_mode(
        pid,
        &up,
        window_local,
        wid,
        click_group_id,
        1,
        button_number,
        3,
        MousePostMode::SkyLightPreferred,
    );

    Ok(())
}

/// Post a mouse event to `pid`.
///
/// Matches Swift's `MouseInput.postBoth(_:toPid:)`:
/// - Fires `SLEventPostToPid` (SkyLight path — reaches backgrounded Chromium/Catalyst).
/// - Also fires `CGEvent::post_to_pid` (public path — lands on AppKit targets where
///   SkyLight mouse delivery drops).
///
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
// These arguments are the individual CGEvent fields stamped by this low-level primitive.
#[allow(clippy::too_many_arguments)]
pub(super) fn post_mouse_event(
    pid: i32,
    event: &CGEvent,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    click_group_id: Option<i64>,
    click_state: i64,
    button_number: i64,
    subtype: i64,
) {
    post_mouse_event_with_mode(
        pid,
        event,
        window_local,
        wid,
        click_group_id,
        click_state,
        button_number,
        subtype,
        MousePostMode::Both,
    );
}

#[allow(clippy::too_many_arguments)]
fn post_mouse_event_with_mode(
    pid: i32,
    event: &CGEvent,
    window_local: Option<(f64, f64)>,
    wid: Option<u32>,
    click_group_id: Option<i64>,
    click_state: i64,
    button_number: i64,
    subtype: i64,
    mode: MousePostMode,
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

    dispatch_mouse_event(post_route(mode, skylight_available()), pid, event);
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
    mode: MousePostMode,
) {
    if let Ok(mv) = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::MouseMoved,
        point,
        CGMouseButton::Left,
    ) {
        post_mouse_event_with_mode(pid, &mv, window_local, wid, click_group_id, 0, 0, 3, mode);
    }
}

/// Synthesize a targeted mouse-wheel scroll at `(screen_x, screen_y)`,
/// posted to `pid`.
///
/// This is the targeted wheel path. Unlike the keystroke path (PageDown /
/// arrow keys), which only ever drives the *focused* / page scroller, a real
/// wheel event is hit-tested by the renderer at the cursor point: whatever
/// element sits under `(screen_x, screen_y)` receives the scroll. That is the
/// only way to scroll a nested `overflow:auto` div that has no `tabindex` and
/// therefore can never take keyboard focus (verified no-op via keystrokes on
/// WKWebView's inner `scroll-tall` and WebView2).
///
/// `CGEventCreateScrollWheelEvent2` builds a line-unit event; we then anchor it
/// at the target point with `CGEventSetLocation` so the renderer routes it
/// correctly, and stamp the same background-delivery fields the click
/// primitives use (window-local point + f40 pid filter + window-routing fields)
/// so it reaches backgrounded Chromium/Catalyst/WKWebView targets.
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
// These arguments are the individual wheel-event fields used by existing callers.
#[allow(clippy::too_many_arguments)]
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

    // Prime AppKit/WebKit's tracking state at the target before the wheel
    // gesture. Background windows can retain a stale hit-test location; a
    // nested overflow scroller then receives the wheel nowhere even though
    // the event is successfully posted to the target pid.
    let primer_source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
    post_mouse_moved_primer(
        pid,
        &primer_source,
        CGPoint::new(screen_x, screen_y),
        window_local,
        wid,
        Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as i64,
        ),
        MousePostMode::Both,
    );
    std::thread::sleep(std::time::Duration::from_millis(12));

    for _ in 0..ticks.max(1) {
        // Fresh source per event, matching the click primitives.
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
        // wheel_count = 2 → both axes carried (vertical = wheel1/axis-1,
        // horizontal = wheel2/axis-2). Convert the driver's pixel-tuned step
        // into a bounded line delta for the event API.
        let wheel_y = (delta_y_per_tick / 120).clamp(-10, 10);
        let wheel_x = (delta_x_per_tick / 120).clamp(-10, 10);
        let event =
            CGEvent::new_scroll_event(source, ScrollEventUnit::LINE, 2, wheel_y, wheel_x, 0)
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
    /// Quartz mouse constructor with a null source, matching established
    /// foreground controller libraries and the global scroll path above.
    fn CGEventCreateMouseEvent(
        source: core_graphics::sys::CGEventSourceRef,
        event_type: core_graphics::event::CGEventType,
        point: CGPoint,
        button: core_graphics::event::CGMouseButton,
    ) -> core_graphics::sys::CGEventRef;

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
            "cmd" | "command" | "super" | "meta" | "win" => {
                flags |= CGEventFlags::CGEventFlagCommand
            }
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "option" | "alt" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            _ => {}
        }
    }
    flags
}

/// Test seam for [`dispatch_mouse_event`]. A test starts a [`Capture`] on its
/// own thread, runs a real input primitive, and reads back the transport each
/// event would have been handed to. Nothing is posted while a capture is
/// active, so the tests need no window server, target app, or TCC grant.
#[cfg(test)]
mod post_sink {
    use super::PostRoute;
    use core_graphics::event::{CGEvent, EventField};
    use std::cell::RefCell;

    pub(super) const MODIFIER_MASK: u64 = 0x0002_0000 // shift
        | 0x0004_0000 // control
        | 0x0008_0000 // option
        | 0x0010_0000; // command

    /// One event handed to a transport: its type name, the transport, and the
    /// button number stamped on it (0 left, 1 right, 2 middle).
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(super) struct Posted {
        pub event: String,
        pub route: PostRoute,
        pub button: i64,
    }

    impl Posted {
        pub(super) fn new(event: &str, route: PostRoute, button: i64) -> Self {
            Self {
                event: event.to_owned(),
                route,
                button,
            }
        }
    }

    struct State {
        skylight_available: bool,
        posted: Vec<Posted>,
        /// Modifier flags carried by each posted event, parallel to `posted`.
        modifiers: Vec<u64>,
        window_routing: Vec<[i64; 4]>,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    pub(super) struct Capture;

    impl Capture {
        /// Start capturing on this thread, with the SkyLight SPI reported as
        /// present or absent.
        pub(super) fn start(skylight_available: bool) -> Self {
            STATE.with(|state| {
                *state.borrow_mut() = Some(State {
                    skylight_available,
                    posted: Vec::new(),
                    modifiers: Vec::new(),
                    window_routing: Vec::new(),
                });
            });
            Self
        }

        /// Shift/control/option/command flags of each posted event. The rest of
        /// the flag word reflects whatever keys the developer holds while the
        /// tests run, so it is masked out.
        pub(super) fn modifiers(&self) -> Vec<u64> {
            STATE.with(|state| {
                state
                    .borrow()
                    .as_ref()
                    .map(|state| state.modifiers.clone())
                    .unwrap_or_default()
            })
        }

        pub(super) fn posted(&self) -> Vec<Posted> {
            STATE.with(|state| {
                state
                    .borrow()
                    .as_ref()
                    .map(|state| state.posted.clone())
                    .unwrap_or_default()
            })
        }

        pub(super) fn window_routing(&self) -> Vec<[i64; 4]> {
            STATE.with(|state| state.borrow().as_ref().unwrap().window_routing.clone())
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            STATE.with(|state| *state.borrow_mut() = None);
        }
    }

    pub(super) fn skylight_override() -> Option<bool> {
        STATE.with(|state| {
            state
                .borrow()
                .as_ref()
                .map(|state| state.skylight_available)
        })
    }

    /// Record `event` when a capture is active; returns whether it did.
    pub(super) fn record(route: PostRoute, event: &CGEvent) -> bool {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return false;
            };
            state.posted.push(Posted {
                event: format!("{:?}", event.get_type()),
                route,
                button: event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER),
            });
            state
                .modifiers
                .push(event.get_flags().bits() & MODIFIER_MASK);
            state
                .window_routing
                .push([51, 91, 92, 58].map(|field| event.get_integer_value_field(field)));
            true
        })
    }
}

#[cfg(test)]
mod tests {
    use super::post_sink::{Capture, Posted};
    use super::*;

    #[test]
    fn background_middle_click_preserves_exact_window_routing() {
        let capture = Capture::start(true);
        middle_click_at_xy_with_window_local(1, 100.0, 100.0, 10.0, 10.0, 7, &[]).unwrap();
        let routing = capture.window_routing();
        assert_eq!(routing.len(), 3);
        for fields in &routing {
            assert_eq!(&fields[..3], &[7, 7, 7]);
            assert_ne!(fields[3], 0, "window stamps require a click-group ID");
        }
        assert_eq!(routing[0][3], routing[1][3]);
        assert_eq!(routing[1][3], routing[2][3]);
        assert_eq!(
            capture.posted(),
            [
                Posted::new("MouseMoved", PostRoute::SkyLight, 0),
                Posted::new("OtherMouseDown", PostRoute::SkyLight, 2),
                Posted::new("OtherMouseUp", PostRoute::SkyLight, 2),
            ]
        );
    }

    #[test]
    fn background_middle_click_uses_public_fallback_once() {
        let capture = Capture::start(false);
        middle_click_at_xy_with_window_local(1, 100.0, 100.0, 10.0, 10.0, 7, &[]).unwrap();
        assert_eq!(
            capture.posted(),
            [
                Posted::new("MouseMoved", PostRoute::PublicPid, 0),
                Posted::new("OtherMouseDown", PostRoute::PublicPid, 2),
                Posted::new("OtherMouseUp", PostRoute::PublicPid, 2),
            ]
        );
    }

    #[test]
    fn window_less_middle_click_keeps_window_routing_unset() {
        let capture = Capture::start(true);
        middle_click_at_xy(1, 100.0, 100.0, &[]).unwrap();
        assert_eq!(capture.window_routing(), [[0; 4]; 3]);
        assert_eq!(
            capture.posted(),
            [
                Posted::new("MouseMoved", PostRoute::SkyLight, 0),
                Posted::new("OtherMouseDown", PostRoute::SkyLight, 2),
                Posted::new("OtherMouseUp", PostRoute::SkyLight, 2),
            ]
        );
    }

    #[test]
    fn capture_bound_background_route_preserves_nonzero_origin_hit() {
        let frame = crate::tools::px_frame::WindowPxFrame {
            bounds: crate::windows::WindowBounds {
                x: 170.0,
                y: 194.0,
                width: 760.0,
                height: 460.0,
            },
            scale: 2.0,
        };
        let capture_local = (394.0, 310.0);
        let admitted_native = (capture_local.0 * frame.scale, capture_local.1 * frame.scale);
        let (screen_x, screen_y, local_x, local_y) =
            frame.to_screen(admitted_native.0, admitted_native.1);
        assert_eq!((screen_x, screen_y), (564.0, 504.0));
        assert_eq!((local_x, local_y), capture_local);

        let plan = chromium_click_route_plan(screen_x, screen_y, local_x, local_y, 1, false);
        assert_eq!(
            plan,
            vec![
                ChromiumClickRouteStep {
                    kind: ChromiumClickEventKind::Move,
                    event_location: (564.0, 504.0),
                    skylight_window_location: (564.0, 504.0),
                    public_window_location: (564.0, 504.0),
                    click_state: 0,
                    phase: 2,
                    button_number: 0,
                    subtype: 3,
                    delay_after_ms: 15,
                },
                ChromiumClickRouteStep {
                    kind: ChromiumClickEventKind::Down,
                    event_location: (-1.0, -1.0),
                    skylight_window_location: (-1.0, -1.0),
                    public_window_location: (-1.0, -1.0),
                    click_state: 1,
                    phase: 1,
                    button_number: 0,
                    subtype: 3,
                    delay_after_ms: 1,
                },
                ChromiumClickRouteStep {
                    kind: ChromiumClickEventKind::Up,
                    event_location: (-1.0, -1.0),
                    skylight_window_location: (-1.0, -1.0),
                    public_window_location: (-1.0, -1.0),
                    click_state: 1,
                    phase: 2,
                    button_number: 0,
                    subtype: 3,
                    delay_after_ms: 100,
                },
                ChromiumClickRouteStep {
                    kind: ChromiumClickEventKind::Down,
                    event_location: (564.0, 504.0),
                    skylight_window_location: (564.0, 504.0),
                    public_window_location: (564.0, 504.0),
                    click_state: 1,
                    phase: 3,
                    button_number: 0,
                    subtype: 3,
                    delay_after_ms: 1,
                },
                ChromiumClickRouteStep {
                    kind: ChromiumClickEventKind::Up,
                    event_location: (564.0, 504.0),
                    skylight_window_location: (564.0, 504.0),
                    public_window_location: (564.0, 504.0),
                    click_state: 1,
                    phase: 3,
                    button_number: 0,
                    subtype: 3,
                    delay_after_ms: 0,
                },
            ]
        );

        let screen_to_canvas = (-frame.bounds.x, -frame.bounds.y);
        let unconverted_journal_point = (
            capture_local.0 + screen_to_canvas.0,
            capture_local.1 + screen_to_canvas.1,
        );
        assert_eq!(unconverted_journal_point, (224.0, 116.0));
        assert!(!(292.0..=496.0).contains(&unconverted_journal_point.0));

        let delivered = plan
            .iter()
            .find(|step| step.kind == ChromiumClickEventKind::Down && step.phase == 3)
            .expect("target mouse-down step");
        assert_eq!(delivered.event_location, (564.0, 504.0));
        assert_eq!(delivered.skylight_window_location, (564.0, 504.0));
        assert_eq!(delivered.public_window_location, (564.0, 504.0));
        let skylight_journal_point = (
            delivered.skylight_window_location.0 + screen_to_canvas.0,
            delivered.skylight_window_location.1 + screen_to_canvas.1,
        );
        assert_eq!(skylight_journal_point, capture_local);
        assert!((292.0..=496.0).contains(&skylight_journal_point.0));
        assert!((132.0..=310.0).contains(&skylight_journal_point.1));
    }

    #[test]
    fn macos_27_route_stamps_the_window_local_point() {
        // macOS 27 reads the stamp as the window-local point: a screen-point
        // stamp at (564, 504) for a window at (170, 194) would hit (564, 504)
        // inside the window instead of (394, 310).
        let plan = chromium_click_route_plan(564.0, 504.0, 394.0, 310.0, 2, true);
        let targets: Vec<_> = plan
            .iter()
            .filter(|step| step.phase != 1 && step.event_location != (-1.0, -1.0))
            .collect();
        assert_eq!(targets.len(), 5, "move + two down/up pairs");
        for step in targets {
            assert_eq!(step.event_location, (564.0, 504.0));
            assert_eq!(step.skylight_window_location, (394.0, 310.0));
            assert_eq!(step.public_window_location, (394.0, 310.0));
        }
        let primers: Vec<_> = plan
            .iter()
            .filter(|step| step.event_location == (-1.0, -1.0))
            .collect();
        assert_eq!(primers.len(), 2);
        for step in primers {
            assert_eq!(step.skylight_window_location, (-1.0, -1.0));
        }
    }

    #[test]
    fn foreground_drag_plan_is_one_coherent_gesture() {
        let events = foreground_drag_events(10.0, 20.0, 40.0, 80.0, 3);
        let kinds: Vec<_> = events.iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            [
                ForegroundDragEventKind::Move,
                ForegroundDragEventKind::Down,
                ForegroundDragEventKind::Dragged,
                ForegroundDragEventKind::Dragged,
                ForegroundDragEventKind::Dragged,
                ForegroundDragEventKind::Up,
            ]
        );
        let points: Vec<_> = events
            .iter()
            .map(|event| (event.point.x, event.point.y))
            .collect();
        assert_eq!(
            points,
            [
                (10.0, 20.0),
                (10.0, 20.0),
                (20.0, 40.0),
                (30.0, 60.0),
                (40.0, 80.0),
                (40.0, 80.0),
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == ForegroundDragEventKind::Up)
                .count(),
            1,
            "the HID gesture must release exactly once"
        );
        let cursor_plan: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    ForegroundDragEventKind::Move | ForegroundDragEventKind::Dragged
                )
            })
            .map(|event| (event.point.x, event.point.y))
            .collect();
        assert_eq!(
            cursor_plan,
            [(10.0, 20.0), (20.0, 40.0), (30.0, 60.0), (40.0, 80.0)]
        );
    }

    #[test]
    fn foreground_drag_plan_keeps_a_drag_sample_for_zero_steps() {
        let events = foreground_drag_events(1.0, 2.0, 3.0, 4.0, 0);
        assert_eq!(events.len(), 4);
        assert_eq!(events[2].kind, ForegroundDragEventKind::Dragged);
        assert_eq!((events[2].point.x, events[2].point.y), (3.0, 4.0));
    }

    #[test]
    fn global_drag_events_use_hardware_like_source_and_button_fields() {
        use core_graphics::event::EventField;

        let point = CGPoint::new(10.0, 20.0);
        let cases = [
            (CGEventType::LeftMouseDown, 1, 1.0),
            (CGEventType::LeftMouseDragged, 1, 1.0),
            (CGEventType::LeftMouseUp, 0, 0.0),
        ];
        for (event_type, click_state, pressure) in cases {
            let event = new_global_mouse_event(event_type, point, CGMouseButton::Left).unwrap();
            assert_eq!(
                event.get_integer_value_field(EventField::EVENT_SOURCE_STATE_ID),
                CGEventSourceStateID::CombinedSessionState as i64
            );
            assert_eq!(
                event.get_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE),
                click_state
            );
            assert_eq!(
                event.get_double_value_field(EventField::MOUSE_EVENT_PRESSURE),
                pressure
            );
        }
    }

    #[test]
    fn skylight_preferred_posts_once_and_never_through_both_transports() {
        use MousePostMode::*;
        use PostRoute::*;
        assert_eq!(post_route(SkyLightPreferred, true), SkyLight);
        assert_eq!(
            post_route(SkyLightPreferred, false),
            PublicPid,
            "the public API is only a fallback for a missing SPI"
        );
        for available in [true, false] {
            assert_eq!(post_route(PublicOnly, available), PublicPid);
            assert_eq!(post_route(HidOnly, available), Hid);
        }
    }

    /// The left background click has posted each event once since the
    /// duplicate-delivery fix. This pins that sequence so the other pointer
    /// primitives can be held to the same shape.
    #[test]
    fn background_left_click_posts_one_move_and_one_down_up_pair_through_skylight() {
        let capture = Capture::start(true);
        click_at_xy_chromium(1, 100.0, 100.0, 10.0, 10.0, 7, 1, &[]).unwrap();
        assert_eq!(
            capture.posted(),
            [
                Posted::new("MouseMoved", PostRoute::SkyLight, 0),
                // Off-screen primer pair that satisfies Chromium's activation gate.
                Posted::new("LeftMouseDown", PostRoute::SkyLight, 0),
                Posted::new("LeftMouseUp", PostRoute::SkyLight, 0),
                Posted::new("LeftMouseDown", PostRoute::SkyLight, 0),
                Posted::new("LeftMouseUp", PostRoute::SkyLight, 0),
            ]
        );
    }

    #[test]
    fn background_left_click_falls_back_to_the_public_api_once_without_the_spi() {
        let capture = Capture::start(false);
        click_at_xy_chromium(1, 100.0, 100.0, 10.0, 10.0, 7, 1, &[]).unwrap();
        let posted = capture.posted();
        assert_eq!(posted.len(), 5);
        assert!(posted.iter().all(|p| p.route == PostRoute::PublicPid));
    }

    #[test]
    fn background_right_click_posts_one_move_and_one_down_up_pair_through_skylight() {
        let capture = Capture::start(true);
        right_click_at_xy_with_window_local(1, 100.0, 100.0, 10.0, 10.0, 7, &[]).unwrap();
        assert_eq!(
            capture.posted(),
            [
                Posted::new("MouseMoved", PostRoute::SkyLight, 0),
                Posted::new("RightMouseDown", PostRoute::SkyLight, 1),
                Posted::new("RightMouseUp", PostRoute::SkyLight, 1),
            ]
        );
    }

    #[test]
    fn background_right_click_falls_back_to_the_public_api_once_without_the_spi() {
        let capture = Capture::start(false);
        right_click_at_xy_with_window_local(1, 100.0, 100.0, 10.0, 10.0, 7, &[]).unwrap();
        assert_eq!(
            capture.posted(),
            [
                Posted::new("MouseMoved", PostRoute::PublicPid, 0),
                Posted::new("RightMouseDown", PostRoute::PublicPid, 1),
                Posted::new("RightMouseUp", PostRoute::PublicPid, 1),
            ]
        );
    }

    #[test]
    fn desktop_pointer_move_posts_one_hid_mouse_moved_at_the_target() {
        // Only the post half: the warp half would move the host's pointer.
        let capture = Capture::start(true);
        post_pointer_moved(CGPoint::new(163.0, 159.0)).unwrap();
        assert_eq!(
            capture.posted(),
            [Posted::new("MouseMoved", PostRoute::Hid, 0)]
        );
    }

    #[test]
    fn window_less_right_click_posts_one_down_up_pair() {
        let capture = Capture::start(true);
        right_click_at_xy(1, 100.0, 100.0, &[]).unwrap();
        let events: Vec<_> = capture
            .posted()
            .into_iter()
            .map(|p| (p.event, p.route))
            .collect();
        assert_eq!(
            events,
            [
                ("MouseMoved".to_owned(), PostRoute::SkyLight),
                ("RightMouseDown".to_owned(), PostRoute::SkyLight),
                ("RightMouseUp".to_owned(), PostRoute::SkyLight),
            ]
        );
    }

    #[test]
    fn right_click_modifiers_ride_on_the_single_down_and_up() {
        let capture = Capture::start(true);
        right_click_at_xy_with_window_local(1, 100.0, 100.0, 10.0, 10.0, 7, &["cmd", "shift"])
            .unwrap();
        let posted = capture.posted();
        let modifiers = capture.modifiers();
        let names: Vec<_> = posted.iter().map(|p| p.event.as_str()).collect();
        assert_eq!(names, ["MouseMoved", "RightMouseDown", "RightMouseUp"]);
        let expected =
            CGEventFlags::CGEventFlagCommand.bits() | CGEventFlags::CGEventFlagShift.bits();
        assert_eq!(&modifiers[1..], [expected, expected]);
    }
    #[test]
    fn background_drag_plan_is_one_gesture_from_start_to_drop() {
        use BackgroundDragEventKind::*;
        let plan = background_drag_plan((100.0, 200.0), (300.0, 100.0), 4, 400);
        let kinds: Vec<_> = plan.iter().map(|step| step.kind).collect();
        assert_eq!(
            kinds,
            [Move, PrimerDown, PrimerUp, Down, Dragged, Dragged, Dragged, Dragged, Up]
        );
        assert_eq!(plan[0].point, (100.0, 200.0));
        assert_eq!(
            plan[1].point,
            (-1.0, -1.0),
            "the primer click stays off-screen"
        );
        assert_eq!(plan[3].point, (100.0, 200.0));
        assert_eq!(plan[4].point, (150.0, 175.0));
        assert_eq!(
            plan[7].point,
            (300.0, 100.0),
            "the last drag lands on the drop point"
        );
        assert_eq!(plan[8].point, (300.0, 100.0));
        for step in &plan[3..] {
            assert_eq!(step.phase, 3, "down, drags and up share one gesture phase");
            assert_eq!(step.click_state, 1);
        }
        assert_eq!(plan[4].delay_after_ms, 100);
        assert!(
            plan[7].delay_after_ms >= 50,
            "the drop point settles before release"
        );
    }

    #[test]
    fn background_drag_plan_keeps_one_drag_for_zero_steps() {
        let plan = background_drag_plan((0.0, 0.0), (10.0, 0.0), 0, 0);
        let drags: Vec<_> = plan
            .iter()
            .filter(|step| step.kind == BackgroundDragEventKind::Dragged)
            .collect();
        assert_eq!(drags.len(), 1);
        assert_eq!(drags[0].point, (10.0, 0.0));
    }

    #[test]
    fn background_drag_posts_each_event_once_to_the_pid_with_window_routing() {
        let capture = Capture::start(true);
        let mut observed = Vec::new();
        drag_at_xy_background(
            42,
            (100.0, 100.0),
            (200.0, 100.0),
            7,
            0,
            2,
            &["shift"],
            DragButton::Left,
            |x, y| observed.push((x, y)),
            || true,
        )
        .unwrap();
        let posted = capture.posted();
        let events: Vec<_> = posted.iter().map(|posted| posted.event.as_str()).collect();
        assert_eq!(
            events,
            [
                "MouseMoved",
                "LeftMouseDown",
                "LeftMouseUp",
                "LeftMouseDown",
                "LeftMouseDragged",
                "LeftMouseDragged",
                "LeftMouseUp"
            ]
        );
        assert!(
            posted
                .iter()
                .all(|posted| posted.route == PostRoute::SkyLight),
            "a background drag never reaches the HID tap or a second transport"
        );
        let routing = capture.window_routing();
        let gesture = routing[0][3];
        for fields in routing {
            assert_eq!(
                &fields[..3],
                &[7, 7, 7],
                "every event names the exact window"
            );
            assert_eq!(fields[3], gesture, "every event carries one gesture ID");
        }
        assert!(capture
            .modifiers()
            .iter()
            .all(|flags| *flags == 0x0002_0000));
        assert_eq!(observed, [(100.0, 100.0), (150.0, 100.0), (200.0, 100.0)]);
    }

    #[test]
    fn background_right_drag_uses_right_button_events_and_public_fallback() {
        let capture = Capture::start(false);
        drag_at_xy_background(
            42,
            (0.0, 0.0),
            (10.0, 10.0),
            7,
            0,
            1,
            &[],
            DragButton::Right,
            |_, _| {},
            || true,
        )
        .unwrap();
        let posted = capture.posted();
        assert!(posted
            .iter()
            .all(|posted| posted.route == PostRoute::PublicPid));
        let gesture: Vec<_> = posted[3..]
            .iter()
            .map(|posted| (posted.event.as_str(), posted.button))
            .collect();
        assert_eq!(
            gesture,
            [
                ("RightMouseDown", 1),
                ("RightMouseDragged", 1),
                ("RightMouseUp", 1)
            ]
        );
    }

    #[test]
    fn background_drag_stopped_early_releases_where_it_stands() {
        let capture = Capture::start(true);
        let mut checks = 0;
        let outcome = drag_at_xy_background(
            42,
            (0.0, 0.0),
            (100.0, 0.0),
            7,
            0,
            4,
            &[],
            DragButton::Left,
            |_, _| {},
            || {
                checks += 1;
                checks < 2
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            BackgroundDragOutcome::StoppedEarly { at: (50.0, 0.0) }
        );
        let events: Vec<_> = capture
            .posted()
            .iter()
            .map(|posted| posted.event.clone())
            .collect();
        assert_eq!(
            events[3..],
            [
                "LeftMouseDown",
                "LeftMouseDragged",
                "LeftMouseDragged",
                "LeftMouseUp"
            ],
            "no drag after the stop, and the release still goes out"
        );
    }

    #[test]
    fn window_less_background_drag_is_refused_before_posting() {
        let capture = Capture::start(true);
        let error = drag_at_xy_observed(
            42,
            0.0,
            0.0,
            10.0,
            10.0,
            None,
            None,
            None,
            0,
            1,
            &[],
            DragButton::Left,
            false,
            |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("window_id"));
        assert!(capture.posted().is_empty());
    }
}

#[cfg(test)]
mod explicit_flags_tests {
    use super::*;

    #[test]
    fn explicit_flags_clear_an_inherited_modifier() {
        let source = CGEventSource::new(CGEventSourceStateID::Private).expect("event source");
        let event = CGEvent::new_mouse_event(
            source,
            CGEventType::LeftMouseDown,
            core_graphics::geometry::CGPoint::new(10.0, 10.0),
            CGMouseButton::Left,
        )
        .expect("mouse event");
        // As if the HID state had Control down when the event was made.
        event.set_flags(CGEventFlags::CGEventFlagControl);
        set_explicit_flags(&event, CGEventFlags::CGEventFlagNull);
        assert_eq!(event.get_flags(), CGEventFlags::CGEventFlagNull);
        set_explicit_flags(&event, CGEventFlags::CGEventFlagCommand);
        assert_eq!(event.get_flags(), CGEventFlags::CGEventFlagCommand);
    }
}
