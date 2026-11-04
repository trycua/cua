//! Memory checkpoints of gVisor containers: restore, fork and warm starts.
//!
//! Docker's own checkpoint API cannot restore a gVisor container (Docker 29
//! sets up the network namespace from the task's pid, which is 0 for a
//! restore, and refuses a checkpoint directory outside the container), so cua
//! does it with runsc itself:
//!
//! * **The runtime.** The engine registers `cua-runsc` ([`WRAPPER_RUNTIME`],
//!   [`WRAPPER_PATH`]), a small shell wrapper around runsc
//!   (`cua-runsc.sh`). A container created with the OCI annotation
//!   [`ANNOTATION_RESTORE`] starts by `runsc restore` into its freshly created
//!   sandbox instead of booting: Docker gives it a network namespace of its
//!   own first, and gVisor's netstack takes that namespace's address on
//!   restore, so a clone has its own IP. Everything else is plain runsc.
//! * **The helper.** Checkpoints are taken by `runsc checkpoint
//!   --leave-running` as root in the engine's Linux, through a small
//!   privileged runc container ([`HELPER_NAME`], an empty image plus the
//!   host's `/usr` and `/`); `docker exec` into it runs the wrapper's `cua`
//!   control commands. The checkpoint (memory, and the root filesystem
//!   writes gVisor keeps in its overlay) lives in the engine
//!   (`/var/lib/docker/cua-runsc/<home>/`); records of what it is live in
//!   this cua home (`<cua home>/runsc/`, owner-only: they hold tokens).
//! * **Identity.** A restored container is the source's memory: its
//!   cua-spacesd still accepts the source's token. Right after the restore
//!   the runtime calls spacesd `Init` with that token, installing the new
//!   Space's own token with `new_machine` (a fresh machine-seal key and
//!   machine-id, entropy mixed in, the clock stepped), and writes a fresh
//!   `/etc/machine-id` and the container's hostname itself.
//! * **Warm starts.** The first start of a configuration (image, size,
//!   ports, environment) boots cold, and a background *golden* container of
//!   the same configuration, holding a throwaway token, is booted to
//!   readiness and checkpointed. Later starts of that configuration restore
//!   it (golden secret rotated to the Space's token at once). Up to
//!   [`MAX_GOLDEN`] are kept.
//!
//! On arm64, gVisor (systrap) does not yet carry pointer-authentication
//! keys across a restore (gvisor#13542, fix pending upstream): every
//! PAC-built binary (all of Ubuntu's) dies with SIGILL after any restore.
//! Memory checkpoints are therefore only used where the engine's kernel does
//! not expose PAC to user space (x86, or arm64 booted with
//! `arm64.nopauth`); elsewhere checkpoints stay filesystem-only and starts
//! cold, and [`Support`] says why.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{CreateContainerOptions, CreateImageOptions};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{LABEL_CREATED, LABEL_HOME, LABEL_KIND, LABEL_MANAGED, engine_err, is_not_found};
use crate::error::{Result, VmmError};
use crate::host;
use crate::types::StartSpec;

/// The engine's runtime name of the wrapper.
pub const WRAPPER_RUNTIME: &str = "cua-runsc";
/// Where the wrapper is installed in the engine's Linux.
pub const WRAPPER_PATH: &str = "/usr/local/lib/cua/cua-runsc";
/// Bumped with every change to `cua-runsc.sh`: an engine with another
/// version gets this one installed.
pub const WRAPPER_VERSION: &str = "1";
/// The OCI annotation naming the checkpoint a container restores on start.
pub const ANNOTATION_RESTORE: &str = "ai.cua.restore";
/// The privileged helper container (one per engine).
pub const HELPER_NAME: &str = "cua-runsc-helper";
/// Its image: empty but for the directories and symlinks the host's `/usr`
/// needs (imported, never pulled).
pub const HELPER_IMAGE: &str = "cua-vmm/helper:1";
/// Golden checkpoints kept per cua home (oldest removed first).
pub const MAX_GOLDEN: usize = 4;
/// Turns warm starts off (`0`).
pub const WARM_ENV: &str = "CUA_GVISOR_WARM";
/// The environment keys that carry a Space's spacesd token.
pub const TOKEN_KEYS: [&str; 2] = ["CUA_ENV_TOKEN", "CUA_SPACESD_TOKEN"];
/// cua-spacesd's port.
pub const SPACESD_PORT: u16 = 3211;

/// The wrapper script for this release.
pub fn wrapper_script() -> String {
    render_wrapper(
        "/usr/local/bin/runsc",
        "/run/cua-runsc",
        "/var/lib/docker/cua-runsc",
    )
}

/// The wrapper with its paths: the real runsc, its per-container state
/// directory and the checkpoint directory.
fn render_wrapper(runsc: &str, state: &str, store: &str) -> String {
    include_str!("cua-runsc.sh")
        .replace("@VERSION@", WRAPPER_VERSION)
        .replace("@RUNSC@", runsc)
        .replace("@STATE@", state)
        .replace("@STORE@", store)
}

/// What an engine supports, from the wrapper's `info`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Support {
    /// Memory checkpoints work: `cua-runsc` is registered and installed,
    /// and restored processes survive.
    pub memory: Result<(), String>,
    /// The engine's checkpoint directory.
    pub store: String,
    /// `runsc --version`'s first line (part of a golden key).
    pub runsc: String,
}

impl Support {
    pub(crate) fn none(why: impl Into<String>) -> Self {
        Self {
            memory: Err(why.into()),
            store: String::new(),
            runsc: String::new(),
        }
    }

