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
