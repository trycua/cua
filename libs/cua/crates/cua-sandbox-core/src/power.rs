//! Turning a sandbox off and on again, the same words for every provider.
//!
//! A provider says how it turns a sandbox off ([`PowerControl`]): it
//! suspends it (memory kept: a paused container, a paused QEMU VM, a
//! platform snapshot) or stops it (disk kept: a Lume VM, a stopped cloud
//! instance). One that can do neither reports none, and nothing offers a
//! power control for its sandboxes. [`crate::Sandboxes::power_off`] and
//! [`crate::Sandboxes::power_on`] do the one the provider has.

use serde::{Deserialize, Serialize};

/// How a sandbox turns off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerControl {
    /// Suspended in memory, resumed where it was.
    Suspend,
    /// Stopped with its disk kept, booted again.
    Stop,
}

impl PowerControl {
    /// `suspend` or `stop`.
    pub fn as_str(self) -> &'static str {
        match self {
            PowerControl::Suspend => "suspend",
            PowerControl::Stop => "stop",
        }
    }

    /// Parses `suspend` or `stop`.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "suspend" => Some(PowerControl::Suspend),
            "stop" => Some(PowerControl::Stop),
            _ => None,
        }
    }

    /// The state turning off leaves the sandbox in.
    pub fn off_state(self) -> PowerState {
        match self {
            PowerControl::Suspend => PowerState::Suspended,
            PowerControl::Stop => PowerState::Stopped,
        }
    }
}

/// Whether a sandbox is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerState {
    /// On.
    Running,
    /// Suspended in memory.
    Suspended,
    /// Stopped, disk kept.
    Stopped,
}

impl PowerState {
    /// `running`, `suspended` or `stopped`.
    pub fn as_str(self) -> &'static str {
        match self {
            PowerState::Running => "running",
            PowerState::Suspended => "suspended",
            PowerState::Stopped => "stopped",
        }
    }

    /// Parses a state word (`paused` is `suspended`); `None` for others.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "running" => Some(PowerState::Running),
            "suspended" | "paused" => Some(PowerState::Suspended),
            "stopped" => Some(PowerState::Stopped),
            _ => None,
        }
    }

    /// Suspended or stopped.
    pub fn is_off(self) -> bool {
        self != PowerState::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_round_trip() {
        for c in [PowerControl::Suspend, PowerControl::Stop] {
            assert_eq!(PowerControl::parse(c.as_str()), Some(c));
        }
        for s in [
            PowerState::Running,
            PowerState::Suspended,
            PowerState::Stopped,
        ] {
            assert_eq!(PowerState::parse(s.as_str()), Some(s));
        }
        assert_eq!(PowerState::parse("paused"), Some(PowerState::Suspended));
        assert_eq!(PowerControl::parse("pause"), None);
        assert_eq!(PowerControl::Suspend.off_state(), PowerState::Suspended);
        assert_eq!(PowerControl::Stop.off_state(), PowerState::Stopped);
    }
}
