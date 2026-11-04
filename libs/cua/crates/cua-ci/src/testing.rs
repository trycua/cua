//! In-memory fakes of the pool's seams: Spaces (with a scripted guest),
//! GitHub, the tarball cache and the clock. Feature `testing`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use crate::cache::{Tarball, TarballSource};
use crate::config::PoolConfig;
use crate::egress::EgressPolicy;
use crate::github::{DeleteOutcome, JitConfig, JitRequest, Runner, RunnerApi};
use crate::pool::Clock;
use crate::secret::Secret;
use crate::spaces::{ShOutput, SpaceRow, SpaceSpec, Spaces, SpacesError};
use crate::{Error, Result};

/// A clock the test moves.
pub struct ManualClock(AtomicU64);

impl ManualClock {
    /// At `ms`.
    pub fn new(ms: u64) -> Self {
        Self(AtomicU64::new(ms))
    }

    /// Moves it forward.
    pub fn advance(&self, d: Duration) {
        self.0.fetch_add(d.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A pool configuration for tests: `org:acme`, macOS, two runners.
pub fn config(extra: &str) -> PoolConfig {
    PoolConfig::parse(&format!(
        r#"
name = "acme"
scope = "org:acme"
labels = ["cua-ci"]
{extra}

[github]
app_id = 1
secret_service = "cua-ci-github-app"
secret_account = "acme"
"#
    ))
    .expect("test config")
}

/// What the scripted guest's runner is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeRunner {
    /// Not started.
    NotStarted,
    /// The wrapper runs.
    Running,
    /// Exited with this code.
    Exited(i32),
    /// Killed without an exit code.
    Gone,
}

/// One fake Space and its guest.
#[derive(Clone, Debug)]
pub struct FakeSpace {
    /// As listed.
    pub row: SpaceRow,
    /// The spec it was created with.
    pub spec: SpaceSpec,
    /// Guest files (path → (bytes, mode)).
    pub files: BTreeMap<String, (Vec<u8>, u32)>,
    /// The runner process.
    pub runner: FakeRunner,
    /// `~/actions-runner/run.sh` exists.
    pub runner_present: bool,
    /// The guest's resolv.conf nameserver lines.
    pub resolv: String,
    /// The runner's console log.
    pub console: String,
    /// Egress policies set after create.
    pub egress_set: Vec<EgressPolicy>,
    /// Status checks fail (the guest does not answer).
    pub unreachable: bool,
    /// Extra lines of the status script's output (`job_done=1`,
    /// `assigned=...`).
    pub status_extra: String,
    /// What the collect script prints (the marks, `---worker`, step lines).
    pub collect: String,
    /// What the secret scan prints (`found=N`, `detail=...`).
    pub secret_scan: String,
    /// What the delta script prints (`dirty=store:sub` lines).
    pub delta: String,
    /// Files the download of the delta folder produces (name to bytes).
    pub delta_files: BTreeMap<String, Vec<u8>>,
}

/// Calls the fake saw, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    /// `create(name)`.
    Create(String),
    /// `delete(id)`.
    Delete(String),
    /// `sh(id, script)`.
    Sh(String, String),
    /// `write_file(id, path, mode)`.
    Write(String, String, u32),
    /// `upload_file(id, local, path)`.
    Upload(String, PathBuf, String),
    /// `download(id, remote)`.
    Download(String, String),
    /// `spawn(id, program, args, cwd, tag)`.
    Spawn(String, String, Vec<String>, String, String),
    /// `set_egress(id)`.
    SetEgress(String),
}

/// Cua Spaces in memory.
#[derive(Default)]
pub struct FakeSpaces {
    /// The Spaces, by id.
    pub spaces: Mutex<BTreeMap<String, FakeSpace>>,
    /// Spaces that are not the pool's (other macOS VMs, ...).
    pub foreign: Mutex<Vec<SpaceRow>>,
    /// Errors the next creates return, in order.
    pub create_errors: Mutex<VecDeque<SpacesError>>,
    /// Every call.
    pub calls: Mutex<Vec<Call>>,
    /// Images that have the runner baked in.
    pub golden: Mutex<bool>,
    /// The resolv.conf of new Linux guests.
    pub linux_resolv: Mutex<String>,
    /// Refuse macOS VMs past this many on the computer (Apple's two).
    pub macos_limit: Mutex<Option<u32>>,
    /// macOS VMs running on the computer that are not Spaces.
    pub other_vms: Mutex<u32>,
    /// The checkpoints this computer holds.
    pub checkpoints: Mutex<Vec<String>>,
    /// Checkpoints cannot be taken (the error to return).
    pub checkpoint_error: Mutex<Option<String>>,
    /// The backend can share host folders.
    pub mounts: Mutex<bool>,
    /// The recipe script fails with this stderr.
    pub recipe_error: Mutex<Option<String>>,
    /// Deleting a Space fails this many more times.
    pub delete_failures: Mutex<u32>,
}

impl FakeSpaces {
    /// Empty.
    pub fn new() -> Self {
        Self {
            macos_limit: Mutex::new(Some(2)),
            linux_resolv: Mutex::new("nameserver 10.0.2.3\n".into()),
            ..Default::default()
        }
    }

