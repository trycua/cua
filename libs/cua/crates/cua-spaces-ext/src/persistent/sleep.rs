// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Sleep for agent Spaces that put themselves to sleep.
//!
//! A Space whose sleep setting is not `never` ([`cua_spaces::sleep`]) is
//! stopped once nothing in it has been running for the setting's quiet
//! window, and started again by the next message or task for one of its
//! agents. "Nothing running" is all of:
//!
//! - no agent of the Space is on `standby` or durable, has a task in flight,
//!   or has a run whose harness is still alive (an idle harness exits on its
//!   own after `AUTO_IDLE_EXIT_SECS`, so a parked agent costs a minute, not
//!   half an hour);
//! - no process of its own in the guest: nothing started after the Space
//!   booted (an unknown answer counts as busy, nothing is ever killed);
//! - no file the agent left in its home folder outside the Cua Volume, which
//!   a stop would lose (the root file system of a container lives in memory).
//!
//! The Space is checked at most every [`CHECK_EVERY`], and only Spaces with
//! an agent are looked at.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use cua_spaces::sleep::Sleep;
use cua_spacesd_client::Command;

use super::{AgentRecord, Persistent, RunStatus, TickReport};
use crate::Result;
use crate::SpacesDrive as _;

/// How often one Space is looked at.
pub(super) const CHECK_EVERY: Duration = Duration::from_secs(10);

/// A process or file younger than this much after the Space booted is the
/// Space's own start-up (desktop, helpers); anything started later is the
/// agent's or the person's.
const BOOT_WINDOW_SECS: u64 = 30;

/// What the host remembers about one sleeping-capable Space.
#[derive(Default)]
pub(super) struct Watch {
    /// Since when every check found it quiet.
    quiet_since: Option<Instant>,
    /// When it was last looked at.
    checked: Option<Instant>,
    /// Why the last check found it busy, logged when it changes.
    reason: Option<String>,
}

/// Prints `procs=<n> files=<n> up=<secs>`: processes that are not the
/// Space's own. The Space's own are its start-up (anything started within
/// the first seconds) and the desktop's helpers (by name, unless started
/// through cua-spacesd, which is how every run, exec and job starts): under
/// load a desktop starts well after the first 30 s. The probe's own
/// processes live under 2 s and are skipped, and files changed in the home folder since boot
/// (hidden folders hold tool state and are skipped).
const PROBE: &str = r#"
up=$(ps -o etimes= -p 1 2>/dev/null | tr -d ' ')
[ -n "$up" ] || { echo "procs=? files=? up=?"; exit 0; }
me=$$
procs=$(ps -eo pid=,ppid=,etimes=,comm= | awk -v up="$up" -v w=__WINDOW__ -v me="$me" '{ids[NR]=$1; pp[$1]=$2; et[$1]=$3; cm[$1]=$4}
END{for(i=1;i<=NR;i++){p=ids[i]; if(p==me||pp[p]==me||et[p]<2) continue; under=0; q=p; d=0
 while(q>1 && d<64){ if(cm[q]=="cua-spacesd"){under=1;break}; q=pp[q]; d++ }
 boot=(et[p]>=up-w); sys=(cm[p] ~ /^(xfce|xfwm|xfdesktop|xfsettingsd|Thunar|panel-|wrapper-2|dbus|pipewire|wireplumber|at-spi|dconf|gpg-agent|Xvfb|pulse)/)
 if(!under && (boot||sys)) continue
 if(under && boot) continue
 c++ } print c+0 }')
files=$(find "$HOME" -xdev -name '.*' -prune -o -type f -mmin -$((up/60+1)) -print 2>/dev/null | head -n 1 | wc -l | tr -d ' ')
echo "procs=$procs files=$files up=$up"
"#;

/// What [`PROBE`] found.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Probe {
    pub procs: u64,
    pub files: u64,
    pub up: u64,
}

pub(super) fn parse_probe(out: &str) -> Option<Probe> {
    let field = |k: &str| -> Option<u64> {
        out.split_whitespace()
            .find_map(|w| w.strip_prefix(k))
            .and_then(|v| v.strip_prefix('='))
            .and_then(|v| v.parse().ok())
    };
    Some(Probe {
        procs: field("procs")?,
        files: field("files")?,
        up: field("up")?,
    })
}