    pub fn ok(&self) -> bool {
        self.memory.is_ok()
    }
}

/// Parses the wrapper's `info` output into a [`Support`].
pub fn parse_info(text: &str) -> Support {
    let kv: BTreeMap<&str, &str> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    let get = |k: &str| kv.get(k).copied().unwrap_or_default().to_string();
    let memory = if kv.get("version").copied() != Some(WRAPPER_VERSION) {
        Err(format!(
            "cua-runsc {} is installed, {WRAPPER_VERSION} is needed",
            get("version")
        ))
    } else if get("store").is_empty() {
        Err("cua-runsc reports no checkpoint directory".into())
    } else if get("pac") == "1" {
        Err(format!(
            "this {} engine exposes pointer authentication, which gVisor does not carry \
             across a restore yet (gvisor#13542); boot its kernel with arm64.nopauth",
            get("arch")
        ))
    } else {
        Ok(())
    };
    Support {
        memory,
        store: get("store"),
        runsc: get("runsc"),
    }
}

/// A checkpoint's record in this cua home: enough to create a container that
/// restores it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapRecord {
    pub name: String,
    /// Its directory in the engine.
    pub dir: String,
    /// The engine it lives in (endpoint URI).
    pub engine: String,
    /// The image ID the source ran (the restore's lower filesystem).
    pub image_id: String,
    /// The source's container environment (`KEY=value`), token included:
    /// the restored spacesd accepts that token until it is rotated.
    pub env: Vec<String>,
    #[serde(default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(default)]
    pub cmd: Option<Vec<String>>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    /// Published guest ports.
    #[serde(default)]
    pub ports: Vec<u16>,
    pub cpus: u32,
    pub memory_mb: u64,
    /// Unix seconds.
    pub created: u64,
}

impl SnapRecord {
    /// The token the restored spacesd accepts.
    pub fn token(&self) -> Option<String> {
        token_of(&self.env)
    }
}

/// The spacesd token in a `KEY=value` environment.
pub fn token_of(env: &[String]) -> Option<String> {
    env.iter().find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (TOKEN_KEYS.contains(&k) && !v.is_empty()).then(|| v.to_string())
    })
}

/// `env` (`KEY=value`) with `over`'s keys replaced or added.
pub fn merge_env(env: &[String], over: &BTreeMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = env
        .iter()
        .filter(|kv| {
            kv.split_once('=')
                .is_none_or(|(k, _)| !over.contains_key(k))
        })
        .cloned()
        .collect();
    out.extend(over.iter().map(|(k, v)| format!("{k}={v}")));
    out
}

/// Host-side records: `<cua home>/runsc/{checkpoints,golden}/<name>.json`.
pub fn records_dir(kind: &str) -> PathBuf {
    host::cua_home().join("runsc").join(kind)
}

pub fn read_record(kind: &str, name: &str) -> Option<SnapRecord> {
    let p = records_dir(kind).join(format!("{name}.json"));
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

pub fn write_record(kind: &str, rec: &SnapRecord) -> Result<()> {
    let d = records_dir(kind);
    std::fs::create_dir_all(&d)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            host::cua_home().join("runsc"),
            std::fs::Permissions::from_mode(0o700),
        );
    }
    let p = d.join(format!("{}.json", rec.name));
    let tmp = d.join(format!(".{}.json.tmp", rec.name));
    {
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(rec).map_err(|e| VmmError::other(e.to_string()))?)?;
    }
    std::fs::rename(tmp, p)?;
    Ok(())
}

pub fn remove_record(kind: &str, name: &str) {
    let _ = std::fs::remove_file(records_dir(kind).join(format!("{name}.json")));
}

/// Every record of `kind`, oldest first.
pub fn records(kind: &str) -> Vec<SnapRecord> {
    let Ok(rd) = std::fs::read_dir(records_dir(kind)) else {
        return vec![];
    };
    let mut v: Vec<SnapRecord> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    v.sort_by_key(|r| r.created);
    v
}

/// What a golden checkpoint is keyed by: everything that shapes a Space
/// before it has an identity. `env` excludes the token.
#[allow(clippy::too_many_arguments)]
pub fn golden_key(
    image_id: &str,
    cpus: u32,
    memory_mb: u64,
    ports: &[u16],
    probes: &[u16],
    env: &BTreeMap<String, String>,
    command: Option<&Vec<String>>,
    runsc: &str,
) -> String {
    let mut ports = ports.to_vec();
    ports.sort_unstable();
    let mut probes = probes.to_vec();
    probes.sort_unstable();
    let env: BTreeMap<&String, &String> = env
        .iter()
        .filter(|(k, _)| !TOKEN_KEYS.contains(&k.as_str()))
        .collect();
    let doc = serde_json::json!({
        "v": 1,
        "wrapper": WRAPPER_VERSION,
        "image": image_id,
        "cpus": cpus,
        "memory_mb": memory_mb,
        "ports": ports,
        "probes": probes,
        "env": env,
        "command": command,
        "runsc": runsc,
    });
    let h = Sha256::digest(doc.to_string().as_bytes());
    h.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// `n` random bytes from the OS.
pub fn random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut b = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom")
        && f.read_exact(&mut b).is_ok()
    {
        return b;
    }
    // No /dev/urandom (Windows): hash the clock and a counter.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let c = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        out.extend_from_slice(&Sha256::digest(format!("{t}:{c}:{}", std::process::id())));
    }
    out.truncate(n);
    out
}

