//! Moving a macOS host from the hand-written LaunchAgent (`cua host setup`
//! without the app, Cua Spaces 0.9.4 and earlier: `~/.cua/host/bin/cua-spacesd`
//! run by `~/Library/LaunchAgents/com.trycua.spacesd.host.plist`) to the Cua
//! Spaces app's own login item, "Cua Spaces Helper"
//! ([`crate::service::AppHelper`]).
//!
//! The move keeps everything that makes the machine this machine: the host
//! config, the driver policy, the machine id, the machine token, the env
//! token, the Spaces it provides and Cua Volume. It never registers again.
//! It stops the old agent, starts the helper, and checks that the helper
//! keeps running and (in relay mode, when the old one was online) comes
//! online at the relay with a new session; only then does it delete the old
//! plist and binary. Anything else puts the old agent back as it was
//! ([`Host::migrate_service`](crate::Host::migrate_service)).
//!
//! [`plan`] is the decision, a pure function of [`MigrationFacts`], so every
//! case is unit tested; [`LegacyService`] is the seam over the old agent.

use crate::Result;
use crate::service::{Launchd, LegacyAgent, RunnerKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `<host dir>/helper-migration.json`: the last move, so a failed one is not
/// tried again by itself with the same app version.
pub const RECORD_FILE: &str = "helper-migration.json";
/// Where the old agent's plist waits while the helper is checked.
pub const LEGACY_PLIST_BACKUP: &str = "legacy-agent.plist";

/// The last move ([`RECORD_FILE`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRecord {
    /// `in-progress`, `done` or `rolled-back`.
    pub state: String,
    /// The app version that tried it.
    pub app_version: String,
    /// When (epoch ms).
    pub at_ms: u64,
    /// Why it was rolled back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What [`plan`] decides from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationFacts {
    /// A Cua Spaces app with the helper is installed.
    pub helper_available: bool,
    /// This cua home has a host config.
    pub configured: bool,
    /// The config's runner (`None`: not configured, or a name this build
    /// does not know).
    pub runner: Option<RunnerKind>,
    /// Relay sharing is paused while signed out (the service is
    /// uninstalled on purpose).
    pub paused: bool,
    /// What is left of the old LaunchAgent.
    pub legacy: LegacyAgent,
    /// `<host dir>/bin/cua-spacesd` exists.
    pub legacy_binary: bool,
    /// The last move.
    pub last: Option<MigrationRecord>,
    /// This app's version.
    pub app_version: String,
    /// Try again even after a failed move with this version
    /// (`cua host migrate --force`).
    pub force: bool,
}

/// What to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationPlan {
    /// Nothing (why, in words).
    Nothing(&'static str),
    /// The helper is in use; remove what is left of the old agent.
    RemoveLeftovers,
    /// The service is uninstalled while signed out: switch the runner and
    /// remove the old agent; resuming starts the helper.
    SwitchPaused,
    /// Stop the old agent, start the helper, check it, keep it or roll
    /// back.
    Move,
    /// A move failed with this app version: keep the old agent.
    KeepAfterFailure,
}

/// The decision. A host that is not set up in this cua home is left alone,
/// even with an old agent around: the agent may belong to another cua home
/// (the staging channel's), and only `cua host remove` there may remove it.
pub fn plan(f: &MigrationFacts) -> MigrationPlan {
    if !f.helper_available {
        return MigrationPlan::Nothing("no Cua Spaces app with its helper is installed");
    }
    if !f.configured {
        return MigrationPlan::Nothing("this machine is not set up for access");
    }
    match f.runner {
        Some(RunnerKind::AppHelper) => {
            if f.legacy.present() || f.legacy_binary {
                MigrationPlan::RemoveLeftovers
            } else {
                MigrationPlan::Nothing("the host service already runs as Cua Spaces Helper")
            }
        }
        Some(RunnerKind::Launchd) => {
            if f.paused {
                return MigrationPlan::SwitchPaused;
            }
            let failed_here = f
                .last
                .as_ref()
                .is_some_and(|r| r.state == "rolled-back" && r.app_version == f.app_version);
            if failed_here && !f.force {
                MigrationPlan::KeepAfterFailure
            } else {
                MigrationPlan::Move
            }
        }
        _ => MigrationPlan::Nothing("the host service does not run from a LaunchAgent"),
    }
}

/// What [`Host::migrate_service`](crate::Host::migrate_service) did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceMigration {
    /// `migrated`, `switched`, `cleaned-up`, `rolled-back`, `kept` or
    /// `not-needed`.
    pub outcome: String,
    /// In words.
    pub detail: String,
    /// macOS will ask for Screen Recording and Accessibility for Cua Spaces
    /// Helper (this machine shares its desktop and they are not granted
    /// yet).
    pub permissions_needed: bool,
}