impl Probe {
    /// Nothing the agent or person left behind, and the Space is past its
    /// own start-up.
    pub(super) fn quiet(&self) -> bool {
        self.procs == 0 && self.files == 0 && self.up > BOOT_WINDOW_SECS + 10
    }
}

/// Whether the supervisor leaves `rec` alone because its Space is stopped:
/// a durable member waits for the Space to run again, and so does any member
/// of a Space the host put to sleep (its run is gone with the Space).
pub(super) fn waits_for_space(
    rec: &AgentRecord,
    off: &BTreeSet<String>,
    asleep: &BTreeSet<String>,
) -> bool {
    off.contains(&rec.space) && (rec.policy.durable || asleep.contains(&rec.space))
}

impl Persistent {
    /// Stopped Spaces that sleep on their own.
    pub(super) fn asleep_spaces(&self) -> BTreeSet<String> {
        self.spaces
            .list()
            .map(|l| {
                l.into_iter()
                    .filter(|s| {
                        super::space_is_off(&s.power_state)
                            && Sleep::from_labels(&s.labels).sleeps()
                    })
                    .map(|s| s.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How long a run of `rec` waits for its next message: a week on
    /// standby, a minute in a Space that sleeps on its own, and the usual
    /// half hour (`None`) in a Space its person keeps running.
    pub(super) fn idle_exit_for(&self, rec: &AgentRecord) -> Option<u32> {
        if rec.policy.standby {
            return Some(7 * 24 * 3600);
        }
        self.spaces
            .sleep_of(&rec.space)
            .sleeps()
            .then_some(cua_spaces::sleep::AUTO_IDLE_EXIT_SECS)
    }

    /// Starts `space` when the host put it to sleep, before something is
    /// sent into it. A Space someone stopped is left as it is.
    pub(crate) async fn wake_space(&self, space: &str) -> Result<()> {
        let (_, state) = self.spaces.power_of(space);
        if state != "stopped" || !self.spaces.sleep_of(space).sleeps() {
            return Ok(());
        }
        tracing::info!(space, "waking a Space that was asleep");
        self.spaces.start(space).await?;
        Ok(())
    }

    /// One pass over the Spaces that sleep on their own.
    pub(super) async fn tick_sleep(&self, _report: &mut TickReport) {
        let Ok(rows) = self.spaces.list() else { return };
        let Ok(records) = self.list() else { return };
        let now = Instant::now();
        let mut seen = BTreeSet::new();
        for row in rows {
            let sleep = Sleep::from_labels(&row.labels);
            if !row.id.starts_with("local:") || !sleep.sleeps() {
                continue;
            }
            seen.insert(row.id.clone());
            if row.power_state != "running" {
                self.live.lock().await.sleep.remove(&row.id);
                continue;
            }
            let members: Vec<&AgentRecord> = records.iter().filter(|r| r.space == row.id).collect();
            if members.is_empty() {
                continue;
            }
            // Cheap reasons first, from the records alone.
            let held = members.iter().find_map(|r| {
                let why = if r.paused {
                    "is paused"
                } else if r.relaunch {
                    "is waiting to be relaunched"
                } else if r.policy.standby {
                    "is on standby"
                } else if r.policy.durable {
                    "is durable"
                } else if r.session_views().iter().any(|s| s.task.is_some()) {
                    "has a task in flight"
                } else {
                    return None;
                };
                Some(format!("{} {why}", r.name))
            });
            let due = {
                let mut live = self.live.lock().await;
                let w = live.sleep.entry(row.id.clone()).or_default();
                if let Some(why) = &held {
                    w.quiet_since = None;
                    if w.reason.as_ref() != Some(why) {
                        tracing::info!(space = %row.id, "not asleep yet: {why}");
                        w.reason = Some(why.clone());
                    }
                }
                held.is_none()
                    && w.checked
                        .is_none_or(|t| now.duration_since(t) >= CHECK_EVERY)
            };
            if !due {
                continue;
            }
            let verdict = self.space_is_quiet(&row.id, &members).await;
            let quiet = verdict.is_ok();
            let ready = {
                let mut live = self.live.lock().await;
                let w = live.sleep.entry(row.id.clone()).or_default();
                w.checked = Some(Instant::now());
                let reason = verdict.as_ref().err().cloned();
                if reason != w.reason {
                    match &reason {
                        Some(r) => tracing::info!(space = %row.id, "not asleep yet: {r}"),
                        None => {
                            tracing::info!(space = %row.id, "nothing is running; asleep in {:?}", sleep.quiet_for())
                        }
                    }
                    w.reason = reason;
                }
                if quiet {
                    let since = *w.quiet_since.get_or_insert(now);
                    now.duration_since(since) >= sleep.quiet_for()
                } else {
                    w.quiet_since = None;
                    false
                }
            };
            if ready {
                if let Err(e) = self.sleep_space(&row.id, &members).await {
                    tracing::warn!(space = %row.id, "could not put the Space to sleep: {e}");
                }
                self.live.lock().await.sleep.remove(&row.id);
            }
        }
        self.live.lock().await.sleep.retain(|k, _| seen.contains(k));
    }

    /// `Ok` when nothing in `space` is running (see the module docs), else
    /// why it is busy. Anything it cannot read counts as busy.
    async fn space_is_quiet(
        &self,
        space: &str,
        members: &[&AgentRecord],
    ) -> std::result::Result<(), String> {
        let agents = self
            .cached_agents(space)
            .await
            .map_err(|e| format!("it does not answer: {e}"))?;
        for m in members {
            for s in m.session_views() {
                let Some(run) = &s.run_id else { continue };
                match agents.status(run).await {
                    Ok(st) if st.alive == Some(false) && st.status != RunStatus::Running => {}
                    Ok(st) => {
                        return Err(format!(
                            "{} has a run that is {} ({})",
                            m.name,
                            st.phase,
                            if st.alive == Some(true) {
                                "alive"
                            } else {
                                "unknown"
                            }
                        ));
                    }
                    Err(e) => return Err(format!("{}'s run cannot be read: {e}", m.name)),
                }
            }
        }
        let script = PROBE.replace("__WINDOW__", &BOOT_WINDOW_SECS.to_string());
        let out = agents
            .guest()
            .run(Command::shell(&script).timeout(Duration::from_secs(20)))
            .await
            .map_err(|e| format!("the probe failed: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        match parse_probe(&text) {
            Some(p) if p.quiet() => Ok(()),
            Some(p) => Err(format!(
                "{} process(es) and {} file(s) of its own, up {} s",
                p.procs, p.files, p.up
            )),
            None => Err(format!("the probe said {:?}", text.trim())),
        }
    }

    /// Stops `space` the way a person's stop would, after its agents let go
    /// of what they hold on the host (the Cua Volume lease and mount).
    async fn sleep_space(&self, space: &str, members: &[&AgentRecord]) -> Result<()> {
        tracing::info!(space, "putting a quiet Space to sleep");
        let mut by_name: HashMap<&str, ()> = HashMap::new();
        for m in members {
            if by_name.insert(m.name.as_str(), ()).is_some() {
                continue;
            }
            self.save(&m.name).await?;
            self.release_lease(&m.name).await;
            self.spaces.volume_release_agent(space, &m.name).await?;
        }
        self.spaces.volume_detach(space).await?;
        self.forget_space(space).await;
        self.spaces.sleep(space).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_answer_reads_and_decides() {
        let p = parse_probe("procs=0 files=0 up=95\n").unwrap();
        assert!(p.quiet());
        assert!(!parse_probe("procs=2 files=0 up=95").unwrap().quiet());
        assert!(!parse_probe("procs=0 files=1 up=95").unwrap().quiet());
        // Still inside its own start-up.
        assert!(!parse_probe("procs=0 files=0 up=20").unwrap().quiet());
        // A guest that could not answer is never quiet.
        assert_eq!(parse_probe("procs=? files=? up=?"), None);
        assert_eq!(parse_probe(""), None);
    }

    #[test]
    fn the_probe_script_names_its_window() {
        assert!(PROBE.contains("__WINDOW__"));
    }
}