pub fn random_hex(n: usize) -> String {
    random_bytes(n).iter().map(|x| format!("{x:02x}")).collect()
}

/// The helper image: the directories a container needs and symlinks into
/// the host's `/usr`, which the helper mounts (Ubuntu's merged-/usr
/// layout: `bin`, `sbin`, `lib`, `lib64`).
fn helper_tar() -> Result<Vec<u8>> {
    let mut b = tar::Builder::new(Vec::new());
    for d in ["usr", "host", "proc", "sys", "dev", "etc", "tmp", "run"] {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Directory);
        h.set_mode(0o755);
        h.set_size(0);
        b.append_data(&mut h, format!("{d}/"), std::io::empty())?;
    }
    for (l, t) in [
        ("bin", "usr/bin"),
        ("sbin", "usr/sbin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib64"),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_mode(0o777);
        h.set_size(0);
        b.append_link(&mut h, l, t)?;
    }
    Ok(b.into_inner()?)
}

/// Output of a helper command.
#[derive(Debug)]
pub struct HelperOut {
    pub code: i64,
    pub stdout: String,
    pub stderr: String,
}

/// The privileged helper of one engine.
#[derive(Clone)]
pub struct Helper {
    docker: Docker,
}

impl Helper {
    pub fn new(docker: Docker) -> Self {
        Self { docker }
    }

    /// Imports the helper image when it is missing.
    async fn ensure_image(&self) -> Result<()> {
        if self.docker.inspect_image(HELPER_IMAGE).await.is_ok() {
            return Ok(());
        }
        let (repo, tag) = HELPER_IMAGE
            .split_once(':')
            .unwrap_or((HELPER_IMAGE, "latest"));
        let opts = CreateImageOptions {
            from_src: Some("-".into()),
            repo: Some(repo.into()),
            tag: Some(tag.into()),
            changes: vec![format!(
                "LABEL {LABEL_MANAGED}=true {LABEL_KIND}=helper {LABEL_HOME}={}",
                super::home_tag()
            )],
            ..Default::default()
        };
        let body = bollard::body_full(helper_tar()?.into());
        let mut s = self.docker.create_image(Some(opts), Some(body), None);
        while let Some(item) = s.next().await {
            item.map_err(|e| VmmError::Engine(format!("import {HELPER_IMAGE}: {e}")))?;
        }
        Ok(())
    }