impl ServiceMigration {
    pub(crate) fn new(outcome: &str, detail: impl Into<String>, permissions_needed: bool) -> Self {
        Self {
            outcome: outcome.into(),
            detail: detail.into(),
            permissions_needed,
        }
    }
}

/// The old LaunchAgent, as the move needs it.
pub trait LegacyService: Send + Sync {
    /// What is left of it.
    fn found(&self) -> LegacyAgent;
    /// Its plist (`~/Library/LaunchAgents/com.trycua.spacesd.host.plist`).
    fn plist(&self) -> PathBuf;
    /// Stops it and takes it out of launchd (the plist stays).
    fn bootout(&self) -> Result<()>;
    /// Loads and starts it from its plist again.
    fn bootstrap(&self) -> Result<()>;
}

impl LegacyService for Launchd {
    fn found(&self) -> LegacyAgent {
        self.legacy()
    }
    fn plist(&self) -> PathBuf {
        self.plist_file()
    }
    fn bootout(&self) -> Result<()> {
        Launchd::bootout(self)
    }
    fn bootstrap(&self) -> Result<()> {
        crate::ServiceManager::start(self)
    }
}

/// How long the helper has to prove itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifyTiming {
    /// Give up after this.
    pub timeout: std::time::Duration,
    /// It must run this long without a restart.
    pub stable: std::time::Duration,
    /// When the old agent was not online either, running this long is
    /// enough (the relay cannot tell more).
    pub offline_grace: std::time::Duration,
    /// Between checks.
    pub poll: std::time::Duration,
}

impl Default for VerifyTiming {
    fn default() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(45),
            stable: std::time::Duration::from_secs(5),
            offline_grace: std::time::Duration::from_secs(15),
            poll: std::time::Duration::from_millis(500),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> MigrationFacts {
        MigrationFacts {
            helper_available: true,
            configured: true,
            runner: Some(RunnerKind::Launchd),
            legacy: LegacyAgent {
                plist: true,
                loaded: true,
                running: true,
            },
            legacy_binary: true,
            app_version: "0.10.0".into(),
            ..Default::default()
        }
    }

    #[test]
    fn an_old_agent_with_the_app_installed_moves() {
        assert_eq!(plan(&facts()), MigrationPlan::Move);
        // Even when the old agent is gone or broken: the helper replaces it.
        let mut f = facts();
        f.legacy = LegacyAgent::default();
        f.legacy_binary = false;
        assert_eq!(plan(&f), MigrationPlan::Move);
    }

    #[test]
    fn without_the_app_nothing_changes() {
        let mut f = facts();
        f.helper_available = false;
        assert!(matches!(plan(&f), MigrationPlan::Nothing(_)));
    }

    #[test]
    fn a_machine_that_is_not_set_up_is_left_alone_even_with_an_old_agent() {
        let mut f = facts();
        f.configured = false;
        f.runner = None;
        assert!(matches!(plan(&f), MigrationPlan::Nothing(_)));
    }

    #[test]
    fn a_paused_host_switches_without_starting_anything() {
        let mut f = facts();
        f.paused = true;
        assert_eq!(plan(&f), MigrationPlan::SwitchPaused);
    }

    #[test]
    fn a_failed_move_is_not_retried_with_the_same_version_unless_forced() {
        let mut f = facts();
        f.last = Some(MigrationRecord {
            state: "rolled-back".into(),
            app_version: "0.10.0".into(),
            at_ms: 1,
            reason: Some("did not come online".into()),
        });
        assert_eq!(plan(&f), MigrationPlan::KeepAfterFailure);
        f.force = true;
        assert_eq!(plan(&f), MigrationPlan::Move);
        // The next app version tries again by itself.
        f.force = false;
        f.app_version = "0.10.1".into();
        assert_eq!(plan(&f), MigrationPlan::Move);
        // A move that was interrupted (in progress) is tried again.
        f.app_version = "0.10.0".into();
        f.last.as_mut().unwrap().state = "in-progress".into();
        assert_eq!(plan(&f), MigrationPlan::Move);
    }

    #[test]
    fn the_helper_in_use_removes_leftovers_or_does_nothing() {
        let mut f = facts();
        f.runner = Some(RunnerKind::AppHelper);
        assert_eq!(plan(&f), MigrationPlan::RemoveLeftovers);
        f.legacy = LegacyAgent::default();
        assert_eq!(
            plan(&f),
            MigrationPlan::RemoveLeftovers,
            "the old binary is left"
        );
        f.legacy_binary = false;
        assert!(matches!(plan(&f), MigrationPlan::Nothing(_)));
    }

    #[test]
    fn other_runners_are_not_moved() {
        for r in [
            None,
            Some(RunnerKind::Process),
            Some(RunnerKind::Systemd),
            Some(RunnerKind::WindowsTask),
        ] {
            let mut f = facts();
            f.runner = r;
            assert!(matches!(plan(&f), MigrationPlan::Nothing(_)), "{r:?}");
        }
    }
}
