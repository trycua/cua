// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua run FILE::FN` and `cua jobs`: Python functions as gang runs on your
//! pool (DESIGN item 12).
//!
//! - The function's spec comes from the decorator: the local `python3`
//!   imports FILE with the embedded pure-Python `cua` shim first on its path
//!   (`python3 -m _cua_entry --spec FILE FN`); nothing is installed.
//! - FILE's directory (`*.py`, `*.json`, `*.jsonl`, `*.txt`, minus
//!   `.cuaignore`) and the shim (as `_cua_runtime/`) go with the run as a
//!   gzip tar of at most 1 MiB.
//! - Every member runs in a Space: the decorator's `kind`, or the one its
//!   host picks (a macOS VM with GPU on a Mac, the machine's own Space on a
//!   dedicated host, a container elsewhere), with `uv` first on its `PATH`:
//!   `uv run --no-project --python PY python -m _cua_entry --launch FILE FN`;
//!   the shim picks the host's packages (mlx -> mlx[cuda] on a Linux host
//!   with an NVIDIA GPU) and execs `uv run ... --with PKG... python -m
//!   _cua_entry FILE FN`.
//! - `cua run` follows every rank's output (`[rank R host]`), says when the
//!   gang is placed again, and saves rank 0's result in
//!   `./cua-runs/<run>/`.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use clap::{Args, Subcommand};
use cua_sdk::CuaError;
use serde_json::{Value, json};

use crate::runs::{request, target_of_run};
use crate::util::{hint, hint_line, line};

/// The jobs runtime every member runs with (`import cua`, `python -m
/// _cua_entry`), by path inside `_cua_runtime/`.
pub const SHIM_FILES: &[(&str, &str)] = &[
    (
        "cua/__init__.py",
        include_str!("../jobs-runtime/cua/__init__.py"),
    ),
    (
        "cua/_store.py",
        include_str!("../jobs-runtime/cua/_store.py"),
    ),
    (
        "cua/checkpoint.py",
        include_str!("../jobs-runtime/cua/checkpoint.py"),
    ),
    (
        "cua/progress.py",
        include_str!("../jobs-runtime/cua/progress.py"),
    ),
    (
        "_cua_entry.py",
        include_str!("../jobs-runtime/_cua_entry.py"),
    ),
];
/// The largest bundle a run carries (the relay stores sealed job specs).
pub const MAX_BUNDLE: usize = 1 << 20;
/// Where rank 0's result goes in its job dir (a gzip tar the shim writes).
const RESULT_PATH: &str = "cua-result.tar.gz";
/// The largest result a run returns (an adapter is a few MB).
const MAX_RESULT: u64 = 16 << 20;

/// `cua run`.
#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    /// `FILE::FN`: a function decorated with `@app.function(...)` in FILE.
    /// Or leave it out and give a command after `--`.
    #[arg(required_unless_present = "command")]
    pub target: Option<String>,
    /// A command to run in a Space instead of `FILE::FN` (`cua run -- CMD
    /// ARGS...`); the current directory goes with it like FILE's does.
    #[arg(last = true)]
    pub command: Vec<String>,
    /// Give the command another provider's sandbox API on Cua Spaces:
    /// `e2b`, `daytona` or `modal`. Its SDK finds the gateway in the
    /// environment (E2B_API_URL, E2B_API_KEY, ...; DAYTONA_API_URL,
    /// DAYTONA_API_KEY; MODAL_SERVER_URL, MODAL_TOKEN_ID,
    /// MODAL_TOKEN_SECRET) and needs no code change; the sandboxes it
    /// makes are Cua Spaces on your pool.
    #[arg(long, requires = "command")]
    pub compat: Option<String>,
    /// The command's Space kind (`container`, `vm`, `microvm`; default:
    /// the host's pick). Command form only.
    #[arg(long, requires = "command")]
    pub kind: Option<String>,
    /// The command's Space image (default: the host's). Command form only.
    #[arg(long, requires = "command")]
    pub image: Option<String>,
    /// Members of the gang, `N` or `MIN:MAX` (default: the decorator's).
    #[arg(long)]
    pub nodes: Option<String>,
    /// Where: `local` (this machine; the default for a command), `auto`
    /// (any of your hosts) or `host:NAME` (default for a function: the
    /// decorator's).
    #[arg(long)]
    pub on: Option<String>,
    /// `gpu` (any), `apple`, `nvidia`, or `none` (default: the decorator's).
    #[arg(long)]
    pub accelerator: Option<String>,
    /// Times the gang is placed again after a failure or a lost computer
    /// (default: the decorator's).
    #[arg(long)]
    pub retries: Option<u32>,
    /// CPUs reserved for each member.
    #[arg(long, default_value_t = 2.0)]
    pub cpus: f64,
    /// Memory reserved for each member, in MiB.
    #[arg(long, default_value_t = 4096)]
    pub memory: u64,
    /// `KEY=VALUE` for every member's environment (over the decorator's
    /// `env=`), or `KEY` alone for this shell's value (secrets: never
    /// printed). Repeatable.
    #[arg(long = "env", short = 'e', value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// The run's id (default: a new one).
    #[arg(long)]
    pub id: Option<String>,
    /// Submit, print the run's id and return.
    #[arg(long)]
    pub detach: bool,
}

/// `cua jobs`.
#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct JobsArgs {
    #[command(subcommand)]
    pub cmd: Option<JobsCmd>,
    /// A run: its members, attempts, hosts, state and cost.
    pub run: Option<String>,
}

/// `cua jobs <cmd>`.
#[derive(Subcommand, Debug)]
pub enum JobsCmd {
    /// A run's output, every rank (`[rank R host]`) or one.
    #[command(after_help = "Examples:
  cua jobs logs -f p-finetune-1a2b3c4d
  cua jobs logs p-finetune-1a2b3c4d --rank 1")]
    Logs {
        /// The run.
        run: String,
        /// Keep following until the run ends.
        #[arg(long, short)]
        follow: bool,
        /// Only this rank.
        #[arg(long)]
        rank: Option<u32>,
    },
    /// Cancel a run (every member stops).
    #[command(after_help = "Examples:
  cua jobs cancel p-finetune-1a2b3c4d")]
    Cancel {
        /// The run.
        run: String,
    },
    /// Save rank 0's result.
    #[command(after_help = "Examples:
  cua jobs result p-finetune-1a2b3c4d
  cua jobs result p-finetune-1a2b3c4d -o ./adapter")]
    Result {
        /// The run.
        run: String,
        /// Directory (default ./cua-runs/<run>).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
}

/// `FILE::FN`.
pub fn split_target(t: &str) -> Result<(PathBuf, String), CuaError> {
    let (file, func) = t.rsplit_once("::").ok_or_else(|| {
        CuaError::InvalidArgument(format!("{t}: use FILE::FUNCTION, like train.py::finetune"))
    })?;
    if file.is_empty() || func.is_empty() || !func.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return Err(CuaError::InvalidArgument(format!(
            "{t}: use FILE::FUNCTION, like train.py::finetune"
        )));
    }
    Ok((PathBuf::from(file), func.to_string()))
}

/// Writes the jobs runtime ([`SHIM_FILES`]) into `dir`.
pub fn write_shim(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join("cua"))?;
    for (name, data) in SHIM_FILES {
        std::fs::write(dir.join(name), data)?;
    }
    Ok(())
}

/// The decorator's spec of `func` in `file`, from the local Python.
pub fn read_spec(file: &Path, func: &str) -> Result<Value, CuaError> {
    let tmp = tempfile::tempdir().map_err(|e| CuaError::Internal(e.to_string()))?;
    write_shim(tmp.path()).map_err(|e| CuaError::Internal(e.to_string()))?;
    let python = std::env::var("CUA_PYTHON").unwrap_or_else(|_| "python3".into());
    let dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let out = std::process::Command::new(&python)
        .args(["-m", "_cua_entry", "--spec"])
        .arg(file)
        .arg(func)
        .current_dir(dir)
        .env("PYTHONPATH", tmp.path())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .map_err(|e| {
            CuaError::InvalidArgument(format!(
                "{python}: {e} (cua run reads the decorator with a local Python 3.10 or newer; set CUA_PYTHON)"
            ))
        })?;
    if !out.status.success() {
        return Err(CuaError::InvalidArgument(format!(
            "reading {}::{func}: {}",
            file.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| CuaError::Internal(format!("the spec of {func}: {e}")))
}

/// A `.cuaignore` pattern match (`*`, `?`; a pattern with `/` matches the
/// relative path, otherwise any path component; a trailing `/`: a
/// directory and everything under it).
pub fn ignored(rel: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        let p = p.trim_start_matches('/');
        if let Some(d) = p.strip_suffix('/') {
            return rel.split('/').any(|c| glob(d, c)) || rel.starts_with(&format!("{d}/"));
        }
        if p.contains('/') {
            glob(p, rel)
        } else {
            rel.split('/').any(|c| glob(p, c))
        }
    })
}

fn glob(p: &str, s: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], s) || (!s.is_empty() && go(p, &s[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &s[1..]),
            _ => false,
        }
    }
    go(p.as_bytes(), s.as_bytes())
}

/// Whether `name` goes in a bundle.
fn bundled(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    matches!(ext, "py" | "json" | "jsonl" | "txt") || name.starts_with("requirements")
}

