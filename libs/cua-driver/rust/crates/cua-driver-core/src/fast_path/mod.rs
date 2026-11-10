// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Fast path: act on cached targets with act-time guards instead of a fresh
//! full window snapshot before every action (CUA-3102).
//!
//! Everything in this module is platform-free and std-only. Platforms read
//! the live state (AX/UIA/AT-SPI attributes, window list, pixel crops) and
//! hand it to these checks; the checks decide whether acting is safe.
//!
//! Modes (`fast_path` config key, env `CUA_DRIVER_FAST_PATH`):
//! - `off` (default): no checks, no behaviour change.
//! - `shadow`: run the checks and report would-refuse verdicts in the result,
//!   never refuse.
//! - `guard`: enforce the checks; refuse with `target_changed` on mismatch.
//! - `on`: `guard`, plus a small delta in each action result and tree-only
//!   re-reads unless a screenshot is asked for.

pub mod change;
pub mod classify;
pub mod fingerprint;
pub mod pixel;
pub mod pixel_guard;
pub mod reason;
pub mod store;
pub mod window;
#[cfg(not(fast_path_standalone))]
pub mod wire;

#[cfg(test)]
mod race_tests;

/// Serialises tests that change the daemon-wide mode.
#[cfg(test)]
pub(crate) fn tests_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

use std::sync::atomic::{AtomicU8, Ordering};

/// Axis-aligned rectangle `[x, y, w, h]` in points (or pixels where noted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_array(a: [f64; 4]) -> Self {
        Self::new(a[0], a[1], a[2], a[3])
    }
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    pub fn area(&self) -> f64 {
        self.w.max(0.0) * self.h.max(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FastPathMode {
    Off,
    Shadow,
    Guard,
    On,
}

impl FastPathMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "off" | "false" | "0" => Some(Self::Off),
            "shadow" => Some(Self::Shadow),
            "guard" => Some(Self::Guard),
            "on" | "true" | "1" => Some(Self::On),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::Guard => "guard",
            Self::On => "on",
        }
    }
    /// Checks run (shadow, guard, on).
    pub fn checks(self) -> bool {
        self != Self::Off
    }
    /// A mismatch refuses the action (guard, on).
    pub fn enforces(self) -> bool {
        matches!(self, Self::Guard | Self::On)
    }
    /// Action results carry a delta and re-reads skip screenshots (on).
    pub fn deltas(self) -> bool {
        self == Self::On
    }
    fn code(self) -> u8 {
        match self {
            Self::Off => 1,
            Self::Shadow => 2,
            Self::Guard => 3,
            Self::On => 4,
        }
    }
    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Off),
            2 => Some(Self::Shadow),
            3 => Some(Self::Guard),
            4 => Some(Self::On),
            _ => None,
        }
    }
}

/// 0 = not initialised yet (read the env var on first use).
static MODE: AtomicU8 = AtomicU8::new(0);

pub const ENV_VAR: &str = "CUA_DRIVER_FAST_PATH";

/// The daemon-wide mode. Defaults to `off`; the env var seeds it once and
/// `set_config {fast_path}` changes it at runtime.
pub fn mode() -> FastPathMode {
    if let Some(mode) = FastPathMode::from_code(MODE.load(Ordering::Relaxed)) {
        return mode;
    }
    let seeded = std::env::var(ENV_VAR)
        .ok()
        .and_then(|raw| FastPathMode::parse(&raw))
        .unwrap_or(FastPathMode::Off);
    let _ = MODE.compare_exchange(0, seeded.code(), Ordering::Relaxed, Ordering::Relaxed);
    FastPathMode::from_code(MODE.load(Ordering::Relaxed)).unwrap_or(FastPathMode::Off)
}

pub fn set_mode(mode: FastPathMode) {
    MODE.store(mode.code(), Ordering::Relaxed);
}

/// Per-call `verify_target` override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyTarget {
    /// Follow the daemon mode.
    Default,
    /// Enforce every applicable check, whatever the mode.
    Strict,
    /// Window guards only (live video, charts): result says `window_only`.
    Window,
    /// No checks for this call.
    Off,
}

impl VerifyTarget {
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(|s| s.trim().to_ascii_lowercase()) {
            None => Ok(Self::Default),
            Some(s) if s.is_empty() || s == "default" => Ok(Self::Default),
            Some(s) if s == "strict" => Ok(Self::Strict),
            Some(s) if s == "window" => Ok(Self::Window),
            Some(s) if s == "off" => Ok(Self::Off),
            Some(other) => Err(format!(
                "verify_target must be one of default, strict, window, off (got \"{other}\")"
            )),
        }
    }
}

/// What a call should do, after combining the daemon mode and the override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effective {
    pub run_checks: bool,
    pub enforce: bool,
    pub pixel_check: bool,
    pub deltas: bool,
}

pub fn effective(mode: FastPathMode, verify: VerifyTarget) -> Effective {
    match verify {
        VerifyTarget::Off => Effective {
            run_checks: false,
            enforce: false,
            pixel_check: false,
            deltas: mode.deltas(),
        },
        VerifyTarget::Strict => Effective {
            run_checks: true,
            enforce: true,
            pixel_check: true,
            deltas: mode.deltas(),
        },
        VerifyTarget::Window => Effective {
            run_checks: true,
            enforce: mode.enforces() || mode == FastPathMode::Off,
            pixel_check: false,
            deltas: mode.deltas(),
        },
        VerifyTarget::Default => Effective {
            run_checks: mode.checks(),
            enforce: mode.enforces(),
            pixel_check: mode.checks(),
            deltas: mode.deltas(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses_and_defaults_off() {
        assert_eq!(FastPathMode::parse(""), Some(FastPathMode::Off));
        assert_eq!(FastPathMode::parse("Guard"), Some(FastPathMode::Guard));
        assert_eq!(FastPathMode::parse("on"), Some(FastPathMode::On));
        assert_eq!(FastPathMode::parse("shadow"), Some(FastPathMode::Shadow));
        assert_eq!(FastPathMode::parse("fast"), None);
        assert!(!FastPathMode::Off.checks());
        assert!(FastPathMode::Shadow.checks() && !FastPathMode::Shadow.enforces());
        assert!(FastPathMode::On.deltas() && !FastPathMode::Guard.deltas());
    }

    #[test]
    fn off_mode_runs_nothing_by_default() {
        let e = effective(FastPathMode::Off, VerifyTarget::Default);
        assert!(!e.run_checks && !e.enforce && !e.pixel_check && !e.deltas);
    }

    #[test]
    fn overrides_combine_with_mode() {
        assert!(effective(FastPathMode::Off, VerifyTarget::Strict).enforce);
        let w = effective(FastPathMode::On, VerifyTarget::Window);
        assert!(w.run_checks && w.enforce && !w.pixel_check && w.deltas);
        assert!(!effective(FastPathMode::On, VerifyTarget::Off).run_checks);
        assert!(VerifyTarget::parse(Some("bogus")).is_err());
        assert_eq!(VerifyTarget::parse(None).unwrap(), VerifyTarget::Default);
    }
}