    /// Creates and starts the helper container when it is not running.
    pub async fn ensure(&self) -> Result<()> {
        match self.docker.inspect_container(HELPER_NAME, None).await {
            Ok(i) if i.state.as_ref().and_then(|s| s.running) == Some(true) => return Ok(()),
            Ok(_) => {
                if self.docker.start_container(HELPER_NAME, None).await.is_ok() {
                    return Ok(());
                }
                let _ = self
                    .docker
                    .remove_container(
                        HELPER_NAME,
                        Some(bollard::query_parameters::RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await;
            }
            Err(e) if is_not_found(&e) => {}
            Err(e) => return Err(engine_err(e)),
        }
        self.ensure_image().await?;
        let mut labels = std::collections::HashMap::new();
        labels.insert(LABEL_MANAGED.to_string(), "true".to_string());
        labels.insert(LABEL_KIND.to_string(), "helper".to_string());
        labels.insert(LABEL_CREATED.to_string(), host::now_secs().to_string());
        labels.insert(LABEL_HOME.to_string(), super::home_tag());
        let body = ContainerCreateBody {
            image: Some(HELPER_IMAGE.into()),
            entrypoint: Some(vec!["/usr/bin/sleep".into(), "infinity".into()]),
            labels: Some(labels),
            host_config: Some(HostConfig {
                runtime: Some("runc".into()),
                privileged: Some(true),
                pid_mode: Some("host".into()),
                network_mode: Some("none".into()),
                binds: Some(vec!["/usr:/usr:ro".into(), "/:/host".into()]),
                // A ceiling for runsc's client; the checkpoint itself is
                // written by the Space's own sandbox.
                memory: Some(256 * 1024 * 1024),
                ..Default::default()
            }),
            ..Default::default()
        };
        match self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(HELPER_NAME.into()),
                    platform: String::new(),
                }),
                body,
            )
            .await
        {
            Ok(_) => {}
            // Another create made it first.
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => {}
            Err(e) => return Err(engine_err(e)),
        }
        match self.docker.start_container(HELPER_NAME, None).await {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 304, ..
            }) => Ok(()),
            Err(e) => Err(engine_err(e)),
        }
    }

    /// Runs `argv` as root in the engine's own filesystem (`chroot /host`).
    pub async fn run(&self, argv: &[&str], timeout: Duration) -> Result<HelperOut> {
        let mut cmd: Vec<String> = vec!["/usr/sbin/chroot".into(), "/host".into()];
        cmd.extend(argv.iter().map(|s| s.to_string()));
        let fut = async {
            let created = self
                .docker
                .create_exec(
                    HELPER_NAME,
                    CreateExecOptions::<String> {
                        cmd: Some(cmd),
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        ..Default::default()
                    },
                )
                .await
                .map_err(engine_err)?;
            let started = self
                .docker
                .start_exec(&created.id, None::<StartExecOptions>)
                .await
                .map_err(engine_err)?;
            let (mut out, mut err) = (Vec::new(), Vec::new());
            if let StartExecResults::Attached { mut output, .. } = started {
                while let Some(msg) = output.next().await {
                    match msg.map_err(engine_err)? {
                        bollard::container::LogOutput::StdOut { message } => {
                            out.extend_from_slice(&message)
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            err.extend_from_slice(&message)
                        }
                        _ => {}
                    }
                }
            }
            let code = self
                .docker
                .inspect_exec(&created.id)
                .await
                .map_err(engine_err)?
                .exit_code
                .unwrap_or(-1);
            Ok(HelperOut {
                code,
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            })
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| VmmError::other(format!("engine helper {argv:?} timed out")))?
    }

    /// [`Self::run`], failing on a non-zero exit.
    pub async fn run_ok(&self, argv: &[&str], timeout: Duration) -> Result<String> {
        let o = self.run(argv, timeout).await?;
        if o.code != 0 {
            return Err(VmmError::other(format!(
                "{} exited {}: {}",
                argv.first().copied().unwrap_or_default(),
                o.code,
                o.stderr.trim()
            )));
        }
        Ok(o.stdout)
    }

    /// Installs the wrapper (when its version differs) and reads what the
    /// engine supports.
    pub async fn probe(&self) -> Result<Support> {
        self.ensure().await?;
        let info = self
            .run(&[WRAPPER_PATH, "cua", "info"], Duration::from_secs(20))
            .await?;
        let current = (info.code == 0).then(|| parse_info(&info.stdout));
        if let Some(s) =
            current.filter(|s| !s.memory.as_ref().is_err_and(|e| e.contains("is needed")))
        {
            return Ok(s);
        }
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(wrapper_script());
        let dir = WRAPPER_PATH
            .rsplit_once('/')
            .map(|(d, _)| d)
            .unwrap_or("/usr/local/lib/cua");
        let script = format!(
            "set -e; mkdir -p {dir}; printf %s \"$1\" | base64 -d > {WRAPPER_PATH}.new; \
             chmod 0755 {WRAPPER_PATH}.new; mv {WRAPPER_PATH}.new {WRAPPER_PATH}"
        );
        self.run_ok(
            &["/bin/sh", "-c", &script, "sh", &b64],
            Duration::from_secs(20),
        )
        .await?;
        tracing::info!(
            version = WRAPPER_VERSION,
            "installed cua-runsc in the engine"
        );
        let info = self
            .run_ok(&[WRAPPER_PATH, "cua", "info"], Duration::from_secs(20))
            .await?;
        Ok(parse_info(&info))
    }

    /// Checkpoints running container `id` into `dir` (it keeps running).
    pub async fn checkpoint(&self, id: &str, dir: &str) -> Result<()> {
        self.run_ok(
            &[WRAPPER_PATH, "cua", "checkpoint", id, dir],
            Duration::from_secs(600),
        )
        .await
        .map(drop)
    }

    /// Forgets deleted containers (the record that they were restored).
    pub async fn forget(&self, ids: &[&str]) -> Result<()> {
        let mut argv = vec![WRAPPER_PATH, "cua", "forget"];
        argv.extend_from_slice(ids);
        self.run_ok(&argv, Duration::from_secs(30)).await.map(drop)
    }

    /// Deletes checkpoint directories.
    pub async fn remove(&self, dirs: &[&str]) -> Result<()> {
        let mut argv = vec![WRAPPER_PATH, "cua", "rm"];
        argv.extend_from_slice(dirs);
        self.run_ok(&argv, Duration::from_secs(120)).await.map(drop)
    }
}

/// Gives a just-restored container its own identity: a fresh
/// `/etc/machine-id`, its own hostname, and cua-spacesd's `Init` with
/// `new_machine` (the Space's token installed in place of `auth`, entropy,
/// the clock). `token` is `None` for an image without cua-spacesd.
pub async fn give_identity(
    docker: &Docker,
    container: &str,
    spacesd: Option<String>,
    auth: Option<String>,
    token: Option<String>,
) -> Result<()> {
    // Machine-id and hostname, as root, best effort (an image may have no
    // shell); the restore keeps the source's.
    let script = "h=$(cat /etc/hostname 2>/dev/null) && [ -n \"$h\" ] && hostname \"$h\" 2>/dev/null; \
                  for f in /etc/machine-id /var/lib/dbus/machine-id; do \
                  [ -f \"$f\" ] && [ ! -L \"$f\" ] && printf '%s\\n' \"$1\" > \"$f\"; done; true";
    let id = random_hex(16);
    let exec = super::DockerExec {
        docker: docker.clone(),
        container: container.to_string(),
    };
    let _ = crate::exec::GuestExec::exec(
        &exec,
        crate::exec::ExecRequest {
            script: format!("set -- {id}; {script}"),
            user: Some("0".into()),
            timeout: Some(Duration::from_secs(10)),
            ..Default::default()
        },
    )
    .await;
    let (Some(url), Some(auth), Some(token)) = (spacesd, auth, token) else {
        return Ok(());
    };
    use cua_spacesd_client::pb;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let req = pb::InitRequest {
        token: if token == auth { String::new() } else { token },
        now: Some(pbjson_types::Timestamp {
            seconds: now.as_secs() as i64,
            nanos: now.subsec_nanos() as i32,
        }),
        new_machine: true,
        entropy: random_bytes(32),
        labels: [("sandbox".to_string(), container.to_string())].into(),
        ..Default::default()
    };
    // The restored spacesd is serving already; a few tries cover the moment
    // the restored netstack takes its new address.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        match cua_spacesd_client::SpacesdClient::connect_url(&url, Some(auth.clone())).await {
            Ok(c) => match c.init(req.clone()).await {
                Ok(_) => return Ok(()),
                Err(e) => last = e.to_string(),
            },
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(VmmError::other(format!(
        "the restored Space's cua-spacesd did not take its identity: {last}"
    )))
}

