//! Durable runs: the runner's durable mode (see `runner/acp-runner.mjs`) and
//! the driver a session re-driver launches on any machine.
//!
//! A durable run keeps its journal (events, state, the tool ledger
//! `tools.jsonl`) in a state directory behind an epoch fence. When the
//! process that drives it dies, the re-driver starts the runner again with a
//! higher epoch: the new runner raises the fence (so the old one is cut off),
//! resumes the agent's ACP session and sends it a recovery note.
//!
//! Two entry points:
//!
//! - [`RunOptions::durable`](crate::RunOptions): the run lives in a Space
//!   ([`Agents::start`](crate::Agents::start)); [`Agents::redrive`](crate::Agents::redrive)
//!   starts it again at a higher epoch.
//! - [`durable_driver_spec`]: everything a *job on any machine* needs
//!   (argv, a small bundle with the runner, the run config without secrets,
//!   and the environment) for a session coordinator to launch as the driver.
//!   The coordinator adds the session env contract (`CUA_SESSION*`).
//!
//! # What a durable run guarantees
//!
//! | Tool calls | Guarantee |
//! |---|---|
//! | those the harness asks permission for | the intent is journaled before the approval; a call id is approved once; a repeat of a call with an unknown outcome is refused |
//! | built-in tools that never ask | the outcome is journaled when the harness reports it; after a crash the call is listed as "may have partially run"; **not** exactly-once (see [`COVERAGE`]) |
//!
//! With [`DurableOptions::ship`] the guarantee holds across hosts: the host
//! that drives the run mirrors the journal to the Cua Volume as immutable
//! chunks (`cua_volume::journal`), and the runner answers a permission
//! request only once the intent is on the Volume ([`SHIP_ACK_FILE`]); a
//! re-drive on another host (or in a re-placed Space) starts from the
//! journal restored from the Volume ([`DurableOptions::restore`]). The
//! Volume fence that cuts off a stale host is a compare-and-swap: atomic
//! across machines on the S3 backend, only between processes of one machine
//! on the `fs` backend.

use crate::harness::{self, Endpoint};
use crate::{Error, Result, installables};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;

/// The environment every driver gets from the session coordinator.
pub const ENV_CONTRACT: [&str; 7] = [
    "CUA_SESSION",
    "CUA_SESSION_EPOCH",
    "CUA_SESSION_ATTEMPT",
    "CUA_SESSION_RESUME",
    "CUA_SESSION_SPACE",
    "CUA_SESSION_STATE_DIR",
    "CUA_SESSION_HEARTBEAT_SECS",
];

/// Driver exit codes (the session contract).
pub mod exit {
    /// Finished.
    pub const DONE: i32 = 0;
    /// Newer epoch exists: ignore.
    pub const FENCED: i32 = 75;
    /// Cannot succeed: do not resume.
    pub const PERMANENT: i32 = 78;
}

/// The journal files of a durable run, in its state directory.
pub const JOURNAL_FILES: [&str; 4] = ["events.jsonl", "state.json", "tools.jsonl", "epoch"];

/// The bundle must stay small: it travels in a job spec.
pub const MAX_BUNDLE_BYTES: usize = 1024 * 1024;

/// What the runner does with a repeat permission request for a tool call
/// whose outcome is unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    /// Refuse it; the recovery note tells the model it may have run.
    #[default]
    AskModel,
    /// Approve it again when the tool only reads (`read`, `search`, `fetch`),
    /// refuse every other kind.
    RerunSafe,
}

impl Policy {
    pub fn as_str(self) -> &'static str {
        match self {
            Policy::AskModel => "ask-model",
            Policy::RerunSafe => "rerun-safe",
        }
    }
}

