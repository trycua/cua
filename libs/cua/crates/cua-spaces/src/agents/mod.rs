//! Coding agents inside a Space: [`cua_agents`] over the Space's spacesd.
//!
//! A run is a detached, tagged spacesd process driving the harness over the
//! Agent Client Protocol; every fact about it is a file in the Space
//! (`~/.cua/agents/<run>/`), so a fresh server lists and continues runs it
//! never started. Host credential files are never copied: API keys reach a
//! run only as explicit per-run env (`RunOptions::env`, or on the MCP
//! surface `env_from_host` naming known provider key variables), written
//! 0600 inside the run and redacted from its event log. Keys saved in Cua
//! Spaces (Settings → Connections, [`keys`]) reach only the runs whose harness
//! reads them ([`run_env`]). Moving a signed-in agent session is teleport's
//! job, behind its consent gate.

use crate::error::{Error, Result};
use crate::space::Space;
use std::collections::BTreeMap;

pub mod keys;
pub use cua_agents::{
    AgentEvent, Agents, Artifact, Attachment, Endpoint, EventPage, McpServer, RunInfo, RunOptions,
    RunResult, RunStatus, Started, Transcript, TranscriptItem, Wire, events, failure, harness,
    installables, quote, runs, transcript,
};

impl From<cua_agents::Error> for Error {
    fn from(e: cua_agents::Error) -> Self {
        match e {
            cua_agents::Error::Invalid(m) => Error::InvalidArgument(m),
            cua_agents::Error::NotFound(m) => Error::NotFound(m),
            cua_agents::Error::Timeout(m) => Error::Timeout(m),
            cua_agents::Error::Client(c) => Error::Env(c),
            cua_agents::Error::Json(j) => Error::Json(j),
            cua_agents::Error::NoCredential(m) => Error::Agent(m),
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
            info.error.as_deref(),
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

/// The extension operation `agent_start` calls with a run it started
/// (`{space, run_id, agent}`), so the daemon can tell the user when the run
/// or one of its turns fails.
pub const WATCH_OP: &str = "agent_start.watch";

/// The extension operation the run tools (`agent_message`, `agent_status`,
/// `agent_events`, `agent_interrupt`, `agent_stop`) call when given an agent
/// member's `name` instead of a `run_id`: `{tool, space, name, text?,
/// force?}`. It answers `{run_id}` (the member's current run), or, for
/// `agent_message`, delivers the message through the member and answers
/// the tool's result.
pub const MEMBER_OP: &str = "agent_member.run";

/// The run tools that take an agent member's `name`.
pub const MEMBER_RUN_TOOLS: &[&str] = &[
    "agent_message",
    "agent_status",
    "agent_events",
    "agent_interrupt",
    "agent_stop",
];

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
        harnesses: {
            let saved = keys::global().names();
            harness::HARNESSES
                .iter()
                .map(|h| h.info_with_host_key(host_key_with(h, &saved)))
                .collect()
        },
        forwardable_env: forwardable_env(),
        runner: "a detached, tagged cua-spacesd process per run; state in ~/.cua/agents/<run> \
                 in the Space, so runs survive disconnects and server restarts",
    }
}

/// The first of `h`'s keys this server can give a run: one saved in Cua
/// Spaces (Settings → Connections), or one set in this process's environment
/// (`env_from_host` forwards it).
pub fn host_key(h: &harness::Harness) -> Option<&'static str> {
    host_key_with(h, &keys::global().names())
}

/// [`host_key`] given the names of the saved keys.
pub fn host_key_with(h: &harness::Harness, saved: &[String]) -> Option<&'static str> {
    h.keys
        .iter()
        .copied()
        .find(|k| saved.iter().any(|s| s == k) || std::env::var(k).is_ok_and(|v| !v.is_empty()))
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

/// Resolves `names` from this process's environment, then from the keys
/// saved in Cua Spaces, refusing anything that is not a known provider key
/// variable or a saved key's name. Missing ones are reported, never
/// guessed.
pub fn env_from_host(names: &[String]) -> Result<(BTreeMap<String, String>, Vec<String>)> {
    env_from_host_with(keys::global(), names)
}

