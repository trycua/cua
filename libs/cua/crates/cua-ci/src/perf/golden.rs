//! The saved-memory warm start.
//!
//! One **golden** VM is booted once and set up completely: Xcode first
//! launch done, Homebrew, Rust, Node and pnpm, sccache and the actions
//! runner installed, the toolchains run once so their pages are hot, the
//! services a CI VM does not need turned off. Then it is checkpointed with
//! its memory (`Space::checkpoint`, 650 ms on an M2 Max). A job VM is a
//! fork of that checkpoint (`Spaces::create` with image
//! `checkpoint:<name>`: restore 610 ms, fork of a running Space 1160 ms),
//! so it starts with everything already running and in memory instead of
//! booting (about 16 s for the runner to resume from a plain save before,
//! 20 to 30 s for a cold boot).
//!
//! What is in a golden image is its [`GoldenSpec`]; the spec's
//! [`digest`](GoldenSpec::digest) names the checkpoint, so a changed spec is
//! a different image and the old one is never mistaken for it
//! ([`assess`]). It is rebuilt when:
//!
//! - the base image, the Xcode version, a pinned tool or the runner version
//!   changes (at once: jobs would otherwise run the wrong toolchain),
//! - a baked lockfile changes (no sooner than `min_rebuild` after the last
//!   build: a dependency bump is not worth a half hour of building),
//! - it is older than `max_age` (a weekly refresh keeps OS and tool
//!   patches coming),
//! - its checkpoint is gone, or a fork of it failed verification.
//!
//! [`refresh`] builds the next one beside the current: the current golden
//! stays in use until the new one is verified by forking it, answering a
//! probe and printing the versions the spec pinned.
//!
//! macOS needs one checkpoint per job slot ([`GoldenSpec::variants`]):
//! two forks of one checkpoint would restore with the same network
//! identity on the same vmnet, which is only safe to rule out by a live
//! run (NEEDS-VERIFICATION). Each variant is a checkpoint of the same
//! build VM, taken one after the other (milliseconds), so they cost one
//! build.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::cache::TarballSource;
use crate::config::{GuestOs, SPACE_LABEL_POOL};
use crate::guest;
use crate::pool::Clock;
use crate::spaces::{SpaceSpec, Spaces, SpacesError};
use crate::{Error, Result};

/// Forks that fail in a row before the golden image is rebuilt.
pub const FORK_FAILURES_TO_REBUILD: u32 = 3;

/// Bump to invalidate every golden image when the recipe's logic changes.
pub const RECIPE_VERSION: u32 = 1;

/// The macOS recipe script.
pub const MACOS_RECIPE: &str = include_str!("../../golden/macos.sh");

/// The Space label that marks a golden build VM.
pub const LABEL_GOLDEN_BUILD: &str = "cua-ci-golden";

/// What is in a golden image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldenSpec {
    /// `macos` or `linux`.
    pub os: GuestOs,
    /// `arm64` or `x64`.
    pub arch: String,
    /// The image the build starts from (`ghcr.io/trycua/macos:26`).
    pub base_image: String,
    /// The base image's digest when known: a base image pushed again under
    /// the same tag changes it.
    #[serde(default)]
    pub base_digest: Option<String>,
    /// The Xcode version (macOS).
    #[serde(default)]
    pub xcode: Option<String>,
    /// Pinned tools: `node`, `pnpm`, `rust`, extra Homebrew formulae...
    #[serde(default)]
    pub tools: BTreeMap<String, String>,
    /// Lockfiles whose dependencies are baked in: path to the sha256 of
    /// the content.
    #[serde(default)]
    pub lockfiles: BTreeMap<String, String>,
    /// The actions/runner version baked in.
    pub runner_version: String,
    /// [`RECIPE_VERSION`] when the spec was made.
    pub recipe_version: u32,
    /// Services a CI VM does not need are turned off.
    #[serde(default)]
    pub trim_guest: bool,
}

/// What changed between two specs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The base image or its digest.
    BaseImage,
    /// Xcode.
    Xcode,
    /// A pinned tool (its name).
    Tool(String),
    /// A baked lockfile (its path).
    Lockfile(String),
    /// The runner.
    Runner,
    /// The recipe or the guest trim.
    Recipe,
}

