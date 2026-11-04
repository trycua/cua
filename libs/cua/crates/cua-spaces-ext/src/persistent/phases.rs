// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! How fast an agent joins a Space, step by step.
//!
//! A message for an agent that has no run yet starts one (a cold join); a
//! message for an idle run is a follow-up (a warm one). Either way the path
//! from the person's message to the agent's first words has steps: the
//! message is posted, the supervisor picks it up, the host prepares the
//! start, the runner is spawned in the guest, tools are installed, the
//! runner starts the agent, the agent initializes, authenticates and opens
//! a session, the turn starts, the first words arrive, the turn ends. Each
//! step is reported once as `cua_agent_join_phase` (only fixed words, a
//! per-install hash of the run and milliseconds since the message; see
//! `cua-telemetry`), so a slow join can be seen and fixed where it is slow.
//!
//! The host steps are timed here on the host clock. The guest steps are
//! read from the run's own `events.jsonl` (the runner timestamps each one in
//! the guest) and placed on the same line: the guest's first event is the
//! moment the runner was spawned. Install events carry whole seconds.
//!
//! This module also keeps two small registries that make joins fast:
//! the members being served right now (a member is served by one task at a
//! time), and the prewarms in flight (a start waits for the install a
//! prewarm already runs instead of repeating it).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cua_spaces::agents::Agents;

/// Longest one member's delivery may take inside the supervisor.
pub(crate) const DISPATCH_BUDGET: Duration = Duration::from_secs(60);
/// How long the steps of one join are followed.
const FOLLOW_FOR: Duration = Duration::from_secs(15 * 60);

pub(crate) fn now_ms() -> u64 {
    cua_volume::now_ms()
}

// --- one task per member ---------------------------------------------------

fn serving() -> &'static Mutex<HashSet<String>> {
    static S: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Held while a member's queued tasks are being served.
pub(crate) struct ServingGuard(String);

impl ServingGuard {
    /// `None` when `member` is already being served.
    pub(crate) fn take(dir: &Path, member: &str) -> Option<ServingGuard> {
        let key = format!("{}\u{0}{member}", dir.display());
        let taken = serving().lock().ok()?.insert(key.clone());
        taken.then(|| ServingGuard(key))
    }
}

impl Drop for ServingGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = serving().lock() {
            s.remove(&self.0);
        }
    }
}

// --- prewarm ---------------------------------------------------------------

fn prewarms() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static P: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

fn prewarm_key(dir: &Path, space: &str, harness: &str) -> String {
    format!("{}\u{0}{space}\u{0}{harness}", dir.display())
}

/// The lock a prewarm of `harness` in `space` holds while it runs.
pub(crate) fn prewarm_lock(dir: &Path, space: &str, harness: &str) -> Arc<tokio::sync::Mutex<()>> {
    prewarms()
        .lock()
        .expect("prewarms")
        .entry(prewarm_key(dir, space, harness))
        .or_default()
        .clone()
}

/// Waits (up to `max`) for a prewarm of `harness` in `space` that is running
/// now, so a start does not install what it is installing.
pub(crate) async fn wait_for_prewarm(dir: &Path, space: &str, harness: &str, max: Duration) {
    let lock = prewarm_lock(dir, space, harness);
    let _ = tokio::time::timeout(max, lock.lock()).await;
}

// --- the message a join answers ---------------------------------------------

/// What a cold start needs to know about the message that woke the agent.
#[derive(Clone)]
pub(crate) struct Woke {
    /// When the message was posted (host clock, unix ms).
    pub posted_ms: u64,
    /// When the supervisor picked it up.
    pub picked_ms: u64,
}

fn wakes() -> &'static Mutex<HashMap<String, Woke>> {
    static W: OnceLock<Mutex<HashMap<String, Woke>>> = OnceLock::new();
    W.get_or_init(Default::default)
}

fn wake_key(dir: &Path, member: &str) -> String {
    format!("{}\u{0}{member}", dir.display())
}

/// Remembers the message `member`'s next start answers.
pub(crate) fn set_woke(dir: &Path, member: &str, w: Woke) {
    if let Ok(mut m) = wakes().lock() {
        m.insert(wake_key(dir, member), w);
    }
}

/// Takes the message `member`'s start answers, if one was remembered.
pub(crate) fn take_woke(dir: &Path, member: &str) -> Option<Woke> {
    wakes().lock().ok()?.remove(&wake_key(dir, member))
}

// --- the steps --------------------------------------------------------------

/// What a join is, for the events it sends.
#[derive(Clone)]
pub(crate) struct Join {
    pub run_id: String,
    pub harness: String,
    /// One of `cua_telemetry::schema::JOIN_RUNTIMES`.
    pub runtime: String,
    pub cold: bool,
    /// The message (host clock, unix ms).
    pub posted_ms: u64,
}

impl Join {
    fn send(&self, phase: &str, ms: u64, tool: &str, failure: &str) {
        cua_telemetry::global().agent_join_phase(
            &self.run_id,
            phase,
            &self.harness,
            &self.runtime,
            self.cold,
            ms,
            tool,
            failure,
        );
    }
}

/// The runtime word of a Space from what its handshake reported.
pub(crate) fn runtime_word(os: &str, kind: &str) -> &'static str {
    match (os, kind) {
        ("linux", "container") => "linux_container",
        ("linux", "vm") => "linux_vm",
        ("macos", _) => "macos_vm",
        ("windows", _) => "windows_vm",
        _ => "other",
    }
}

/// The failure word for an error line the runner wrote.
pub(crate) fn failure_word(text: &str) -> &'static str {
    let t = text.to_ascii_lowercase();
    if cua_agents::failure::is_auth_failure(text) || t.contains("authentication") {
        "auth"
    } else if t.contains("timed out") || t.contains("timeout") {
        "timeout"
    } else if t.contains("install") {
        "install"
    } else {
        "other"
    }
}

