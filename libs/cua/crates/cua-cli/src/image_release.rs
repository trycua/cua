//! `cua images release`: one pipeline, local and CI alike, for a sandbox
//! image: build, doctor, content digest, save, push, publish (immutable
//! pins), verify and promote.
//!
//! An image directory describes its pipeline in `release.json`: every step is
//! a command template (the same scripts and `cua images` calls the image
//! workflows run), so a CI job is `cua images release <dir> --steps ...` and
//! the same command reproduces it on a laptop.
//!
//! ```text
//! <work>/                 --work, $CUA_RELEASE_WORK, else <cua home>/build/release-<name>[-<tier>]
//!   out/                  build outputs (disks)
//!   artifacts/<arch>/     the doctored rootfs and disk (what push publishes)
//!   evidence/             plain directory, uploaded as-is by CI
//!     state-<scope>.json  step results (merged across jobs for --resume and gates)
//!     logs/<step>.log     one log per step
//!     summary.json|md     the run's table
//! ```
//!
//! Phases run in order: prepare, build, gate (doctor lanes), stage, and
//! with `--publish` push, publish and verify; `--promote` adds promote. Push
//! refuses unless every required gate of every arch in `release.json`
//! passed (in this run or a merged state file).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use cua_sdk::CuaError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn bad(msg: impl Into<String>) -> CuaError {
    CuaError::InvalidArgument(msg.into())
}

fn failed(msg: impl Into<String>) -> CuaError {
    CuaError::Internal(msg.into())
}

/// Pipeline phases, in run order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Prepare,
    Build,
    Gate,
    Stage,
    Push,
    Publish,
    Verify,
    Promote,
}

impl Phase {
    pub const ALL: [Phase; 8] = [
        Phase::Prepare,
        Phase::Build,
        Phase::Gate,
        Phase::Stage,
        Phase::Push,
        Phase::Publish,
        Phase::Verify,
        Phase::Promote,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Phase::Prepare => "prepare",
            Phase::Build => "build",
            Phase::Gate => "gate",
            Phase::Stage => "stage",
            Phase::Push => "push",
            Phase::Publish => "publish",
            Phase::Verify => "verify",
            Phase::Promote => "promote",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == s)
    }
}

/// A tier of the image (`slim`, `full`): its series and per-arch build tag.
#[derive(Debug, Clone, Deserialize)]
pub struct TierDef {
    pub series: String,
    pub tag: String,
}

/// Where a gate is required: `true` (every arch), `false`, or a list of arches.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Required {
    All(bool),
    Arches(Vec<String>),
}

impl Default for Required {
    fn default() -> Self {
        Required::All(true)
    }
}

impl Required {
    fn on(&self, arch: Option<&str>) -> bool {
        match self {
            Required::All(b) => *b,
            Required::Arches(a) => arch.is_some_and(|x| a.iter().any(|y| y == x)),
        }
    }
}