impl Change {
    /// A change that rebuilds at once (everything but a lockfile).
    pub fn is_urgent(&self) -> bool {
        !matches!(self, Change::Lockfile(_))
    }

    /// For people and events.
    pub fn describe(&self) -> String {
        match self {
            Change::BaseImage => "the base image changed".into(),
            Change::Xcode => "the Xcode version changed".into(),
            Change::Tool(t) if t.starts_with("vm_") => "the VM size changed".into(),
            Change::Tool(t) => format!("the pinned {t} changed"),
            Change::Lockfile(p) => format!("the lockfile {p} changed"),
            Change::Runner => "the actions runner version changed".into(),
            Change::Recipe => "the build recipe changed".into(),
        }
    }
}

impl GoldenSpec {
    /// A hash of everything in the spec (canonical JSON: the maps are
    /// ordered).
    pub fn digest(&self) -> String {
        let json = serde_json::to_vec(self).expect("spec json");
        hex::encode(Sha256::digest(json))
    }

    /// The first 12 characters of [`GoldenSpec::digest`].
    pub fn short(&self) -> String {
        self.digest()[..12].to_string()
    }

    /// How many checkpoints one build takes: one per job slot on macOS
    /// (Apple's two VMs), one elsewhere.
    pub fn variants(&self) -> u32 {
        if self.os == GuestOs::Macos {
            crate::config::MACOS_VMS_PER_HOST
        } else {
            1
        }
    }

    /// The checkpoint of variant `i`.
    pub fn checkpoint_name(&self, i: u32) -> String {
        format!(
            "cua-ci-golden-{}-{}-{}-v{}",
            self.os.as_str(),
            self.arch,
            self.short(),
            i
        )
    }

    /// The image a fork starts from.
    pub fn fork_image(&self, i: u32) -> String {
        format!("checkpoint:{}", self.checkpoint_name(i))
    }

    /// What differs from `other`.
    pub fn changes_from(&self, other: &GoldenSpec) -> Vec<Change> {
        let mut c = Vec::new();
        if self.base_image != other.base_image || self.base_digest != other.base_digest {
            c.push(Change::BaseImage);
        }
        if self.xcode != other.xcode {
            c.push(Change::Xcode);
        }
        for k in self
            .tools
            .keys()
            .chain(other.tools.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            if self.tools.get(k) != other.tools.get(k) {
                c.push(Change::Tool(k.clone()));
            }
        }
        for k in self
            .lockfiles
            .keys()
            .chain(other.lockfiles.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            if self.lockfiles.get(k) != other.lockfiles.get(k) {
                c.push(Change::Lockfile(k.clone()));
            }
        }
        if self.runner_version != other.runner_version {
            c.push(Change::Runner);
        }
        if self.recipe_version != other.recipe_version
            || self.trim_guest != other.trim_guest
            || self.os != other.os
            || self.arch != other.arch
        {
            c.push(Change::Recipe);
        }
        c
    }

    /// The `export` lines the recipe reads its pins from.
    pub fn recipe_env(&self) -> String {
        let q = guest::sh_quote;
        let mut out = String::new();
        if let Some(x) = &self.xcode {
            out.push_str(&format!("export CUA_CI_XCODE={}\n", q(x)));
        }
        for (k, name) in [
            ("node", "CUA_CI_NODE"),
            ("pnpm", "CUA_CI_PNPM"),
            ("rust", "CUA_CI_RUST"),
            ("brew", "CUA_CI_BREW"),
        ] {
            if let Some(v) = self.tools.get(k) {
                out.push_str(&format!("export {name}={}\n", q(v)));
            }
        }
        out
    }
}

/// One built golden image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldenRecord {
    /// What is in it.
    pub spec: GoldenSpec,
    /// The spec's digest.
    pub digest: String,
    /// Its checkpoints, by variant.
    pub checkpoints: Vec<String>,
    /// Built (Unix ms).
    pub built_ms: u64,
    /// Verified by a fork (Unix ms).
    pub verified_ms: Option<u64>,
    /// How long the build took.
    pub build_ms: u64,
    /// The fork to probe-answered time of the verification fork.
    pub fork_ready_ms: Option<u64>,
    /// The versions the image reported (`xcode=26.0.1`, ...), for
    /// toolchain keys.
    #[serde(default)]
    pub versions: BTreeMap<String, String>,
}