/// A start that failed before it had a run: the `failed` step, with what it
/// failed on (`key` stands for the run: a start with no run has no id).
pub(crate) fn start_failed(key: &str, harness: &str, runtime: &str, posted_ms: u64, error: &str) {
    cua_telemetry::global().agent_join_phase(
        key,
        "failed",
        harness,
        runtime,
        true,
        now_ms().saturating_sub(posted_ms),
        "none",
        failure_word(error),
    );
}

/// Follows `join`'s run and reports each step as it is reached, until the
/// turn ends, the run fails or exits, or [`FOLLOW_FOR`] passes. `marks` are
/// the host steps already passed (name, ms since the message);
/// `guest_spawned_ms` is when the runner was spawned (ms since the message).
/// The "woke up and joined" note is the supervisor's (it reads the same
/// events when the agent is up); this only reports timings.
pub(crate) fn follow(
    agents: Agents,
    join: Join,
    marks: Vec<(&'static str, u64)>,
    guest_spawned_ms: u64,
) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    rt.spawn(async move {
        for (phase, ms) in &marks {
            join.send(phase, *ms, "none", "none");
        }
        let started = tokio::time::Instant::now();
        let mut cursor = 0u64;
        let mut anchor: Option<u64> = None;
        let mut turn_open = false;
        let mut seen_first_token = false;
        let mut installed: HashSet<String> = HashSet::new();
        let mut reported: HashSet<&'static str> = HashSet::new();
        let mut every = Duration::from_millis(200);
        while started.elapsed() < FOLLOW_FOR {
            let page = match agents.events(&join.run_id, cursor, 200).await {
                Ok(p) => p,
                Err(cua_agents::Error::NotFound(_)) => return,
                Err(_) => {
                    tokio::time::sleep(every).await;
                    continue;
                }
            };
            cursor = page.cursor;
            for ev in &page.events {
                let ts = ev.ts_ms;
                // A follow-up reads a run that has history: only what the
                // guest wrote since the message counts (a few seconds of
                // clock difference between host and guest are forgiven).
                if !join.cold && ts + 5_000 < join.posted_ms {
                    continue;
                }
                // A cold run's steps are placed from the runner's spawn; a
                // warm one's by the shared clock.
                let ms = if join.cold {
                    let a = *anchor.get_or_insert(ts);
                    guest_spawned_ms + ts.saturating_sub(a)
                } else {
                    ts.saturating_sub(join.posted_ms)
                };
                let ty = ev.raw.get("type").and_then(|t| t.as_str()).unwrap_or("");
                let once = |name: &'static str, reported: &mut HashSet<&'static str>| {
                    reported.insert(name)
                };
                match ty {
                    "install" => {
                        let id = ev.raw.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let phase = ev.raw.get("phase").and_then(|v| v.as_str()).unwrap_or("");
                        if matches!(phase, "done" | "cached") && installed.insert(id.to_string()) {
                            join.send("install_done", ms, id, "none");
                        } else if phase == "error" {
                            join.send("failed", ms, "none", "install");
                            return;
                        }
                    }
                    "runner_started" if once("runner_started", &mut reported) => {
                        join.send("runner_started", ms, "none", "none")
                    }
                    "agent_spawned" if once("agent_spawned", &mut reported) => {
                        join.send("agent_spawned", ms, "none", "none")
                    }
                    "initialized" if once("acp_initialized", &mut reported) => {
                        join.send("acp_initialized", ms, "none", "none")
                    }
                    "authenticated" if once("acp_authenticated", &mut reported) => {
                        join.send("acp_authenticated", ms, "none", "none")
                    }
                    "session" if once("session_ready", &mut reported) => {
                        join.send("session_ready", ms, "none", "none");
                    }
                    "turn_started" => {
                        turn_open = true;
                        seen_first_token = false;
                        if once("turn_started", &mut reported) {
                            join.send("turn_started", ms, "none", "none");
                        }
                    }
                    "update" if turn_open && !seen_first_token && ev.kind == "message" => {
                        seen_first_token = true;
                        join.send("first_token", ms, "none", "none");
                    }
                    "turn_ended" if turn_open => {
                        join.send("turn_ended", ms, "none", "none");
                        return;
                    }
                    "error" => {
                        let text = ev.text.as_deref().unwrap_or("");
                        join.send("failed", ms, "none", failure_word(text));
                        return;
                    }
                    "run_exited" => return,
                    _ => {}
                }
            }
            if page.caught_up {
                tokio::time::sleep(every).await;
                if started.elapsed() > Duration::from_secs(30) {
                    every = Duration::from_secs(1);
                }
            }
        }
        join.send("failed", FOLLOW_FOR.as_millis() as u64, "none", "timeout");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_task_serves_a_member_at_a_time() {
        let dir = Path::new("/tmp/phases-test");
        let a = ServingGuard::take(dir, "claude").expect("first");
        assert!(ServingGuard::take(dir, "claude").is_none());
        assert!(ServingGuard::take(dir, "hermes").is_some());
        drop(a);
        assert!(ServingGuard::take(dir, "claude").is_some());
    }

    #[test]
    fn runtime_and_failure_words() {
        assert_eq!(runtime_word("linux", "container"), "linux_container");
        assert_eq!(runtime_word("macos", "vm"), "macos_vm");
        assert_eq!(runtime_word("", ""), "other");
        assert_eq!(failure_word("Authentication required"), "auth");
        assert_eq!(failure_word("initialize timed out"), "timeout");
        assert_eq!(failure_word("npm install failed"), "install");
        assert_eq!(failure_word("boom"), "other");
    }
}