/// One step template of `release.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct StepDef {
    /// Step id (templated, e.g. `doctor/{arch}/runc`).
    pub id: String,
    pub phase: Phase,
    /// Run once per arch (`{arch}` is set).
    #[serde(default)]
    pub per_arch: bool,
    /// Only these arches (per-arch steps).
    #[serde(default)]
    pub arches: Option<Vec<String>>,
    /// Host capabilities the step needs (`kvm`, `runsc`, `docker`, `linux`,
    /// `macos`, `windows`; `!cap` for its absence). Unmet: the step is skipped.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Gates and verify steps: where the step must pass before anything is
    /// pushed (gates) or promoted (verify steps).
    #[serde(default)]
    pub required: Required,
    /// The command (templated argv).
    pub run: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Also write the command's stdout to this file.
    #[serde(default)]
    pub stdout: Option<String>,
    /// Files the step writes; a resumed step whose outputs are gone reruns.
    #[serde(default)]
    pub produces: Vec<String>,
    /// Treat the step as done when this file exists (e.g. a prebuilt binary).
    #[serde(default)]
    pub unless_exists: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `release.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseFile {
    pub schema_version: u32,
    /// Image name (default: the directory name).
    #[serde(default)]
    pub name: Option<String>,
    pub repo: String,
    pub arches: Vec<String>,
    #[serde(default)]
    pub tiers: BTreeMap<String, TierDef>,
    /// Series and tag when the image has no tiers.
    #[serde(default)]
    pub series: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    /// Default variables (`--var` overrides).
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    pub steps: Vec<StepDef>,
}

impl ReleaseFile {
    pub fn load(dir: &Path) -> Result<Self, CuaError> {
        let path = dir.join("release.json");
        let text = std::fs::read_to_string(&path).map_err(|e| {
            bad(format!(
                "{}: {e} (an image release needs a release.json)",
                path.display()
            ))
        })?;
        let f: ReleaseFile =
            serde_json::from_str(&text).map_err(|e| bad(format!("{}: {e}", path.display())))?;
        if f.schema_version != 1 {
            return Err(bad(format!(
                "{}: schema_version {} (want 1)",
                path.display(),
                f.schema_version
            )));
        }
        Ok(f)
    }
}

/// Options of `cua images release`.
#[derive(Debug, Clone, Default)]
pub struct ReleaseOpts {
    pub dir: PathBuf,
    pub tier: Option<String>,
    pub arches: Option<Vec<String>>,
    pub stamp: Option<String>,
    pub work: Option<PathBuf>,
    /// Only these steps: phase names or step-id prefixes.
    pub steps: Option<Vec<String>>,
    /// Rerun from this phase or step id; earlier steps must have passed.
    pub from: Option<String>,
    pub resume: bool,
    pub dry_run: bool,
    pub publish: bool,
    pub promote: bool,
    pub vars: Vec<String>,
    /// Capabilities forced on (`--assume kvm`) or off (`--assume !runsc`).
    pub assume: Vec<String>,
    /// Name of this run's state file, `state-<scope>.json` (default: the
    /// arches, or `all`). Jobs that share an evidence directory use distinct
    /// scopes.
    pub scope: Option<String>,
}

/// A planned step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Planned {
    pub id: String,
    pub phase: Phase,
    pub arch: Option<String>,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub stdout: Option<String>,
    pub produces: Vec<String>,
    pub unless_exists: Option<String>,
    /// `None`: runnable; `Some(reason)`: skipped (unmet capability).
    pub unavailable: Option<String>,
    /// A gate that must pass before push.
    pub gate_required: bool,
    pub description: Option<String>,
}

impl Planned {
    fn fingerprint(&self, inputs: &str) -> String {
        let mut h = Sha256::new();
        for a in &self.argv {
            h.update(a.as_bytes());
            h.update([0]);
        }
        for (k, v) in &self.env {
            h.update(format!("{k}={v}\0").as_bytes());
        }
        h.update(self.stdout.as_deref().unwrap_or("").as_bytes());
        h.update([0]);
        h.update(inputs.as_bytes());
        format!("{:x}", h.finalize())
    }
}

/// Recorded result of a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepState {
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub seconds: f64,
    #[serde(default)]
    pub log: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

/// Step results, merged from every `state-*.json` in the evidence directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub steps: BTreeMap<String, StepState>,
}

impl State {
    pub fn load_merged(evidence: &Path) -> State {
        let mut st = State::default();
        let Ok(rd) = std::fs::read_dir(evidence) else {
            return st;
        };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("state-") && n.ends_with(".json"))
            })
            .collect();
        files.sort();
        for f in files {
            if let Ok(t) = std::fs::read_to_string(&f)
                && let Ok(s) = serde_json::from_str::<State>(&t)
            {
                for (k, v) in s.steps {
                    // A pass anywhere wins over a later-merged failure of the
                    // same step only when it is the same fingerprint.
                    match st.steps.get(&k) {
                        Some(old) if old.status == "passed" && v.status != "passed" => {}
                        _ => {
                            st.steps.insert(k, v);
                        }
                    }
                }
            }
        }
        st
    }

    pub fn passed(&self, id: &str, fingerprint: Option<&str>) -> bool {
        self.steps
            .get(id)
            .is_some_and(|s| s.status == "passed" && fingerprint.is_none_or(|f| s.fingerprint == f))
    }
}

/// Host capabilities, detected once (overridable with `--assume`).
#[derive(Debug, Clone, Default)]
pub struct Caps(pub BTreeSet<String>);

impl Caps {
    pub fn detect() -> Self {
        let mut c = BTreeSet::new();
        c.insert(std::env::consts::OS.to_string());
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "amd64"
        };
        // native-<arch>: runs that arch without emulation (gVisor needs it).
        c.insert(format!("native-{arch}"));
        // accel-<arch>: hardware-accelerated VMs of that arch (KVM, HVF).
        if cfg!(target_os = "macos") {
            c.insert(format!("accel-{arch}"));
        }
        if cfg!(target_os = "linux")
            && std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/kvm")
                .is_ok()
        {
            c.insert("kvm".into());
            c.insert(format!("accel-{arch}"));
        }
        if let Ok(out) = Command::new("docker")
            .args(["info", "--format", "{{json .Runtimes}}"])
            .stderr(Stdio::null())
            .output()
            && out.status.success()
        {
            c.insert("docker".into());
            if String::from_utf8_lossy(&out.stdout).contains("\"runsc\"") {
                c.insert("runsc".into());
            }
        }
        Caps(c)
    }

    pub fn apply(&mut self, assume: &[String]) {
        for a in assume {
            match a.strip_prefix('!') {
                Some(off) => {
                    self.0.remove(off);
                }
                None => {
                    self.0.insert(a.clone());
                }
            }
        }
    }

    /// `None` when every requirement holds, else the first unmet one.
    pub fn unmet(&self, requires: &[String]) -> Option<String> {
        requires.iter().find_map(|r| match r.strip_prefix('!') {
            Some(off) if self.0.contains(off) => Some(format!("host has {off}")),
            Some(_) => None,
            None if !self.0.contains(r) => Some(format!("needs {r}")),
            None => None,
        })
    }
}

/// Render `{var}` placeholders (`{{` and `}}` are literal braces); unknown
/// variables are an error.
pub fn render(t: &str, vars: &BTreeMap<String, String>) -> Result<String, CuaError> {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        if rest[i..].starts_with("{{") || rest[i..].starts_with("}}") {
            out.push_str(&rest[i..i + 1]);
            rest = &rest[i + 2..];
            continue;
        }
        if rest[i..].starts_with('}') {
            return Err(bad(format!("unmatched }} in {t:?}")));
        }
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            return Err(bad(format!("unclosed {{ in {t:?}")));
        };
        let key = &after[..j];
        let v = vars
            .get(key)
            .ok_or_else(|| bad(format!("unknown variable {{{key}}} in {t:?}")))?;
        out.push_str(v);
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Everything the plan needs besides the release file.
#[derive(Debug, Clone)]
pub struct Context {
    pub vars: BTreeMap<String, String>,
    pub arches: Vec<String>,
    pub caps: Caps,
    pub work: PathBuf,
}

impl Context {
    pub fn evidence(&self) -> PathBuf {
        self.work.join("evidence")
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// Builds the variables and arches for a run.
pub fn context(rel: &ReleaseFile, o: &ReleaseOpts, caps: Caps) -> Result<Context, CuaError> {
    let dir = o
        .dir
        .canonicalize()
        .map_err(|e| bad(format!("{}: {e}", o.dir.display())))?;
    let root = git(&dir, &["rev-parse", "--show-toplevel"])
        .map(|s| PathBuf::from(s.trim()))
        .unwrap_or_else(|| dir.clone());
    let name = rel.name.clone().unwrap_or_else(|| {
        dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_string()
    });
    let sha = git(&root, &["rev-parse", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let stamp = o.stamp.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            chrono::Utc::now().format("%Y%m%d"),
            &sha[..sha.len().min(7)]
        )
    });
    let (tier, series, tag_t) = if rel.tiers.is_empty() {
        if o.tier.is_some() {
            return Err(bad(format!("{name} has no tiers")));
        }
        (
            String::new(),
            rel.series.clone().unwrap_or_default(),
            rel.tag.clone().unwrap_or_else(|| "build-{stamp}".into()),
        )
    } else {
        let t = o.tier.clone().ok_or_else(|| {
            bad(format!(
                "{name} has tiers ({}); pass --tier",
                rel.tiers.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;
        let def = rel.tiers.get(&t).ok_or_else(|| {
            bad(format!(
                "unknown tier {t}; {name} has {}",
                rel.tiers.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;
        (t, def.series.clone(), def.tag.clone())
    };
    let arches = match &o.arches {
        Some(a) => {
            for x in a {
                if !rel.arches.contains(x) {
                    return Err(bad(format!(
                        "{name} does not build {x} (release.json arches: {})",
                        rel.arches.join(", ")
                    )));
                }
            }
            a.clone()
        }
        None => rel.arches.clone(),
    };
    let work = o
        .work
        .clone()
        .or_else(|| std::env::var_os("CUA_RELEASE_WORK").map(PathBuf::from))
        .unwrap_or_else(|| {
            let leaf = if tier.is_empty() {
                format!("release-{name}")
            } else {
                format!("release-{name}-{tier}")
            };
            cua_disk::Layout::default().build().join(leaf)
        });
    let work = if work.is_absolute() {
        work
    } else {
        std::env::current_dir()
            .map_err(|e| failed(e.to_string()))?
            .join(work)
    };
    let mut vars: BTreeMap<String, String> = rel.vars.clone();
    let cua = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "cua".into());
    let run_url = match (
        std::env::var("GITHUB_SERVER_URL"),
        std::env::var("GITHUB_REPOSITORY"),
        std::env::var("GITHUB_RUN_ID"),
    ) {
        (Ok(s), Ok(r), Ok(i)) => format!("{s}/{r}/actions/runs/{i}"),
        _ => format!("local:{}", sha.get(..12).unwrap_or("")),
    };
    for (k, v) in [
        ("dir", dir.display().to_string()),
        ("root", root.display().to_string()),
        ("name", name.clone()),
        ("tier", tier.clone()),
        ("series", series),
        ("stamp", stamp),
        ("repo", rel.repo.clone()),
        ("work", work.display().to_string()),
        ("evidence", work.join("evidence").display().to_string()),
        ("artifacts", work.join("artifacts").display().to_string()),
        ("out", work.join("out").display().to_string()),
        ("cua", cua),
        ("run_url", run_url),
        ("sha", sha),
        ("arches", arches.join(",")),
    ] {
        vars.insert(k.into(), v);
    }
    // --var wins over release.json and the built-ins.
    let mut overridden = BTreeSet::new();
    for kv in &o.vars {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| bad(format!("--var {kv}: expected KEY=VALUE")))?;
        vars.insert(k.into(), v.into());
        overridden.insert(k.to_string());
    }
    if !overridden.contains("tag") {
        let tag = render(&tag_t, &vars)?;
        vars.insert("tag".into(), tag);
    }
    // release.json defaults and --var values may use the built-ins
    // (`"bin": "{work}/bin"`); one pass, per-arch variables excluded.
    let snapshot = vars.clone();
    for (k, v) in vars.iter_mut() {
        if v.contains('{') && !v.contains("{arch}") {
            *v = render(v, &snapshot).map_err(|e| bad(format!("variable {k}: {e}")))?;
        }
    }
    Ok(Context {
        vars,
        arches,
        caps,
        work,
    })
}

/// Which phases a run includes.
pub fn phases(o: &ReleaseOpts) -> BTreeSet<Phase> {
    let mut p: BTreeSet<Phase> = [Phase::Prepare, Phase::Build, Phase::Gate, Phase::Stage]
        .into_iter()
        .collect();
    if o.publish {
        p.extend([Phase::Push, Phase::Publish, Phase::Verify]);
    }
    if o.promote {
        p.insert(Phase::Promote);
    }
    p
}

/// The full plan (every phase), in run order.
pub fn plan(rel: &ReleaseFile, ctx: &Context) -> Result<Vec<Planned>, CuaError> {
    let mut steps = Vec::new();
    for phase in Phase::ALL {
        for def in rel.steps.iter().filter(|d| d.phase == phase) {
            let arches: Vec<Option<String>> = if def.per_arch {
                ctx.arches
                    .iter()
                    .filter(|a| def.arches.as_ref().is_none_or(|only| only.contains(a)))
                    .map(|a| Some(a.clone()))
                    .collect()
            } else {
                vec![None]
            };
            for arch in arches {
                let mut vars = ctx.vars.clone();
                if let Some(a) = &arch {
                    vars.insert("arch".into(), a.clone());
                }
                let r = |t: &String| render(t, &vars);
                let id = r(&def.id)?;
                steps.push(Planned {
                    id,
                    phase,
                    arch: arch.clone(),
                    argv: def.run.iter().map(r).collect::<Result<_, _>>()?,
                    env: def
                        .env
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), r(v)?)))
                        .collect::<Result<_, CuaError>>()?,
                    stdout: def.stdout.as_ref().map(r).transpose()?,
                    produces: def.produces.iter().map(r).collect::<Result<_, _>>()?,
                    unless_exists: def.unless_exists.as_ref().map(r).transpose()?,
                    unavailable: ctx
                        .caps
                        .unmet(&def.requires.iter().map(r).collect::<Result<Vec<_>, _>>()?),
                    gate_required: matches!(phase, Phase::Gate | Phase::Verify)
                        && def.required.on(arch.as_deref()),
                    description: def.description.clone(),
                });
            }
        }
    }
    let mut seen = BTreeSet::new();
    for s in &steps {
        if !seen.insert(s.id.clone()) {
            return Err(bad(format!("release.json: step id {} is not unique", s.id)));
        }
    }
    Ok(steps)
}

/// What to do with a planned step in this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", content = "reason", rename_all = "lowercase")]
pub enum Decision {
    Run,
    /// Already passed with the same inputs (resume), or `unless_exists`.
    Done(String),
    /// Unmet host capability.
    Unavailable(String),
    /// Not selected (phase not included, `--steps` filter).
    Excluded,
}

fn selected(step: &Planned, filter: &Option<Vec<String>>) -> bool {
    match filter {
        None => true,
        Some(f) => f.iter().any(|x| {
            Phase::parse(x).is_some_and(|p| p == step.phase)
                || step.id == *x
                || step.id.starts_with(&format!("{x}/"))
        }),
    }
}

/// Index of the first step `--from` names.
fn from_index(steps: &[Planned], from: &str) -> Result<usize, CuaError> {
    steps
        .iter()
        .position(|s| {
            Phase::parse(from).is_some_and(|p| s.phase >= p)
                || s.id == from
                || s.id.starts_with(&format!("{from}/"))
        })
        .ok_or_else(|| bad(format!("--from {from}: no such phase or step")))
}

/// Decide every step: the resume / `--from` / filter logic.
pub fn decide(
    steps: &[Planned],
    o: &ReleaseOpts,
    state: &State,
    inputs: &str,
    exists: &dyn Fn(&str) -> bool,
) -> Result<Vec<Decision>, CuaError> {
    let phases = phases(o);
    let from = o
        .from
        .as_deref()
        .map(|f| from_index(steps, f))
        .transpose()?;
    let mut out = Vec::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        if !phases.contains(&s.phase) || !selected(s, &o.steps) {
            out.push(Decision::Excluded);
            continue;
        }
        if let Some(reason) = &s.unavailable {
            out.push(Decision::Unavailable(reason.clone()));
            continue;
        }
        if let Some(p) = &s.unless_exists
            && exists(p)
        {
            out.push(Decision::Done(format!("{p} exists")));
            continue;
        }
        let fp = s.fingerprint(inputs);
        let outputs_present = s.produces.iter().all(|p| exists(p));
        let before_from = from.is_some_and(|f| i < f);
        if before_from {
            if state.passed(&s.id, Some(&fp)) && outputs_present {
                out.push(Decision::Done("passed earlier (before --from)".into()));
            } else {
                return Err(bad(format!(
                    "--from {}: step {} has not passed with these inputs; run it (or drop --from)",
                    o.from.as_deref().unwrap_or(""),
                    s.id
                )));
            }
            continue;
        }
        if o.resume && from.is_none() && state.passed(&s.id, Some(&fp)) && outputs_present {
            out.push(Decision::Done("passed earlier (--resume)".into()));
            continue;
        }
        out.push(Decision::Run);
    }
    Ok(out)
}

/// Gates that block push: required gates of the run's arches that have not
/// passed (in this run or a merged state file).
pub fn missing_gates(steps: &[Planned], state: &State) -> Vec<String> {
    steps
        .iter()
        .filter(|s| s.phase == Phase::Gate && s.gate_required)
        .filter(|s| !state.passed(&s.id, None))
        .map(|s| s.id.clone())
        .collect()
}

/// Verify steps that block promote: every required verify step of the plan
/// that has not passed (in this run or a merged state file).
pub fn missing_verification(steps: &[Planned], state: &State) -> Vec<String> {
    steps
        .iter()
        .filter(|s| s.phase == Phase::Verify && s.gate_required)
        .filter(|s| !state.passed(&s.id, None))
        .map(|s| s.id.clone())
        .collect()
}

/// A digest of the inputs every fingerprint includes: HEAD and the
/// uncommitted diff of the tree.
pub fn inputs_digest(root: &Path) -> String {
    let mut h = Sha256::new();
    h.update(
        git(root, &["rev-parse", "HEAD"])
            .unwrap_or_default()
            .as_bytes(),
    );
    h.update(
        git(root, &["diff", "HEAD", "--binary"])
            .unwrap_or_default()
            .as_bytes(),
    );
    format!("{:x}", h.finalize())
}

fn show(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() || a.contains(' ') {
                format!("'{a}'")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn log_name(id: &str) -> String {
    id.replace('/', "__")
}

/// Run one step: stream its output to the terminal and its log, and its
/// stdout also to `stdout` when set.
fn execute(step: &Planned, cwd: &Path, log: &Path) -> Result<(), CuaError> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
    }
    let mut logf =
        std::fs::File::create(log).map_err(|e| failed(format!("{}: {e}", log.display())))?;
    let _ = writeln!(logf, "+ {}", show(&step.argv));
    eprintln!("+ {}", show(&step.argv));
    let mut stdout_file = match &step.stdout {
        Some(p) => {
            if let Some(d) = Path::new(p).parent() {
                let _ = std::fs::create_dir_all(d);
            }
            Some(std::fs::File::create(p).map_err(|e| failed(format!("{p}: {e}")))?)
        }
        None => None,
    };
    let mut child = Command::new(&step.argv[0])
        .args(&step.argv[1..])
        .envs(&step.env)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| failed(format!("{}: {e}", step.argv[0])))?;
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let out = child.stdout.take().expect("piped");
    let err = child.stderr.take().expect("piped");
    let t1 = {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send((true, line));
            }
        })
    };
    let t2 = std::thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = tx.send((false, line));
        }
    });
    for (is_out, line) in rx {
        eprintln!("  {line}");
        let _ = writeln!(logf, "{line}");
        if is_out && let Some(f) = stdout_file.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
    let _ = t1.join();
    let _ = t2.join();
    let status = child.wait().map_err(|e| failed(e.to_string()))?;
    if !status.success() {
        return Err(failed(format!(
            "{} failed ({status}); log: {}",
            step.id,
            log.display()
        )));
    }
    for p in &step.produces {
        if !Path::new(p).exists() {
            return Err(failed(format!(
                "{} succeeded but did not write {p}",
                step.id
            )));
        }
    }
    Ok(())
}

/// One row of the summary.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub id: String,
    pub phase: Phase,
    pub status: String,
    pub detail: String,
    pub seconds: f64,
    pub log: Option<String>,
}

fn summary_md(rows: &[Row], header: &str) -> String {
    let mut s = format!(
        "### {header}\n\n| Step | Phase | Result | Time | Detail |\n|---|---|---|---|---|\n"
    );
    for r in rows {
        s.push_str(&format!(
            "| `{}` | {} | {} | {:.0}s | {} |\n",
            r.id,
            r.phase.name(),
            r.status,
            r.seconds,
            r.detail.replace('|', "\\|")
        ));
    }
    s
}

/// `cua images release <DIR>`.
pub fn release(o: &ReleaseOpts) -> Result<Value, CuaError> {
    let rel = ReleaseFile::load(&o.dir)?;
    let mut caps = Caps::detect();
    caps.apply(&o.assume);
    let ctx = context(&rel, o, caps)?;
    let steps = plan(&rel, &ctx)?;
    let evidence = ctx.evidence();
    let root = PathBuf::from(&ctx.vars["root"]);
    let inputs = inputs_digest(&root);
    let mut state = State::load_merged(&evidence);
    let exists = |p: &str| Path::new(p).exists();
    let decisions = decide(&steps, o, &state, &inputs, &exists)?;
    let header = format!(
        "{} {}{} ({}): {}",
        ctx.vars["name"],
        ctx.vars["series"],
        if ctx.vars["tier"].is_empty() {
            String::new()
        } else {
            format!(" tier {}", ctx.vars["tier"])
        },
        ctx.arches.join(", "),
        ctx.vars["tag"]
    );
    if o.dry_run {
        let plan: Vec<Value> = steps
            .iter()
            .zip(&decisions)
            .map(|(s, d)| json!({"id": s.id, "phase": s.phase, "arch": s.arch, "decision": d, "run": show(&s.argv), "gate_required": s.gate_required}))
            .collect();
        eprintln!(
            "{header}\nwork: {}\nhost: {}",
            ctx.work.display(),
            ctx.caps.0.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        for (s, d) in steps.iter().zip(&decisions) {
            let what = match d {
                Decision::Run => "run".to_string(),
                Decision::Done(r) => format!("done: {r}"),
                Decision::Unavailable(r) => format!("skip: {r}"),
                Decision::Excluded => continue,
            };
            eprintln!("  {:<9} {:<34} {what}", s.phase.name(), s.id);
            if let Some(d) = &s.description {
                eprintln!("            ({d})");
            }
            eprintln!("            {}", show(&s.argv));
        }
        return Ok(json!({"dry_run": true, "work": ctx.work, "vars": ctx.vars, "steps": plan}));
    }
    std::fs::create_dir_all(&evidence)
        .map_err(|e| failed(format!("{}: {e}", evidence.display())))?;
    let scope = match (&o.scope, &o.arches) {
        (Some(s), _) => s.clone(),
        (None, Some(_)) => ctx.arches.join("-"),
        (None, None) => "all".into(),
    };
    if !scope
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(bad(format!("--scope {scope}: letters, digits, - _ . only")));
    }
    let state_file = evidence.join(format!("state-{scope}.json"));
    let mut own = std::fs::read_to_string(&state_file)
        .ok()
        .and_then(|t| serde_json::from_str::<State>(&t).ok())
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut failure: Option<CuaError> = None;
    let mut failure_phase: Option<Phase> = None;
    for (s, d) in steps.iter().zip(&decisions) {
        match d {
            Decision::Excluded => continue,
            Decision::Done(r) => {
                rows.push(Row {
                    id: s.id.clone(),
                    phase: s.phase,
                    status: "done".into(),
                    detail: r.clone(),
                    seconds: 0.0,
                    log: None,
                });
                continue;
            }
            Decision::Unavailable(r) => {
                let st = StepState {
                    status: "skipped".into(),
                    reason: Some(r.clone()),
                    fingerprint: s.fingerprint(&inputs),
                    seconds: 0.0,
                    log: None,
                    finished_at: Some(chrono::Utc::now().to_rfc3339()),
                };
                own.steps.insert(s.id.clone(), st);
                rows.push(Row {
                    id: s.id.clone(),
                    phase: s.phase,
                    status: "skipped".into(),
                    detail: r.clone(),
                    seconds: 0.0,
                    log: None,
                });
                continue;
            }
            Decision::Run => {}
        }
        // A failed gate does not stop the other gates (one run reports every
        // lane); anything after the gates waits for all of them.
        let gate_failed_only = failure_phase == Some(Phase::Gate) && s.phase == Phase::Gate;
        if failure.is_some() && !gate_failed_only {
            rows.push(Row {
                id: s.id.clone(),
                phase: s.phase,
                status: "not run".into(),
                detail: "an earlier step failed".into(),
                seconds: 0.0,
                log: None,
            });
            continue;
        }
        if matches!(s.phase, Phase::Push | Phase::Promote) {
            // Push: every required gate; promote: every verify step. From
            // this run or a merged state file.
            let mut merged = State::load_merged(&evidence);
            for (k, v) in &own.steps {
                merged.steps.insert(k.clone(), v.clone());
            }
            let (missing, what) = if s.phase == Phase::Push {
                (missing_gates(&steps, &merged), "push: gates")
            } else {
                (
                    missing_verification(&steps, &merged),
                    "promote: verify steps",
                )
            };
            if !missing.is_empty() {
                let e = failed(format!(
                    "refusing to {what} not passed: {}",
                    missing.join(", ")
                ));
                rows.push(Row {
                    id: s.id.clone(),
                    phase: s.phase,
                    status: "refused".into(),
                    detail: e.to_string(),
                    seconds: 0.0,
                    log: None,
                });
                failure = Some(e);
                failure_phase = Some(s.phase);
                continue;
            }
        }
        let log = evidence
            .join("logs")
            .join(format!("{}.log", log_name(&s.id)));
        eprintln!("== {} ({})", s.id, s.phase.name());
        let t0 = Instant::now();
        let r = execute(s, &root, &log);
        let secs = t0.elapsed().as_secs_f64();
        let ok = r.is_ok();
        own.steps.insert(
            s.id.clone(),
            StepState {
                status: if ok { "passed" } else { "failed" }.into(),
                reason: r.as_ref().err().map(|e| e.to_string()),
                fingerprint: s.fingerprint(&inputs),
                seconds: secs,
                log: Some(log.display().to_string()),
                finished_at: Some(chrono::Utc::now().to_rfc3339()),
            },
        );
        let _ = std::fs::write(
            &state_file,
            serde_json::to_vec_pretty(&own).unwrap_or_default(),
        );
        rows.push(Row {
            id: s.id.clone(),
            phase: s.phase,
            status: if ok { "passed" } else { "failed" }.into(),
            detail: r.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
            seconds: secs,
            log: Some(log.display().to_string()),
        });
        if let Err(e) = r
            && failure.is_none()
        {
            failure = Some(e);
            failure_phase = Some(s.phase);
        }
    }
    let _ = std::fs::write(
        &state_file,
        serde_json::to_vec_pretty(&own).unwrap_or_default(),
    );
    state.steps.extend(own.steps.clone());
    let md = summary_md(&rows, &header);
    let _ = std::fs::write(evidence.join(format!("summary-{scope}.md")), &md);
    let summary = json!({"image": ctx.vars["name"], "tier": ctx.vars["tier"], "series": ctx.vars["series"], "tag": ctx.vars["tag"], "stamp": ctx.vars["stamp"], "arches": ctx.arches, "work": ctx.work, "steps": rows});
    let _ = std::fs::write(
        evidence.join(format!("summary-{scope}.json")),
        serde_json::to_vec_pretty(&summary).unwrap_or_default(),
    );
    if let Ok(p) = std::env::var("GITHUB_STEP_SUMMARY")
        && let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(p)
    {
        let _ = f.write_all(md.as_bytes());
    }
    eprintln!("\n{md}");
    match failure {
        Some(e) => Err(e),
        None => Ok(summary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(json: &str) -> ReleaseFile {
        serde_json::from_str(json).unwrap()
    }

    const TIERED: &str = r#"{
      "schema_version": 1, "repo": "ghcr.io/x/linux", "arches": ["amd64", "arm64"],
      "tiers": {"slim": {"series": "24.04-slim", "tag": "build-slim-{stamp}"}, "full": {"series": "24.04", "tag": "build-{stamp}"}},
      "steps": [
        {"id": "build/{arch}", "phase": "build", "per_arch": true, "run": ["{cua}", "images", "build", "{dir}", "--target", "{tier}", "--platform", "linux/{arch}", "--tag", "{tag}"], "produces": ["{out}/{arch}/disk.img"]},
        {"id": "doctor/{arch}/runc", "phase": "gate", "per_arch": true, "run": ["doctor", "runc", "{repo}:docker-{tag}-{arch}"]},
        {"id": "doctor/{arch}/runsc", "phase": "gate", "per_arch": true, "requires": ["runsc"], "run": ["doctor", "runsc"]},
        {"id": "doctor/{arch}/qemu", "phase": "gate", "per_arch": true, "requires": ["kvm"], "required": ["amd64"], "run": ["doctor", "qemu"]},
        {"id": "boot/{arch}", "phase": "gate", "per_arch": true, "requires": ["!kvm"], "arches": ["arm64"], "required": false, "run": ["boot"]},
        {"id": "push/{arch}", "phase": "push", "per_arch": true, "run": ["push", "{arch}"]},
        {"id": "publish", "phase": "publish", "run": ["publish", "{series}", "{stamp}"]},
        {"id": "promote", "phase": "promote", "run": ["promote", "{evidence}/pins.json"]}
      ]}"#;

    fn ctx_for(r: &ReleaseFile, o: &ReleaseOpts, caps: &[&str]) -> Context {
        let dir = tempfile::tempdir().unwrap();
        let o = ReleaseOpts {
            dir: dir.path().to_path_buf(),
            stamp: Some("20260926-abcdef1".into()),
            work: Some(PathBuf::from("/w")),
            ..o.clone()
        };
        let c = context(r, &o, Caps(caps.iter().map(|s| s.to_string()).collect())).unwrap();
        std::mem::forget(dir);
        c
    }

    #[test]
    fn tiers_set_series_and_tag_and_every_step_is_rendered() {
        let r = rel(TIERED);
        let o = ReleaseOpts {
            tier: Some("slim".into()),
            ..Default::default()
        };
        let c = ctx_for(&r, &o, &["linux", "docker", "runsc", "kvm"]);
        assert_eq!(c.vars["series"], "24.04-slim");
        assert_eq!(c.vars["tag"], "build-slim-20260926-abcdef1");
        let p = plan(&r, &c).unwrap();
        let ids: Vec<&str> = p.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "build/amd64",
                "build/arm64",
                "doctor/amd64/runc",
                "doctor/arm64/runc",
                "doctor/amd64/runsc",
                "doctor/arm64/runsc",
                "doctor/amd64/qemu",
                "doctor/arm64/qemu",
                "boot/arm64",
                "push/amd64",
                "push/arm64",
                "publish",
                "promote"
            ]
        );
        assert!(
            p[0].argv.contains(&"--target".to_string()) && p[0].argv.contains(&"slim".to_string())
        );
        // Host-native paths: `D:\w\out\...` on Windows (absolute paths take
        // the current drive), `/w/out/...` elsewhere.
        let posix = |s: &str| {
            let s = s.replace('\\', "/");
            match s.as_bytes() {
                [d, b':', b'/', ..] if d.is_ascii_alphabetic() => s[2..].to_string(),
                _ => s,
            }
        };
        assert_eq!(posix(&p[0].produces[0]), "/w/out/amd64/disk.img");
        assert_eq!(p[0].produces.len(), 1);
        assert_eq!(
            p[2].argv[2],
            "ghcr.io/x/linux:docker-build-slim-20260926-abcdef1-amd64"
        );
        // boot is only for arm64 and skipped on a KVM host.
        assert!(p[8].unavailable.is_some());
        // qemu gates only required on amd64.
        assert!(p[6].gate_required && !p[7].gate_required);
        assert!(!p[8].gate_required);
        assert_eq!(posix(&p.last().unwrap().argv[1]), "/w/evidence/pins.json");
    }

    #[test]
    fn tier_is_required_and_checked() {
        let r = rel(TIERED);
        let dir = tempfile::tempdir().unwrap();
        let base = ReleaseOpts {
            dir: dir.path().into(),
            ..Default::default()
        };
        assert!(context(&r, &base, Caps::default()).is_err());
        let o = ReleaseOpts {
            tier: Some("huge".into()),
            ..base.clone()
        };
        assert!(context(&r, &o, Caps::default()).is_err());
        let o = ReleaseOpts {
            tier: Some("full".into()),
            arches: Some(vec!["riscv64".into()]),
            ..base
        };
        assert!(context(&r, &o, Caps::default()).is_err());
    }

    #[test]
    fn render_rejects_unknown_variables() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), "1".into());
        assert_eq!(render("x{a}y{a}", &v).unwrap(), "x1y1");
        assert!(render("{b}", &v).is_err());
        assert!(render("{a", &v).is_err());
        assert!(render("a}", &v).is_err());
        assert_eq!(render("{{.Id}} {a}", &v).unwrap(), "{.Id} 1");
    }

    #[test]
    fn caps_negation_and_assume() {
        let mut c = Caps(["linux".to_string()].into_iter().collect());
        assert!(c.unmet(&["kvm".into()]).is_some());
        assert!(c.unmet(&["!kvm".into()]).is_none());
        c.apply(&["kvm".into()]);
        assert!(c.unmet(&["kvm".into()]).is_none());
        assert!(c.unmet(&["!kvm".into()]).is_some());
        c.apply(&["!kvm".into()]);
        assert!(c.unmet(&["kvm".into()]).is_some());
    }

    fn passed_state(steps: &[Planned], ids: &[&str], inputs: &str) -> State {
        let mut st = State::default();
        for s in steps.iter().filter(|s| ids.contains(&s.id.as_str())) {
            st.steps.insert(
                s.id.clone(),
                StepState {
                    status: "passed".into(),
                    reason: None,
                    fingerprint: s.fingerprint(inputs),
                    seconds: 1.0,
                    log: None,
                    finished_at: None,
                },
            );
        }
        st
    }

    #[test]
    fn default_run_stops_before_push_and_skips_unavailable_lanes() {
        let r = rel(TIERED);
        let o = ReleaseOpts {
            tier: Some("full".into()),
            ..Default::default()
        };
        let c = ctx_for(&r, &o, &["linux", "docker"]);
        let p = plan(&r, &c).unwrap();
        let d = decide(&p, &o, &State::default(), "i", &|_| false).unwrap();
        let by: BTreeMap<&str, &Decision> = p.iter().map(|s| s.id.as_str()).zip(d.iter()).collect();
        assert_eq!(by["build/amd64"], &Decision::Run);
        assert!(matches!(by["doctor/amd64/runsc"], Decision::Unavailable(_)));
        assert!(matches!(by["doctor/amd64/qemu"], Decision::Unavailable(_)));
        assert_eq!(by["boot/arm64"], &Decision::Run);
        assert_eq!(by["push/amd64"], &Decision::Excluded);
        assert_eq!(by["promote"], &Decision::Excluded);
        let o = ReleaseOpts {
            publish: true,
            promote: true,
            ..o
        };
        let d = decide(&p, &o, &State::default(), "i", &|_| false).unwrap();
        assert!(d.iter().all(|x| *x != Decision::Excluded));
    }

    #[test]
    fn resume_skips_passed_steps_with_the_same_inputs_and_outputs() {
        let r = rel(TIERED);
        let o = ReleaseOpts {
            tier: Some("full".into()),
            resume: true,
            ..Default::default()
        };
        let c = ctx_for(&r, &o, &["linux", "docker", "runsc", "kvm"]);
        let p = plan(&r, &c).unwrap();
        let st = passed_state(&p, &["build/amd64", "doctor/amd64/runc"], "i");
        // Outputs present: done.
        let d = decide(&p, &o, &st, "i", &|_| true).unwrap();
        assert!(matches!(d[0], Decision::Done(_)));
        assert_eq!(d[1], Decision::Run);
        assert!(matches!(d[2], Decision::Done(_)));
        // Changed inputs: rerun.
        let d = decide(&p, &o, &st, "other", &|_| true).unwrap();
        assert_eq!(d[0], Decision::Run);
        // The disk is gone: rerun the build.
        let d = decide(&p, &o, &st, "i", &|_| false).unwrap();
        assert_eq!(d[0], Decision::Run);
        // Without --resume everything selected runs.
        let o2 = ReleaseOpts {
            resume: false,
            ..o.clone()
        };
        assert_eq!(
            decide(&p, &o2, &st, "i", &|_| true).unwrap()[0],
            Decision::Run
        );
    }

    #[test]
    fn from_requires_earlier_steps_and_reruns_the_rest() {
        let r = rel(TIERED);
        let o = ReleaseOpts {
            tier: Some("full".into()),
            from: Some("gate".into()),
            ..Default::default()
        };
        let c = ctx_for(&r, &o, &["linux", "docker"]);
        let p = plan(&r, &c).unwrap();
        // Builds not recorded: --from gate refuses.
        assert!(decide(&p, &o, &State::default(), "i", &|_| true).is_err());
        let st = passed_state(
            &p,
            &["build/amd64", "build/arm64", "doctor/amd64/runc"],
            "i",
        );
        let d = decide(&p, &o, &st, "i", &|_| true).unwrap();
        assert!(matches!(d[0], Decision::Done(_)) && matches!(d[1], Decision::Done(_)));
        // A passed gate at/after --from runs again.
        assert_eq!(d[2], Decision::Run);
        // --from a step id prefix: the steps before it passed (runsc and
        // qemu are unavailable on this host, so they neither block nor run).
        let o = ReleaseOpts {
            from: Some("doctor/arm64".into()),
            ..o
        };
        let d = decide(&p, &o, &st, "i", &|_| true).unwrap();
        assert_eq!(p[3].id, "doctor/arm64/runc");
        assert_eq!(d[3], Decision::Run);
        assert!(matches!(d[2], Decision::Done(_)));
        assert!(
            decide(
                &p,
                &ReleaseOpts {
                    from: Some("nope".into()),
                    ..o
                },
                &st,
                "i",
                &|_| true
            )
            .is_err()
        );
    }

    #[test]
    fn steps_filter_by_phase_or_id_prefix() {
        let r = rel(TIERED);
        let o = ReleaseOpts {
            tier: Some("full".into()),
            publish: true,
            steps: Some(vec!["push".into(), "doctor/arm64".into()]),
            ..Default::default()
        };
        let c = ctx_for(&r, &o, &["linux", "docker", "runsc", "kvm"]);
        let p = plan(&r, &c).unwrap();
        let d = decide(&p, &o, &State::default(), "i", &|_| false).unwrap();
        let run: Vec<&str> = p
            .iter()
            .zip(&d)
            .filter(|(_, d)| **d == Decision::Run)
            .map(|(s, _)| s.id.as_str())
            .collect();
        assert_eq!(
            run,
            [
                "doctor/arm64/runc",
                "doctor/arm64/runsc",
                "doctor/arm64/qemu",
                "push/amd64",
                "push/arm64"
            ]
        );
    }

    #[test]
    fn push_needs_every_required_gate() {
        let r = rel(TIERED);
        let o = ReleaseOpts {
            tier: Some("full".into()),
            ..Default::default()
        };
        let c = ctx_for(&r, &o, &["linux", "docker", "runsc"]);
        let p = plan(&r, &c).unwrap();
        let all_but_qemu = [
            "doctor/amd64/runc",
            "doctor/arm64/runc",
            "doctor/amd64/runsc",
            "doctor/arm64/runsc",
        ];
        let st = passed_state(&p, &all_but_qemu, "i");
        assert_eq!(missing_gates(&p, &st), ["doctor/amd64/qemu"]);
        let mut st2 = st.clone();
        st2.steps.insert(
            "doctor/amd64/qemu".into(),
            StepState {
                status: "passed".into(),
                reason: None,
                fingerprint: "from another job".into(),
                seconds: 0.0,
                log: None,
                finished_at: None,
            },
        );
        assert!(missing_gates(&p, &st2).is_empty());
    }

    #[test]
    fn promote_needs_every_verify_step() {
        let r = rel(
            r#"{"schema_version":1,"repo":"r","arches":["amd64"],"series":"1","steps":[
            {"id":"g","phase":"gate","run":["true"]},
            {"id":"verify/{arch}","phase":"verify","per_arch":true,"run":["true"]},
            {"id":"promote","phase":"promote","run":["true"]}]}"#,
        );
        let c = ctx_for(&r, &ReleaseOpts::default(), &[]);
        let p = plan(&r, &c).unwrap();
        assert_eq!(
            missing_verification(&p, &State::default()),
            ["verify/amd64"]
        );
        let st = passed_state(&p, &["verify/amd64"], "i");
        assert!(missing_verification(&p, &st).is_empty());
    }

    #[test]
    fn a_failed_gate_runs_the_other_gates_and_stops_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::write(
            dir.path().join("release.json"),
            r#"{"schema_version":1,"repo":"r","arches":["amd64"],"series":"1","steps":[
              {"id":"a","phase":"gate","run":["false"]},
              {"id":"b","phase":"gate","run":["touch","{work}/b-ran"]},
              {"id":"c","phase":"stage","run":["touch","{work}/c-ran"]}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let o = ReleaseOpts {
            dir: dir.path().into(),
            work: Some(work.clone()),
            stamp: Some("20260926-abcdef1".into()),
            ..Default::default()
        };
        assert!(release(&o).is_err());
        assert!(work.join("b-ran").exists(), "the second gate still ran");
        assert!(!work.join("c-ran").exists(), "stage waits for every gate");
        let st = State::load_merged(&work.join("evidence"));
        assert_eq!(st.steps["a"].status, "failed");
        assert_eq!(st.steps["b"].status, "passed");
        assert!(!st.steps.contains_key("c"));
        // Fix the gate and resume: only a and c run.
        std::fs::write(
            dir.path().join("release.json"),
            r#"{"schema_version":1,"repo":"r","arches":["amd64"],"series":"1","steps":[
              {"id":"a","phase":"gate","run":["true"]},
              {"id":"b","phase":"gate","run":["touch","{work}/b-ran"]},
              {"id":"c","phase":"stage","run":["touch","{work}/c-ran"]}]}"#,
        )
        .unwrap();
        std::fs::remove_file(work.join("b-ran")).unwrap();
        let v = release(&ReleaseOpts { resume: true, ..o }).unwrap();
        let status: Vec<(String, String)> = v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["id"].as_str().unwrap().into(),
                    r["status"].as_str().unwrap().into(),
                )
            })
            .collect();
        assert_eq!(
            status,
            [
                ("a".to_string(), "passed".to_string()),
                ("b".into(), "done".into()),
                ("c".into(), "passed".into())
            ]
        );
        assert!(work.join("c-ran").exists() && !work.join("b-ran").exists());
    }

    #[test]
    fn states_merge_across_jobs() {
        let d = tempfile::tempdir().unwrap();
        let mk = |id: &str, status: &str| State {
            steps: [(
                id.to_string(),
                StepState {
                    status: status.into(),
                    reason: None,
                    fingerprint: "f".into(),
                    seconds: 0.0,
                    log: None,
                    finished_at: None,
                },
            )]
            .into_iter()
            .collect(),
        };
        std::fs::write(
            d.path().join("state-amd64.json"),
            serde_json::to_vec(&mk("a", "passed")).unwrap(),
        )
        .unwrap();
        std::fs::write(
            d.path().join("state-arm64.json"),
            serde_json::to_vec(&mk("b", "passed")).unwrap(),
        )
        .unwrap();
        std::fs::write(
            d.path().join("state-zz.json"),
            serde_json::to_vec(&mk("a", "failed")).unwrap(),
        )
        .unwrap();
        let st = State::load_merged(d.path());
        assert!(st.passed("a", None) && st.passed("b", Some("f")) && !st.passed("c", None));
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let r = rel(
            r#"{"schema_version":1,"repo":"r","arches":["amd64","arm64"],"steps":[{"id":"x","phase":"build","per_arch":true,"run":["true"]}]}"#,
        );
        let c = ctx_for(&r, &ReleaseOpts::default(), &[]);
        assert!(plan(&r, &c).is_err());
    }

    /// Every release.json in the tree loads and plans.
    #[test]
    fn repo_release_files_plan() {
        let images = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../images");
        let mut found = 0;
        let mut stack = vec![images];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.file_name().is_some_and(|n| n == "release.json") {
                    let dir = p.parent().unwrap().to_path_buf();
                    let r = ReleaseFile::load(&dir).unwrap();
                    let tiers: Vec<Option<String>> = if r.tiers.is_empty() {
                        vec![None]
                    } else {
                        r.tiers.keys().cloned().map(Some).collect()
                    };
                    for tier in tiers {
                        let o = ReleaseOpts {
                            dir: dir.clone(),
                            tier,
                            stamp: Some("20260926-abcdef1".into()),
                            work: Some("/w".into()),
                            publish: true,
                            promote: true,
                            ..Default::default()
                        };
                        let c = context(&r, &o, Caps::default()).unwrap();
                        let p = plan(&r, &c).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
                        assert!(
                            p.iter().any(|s| s.phase == Phase::Build),
                            "{}",
                            dir.display()
                        );
                        assert!(
                            p.iter().any(|s| s.phase == Phase::Gate && s.gate_required),
                            "{}: no required gate",
                            dir.display()
                        );
                        assert!(
                            p.iter().any(|s| s.phase == Phase::Promote),
                            "{}",
                            dir.display()
                        );
                    }
                    found += 1;
                }
            }
        }
        assert!(found >= 1);
    }
}