/// The files of `dir` that go with a run (relative paths, sorted).
pub fn bundle_files(dir: &Path) -> Result<Vec<String>, CuaError> {
    let patterns: Vec<String> = std::fs::read_to_string(dir.join(".cuaignore"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let rd = std::fs::read_dir(dir.join(&rel))
            .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", dir.display())))?;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let r = if rel.as_os_str().is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", rel.to_string_lossy())
            };
            let ft = match e.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if name.starts_with('.') || ignored(&r, &patterns) {
                continue;
            }
            if ft.is_dir() {
                if !matches!(
                    name.as_str(),
                    "__pycache__" | "cua-runs" | "node_modules" | "venv" | "_cua_runtime"
                ) {
                    stack.push(PathBuf::from(&r));
                }
            } else if ft.is_file() && bundled(&name) {
                out.push(r);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The run's bundle: `files` of `dir` plus the shim as `_cua_runtime/`, a
/// gzip tar of at most [`MAX_BUNDLE`].
pub fn bundle(dir: &Path, files: &[String]) -> Result<Vec<u8>, CuaError> {
    let io = |e: std::io::Error| CuaError::Internal(format!("bundle: {e}"));
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    let mut add = |name: &str, data: &[u8]| -> std::io::Result<()> {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append_data(&mut h, name, data)
    };
    for f in files {
        let data = std::fs::read(dir.join(f)).map_err(io)?;
        add(f, &data).map_err(io)?;
    }
    for (name, data) in SHIM_FILES {
        add(&format!("_cua_runtime/{name}"), data.as_bytes()).map_err(io)?;
    }
    let bytes = tar.into_inner().and_then(|g| g.finish()).map_err(io)?;
    if bytes.len() > MAX_BUNDLE {
        let mut sizes: Vec<(u64, &String)> = files
            .iter()
            .map(|f| {
                (
                    std::fs::metadata(dir.join(f)).map(|m| m.len()).unwrap_or(0),
                    f,
                )
            })
            .collect();
        sizes.sort_by_key(|s| std::cmp::Reverse(s.0));
        let big: Vec<String> = sizes
            .iter()
            .take(3)
            .map(|(n, f)| format!("{f} ({} KB)", n / 1024))
            .collect();
        return Err(CuaError::InvalidArgument(format!(
            "{} is {} KB compressed; a run carries at most 1024 KB (largest: {}). \
             Leave files out with a .cuaignore, or have the job download them.",
            dir.display(),
            bytes.len() / 1024,
            big.join(", ")
        )));
    }
    Ok(bytes)
}

/// `--nodes`: `N` or `MIN:MAX`.
pub fn parse_nodes(s: &str) -> Result<(u32, u32), CuaError> {
    let bad = || CuaError::InvalidArgument(format!("--nodes {s}: use N or MIN:MAX (1:2)"));
    let (a, b) = match s.split_once(':') {
        Some((a, b)) => (
            a.trim().parse().map_err(|_| bad())?,
            b.trim().parse().map_err(|_| bad())?,
        ),
        None => {
            let n = s.trim().parse().map_err(|_| bad())?;
            (n, n)
        }
    };
    if a < 1 || b < a {
        return Err(bad());
    }
    Ok((a, b))
}

/// The pool's `on` for a decorator or `--on` value.
pub fn placement(on: &str) -> Result<String, CuaError> {
    let on = on.trim();
    if let Some(c) = crate::runs::cloud_on(on) {
        return c;
    }
    match on {
        "" | "auto" => Ok(String::new()),
        // This machine, by the name it has in its own pool.
        "local" => Ok("local".to_string()),
        _ if on.starts_with("host:") => Ok(on["host:".len()..].to_string()),
        _ => Err(CuaError::InvalidArgument(format!(
            "on={on:?}: use auto, a cloud (aws, gcp, hetzner) or host:NAME"
        ))),
    }
}

/// The submit request of a run.
pub struct Submit {
    pub run: String,
    pub file: String,
    pub func: String,
    pub spec: Value,
    pub nodes: (u32, u32),
    pub on: String,
    pub accelerator: String,
    pub retries: u32,
    /// The decorator's `Retries` (backoff), with `retries` as max_retries.
    pub retry: Value,
    pub cpus: f64,
    pub memory_mb: u64,
    pub bundle: Vec<u8>,
    pub secrets: BTreeMap<String, String>,
}

impl Submit {
    /// The `submit` params.
    pub fn params(&self) -> Value {
        let image = &self.spec["image"];
        let python = image["python"].as_str().unwrap_or("3.12").to_string();
        let mut env = serde_json::Map::new();
        if let Some(e) = self.spec["env"].as_object() {
            for (k, v) in e {
                env.insert(k.clone(), json!(v.as_str().unwrap_or_default()));
            }
        }
        for (k, v) in &self.secrets {
            env.insert(k.clone(), json!(v));
        }
        env.insert("PYTHONPATH".into(), json!("_cua_runtime"));
        env.insert("PYTHONUNBUFFERED".into(), json!("1"));
        env.insert("CUA_JOB_PYTHON".into(), json!(python));
        env.insert(
            "CUA_JOB_PACKAGES".into(),
            json!(image["packages"].clone().to_string()),
        );
        // Recorded so the rewrite a host applies is visible in the job.
        env.insert(
            "CUA_JOB_CUDA_REWRITES".into(),
            json!(image["cuda_rewrites"].clone().to_string()),
        );
        json!({
            "run": self.run,
            "gang": true,
            "nodes": {"min": self.nodes.0, "max": self.nodes.1},
            // The decorator's `kind`; "" lets each host pick the Space: a
            // macOS VM on a Mac, the machine's own Space on a dedicated host,
            // a container elsewhere.
            "kind": self.spec["kind"].as_str().unwrap_or(""),
            "command": [
                "uv", "run", "--no-project", "--python", python,
                "python", "-m", "_cua_entry", "--launch", self.file, self.func,
            ],
            "env": env,
            "bundle": base64::engine::general_purpose::STANDARD.encode(&self.bundle),
            "accelerator": self.accelerator,
            "cpus": self.cpus,
            "memory_mb": self.memory_mb,
            "timeout_secs": self.spec["timeout_secs"].as_u64().unwrap_or(7200),
            "retries": self.retries,
            "retry": self.retry,
            // Lost machines, the start barrier and the stall watchdog.
            "infra_retries": self.spec["infra_retries"].as_u64().unwrap_or(10),
            "barrier_timeout_secs": self.spec["barrier_timeout_secs"].as_u64().unwrap_or(900),
            "stall_timeout_secs": self.spec["stall_timeout_secs"].as_u64().unwrap_or(600),
            "result_path": RESULT_PATH,
            "max_output_bytes": MAX_RESULT,
            "on": self.on,
        })
    }
}

/// Errors after which a submit may have landed (send it again, same id).
fn transient(e: &CuaError) -> bool {
    matches!(e, CuaError::Timeout(_) | CuaError::Internal(_))
}

/// Sends `params` until the coordinator of its run (this machine's for an
/// `l-` run, the pool's for a `p-` one) acknowledges it (the same run id
/// makes a repeat a no-op), at most 4 times.
async fn submit_acknowledged(params: &Value) -> Result<Value, CuaError> {
    let target = target_of_run(params["run"].as_str().unwrap_or_default());
    let mut delay = Duration::from_secs(1);
    for i in 0.. {
        match request(target, "submit", params.clone()).await {
            Err(e) if transient(&e) && i < 3 => {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            r => return r,
        }
    }
    unreachable!()
}

/// The key of a submission (hex): file, function, bundle, spec and flags.
pub fn submission_key(file: &str, func: &str, bundle: &[u8], spec: &Value, a: &RunArgs) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    for part in [
        file.as_bytes(),
        func.as_bytes(),
        bundle,
        spec.to_string().as_bytes(),
        format!(
            "{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}",
            a.nodes, a.on, a.accelerator, a.retries, a.cpus, a.memory, a.env
        )
        .as_bytes(),
    ] {
        h.update((part.len() as u64).to_le_bytes());
        h.update(part);
    }
    h.finalize()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A run id saved for `key` whose submit was not acknowledged.
pub fn pending_id(dir: &Path, key: &str) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| std::fs::read_to_string(e.path()).is_ok_and(|k| k.trim() == key))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|id| id.starts_with("p-") || id.starts_with("l-"))
}

/// Saves `run_id` for `key` before its submit is sent.
pub fn save_pending(dir: &Path, run_id: &str, key: &str) -> Result<(), CuaError> {
    let io = |e: std::io::Error| CuaError::Internal(format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    let tmp = dir.join(format!(".{run_id}.tmp"));
    std::fs::write(&tmp, format!("{key}\n")).map_err(io)?;
    std::fs::rename(&tmp, dir.join(run_id)).map_err(io)
}

/// The run id prefix for placement `on` (as [`placement`] gives it): `l-`
/// for this machine (`local`: its own coordinator and ledger; nothing
/// about the run leaves the machine, and it needs none of the pool's
/// keys), `p-` for the account's machines.
pub fn run_prefix(on: &str) -> &'static str {
    if on == "local" { "l-" } else { "p-" }
}

/// Checks that a given `--id` fits placement `on`.
fn check_id(id: &str, on: &str) -> Result<(), CuaError> {
    let want = run_prefix(on);
    if id.starts_with(want) {
        return Ok(());
    }
    Err(CuaError::InvalidArgument(format!(
        "--id {id}: {} start with {want}",
        if want == "l-" {
            "runs on this machine (--on local)"
        } else {
            "runs on your pool"
        }
    )))
}

fn new_run_id(prefix: &str, func: &str) -> String {
    let f: String = func
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(20)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("{prefix}{f}-{:08x}", rand::random::<u32>())
}

