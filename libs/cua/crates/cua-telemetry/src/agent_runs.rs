//! Agent runs, recorded by the install that starts them.
//!
//! A run lives in a Space and outlives the process that started it, so its
//! end is seen later, by whichever process of this install next reads its
//! status (a watcher in the daemon or app, `agent_status`, `agent_list`,
//! `cua agent logs -f`, the SDK's `wait`). Starting a run leaves a small
//! marker in `$CUA_HOME/telemetry/agent_runs/<run id>` (harness id,
//! location word, entry point, start time: nothing else); the first process
//! to see the run end removes the marker and sends `cua_agent_run_completed`
//! once, and `first_agent_run` on this install's first run that ended
//! cleanly. Runs this install did not start have no marker and are never
//! counted here, so a run is counted once, by the host that started it.
//!
//! Inside an agent run (`CUA_AGENT_RUN_ID` set by the run's launcher) no
//! agent run is recorded: that environment is a Space, not an install.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::client::{Captured, Telemetry};
use crate::events::{self, Outcome};
use crate::{config, fsutil};

const DIR: &str = "agent_runs";
/// Markers of runs never seen ending are dropped after this long.
pub const MAX_PENDING_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Markers kept at most (the oldest go first).
pub const MAX_PENDING: usize = 200;

/// What a marker keeps: fixed words only, re-classified when read.
#[derive(Debug, Serialize, Deserialize)]
struct Pending {
    harness: String,
    location: String,
    entry: String,
    started_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A run id usable as a file name (`run-1a2b3c4d`).
fn valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Telemetry {
    /// This process runs inside a Cua agent run (in a Space).
    pub fn in_agent_run(&self) -> bool {
        self.env(config::ENV_IN_AGENT_RUN)
            .is_some_and(|v| !v.trim().is_empty())
    }

    fn pending_path(&self, run_id: &str) -> Option<PathBuf> {
        valid_run_id(run_id).then_some(())?;
        Some(self.state_dir()?.join(DIR).join(run_id))
    }

    /// A run was started on this install: sends `cua_agent_run_started` and
    /// keeps the marker its end is recorded against. `on` is a location
    /// word or a Space id; `entry` one of [`crate::schema::AGENT_ENTRIES`].
    pub fn agent_run_started(
        &self,
        run_id: &str,
        harness: &str,
        on: &str,
        entry: &str,
    ) -> Captured {
        if self.in_agent_run() {
            return Captured::Disabled;
        }
        if self.is_enabled()
            && let Some(path) = self.pending_path(run_id)
        {
            self.prune_pending();
            let p = Pending {
                harness: events::harness(harness).into(),
                location: events::location(on).into(),
                entry: events::agent_entry(entry).into(),
                started_ms: now_ms(),
            };
            if let Ok(bytes) = serde_json::to_vec(&p) {
                let _ = fsutil::write_replace(&path, &bytes);
            }
        }
        self.capture(events::agent_run_started(
            harness,
            on,
            entry,
            Outcome::Ok,
            None,
        ))
    }

    /// Starting a run failed: sends `cua_agent_run_started` with the error
    /// variant (never the message).
    pub fn agent_run_start_failed(
        &self,
        harness: &str,
        on: &str,
        entry: &str,
        error_variant: Option<&str>,
    ) -> Captured {
        if self.in_agent_run() {
            return Captured::Disabled;
        }
        self.capture(events::agent_run_started(
            harness,
            on,
            entry,
            Outcome::Error,
            Some(error_variant.unwrap_or("Other")),
        ))
    }

    /// Whether this install started `run_id` and has not seen it end yet.
    pub fn agent_run_pending(&self, run_id: &str) -> bool {
        self.pending_path(run_id).is_some_and(|p| p.exists())
    }

    /// A run ended (its first turn finished, or it failed, crashed or was
    /// stopped): sends `cua_agent_run_completed` once for a run this
    /// install started, and `first_agent_run` the first time one ends
    /// cleanly. Any other run (not started here, or already recorded)
    /// answers [`Captured::AlreadyRecorded`].
    pub fn agent_run_finished(
        &self,
        run_id: &str,
        outcome: Outcome,
        error_variant: Option<&str>,
    ) -> Captured {
        if self.in_agent_run() {
            return Captured::Disabled;
        }
        let Some(path) = self.pending_path(run_id) else {
            return Captured::AlreadyRecorded;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return Captured::AlreadyRecorded;
        };
        // Whoever removes the marker records the end: once, across
        // processes.
        if std::fs::remove_file(&path).is_err() {
            return Captured::AlreadyRecorded;
        }
        let Ok(p) = serde_json::from_slice::<Pending>(&bytes) else {
            return Captured::Invalid("unreadable agent run marker".into());
        };
        let elapsed = Duration::from_millis(now_ms().saturating_sub(p.started_ms));
        let r = self.capture(events::agent_run_completed(
            &p.harness,
            &p.location,
            &p.entry,
            outcome,
            error_variant,
            elapsed,
        ));
        if outcome == Outcome::Ok {
            self.capture_step("first_agent_run", outcome);
        }
        r
    }

    /// Drops markers older than [`MAX_PENDING_AGE`], and the oldest beyond
    /// [`MAX_PENDING`].
    fn prune_pending(&self) {
        let Some(dir) = self.state_dir().map(|d| d.join(DIR)) else {
            return;
        };
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return;
        };
        let now = SystemTime::now();
        let mut kept: Vec<(SystemTime, PathBuf)> = vec![];
        for e in rd.flatten() {
            let path = e.path();
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            if now.duration_since(modified).unwrap_or_default() > MAX_PENDING_AGE {
                let _ = std::fs::remove_file(&path);
            } else {
                kept.push((modified, path));
            }
        }
        if kept.len() >= MAX_PENDING {
            kept.sort();
            for (_, path) in &kept[..=kept.len() - MAX_PENDING] {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_file_names_only() {
        assert!(valid_run_id("run-1a2b3c4d"));
        assert!(!valid_run_id(""));
        assert!(!valid_run_id("../x"));
        assert!(!valid_run_id("a/b"));
        assert!(!valid_run_id(&"a".repeat(65)));
    }
}