/// Durable settings of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableOptions {
    /// This drive's epoch (the coordinator issues it; only grows). In a
    /// driver job `CUA_SESSION_EPOCH` overrides it.
    pub epoch: u64,
    /// The journal exists and should be resumed. `CUA_SESSION_RESUME`
    /// overrides it; the journal itself is the truth.
    pub resume: bool,
    /// Where the journal lives. In a Space: default the run directory. For a
    /// driver spec: default `~/.cua/durable/<session>`, else empty (the
    /// runner then uses `$CUA_JOB_DATA/session`).
    pub state_dir: Option<String>,
    /// The session's name (for the default state directory and the journal).
    pub session: Option<String>,
    /// Repeat-request policy.
    pub policy: Policy,
    /// How many times the runner restarts a crashed agent in-process before
    /// it exits non-zero for the re-driver. Default 3.
    pub max_agent_restarts: u32,
    /// Keep the harness asking permission for its tools, so each call is
    /// guarded: the runner does not switch the harness to its full-access
    /// mode (it selects the harness's own ask-for-approval mode, see
    /// [`guard_mode_kinds`]), approves with "allow once" (never "always"). The sandbox stays the
    /// boundary (the runner approves everything). Default true; false trades
    /// the guarantee for fewer round trips.
    pub guard: bool,
    /// Mirror the journal to the Cua Volume (see [`Ship`]): the host that
    /// drives the run ships it, and the runner answers a permission request
    /// only once the host acknowledged the intent on the Volume. `None`:
    /// the journal stays local.
    pub ship: Option<Ship>,
    /// Journal files to put in the state directory before the runner starts
    /// (file name -> bytes: `events.jsonl`, `tools.jsonl`, `state.json`,
    /// `epoch`), for a re-drive from a journal restored from the Cua Volume
    /// on another host or in a re-placed Space. Use a higher `epoch` than the
    /// restored one and `resume`. Empty: none.
    pub restore: BTreeMap<String, Vec<u8>>,
}

/// The ship file the host writes into the state directory once journal
/// bytes are on the Volume: `{"epoch": E, "end": {"tools.jsonl": N,
/// "events.jsonl": M}}`.
pub const SHIP_ACK_FILE: &str = "ship.ack";

/// Journal shipping (the runner's side of `cua_volume::journal`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ship {
    /// How long the runner waits for the host to acknowledge an intent
    /// before it refuses the tool call (journaled as a denied outcome, so
    /// the call never ran and at-most-once holds). Default 30 s.
    pub ack_timeout_ms: u64,
}

impl Default for Ship {
    fn default() -> Self {
        Ship {
            ack_timeout_ms: 30_000,
        }
    }
}

impl Default for DurableOptions {
    fn default() -> Self {
        DurableOptions {
            epoch: 1,
            resume: false,
            state_dir: None,
            session: None,
            policy: Policy::default(),
            max_agent_restarts: 3,
            guard: true,
            ship: None,
            restore: BTreeMap::new(),
        }
    }
}

impl DurableOptions {
    /// The `durable` section of `run.json`. Never carries a secret.
    pub fn run_section(&self, default_state_dir: Option<&str>) -> Value {
        json!({
            "epoch": self.epoch,
            "resume": self.resume,
            "stateDir": self.state_dir.as_deref().or(default_state_dir),
            "session": self.session,
            "policy": self.policy.as_str(),
            "maxAgentRestarts": self.max_agent_restarts,
            "guard": self.guard,
            "ship": self.ship.as_ref().map(|s| json!({
                "ackFile": SHIP_ACK_FILE,
                "ackTimeoutMs": s.ack_timeout_ms,
            })),
        })
    }
}

/// `run.json` for the next drive: the same run at a higher epoch.
pub fn redrive_run_json(raw: &[u8], epoch: u64) -> Result<Vec<u8>> {
    let mut run: Value = serde_json::from_slice(raw)?;
    let d = run
        .get_mut("durable")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::Invalid("the run is not durable".into()))?;
    let old = d.get("epoch").and_then(Value::as_u64).unwrap_or(0);
    if epoch <= old {
        return Err(Error::Invalid(format!(
            "epoch {epoch} is not above the run's epoch {old}"
        )));
    }
    d.insert("epoch".into(), json!(epoch));
    d.insert("resume".into(), json!(true));
    Ok(serde_json::to_vec_pretty(&run)?)
}

/// What a session coordinator needs to launch a durable driver. The field
/// names match `SessionSpec` (`argv`, `env`, `bundle`, `state_dir`, `label`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverSpec {
    /// The driver command, run in the unpacked bundle.
    pub argv: Vec<String>,
    /// The driver's environment: the caller's keys (secrets: they belong in
    /// the job's sealed env, never in logs or argv) and the MCP header
    /// values. The coordinator adds [`ENV_CONTRACT`].
    pub env: BTreeMap<String, String>,
    /// Gzip tar of the runner, `run.json` (no secrets) and the launcher,
    /// base64.
    pub bundle: String,
    /// Where the driver keeps its journal.
    pub state_dir: String,
    /// What the session is, for people: the harness id.
    pub label: String,
    /// Size of the gzip tar before base64.
    pub bundle_bytes: usize,
}