    fn call(&self, c: Call) {
        self.calls.lock().unwrap().push(c);
    }

    /// The pool's Spaces.
    pub fn ids(&self) -> Vec<String> {
        self.spaces.lock().unwrap().keys().cloned().collect()
    }

    /// One Space.
    pub fn get(&self, id: &str) -> FakeSpace {
        self.spaces.lock().unwrap()[id].clone()
    }

    /// Changes one Space's guest.
    pub fn edit(&self, id: &str, f: impl FnOnce(&mut FakeSpace)) {
        f(self.spaces.lock().unwrap().get_mut(id).expect("space"));
    }

    /// The Space disappears (crashed, deleted elsewhere).
    pub fn vanish(&self, id: &str) {
        self.spaces.lock().unwrap().remove(id);
    }

    /// Adds a Space as if created elsewhere (labels included).
    pub fn insert(&self, row: SpaceRow) {
        let s = FakeSpace {
            spec: SpaceSpec::default(),
            files: BTreeMap::new(),
            runner: FakeRunner::Running,
            runner_present: true,
            resolv: String::new(),
            console: String::new(),
            egress_set: vec![],
            unreachable: false,
            status_extra: String::new(),
            collect: String::new(),
            secret_scan: String::new(),
            delta: String::new(),
            delta_files: BTreeMap::new(),
            row: row.clone(),
        };
        self.spaces.lock().unwrap().insert(row.id.clone(), s);
    }

    /// The Spaces created from a checkpoint, newest last.
    pub fn forks(&self) -> Vec<String> {
        self.calls()
            .iter()
            .filter_map(|c| match c {
                Call::Create(n) => Some(n.clone()),
                _ => None,
            })
            .filter(|n| {
                self.spaces
                    .lock()
                    .unwrap()
                    .values()
                    .any(|s| s.row.name == *n && s.spec.image.starts_with("checkpoint:"))
            })
            .collect()
    }

    /// Every call so far.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl Spaces for FakeSpaces {
    async fn create(&self, spec: &SpaceSpec) -> Result<SpaceRow, SpacesError> {
        self.call(Call::Create(spec.name.clone()));
        if let Some(e) = self.create_errors.lock().unwrap().pop_front() {
            return Err(e);
        }
        if let Some(name) = spec.image.strip_prefix("checkpoint:")
            && !self.checkpoints.lock().unwrap().iter().any(|c| c == name)
        {
            return Err(SpacesError::Other(format!("no such checkpoint {name}")));
        }
        // As the real one: a Space started from a checkpoint keeps its saved
        // state, so it takes no egress policy (labels are kept).
        if spec.image.starts_with("checkpoint:") && spec.egress.is_some() {
            return Err(SpacesError::Other(
                "a Space started from checkpoint takes no egress policy".into(),
            ));
        }
        let macos = spec.image.contains("macos");
        if macos && spec.egress.is_some() {
            return Err(SpacesError::EgressUnsupported(
                "An egress policy (macOS VMs share Apple's vmnet NAT, which only pf can filter, \
                 and pf needs root) is not supported for lume Spaces"
                    .into(),
            ));
        }
        if macos && let Some(limit) = *self.macos_limit.lock().unwrap() {
            let ours = self
                .spaces
                .lock()
                .unwrap()
                .values()
                .filter(|s| s.row.is_local_macos_vm())
                .count();
            let other = self
                .foreign
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.is_local_macos_vm())
                .count();
            let vms = *self.other_vms.lock().unwrap() as usize;
            if (ours + other + vms) as u32 >= limit {
                return Err(SpacesError::LimitExceeded(format!(
                    "this Mac already runs {} macOS VMs and allows {limit}",
                    ours + other
                )));
            }
        }
        let row = SpaceRow {
            id: format!("local:{}", spec.name),
            name: spec.name.clone(),
            labels: spec.labels.clone(),
            os: if macos { "macos" } else { "linux" }.into(),
            arch: "arm64".into(),
            provider: "local".into(),
            power_state: "running".into(),
        };
        let s = FakeSpace {
            row: row.clone(),
            spec: spec.clone(),
            files: BTreeMap::new(),
            runner: FakeRunner::NotStarted,
            runner_present: *self.golden.lock().unwrap() || spec.image.starts_with("checkpoint:"),
            resolv: if macos {
                "nameserver 192.168.64.1\n".into()
            } else {
                self.linux_resolv.lock().unwrap().clone()
            },
            console: String::new(),
            egress_set: vec![],
            unreachable: false,
            status_extra: String::new(),
            collect: String::new(),
            secret_scan: String::new(),
            delta: String::new(),
            delta_files: BTreeMap::new(),
        };
        self.spaces.lock().unwrap().insert(row.id.clone(), s);
        Ok(row)
    }