/// `cua run FILE::FN`.
pub async fn run(a: RunArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    if !a.command.is_empty() {
        if a.target.is_some() {
            return Err(CuaError::InvalidArgument(
                "give FILE::FN or a command after --, not both".into(),
            ));
        }
        return run_command(a, json, out).await;
    }
    let target = a.target.clone().unwrap_or_default();
    let (file, func) = split_target(&target)?;
    if !file.is_file() {
        return Err(CuaError::InvalidArgument(format!(
            "{}: no such file",
            file.display()
        )));
    }
    let spec = read_spec(&file, &func)?;
    let dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let fname = file
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let files = bundle_files(&dir)?;
    if !files.contains(&fname) {
        return Err(CuaError::InvalidArgument(format!(
            "{fname} is left out of the bundle (.cuaignore)"
        )));
    }
    let bundle = bundle(&dir, &files)?;
    let nodes = match &a.nodes {
        Some(n) => parse_nodes(n)?,
        None => (
            spec["nodes"]["min"].as_u64().unwrap_or(1) as u32,
            spec["nodes"]["max"].as_u64().unwrap_or(1) as u32,
        ),
    };
    let on = placement(
        a.on.as_deref()
            .unwrap_or(spec["on"].as_str().unwrap_or("auto")),
    )?;
    let accelerator = match a.accelerator.as_deref() {
        Some("none") => String::new(),
        Some(x @ ("gpu" | "apple" | "nvidia")) => x.to_string(),
        Some(x) => {
            return Err(CuaError::InvalidArgument(format!(
                "--accelerator {x}: use gpu, apple, nvidia or none"
            )));
        }
        None => spec["accelerator"].as_str().unwrap_or("").to_string(),
    };
    let retries = a
        .retries
        .unwrap_or(spec["retries"]["max_retries"].as_u64().unwrap_or(3) as u32);
    let mut spec = spec;
    for kv in &a.env {
        let (k, v) = kv
            .split_once('=')
            .filter(|(k, _)| !k.is_empty())
            .ok_or_else(|| CuaError::InvalidArgument(format!("--env {kv}: use KEY=VALUE")))?;
        spec["env"][k] = json!(v);
    }
    let mut secrets = BTreeMap::new();
    for name in spec["secrets"].as_array().cloned().unwrap_or_default() {
        let name = name.as_str().unwrap_or_default().to_string();
        let v = std::env::var(&name).map_err(|_| {
            CuaError::InvalidArgument(format!(
                "{func} needs {name} from this shell (Secret.from_local_env): it is not set"
            ))
        })?;
        secrets.insert(name, v);
    }
    // What this submission is: the same file, function, bundle and spec
    // give the same key, so a submit whose acknowledgement was lost (this
    // process died, or the answer never came) is sent again under its id.
    let key = submission_key(&fname, &func, &bundle, &spec, &a);
    let pending_dir = PathBuf::from("cua-runs").join(".pending");
    let reused = if a.id.is_none() {
        pending_id(&pending_dir, &key)
    } else {
        None
    };
    let run_id = match (&a.id, &reused) {
        (Some(id), _) => id.clone(),
        (None, Some(id)) if id.starts_with(run_prefix(&on)) => id.clone(),
        (None, _) => new_run_id(run_prefix(&on), &func),
    };
    check_id(&run_id, &on)?;
    if reused.is_some() && !json {
        line(
            out,
            format!("{run_id}: the last submit of this job was not acknowledged; sending it again"),
        );
    }
    let r = &spec["retries"];
    let retry = json!({
        "max_retries": retries,
        "backoff_coefficient": r["backoff_coefficient"].as_f64().unwrap_or(2.0),
        "initial_delay": r["initial_delay"].as_f64().unwrap_or(2.0),
        "max_delay": r["max_delay"].as_f64().unwrap_or(60.0),
    });
    let s = Submit {
        run: run_id.clone(),
        file: fname.clone(),
        func: func.clone(),
        spec: spec.clone(),
        nodes,
        on,
        accelerator: accelerator.clone(),
        retries,
        retry,
        cpus: a.cpus,
        memory_mb: a.memory,
        bundle,
        secrets,
    };
    save_pending(&pending_dir, &run_id, &key)?;
    submit_acknowledged(&s.params()).await.inspect_err(|e| {
        // Refused (not lost): nothing to send again.
        if !transient(e) {
            let _ = std::fs::remove_file(pending_dir.join(&run_id));
        }
    })?;
    let _ = std::fs::remove_file(pending_dir.join(&run_id));
    let what = format!(
        "{run_id}: {fname}::{func} on {} member{}{}{} ({} files, {} KB)",
        if nodes.0 == nodes.1 {
            nodes.0.to_string()
        } else {
            format!("{}-{}", nodes.0, nodes.1)
        },
        if nodes.1 == 1 { "" } else { "s" },
        if accelerator.is_empty() {
            String::new()
        } else {
            format!(", accelerator {accelerator}")
        },
        if retries > 0 {
            format!(", up to {retries} retries")
        } else {
            String::new()
        },
        files.len(),
        s.bundle.len() / 1024
    );
    if a.detach {
        if json {
            line(out, json!({"run": run_id}).to_string());
        } else {
            line(out, format!("{what}: submitted"));
            hint_line(out, format!("Follow it: cua jobs logs -f {run_id}"));
        }
        return Ok(0);
    }
    if !json {
        for l in follow_hint(&run_id) {
            line(out, l);
        }
        line(out, what);
    }
    let started = Instant::now();
    let summary = follow(&run_id, None, json, out).await?;
    finish(&run_id, &summary, started.elapsed(), None, json, out).await
}

/// The files of a command run's directory: with git, what git tracks or
/// would track (`git ls-files -co --exclude-standard`); otherwise every
/// file but hidden ones, virtual environments and caches. `.cuaignore`
/// applies either way.
pub fn project_files(dir: &Path) -> Result<Vec<String>, CuaError> {
    let patterns: Vec<String> = std::fs::read_to_string(dir.join(".cuaignore"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    let git = std::process::Command::new("git")
        .args(["ls-files", "-z", "-co", "--exclude-standard"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success());
    let mut files: Vec<String> = match git {
        Some(o) => o
            .stdout
            .split(|b| *b == 0)
            .filter(|f| !f.is_empty())
            .map(|f| String::from_utf8_lossy(f).to_string())
            .filter(|f| dir.join(f).is_file())
            .collect(),
        None => {
            let mut out = Vec::new();
            let mut stack = vec![PathBuf::new()];
            while let Some(rel) = stack.pop() {
                let rd = std::fs::read_dir(dir.join(&rel))
                    .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", dir.display())))?;
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let r = if rel.as_os_str().is_empty() {
                        name.clone()
                    } else {
                        format!("{}/{name}", rel.to_string_lossy())
                    };
                    let Ok(ft) = e.file_type() else { continue };
                    if name.starts_with('.') {
                        continue;
                    }
                    if ft.is_dir() {
                        if !matches!(
                            name.as_str(),
                            "__pycache__" | "cua-runs" | "node_modules" | "venv" | "target"
                        ) {
                            stack.push(PathBuf::from(&r));
                        }
                    } else if ft.is_file() {
                        out.push(r);
                    }
                }
            }
            out
        }
    };
    files.retain(|f| !ignored(f, &patterns) && !f.starts_with("cua-runs/"));
    files.sort();
    Ok(files)
}

/// A command run's bundle: `files` of `dir` with their modes (scripts stay
/// executable), at most `cua_cluster::model::MAX_BUNDLE`.
pub fn project_bundle(dir: &Path, files: &[String]) -> Result<Vec<u8>, CuaError> {
    let io = |e: std::io::Error| CuaError::Internal(format!("bundle: {e}"));
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    for f in files {
        let p = dir.join(f);
        let data = std::fs::read(&p).map_err(io)?;
        let mut mode = 0o644;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&p)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
            {
                mode = 0o755;
            }
        }
        // A Windows file has no mode to keep, and the node that runs it does:
        // a script (it starts with `#!`) stays executable.
        #[cfg(not(unix))]
        {
            if data.starts_with(b"#!") {
                mode = 0o755;
            }
        }
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(mode);
        h.set_cksum();
        tar.append_data(&mut h, f, &data[..]).map_err(io)?;
    }
    let bytes = tar.into_inner().and_then(|g| g.finish()).map_err(io)?;
    let max = cua_cluster::model::MAX_BUNDLE;
    if bytes.len() > max {
        return Err(CuaError::InvalidArgument(format!(
            "this directory is {} KB compressed; a run takes at most {} KB: leave data out with .cuaignore",
            bytes.len() / 1024,
            max / 1024
        )));
    }
    Ok(bytes)
}

/// The command a member runs: a bare `python` in a uv project (a
/// `pyproject.toml` here) runs in it (`uv run [--frozen] --`, which installs
/// it from `uv.lock`); anything else as given.
pub fn member_command(command: &[String], files: &[String]) -> Vec<String> {
    let bare_python = matches!(
        command.first().map(String::as_str),
        Some("python" | "python3")
    );
    if !bare_python || !files.iter().any(|f| f == "pyproject.toml") {
        return command.to_vec();
    }
    let mut v = vec!["uv".to_string(), "run".to_string()];
    if files.iter().any(|f| f == "uv.lock") {
        v.push("--frozen".into());
    }
    v.push("--".into());
    v.extend(command.iter().cloned());
    v
}

/// The SDK a `--compat` run adds when the project does not list it: the
/// Python package and the exact version Cua tests (the newest in
/// `libs/cua/tests/compat/SDK_VERSIONS.json`, which a test here keeps equal), and the
/// npm package where there is one.
pub struct CompatSdk {
    pub py: &'static str,
    pub py_version: &'static str,
    pub npm: Option<(&'static str, &'static str)>,
}

pub fn compat_sdk(provider: &str) -> Option<CompatSdk> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "e2b" => Some(CompatSdk {
            py: "e2b",
            py_version: "2.58.0",
            npm: Some(("e2b", "2.58.0")),
        }),
        "daytona" => Some(CompatSdk {
            py: "daytona",
            py_version: "0.225.0",
            npm: Some(("@daytona/sdk", "0.225.0")),
        }),
        "modal" => Some(CompatSdk {
            py: "modal",
            py_version: "1.6.2",
            npm: None,
        }),
        _ => None,
    }
}

