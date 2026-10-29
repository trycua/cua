//! Keep the display awake while an agent is working.
//!
//! On macOS 14 and later the screen saver and display sleep put the login
//! session behind the lock screen, even when no password is required. While
//! that holds, every app's accessibility windows read as empty and input goes
//! to the lock screen, so background actions cannot run. An agent working on
//! an idle Mac (no one touching the keyboard or mouse) hits this as soon as
//! the idle timer fires.
//!
//! Each tool call holds an IOKit `PreventUserIdleDisplaySleep` assertion (the
//! one `caffeinate -d` and video players take) until no tool has run for the
//! idle window. It only stops the *idle* timer: it never wakes a sleeping
//! display, dismisses the screen saver, or gets past a lock or password, and
//! the user can still sleep or lock the Mac by hand.
//!
//! The idle display-sleep path dims the display first, and loginwindow can
//! lock on that dim (`kLWLockFromDisplayDim`) without the screen saver ever
//! running. A `PreventUserIdleDisplaySleep` assertion holds off the idle dim as
//! well as the sleep that follows (the display stays at full brightness during
//! video playback, which takes the same assertion), so that lock does not fire
//! while the guard is held. It is not a system-sleep assertion: keeping the
//! machine awake is not enough, because the lock comes from the display path.
//!
//! `CUA_DRIVER_KEEP_DISPLAY_AWAKE` sets the idle window in seconds (default
//! 600); `0`, `off` or `false` turns the guard off.
//!
//! A display that is already off stays off under the guard, and while it is
//! off WindowServer lists no active display: a ScreenCaptureKit display
//! capture then completes with neither an image nor an error, which the
//! `screencapturekit` Swift bridge traps on (SIGILL). [`wake_for_capture`]
//! declares user activity to turn the display on before such a capture, and
//! refuses with [`DisplayAsleep`] when it stays off. A display that comes on
//! shows the login shield first: on an unlocked Mac the capture waits for it to
//! drop, on a locked one it captures the lock screen.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::display::CGDisplay;

/// Environment variable that tunes or disables the guard.
pub const KEEP_DISPLAY_AWAKE_ENV: &str = "CUA_DRIVER_KEEP_DISPLAY_AWAKE";

/// How long the display stays awake after the last tool call by default.
pub const DEFAULT_IDLE_WINDOW: Duration = Duration::from_secs(600);

/// Refusal code of a display capture while the display stays off.
pub const DISPLAY_ASLEEP: &str = "display_asleep";

const WAKE_WAIT: Duration = Duration::from_secs(3);
const WAKE_POLL: Duration = Duration::from_millis(50);
const SHIELD_WAIT: Duration = Duration::from_millis(1500);
const SHIELD_FADE: Duration = Duration::from_millis(500);

type IOPMAssertionID = u32;
type IOReturn = i32;
const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
const K_IO_RETURN_SUCCESS: IOReturn = 0;
const K_IOPM_USER_ACTIVE_LOCAL: u32 = 0;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFStringRef,
        assertion_level: u32,
        assertion_name: CFStringRef,
        assertion_id: *mut IOPMAssertionID,
    ) -> IOReturn;
    fn IOPMAssertionDeclareUserActivity(
        assertion_name: CFStringRef,
        user_type: u32,
        assertion_id: *mut IOPMAssertionID,
    ) -> IOReturn;
    fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;
}

/// The main display stayed off, so there is nothing to capture.
#[derive(Debug)]
pub struct DisplayAsleep;

impl std::fmt::Display for DisplayAsleep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{DISPLAY_ASLEEP}: the display is off and did not turn on within {} s, so \
             there is no screen to capture; wake the Mac, then retry",
            WAKE_WAIT.as_secs()
        )
    }
}

impl std::error::Error for DisplayAsleep {}

/// Turn the display on for a display capture when it is off.
///
/// Costs two WindowServer reads when the display is on. When it is off this
/// declares local user activity, as a key press would, waits up to
/// [`WAKE_WAIT`] for an active display, then up to [`SHIELD_WAIT`] for an
/// unlocked session to drop the login shield and [`SHIELD_FADE`] for the
/// desktop to fade in.
pub fn wake_for_capture() -> Result<(), DisplayAsleep> {
    if !display_off() {
        return Ok(());
    }
    declare_user_activity();
    if !wait_until(|| !display_off(), WAKE_WAIT, WAKE_POLL) {
        return Err(DisplayAsleep);
    }
    if wait_until(|| !crate::session::screen_locked(), SHIELD_WAIT, WAKE_POLL) {
        std::thread::sleep(SHIELD_FADE);
    }
    Ok(())
}

fn wait_until(done: impl Fn() -> bool, wait: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + wait;
    while !done() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
    true
}

fn display_off() -> bool {
    !CGDisplay::active_display_count().is_ok_and(|count| count > 0) || CGDisplay::main().is_asleep()
}

fn declare_user_activity() {
    let name = CFString::from_static_string("Cua Driver: capture the display");
    let mut id: IOPMAssertionID = 0;
    let status = unsafe {
        IOPMAssertionDeclareUserActivity(
            name.as_concrete_TypeRef(),
            K_IOPM_USER_ACTIVE_LOCAL,
            &mut id,
        )
    };
    if status == K_IO_RETURN_SUCCESS {
        tracing::debug!("declared user activity to wake the display");
        unsafe { IOPMAssertionRelease(id) };
    } else {
        tracing::warn!("could not wake the display: IOPMAssertionDeclareUserActivity {status}");
    }
}