/// Which tool calls each harness guards (see the durable-agents guide).
/// `guarded`: permission requests (intent before approval, exactly once);
/// `unguarded`: only outcomes are journaled.
pub struct Coverage {
    pub harness: &'static str,
    pub resume: &'static str,
    pub guarded: &'static str,
    pub unguarded: &'static str,
}

/// Per-harness coverage, checked against [`harness::HARNESSES`] by a test so
/// a new harness cannot ship without a row. Lab-verified facts are in the
/// guide; this table is the source the guide's table is generated from.
pub const COVERAGE: &[Coverage] = &[
    Coverage {
        harness: "claude-code",
        resume: "session/resume or session/load (claude-agent-acp reloads the Claude Code transcript)",
        guarded: "tools that need permission under the adapter's permission mode (Bash, Edit, Write, MCP tools)",
        unguarded: "read-only built-ins that Claude Code allows without asking (Read, Glob, Grep, WebFetch, TodoWrite); in full-access mode every tool runs without a request",
    },
    Coverage {
        harness: "openai-codex",
        resume: "session/load (codex-acp reloads the rollout)",
        guarded: "commands and patches that need escalation, in the Ask-for-approval mode guard mode selects (the default mode runs sandboxed commands without asking)",
        unguarded: "everything under full access (approval never); reads and MCP tools Codex trusts",
    },
    Coverage {
        harness: "gemini-cli",
        resume: "session/load",
        guarded: "tools under the default approval mode",
        unguarded: "tools in yolo mode and read-only tools",
    },
    Coverage {
        harness: "google-antigravity",
        resume: "not verified",
        guarded: "not verified",
        unguarded: "assume all (outcomes only)",
    },
    Coverage {
        harness: "opencode",
        resume: "session/resume and session/load",
        guarded: "bash, edit and webfetch under guard mode (OPENCODE_PERMISSION asks)",
        unguarded: "other tools, and every tool with guard mode off (opencode allows by default)",
    },
    Coverage {
        harness: "goose",
        resume: "session/load",
        guarded: "tools under the smart-approve / approve modes",
        unguarded: "tools in autonomous mode",
    },
    Coverage {
        harness: "pi",
        resume: "not verified (community adapter)",
        guarded: "none known",
        unguarded: "all (outcomes only)",
    },
    Coverage {
        harness: "hermes",
        resume: "session/load (hermes-acp replays its SQLite session)",
        guarded: "dangerous commands (Hermes asks via ACP permission)",
        unguarded: "all other tools",
    },
    Coverage {
        harness: "openclaw",
        resume: "not verified (bridge mode)",
        guarded: "not verified",
        unguarded: "assume all (outcomes only)",
    },
];

fn tar_file(
    tar: &mut tar::Builder<Vec<u8>>,
    path: &str,
    body: &[u8],
    mode: u32,
) -> std::io::Result<()> {
    let mut h = tar::Header::new_gnu();
    h.set_size(body.len() as u64);
    h.set_mode(mode);
    h.set_mtime(0);
    h.set_uid(0);
    h.set_gid(0);
    h.set_entry_type(tar::EntryType::Regular);
    tar.append_data(&mut h, path, body)
}

/// The agent-side mode (by ACP `_meta.kind`) guard mode selects, for a harness
/// whose default mode does not ask. Measured on codex-acp 1.13.1 (Codex
/// 0.156.1): the default mode is "Approve for me" (a model reviews escalations,
/// nothing is asked of the client), `-c approval_policy=...` on the launch has
/// no effect and `approval_policy = "untrusted"` in config.toml is refused
/// ("no longer supported"); only the "Ask for approval" mode (kind `standard`)
/// sends `session/request_permission`. Empty: the harness is left in the mode
/// it starts in (Claude Code asks for Bash, Write, Edit and WebFetch there).
pub fn guard_mode_kinds(h: &harness::Harness) -> &'static [&'static str] {
    match h.id {
        "openai-codex" => &["standard"],
        _ => &[],
    }
}