/// What survives a restart: `golden.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldenState {
    /// The image jobs fork from.
    #[serde(default)]
    pub current: Option<GoldenRecord>,
    /// The one before, kept until nothing runs from it.
    #[serde(default)]
    pub previous: Option<GoldenRecord>,
    /// Forks of the current one that failed in a row (at
    /// [`FORK_FAILURES_TO_REBUILD`] it is rebuilt).
    #[serde(default)]
    pub fork_failures: u32,
    /// The last build attempt (Unix ms), success or not.
    #[serde(default)]
    pub last_attempt_ms: u64,
}

impl GoldenState {
    /// Reads `golden.json` (empty when there is none).
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes `golden.json`.
    pub fn save(&self, path: &Path) -> Result<()> {
        cua_home::guard_write(path)?;
        let body =
            serde_json::to_vec_pretty(self).map_err(|e| Error::Io(std::io::Error::other(e)))?;
        cua_home::write_private(path, &body)?;
        Ok(())
    }

    /// The key warm VMs carry (the current digest).
    pub fn key(&self) -> Option<String> {
        self.current.as_ref().map(|r| r.digest.clone())
    }
}

/// Why the golden image is rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// There is none.
    Missing,
    /// What is wanted differs.
    Changed(Vec<Change>),
    /// Older than `max_age`.
    Age,
    /// A checkpoint of the image is gone.
    CheckpointGone(String),
    /// Forks of it keep failing.
    ForkFailed,
}

impl Why {
    /// For people and events.
    pub fn describe(&self) -> String {
        match self {
            Why::Missing => "no golden image yet".into(),
            Why::Changed(c) => c
                .iter()
                .map(Change::describe)
                .collect::<Vec<_>>()
                .join(", "),
            Why::Age => "the golden image is past its maximum age".into(),
            Why::CheckpointGone(n) => format!("its checkpoint {n} is gone"),
            Why::ForkFailed => "forks of it keep failing".into(),
        }
    }
}

/// What to do about the golden image now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Keep using it.
    Fresh,
    /// Build the next one.
    Rebuild(Why),
    /// A rebuild is due but waits (a lockfile-only change inside
    /// `min_rebuild`, or a failed build inside the retry pause).
    Wait,
}

/// Timing of rebuilds.
#[derive(Clone, Copy, Debug)]
pub struct Cadence {
    /// A rebuild for a lockfile change waits this long after the last build.
    pub min_rebuild: Duration,
    /// A rebuild is rebuilt anyway after this long.
    pub max_age: Duration,
    /// After a failed build, wait this long before trying again.
    pub retry_pause: Duration,
}

impl Default for Cadence {
    fn default() -> Self {
        Self {
            min_rebuild: Duration::from_secs(24 * 3600),
            max_age: Duration::from_secs(7 * 86_400),
            retry_pause: Duration::from_secs(30 * 60),
        }
    }
}

/// Decides whether to keep the golden image. `existing` are the
/// checkpoints this computer holds.
pub fn assess(
    want: &GoldenSpec,
    st: &GoldenState,
    now: u64,
    c: Cadence,
    existing: &[String],
) -> Verdict {
    let paused = now.saturating_sub(st.last_attempt_ms) < c.retry_pause.as_millis() as u64
        && st.last_attempt_ms > st.current.as_ref().map(|r| r.built_ms).unwrap_or(0);
    let Some(cur) = &st.current else {
        return if paused {
            Verdict::Wait
        } else {
            Verdict::Rebuild(Why::Missing)
        };
    };
    let rebuild = |w: Why| {
        if paused {
            Verdict::Wait
        } else {
            Verdict::Rebuild(w)
        }
    };
    if let Some(gone) = cur.checkpoints.iter().find(|n| !existing.contains(n)) {
        return rebuild(Why::CheckpointGone(gone.clone()));
    }
    if st.fork_failures >= FORK_FAILURES_TO_REBUILD {
        return rebuild(Why::ForkFailed);
    }
    let changes = want.changes_from(&cur.spec);
    if !changes.is_empty() {
        let age = now.saturating_sub(cur.built_ms);
        if changes.iter().any(Change::is_urgent) || age >= c.min_rebuild.as_millis() as u64 {
            return rebuild(Why::Changed(changes));
        }
        return Verdict::Wait;
    }
    if now.saturating_sub(cur.built_ms) >= c.max_age.as_millis() as u64 {
        return rebuild(Why::Age);
    }
    Verdict::Fresh
}