/// Parse the env value into the idle window, or `None` when disabled.
pub fn idle_window_from_env(value: Option<&str>) -> Option<Duration> {
    let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Some(DEFAULT_IDLE_WINDOW);
    };
    match raw.to_ascii_lowercase().as_str() {
        "0" | "off" | "false" | "no" => None,
        "on" | "true" | "yes" => Some(DEFAULT_IDLE_WINDOW),
        other => match other.parse::<u64>() {
            Ok(secs) => Some(Duration::from_secs(secs.min(86_400))),
            Err(_) => {
                tracing::warn!(
                    "{KEEP_DISPLAY_AWAKE_ENV}={raw:?} is not a number of seconds or off; \
                     using the default"
                );
                Some(DEFAULT_IDLE_WINDOW)
            }
        },
    }
}

struct Guard {
    assertion: IOPMAssertionID,
    last_activity: Instant,
}

static GUARD: Mutex<Option<Guard>> = Mutex::new(None);

/// Register the guard as the registry's tool-activity hook, unless the env
/// turns it off. Safe to call more than once.
pub fn install() {
    let Some(idle) = idle_window_from_env(std::env::var(KEEP_DISPLAY_AWAKE_ENV).ok().as_deref())
    else {
        tracing::info!("display-awake guard off ({KEEP_DISPLAY_AWAKE_ENV})");
        return;
    };
    cua_driver_core::tool_activity::set_tool_activity_fn(move || note_activity(idle));
}

/// Hold the assertion now and keep it until `idle` passes with no activity.
pub fn note_activity(idle: Duration) {
    let mut guard = GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(held) = guard.as_mut() {
        held.last_activity = Instant::now();
        return;
    }
    let Some(assertion) = create_assertion() else {
        return;
    };
    *guard = Some(Guard {
        assertion,
        last_activity: Instant::now(),
    });
    drop(guard);
    let spawned = std::thread::Builder::new()
        .name("cua-display-awake".into())
        .spawn(move || release_when_idle(idle));
    if spawned.is_err() {
        release_now();
    }
}

fn release_when_idle(idle: Duration) {
    loop {
        let wait = {
            let guard = GUARD
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(held) = guard.as_ref() else {
                return;
            };
            remaining(held.last_activity.elapsed(), idle)
        };
        match wait {
            Some(wait) => std::thread::sleep(wait),
            None => {
                release_now();
                return;
            }
        }
    }
}

/// Time left before release, or `None` once the idle window has passed.
fn remaining(elapsed: Duration, idle: Duration) -> Option<Duration> {
    idle.checked_sub(elapsed).filter(|left| !left.is_zero())
}

fn release_now() {
    let held = GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(held) = held {
        unsafe { IOPMAssertionRelease(held.assertion) };
        tracing::debug!("display-awake guard released");
    }
}

fn create_assertion() -> Option<IOPMAssertionID> {
    let kind = CFString::from_static_string("PreventUserIdleDisplaySleep");
    let name = CFString::from_static_string("Cua Driver: an agent is working on this Mac");
    let mut id: IOPMAssertionID = 0;
    let status = unsafe {
        IOPMAssertionCreateWithName(
            kind.as_concrete_TypeRef(),
            K_IOPM_ASSERTION_LEVEL_ON,
            name.as_concrete_TypeRef(),
            &mut id,
        )
    };
    if status == K_IO_RETURN_SUCCESS {
        tracing::debug!("display-awake guard held");
        Some(id)
    } else {
        tracing::warn!("could not keep the display awake: IOPMAssertionCreateWithName {status}");
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_defaults_to_ten_minutes_and_can_be_turned_off() {
        assert_eq!(idle_window_from_env(None), Some(DEFAULT_IDLE_WINDOW));
        assert_eq!(idle_window_from_env(Some("")), Some(DEFAULT_IDLE_WINDOW));
        assert_eq!(idle_window_from_env(Some("on")), Some(DEFAULT_IDLE_WINDOW));
        for off in ["0", "off", "FALSE", "no"] {
            assert_eq!(idle_window_from_env(Some(off)), None, "{off}");
        }
        assert_eq!(
            idle_window_from_env(Some("120")),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            idle_window_from_env(Some("999999")),
            Some(Duration::from_secs(86_400))
        );
        assert_eq!(
            idle_window_from_env(Some("soon")),
            Some(DEFAULT_IDLE_WINDOW)
        );
    }

    #[test]
    fn release_waits_for_the_full_idle_window() {
        let idle = Duration::from_secs(600);
        assert_eq!(
            remaining(Duration::from_secs(100), idle),
            Some(Duration::from_secs(500))
        );
        assert_eq!(remaining(idle, idle), None);
        assert_eq!(remaining(Duration::from_secs(700), idle), None);
    }

    #[test]
    fn a_wait_returns_as_soon_as_its_condition_holds() {
        let polls = std::cell::Cell::new(0);
        let held = wait_until(
            || {
                polls.set(polls.get() + 1);
                polls.get() >= 4
            },
            Duration::from_secs(5),
            Duration::from_millis(1),
        );
        assert!(held);
        assert_eq!(polls.get(), 4);
        assert!(wait_until(|| true, Duration::ZERO, Duration::from_secs(5)));
    }

    #[test]
    fn a_wait_whose_condition_never_holds_gives_up_within_the_bound() {
        let started = Instant::now();
        let held = wait_until(
            || false,
            Duration::from_millis(30),
            Duration::from_millis(5),
        );
        assert!(!held);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn display_asleep_names_its_code() {
        assert!(DisplayAsleep.to_string().starts_with(DISPLAY_ASLEEP));
    }

    /// Holding and releasing the real assertion works and leaves nothing
    /// held. The idle timer is not reset, so this does not wake the display.
    #[test]
    fn assertion_is_created_and_released() {
        let id = create_assertion().expect("IOPMAssertionCreateWithName");
        assert_eq!(unsafe { IOPMAssertionRelease(id) }, K_IO_RETURN_SUCCESS);
    }
}
