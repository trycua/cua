//! Main-display power state.
//!
//! When the display is asleep (idle sleep, lid closed without an awake
//! external display), two things break silently:
//!   * `screencapture` fails for every window, so `get_window_state` /
//!     `zoom` / `debug_image_out` return opaque errors, and
//!   * posted CGEvents may never be rendered by the target app, and the
//!     agent has no screenshot to verify them against.
//!
//! An agent that doesn't know the display is asleep burns its budget
//! re-deriving coordinates and re-posting clicks into the void. Capture
//! errors and action results on macOS therefore carry an explicit
//! "display is asleep" marker while this state holds.

use std::os::raw::c_uint;

extern "C" {
    fn CGMainDisplayID() -> c_uint;
    fn CGDisplayIsAsleep(display: c_uint) -> u32;
}

/// True when the main display is asleep. Pure WindowServer query — cheap
/// enough to call on every action/capture result.
pub fn main_display_asleep() -> bool {
    unsafe { CGDisplayIsAsleep(CGMainDisplayID()) != 0 }
}

/// Hint appended to capture errors while the display sleeps.
pub const ASLEEP_CAPTURE_HINT: &str =
    "the main display is asleep, so macOS cannot capture windows. Wake it \
     (user presence or `caffeinate -u -t 1`) and retry";

const ASLEEP_ACTION_SUFFIX: &str =
    "\n\n😴 Main display is ASLEEP: the event was posted, but the app may \
     never render it and no screenshot can verify it. Wake the display \
     (user presence or `caffeinate -u -t 1`) before retrying or verifying — \
     do NOT re-derive coordinates from this failure.";

/// Suffix for action success messages: empty while the display is awake.
pub fn asleep_suffix() -> &'static str {
    asleep_suffix_for(main_display_asleep())
}

/// Pure mapping used by `asleep_suffix` — split out for unit testing.
fn asleep_suffix_for(asleep: bool) -> &'static str {
    if asleep {
        ASLEEP_ACTION_SUFFIX
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_empty_when_awake() {
        assert_eq!(asleep_suffix_for(false), "");
    }

    #[test]
    fn suffix_warns_when_asleep() {
        let s = asleep_suffix_for(true);
        assert!(s.contains("ASLEEP"));
        assert!(s.contains("caffeinate"));
    }
}