/// Whether a dependency file (`pyproject.toml`, `requirements.txt`,
/// `uv.lock`, `package.json`) names `package`.
fn lists_package(text: &str, package: &str) -> bool {
    let t = text.to_ascii_lowercase();
    let p = package.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = t[from..].find(&p) {
        let at = from + i;
        let before = t[..at].chars().next_back();
        let after = t[at + p.len()..].chars().next();
        let name_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.';
        if !before.is_some_and(name_char) && !after.is_some_and(name_char) {
            return true;
        }
        from = at + p.len();
    }
    false
}

/// The command of a `--compat` run, with the provider's SDK installed when
/// the project's own dependency files do not list it: a bare `python` runs
/// under `uv run` (`--with` the SDK, over the project's `pyproject.toml` or
/// `requirements.txt`, or with nothing else), a bare `node` after an
/// `npm install`. Returns the command and, when it added the SDK, the
/// notice to print. Anything else (a shell script, `uv run` itself) is the
/// user's own.
pub fn compat_command(
    command: &[String],
    files: &[String],
    dir: &Path,
    provider: &str,
) -> (Vec<String>, Option<String>) {
    let Some(sdk) = compat_sdk(provider) else {
        return (member_command(command, files), None);
    };
    let has = |f: &str| files.iter().any(|x| x == f);
    let read = |f: &str| {
        if has(f) {
            std::fs::read_to_string(dir.join(f)).unwrap_or_default()
        } else {
            String::new()
        }
    };
    match command.first().map(String::as_str) {
        Some("python" | "python3") => {
            let listed = ["pyproject.toml", "requirements.txt", "uv.lock"]
                .iter()
                .any(|f| lists_package(&read(f), sdk.py));
            let spec = format!("{}=={}", sdk.py, sdk.py_version);
            let mut v = vec!["uv".to_string(), "run".to_string()];
            if has("pyproject.toml") {
                if has("uv.lock") {
                    v.push("--frozen".into());
                }
            } else if has("requirements.txt") {
                v.extend(["--with-requirements".into(), "requirements.txt".into()]);
            }
            if !listed {
                v.extend(["--with".into(), spec.clone()]);
            }
            v.push("--".into());
            v.extend(command.iter().cloned());
            (
                v,
                (!listed).then(|| format!("adding {spec} (not in your dependencies)")),
            )
        }
        Some("node") if sdk.npm.is_some() => {
            let (pkg, version) = sdk.npm.unwrap();
            let listed = lists_package(&read("package.json"), pkg);
            let spec = format!("{pkg}@{version}");
            let install = if listed {
                "npm install --no-audit --no-fund --silent".to_string()
            } else {
                format!("npm install --no-audit --no-fund --silent {spec}")
            };
            let mut v = vec![
                "sh".to_string(),
                "-c".into(),
                format!("{install} && exec \"$@\""),
                "sh".into(),
            ];
            v.extend(command.iter().cloned());
            (
                v,
                (!listed).then(|| format!("adding {spec} (not in your dependencies)")),
            )
        }
        _ => (member_command(command, files), None),
    }
}

/// The job service `--compat <provider>` asks for.
pub fn compat_service(provider: &str) -> Result<String, CuaError> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "e2b" => Ok("compat:e2b".into()),
        "daytona" => Ok("compat:daytona".into()),
        "modal" => Ok("compat:modal".into()),
        other => Err(CuaError::InvalidArgument(format!(
            "--compat {other}: use e2b, daytona or modal"
        ))),
    }
}

/// The `submit` params of a command run (`cua run -- CMD`).
pub fn command_params(
    run: &str,
    command: &[String],
    a: &RunArgs,
    on: &str,
    bundle: &[u8],
    services: &[String],
) -> Result<Value, CuaError> {
    let mut env = serde_json::Map::new();
    for kv in &a.env {
        // `KEY=VALUE`, or `KEY`: this shell's value (a secret is passed
        // without appearing on the command line or in any output).
        let (k, v) = match kv.split_once('=') {
            Some((k, v)) if !k.is_empty() => (k.to_string(), v.to_string()),
            Some(_) => {
                return Err(CuaError::InvalidArgument(format!(
                    "--env {kv}: use KEY=VALUE or KEY"
                )));
            }
            None => {
                let v = std::env::var(kv).map_err(|_| {
                    CuaError::InvalidArgument(format!("--env {kv}: {kv} is not set in this shell"))
                })?;
                (kv.clone(), v)
            }
        };
        env.insert(k, json!(v));
    }
    env.insert("PYTHONUNBUFFERED".into(), json!("1"));
    Ok(json!({
        "run": run,
        "gang": true,
        "nodes": {"min": 1, "max": 1},
        "kind": a.kind.clone().unwrap_or_default(),
        "image": a.image.clone().unwrap_or_default(),
        "command": command,
        "env": env,
        "bundle": base64::engine::general_purpose::STANDARD.encode(bundle),
        "accelerator": a.accelerator.clone().filter(|x| x != "none").unwrap_or_default(),
        "cpus": a.cpus,
        "memory_mb": a.memory,
        "timeout_secs": 7200,
        "retries": a.retries.unwrap_or(0),
        "infra_retries": 10,
        "on": on,
        "services": services,
    }))
}

/// Where a command run goes when `--on` is left out: the core default
/// placement ([`cua_sandbox_core::placement::On::Local`], this machine,
/// until the elastic placement overflows into a linked cloud); `--on auto`
/// spreads it over your pool. This is the one place a command run
/// resolves an omitted `--on`.
pub fn command_on(on: Option<&str>) -> String {
    on.map(str::trim)
        .filter(|o| !o.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| cua_sandbox_core::placement::On::Local.to_string())
}

/// `cua run [--compat P] -- CMD ARGS...`: the command in a Space on your
/// pool, with the current directory, followed like any run.
async fn run_command(a: RunArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let services = match &a.compat {
        Some(p) => vec![compat_service(p)?],
        None => vec![],
    };
    let files = project_files(Path::new("."))?;
    let bundle = project_bundle(Path::new("."), &files)?;
    let on = placement(&command_on(a.on.as_deref()))?;
    let (command, added) = match &a.compat {
        Some(p) => compat_command(&a.command, &files, Path::new("."), p),
        None => (member_command(&a.command, &files), None),
    };
    if let (Some(n), false) = (&added, json) {
        line(out, n.clone());
    }
    let name = Path::new(&a.command[0])
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "cmd".into());
    let run_id = match &a.id {
        Some(id) => {
            check_id(id, &on)?;
            id.clone()
        }
        None => new_run_id(run_prefix(&on), &name),
    };
    let params = command_params(&run_id, &command, &a, &on, &bundle, &services)?;
    submit_acknowledged(&params).await?;
    let what = format!(
        "{run_id}: `{}`{} ({} files, {} KB)",
        a.command.join(" "),
        a.compat
            .as_ref()
            .map(|p| format!(" with the {p} API on Cua Spaces"))
            .unwrap_or_default(),
        files.len(),
        bundle.len() / 1024
    );
    if a.detach {
        if json {
            line(out, json!({"run": run_id}).to_string());
        } else {
            line(out, format!("{what}: submitted"));
            hint_line(out, format!("Follow it: cua jobs logs -f {run_id}"));
        }
        return Ok(0);
    }
    if !json {
        for l in follow_hint(&run_id) {
            line(out, l);
        }
        line(out, what);
    }
    let started = Instant::now();
    let summary = follow(&run_id, None, json, out).await?;
    finish(&run_id, &summary, started.elapsed(), None, json, out).await
}

/// What `cua run` prints before it follows a run: short lines, so a
/// narrow terminal does not wrap them mid-word.
fn follow_hint(run_id: &str) -> [String; 2] {
    [
        "Ctrl-C stops following; the run keeps going.".to_string(),
        hint(&format!(
            "(cancel: cua jobs cancel {run_id}; follow again: cua jobs logs -f {run_id})"
        )),
    ]
}

/// What a finished run prints and saves.
async fn finish(
    run_id: &str,
    summary: &Value,
    wall: Duration,
    to: Option<PathBuf>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let state = summary["state"].as_str().unwrap_or("");
    let attempts = summary["gang"]["attempt"].as_u64().unwrap_or(1);
    let cost = summary["cost_usd"].as_f64().unwrap_or(0.0);
    let machine = summary["machine_seconds"].as_f64().unwrap_or(0.0);
    let mut saved = Vec::new();
    if state == "succeeded" {
        saved = save_result(run_id, to).await?;
    }
    if json {
        line(
            out,
            json!({"run": run_id, "state": state, "attempts": attempts, "wall_secs": wall.as_secs(),
                   "machine_seconds": machine, "cost_usd": cost,
                   "result": saved.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()})
            .to_string(),
        );
    } else {
        for p in &saved {
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            line(out, format!("result: {} ({} KB)", p.display(), size / 1024));
        }
        line(
            out,
            format!(
                "{run_id} {state} in {} · {attempts} attempt{} · computer time {} · cost ${cost:.2}",
                hms(wall.as_secs_f64()),
                if attempts == 1 { "" } else { "s" },
                hms(machine),
            ),
        );
        if state != "succeeded" {
            let err = summary["gang"]["error"]
                .as_str()
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .or_else(|| failure_reason(summary, attempts))
                .unwrap_or_default();
            if !err.is_empty() {
                line(out, format!("why: {err}"));
            }
        }
    }
    Ok(if state == "succeeded" { 0 } else { 1 })
}

/// `1h02m03s`, `4m05s`, `12s`.
pub fn hms(secs: f64) -> String {
    let s = secs.round() as u64;
    match (s / 3600, (s / 60) % 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m{s:02}s"),
        (h, m, s) => format!("{h}h{m:02}m{s:02}s"),
    }
}

