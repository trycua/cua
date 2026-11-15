// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Waking an agent when a `space_job` ends.
//!
//! `space_job start` with `notify` registers the job here
//! (`watched-jobs.json`, the same shape as the watched runs). The supervisor
//! ([`Persistent::tick`], run by `cua daemon`) asks each Space for its job's
//! state. When a job has ended it:
//!
//! - gives the agent that started it (or the one named in `wake`) a task in
//!   the Space's thread, from the daemon, with the job's one-line result and
//!   the end of its log. The dispatcher delivers it as the agent's next
//!   turn, so the agent can end its turn and sleep while the job runs; and
//! - tells the person (the notifications feed) when there is no agent to
//!   wake.
//!
//! The file survives a daemon restart, and a Space that is off or does not
//! answer is asked again on the next pass, so a job is followed to its end
//! for as long as [`WATCH_JOBS_FOR`].

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{AGENT_TICK_BUDGET, Persistent, TickReport, locked_json, read_json};
use crate::thread::policy::Right;
use crate::thread::{Author, Draft, EntryKind, TaskTo};
use cua_spaces::extension::Caller;
use cua_spaces::space_jobs::{self, JobStatus};
use cua_spaces::{Error, Result};

/// How long a job is followed at most.
pub const WATCH_JOBS_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
/// Jobs followed at most.
const MAX_WATCHED_JOBS: usize = 200;
/// Bytes of a job's log in the message that wakes the agent.
pub const TAIL_BYTES: u64 = 2000;

/// A job whose end is waited for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedJob {
    /// The Space id.
    pub space: String,
    /// The job id.
    pub job: String,
    /// Its label.
    #[serde(default)]
    pub name: String,
    /// The agent member to give a task when it ends (none: tell the person).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Unix ms the watch began.
    pub added_ms: u64,
    /// Consecutive passes that could not reach the Space.
    #[serde(default)]
    pub misses: u32,
}

impl Persistent {
    fn watched_jobs_path(&self) -> PathBuf {
        self.dir.join("watched-jobs.json")
    }

    /// The jobs being followed.
    pub fn watched_jobs(&self) -> Result<Vec<WatchedJob>> {
        read_json(&self.watched_jobs_path())
    }

    /// Follows `job` of `space` until it ends. The agent to wake is `wake`
    /// (an agent member `caller` may task) or, without it, `caller` when it
    /// is an agent member of that Space.
    pub fn watch_job(
        &self,
        caller: &Caller,
        space: &str,
        job: &str,
        name: &str,
        wake: Option<&str>,
    ) -> Result<Value> {
        let agent = match wake.filter(|w| !w.is_empty()) {
            Some(w) => {
                let rec = self.get(w)?;
                if rec.space != space {
                    return Err(Error::invalid(format!("{w} is not a member of this Space")));
                }
                if !self.allows(caller, &rec, Right::Task) {
                    return Err(Error::invalid(format!("you can't give {w} tasks")));
                }
                Some(w.to_string())
            }
            None => caller
                .agent_member()
                .filter(|(_, s)| *s == space)
                .map(|(n, _)| n.to_string()),
        };
        let w = WatchedJob {
            space: space.into(),
            job: job.into(),
            name: name.into(),
            agent: agent.clone(),
            added_ms: cua_volume::now_ms(),
            misses: 0,
        };
        locked_json(&self.watched_jobs_path(), |all: &mut Vec<WatchedJob>| {
            if !all.iter().any(|x| x.job == w.job && x.space == w.space) {
                all.push(w);
            }
            let extra = all.len().saturating_sub(MAX_WATCHED_JOBS);
            all.drain(..extra);
        })?;
        Ok(json!({"watching": true, "wake": agent}))
    }

    /// One pass over the watched jobs.
    pub(super) async fn tick_watched_jobs(&self, report: &mut TickReport) {
        let jobs = match self.watched_jobs() {
            Ok(j) => j,
            Err(e) => {
                report.errors.push(format!("watched jobs: {e}"));
                return;
            }
        };
        if jobs.is_empty() {
            return;
        }
        let off = self.spaces_off();
        let mut next: Vec<(WatchedJob, Option<WatchedJob>)> = vec![];
        for w in jobs {
            let expired =
                cua_volume::now_ms().saturating_sub(w.added_ms) > WATCH_JOBS_FOR.as_millis() as u64;
            if expired {
                next.push((w, None));
                continue;
            }
            // A Space someone stopped is not woken to ask; its job is
            // asked about when it runs again.
            if off.contains(&w.space) {
                next.push((w.clone(), Some(w)));
                continue;
            }
            let after =
                match tokio::time::timeout(AGENT_TICK_BUDGET, self.tick_watched_job(&w, report))
                    .await
                {
                    Ok(Ok(done)) => (!done).then(|| WatchedJob {
                        misses: 0,
                        ..w.clone()
                    }),
                    Ok(Err(e)) => {
                        if matches!(e, Error::NotFound(_)) {
                            // The job's record is gone (pruned, or the Space
                            // was recreated): say so once and stop.
                            self.post_job_lost(&w, report);
                            None
                        } else {
                            self.forget_space(&w.space).await;
                            report.errors.push(format!("{}: {e}", w.job));
                            Some(WatchedJob {
                                misses: w.misses + 1,
                                ..w.clone()
                            })
                        }
                    }
                    Err(_) => {
                        report
                            .errors
                            .push(format!("{}: job check timed out", w.job));
                        Some(w.clone())
                    }
                };
            next.push((w, after));
        }
        let saved = locked_json(&self.watched_jobs_path(), |all: &mut Vec<WatchedJob>| {
            all.retain_mut(|x| {
                match next
                    .iter()
                    .find(|(w, _)| w.job == x.job && w.space == x.space)
                {
                    Some((_, Some(w))) => {
                        *x = w.clone();
                        true
                    }
                    Some((_, None)) => false,
                    None => true,
                }
            });
        });
        if let Err(e) = saved {
            report.errors.push(format!("watched jobs: {e}"));
        }
    }