/// The variant for a new slot: the lowest not in use, else the first.
pub fn pick_variant(in_use: &[u32], variants: u32) -> u32 {
    (0..variants.max(1))
        .find(|v| !in_use.contains(v))
        .unwrap_or(0)
}

/// What the guest must do after it was restored from a checkpoint, before
/// it is a CI VM: a restored VM wakes up in the past with the old
/// computer's network lease.
pub fn fork_fixup_script(host_epoch_secs: u64, os: GuestOs) -> String {
    match os {
        GuestOs::Macos => format!(
            "# cua-ci:fixup\n\
             sudo -n date -u -f %s {host_epoch_secs} >/dev/null 2>&1 || true\n\
             sudo -n ipconfig set en0 DHCP >/dev/null 2>&1 || true\n\
             rm -f \"$HOME/actions-runner/.cua-exit\" \"$HOME/actions-runner/.cua-pid\" \
             \"$HOME/actions-runner/.cua-jitconfig\"\n\
             exit 0\n"
        ),
        GuestOs::Linux => format!(
            "# cua-ci:fixup\n\
             sudo -n date -u -s @{host_epoch_secs} >/dev/null 2>&1 || true\n\
             rm -f \"$HOME/actions-runner/.cua-exit\" \"$HOME/actions-runner/.cua-pid\" \
             \"$HOME/actions-runner/.cua-jitconfig\"\n\
             exit 0\n"
        ),
    }
}

/// Parses the `key=value` lines of `~/.cua-ci-versions`.
pub fn parse_versions(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect()
}

impl GoldenSpec {
    /// A spec with the usual pins: `os` and `arch`, the base image, the
    /// runner version, and whatever tools, Xcode and lockfiles the caller
    /// reads from the computer and the repositories.
    pub fn basic(
        os: GuestOs,
        arch: &str,
        base_image: &str,
        runner_version: &str,
        trim_guest: bool,
    ) -> Self {
        Self {
            os,
            arch: arch.to_string(),
            base_image: base_image.to_string(),
            base_digest: None,
            xcode: None,
            tools: BTreeMap::new(),
            lockfiles: BTreeMap::new(),
            runner_version: runner_version.to_string(),
            recipe_version: RECIPE_VERSION,
            trim_guest,
        }
    }
}

/// The sha256 of each of `paths` (those that exist), keyed by the path as
/// given: the `lockfiles` of a [`GoldenSpec`].
pub fn lockfile_digests(paths: &[&Path]) -> BTreeMap<String, String> {
    paths
        .iter()
        .filter_map(|p| {
            let bytes = std::fs::read(p).ok()?;
            Some((p.display().to_string(), hex::encode(Sha256::digest(bytes))))
        })
        .collect()
}

/// What a refresh runs on.
pub struct Refresh {
    /// Spaces.
    pub spaces: Arc<dyn Spaces>,
    /// Runner tarballs.
    pub tarballs: Arc<dyn TarballSource>,
    /// The time.
    pub clock: Arc<dyn Clock>,
    /// Where Spaces run (`local`).
    pub on: String,
    /// vCPUs and MiB of the build VM (a golden VM is sized like the job
    /// VMs it will fork into).
    pub size: (Option<u32>, Option<u64>),
}

/// The result of a refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refreshed {
    /// The new image (already verified).
    pub record: GoldenRecord,
    /// Checkpoints of the one it replaces: delete them once no VM runs
    /// from them.
    pub retire: Vec<String>,
}

const RECIPE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const SH_TIMEOUT: Duration = Duration::from_secs(300);