/// Golden checkpoints being recorded in this process (by key).
static BUILDING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Configurations whose image's cua-spacesd cannot become a new machine
/// ([`crate::identity_gate`]): they are never recorded as a golden in this
/// process, so their Spaces start cold, each with its own secrets.
static NO_GOLDEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// How long a golden container settles after it is ready, before it is
/// checkpointed (the desktop finishes drawing; a restore resumes from there).
const GOLDEN_SETTLE: Duration = Duration::from_secs(2);

/// A start that a golden checkpoint serves, or records.
#[derive(Clone, Debug)]
pub struct GoldenPlan {
    pub key: String,
    pub image_id: String,
    /// The guest port whose listener marks readiness.
    pub ready_port: u16,
    /// The recorded checkpoint, when there is one.
    pub record: Option<SnapRecord>,
}

impl SnapRecord {
    /// A [`StartSpec`] that creates a container like the record's source
    /// (its environment, command, ports and size).
    pub fn start_spec(&self, name: &str) -> StartSpec {
        let mut spec = StartSpec::new(name, crate::ImageSource::Existing)
            .cpus(self.cpus.max(1))
            .memory_mb(self.memory_mb.max(256));
        spec.env = self
            .env
            .iter()
            .filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect();
        let mut argv: Vec<String> = self.entrypoint.clone().unwrap_or_default();
        argv.extend(self.cmd.clone().unwrap_or_default());
        spec.command = (!argv.is_empty()).then_some(argv);
        spec.ports = self.ports.clone();
        spec
    }
}

