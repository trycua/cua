//! Coding agents inside a Space: [`cua_agents`] over the Space's spacesd.
//!
//! A run is a detached, tagged spacesd process driving the harness over the
//! Agent Client Protocol; every fact about it is a file in the Space
//! (`~/.cua/agents/<run>/`), so a fresh server lists and continues runs it
//! never started. Host credential files are never copied: API keys reach a
//! run only as explicit per-run env (`RunOptions::env`, or on the MCP
//! surface `env_from_host` naming known provider key variables), written
//! 0600 inside the run and redacted from its event log. Moving a signed-in
//! agent session is teleport's job, behind its consent gate.

use crate::error::{Error, Result};
use crate::space::Space;
pub use cua_agents::{
    AgentEvent, Agents, Artifact, Attachment, Endpoint, EventPage, McpServer, RunInfo, RunOptions,
    RunResult, RunStatus, Started, Transcript, TranscriptItem, Wire, events, harness, installables,
    quote, runs, transcript,
};

impl From<cua_agents::Error> for Error {
    fn from(e: cua_agents::Error) -> Self {
        match e {
            cua_agents::Error::Invalid(m) => Error::InvalidArgument(m),
            cua_agents::Error::NotFound(m) => Error::NotFound(m),
            cua_agents::Error::Timeout(m) => Error::Timeout(m),
            cua_agents::Error::Client(c) => Error::Env(c),
            cua_agents::Error::Json(j) => Error::Json(j),
            other => Error::Agent(other.to_string()),
        }
    }
}

impl Space {
    /// The agent runner for this Space. Runs get the Space's own MCP (the
    /// spacesd's `/mcp`, cua-driver tools) unless they opt out.
    pub async fn agents(&self) -> Result<Agents> {
        if self.is_windows() {
            return Err(Error::Agent(
                "agent runs need a POSIX shell in the Space; Windows guests are not wired".into(),
            ));
        }
        let guest = self.spacesd()?.clone();
        let agents = Agents::new(guest).await?;
        Ok(match self.env_token() {
            Some(t) => agents.with_guest_mcp(Some(t.to_string())).await,
            None => agents,
        })
    }
}

/// Agent-run telemetry: the host install that starts a run records it
/// (`cua_agent_run_started`, then `cua_agent_run_completed` once, and
/// `first_agent_run`); see [`cua_telemetry::agent_runs`]. Only fixed words
/// leave: the harness id, the location word of the Space id, the entry
/// point, an outcome and an error category.
pub mod telemetry {
    use super::{Agents, RunInfo, Started};
    use crate::error::Error;
    use cua_telemetry::Outcome;
    use std::time::Duration;

    /// How long a watcher follows a run before leaving its end to the next
    /// status read.
    pub const WATCH_FOR: Duration = Duration::from_secs(2 * 3600);

    /// The error variant of `e` (never its message).
    pub fn error_variant(e: &Error) -> &'static str {
        match e {
            Error::InvalidArgument(_) => "InvalidArgument",
            Error::NotFound(_) => "NotFound",
            Error::SpacesdNotAvailable { .. } => "SpacesdNotAvailable",
            Error::CapabilityMissing { .. } => "CapabilityMissing",
            Error::HostCapabilityMissing { .. } => "HostCapabilityMissing",
            Error::Agent(_) => "Agent",
            Error::Timeout(_) => "Timeout",
            Error::Env(_) => "Env",
            Error::CloudClosed => "CloudClosed",
            Error::Cancelled(_) => "Cancelled",
            _ => "Other",
        }
    }

    /// A run was started from `entry` in the Space `space` (its id, such as
    /// `local:dev`; only the location word is kept). In a process with a
    /// Tokio runtime a watcher records its end.
    pub fn started(agents: &Agents, space: &str, entry: &str, s: &Started) {
        let t = cua_telemetry::global();
        t.agent_run_started(&s.run_id, &s.harness, space, entry);
        if t.agent_run_pending(&s.run_id) {
            watch(agents.clone(), s.run_id.clone());
        }
    }

    /// Starting a run failed.
    pub fn start_failed(harness: &str, space: &str, entry: &str, e: &Error) {
        cua_telemetry::global().agent_run_start_failed(
            harness,
            space,
            entry,
            Some(error_variant(e)),
        );
    }

    /// A run's status was read: records its end if this install started it
    /// and it has ended.
    pub fn observe(info: &RunInfo) {
        if let Some((outcome, variant)) = cua_telemetry::events::agent_run_end(
            info.status.as_str(),
            info.stop_reason.as_deref(),
            info.error.is_some(),
        ) {
            cua_telemetry::global().agent_run_finished(&info.run_id, outcome, variant);
        }
    }

    /// Before stopping `run_id`: records an end it already reached, so the
    /// stop is not taken for a cancel.
    pub async fn before_stop(agents: &Agents, run_id: &str) {
        if cua_telemetry::global().agent_run_pending(run_id)
            && let Ok(info) = agents.status(run_id).await
        {
            observe(&info);
        }
    }

    /// A run was stopped before its first turn ended (if it had ended, its
    /// end is already recorded and this does nothing).
    pub fn stopped(run_id: &str) {
        cua_telemetry::global().agent_run_finished(run_id, Outcome::Cancelled, None);
    }

    /// Follows `run_id` in the background until it ends (at most
    /// [`WATCH_FOR`]), polling gently. A no-op without a Tokio runtime or
    /// when nothing is pending; a process that exits first leaves the end
    /// to the next status read of this install.
    pub fn watch(agents: Agents, run_id: String) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        rt.spawn(async move {
            let t = cua_telemetry::global();
            let deadline = tokio::time::Instant::now() + WATCH_FOR;
            let mut every = Duration::from_secs(3);
            while tokio::time::Instant::now() < deadline && t.agent_run_pending(&run_id) {
                tokio::time::sleep(every).await;
                every = (every * 2).min(Duration::from_secs(30));
                match agents.status(&run_id).await {
                    Ok(info) => observe(&info),
                    // The run directory is gone (removed): nothing to wait for.
                    Err(cua_agents::Error::NotFound(_)) => {
                        stopped(&run_id);
                        return;
                    }
                    Err(_) => {}
                }
            }
        });
    }
}