    async fn delete(&self, id: &str) -> Result<(), SpacesError> {
        self.call(Call::Delete(id.into()));
        {
            let mut f = self.delete_failures.lock().unwrap();
            if *f > 0 {
                *f -= 1;
                return Err(SpacesError::Other("the runtime is busy".into()));
            }
        }
        match self.spaces.lock().unwrap().remove(id) {
            Some(_) => Ok(()),
            None => Err(SpacesError::NotFound(id.into())),
        }
    }

    async fn list(&self) -> Result<Vec<SpaceRow>, SpacesError> {
        let mut out: Vec<SpaceRow> = self
            .spaces
            .lock()
            .unwrap()
            .values()
            .map(|s| s.row.clone())
            .collect();
        out.extend(self.foreign.lock().unwrap().iter().cloned());
        Ok(out)
    }

    async fn sh(&self, id: &str, script: &str, _t: Duration) -> Result<ShOutput, SpacesError> {
        self.call(Call::Sh(id.into(), script.into()));
        let mut spaces = self.spaces.lock().unwrap();
        let s = spaces
            .get_mut(id)
            .ok_or_else(|| SpacesError::NotFound(id.into()))?;
        let ok = |stdout: String| ShOutput {
            stdout,
            stderr: String::new(),
            exit_code: Some(0),
        };
        let tag = script.lines().next().unwrap_or_default();
        let home = "/Users/lume";
        Ok(match tag {
            "# cua-ci:probe" => {
                let os = if s.row.os == "macos" {
                    "Darwin"
                } else {
                    "Linux"
                };
                let resolv: String = s.resolv.lines().map(|l| format!("resolv={l}\n")).collect();
                ok(format!(
                    "home={home}\nos={os}\narch=arm64\nrunner={}\n{resolv}",
                    if s.runner_present {
                        "present"
                    } else {
                        "missing"
                    }
                ))
            }
            "# cua-ci:install" => {
                let tgz = crate::guest::tarball_path(home);
                if s.files.remove(&tgz).is_none() {
                    return Ok(ShOutput {
                        stdout: String::new(),
                        stderr: "tar: no such file".into(),
                        exit_code: Some(2),
                    });
                }
                s.runner_present = true;
                ok(String::new())
            }
            "# cua-ci:prepare" => ok(String::new()),
            "# cua-ci:status" => {
                if s.unreachable {
                    return Err(SpacesError::Other("the guest does not answer".into()));
                }
                let mut out = match s.runner {
                    FakeRunner::NotStarted => "starting".to_string(),
                    FakeRunner::Running => "running".to_string(),
                    FakeRunner::Exited(c) => format!("exit={c}"),
                    FakeRunner::Gone => "gone".to_string(),
                };
                out.push('\n');
                out.push_str(&s.status_extra);
                ok(out)
            }
            "# cua-ci:collect" => ok(s.collect.clone()),
            "# cua-ci:secret-scan" => ok(s.secret_scan.clone()),
            "# cua-ci:delta" => ok(s.delta.clone()),
            "# cua-ci:recipe" => {
                if let Some(e) = self.recipe_error.lock().unwrap().clone() {
                    return Ok(ShOutput {
                        stdout: String::new(),
                        stderr: e,
                        exit_code: Some(70),
                    });
                }
                ok(String::new())
            }
            _ => ok(String::new()),
        })
    }