/// The error that ended attempt `attempt` (not the members stopped with
/// it).
pub fn failure_reason(run: &Value, attempt: u64) -> Option<String> {
    let members = run["members"].as_array()?;
    let of: Vec<&Value> = members
        .iter()
        .filter(|m| m["attempt"].as_u64() == Some(attempt))
        .collect();
    // A member lost with its host is the cause: its peers' collectives (an
    // MLX ring, NCCL) break when it goes, so their errors are a consequence
    // (CUA-1822).
    if let Some(m) = of
        .iter()
        .find(|m| m["error"].as_str().unwrap_or("").ends_with(" went down"))
    {
        let e = m["error"].as_str().unwrap_or("");
        return Some(format!(
            "{} (rank {})",
            e.strip_prefix("canceled; ").unwrap_or(e),
            m["rank"]
        ));
    }
    // The member that failed first by its host's clock, not the lowest
    // rank: a peer whose ring broke a second later is a consequence; at the
    // same time, its own failure before a broken connection (CUA-1826).
    let first = |m: &&&Value| {
        let at = m["ended_ms"]
            .as_i64()
            .filter(|t| *t > 0)
            .or_else(|| m["finished_ms"].as_i64().filter(|t| *t > 0))
            .unwrap_or(i64::MAX);
        (
            at,
            m["peer_lost"].as_bool().unwrap_or(false),
            m["rank"].as_u64().unwrap_or(0),
        )
    };
    of.iter()
        .filter(|m| {
            let e = m["error"].as_str().unwrap_or("");
            !e.is_empty() && !e.starts_with("stopped with its gang")
        })
        .min_by_key(first)
        .or_else(|| {
            of.iter()
                .find(|m| !m["error"].as_str().unwrap_or("").is_empty())
        })
        .map(|m| {
            format!(
                "rank {} on {} {}: {}",
                m["rank"],
                m["host"].as_str().unwrap_or("?"),
                m["state"].as_str().unwrap_or(""),
                m["error"].as_str().unwrap_or("")
            )
        })
}

/// Unpacks rank 0's result (a gzip tar of a file or a directory) into `to`
/// (default `./cua-runs/<run>`); returns the files written.
pub async fn save_result(run_id: &str, to: Option<PathBuf>) -> Result<Vec<PathBuf>, CuaError> {
    let r = request(target_of_run(run_id), "run_result", json!({"run": run_id})).await?;
    if r["done"].as_bool() != Some(true) {
        return Err(CuaError::InvalidArgument(format!(
            "{run_id} has not ended yet ({})",
            r["state"].as_str().unwrap_or("")
        )));
    }
    let Some(b64) = r["result_b64"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(vec![]);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| CuaError::Internal(format!("the result: {e}")))?;
    let dir = to.unwrap_or_else(|| PathBuf::from("cua-runs").join(run_id));
    unpack_result(&bytes, &dir)
}

/// [`save_result`]'s unpacking: only regular files with plain relative
/// paths (no `..`, no absolute paths, nothing hidden) are written.
pub fn unpack_result(bytes: &[u8], dir: &Path) -> Result<Vec<PathBuf>, CuaError> {
    let io = |e: std::io::Error| CuaError::Internal(format!("the result: {e}"));
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut written = Vec::new();
    std::fs::create_dir_all(dir).map_err(io)?;
    for entry in tar.entries().map_err(io)? {
        let mut e = entry.map_err(io)?;
        if !e.header().entry_type().is_file() {
            continue;
        }
        let rel = e.path().map_err(io)?.into_owned();
        let plain = rel.components().all(|c| match c {
            std::path::Component::Normal(p) => {
                let p = p.to_string_lossy();
                !p.starts_with('.') && !p.contains('\\')
            }
            _ => false,
        });
        if !plain || rel.as_os_str().is_empty() {
            continue;
        }
        let mut data = Vec::new();
        e.read_to_end(&mut data).map_err(io)?;
        let p = dir.join(&rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&p, data).map_err(io)?;
        written.push(p);
    }
    Ok(written)
}

/// Per-rank output state while following.
#[derive(Default)]
struct Stream {
    next: u64,
    partial: Vec<u8>,
    done: bool,
    /// Why the last read failed (the member is not on a host yet, or its
    /// host does not answer).
    error: Option<String>,
    /// Some output came through.
    printed: bool,
}

/// Prints the complete lines of `data` (with `prefix`), keeping a partial
/// last line for later.
fn emit(s: &mut Stream, data: &[u8], prefix: &str, flush: bool, out: &mut dyn Write) {
    s.partial.extend_from_slice(data);
    while let Some(i) = s.partial.iter().position(|b| *b == b'\n') {
        let l: Vec<u8> = s.partial.drain(..=i).collect();
        let text = String::from_utf8_lossy(&l[..l.len() - 1]);
        line(out, format!("{prefix}{}", text.trim_end_matches('\r')));
    }
    if flush && !s.partial.is_empty() {
        let text = String::from_utf8_lossy(&s.partial).to_string();
        s.partial.clear();
        line(out, format!("{prefix}{text}"));
    }
}

/// Reads what is new of one member's output.
async fn pull(
    run_id: &str,
    m: &Value,
    attempt: u64,
    streams: &mut HashMap<(u64, u64), Stream>,
    out: &mut dyn Write,
) {
    let rank = m["rank"].as_u64().unwrap_or(0);
    let s = streams.entry((attempt, rank)).or_default();
    if s.done {
        return;
    }
    let prefix = format!("[rank {rank} {}] ", m["host"].as_str().unwrap_or("?"));
    for _ in 0..64 {
        let r = match request(
            target_of_run(run_id),
            "job_output",
            json!({"run": run_id, "rank": rank, "attempt": attempt, "from": s.next}),
        )
        .await
        {
            Ok(r) => {
                if std::env::var_os("CUA_JOBS_TRACE").is_some() {
                    eprintln!("job_output rank {rank} attempt {attempt}: {r}");
                }
                s.error = None;
                r
            }
            Err(e) => {
                s.error = Some(e.to_string());
                return;
            }
        };
        let data = r["data_b64"]
            .as_str()
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .unwrap_or_default();
        let done = r["done"].as_bool() == Some(true);
        emit(s, &data, &prefix, done, out);
        s.printed |= !data.is_empty();
        s.next = r["next"].as_u64().unwrap_or(s.next);
        if done {
            s.done = true;
            // Nothing in its output log (a host that keeps no log): the
            // tail the host reported when the member ended.
            if !s.printed
                && let Some(id) = m["job"].as_i64()
                && let Ok(j) = request(target_of_run(run_id), "job", json!({"id": id})).await
            {
                let tail = format!(
                    "{}{}",
                    j["stdout"].as_str().unwrap_or(""),
                    j["stderr"].as_str().unwrap_or("")
                );
                emit(s, tail.as_bytes(), &prefix, true, out);
            }
            return;
        }
        if data.is_empty() || s.next >= r["size"].as_u64().unwrap_or(0) {
            return;
        }
    }
}

