//! Coding agents inside a Space: [`cua_agents`] over the Space's spacesd.
//!
//! A run is a detached, tagged spacesd process driving the harness over the
//! Agent Client Protocol; every fact about it is a file in the Space
//! (`~/.cua/agents/<run>/`), so a fresh server lists and continues runs it
//! never started. Host credential files are never copied: API keys reach a
//! run only as explicit per-run env (`RunOptions::env`, or on the MCP
//! surface `env_from_host` naming known provider key variables), written
//! 0600 inside the run and redacted from its event log. Keys saved in Cua
//! Spaces (Settings → Agents, [`keys`]) reach only the runs whose harness
//! reads them ([`run_env`]). Moving a signed-in agent session is teleport's
//! job, behind its consent gate.

use crate::error::{Error, Result};
use crate::space::Space;
use std::collections::BTreeMap;

pub mod keys;
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
            Error::Fleet(_) => "Fleet",
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

/// The extension operation `agent_start` calls with a run it started
/// (`{space, run_id, agent}`), so the daemon can tell the user when the run
/// or one of its turns fails.
pub const WATCH_OP: &str = "agent_start.watch";

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
/// Spaces (Settings → Agents), or one set in this process's environment
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
                 (or a key saved in Cua Spaces → Settings → Agents)",
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
    let mut out = env.clone();
    out.extend(host_env);
    if !custom_endpoint && let Some(h) = harness::harness(harness) {
        let wanted: Vec<&str> = h
            .keys
            .iter()
            .copied()
            .filter(|k| out.get(*k).is_none_or(|v| v.is_empty()))
            .collect();
        out.extend(saved.values(&wanted));
    }
    Ok((out, missing))
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
    let (env, missing) = run_env_with(
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
    let endpoint = a.base_url.clone().map(|base_url| Endpoint {
        base_url,
        wire,
        model: a.model.clone(),
    });
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
            assert!(hint.contains("Settings → Agents"), "{hint}");
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
}