    async fn write_file(
        &self,
        id: &str,
        path: &str,
        data: &[u8],
        mode: u32,
    ) -> Result<(), SpacesError> {
        self.call(Call::Write(id.into(), path.into(), mode));
        let mut spaces = self.spaces.lock().unwrap();
        let s = spaces
            .get_mut(id)
            .ok_or_else(|| SpacesError::NotFound(id.into()))?;
        s.files.insert(path.into(), (data.to_vec(), mode));
        Ok(())
    }

    async fn upload_file(&self, id: &str, local: &Path, path: &str) -> Result<(), SpacesError> {
        self.call(Call::Upload(id.into(), local.to_path_buf(), path.into()));
        let data = std::fs::read(local).map_err(|e| SpacesError::Other(e.to_string()))?;
        let mut spaces = self.spaces.lock().unwrap();
        let s = spaces
            .get_mut(id)
            .ok_or_else(|| SpacesError::NotFound(id.into()))?;
        s.files.insert(path.into(), (data, 0o644));
        Ok(())
    }

    async fn download(&self, id: &str, remote: &str, local_dir: &Path) -> Result<(), SpacesError> {
        self.call(Call::Download(id.into(), remote.into()));
        let console = {
            let spaces = self.spaces.lock().unwrap();
            let s = spaces
                .get(id)
                .ok_or_else(|| SpacesError::NotFound(id.into()))?;
            s.console.clone()
        };
        let io = |e: std::io::Error| SpacesError::Other(e.to_string());
        if remote.ends_with("/cua-run.log") {
            std::fs::write(local_dir.join("cua-run.log"), console).map_err(io)?;
        } else if remote.ends_with("/delta") {
            let files = {
                let spaces = self.spaces.lock().unwrap();
                spaces[id].delta_files.clone()
            };
            std::fs::create_dir_all(local_dir.join("delta")).map_err(io)?;
            for (n, b) in files {
                std::fs::write(local_dir.join("delta").join(n), b).map_err(io)?;
            }
        } else if remote.ends_with("/_diag") {
            std::fs::create_dir_all(local_dir.join("_diag")).map_err(io)?;
            std::fs::write(local_dir.join("_diag/Runner_1.log"), "diag").map_err(io)?;
        }
        Ok(())
    }

    async fn spawn(
        &self,
        id: &str,
        program: &str,
        args: &[String],
        cwd: &str,
        tag: &str,
    ) -> Result<(), SpacesError> {
        self.call(Call::Spawn(
            id.into(),
            program.into(),
            args.to_vec(),
            cwd.into(),
            tag.into(),
        ));
        let mut spaces = self.spaces.lock().unwrap();
        let s = spaces
            .get_mut(id)
            .ok_or_else(|| SpacesError::NotFound(id.into()))?;
        // The wrapper refuses to start without its configuration.
        let has_cfg = s.files.contains_key(&crate::guest::jit_path("/Users/lume"));
        s.runner = if has_cfg {
            FakeRunner::Running
        } else {
            FakeRunner::Exited(97)
        };
        Ok(())
    }

    fn supports_mounts(&self) -> bool {
        *self.mounts.lock().unwrap()
    }

    async fn checkpoint(&self, id: &str, name: &str) -> Result<(), SpacesError> {
        if let Some(e) = self.checkpoint_error.lock().unwrap().clone() {
            return Err(SpacesError::Other(e));
        }
        if !self.spaces.lock().unwrap().contains_key(id) {
            return Err(SpacesError::NotFound(id.into()));
        }
        self.checkpoints.lock().unwrap().push(name.into());
        Ok(())
    }

    async fn checkpoints(&self) -> Result<Vec<String>, SpacesError> {
        Ok(self.checkpoints.lock().unwrap().clone())
    }

    async fn delete_checkpoint(&self, name: &str) -> Result<(), SpacesError> {
        self.checkpoints.lock().unwrap().retain(|c| c != name);
        Ok(())
    }

    async fn macos_vms_running(&self) -> Option<u32> {
        let spaces = self
            .spaces
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.row.is_local_macos_vm())
            .count()
            + self
                .foreign
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.is_local_macos_vm())
                .count();
        Some(spaces as u32 + *self.other_vms.lock().unwrap())
    }

    async fn set_egress(&self, id: &str, policy: &EgressPolicy) -> Result<(), SpacesError> {
        self.call(Call::SetEgress(id.into()));
        let mut spaces = self.spaces.lock().unwrap();
        let s = spaces
            .get_mut(id)
            .ok_or_else(|| SpacesError::NotFound(id.into()))?;
        s.egress_set.push(policy.clone());
        Ok(())
    }
}