    /// Asks about one job; `true` once it ended and was reported.
    async fn tick_watched_job(&self, w: &WatchedJob, report: &mut TickReport) -> Result<bool> {
        let space = self.spaces.space(&w.space).await?;
        let status = space.job_status(&w.job).await?;
        if !status.finished() {
            return Ok(false);
        }
        let tail = space
            .job_logs(&w.job, None, TAIL_BYTES, Duration::ZERO)
            .await
            .ok();
        self.post_job_end(
            w,
            &status,
            tail.as_ref().map(|l| (l.text.as_str(), l.next_offset)),
        )?;
        report.notified += 1;
        Ok(true)
    }

    /// The message that wakes the agent, or tells the person.
    pub fn post_job_end(
        &self,
        w: &WatchedJob,
        status: &JobStatus,
        tail: Option<(&str, u64)>,
    ) -> Result<()> {
        let text = end_message(status, tail);
        let store = self.thread_store(&w.space);
        let woke = match &w.agent {
            Some(agent) if self.get(agent).is_ok() => {
                let mut d = Draft::new(Author::system("jobs"), EntryKind::Message, text.clone());
                d.mentions = vec![agent.clone()];
                d.task = Some(TaskTo {
                    to: agent.clone(),
                    session: None,
                });
                store.append(d)?;
                true
            }
            _ => {
                store.append(Draft::note("jobs", None, text.clone()))?;
                false
            }
        };
        if !woke {
            let failed = status.state != "exited" || status.exit_code != Some(0);
            let label = if status.name.is_empty() {
                status.command.as_str()
            } else {
                status.name.as_str()
            };
            let title = format!(
                "{} {}",
                label.lines().next().unwrap_or("A job"),
                if failed { "failed" } else { "finished" }
            );
            self.feed().post(
                w.agent.as_deref(),
                if failed { "error" } else { "message" },
                &title,
                &space_jobs::result_line(status),
                None,
                Some(&w.space),
            )?;
        }
        Ok(())
    }

    fn post_job_lost(&self, w: &WatchedJob, report: &mut TickReport) {
        let status = JobStatus {
            job: w.job.clone(),
            name: w.name.clone(),
            state: "lost".into(),
            exit_code: None,
            signal: None,
            started_at: None,
            ended_at: None,
            elapsed_s: 0,
            size: 0,
            command: String::new(),
        };
        match self.post_job_end(w, &status, None) {
            Ok(()) => report.notified += 1,
            Err(e) => report.errors.push(format!("{}: {e}", w.job)),
        }
    }
}

/// The text of the message: the structured result line, the end of the log,
/// and what to do next.
pub fn end_message(status: &JobStatus, tail: Option<(&str, u64)>) -> String {
    let mut text = space_jobs::result_line(status);
    if let Some((log, next)) = tail {
        let log = log.trim_end();
        if !log.is_empty() {
            text += &format!("\n\nEnd of the log:\n{log}");
        }
        text += &format!(
            "\n\nRead more with space_job {{action: \"logs\", job: \"{}\", offset: <byte>}} (the log is {next} bytes).",
            status.job
        );
    } else {
        text += "\n\nThe Space no longer has this job's record (it was removed, or the Space was recreated), so its result is unknown: run it again if you still need it.";
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: &str, code: Option<i32>) -> JobStatus {
        JobStatus {
            job: "j1".into(),
            name: "unit tests".into(),
            state: state.into(),
            exit_code: code,
            signal: None,
            started_at: Some(1),
            ended_at: Some(61),
            elapsed_s: 60,
            size: 11,
            command: "cargo test".into(),
        }
    }

    #[test]
    fn the_message_starts_with_the_structured_line() {
        let m = end_message(&status("exited", Some(0)), Some(("ok\n", 11)));
        assert!(m.starts_with("JOB j1 exited exit=0 1m00s log=11B | unit tests\n"));
        assert!(m.contains("End of the log:\nok"));
        assert!(m.contains("offset: <byte>"));
        let lost = end_message(&status("lost", None), None);
        assert!(lost.contains("result is unknown"));
    }
}
