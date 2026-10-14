//! Opt-in gates for features that are not customer-facing yet.
//!
//! Teams (`cua team`, sharing with `team:<id>`, `cua host pair --team`) are
//! a stopgap stored in the relay until Teams move to the identity provider's
//! organizations and billing entitlements. They stay off unless
//! `CUA_EXPERIMENTAL_TEAMS` is set (to anything but `0`, `false` or empty),
//! for internal and staging use.

use std::sync::atomic::{AtomicU8, Ordering};

/// Environment variable that turns Teams on.
pub const TEAMS_ENV: &str = "CUA_EXPERIMENTAL_TEAMS";

/// What a Teams call says while Teams are off.
pub const TEAMS_UNAVAILABLE: &str = "Teams isn't available yet.";

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

/// Whether a flag value turns its feature on.
pub fn flag_on(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") | Some("0") => false,
        Some(v) => !v.eq_ignore_ascii_case("false"),
    }
}

#[cfg(test)]
mod tests {
    use super::flag_on;

    #[test]
    fn only_a_set_non_false_value_turns_a_flag_on() {
        for off in [
            None,
            Some(""),
            Some(" "),
            Some("0"),
            Some("false"),
            Some("FALSE"),
        ] {
            assert!(!flag_on(off), "{off:?}");
        }
        for on in [Some("1"), Some("true"), Some("yes")] {
            assert!(flag_on(on), "{on:?}");
        }
    }
}