/// `agent_capabilities`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CapabilitiesReport {
    pub protocol: &'static str,
    pub statuses: Vec<&'static str>,
    pub event_kinds: &'static [&'static str],
    pub harnesses: Vec<harness::HarnessInfo>,
    /// Host env variables `env_from_host` may name.
    pub forwardable_env: Vec<&'static str>,
    pub runner: &'static str,
}

/// What every harness is, can and cannot do.
pub fn capabilities() -> CapabilitiesReport {
    CapabilitiesReport {
        protocol: "Agent Client Protocol (agentclientprotocol.com), normalized to event kinds",
        statuses: vec!["running", "idle", "failed", "crashed", "unknown"],
        event_kinds: events::KINDS,
        harnesses: harness::HARNESSES.iter().map(|h| h.info()).collect(),
        forwardable_env: forwardable_env(),
        runner: "a detached, tagged cua-spacesd process per run; state in ~/.cua/agents/<run> \
                 in the Space, so runs survive disconnects and server restarts",
    }
}

/// The provider key variables a host may forward to a run: every
/// harness's documented key names, nothing else.
pub fn forwardable_env() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = harness::HARNESSES
        .iter()
        .flat_map(|h| h.keys.iter().copied())
        .chain(["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"])
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Resolves `names` from this process's environment, refusing anything
/// that is not a known provider key variable. Missing ones are reported,
/// never guessed.
pub fn env_from_host(
    names: &[String],
) -> Result<(std::collections::BTreeMap<String, String>, Vec<String>)> {
    let allowed = forwardable_env();
    let mut env = std::collections::BTreeMap::new();
    let mut missing = vec![];
    for n in names {
        if !allowed.contains(&n.as_str()) {
            return Err(Error::InvalidArgument(format!(
                "{n} is not a provider key variable this server forwards; allowed: {}",
                allowed.join(", ")
            )));
        }
        match std::env::var(n).ok().filter(|v| !v.is_empty()) {
            Some(v) => {
                env.insert(n.clone(), v);
            }
            None => missing.push(n.clone()),
        }
    }
    Ok((env, missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_cover_every_contract_agent() {
        let c = capabilities();
        let mut ids: Vec<&str> = c.harnesses.iter().map(|h| h.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, cua_spaces_contract::inputs::AGENT_IDS);
    }

    #[test]
    fn only_provider_keys_are_forwarded() {
        assert!(env_from_host(&["HOME".into()]).is_err());
        assert!(env_from_host(&["AWS_SECRET_ACCESS_KEY".into()]).is_err());
        let (env, missing) = env_from_host(&["CUA_TEST_NOT_A_KEY_ANTHROPIC"
            .replace("CUA_TEST_NOT_A_KEY_", "")
            + "_API_KEY"])
        .unwrap();
        assert!(env.len() + missing.len() == 1);
    }
}