/// Environment guard mode adds to a harness's launch so it asks permission. OpenCode allows every tool by
/// default; `OPENCODE_PERMISSION` (an inline permission config, measured on opencode 1.18.32 with and without
/// the user's own config) makes bash, edit and webfetch send `session/request_permission`.
pub fn guard_env(h: &harness::Harness) -> Vec<(String, String)> {
    match h.id {
        "opencode" => vec![(
            "OPENCODE_PERMISSION".into(),
            r#"{"bash":"ask","edit":"ask","webfetch":"ask"}"#.into(),
        )],
        _ => vec![],
    }
}

/// Install errors that no retry fixes (a missing tool the job Space cannot
/// add, a checksum or commit that does not match, no build for this CPU): the
/// driver exits 78 and the session fails at once instead of five drives later.
/// A failed download or a package manager error stays a plain exit 1 (retried).
const PERMANENT_INSTALL_ERRORS: &str =
    "git is not available|commit mismatch|integrity mismatch|sha256 mismatch|not published for|npm is not available";

/// What the driver does when its install step fails (run in `sh`, in the
/// driver's directory): exit 78 for an error no retry fixes, else exit 1.
fn install_failure_handler() -> String {
    format!(
        "tail -n 20 install.log >&2; \
         if grep '\"phase\":\"error\"' install.log | tail -n 1 | grep -Eq '{PERMANENT_INSTALL_ERRORS}'; then \
         echo 'driver: this harness cannot be installed in the job Space; not retrying' >&2; exit 78; fi; \
         echo 'driver: install failed; see install.log' >&2; exit 1;"
    )
}

/// The driver's launcher: installs what the harness needs (idempotent, fast
/// when present), then runs the durable runner under the session env.
fn driver_script(ids: &[&str], install: bool) -> Result<String> {
    let sdk = installables::install_dir("$HOME", "acp-sdk").expect("in manifest");
    Ok(format!(
        "#!/bin/sh\n# cua durable ACP driver (generated; holds no secret)\numask 077\n\
         cd \"$(dirname \"$0\")\" || exit 78\n\
         export CUA_RUN_DIR=\"$(pwd)\"\n\
         [ -n \"${{CUA_SESSION_EPOCH:-}}\" ] || {{ echo 'driver: CUA_SESSION_EPOCH is not set' >&2; exit 78; }}\n\
         [ -n \"${{CUA_SESSION_STATE_DIR:-}}${{CUA_JOB_DATA:-}}\" ] || {{ echo 'driver: neither CUA_SESSION_STATE_DIR nor CUA_JOB_DATA is set' >&2; exit 78; }}\n\
         {}\
         ln -sfn \"{sdk}/node_modules\" node_modules\n\
         exec \"$HOME/.cua/bin/node\" ./acp-runner.mjs \"$CUA_RUN_DIR\"\n",
        if install {
            format!(
                "(\n{}\n) >> install.log 2>&1 || {{ {} }}\n",
                installables::script(ids)?,
                install_failure_handler()
            )
        } else {
            "export PATH=\"$HOME/.cua/bin:$PATH\"\n".to_string()
        }
    ))
}