/// Follows `run_id` until it ends: every rank's output (or `rank`'s), and
/// a line whenever the gang is placed again. Returns the final `run`.
pub async fn follow(
    run_id: &str,
    rank: Option<u32>,
    json: bool,
    out: &mut dyn Write,
) -> Result<Value, CuaError> {
    let mut streams: HashMap<(u64, u64), Stream> = HashMap::new();
    let mut shown_attempt = 0u64;
    let mut placed_attempt = 0u64;
    let mut waiting_said = String::new();
    loop {
        let r = match request(target_of_run(run_id), "run", json!({"run": run_id})).await {
            Ok(r) => r,
            Err(CuaError::Timeout(_)) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        let attempt = r["gang"]["attempt"].as_u64().unwrap_or(1).max(1);
        let members: Vec<Value> = r["members"].as_array().cloned().unwrap_or_default();
        let of = |a: u64| -> Vec<Value> {
            members
                .iter()
                .filter(|m| m["attempt"].as_u64().unwrap_or(1) == a)
                .filter(|m| rank.is_none_or(|k| m["rank"].as_u64() == Some(u64::from(k))))
                .cloned()
                .collect()
        };
        if attempt != shown_attempt {
            // What the last attempt still had to say, then the change.
            if shown_attempt > 0 && !json {
                for m in of(shown_attempt) {
                    pull(run_id, &m, shown_attempt, &mut streams, out).await;
                }
            }
            shown_attempt = attempt;
        }
        let now: Vec<Value> = of(attempt);
        let hosts: Vec<String> = now
            .iter()
            .filter_map(|m| {
                m["host"]
                    .as_str()
                    .filter(|h| !h.is_empty())
                    .map(str::to_string)
            })
            .collect();
        if placed_attempt != attempt && !hosts.is_empty() && !json {
            let size = r["gang"]["size"].as_u64().unwrap_or(hosts.len() as u64);
            if attempt == 1 {
                line(
                    out,
                    format!("attempt 1: placed at size {size} on {}", hosts.join(", ")),
                );
            } else {
                let why = failure_reason(&r, attempt - 1).unwrap_or_else(|| "a failure".into());
                line(
                    out,
                    format!(
                        "attempt {attempt}: gang re-placed at size {size} on {} after {why}",
                        hosts.join(", ")
                    ),
                );
            }
            placed_attempt = attempt;
        }
        if hosts.is_empty()
            && let Some(w) = r["gang"]["waiting"].as_str().filter(|w| !w.is_empty())
            && w != waiting_said
            && !json
        {
            line(out, format!("attempt {attempt}: waiting: {w}"));
            waiting_said = w.to_string();
        }
        if !json {
            for m in &now {
                if m["host"].as_str().is_some_and(|h| !h.is_empty()) {
                    pull(run_id, m, attempt, &mut streams, out).await;
                }
            }
        }
        // `done` with a terminal state: a gang between attempts is queued.
        if r["done"].as_bool() == Some(true)
            && matches!(
                r["state"].as_str(),
                Some("succeeded" | "failed" | "canceled" | "error")
            )
        {
            if !json {
                for m in &now {
                    pull(run_id, m, attempt, &mut streams, out).await;
                }
            }
            return Ok(r);
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
}

/// The runs on this machine and on your pool, newest first by id order
/// within each (`cua jobs`). The pool's being out of reach (or its keys
/// not shared with this device yet) does not hide this machine's runs.
async fn all_runs() -> Result<Value, CuaError> {
    let local = request("local", "runs", json!({"limit": 20})).await;
    let pool = request("pool", "runs", json!({"limit": 20})).await;
    let mut runs: Vec<Value> = Vec::new();
    for v in [&local, &pool].into_iter().flatten() {
        runs.extend(v["runs"].as_array().cloned().unwrap_or_default());
    }
    match (local, pool) {
        (_, Err(e)) if runs.is_empty() => Err(e),
        (_, Err(e)) => Ok(json!({"runs": runs, "pool_error": e.to_string()})),
        _ => Ok(json!({"runs": runs})),
    }
}

/// `cua jobs ...`.
pub async fn jobs(a: JobsArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    match a.cmd {
        Some(JobsCmd::Logs {
            run,
            follow: f,
            rank,
        }) => {
            if f {
                let r = follow(&run, rank, json, out).await?;
                if json {
                    line(out, r.to_string());
                }
                return Ok(if r["state"] == "succeeded" { 0 } else { 1 });
            }
            let r = request(target_of_run(&run), "run", json!({"run": run})).await?;
            let attempt = r["gang"]["attempt"].as_u64().unwrap_or(1);
            let mut streams = HashMap::new();
            for m in r["members"].as_array().cloned().unwrap_or_default() {
                if m["attempt"].as_u64().unwrap_or(1) == attempt
                    && rank.is_none_or(|k| m["rank"].as_u64() == Some(u64::from(k)))
                {
                    pull(&run, &m, attempt, &mut streams, out).await;
                    if let Some(s) = streams.get_mut(&(attempt, m["rank"].as_u64().unwrap_or(0))) {
                        let prefix = format!(
                            "[rank {} {}] ",
                            m["rank"],
                            m["host"].as_str().unwrap_or("?")
                        );
                        emit(s, &[], &prefix, true, out);
                        if let Some(e) = &s.error {
                            line(out, format!("{prefix}(output not available: {e})"));
                        }
                    }
                }
            }
            Ok(0)
        }
        Some(JobsCmd::Cancel { run }) => {
            let v = request(target_of_run(&run), "cancel_run", json!({"run": run})).await?;
            if json {
                line(out, v.to_string());
            } else {
                line(out, format!("{run}: canceling"));
            }
            Ok(0)
        }
        Some(JobsCmd::Result { run, output }) => {
            let saved = save_result(&run, output).await?;
            if json {
                line(
                    out,
                    json!({"run": run, "files": saved.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()})
                        .to_string(),
                );
            } else if saved.is_empty() {
                line(out, format!("{run} returned no result"));
            } else {
                for p in saved {
                    line(out, format!("{}", p.display()));
                }
            }
            Ok(0)
        }
        None => match a.run {
            Some(run) => {
                let r = request(target_of_run(&run), "run", json!({"run": run})).await?;
                if json {
                    line(out, r.to_string());
                } else {
                    for l in run_table(&r) {
                        line(out, l);
                    }
                }
                Ok(0)
            }
            None => {
                let v = all_runs().await?;
                if json {
                    line(out, v.to_string());
                } else {
                    for l in runs_table(&v) {
                        line(out, l);
                    }
                    if let Some(e) = v["pool_error"].as_str() {
                        line(out, format!("(runs on your pool are not shown: {e})"));
                    }
                }
                Ok(0)
            }
        },
    }
}

/// `cua jobs`: one line per run.
pub fn runs_table(v: &Value) -> Vec<String> {
    let mut rows = vec![format!(
        "{:<26}  {:<10}  {:>5}  {:>7}  {:<24}  {:>10}  {:>8}",
        "RUN", "STATE", "SIZE", "ATTEMPT", "HOSTS", "MACHINE", "COST"
    )];
    let runs = v["runs"].as_array().cloned().unwrap_or_default();
    for r in &runs {
        let g = &r["gang"];
        rows.push(format!(
            "{:<26}  {:<10}  {:>5}  {:>7}  {:<24}  {:>10}  {:>8}",
            r["run"].as_str().unwrap_or(""),
            r["state"].as_str().unwrap_or(""),
            g["size"]
                .as_u64()
                .map(|s| s.to_string())
                .unwrap_or_else(|| r["total"].to_string()),
            g["attempt"].as_u64().unwrap_or(1),
            g["hosts"]
                .as_array()
                .map(|h| h
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","))
                .unwrap_or_default(),
            hms(r["machine_seconds"].as_f64().unwrap_or(0.0)),
            format!("${:.2}", r["cost_usd"].as_f64().unwrap_or(0.0)),
        ));
    }
    if runs.is_empty() {
        rows.push(hint("No runs yet: cua run train.py::finetune"));
    }
    rows
}

/// `cua jobs RUN`: the run and its members by attempt.
pub fn run_table(r: &Value) -> Vec<String> {
    let g = &r["gang"];
    let mut rows = vec![format!(
        "{}: {} · attempt {} · size {} ({}:{}) · computer time {} · cost ${:.2}",
        r["run"].as_str().unwrap_or(""),
        r["state"].as_str().unwrap_or(""),
        g["attempt"].as_u64().unwrap_or(1),
        g["size"].as_u64().unwrap_or(0),
        g["min"].as_u64().unwrap_or(1),
        g["max"].as_u64().unwrap_or(1),
        hms(r["machine_seconds"].as_f64().unwrap_or(0.0)),
        r["cost_usd"].as_f64().unwrap_or(0.0)
    )];
    if let Some(w) = g["waiting"].as_str().filter(|w| !w.is_empty()) {
        rows.push(format!("waiting: {w}"));
    }
    rows.push(format!(
        "{:>7}  {:>4}  {:<16}  {:<10}  {:>8}  {:>7}  ERROR",
        "ATTEMPT", "RANK", "HOST", "STATE", "TIME", "COST"
    ));
    for m in r["members"].as_array().cloned().unwrap_or_default() {
        rows.push(format!(
            "{:>7}  {:>4}  {:<16}  {:<10}  {:>8}  {:>7}  {}",
            m["attempt"].as_u64().unwrap_or(1),
            m["rank"].as_u64().unwrap_or(0),
            m["host"].as_str().unwrap_or("-"),
            m["state"].as_str().unwrap_or(""),
            hms(m["seconds"].as_f64().unwrap_or(0.0)),
            format!("${:.3}", m["cost_usd"].as_f64().unwrap_or(0.0)),
            m["error"].as_str().unwrap_or("")
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_follow_hint_fits_a_narrow_terminal() {
        let h = follow_hint("p-finetune-55704e0f");
        assert_eq!(h[0], "Ctrl-C stops following; the run keeps going.");
        assert!(h[1].contains("cua jobs cancel p-finetune-55704e0f"));
        assert!(h[1].contains("cua jobs logs -f p-finetune-55704e0f"));
        assert!(h.iter().all(|l| l.chars().count() <= 100), "{h:?}");
    }

    use super::*;

    /// A run on this machine (`--on local`, the default for a command) is
    /// an `l-` run: its own coordinator answers it, so it never waits for
    /// the pool or its keys; any other placement is a `p-` run on the pool.
    #[test]
    fn a_local_run_is_an_l_run_and_the_pool_a_p_run() {
        assert_eq!(run_prefix(&placement(&command_on(None)).unwrap()), "l-");
        assert_eq!(run_prefix(&placement("host:local").unwrap()), "l-");
        assert_eq!(run_prefix(&placement("auto").unwrap()), "p-");
        assert_eq!(run_prefix(&placement("hetzner").unwrap()), "p-");
        assert_eq!(run_prefix(&placement("host:studio").unwrap()), "p-");
        assert!(new_run_id("l-", "app.py").starts_with("l-apppy-"));
        assert_eq!(target_of_run(&new_run_id("l-", "x")), "local");
        assert_eq!(target_of_run(&new_run_id("p-", "x")), "pool");
        assert!(check_id("l-mine", "local").is_ok());
        assert!(check_id("p-mine", "").is_ok());
        let e = check_id("p-mine", "local").unwrap_err().to_string();
        assert!(e.contains("start with l-"), "{e}");
        let e = check_id("l-mine", "cloud:hetzner").unwrap_err().to_string();
        assert!(e.contains("start with p-"), "{e}");
    }

    #[test]
    fn targets_nodes_and_placement() {
        let (f, n) = split_target("examples/train.py::finetune").unwrap();
        assert_eq!(
            (f.to_str().unwrap(), n.as_str()),
            ("examples/train.py", "finetune")
        );
        assert!(split_target("train.py").is_err());
        assert!(split_target("train.py::").is_err());
        assert!(split_target("train.py::a-b").is_err());
        assert_eq!(parse_nodes("2").unwrap(), (2, 2));
        assert_eq!(parse_nodes("1:2").unwrap(), (1, 2));
        assert!(
            parse_nodes("0").is_err() && parse_nodes("3:2").is_err() && parse_nodes("x").is_err()
        );
        assert_eq!(placement("auto").unwrap(), "");
        assert_eq!(placement("host:aws-1").unwrap(), "aws-1");
        assert_eq!(placement("aws").unwrap(), "cloud:aws");
        assert!(placement("cloud").is_err() && placement("modal").is_err());
        assert_eq!(hms(5.0), "5s");
        assert_eq!(hms(125.0), "2m05s");
        assert_eq!(hms(3725.0), "1h02m05s");
    }

    #[test]
    fn cuaignore_patterns() {
        let p: Vec<String> = ["*.jsonl", "data/", "secret/key.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(ignored("big.jsonl", &p));
        assert!(ignored("sub/big.jsonl", &p));
        assert!(ignored("data/x.py", &p));
        assert!(ignored("secret/key.txt", &p));
        assert!(!ignored("secret/other.txt", &p));
        assert!(!ignored("train.py", &p));
    }

    #[test]
    fn the_bundle_holds_the_job_files_and_the_shim() {
        let d = tempfile::tempdir().unwrap();
        let w = |p: &str, s: &str| {
            let p = d.path().join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, s).unwrap();
        };
        w("train.py", "import cua\n");
        w("data.jsonl", "{}\n");
        w("requirements.txt", "mlx\n");
        w("lib/util.py", "x = 1\n");
        w("weights.bin", "binary");
        w(".env", "SECRET=1");
        w("__pycache__/train.cpython-312.pyc", "x");
        w("cua-runs/p-1/adapters.safetensors", "x");
        w("skip/me.py", "x");
        w(".cuaignore", "skip/\n");
        let files = bundle_files(d.path()).unwrap();
        assert_eq!(
            files,
            vec!["data.jsonl", "lib/util.py", "requirements.txt", "train.py"]
        );
        let b = bundle(d.path(), &files).unwrap();
        let mut names: Vec<String> = tar::Archive::new(flate2::read::GzDecoder::new(&b[..]))
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "_cua_runtime/_cua_entry.py",
                "_cua_runtime/cua/__init__.py",
                "_cua_runtime/cua/_store.py",
                "_cua_runtime/cua/checkpoint.py",
                "_cua_runtime/cua/progress.py",
                "data.jsonl",
                "lib/util.py",
                "requirements.txt",
                "train.py"
            ]
        );
        // Too big: refused with the largest files named.
        // Random letters: about 4.7 bits a byte, so ~1.2 MB compressed.
        let big: String = (0..2_000_000u32)
            .map(|_| char::from(b'a' + rand::random::<u8>() % 26))
            .collect();
        w("huge.txt", &big);
        let files = bundle_files(d.path()).unwrap();
        let e = bundle(d.path(), &files).unwrap_err().to_string();
        assert!(e.contains("huge.txt") && e.contains("1024 KB"), "{e}");
    }

    #[test]
    fn submit_is_a_gang_in_spaces_that_launches_through_the_shim() {
        let spec = json!({
            "kind": "vm",
            "image": {"python": "3.12", "packages": ["mlx==0.32.4", "mlx-lm==0.32.1"],
                      "cuda_rewrites": {"mlx": "mlx[cuda]"}},
            "env": {"TRAIN_STEPS": "200"}, "timeout_secs": 3600,
        });
        let s = Submit {
            run: "p-finetune-1".into(),
            file: "train.py".into(),
            func: "finetune".into(),
            spec,
            nodes: (1, 2),
            on: String::new(),
            accelerator: "gpu".into(),
            retries: 3,
            retry: json!({"max_retries": 3, "backoff_coefficient": 2.0, "initial_delay": 2.0, "max_delay": 60.0}),
            cpus: 2.0,
            memory_mb: 4096,
            bundle: b"tgz".to_vec(),
            secrets: BTreeMap::from([("HF_TOKEN".to_string(), "hf_x".to_string())]),
        };
        let p = s.params();
        assert_eq!(p["gang"], true);
        assert_eq!(p["kind"], "vm");
        let mut s = s;
        s.spec["kind"] = Value::Null;
        assert_eq!(s.params()["kind"], "");
        assert_eq!(p["nodes"], json!({"min": 1, "max": 2}));
        assert_eq!(
            p["command"],
            json!([
                "uv",
                "run",
                "--no-project",
                "--python",
                "3.12",
                "python",
                "-m",
                "_cua_entry",
                "--launch",
                "train.py",
                "finetune"
            ])
        );
        assert_eq!(p["env"]["PYTHONPATH"], "_cua_runtime");
        assert_eq!(p["env"]["TRAIN_STEPS"], "200");
        assert_eq!(p["env"]["HF_TOKEN"], "hf_x");
        assert_eq!(
            serde_json::from_str::<Value>(p["env"]["CUA_JOB_PACKAGES"].as_str().unwrap()).unwrap(),
            json!(["mlx==0.32.4", "mlx-lm==0.32.1"])
        );
        assert_eq!(p["result_path"], RESULT_PATH);
        assert_eq!(p["max_output_bytes"], 16 << 20);
        assert_eq!(p["timeout_secs"], 3600);
        assert_eq!(p["bundle"], "dGd6");
        assert_eq!(
            p["retry"],
            json!({"max_retries": 3, "backoff_coefficient": 2.0, "initial_delay": 2.0, "max_delay": 60.0})
        );
    }

    /// A run id is saved before its submit goes out and found again by the
    /// same submission (a lost acknowledgement), never by another one.
    #[test]
    fn compat_runs_add_the_sdk_only_when_the_project_lacks_it() {
        let cmd = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let dir = |files: &[(&str, &str)]| {
            let d = tempfile::tempdir().unwrap();
            for (f, c) in files {
                std::fs::write(d.path().join(f), c).unwrap();
            }
            let names = files.iter().map(|(f, _)| f.to_string()).collect::<Vec<_>>();
            (d, names)
        };
        let run = |files: &[(&str, &str)], c: &[&str]| {
            let (d, names) = dir(files);
            compat_command(&cmd(c), &names, d.path(), "e2b")
        };
        // (a) no dependency file
        let (v, n) = run(&[("app.py", "")], &["python", "app.py"]);
        assert_eq!(
            v,
            cmd(&[
                "uv",
                "run",
                "--with",
                "e2b==2.58.0",
                "--",
                "python",
                "app.py"
            ])
        );
        assert_eq!(
            n.as_deref(),
            Some("adding e2b==2.58.0 (not in your dependencies)")
        );
        // (b) requirements.txt without the SDK
        let (v, n) = run(&[("requirements.txt", "requests\n")], &["python", "app.py"]);
        assert_eq!(
            v,
            cmd(&[
                "uv",
                "run",
                "--with-requirements",
                "requirements.txt",
                "--with",
                "e2b==2.58.0",
                "--",
                "python",
                "app.py"
            ])
        );
        assert!(n.is_some());
        // (c) pyproject listing the SDK: nothing added, no notice
        let (v, n) = run(
            &[("pyproject.toml", "[project]\ndependencies = [\"e2b>=2\"]\n")],
            &["python", "app.py"],
        );
        assert_eq!(v, cmd(&["uv", "run", "--", "python", "app.py"]));
        assert_eq!(n, None);
        // (d) uv.lock: frozen; a package that only contains the name is not it
        let (v, n) = run(
            &[
                (
                    "pyproject.toml",
                    "[project]\ndependencies = [\"e2b-code-interpreter\"]\n",
                ),
                ("uv.lock", ""),
            ],
            &["python", "app.py"],
        );
        assert_eq!(v[..3], cmd(&["uv", "run", "--frozen"])[..]);
        assert!(v.contains(&"e2b==2.58.0".to_string()) && n.is_some());
        // a requirements.txt that lists it
        let (_, n) = run(
            &[("requirements.txt", "E2B==2.42.0\n")],
            &["python3", "x.py"],
        );
        assert_eq!(n, None);
        // node, and a command that is the user's own
        let (v, n) = run(&[("package.json", "{}")], &["node", "app.js"]);
        assert!(v[2].contains("npm install") && v[2].contains("e2b@2.58.0") && n.is_some());
        let (v, n) = run(
            &[("package.json", "{\"dependencies\":{\"e2b\":\"^2\"}}")],
            &["node", "app.js"],
        );
        assert!(!v[2].contains("e2b@") && n.is_none());
        let (v, n) = run(&[], &["./run.sh"]);
        assert_eq!(v, cmd(&["./run.sh"]));
        assert_eq!(n, None);
    }

    #[test]
    fn the_added_sdk_versions_are_the_newest_claimed() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../tests/compat/SDK_VERSIONS.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return; // a packaged crate has no repo tests dir
        };
        let j: Value = serde_json::from_str(&text).unwrap();
        let newest = |v: &Value| {
            let mut c: Vec<String> = v["claimed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect();
            c.sort_by_key(|x| {
                x.split('.')
                    .map(|n| n.parse::<u64>().unwrap_or(0))
                    .collect::<Vec<_>>()
            });
            c.pop().unwrap()
        };
        for (p, name) in [("e2b", "e2b"), ("daytona", "daytona"), ("modal", "modal")] {
            let sdk = compat_sdk(p).unwrap();
            assert_eq!(sdk.py, name);
            assert_eq!(
                sdk.py_version,
                newest(&j[p]["py"]),
                "{p}: update compat_sdk with SDK_VERSIONS.json"
            );
            if let Some((_, v)) = sdk.npm {
                assert_eq!(v, newest(&j[p]["ts"]), "{p} npm");
            }
        }
    }

    #[test]
    fn command_runs_ask_for_their_compat_service() {
        use clap::Parser;
        #[derive(Parser)]
        struct T {
            #[command(flatten)]
            a: RunArgs,
        }
        let t = T::try_parse_from([
            "x", "--compat", "e2b", "--on", "auto", "--", "python", "smoke.py",
        ])
        .unwrap();
        assert_eq!(t.a.command, ["python", "smoke.py"]);
        assert!(t.a.target.is_none());
        let svc = compat_service(t.a.compat.as_deref().unwrap()).unwrap();
        let p = command_params("p-smoke-1", &t.a.command, &t.a, "auto", b"b", &[svc]).unwrap();
        assert_eq!(p["command"], json!(["python", "smoke.py"]));
        assert_eq!(p["services"], json!(["compat:e2b"]));
        assert_eq!(p["nodes"], json!({"min": 1, "max": 1}));
        assert_eq!(compat_service("Modal").unwrap(), "compat:modal");
        assert!(compat_service("nope").is_err());
        // A command with no --on runs on this machine; `auto` spreads it.
        assert_eq!(placement(&command_on(None)).unwrap(), "local");
        assert_eq!(placement(&command_on(Some("host:local"))).unwrap(), "local");
        assert_eq!(placement(&command_on(Some("auto"))).unwrap(), "");
        assert_eq!(placement(&command_on(Some("host:mini"))).unwrap(), "mini");
        // --compat needs a command.
        assert!(T::try_parse_from(["x", "--compat", "e2b", "f.py::g"]).is_err());
        // A target or a command.
        assert!(T::try_parse_from(["x"]).is_err());
    }

    #[test]
    fn a_bare_python_runs_in_its_uv_project() {
        let cmd: Vec<String> = ["python", "-m", "m"].map(String::from).to_vec();
        let with = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            member_command(&cmd, &with(&["pyproject.toml", "uv.lock"])),
            ["uv", "run", "--frozen", "--", "python", "-m", "m"]
        );
        assert_eq!(member_command(&cmd, &with(&["x.py"])), cmd);
        let uv: Vec<String> = ["uv", "run", "x"].map(String::from).to_vec();
        assert_eq!(member_command(&uv, &with(&["pyproject.toml"])), uv);
    }

    #[test]
    fn project_files_follow_git_and_keep_modes() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.py"), "x").unwrap();
        std::fs::create_dir(d.path().join(".venv")).unwrap();
        std::fs::write(d.path().join(".venv/big"), "x").unwrap();
        std::fs::write(d.path().join("run.sh"), "#!/bin/sh").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                d.path().join("run.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let files = project_files(d.path()).unwrap();
        assert_eq!(files, ["a.py", "run.sh"]);
        let b = project_bundle(d.path(), &files).unwrap();
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&b[..]));
        let modes: Vec<(String, u32)> = ar
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.path().unwrap().to_string_lossy().to_string(),
                    e.header().mode().unwrap(),
                )
            })
            .collect();
        assert!(modes.contains(&("run.sh".into(), 0o755)), "{modes:?}");
    }

    #[test]
    fn unacknowledged_submits_keep_their_run_id() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("cua-runs/.pending");
        let args = RunArgs {
            target: Some("train.py::finetune".into()),
            command: vec![],
            compat: None,
            kind: None,
            image: None,
            nodes: Some("1:2".into()),
            on: None,
            accelerator: None,
            retries: None,
            cpus: 2.0,
            memory: 4096,
            env: vec![],
            id: None,
            detach: false,
        };
        let spec = json!({"nodes": {"min": 1, "max": 2}});
        let k = submission_key("train.py", "finetune", b"bundle", &spec, &args);
        assert_eq!(
            k,
            submission_key("train.py", "finetune", b"bundle", &spec, &args)
        );
        assert_ne!(
            k,
            submission_key("train.py", "finetune", b"bundle2", &spec, &args)
        );
        let more = RunArgs {
            nodes: Some("2".into()),
            ..args.clone()
        };
        assert_ne!(
            k,
            submission_key("train.py", "finetune", b"bundle", &spec, &more)
        );
        assert_eq!(pending_id(&dir, &k), None);
        save_pending(&dir, "p-finetune-0000beef", &k).unwrap();
        assert_eq!(pending_id(&dir, &k).as_deref(), Some("p-finetune-0000beef"));
        assert_eq!(pending_id(&dir, "other"), None);
        std::fs::remove_file(dir.join("p-finetune-0000beef")).unwrap();
        assert_eq!(pending_id(&dir, &k), None);
    }

    #[test]
    fn results_unpack_plain_relative_paths_only() {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut t = tar::Builder::new(gz);
        for (n, d) in [
            ("adapters.safetensors", &b"weights"[..]),
            ("model/config.json", b"{}"),
            (".hidden", b"h"),
            ("../evil", b"x"),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(d.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            // `append_data` refuses `..`; write the raw name like a hostile tar would.
            if n.contains("..") {
                h.as_gnu_mut().unwrap().name[..n.len()].copy_from_slice(n.as_bytes());
                h.set_cksum();
                t.append(&h, d).unwrap();
            } else {
                t.append_data(&mut h, n, d).unwrap();
            }
        }
        let bytes = t.into_inner().unwrap().finish().unwrap();
        let d = tempfile::tempdir().unwrap();
        let got = unpack_result(&bytes, &d.path().join("p-1")).unwrap();
        assert_eq!(
            got,
            vec![
                d.path().join("p-1/adapters.safetensors"),
                d.path().join("p-1/model/config.json")
            ]
        );
        assert_eq!(std::fs::read(&got[0]).unwrap(), b"weights");
        assert!(!d.path().join("evil").exists());
    }

    #[test]
    fn the_reason_of_a_lost_attempt_names_the_member_that_failed() {
        let r = json!({"members": [
            {"attempt": 1, "rank": 0, "host": "aws-1", "state": "canceled", "error": "stopped with its gang: rank 1 on aws-2 lost"},
            {"attempt": 1, "rank": 1, "host": "aws-2", "state": "lost", "error": "aws-2 stopped answering"},
            {"attempt": 2, "rank": 0, "host": "aws-1", "state": "running", "error": ""},
        ]});
        assert_eq!(
            failure_reason(&r, 1).unwrap(),
            "rank 1 on aws-2 lost: aws-2 stopped answering"
        );
        assert!(failure_reason(&r, 2).is_none());
    }

    #[test]
    fn the_member_that_failed_first_is_named_not_the_lowest_rank() {
        // Run p-finetune-b8f9b48e (CUA-1826): rank 1's GPU timed out, rank
        // 0's ring broke 1.3 s later.
        let r = json!({"members": [
            {"attempt": 1, "rank": 0, "host": "macbook", "state": "failed", "error": "exited with code 1: cua: mlx_lm.lora exited with status 1", "ended_ms": 1792196818162i64, "finished_ms": 1792196821300i64, "peer_lost": true},
            {"attempt": 1, "rank": 1, "host": "studio", "state": "failed", "error": "exited with code 1: cua: mlx_lm.lora exited with status 1", "ended_ms": 1792196816864i64, "finished_ms": 1792196829000i64, "peer_lost": false},
        ]});
        assert!(
            failure_reason(&r, 1)
                .unwrap()
                .starts_with("rank 1 on studio failed")
        );
        // The other way round.
        let r = json!({"members": [
            {"attempt": 1, "rank": 0, "host": "macbook", "state": "failed", "error": "exited with code 1", "ended_ms": 1000, "peer_lost": false},
            {"attempt": 1, "rank": 1, "host": "studio", "state": "failed", "error": "exited with code 1", "ended_ms": 1200, "peer_lost": true},
        ]});
        assert!(
            failure_reason(&r, 1)
                .unwrap()
                .starts_with("rank 0 on macbook")
        );
        // No times (an older pool): the own failure before the broken ring.
        let r = json!({"members": [
            {"attempt": 1, "rank": 0, "host": "macbook", "state": "failed", "error": "exited with code 1", "peer_lost": true},
            {"attempt": 1, "rank": 1, "host": "studio", "state": "failed", "error": "exited with code 1", "peer_lost": false},
        ]});
        assert!(
            failure_reason(&r, 1)
                .unwrap()
                .starts_with("rank 1 on studio")
        );
    }

    #[test]
    fn a_host_that_went_down_is_named_before_the_peer_whose_ring_broke() {
        // As `cua run` sees it after an EC2 instance is terminated mid-run:
        // the survivor exits first, the lost member's cancel lands as its
        // host goes down.
        let r = json!({"members": [
            {"attempt": 1, "rank": 0, "host": "aws-1", "state": "failed", "error": "exited with code 1: cua: mlx_lm.lora exited with status 1"},
            {"attempt": 1, "rank": 1, "host": "aws-2", "state": "canceled", "error": "canceled; aws-2 went down"},
            {"attempt": 2, "rank": 0, "host": "aws-1", "state": "running", "error": ""},
        ]});
        assert_eq!(failure_reason(&r, 1).unwrap(), "aws-2 went down (rank 1)");
        let r = json!({"members": [
            {"attempt": 1, "rank": 0, "host": "macbook", "state": "failed", "error": "exited with code 1"},
            {"attempt": 1, "rank": 1, "host": "studio", "state": "error", "error": "studio went down"},
        ]});
        assert_eq!(failure_reason(&r, 1).unwrap(), "studio went down (rank 1)");
    }

    #[test]
    fn output_lines_get_the_rank_prefix() {
        let mut s = Stream::default();
        let mut out = Vec::new();
        emit(&mut s, b"step 1\nstep", "[rank 0 aws-1] ", false, &mut out);
        emit(
            &mut s,
            b" 2\nresumed from step 40\n",
            "[rank 0 aws-1] ",
            false,
            &mut out,
        );
        emit(&mut s, b"tail", "[rank 0 aws-1] ", true, &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "[rank 0 aws-1] step 1\n[rank 0 aws-1] step 2\n[rank 0 aws-1] resumed from step 40\n[rank 0 aws-1] tail\n"
        );
    }

    #[test]
    fn the_embedded_shim_reads_a_spec() {
        // The real interpreter path the CLI uses (skipped without python3).
        if std::process::Command::new("python3")
            .arg("-V")
            .output()
            .is_err()
        {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("job.py");
        std::fs::write(
            &f,
            "import cua\napp = cua.App('t')\n@app.function(image=cua.Image.python('3.11').uv_pip_install('mlx'), nodes=(1, 2), accelerator='gpu')\ndef go(cluster):\n    return 1\n",
        )
        .unwrap();
        let s = read_spec(&f, "go").unwrap();
        assert_eq!(s["nodes"], json!({"min": 1, "max": 2}));
        assert_eq!(s["image"]["python"], "3.11");
        assert_eq!(s["accelerator"], "gpu");
        assert!(read_spec(&f, "missing").is_err());
    }
}