impl super::ContainerRuntime {
    /// Whether this engine takes memory checkpoints (probed once; installs
    /// or updates the wrapper when the engine registers it).
    pub async fn support(&self) -> &Support {
        self.snap
            .get_or_init(|| async {
                match self.runtimes().await {
                    Ok(r) if r.iter().any(|x| x == WRAPPER_RUNTIME) => {}
                    Ok(_) => {
                        return Support::none(format!(
                            "the engine has no {WRAPPER_RUNTIME} runtime registered"
                        ));
                    }
                    Err(e) => return Support::none(e.to_string()),
                }
                match self.helper().probe().await {
                    Ok(s) => {
                        if let Err(why) = &s.memory {
                            tracing::info!(why, "gVisor memory checkpoints are off on this engine");
                        }
                        s
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "the engine's cua-runsc helper failed; gVisor memory checkpoints are off");
                        Support::none(e.to_string())
                    }
                }
            })
            .await
    }

    /// The engine's privileged helper ([`Helper`]).
    pub fn helper(&self) -> Helper {
        Helper::new(self.docker.clone())
    }

    /// The engine directory of checkpoint `what` (`ckpt-<name>`,
    /// `fork-<name>`, `golden-<key>`) of this cua home.
    pub(crate) fn snap_dir(sup: &Support, what: &str) -> String {
        format!("{}/{}/{what}", sup.store, super::home_tag())
    }

    /// Whether `i` runs under the wrapper (and so can be checkpointed with
    /// memory).
    pub(crate) fn under_wrapper(i: &bollard::models::ContainerInspectResponse) -> bool {
        i.host_config.as_ref().and_then(|h| h.runtime.as_deref()) == Some(WRAPPER_RUNTIME)
    }

    /// Checkpoints running container `i` (memory and root overlay) into the
    /// engine directory `what` and describes it.
    pub(crate) async fn mem_checkpoint(
        &self,
        name: &str,
        i: &bollard::models::ContainerInspectResponse,
        what: &str,
    ) -> Result<SnapRecord> {
        let sup = self.support().await.clone();
        sup.memory.clone().map_err(VmmError::other)?;
        let id =
            i.id.clone()
                .ok_or_else(|| VmmError::NotFound(name.into()))?;
        let dir = Self::snap_dir(&sup, what);
        let started = std::time::Instant::now();
        self.helper().checkpoint(&id, &dir).await?;
        tracing::info!(
            container = name,
            ms = started.elapsed().as_millis() as u64,
            "memory checkpoint"
        );
        let cfg = i.config.clone().unwrap_or_default();
        let hc = i.host_config.clone().unwrap_or_default();
        let mut env = cfg.env.clone().unwrap_or_default();
        // A Space claimed from a warm pool still carries the pool secret in
        // its container environment; its cua-spacesd holds the Space's own
        // token, which the claim stored in the token file. The restore must
        // present that one.
        if cfg
            .labels
            .as_ref()
            .is_some_and(|l| l.contains_key(super::LABEL_POOL))
        {
            let req = crate::exec::ExecRequest {
                user: Some("0".into()),
                ..crate::exec::ExecRequest::sh("cat /etc/cua/env-token")
                    .timeout(std::time::Duration::from_secs(20))
            };
            let held = match crate::exec::GuestExec::exec(&self.exec_handle(name), req).await {
                Ok(out) if out.exit_code == 0 => {
                    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
                }
                _ => None,
            }
            .filter(|t| !t.is_empty());
            if let Some(token) = held {
                let key = TOKEN_KEYS
                    .iter()
                    .find(|k| env.iter().any(|kv| kv.starts_with(&format!("{k}="))))
                    .unwrap_or(&TOKEN_KEYS[0]);
                env = merge_env(&env, &BTreeMap::from([(key.to_string(), token)]));
            }
        }
        Ok(SnapRecord {
            name: what.to_string(),
            dir,
            engine: self.endpoint.uri.clone(),
            image_id: i.image.clone().unwrap_or_default(),
            env,
            entrypoint: cfg.entrypoint,
            cmd: cfg.cmd,
            working_dir: cfg.working_dir,
            user: cfg.user,
            ports: Self::published(i),
            cpus: (hc.nano_cpus.unwrap_or(0) / 1_000_000_000).max(1) as u32,
            memory_mb: (hc.memory.unwrap_or(0) / (1024 * 1024)).max(256) as u64,
            created: host::now_secs(),
        })
    }

    /// Creates (stopped) container `name` that restores `rec` when started.
    pub(crate) async fn create_restoring(
        &self,
        name: &str,
        rec: &SnapRecord,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut spec = rec.start_spec(name);
        spec.container_runtime = Some("runsc".into());
        self.create(name, &rec.image_id, &spec, labels, None, Some(&rec.dir))
            .await
    }

    /// The golden checkpoint that serves `spec` (a fresh start of `reference`),
    /// or `None` when warm starts do not apply to it.
    pub(crate) async fn golden_plan(
        &self,
        spec: &StartSpec,
        reference: &str,
    ) -> Option<GoldenPlan> {
        if !self.cfg.warm
            || !spec.sidecars.is_empty()
            || spec.gpu.is_some()
            || spec.restrict_network
            // An injected cua-spacesd starts cold: its upload is part of the
            // create, which a restore does not run.
            || spec.inject_spacesd.is_some()
            // A checkpoint restores its own mounts, never this start's.
            || !spec.mounts.is_empty()
            || self.group_runtime(spec).await.ok()? != "runsc"
        {
            return None;
        }
        let sup = self.support().await;
        if !sup.ok() {
            return None;
        }
        // Ready = a TCP listener: the probes', else cua-spacesd's.
        let mut probe_ports = Vec::new();
        for p in &spec.probes {
            match p {
                crate::Probe::Tcp { port } => probe_ports.push(*port),
                _ => return None,
            }
        }
        let ready_port = match probe_ports.first() {
            Some(p) => *p,
            None if spec.ports.contains(&SPACESD_PORT)
                && TOKEN_KEYS.iter().any(|k| spec.env.contains_key(*k)) =>
            {
                SPACESD_PORT
            }
            None => return None,
        };
        let image_id = self.docker.inspect_image(reference).await.ok()?.id?;
        let key = golden_key(
            &image_id,
            spec.cpus,
            spec.memory_mb,
            &spec.ports,
            &probe_ports,
            &spec.env,
            spec.command.as_ref(),
            &sup.runsc,
        );
        let record = read_record("golden", &format!("golden-{key}"))
            .filter(|r| r.engine == self.endpoint.uri && r.image_id == image_id);
        Some(GoldenPlan {
            key,
            image_id,
            ready_port,
            record,
        })
    }

    /// Drops a golden checkpoint (its record and its directory).
    pub(crate) async fn drop_golden(&self, rec: &SnapRecord) {
        remove_record("golden", &rec.name);
        let _ = self.helper().remove(&[&rec.dir]).await;
    }

    /// Records the golden checkpoint of `plan` in the background: a
    /// container of `spec`'s configuration holding a throwaway token is
    /// booted to readiness, checkpointed and removed. One per key at a time.
    pub(crate) async fn spawn_golden(&self, spec: &StartSpec, reference: &str, plan: GoldenPlan) {
        {
            if NO_GOLDEN
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&plan.key)
            {
                return;
            }
            let mut b = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
            if b.contains(&plan.key) {
                return;
            }
            b.push(plan.key.clone());
        }
        let sup = self.support().await.clone();
        let engine_cpus = self.engine_cpus().await;
        let job = GoldenJob {
            docker: self.docker.clone(),
            engine: self.endpoint.uri.clone(),
            dir: Self::snap_dir(&sup, &format!("golden-{}", plan.key)),
            reference: reference.to_string(),
            spec: spec.clone(),
            plan,
            engine_cpus,
        };
        tokio::spawn(async move {
            let key = job.plan.key.clone();
            let started = std::time::Instant::now();
            match job.run().await {
                Ok(()) => tracing::info!(
                    key,
                    ms = started.elapsed().as_millis() as u64,
                    "recorded a golden gVisor checkpoint"
                ),
                Err(e) => {
                    tracing::warn!(key, error = %e, "could not record a golden gVisor checkpoint")
                }
            }
            BUILDING
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|k| k != &key);
        });
    }
}

/// One golden recording ([`super::ContainerRuntime::spawn_golden`]).
struct GoldenJob {
    docker: Docker,
    engine: String,
    dir: String,
    reference: String,
    spec: StartSpec,
    plan: GoldenPlan,
    engine_cpus: i64,
}