/// GitHub in memory.
#[derive(Default)]
pub struct FakeGitHub {
    /// Registered runners.
    pub runners: Mutex<BTreeMap<u64, Runner>>,
    /// Every JIT request.
    pub jit_requests: Mutex<Vec<JitRequest>>,
    /// Every delete.
    pub deletes: Mutex<Vec<u64>>,
    /// JIT requests that fail next.
    pub jit_failures: Mutex<u32>,
    /// Runner groups by name.
    pub groups: Mutex<HashMap<String, u64>>,
    next: AtomicU64,
}

impl FakeGitHub {
    /// Empty.
    pub fn new() -> Self {
        Self {
            next: AtomicU64::new(100),
            ..Default::default()
        }
    }

    /// The JIT configuration GitHub handed out for runner `id`.
    pub fn jit_secret(id: u64) -> String {
        format!("JITCONFIG-SECRET-{id}")
    }

    /// Changes a runner.
    pub fn edit(&self, id: u64, f: impl FnOnce(&mut Runner)) {
        if let Some(r) = self.runners.lock().unwrap().get_mut(&id) {
            f(r);
        }
    }

    /// GitHub removes an ephemeral runner after its job.
    pub fn forget(&self, id: u64) {
        self.runners.lock().unwrap().remove(&id);
    }

    /// Adds a runner registered elsewhere.
    pub fn add(&self, r: Runner) {
        self.runners.lock().unwrap().insert(r.id, r);
    }

    /// Runner ids.
    pub fn ids(&self) -> Vec<u64> {
        self.runners.lock().unwrap().keys().copied().collect()
    }
}

#[async_trait]
impl RunnerApi for FakeGitHub {
    async fn runner_group_id(&self, name: Option<&str>) -> Result<u64> {
        match name {
            None => Ok(1),
            Some(n) => self
                .groups
                .lock()
                .unwrap()
                .get(n)
                .copied()
                .ok_or_else(|| Error::GitHub(format!("no runner group named {n:?}"))),
        }
    }

    async fn generate_jit(&self, req: &JitRequest) -> Result<JitConfig> {
        {
            let mut f = self.jit_failures.lock().unwrap();
            if *f > 0 {
                *f -= 1;
                return Err(Error::GitHub("POST generate-jitconfig: 500".into()));
            }
        }
        self.jit_requests.lock().unwrap().push(req.clone());
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        self.runners.lock().unwrap().insert(
            id,
            Runner {
                id,
                name: req.name.clone(),
                status: "offline".into(),
                busy: false,
                labels: req.labels.clone(),
            },
        );
        Ok(JitConfig {
            runner_id: id,
            runner_name: req.name.clone(),
            encoded: Secret::new(Self::jit_secret(id)),
        })
    }

    async fn list_runners(&self) -> Result<Vec<Runner>> {
        Ok(self.runners.lock().unwrap().values().cloned().collect())
    }

    async fn delete_runner(&self, id: u64) -> Result<DeleteOutcome> {
        self.deletes.lock().unwrap().push(id);
        let mut rs = self.runners.lock().unwrap();
        match rs.get(&id) {
            None => Ok(DeleteOutcome::NotFound),
            Some(r) if r.busy => Ok(DeleteOutcome::Busy),
            Some(_) => {
                rs.remove(&id);
                Ok(DeleteOutcome::Deleted)
            }
        }
    }
}

/// A tarball cache with one fake tarball.
pub struct FakeTarballs {
    path: PathBuf,
    /// Platforms asked for.
    pub asked: Mutex<Vec<String>>,
}

impl FakeTarballs {
    /// Writes the fake tarball under `dir`.
    pub fn new(dir: &Path) -> Self {
        let path = dir.join("actions-runner-fake.tar.gz");
        std::fs::write(&path, b"fake tarball").expect("tarball");
        Self {
            path,
            asked: Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl TarballSource for FakeTarballs {
    async fn tarball(&self, platform: &str) -> Result<Tarball> {
        self.asked.lock().unwrap().push(platform.into());
        Ok(Tarball {
            path: self.path.clone(),
            version: "2.338.2".into(),
            sha256: "0".repeat(64),
        })
    }
}
