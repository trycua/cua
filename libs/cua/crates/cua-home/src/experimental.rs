//! Feature gates.
//!
//! Teams (`cua team`, sharing with `team:<id>`, `cua host pair --team`) are
//! on by default. Who may use them is decided by the service: the Spaces API
//! (org membership and the Stripe Teams entitlement) and the relay refuse
//! anyone who is not entitled. `CUA_EXPERIMENTAL_TEAMS=0` (or `false`,
//! `off`, `no`) is a local kill switch that hides and refuses Teams.

use std::sync::atomic::{AtomicU8, Ordering};

/// Environment variable that is the kill switch for Teams (`0` turns them off).
pub const TEAMS_ENV: &str = "CUA_EXPERIMENTAL_TEAMS";

/// What a Teams call says while Teams are off.
pub const TEAMS_UNAVAILABLE: &str = "Teams is turned off (CUA_EXPERIMENTAL_TEAMS=0).";

/// 0: follow the environment; 1: off; 2: on.
static TEAMS_OVERRIDE: AtomicU8 = AtomicU8::new(0);

/// Whether Teams are on in this process: the override, else
/// [`TEAMS_ENV`].
pub fn teams_enabled() -> bool {
    match TEAMS_OVERRIDE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => flag_on(std::env::var(TEAMS_ENV).ok().as_deref()),
    }
}

/// Turns Teams on or off for this whole process (`None` follows
/// [`TEAMS_ENV`] again). For tests and embedders, which cannot safely set
/// an environment variable once threads run.
pub fn set_teams_enabled(on: Option<bool>) {
    let v = match on {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    TEAMS_OVERRIDE.store(v, Ordering::Relaxed);
}

/// Whether a flag value leaves its feature on: unset or empty is on; `0`,
/// `false`, `off` and `no` are off.
pub fn flag_on(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") => true,
        Some(v) => {
            !(v == "0"
                || v.eq_ignore_ascii_case("false")
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("no"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::flag_on;

    #[test]
    fn a_flag_is_on_unless_it_is_set_to_an_off_value() {
        for off in [
            Some("0"),
            Some(" 0 "),
            Some("false"),
            Some("FALSE"),
            Some("off"),
            Some("no"),
        ] {
            assert!(!flag_on(off), "{off:?}");
        }
        for on in [
            None,
            Some(""),
            Some(" "),
            Some("1"),
            Some("true"),
            Some("yes"),
        ] {
            assert!(flag_on(on), "{on:?}");
        }
    }
}