/// Builds the next golden image: a build Space from the base image, the
/// recipe, the hooks and the runner, a warm-up, one checkpoint per
/// variant, and a verification fork. The build Space and the
/// verification fork are always deleted, success or not. The current
/// image keeps serving until the caller stores the result.
pub async fn refresh(
    r: &Refresh,
    spec: &GoldenSpec,
    current: Option<&GoldenRecord>,
    in_use: &(dyn Fn(&str) -> bool + Sync),
) -> Result<Refreshed> {
    let t0 = r.clock.now_ms();
    let name = format!("cua-ci-golden-build-{}", &spec.short()[..8]);
    let build = r
        .spaces
        .create(&SpaceSpec {
            name: name.clone(),
            image: spec.base_image.clone(),
            on: r.on.clone(),
            cpus: r.size.0,
            memory_mb: r.size.1,
            labels: BTreeMap::from([
                (SPACE_LABEL_POOL.to_string(), LABEL_GOLDEN_BUILD.to_string()),
                (LABEL_GOLDEN_BUILD.to_string(), spec.short()),
            ]),
            timeout: Duration::from_secs(20 * 60),
            create_id: format!("cua-ci:golden:{}", spec.short()),
            ..Default::default()
        })
        .await
        .map_err(Error::Spaces)?;
    let id = build.id.clone();
    let out = build_in(r, spec, &id).await;
    // The build VM never outlives the refresh.
    let _ = r.spaces.delete(&id).await;
    let (names, versions) = out?;

    // Verify: a fork answers, reports the pinned versions, and how fast.
    let fork = verify_fork(r, spec, &names[0]).await;
    let (fork_ms, vers) = match fork {
        Ok(v) => v,
        Err(e) => {
            for n in &names {
                let _ = r.spaces.delete_checkpoint(n).await;
            }
            return Err(e);
        }
    };
    let mut versions = versions;
    versions.extend(vers);
    let now = r.clock.now_ms();
    let record = GoldenRecord {
        spec: spec.clone(),
        digest: spec.digest(),
        checkpoints: names,
        built_ms: now,
        verified_ms: Some(now),
        build_ms: now.saturating_sub(t0),
        fork_ready_ms: Some(fork_ms),
        versions,
    };
    let retire = current
        .map(|c| {
            c.checkpoints
                .iter()
                .filter(|n| !in_use(n))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Ok(Refreshed { record, retire })
}

/// The recipe inside the build VM, then the checkpoints.
async fn build_in(
    r: &Refresh,
    spec: &GoldenSpec,
    id: &str,
) -> Result<(Vec<String>, BTreeMap<String, String>)> {
    let g = |e: SpacesError| Error::Spaces(e);
    let probe = r.spaces.sh(id, guest::PROBE, SH_TIMEOUT).await.map_err(g)?;
    let probe = guest::parse_probe(&probe.stdout).map_err(Error::Guest)?;
    let home = probe.home.clone();

    // The hooks and the wrapper first: the recipe's warm-up runs with them.
    for (path, body) in super::boot::baked_files(&home, spec.trim_guest) {
        r.spaces
            .write_file(id, &path, body.as_bytes(), 0o755)
            .await
            .map_err(g)?;
    }
    if !probe.runner_present {
        let platform = guest::platform(&probe.os, &probe.arch).ok_or_else(|| {
            Error::Guest(format!("no actions runner for {} {}", probe.os, probe.arch))
        })?;
        let tarball = r.tarballs.tarball(platform).await?;
        r.spaces
            .upload_file(id, &tarball.path, &guest::tarball_path(&home))
            .await
            .map_err(g)?;
        let o = r
            .spaces
            .sh(id, &guest::install_script(&home), Duration::from_secs(600))
            .await
            .map_err(g)?;
        if !o.ok() {
            return Err(Error::Guest(format!(
                "unpacking the runner failed: {}",
                o.stderr.trim()
            )));
        }
    }
    if spec.trim_guest {
        let o = r
            .spaces
            .sh(id, &super::boot::trim_script(spec.os), SH_TIMEOUT)
            .await
            .map_err(g)?;
        if !o.ok() {
            return Err(Error::Guest(format!(
                "trimming the guest failed: {}",
                o.stderr.trim()
            )));
        }
    }
    let recipe = match spec.os {
        GuestOs::Macos => format!("# cua-ci:recipe\n{}{}", spec.recipe_env(), MACOS_RECIPE),
        // Linux images carry their recipe in the Dockerfile; a Linux
        // checkpoint only needs the runner and the hooks above.
        GuestOs::Linux => "# cua-ci:recipe\ntrue\n".to_string(),
    };
    let o = r.spaces.sh(id, &recipe, RECIPE_TIMEOUT).await.map_err(g)?;
    if !o.ok() {
        return Err(Error::Guest(format!(
            "the golden recipe failed ({:?}): {}",
            o.exit_code,
            o.stderr
                .trim()
                .chars()
                .rev()
                .take(800)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>()
        )));
    }
    let vers = r
        .spaces
        .sh(
            id,
            "cat \"$HOME/.cua-ci-versions\" 2>/dev/null; exit 0",
            SH_TIMEOUT,
        )
        .await
        .map(|o| parse_versions(&o.stdout))
        .unwrap_or_default();
    check_versions(spec, &vers)?;

    // Quiet before the checkpoint: a runner mid-write would be frozen so.
    let _ = r
        .spaces
        .sh(
            id,
            &format!(
                "rm -f {d}/.cua-exit {d}/.cua-pid {d}/.cua-jitconfig; sync; exit 0",
                d = guest::sh_quote(&guest::runner_dir(&home))
            ),
            SH_TIMEOUT,
        )
        .await;
    let mut names = Vec::new();
    for i in 0..spec.variants() {
        let n = spec.checkpoint_name(i);
        r.spaces.checkpoint(id, &n).await.map_err(g)?;
        names.push(n);
    }
    Ok((names, vers))
}

/// The versions the image reports must match the pins of the spec.
pub fn check_versions(spec: &GoldenSpec, got: &BTreeMap<String, String>) -> Result<()> {
    let mut bad = Vec::new();
    if let (Some(want), Some(have)) = (&spec.xcode, got.get("xcode"))
        && want != have
    {
        bad.push(format!("xcode {have} (wanted {want})"));
    }
    for (tool, key) in [("node", "node"), ("pnpm", "pnpm")] {
        if let (Some(want), Some(have)) = (spec.tools.get(tool), got.get(key))
            && !have.starts_with(want.trim_start_matches('v'))
        {
            bad.push(format!("{tool} {have} (wanted {want})"));
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(Error::Guest(format!(
            "the image does not match its spec: {}",
            bad.join(", ")
        )))
    }
}

/// Forks `checkpoint`, runs the fixup and a probe, reads the versions and
/// deletes the fork. Returns the create-to-probe time and the versions.
async fn verify_fork(
    r: &Refresh,
    spec: &GoldenSpec,
    checkpoint: &str,
) -> Result<(u64, BTreeMap<String, String>)> {
    let t0 = r.clock.now_ms();
    let name = format!("cua-ci-golden-verify-{}", &spec.short()[..8]);
    let row = r
        .spaces
        .create(&SpaceSpec {
            name,
            image: format!("checkpoint:{checkpoint}"),
            on: r.on.clone(),
            labels: BTreeMap::from([(
                SPACE_LABEL_POOL.to_string(),
                LABEL_GOLDEN_BUILD.to_string(),
            )]),
            timeout: Duration::from_secs(5 * 60),
            create_id: format!("cua-ci:golden-verify:{}", spec.short()),
            ..Default::default()
        })
        .await
        .map_err(|e| Error::Guest(format!("forking the new golden image failed: {e}")))?;
    let res = async {
        let o = r
            .spaces
            .sh(&row.id, guest::PROBE, SH_TIMEOUT)
            .await
            .map_err(Error::Spaces)?;
        guest::parse_probe(&o.stdout).map_err(Error::Guest)?;
        let ms = r.clock.now_ms().saturating_sub(t0);
        let fix = fork_fixup_script(r.clock.now_ms() / 1000, spec.os);
        r.spaces
            .sh(&row.id, &fix, SH_TIMEOUT)
            .await
            .map_err(Error::Spaces)?;
        let v = r
            .spaces
            .sh(
                &row.id,
                "cat \"$HOME/.cua-ci-versions\" 2>/dev/null; exit 0",
                SH_TIMEOUT,
            )
            .await
            .map(|o| parse_versions(&o.stdout))
            .unwrap_or_default();
        Ok::<_, Error>((ms, v))
    }
    .await;
    let _ = r.spaces.delete(&row.id).await;
    res
}

#[cfg(test)]
mod tests;