impl GoldenJob {
    async fn run(self) -> Result<()> {
        let name = format!("cua-golden-{}", self.plan.key);
        let rm = || async {
            let _ = self
                .docker
                .remove_container(
                    &name,
                    Some(bollard::query_parameters::RemoveContainerOptions {
                        force: true,
                        v: true,
                        ..Default::default()
                    }),
                )
                .await;
        };
        rm().await;
        let r = self.record(&name).await;
        rm().await;
        let rec = r?;
        write_record("golden", &rec)?;
        // Keep the newest few of this home's golden checkpoints.
        let mine: Vec<SnapRecord> = records("golden");
        if mine.len() > MAX_GOLDEN {
            let helper = Helper::new(self.docker.clone());
            for old in &mine[..mine.len() - MAX_GOLDEN] {
                remove_record("golden", &old.name);
                if old.engine == self.engine {
                    let _ = helper.remove(&[&old.dir]).await;
                }
            }
        }
        Ok(())
    }

    async fn record(&self, name: &str) -> Result<SnapRecord> {
        // The golden container's token: random, used by nothing but the
        // restore that rotates it away at once.
        let secret = random_hex(24);
        let mut spec = self.spec.clone();
        spec.name = name.to_string();
        spec.ports.clear();
        for k in TOKEN_KEYS {
            if let Some(v) = spec.env.get_mut(k) {
                *v = secret.clone();
            }
        }
        let mut labels = std::collections::HashMap::new();
        labels.insert(LABEL_KIND.to_string(), "golden".to_string());
        let mut body = super::create_body(
            name,
            &self.reference,
            &spec,
            labels,
            WRAPPER_RUNTIME.to_string(),
        );
        // Not a sandbox: `list` and the disk view skip it.
        if let Some(l) = body.labels.as_mut() {
            l.remove(super::LABEL_BACKEND);
            l.remove(super::LABEL_SANDBOX);
        }
        if let Some(hc) = body.host_config.as_mut() {
            hc.nano_cpus = hc
                .nano_cpus
                .map(|n| super::cap_nano_cpus(n, self.engine_cpus));
        }
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(name.to_string()),
                    platform: String::new(),
                }),
                body,
            )
            .await
            .map_err(engine_err)?;
        self.docker
            .start_container(name, None)
            .await
            .map_err(engine_err)?;
        // Ready: the port listens inside the container.
        let deadline =
            tokio::time::Instant::now() + self.spec.ready_timeout.min(Duration::from_secs(180));
        let exec = super::DockerExec {
            docker: self.docker.clone(),
            container: name.to_string(),
        };
        loop {
            let out = crate::exec::GuestExec::exec(
                &exec,
                crate::exec::ExecRequest::sh("cat /proc/net/tcp /proc/net/tcp6 2>/dev/null")
                    .timeout(Duration::from_secs(5)),
            )
            .await;
            if let Ok(o) = out
                && crate::probe::proc_net_listens(
                    &String::from_utf8_lossy(&o.stdout),
                    self.plan.ready_port,
                ) == Some(true)
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(VmmError::other("the golden container never became ready"));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // Every Space restored from it would share its cua-spacesd's
        // per-machine secrets unless that cua-spacesd renews them.
        let info = crate::exec::GuestExec::exec(
            &exec,
            crate::exec::ExecRequest {
                user: Some("0".into()),
                ..crate::exec::ExecRequest::sh(crate::identity_gate::BUILD_INFO_SCRIPT)
                    .timeout(Duration::from_secs(10))
            },
        )
        .await
        .ok()
        .and_then(|o| crate::identity_gate::revision_in(&String::from_utf8_lossy(&o.stdout)));
        if !crate::identity_gate::renews_identity(info) {
            NO_GOLDEN
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(self.plan.key.clone());
            return Err(VmmError::other(format!(
                "this image's cua-spacesd (protocol revision {}) cannot become a new machine, \
                 so its Spaces start cold rather than share one snapshot's keys",
                info.map_or("unknown".to_string(), |r| r.to_string())
            )));
        }
        tokio::time::sleep(GOLDEN_SETTLE).await;
        let i = self
            .docker
            .inspect_container(name, None)
            .await
            .map_err(engine_err)?;
        let id = i.id.clone().unwrap_or_default();
        Helper::new(self.docker.clone())
            .checkpoint(&id, &self.dir)
            .await?;
        let cfg = i.config.unwrap_or_default();
        Ok(SnapRecord {
            name: format!("golden-{}", self.plan.key),
            dir: self.dir.clone(),
            engine: self.engine.clone(),
            image_id: self.plan.image_id.clone(),
            env: cfg.env.unwrap_or_default(),
            entrypoint: cfg.entrypoint,
            cmd: cfg.cmd,
            working_dir: cfg.working_dir,
            user: cfg.user,
            ports: self.spec.ports.clone(),
            cpus: self.spec.cpus,
            memory_mb: self.spec.memory_mb,
            created: host::now_secs(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_parses_and_refuses_pac() {
        let ok = parse_info(&format!(
            "version={WRAPPER_VERSION}\nstore=/var/lib/docker/cua-runsc\npac=0\narch=x86_64\nrunsc=runsc version release-20260921.0\n"
        ));
        assert!(ok.ok());
        assert_eq!(ok.store, "/var/lib/docker/cua-runsc");
        assert_eq!(ok.runsc, "runsc version release-20260921.0");
        let pac = parse_info(&format!(
            "version={WRAPPER_VERSION}\nstore=/s\npac=1\narch=aarch64\n"
        ));
        assert!(pac.memory.unwrap_err().contains("arm64.nopauth"));
        let old = parse_info("version=0\nstore=/s\npac=0\n");
        assert!(old.memory.unwrap_err().contains("is needed"));
    }

    #[test]
    fn the_built_in_runtime_registers_the_wrapper() {
        let y = crate::managed::profile_yaml("aarch64", 4, 4);
        assert!(y.contains(&format!(
            "    {WRAPPER_RUNTIME}:\n      path: {WRAPPER_PATH}\n"
        )));
    }

    #[test]
    fn tokens_and_env_merge() {
        let env = vec![
            "A=1".to_string(),
            "CUA_ENV_TOKEN=old".to_string(),
            "B=x=y".to_string(),
        ];
        assert_eq!(token_of(&env).as_deref(), Some("old"));
        let over: BTreeMap<String, String> =
            [("CUA_ENV_TOKEN".to_string(), "new".to_string())].into();
        let m = merge_env(&env, &over);
        assert_eq!(m, vec!["A=1", "B=x=y", "CUA_ENV_TOKEN=new"]);
        assert_eq!(token_of(&m).as_deref(), Some("new"));
    }

    #[test]
    fn golden_key_ignores_the_token_and_the_order() {
        let mut env: BTreeMap<String, String> = [("A".to_string(), "1".to_string())].into();
        let k1 = golden_key("sha256:x", 2, 2048, &[3211, 80], &[3211], &env, None, "r1");
        env.insert("CUA_ENV_TOKEN".into(), "t".into());
        let k2 = golden_key("sha256:x", 2, 2048, &[80, 3211], &[3211], &env, None, "r1");
        assert_eq!(k1, k2);
        assert_ne!(
            k1,
            golden_key("sha256:y", 2, 2048, &[80, 3211], &[3211], &env, None, "r1")
        );
        assert_ne!(
            k1,
            golden_key("sha256:x", 2, 4096, &[80, 3211], &[3211], &env, None, "r1")
        );
        assert_ne!(
            k1,
            golden_key("sha256:x", 2, 2048, &[80, 3211], &[3211], &env, None, "r2")
        );
        env.insert("B".into(), "2".into());
        assert_ne!(
            k1,
            golden_key("sha256:x", 2, 2048, &[80, 3211], &[3211], &env, None, "r1")
        );
    }

    #[test]
    fn helper_image_is_a_valid_tar() {
        let t = helper_tar().unwrap();
        let mut a = tar::Archive::new(&t[..]);
        let names: Vec<String> = a
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"host/".to_string()) || names.contains(&"host".to_string()));
        assert!(names.contains(&"lib".to_string()));
    }

    /// The wrapper, run against a fake runsc that records its argv.
    #[cfg(unix)]
    #[test]
    fn wrapper_restores_an_annotated_container_and_passes_the_rest_through() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("argv");
        let fake = d.path().join("runsc");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\necho \"$*\" >> {}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let w = d.path().join("cua-runsc");
        let state = d.path().join("state");
        std::fs::write(
            &w,
            render_wrapper(
                fake.to_str().unwrap(),
                state.to_str().unwrap(),
                d.path().join("store").to_str().unwrap(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bundle = d.path().join("bundle");
        std::fs::create_dir_all(&bundle).unwrap();
        let run = |args: &[&str]| {
            let st = std::process::Command::new(&w).args(args).status().unwrap();
            assert!(st.success(), "{args:?}");
        };
        let g = [
            "--root",
            "/r/moby",
            "--log",
            "/l/log.json",
            "--log-format",
            "json",
            "--systemd-cgroup",
        ];
        let b = bundle.to_str().unwrap();
        // Annotated: create passes through, start restores.
        std::fs::write(
            bundle.join("config.json"),
            r#"{"ociVersion":"1.2.1","annotations":{"ai.cua.restore":"/store/h/golden/k1","x":"y"}}"#,
        )
        .unwrap();
        let mut a: Vec<&str> = g.to_vec();
        a.extend(["create", "--bundle", b, "--pid-file", "/p/init.pid", "c1"]);
        run(&a);
        let mut a: Vec<&str> = g.to_vec();
        a.extend(["start", "c1"]);
        run(&a);
        // Stopped and started again (create, start): it boots.
        let mut c: Vec<&str> = g.to_vec();
        c.extend(["create", "--bundle", b, "--pid-file", "/p/init.pid", "c1"]);
        run(&c);
        run(&a);
        // Not annotated.
        std::fs::write(bundle.join("config.json"), r#"{"annotations":{}}"#).unwrap();
        let mut a: Vec<&str> = g.to_vec();
        a.extend(["create", "--bundle", b, "c2"]);
        run(&a);
        let mut a: Vec<&str> = g.to_vec();
        a.extend(["start", "c2"]);
        run(&a);
        assert_eq!(
            std::fs::read_to_string(state.join("c2.root")).unwrap(),
            "/r/moby"
        );
        let mut a: Vec<&str> = g.to_vec();
        a.extend(["delete", "--force", "c2"]);
        run(&a);
        assert!(!state.join("c2.root").exists());
        let lines: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        let gs = g.join(" ");
        assert_eq!(
            lines[0],
            format!("{gs} create --bundle {b} --pid-file /p/init.pid c1")
        );
        assert_eq!(
            lines[1],
            format!(
                "{gs} --restore-spec-validation=warning restore --detach --background --image-path /store/h/golden/k1 c1"
            )
        );
        assert_eq!(
            lines[2],
            format!("{gs} create --bundle {b} --pid-file /p/init.pid c1")
        );
        assert_eq!(lines[3], format!("{gs} start c1"));
        assert_eq!(lines[4], format!("{gs} create --bundle {b} c2"));
        assert_eq!(lines[5], format!("{gs} start c2"));
        assert_eq!(lines[6], format!("{gs} delete --force c2"));
    }
}
