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
//! `CUA_DRIVER_KEEP_DISPLAY_AWAKE` sets the idle window in seconds (default
//! 600); `0`, `off` or `false` turns the guard off.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};

/// Environment variable that tunes or disables the guard.
pub const KEEP_DISPLAY_AWAKE_ENV: &str = "CUA_DRIVER_KEEP_DISPLAY_AWAKE";

/// How long the display stays awake after the last tool call by default.
pub const DEFAULT_IDLE_WINDOW: Duration = Duration::from_secs(600);

type IOPMAssertionID = u32;
type IOReturn = i32;
const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
const K_IO_RETURN_SUCCESS: IOReturn = 0;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFStringRef,
        assertion_level: u32,
        assertion_name: CFStringRef,
        assertion_id: *mut IOPMAssertionID,
    ) -> IOReturn;
    fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;
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

    /// Holding and releasing the real assertion works and leaves nothing
    /// held. The idle timer is not reset, so this does not wake the display.
    #[test]
    fn assertion_is_created_and_released() {
        let id = create_assertion().expect("IOPMAssertionCreateWithName");
        assert_eq!(unsafe { IOPMAssertionRelease(id) }, K_IO_RETURN_SUCCESS);
    }
}