/// Builds the driver a re-driver launches for `harness` on `prompt`.
///
/// Deterministic (same inputs, same bytes), under [`MAX_BUNDLE_BYTES`], and
/// free of secrets: keys in `opts.env` go to [`DriverSpec::env`] only, and
/// `run.json` names them (`secretEnv`) so the runner redacts them from the
/// journal.
pub fn durable_driver_spec(
    harness_id: &str,
    prompt: &str,
    opts: &crate::RunOptions,
) -> Result<DriverSpec> {
    let h = harness::harness(harness_id).ok_or_else(|| {
        Error::Invalid(format!(
            "unknown harness {harness_id:?}; known: {}",
            harness::ids().join(", ")
        ))
    })?;
    if !h.ready {
        return Err(Error::Unsupported(format!(
            "{} is not supported yet",
            h.name
        )));
    }
    let d = opts
        .durable
        .clone()
        .ok_or_else(|| Error::Invalid("durable options are required".into()))?;
    for k in opts.env.keys() {
        crate::runs::valid_env_name(k)?;
    }
    // Empty: the coordinator leaves CUA_SESSION_STATE_DIR empty and the
    // runner uses $CUA_JOB_DATA/session.
    let state_dir = match (&d.state_dir, &d.session) {
        (Some(s), _) => s.clone(),
        (None, Some(n)) if crate::runs::valid_home_name(n) => format!("~/.cua/durable/{n}"),
        (None, Some(n)) => {
            return Err(Error::Invalid(format!(
                "session name {n:?}: use 1-63 of a-z, 0-9, `.`, `_`, `-`"
            )));
        }
        (None, None) => String::new(),
    };
    if opts.home.is_some() || opts.bridge || opts.repo.is_some() {
        return Err(Error::Unsupported(
            "a durable driver does not take home, bridge or repo (keep state in the session's Space)".into(),
        ));
    }
    let run_dir = "${CUA_RUN_DIR}";
    let cwd = opts
        .cwd
        .clone()
        .unwrap_or_else(|| "${CUA_SESSION_STATE_DIR}/work".into());
    let endpoint: Option<Endpoint> = opts.endpoint.clone().map(|mut e| {
        if e.model.is_none() {
            e.model = opts.model.clone();
        }
        e
    });
    let mut launch = harness::launch_with_config(
        h,
        endpoint.as_ref(),
        opts.model.as_deref(),
        run_dir,
        &cwd,
        None,
        opts.hermes_config.as_deref(),
    )?;
    if d.guard {
        launch.env.extend(guard_env(h));
    }
    if launch.wrapper.is_some() || !launch.prelude.is_empty() {
        return Err(Error::Unsupported(format!(
            "{} needs a launch wrapper, which a durable driver does not run yet",
            h.name
        )));
    }
    if let Some(f) = launch.files.iter().find(|f| f.rel.starts_with('/')) {
        return Err(Error::Unsupported(format!(
            "{} writes {} outside the run directory",
            h.name, f.rel
        )));
    }
    let mut env: BTreeMap<String, String> = opts.env.clone();
    // A copied Codex sign-in is a file for the guest, never agent env (a
    // durable driver doesn't write it yet).
    env.remove(crate::runs::CODEX_AUTH_ENV);
    let mut mcp: Vec<Value> = vec![];
    for (i, s) in opts.mcp_servers.iter().enumerate() {
        if let Some(url) = &s.url {
            let headers: Vec<Value> = s
                .headers
                .iter()
                .enumerate()
                .map(|(j, (k, v))| {
                    let name = format!("CUA_MCP_{i}_{j}");
                    env.insert(name.clone(), v.clone());
                    json!({"name": k, "value": format!("${{{name}}}")})
                })
                .collect();
            mcp.push(json!({"type": "http", "name": s.name, "url": url, "headers": headers}));
        } else if let Some(cmd) = &s.command {
            let senv: Vec<Value> = s
                .env
                .iter()
                .enumerate()
                .map(|(j, (k, v))| {
                    let name = format!("CUA_MCP_{i}_E{j}");
                    env.insert(name.clone(), v.clone());
                    json!({"name": k, "value": format!("${{{name}}}")})
                })
                .collect();
            mcp.push(json!({"name": s.name, "command": cmd, "args": s.args, "env": senv}));
        } else {
            return Err(Error::Invalid(format!(
                "MCP server {:?} needs a url or a command",
                s.name
            )));
        }
    }
    let mut agent_env: BTreeMap<String, String> = launch.env.iter().cloned().collect();
    agent_env.insert(
        "PATH".into(),
        "${HOME}/.cua/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
    );
    let mut args: Vec<String> = h.acp[1..].iter().map(|s| s.to_string()).collect();
    args.extend(launch.args.iter().cloned());
    let secret_env: Vec<&String> = env.keys().collect();
    let mut run = json!({
        "harness": h.id,
        "agent": {"command": format!("~/.cua/bin/{}", h.acp[0]), "args": args},
        "cwd": cwd,
        "env": agent_env,
        "prompt": prompt,
        "files": [],
        "mcpServers": mcp,
        "exitWhenIdle": true,
        "idleExitSecs": opts.idle_exit_secs.unwrap_or(1800),
        "authMethod": h.auth_method,
        "keyCheck": harness::key_check(h, &opts.env, endpoint.is_some()),
        "mcpBridge": "~/.cua/bin/mcp-remote",
        "modeKinds": h.mode_kinds,
        "guardModeKinds": guard_mode_kinds(h),
        "secretEnv": secret_env,
        "durable": d.run_section(Some(&state_dir)),
    });
    // The state dir and the epoch come from the env contract at run time.
    run["durable"]["epoch"] = json!(d.epoch.max(1));

    let mut ids: Vec<&str> = vec!["acp-sdk", "mcp-remote"];
    ids.extend(h.installs.iter().copied());
    let mut tar = tar::Builder::new(Vec::new());
    tar_file(
        &mut tar,
        "acp-runner.mjs",
        crate::runs::RUNNER.as_bytes(),
        0o644,
    )?;
    tar_file(
        &mut tar,
        "driver.sh",
        driver_script(&ids, opts.install.unwrap_or(true))?.as_bytes(),
        0o755,
    )?;
    tar_file(
        &mut tar,
        "run.json",
        &serde_json::to_vec_pretty(&run)?,
        0o600,
    )?;
    let mut files = launch.files.clone();
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    for f in &files {
        tar_file(&mut tar, &f.rel, f.body.as_bytes(), 0o600)?;
    }
    let raw = tar.into_inner()?;
    let mut gz = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::new(9));
    gz.write_all(&raw)?;
    let gz = gz.finish()?;
    if gz.len() > MAX_BUNDLE_BYTES {
        return Err(Error::Invalid(format!(
            "the driver bundle is {} bytes (limit {MAX_BUNDLE_BYTES})",
            gz.len()
        )));
    }
    Ok(DriverSpec {
        argv: vec!["/bin/sh".into(), "driver.sh".into()],
        env,
        bundle_bytes: gz.len(),
        bundle: base64::engine::general_purpose::STANDARD.encode(&gz),
        state_dir,
        label: h.id.into(),
    })
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Guest(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{McpServer, RunOptions};
    use std::io::Read;

    const KEY: &str = "sk-ant-secret-0123456789abcdef";

    fn opts() -> RunOptions {
        RunOptions {
            env: BTreeMap::from([("ANTHROPIC_API_KEY".into(), KEY.into())]),
            mcp_servers: vec![McpServer {
                name: "x".into(),
                url: Some("https://mcp.example/mcp".into()),
                headers: BTreeMap::from([(
                    "Authorization".into(),
                    "Bearer hdr-secret-98765".into(),
                )]),
                ..Default::default()
            }],
            durable: Some(DurableOptions {
                session: Some("nightly".into()),
                ..Default::default()
            }),
            install: Some(false),
            ..Default::default()
        }
    }

    fn unpack(spec: &DriverSpec) -> BTreeMap<String, Vec<u8>> {
        let gz = base64::engine::general_purpose::STANDARD
            .decode(&spec.bundle)
            .unwrap();
        let mut raw = vec![];
        flate2::read::GzDecoder::new(&gz[..])
            .read_to_end(&mut raw)
            .unwrap();
        let mut out = BTreeMap::new();
        for e in tar::Archive::new(&raw[..]).entries().unwrap() {
            let mut e = e.unwrap();
            let name = e.path().unwrap().to_string_lossy().into_owned();
            let mut b = vec![];
            e.read_to_end(&mut b).unwrap();
            out.insert(name, b);
        }
        out
    }

    #[test]
    fn the_spec_has_no_secret_in_argv_or_bundle() {
        let spec = durable_driver_spec("claude-code", "fix the bug", &opts()).unwrap();
        assert_eq!(spec.argv, ["/bin/sh", "driver.sh"]);
        for a in &spec.argv {
            assert!(!a.contains("secret"));
        }
        // The secrets are in the env (the job's sealed env) and nowhere else.
        assert_eq!(spec.env["ANTHROPIC_API_KEY"], KEY);
        assert_eq!(spec.env["CUA_MCP_0_0"], "Bearer hdr-secret-98765");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&spec.bundle)
            .unwrap();
        let files = unpack(&spec);
        assert_eq!(
            files.keys().cloned().collect::<Vec<_>>(),
            ["acp-runner.mjs", "driver.sh", "run.json"]
        );
        for (name, body) in &files {
            let text = String::from_utf8_lossy(body);
            assert!(!text.contains(KEY), "{name}");
            assert!(!text.contains("hdr-secret"), "{name}");
        }
        assert!(!String::from_utf8_lossy(&raw).contains(KEY));
        // The run config names the secrets so the runner redacts them.
        let run: Value = serde_json::from_slice(&files["run.json"]).unwrap();
        let names: Vec<&str> = run["secretEnv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(names.contains(&"ANTHROPIC_API_KEY") && names.contains(&"CUA_MCP_0_0"));
        assert_eq!(
            run["mcpServers"][0]["headers"][0]["value"],
            "${CUA_MCP_0_0}"
        );
        assert_eq!(run["durable"]["policy"], "ask-model");
        assert_eq!(run["durable"]["stateDir"], "~/.cua/durable/nightly");
        assert_eq!(run["exitWhenIdle"], true);
        assert_eq!(run["prompt"], "fix the bug");
        assert_eq!(spec.state_dir, "~/.cua/durable/nightly");
        assert_eq!(spec.label, "claude-code");
    }

    #[test]
    fn the_spec_is_deterministic_and_small() {
        let a = durable_driver_spec("openai-codex", "p", &opts()).unwrap();
        let b = durable_driver_spec("openai-codex", "p", &opts()).unwrap();
        assert_eq!(a, b);
        assert!(a.bundle_bytes < MAX_BUNDLE_BYTES, "{}", a.bundle_bytes);
        assert!(a.bundle_bytes < 100 * 1024, "{}", a.bundle_bytes);
        let c = durable_driver_spec("openai-codex", "another prompt", &opts()).unwrap();
        assert_ne!(a.bundle, c.bundle);
    }

    #[test]
    fn the_bundle_carries_the_runner_it_was_built_with() {
        let spec = durable_driver_spec("claude-code", "p", &opts()).unwrap();
        assert_eq!(
            unpack(&spec)["acp-runner.mjs"],
            crate::runs::RUNNER.as_bytes()
        );
        let sh = String::from_utf8(unpack(&spec)["driver.sh"].clone()).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-n")
            .arg("-c")
            .arg(&sh)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        for name in ENV_CONTRACT
            .iter()
            .filter(|n| n.ends_with("EPOCH") || n.ends_with("STATE_DIR"))
        {
            assert!(sh.contains(name));
        }
    }

    #[test]
    fn every_ready_harness_has_a_spec_or_says_why_not() {
        for h in harness::HARNESSES.iter().filter(|h| h.ready) {
            let r = durable_driver_spec(h.id, "p", &opts());
            match r {
                Ok(spec) => {
                    assert!(spec.bundle_bytes < MAX_BUNDLE_BYTES, "{}", h.id);
                    let run: Value = serde_json::from_slice(&unpack(&spec)["run.json"]).unwrap();
                    assert!(
                        !run.to_string().contains(KEY),
                        "{} run.json carries the key",
                        h.id
                    );
                }
                Err(Error::Unsupported(why)) => {
                    assert!(!why.is_empty(), "{}", h.id);
                    assert_eq!(h.id, "openclaw", "only the wrapper harness is refused");
                }
                Err(e) => panic!("{}: {e}", h.id),
            }
        }
    }

    #[test]
    fn a_permanent_install_failure_exits_78_and_a_transient_one_exits_1() {
        let run = |detail: &str| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("install.log"),
                format!("{{\"id\":\"hermes\",\"phase\":\"error\",\"detail\":\"{detail}\"}}\n"),
            )
            .unwrap();
            let out = std::process::Command::new("/bin/sh")
                .current_dir(dir.path())
                .arg("-c")
                .arg(format!("false || {{ {} }}", install_failure_handler()))
                .output()
                .unwrap();
            (out.status.code(), String::from_utf8_lossy(&out.stderr).to_string())
        };
        let (code, err) = run("git is not available");
        assert_eq!(code, Some(78), "{err}");
        assert!(err.contains("cannot be installed in the job Space"), "{err}");
        assert_eq!(run("commit mismatch").0, Some(78));
        assert_eq!(run("download failed").0, Some(1));
        assert_eq!(run("uv sync failed").0, Some(1));
    }

    #[test]
    fn the_state_dir_defaults_and_may_be_left_to_the_job() {
        let mut o = opts();
        o.durable.as_mut().unwrap().session = None;
        let spec = durable_driver_spec("claude-code", "p", &o).unwrap();
        assert_eq!(spec.state_dir, "");
        o.durable.as_mut().unwrap().state_dir = Some("/data/s".into());
        let spec = durable_driver_spec("claude-code", "p", &o).unwrap();
        assert_eq!(spec.state_dir, "/data/s");
        o.durable = None;
        assert!(durable_driver_spec("claude-code", "p", &o).is_err());
        let mut o = opts();
        o.durable.as_mut().unwrap().session = Some("Bad Name".into());
        assert!(durable_driver_spec("claude-code", "p", &o).is_err());
    }

    #[test]
    fn redrive_raises_the_epoch_only() {
        let raw = serde_json::to_vec(
            &json!({"durable": {"epoch": 2, "resume": false, "stateDir": "/s"}, "x": 1}),
        )
        .unwrap();
        let next: Value = serde_json::from_slice(&redrive_run_json(&raw, 3).unwrap()).unwrap();
        assert_eq!(next["durable"]["epoch"], 3);
        assert_eq!(next["durable"]["resume"], true);
        assert_eq!(next["durable"]["stateDir"], "/s");
        assert_eq!(next["x"], 1);
        assert!(redrive_run_json(&raw, 2).is_err());
        assert!(redrive_run_json(b"{\"a\":1}", 3).is_err());
    }

    /// The ledger, the env contract and the exit codes are one contract with
    /// the runner and the coordinator; this keeps the Rust names and the
    /// runner's in step.
    #[test]
    fn the_runner_and_the_rust_side_agree() {
        let runner = crate::runs::RUNNER;
        for name in ENV_CONTRACT {
            assert!(runner.contains(name), "the runner does not read {name}");
        }
        assert!(runner.contains(&format!("const EXIT_PERMANENT = {}", exit::PERMANENT)));
        assert!(runner.contains(&format!("const EXIT_FENCED = {}", exit::FENCED)));
        for f in JOURNAL_FILES {
            assert!(
                runner.contains(f.trim_end_matches(".jsonl")),
                "the runner never writes {f}"
            );
        }
        for p in [Policy::AskModel, Policy::RerunSafe] {
            assert!(runner.contains(p.as_str()));
        }
        // The ship ack: the runner reads the file the host writes, by the
        // keys cua_volume::journal ships.
        for k in ["ackFile", "ackTimeoutMs", "tools.jsonl"] {
            assert!(runner.contains(k), "the runner does not read {k}");
        }
    }

    #[test]
    fn every_ready_harness_has_a_coverage_row() {
        for h in harness::HARNESSES.iter().filter(|h| h.ready) {
            assert!(
                COVERAGE.iter().any(|c| c.harness == h.id),
                "no durable coverage row for {}",
                h.id
            );
        }
        for c in COVERAGE {
            assert!(harness::harness(c.harness).is_some(), "{}", c.harness);
        }
    }

    #[test]
    fn guard_selects_the_ask_mode_for_codex_and_reaches_run_json() {
        let spec = durable_driver_spec("openai-codex", "p", &opts()).unwrap();
        let run: Value = serde_json::from_slice(&unpack(&spec)["run.json"]).unwrap();
        assert_eq!(run["guardModeKinds"], json!(["standard"]));
        assert_eq!(run["durable"]["guard"], true);
        // The launch carries no -c overrides: codex-acp ignores them.
        assert!(!run["agent"]["args"].to_string().contains("approval_policy"));
        assert!(guard_mode_kinds(harness::harness("claude-code").unwrap()).is_empty());
    }

    #[test]
    fn guard_makes_opencode_ask() {
        let spec = durable_driver_spec("opencode", "p", &opts()).unwrap();
        let run: Value = serde_json::from_slice(&unpack(&spec)["run.json"]).unwrap();
        assert!(
            run["env"]["OPENCODE_PERMISSION"]
                .as_str()
                .unwrap()
                .contains("\"bash\":\"ask\"")
        );
        let mut o = opts();
        o.durable.as_mut().unwrap().guard = false;
        let spec = durable_driver_spec("opencode", "p", &o).unwrap();
        let run: Value = serde_json::from_slice(&unpack(&spec)["run.json"]).unwrap();
        assert!(run["env"].get("OPENCODE_PERMISSION").is_none());
    }

    #[test]
    fn run_section_has_no_secret_and_defaults_the_state_dir() {
        let d = DurableOptions {
            epoch: 4,
            resume: true,
            ..Default::default()
        };
        let v = d.run_section(Some("/runs/r1"));
        assert_eq!(v["stateDir"], "/runs/r1");
        assert_eq!(v["epoch"], 4);
        assert_eq!(v["maxAgentRestarts"], 3);
        assert!(v["ship"].is_null());
        let d = DurableOptions {
            ship: Some(Ship::default()),
            ..Default::default()
        };
        let v = d.run_section(None);
        assert_eq!(v["ship"]["ackFile"], SHIP_ACK_FILE);
        assert_eq!(v["ship"]["ackTimeoutMs"], 30_000);
    }
}
