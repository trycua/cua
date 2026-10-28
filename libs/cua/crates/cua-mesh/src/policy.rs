// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! When to use the mesh: on, unless turned off.
//!
//! - `CUA_MESH=prefer|require|off`, where the process runs, always wins
//!   (`cua`, the SDK, the cua daemon, a hosted machine's service).
//! - Anything else: [`Policy::Prefer`], on every channel of the Cua Spaces
//!   app and for the CLI alone: the mesh when both ends and the relay speak
//!   it, the relay's HTTPS path otherwise (an older machine, a relay with no
//!   mesh such as a self-hosted `cua-relay`, a mesh that does not answer).
//!   `CUA_MESH=off` is the escape hatch back to the HTTPS path only.

/// The environment variable that chooses the policy explicitly.
pub const ENV: &str = "CUA_MESH";

/// How relay connections use the mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// The mesh when both sides speak it, the relay's HTTPS path otherwise
    /// (the default).
    Prefer,
    /// The mesh only (a relay that can read sessions is never used).
    Require,
    /// The relay's HTTPS path only (`CUA_MESH=off`).
    Off,
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

    /// The policy from an explicit `CUA_MESH` value, else [`Self::Prefer`].
    pub fn resolve(explicit: Option<&str>) -> Self {
        explicit.and_then(Self::parse).unwrap_or(Self::Prefer)
    }

    /// The policy now: `CUA_MESH`, else [`Self::Prefer`].
    pub fn current() -> Self {
        Self::resolve(std::env::var(ENV).ok().as_deref())
    }

    /// The `CUA_MESH` value that chooses this policy.
    pub fn as_env(self) -> &'static str {
        match self {
            Self::Prefer => "prefer",
            Self::Require => "require",
            Self::Off => "off",
        }
    }

    /// Whether this policy uses the mesh at all.
    pub fn uses_mesh(self) -> bool {
        self != Self::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefer_by_default() {
        assert_eq!(Policy::resolve(None), Policy::Prefer);
        // Unknown and empty values do not choose, so they keep the default.
        assert_eq!(Policy::resolve(Some("")), Policy::Prefer);
        assert_eq!(Policy::resolve(Some("maybe")), Policy::Prefer);
        assert!(Policy::resolve(None).uses_mesh());
    }

    #[test]
    fn cua_mesh_off_and_require_win() {
        for v in ["off", "OFF", " 0 ", "false", "no"] {
            assert_eq!(Policy::resolve(Some(v)), Policy::Off, "{v}");
        }
        assert!(!Policy::resolve(Some("off")).uses_mesh());
        for v in ["require", " REQUIRE ", "required", "only"] {
            assert_eq!(Policy::resolve(Some(v)), Policy::Require, "{v}");
        }
        for v in ["prefer", "on", "1", "true", "yes"] {
            assert_eq!(Policy::resolve(Some(v)), Policy::Prefer, "{v}");
        }
    }
}
