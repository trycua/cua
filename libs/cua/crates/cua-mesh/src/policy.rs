// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! When to use the mesh: off unless asked for.
//!
//! - `CUA_MESH=prefer|require|off`, where the process runs, always wins
//!   (`cua`, the SDK, the cua daemon, a hosted machine's service).
//! - Otherwise the Cua Spaces app's update channel decides for the app's
//!   own processes (its cua daemon and this machine's host): the app
//!   records it in `<cua home>/app-update-channel` ([`CHANNEL_FILE`]).
//!   [`ON_IN_BETA`] is the single switch that turns the mesh on for the
//!   Beta channel; it is off in this release, so the mesh is opt-in on
//!   every channel.
//! - Anything else: [`Policy::Off`], the relay's HTTPS path, as before the
//!   mesh existed.

use std::path::Path;

/// The environment variable that chooses the policy explicitly.
pub const ENV: &str = "CUA_MESH";

/// The file, in a Cua home, where the Cua Spaces app records its update
/// channel (`beta` or `stable`).
pub const CHANNEL_FILE: &str = "app-update-channel";

/// Whether the Beta-channel app uses the mesh without `CUA_MESH`. Off: the
/// mesh is opt-in on every channel until it is turned on for Beta.
pub const ON_IN_BETA: bool = false;

/// How relay connections use the mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// The mesh when both sides speak it, the relay's HTTPS path otherwise.
    Prefer,
    /// The mesh only (a relay that can read sessions is never used).
    Require,
    /// The relay's HTTPS path only (the default).
    Off,
}

/// The Cua Spaces app's update channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Stable releases.
    Stable,
    /// Beta (pre)releases.
    Beta,
}

impl Policy {
    /// A `CUA_MESH` value; `None` when it does not choose (unset, empty or
    /// unknown).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Some(Self::Off),
            "require" | "required" | "only" => Some(Self::Require),
            "prefer" | "on" | "1" | "true" | "yes" => Some(Self::Prefer),
            _ => None,
        }
    }

    /// The policy from an explicit `CUA_MESH` value, else the app's channel
    /// (when this process is the app's), else off.
    pub fn resolve(explicit: Option<&str>, channel: Option<Channel>, on_in_beta: bool) -> Self {
        if let Some(p) = explicit.and_then(Self::parse) {
            return p;
        }
        match channel {
            Some(Channel::Beta) if on_in_beta => Self::Prefer,
            _ => Self::Off,
        }
    }

    /// The policy now: `CUA_MESH`, else the channel recorded in `app_home`
    /// (the Cua home of the app this process belongs to; `None` for a
    /// process that is not the app's, such as a plain `cua` run).
    pub fn current(app_home: Option<&Path>) -> Self {
        let explicit = std::env::var(ENV).ok();
        if let Some(p) = explicit.as_deref().and_then(Self::parse) {
            return p;
        }
        let channel = app_home.and_then(read_channel);
        Self::resolve(None, channel, ON_IN_BETA)
    }

    /// Whether this policy uses the mesh at all.
    pub fn uses_mesh(self) -> bool {
        self != Self::Off
    }
}

/// The channel the app recorded in `home` ([`CHANNEL_FILE`]).
pub fn read_channel(home: &Path) -> Option<Channel> {
    let raw = std::fs::read_to_string(home.join(CHANNEL_FILE)).ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "beta" => Some(Channel::Beta),
        "stable" => Some(Channel::Stable),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_unless_asked_for() {
        assert_eq!(Policy::resolve(None, None, ON_IN_BETA), Policy::Off);
        assert_eq!(
            Policy::resolve(None, Some(Channel::Stable), ON_IN_BETA),
            Policy::Off
        );
        // Unknown and empty values do not choose.
        assert_eq!(Policy::resolve(Some(""), None, ON_IN_BETA), Policy::Off);
        assert_eq!(
            Policy::resolve(Some("maybe"), None, ON_IN_BETA),
            Policy::Off
        );
    }

    #[test]
    fn the_beta_channel_is_off_in_this_release() {
        // The one switch: flipping it is the "on for Beta" change, and this
        // test changes with it.
        const { assert!(!ON_IN_BETA) };
        assert_eq!(
            Policy::resolve(None, Some(Channel::Beta), ON_IN_BETA),
            Policy::Off
        );
    }

    #[test]
    fn with_the_switch_on_only_beta_prefers_the_mesh() {
        assert_eq!(
            Policy::resolve(None, Some(Channel::Beta), true),
            Policy::Prefer
        );
        assert_eq!(
            Policy::resolve(None, Some(Channel::Stable), true),
            Policy::Off
        );
        assert_eq!(Policy::resolve(None, None, true), Policy::Off);
    }

    #[test]
    fn cua_mesh_always_wins() {
        for channel in [None, Some(Channel::Stable), Some(Channel::Beta)] {
            for on in [false, true] {
                assert_eq!(Policy::resolve(Some("prefer"), channel, on), Policy::Prefer);
                assert_eq!(
                    Policy::resolve(Some(" REQUIRE "), channel, on),
                    Policy::Require
                );
                assert_eq!(Policy::resolve(Some("off"), channel, on), Policy::Off);
            }
        }
    }

    #[test]
    fn the_app_records_its_channel_in_the_cua_home() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_channel(dir.path()), None);
        std::fs::write(dir.path().join(CHANNEL_FILE), "beta\n").unwrap();
        assert_eq!(read_channel(dir.path()), Some(Channel::Beta));
        std::fs::write(dir.path().join(CHANNEL_FILE), "stable").unwrap();
        assert_eq!(read_channel(dir.path()), Some(Channel::Stable));
        std::fs::write(dir.path().join(CHANNEL_FILE), "nightly").unwrap();
        assert_eq!(read_channel(dir.path()), None);
    }
}
