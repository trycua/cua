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

/// Why a sandbox stopped without being asked to, as its engine recorded
/// it (a container's exit code and `OOMKilled`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StopReason {
    /// The exit code (137: killed with SIGKILL).
    pub exit_code: i64,
    /// The kernel killed it for memory.
    #[serde(default)]
    pub out_of_memory: bool,
}

impl StopReason {
    /// The reason in plain words: "out of memory", "killed", "exited", or
    /// "exited with code N".
    pub fn words(&self) -> String {
        if self.out_of_memory {
            return "out of memory".into();
        }
        match self.exit_code {
            0 => "exited".into(),
            137 => "killed".into(),
            code => format!("exited with code {code}"),
        }
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

    #[test]
    fn a_stop_reason_is_said_in_plain_words() {
        let r = |exit_code, out_of_memory| StopReason {
            exit_code,
            out_of_memory,
        };
        assert_eq!(r(137, true).words(), "out of memory");
        assert_eq!(r(137, false).words(), "killed");
        assert_eq!(r(0, false).words(), "exited");
        assert_eq!(r(1, false).words(), "exited with code 1");
        // A record without the OOM flag (an older writer) reads as not OOM.
        let old: StopReason = serde_json::from_str(r#"{"exit_code":137}"#).unwrap();
        assert_eq!(old, r(137, false));
    }
}