/// [`env_from_host`] over the given saved keys.
pub fn env_from_host_with(
    saved: &keys::AgentKeys,
    names: &[String],
) -> Result<(BTreeMap<String, String>, Vec<String>)> {
    let allowed = forwardable_env();
    let saved_names = if names.is_empty() {
        vec![]
    } else {
        saved.names()
    };
    let mut env = BTreeMap::new();
    let mut missing = vec![];
    for n in names {
        if !allowed.contains(&n.as_str()) && !saved_names.contains(n) {
            return Err(Error::InvalidArgument(format!(
                "{n} is not a provider key variable this server forwards; allowed: {} \
                 (or a key saved in Cua Spaces → Settings → Connections)",
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
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(String::as_str).collect();
        let found = saved.values(&names);
        missing.retain(|n| !found.contains_key(n));
        env.extend(found);
    }
    Ok((env, missing))
}

/// The provider keys a run of `harness` gets on top of `env` (the caller's
/// own) and `env_from_host`: every key saved in Cua Spaces that the
/// harness reads and `env` does not set already. Nothing is added for a
/// custom endpoint (a key would go to that endpoint), nor for a harness
/// that reads no key; an unknown harness gets nothing. Returns the env and
/// the `env_from_host` names that could not be found.
pub fn run_env(
    harness: &str,
    env: &BTreeMap<String, String>,
    env_from_host: &[String],
    custom_endpoint: bool,
) -> Result<(BTreeMap<String, String>, Vec<String>)> {
    run_env_with(keys::global(), harness, env, env_from_host, custom_endpoint)
}

/// [`run_env`] over the given saved keys.
pub fn run_env_with(
    saved: &keys::AgentKeys,
    harness: &str,
    env: &BTreeMap<String, String>,
    env_from_host: &[String],
    custom_endpoint: bool,
) -> Result<(BTreeMap<String, String>, Vec<String>)> {
    let (host_env, missing) = env_from_host_with(saved, env_from_host)?;
    // A copy of Claude Code's own sign-in that is running out is read again
    // from Claude Code first (never refreshed by Cua: [`keys::AgentKeys::sync_claude_login`]).
    if harness == "claude-code" {
        saved.sync_claude_login(&keys::HostClaudeLogin, true);
    }
    let mut out = env.clone();
    out.extend(host_env);
    if !custom_endpoint && harness::harness(harness).is_some() {
        // Every saved credential the app can use, by the registry's delivery
        // rules (a sign-in scoped to another app is never among them).
        let names = keys::registry::all_env_names();
        let saved_values = saved.values(&names);
        let delivered = keys::registry::deliver(harness, &|n| saved_values.get(n).cloned());
        for (dest, value) in delivered {
            if out.get(&dest).is_none_or(|v| v.is_empty()) {
                out.insert(dest, value);
            }
        }
    }
    oauth_token_as_api_key(harness, &mut out);
    Ok((out, missing))
}

/// A Claude token (`sk-ant-oat...`) in `ANTHROPIC_API_KEY` is a subscription
/// sign-in that was put where a key goes (typed there, or exported that way
/// in a shell). Claude Code gets it as the sign-in it is
/// (`CLAUDE_CODE_OAUTH_TOKEN`; an API key slot answers it with a 401); no
/// other harness gets it at all: the Claude subscription is Claude Code's alone.
fn oauth_token_as_api_key(harness: &str, env: &mut BTreeMap<String, String>) {
    const KEY: &str = "ANTHROPIC_API_KEY";
    const TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";
    if !env
        .get(KEY)
        .is_some_and(|v| v.trim().starts_with("sk-ant-oat"))
    {
        return;
    }
    let value = env.remove(KEY).unwrap_or_default();
    if harness == "claude-code" && env.get(TOKEN).is_none_or(|v| v.is_empty()) {
        env.insert(TOKEN.into(), value.trim().to_string());
    }
}

/// What the person's connections add to a run beyond its env: an endpoint for
/// an app that has no variable of its own for the connected provider, and the
/// config Hermes starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionPlan {
    /// Variables to add (the endpoint's key under the name the app reads it as).
    pub env: BTreeMap<String, String>,
    pub endpoint: Option<Endpoint>,
    /// Hermes's generated `config.yaml`.
    pub hermes_config: Option<String>,
}

/// The [`ConnectionPlan`] for a run of `harness` whose env is `env` (after
/// [`run_env_with`]). Nothing for a run with a custom endpoint of its own.
pub fn connection_plan(
    saved: &keys::AgentKeys,
    harness: &str,
    env: &BTreeMap<String, String>,
    custom_endpoint: bool,
) -> ConnectionPlan {
    use keys::registry as reg;
    let mut plan = ConnectionPlan::default();
    if custom_endpoint || harness::harness(harness).is_none() {
        return plan;
    }
    let names = reg::all_env_names();
    let saved_values = saved.values(&names);
    let has = |n: &str| {
        env.get(n).is_some_and(|v| !v.is_empty()) || saved_values.get(n).is_some_and(|v| !v.is_empty())
    };
    // What is connected: the saved connections, and the providers whose key the
    // caller passed in the run's env (never a sign-in scoped to another app).
    let mut connected = saved.connections();
    for name in env.keys() {
        if let Some(def) = reg::provider_of_env(name)
            && def.scope_of(name).is_none()
            && !connected.iter().any(|c| c.provider == def.id)
            && has(name)
        {
            connected.push(reg::Connected {
                provider: def.id.to_string(),
                ..Default::default()
            });
        }
    }
    if harness == "hermes" {
        let picks = reg::hermes::select(&connected, &has);
        plan.hermes_config = reg::hermes::config_yaml(&picks);
        // A custom endpoint with no key still needs the variable Hermes's file names.
        return plan;
    }
    let h = harness::harness(harness).expect("checked");
    // An app that has a credential of its own is left to use it.
    let own = h.keys.iter().any(|k| env.get(*k).is_some_and(|v| !v.is_empty()));
    if own {
        return plan;
    }
    if let Some(e) = reg::endpoint_for(harness, &connected, &has) {
        let wire = Wire::parse(e.wire);
        let var = harness::endpoint_key_var(h, wire);
        let key = e
            .key_src
            .and_then(|s| env.get(s).filter(|v| !v.is_empty()).cloned().or_else(|| saved_values.get(s).cloned()))
            .unwrap_or_else(|| "no-key-required".to_string());
        plan.env.insert(var.to_string(), key);
        plan.endpoint = Some(Endpoint {
            base_url: e.base_url,
            wire,
            model: Some(e.model),
        });
    }
    plan
}

/// Largest host file `agent_start.files` attaches.
const MAX_ATTACHMENT: u64 = 8 * 1024 * 1024;

/// The run options of an `agent_start` (MCP, the SDKs' `Space.agent_start`,
/// the app and `cua agent run` all arrive here): the caller's env, its
/// `env_from_host` keys and the keys saved in Cua Spaces that the harness
/// reads ([`run_env`]). Also returns the `env_from_host` names not found.
pub fn start_options(
    a: &cua_spaces_contract::inputs::AgentStart,
) -> Result<(RunOptions, Vec<String>)> {
    start_options_with(keys::global(), a)
}

/// [`start_options`] over the given saved keys.
pub fn start_options_with(
    saved: &keys::AgentKeys,
    a: &cua_spaces_contract::inputs::AgentStart,
) -> Result<(RunOptions, Vec<String>)> {
    let (mut env, missing) = run_env_with(
        saved,
        &a.agent,
        &a.env,
        &a.env_from_host,
        a.base_url.is_some(),
    )?;
    let wire =
        match a.wire.as_deref() {
            None => None,
            Some(w) => Some(Wire::parse(w).ok_or_else(|| {
                Error::InvalidArgument(format!("agent_start: unknown wire {w:?}"))
            })?),
        };
    let plan = connection_plan(saved, &a.agent, &env, a.base_url.is_some());
    for (k, v) in plan.env {
        env.entry(k).or_insert(v);
    }
    let endpoint = match a.base_url.clone() {
        Some(base_url) => Some(Endpoint {
            base_url,
            wire,
            model: a.model.clone(),
        }),
        None => plan.endpoint.map(|mut e| {
            if a.model.is_some() {
                e.model = a.model.clone();
            }
            e
        }),
    };
    let opts = RunOptions {
        cwd: a.cwd.clone(),
        repo: a.repo.clone(),
        branch: a.branch.clone(),
        env,
        mcp_servers: a
            .mcp_servers
            .iter()
            .map(|m| McpServer {
                name: m.name.clone(),
                url: m.url.clone(),
                command: m.command.clone(),
                args: m.args.clone(),
                ..Default::default()
            })
            .collect(),
        endpoint,
        model: a.model.clone(),
        hermes_config: plan.hermes_config,
        files: attachments(&a.files)?,
        sandbox_mcp: a.sandbox_mcp,
        skills: a.skills,
        label: a.label.clone(),
        exit_when_idle: a.exit_when_idle.unwrap_or(false),
        ..Default::default()
    };
    Ok((opts, missing))
}

/// `agent_start.files`: host files read for the first prompt.
fn attachments(paths: &[String]) -> Result<Vec<Attachment>> {
    paths
        .iter()
        .map(|p| {
            let path = match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
                (Some(rest), Some(home)) => std::path::PathBuf::from(home).join(rest),
                _ => std::path::PathBuf::from(p),
            };
            let bad =
                |e: std::io::Error| Error::InvalidArgument(format!("agent_start: file {p}: {e}"));
            if std::fs::metadata(&path).map_err(bad)?.len() > MAX_ATTACHMENT {
                return Err(Error::InvalidArgument(format!(
                    "agent_start: file {p} is larger than 8 MiB"
                )));
            }
            let bytes = std::fs::read(&path).map_err(bad)?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            Ok(Attachment { name, bytes })
        })
        .collect()
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
    fn readiness_follows_the_credential_this_server_can_forward() {
        let c = capabilities();
        for h in &c.harnesses {
            let def = harness::harness(h.id).unwrap();
            let auth = serde_json::to_value(h.auth).unwrap();
            match host_key(def) {
                Some(_) => assert_eq!(auth, "ok", "{}", h.id),
                None if def.logins.is_some() => {
                    assert_eq!(auth, "missing", "{}", h.id);
                    assert!(!h.ready, "{} is ready without a credential", h.id);
                    let hint = h.auth_hint.as_deref().unwrap();
                    assert!(hint.contains(def.keys[0]) && hint.contains("env_from_host"));
                }
                None => assert_eq!(auth, "unknown", "{}", h.id),
            }
            assert_eq!(h.supported, def.ready);
        }
    }

    fn saved(pairs: &[(&str, Option<&str>, &str)]) -> (keys::AgentKeys, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "cua-run-env-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let k = keys::AgentKeys::new(cua_auth::Store::TestKeychain(dir.clone()));
        for (p, e, v) in pairs {
            k.set(p, *e, v).unwrap();
        }
        (k, dir)
    }

    /// Hermes (and every harness but Claude Code) gets exactly what its own adapter takes
    /// as an API key: never the Claude subscription sign-in, in any slot.
    /// Hermes' adapter would use `CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_TOKEN` or a token in
    /// `ANTHROPIC_API_KEY` as a subscription login (and, given `~/.claude/.credentials.json`
    /// with a refresh token, refresh it and spend Claude Code's single-use refresh token), so
    /// none of those is ever forwarded to it.
    #[test]
    fn hermes_gets_api_keys_and_never_the_claude_subscription_sign_in() {
        let none = BTreeMap::new();
        let token = format!("sk-ant-oat01-{}", "aB3_".repeat(24));
        let (k, dir) = saved(&[
            ("openai", None, "sk-test-0000-bbbb"),
            ("other", Some("OPENROUTER_API_KEY"), "sk-or-test-0000-cccc"),
        ]);
        // The imported Claude sign-in is saved too.
        k.import(
            "claude-code",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "Signed in as ada@example.com (Claude Max)",
            &token,
            Some(u64::MAX / 2),
            Some("claude-login-file"),
        )
        .unwrap();
        let (env, _) = run_env_with(&k, "hermes", &none, &[], false).unwrap();
        assert_eq!(
            env.keys().collect::<Vec<_>>(),
            ["OPENAI_API_KEY", "OPENROUTER_API_KEY"],
            "Hermes gets provider keys only"
        );
        let (env, _) = run_env_with(&k, "claude-code", &none, &[], false).unwrap();
        assert_eq!(env["CLAUDE_CODE_OAUTH_TOKEN"], token);
        for h in [
            "hermes",
            "opencode",
            "goose",
            "pi",
            "openclaw",
            "gemini-cli",
            "openai-codex",
        ] {
            let (env, _) = run_env_with(&k, h, &none, &[], false).unwrap();
            assert!(
                env.values().all(|v| !v.contains("sk-ant-oat")),
                "{h} must not get the Claude subscription token"
            );
            assert!(
                !env.contains_key("CLAUDE_CODE_OAUTH_TOKEN")
                    && !env.contains_key("ANTHROPIC_TOKEN"),
                "{h}"
            );
        }
        // A token that was exported as ANTHROPIC_API_KEY: Claude Code gets it as its sign-in,
        // Hermes gets nothing from it (so Hermes says it needs an API key).
        let shell = BTreeMap::from([("ANTHROPIC_API_KEY".to_string(), token.clone())]);
        let (env, _) = run_env_with(&k, "hermes", &shell, &[], false).unwrap();
        assert!(
            !env.contains_key("ANTHROPIC_API_KEY") && !env.contains_key("CLAUDE_CODE_OAUTH_TOKEN")
        );
        let (env, _) = run_env_with(&k, "claude-code", &shell, &[], false).unwrap();
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(env["CLAUDE_CODE_OAUTH_TOKEN"], token);
        // A real API key still goes to Hermes as ANTHROPIC_API_KEY.
        let key = BTreeMap::from([(
            "ANTHROPIC_API_KEY".to_string(),
            "sk-ant-api03-real".to_string(),
        )]);
        let (env, _) = run_env_with(&k, "hermes", &key, &[], false).unwrap();
        assert_eq!(env["ANTHROPIC_API_KEY"], "sk-ant-api03-real");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_keys_reach_only_the_harnesses_that_read_them() {
        let (k, dir) = saved(&[
            ("anthropic", None, "sk-ant-test-0000-aaaa"),
            ("openai", None, "sk-test-0000-bbbb"),
            ("other", Some("MISTRAL_API_KEY"), "mk-test-0000-cccc"),
        ]);
        let none = BTreeMap::new();
        let (env, missing) = run_env_with(&k, "claude-code", &none, &[], false).unwrap();
        assert_eq!(
            env,
            BTreeMap::from([("ANTHROPIC_API_KEY".into(), "sk-ant-test-0000-aaaa".into())])
        );
        assert!(missing.is_empty());
        let (env, _) = run_env_with(&k, "openai-codex", &none, &[], false).unwrap();
        assert_eq!(
            env.keys().collect::<Vec<_>>(),
            vec!["OPENAI_API_KEY"],
            "codex gets only its key"
        );
        // A harness that reads neither key gets neither; an unknown one
        // gets nothing.
        let (env, _) = run_env_with(&k, "gemini-cli", &none, &[], false).unwrap();
        assert!(env.is_empty(), "{:?}", env.keys());
        let (env, _) = run_env_with(&k, "no-such-agent", &none, &[], false).unwrap();
        assert!(env.is_empty());
        // A harness that reads several keys gets each one saved.
        let (env, _) = run_env_with(&k, "opencode", &none, &[], false).unwrap();
        assert_eq!(
            env.keys().collect::<Vec<_>>(),
            vec!["ANTHROPIC_API_KEY", "OPENAI_API_KEY"]
        );
        // Nothing goes to a custom endpoint unless named.
        let (env, _) = run_env_with(&k, "claude-code", &none, &[], true).unwrap();
        assert!(env.is_empty());
        // The caller's own value wins.
        let mine = BTreeMap::from([("ANTHROPIC_API_KEY".to_string(), "mine".to_string())]);
        let (env, _) = run_env_with(&k, "claude-code", &mine, &[], false).unwrap();
        assert_eq!(env["ANTHROPIC_API_KEY"], "mine");
        // An Other key: only when named in env_from_host.
        let (env, missing) =
            run_env_with(&k, "gemini-cli", &none, &["MISTRAL_API_KEY".into()], false).unwrap();
        assert_eq!(env["MISTRAL_API_KEY"], "mk-test-0000-cccc");
        assert!(missing.is_empty());
        assert!(run_env_with(&k, "gemini-cli", &none, &["NOT_SAVED_KEY".into()], false).is_err());
        assert!(run_env_with(&k, "gemini-cli", &none, &["HOME".into()], false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_start_options_carry_the_saved_key_and_the_cli_options() {
        let (k, dir) = saved(&[("anthropic", None, "sk-ant-test-0000-aaaa")]);
        let file = dir.join("notes.txt");
        std::fs::write(&file, b"context").unwrap();
        // What `cua agent run` sends (its JSON is the contract's input).
        let a: cua_spaces_contract::inputs::AgentStart =
            serde_json::from_value(serde_json::json!({
                "space": "local:dev", "agent": "claude-code", "prompt": "hi",
                "files": [file.display().to_string()], "wire": "anthropic",
                "sandbox_mcp": false, "skills": false, "label": "e2e", "exit_when_idle": true,
            }))
            .unwrap();
        let (opts, missing) = start_options_with(&k, &a).unwrap();
        assert!(missing.is_empty());
        assert_eq!(
            opts.env,
            BTreeMap::from([("ANTHROPIC_API_KEY".into(), "sk-ant-test-0000-aaaa".into())]),
            "the saved key reaches the run, as on MCP"
        );
        assert_eq!(opts.files.len(), 1);
        assert_eq!(opts.files[0].name, "notes.txt");
        assert_eq!(opts.files[0].bytes, b"context");
        assert_eq!(
            (
                opts.sandbox_mcp,
                opts.skills,
                opts.label.as_deref(),
                opts.exit_when_idle
            ),
            (Some(false), Some(false), Some("e2e"), true)
        );
        // A key from the caller's shell wins over the saved one.
        let mut mine = a.clone();
        mine.env = BTreeMap::from([("ANTHROPIC_API_KEY".into(), "from-shell".into())]);
        assert_eq!(
            start_options_with(&k, &mine).unwrap().0.env["ANTHROPIC_API_KEY"],
            "from-shell"
        );
        // A custom endpoint gets no saved key, and keeps its wire.
        let mut ep = a.clone();
        ep.base_url = Some("http://127.0.0.1:9".into());
        let (opts, _) = start_options_with(&k, &ep).unwrap();
        assert!(opts.env.is_empty());
        assert_eq!(opts.endpoint.unwrap().wire, Some(Wire::Anthropic));
        // Bad input is refused without quoting any key.
        let mut bad = a.clone();
        bad.wire = Some("smoke-signals".into());
        let e = start_options_with(&k, &bad).unwrap_err().to_string();
        assert!(e.contains("unknown wire") && !e.contains("sk-ant"), "{e}");
        let mut gone = a.clone();
        gone.files = vec![dir.join("missing").display().to_string()];
        assert!(start_options_with(&k, &gone).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readiness_counts_saved_keys() {
        let (k, dir) = saved(&[("anthropic", None, "sk-ant-test-0000-aaaa")]);
        let names = k.names();
        let claude = harness::harness("claude-code").unwrap();
        let info = claude.info_with_host_key(host_key_with(claude, &names));
        assert_eq!(serde_json::to_value(info.auth).unwrap(), "ok");
        if host_key_with(claude, &[]).is_none() {
            let info = claude.info_with_host_key(None);
            assert!(!info.ready);
            let hint = info.auth_hint.unwrap();
            assert!(hint.contains("Settings → Connections"), "{hint}");
        }
        let gemini = harness::harness("gemini-cli").unwrap();
        if host_key_with(gemini, &[]).is_none() {
            assert_eq!(host_key_with(gemini, &names), None);
        }
        let _ = std::fs::remove_dir_all(&dir);
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

    /// A key saved for any provider reaches only the apps the provider file delivers it to, and the
    /// two sign-ins reach their own app only.
    #[test]
    fn every_provider_key_reaches_only_the_apps_that_can_use_it() {
        let none = BTreeMap::new();
        for p in keys::registry::visible() {
            if p.key_method().is_none() {
                continue;
            }
            let (k, dir) = saved(&[]);
            let mut fields = BTreeMap::new();
            if p.kind == keys::registry::Kind::Custom {
                fields.insert("base_url".to_string(), "http://127.0.0.1:1234/v1".to_string());
                fields.insert("model".to_string(), "m".to_string());
                fields.insert("wire".to_string(), "openai-chat".to_string());
            } else if p.default_model.is_empty() {
                fields.insert("model".to_string(), "some-model".to_string());
            }
            k.set_with(p.id, None, "secret-value-0000", None, &fields).unwrap();
            for h in harness::HARNESSES {
                let (env, _) = run_env_with(&k, h.id, &none, &[], false).unwrap();
                let allowed: Vec<&str> = p.delivery_for(h.id).iter().map(|e| e.dest).collect();
                for name in env.keys() {
                    assert!(allowed.contains(&name.as_str()), "{}: {} reached {name}", p.id, h.id);
                    assert!(h.keys.contains(&name.as_str()), "{}: {name} is not one {} reads", p.id, h.id);
                }
                if allowed.is_empty() {
                    assert!(env.is_empty(), "{} reached {} without a delivery entry", p.id, h.id);
                }
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        // The Claude sign-in and the ChatGPT sign-in, beside a key for the same provider.
        let (k, dir) = saved(&[("anthropic", None, "sk-ant-test-0000-aaaa"), ("openai", None, "sk-test-0000-bbbb")]);
        k.import("claude-code", "CLAUDE_CODE_OAUTH_TOKEN", "Claude sign-in", "sk-ant-oat01-whatever", None, None).unwrap();
        k.import("codex", "CODEX_AUTH_JSON", "ChatGPT sign-in", "{\"tokens\":{}}", None, None).unwrap();
        for h in harness::HARNESSES {
            let (env, _) = run_env_with(&k, h.id, &none, &[], false).unwrap();
            assert_eq!(env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"), h.id == "claude-code", "{}", h.id);
            assert_eq!(env.contains_key("CODEX_AUTH_JSON"), h.id == "openai-codex", "{}", h.id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hermes_is_told_which_provider_to_use() {
        let none = BTreeMap::new();
        let (k, dir) = saved(&[("openrouter", None, "sk-or-test-0000-aaaa")]);
        let (env, _) = run_env_with(&k, "hermes", &none, &[], false).unwrap();
        let plan = connection_plan(&k, "hermes", &env, false);
        let y = plan.hermes_config.unwrap();
        assert!(y.contains("provider: \"openrouter\"") && y.contains("adopt_external_logins: false"), "{y}");
        assert!(plan.endpoint.is_none() && plan.env.is_empty());
        // Through the options an agent_start gets.
        let a: cua_spaces_contract::inputs::AgentStart = serde_json::from_value(serde_json::json!({
            "space": "local:dev", "agent": "hermes", "prompt": "hi"})).unwrap();
        let (opts, _) = start_options_with(&k, &a).unwrap();
        assert_eq!(opts.env.keys().collect::<Vec<_>>(), ["OPENROUTER_API_KEY"]);
        assert!(opts.hermes_config.unwrap().contains("openrouter"));
        // A Claude sign-in alone is nothing Hermes can use: no config, no key.
        let (k2, dir2) = saved(&[]);
        k2.import("claude-code", "CLAUDE_CODE_OAUTH_TOKEN", "Claude sign-in", "sk-ant-oat01-x", None, None).unwrap();
        let (opts, _) = start_options_with(&k2, &a).unwrap();
        assert!(opts.env.is_empty() && opts.hermes_config.is_none());
        // A custom endpoint of the caller's own is left alone.
        let mut ep = a.clone();
        ep.base_url = Some("http://127.0.0.1:9".into());
        let (opts, _) = start_options_with(&k, &ep).unwrap();
        assert!(opts.hermes_config.is_none() && opts.env.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn an_app_without_a_variable_for_the_provider_is_pointed_at_its_endpoint() {
        let (k, dir) = saved(&[("deepseek", None, "ds-test-0000-aaaa")]);
        let start = |agent: &str| {
            let a: cua_spaces_contract::inputs::AgentStart = serde_json::from_value(serde_json::json!({
                "space": "local:dev", "agent": agent, "prompt": "hi"})).unwrap();
            start_options_with(&k, &a).unwrap().0
        };
        let o = start("opencode");
        let e = o.endpoint.unwrap();
        assert_eq!((e.base_url.as_str(), e.wire, e.model.as_deref()), ("https://api.deepseek.com/v1", Some(Wire::OpenaiChat), Some("deepseek-flash")));
        assert_eq!(o.env["OPENAI_API_KEY"], "ds-test-0000-aaaa");
        // Claude Code and Gemini take no such provider: nothing is added, nothing is pointed anywhere.
        for agent in ["claude-code", "gemini-cli", "google-antigravity"] {
            let o = start(agent);
            assert!(o.endpoint.is_none() && o.env.is_empty(), "{agent}");
        }
        // Codex speaks the Responses wire, which DeepSeek does not serve.
        assert!(start("openai-codex").endpoint.is_none());
        // A key the app reads itself is used as it always was, with no endpoint.
        k.set("openrouter", None, "sk-or-test-0000-bbbb").unwrap();
        let o = start("opencode");
        assert!(o.endpoint.is_none());
        assert_eq!(o.env.keys().collect::<Vec<_>>(), ["OPENROUTER_API_KEY"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_custom_endpoint_is_saved_with_its_url_and_model_and_needs_no_key() {
        let (k, dir) = saved(&[]);
        let mut f = BTreeMap::new();
        // The URL and model are required; the wire is one of the closed set.
        assert!(k.set_with("custom", None, "", None, &f).is_err());
        f.insert("base_url".to_string(), "http://127.0.0.1:1234/v1".to_string());
        f.insert("model".to_string(), "qwen".to_string());
        f.insert("wire".to_string(), "carrier-pigeon".to_string());
        assert!(k.set_with("custom", None, "", None, &f).is_err());
        f.insert("wire".to_string(), "openai-chat".to_string());
        f.insert("base_url".to_string(), "ftp://nope".to_string());
        assert!(k.set_with("custom", None, "", None, &f).is_err());
        f.insert("base_url".to_string(), "http://127.0.0.1:1234/v1".to_string());
        let info = k.set_with("custom", None, "", None, &f).unwrap();
        assert_eq!(info.env, "CUSTOM_ENDPOINT_API_KEY");
        assert_eq!(info.fields["base_url"], "http://127.0.0.1:1234/v1");
        assert_eq!(info.last4, "");
        // It connects with no key: Hermes and the endpoint apps get its URL.
        let none = BTreeMap::new();
        let (env, _) = run_env_with(&k, "hermes", &none, &[], false).unwrap();
        assert!(env.is_empty(), "no key is delivered when there is none");
        let y = connection_plan(&k, "hermes", &env, false).hermes_config.unwrap();
        assert!(y.contains("provider: \"custom\"") && y.contains("base_url: \"http://127.0.0.1:1234/v1\"") && y.contains("no-key-required"), "{y}");
        let plan = connection_plan(&k, "goose", &BTreeMap::new(), false);
        assert_eq!(plan.endpoint.unwrap().model.as_deref(), Some("qwen"));
        assert_eq!(plan.env["OPENAI_API_KEY"], "no-key-required");
        // The key, when there is one, stays out of every report.
        k.set_with("custom", None, "local-key-1234-abcd", None, &f).unwrap();
        let text = serde_json::to_string(&k.report().unwrap()).unwrap();
        assert!(!text.contains("local-key"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plan_and_a_model_are_chosen_when_connecting_and_checked() {
        let (k, dir) = saved(&[]);
        let mut f = BTreeMap::new();
        // Z.AI bills its plans separately: the plan must be one it lists.
        assert!(k.set_with("zai", None, "zai-key-0000-aaaa", Some("moon"), &f).is_err());
        let info = k.set_with("zai", None, "zai-key-0000-aaaa", Some("coding-global"), &f).unwrap();
        assert_eq!(info.region.as_deref(), Some("coding-global"));
        // A provider with no default model asks for one.
        assert!(k.set_with("deepinfra", None, "di-key-0000-aaaa", None, &f).is_err());
        f.insert("model".to_string(), "meta-llama/x".to_string());
        assert!(k.set_with("deepinfra", None, "di-key-0000-aaaa", None, &f).is_ok());
        // A field the provider does not ask for is refused; so is a hidden provider.
        f.insert("base_url".to_string(), "https://evil.example".to_string());
        assert!(k.set_with("deepinfra", None, "di-key-0000-aaaa", None, &f).is_err());
        assert!(k.set("nous", None, "x-key-0000-aaaa").is_err());
        assert!(k.set("xai-oauth", None, "x-key-0000-aaaa").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
