//! External runtime providers: local runtimes installed separately from
//! cua, discovered at run time, that run Spaces exactly like the built-in
//! ones (Lume, QEMU, gVisor, Firecracker, Containerization).
//!
//! A provider is an executable named `cua-runtime-<name>`, found in
//! `$CUA_HOME/runtime-providers/` (default `~/.cua/runtime-providers/`) and
//! then on `PATH`. (`$CUA_HOME/runtimes/` holds the runtimes cua installs
//! itself and is never searched.)
//! `<name>` matches `[a-z0-9][a-z0-9-]{0,31}` and becomes a runtime word
//! (`cua sandbox create --runtime <name>`); a name that is already a
//! built-in runtime or kind word is ignored (built-ins win). Discovery is
//! cached per process and checked again when a searched directory or a
//! provider executable changes, so a freshly installed provider shows up
//! in a running daemon or app without a restart.
//!
//! The protocol (version [`PROTOCOL_VERSION`]) is one process per call:
//! `cua-runtime-<name> <verb>`, the request as JSON on stdin, the response
//! as JSON on stdout, exit 0 on success. A failure exits non-zero and
//! prints `{"error":{"kind":"not_found|unsupported|invalid|unavailable|other",
//! "message":"..."}}` on stdout. stderr is free-form log text. Every call
//! has a timeout and the child is killed when it expires or the caller
//! goes away. The environment carries `CUA_RUNTIME_PROTOCOL=1`.
//!
//! | verb | request | response |
//! |---|---|---|
//! | `info` | `{}` | [`Manifest`] |
//! | `doctor` | `{}` | [`DoctorStatus`] |
//! | `start` | [`LocalStartSpec`] (+ `registry_credentials` when present) | [`LocalInstance`] (+ optional `image`: [`ReportedImage`]) |
//! | `stop`, `delete`, `suspend` | `{"name"}` | `{}` |
//! | `resume` | `{"name"}` | [`LocalInstance`] |
//! | `status` | `{"name"}` | `{"status"}` |
//! | `endpoints` | `{"name"}` | [`LocalEndpoints`] |
//! | `list` | `{}` | `[LocalSummary]` |
//! | `exec` (capability) | `{"name","argv","env"}` | `{"exit_code","stdout_b64","stderr_b64"}` |
//! | `screenshot` (capability) | `{"name"}` | `{"png_b64","width","height"}` |
//! | `checkpoint` (capability) | `{"name","checkpoint"}` | `{}` |
//! | `fork` (capability) | `{"source","new_name"}` | `{}` |
//!
//! A provider that declares `handles_images` gets the image reference as
//! given: cua does not resolve it (the provider's images may use media
//! types only it understands). The provider may report the guest OS,
//! architecture and whether the image runs cua-spacesd in `start`'s
//! `image` field, which readiness then uses like a resolved image's.
//!
//! [`WithExternal`] puts the providers in front of another
//! [`LocalRuntime`] (the built-in engines): a start whose runtime names a
//! provider goes to it, and every later call on that instance (by name, or
//! by the persisted `runtime_type`, which is the provider's name) goes
//! back to the same provider. With no provider installed it only forwards.

use crate::placement::{Kind, Runtime};
use crate::runtime::{
    GuestOutput, GuestScreenshot, ImageInfo, InstanceStatus, LocalEndpoints, LocalInstance,
    LocalRuntime, LocalStartSpec, LocalSummary, RuntimeError, RuntimeResult,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// The protocol version this build speaks (`CUA_RUNTIME_PROTOCOL`).
pub const PROTOCOL_VERSION: u32 = 1;

/// The executable name prefix: `cua-runtime-<name>`.
pub const EXECUTABLE_PREFIX: &str = "cua-runtime-";

/// The directory of the cua home searched for providers before `PATH`.
pub const PROVIDERS_DIR: &str = "runtime-providers";

/// The environment variable that carries [`PROTOCOL_VERSION`] to a provider.
pub const PROTOCOL_ENV: &str = "CUA_RUNTIME_PROTOCOL";

/// How long `info` may take.
pub const INFO_TIMEOUT: Duration = Duration::from_secs(10);

/// How long any call but `start` may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// What `start` may take beyond the spec's `ready_timeout`.
pub const START_MARGIN: Duration = Duration::from_secs(60);

/// How many stderr lines an error message carries.
const STDERR_TAIL: usize = 20;

/// The guest port cua-spacesd listens on unless a provider says otherwise.
const DEFAULT_SPACESD_PORT: u16 = crate::ENV_PORT;

/// Words a provider cannot take: the built-in runtimes, their aliases,
/// the kinds, the locations, and backend labels persisted as
/// `runtime_type`.
const RESERVED: &[&str] = &[
    "auto",
    "gvisor",
    "runsc",
    "runc",
    "qemu",
    "vz",
    "lume",
    "kubevirt",
    "firecracker",
    "vz",
    "virtualization",
    "containerization",
    "apple",
    "apple-container",
    "container",
    "containers",
    "docker",
    "vm",
    "microvm",
    "micro-vm",
    "local",
    "cloud",
    "fleet",
    "direct",
    "relay",
    "host",
    "managed",
    "vmm",
    "qemu-docker",
    "qemu-baremetal",
    "external",
];

/// Whether `name` is a valid provider name: `[a-z0-9][a-z0-9-]{0,31}`.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Whether `name` is a built-in runtime, kind or location word.
pub fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name)
}

/// What a provider can do beyond the required verbs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderCapabilities {
    /// `suspend` pauses in memory (otherwise a Space powers off with `stop`).
    pub suspend: bool,
    /// `fork` makes a copy of a stopped instance or checkpoint.
    pub fork: bool,
    /// `checkpoint` captures an instance.
    pub checkpoint: bool,
    /// `exec` runs a command in the guest without cua-spacesd.
    pub exec: bool,
    /// `screenshot` captures the guest display without cua-spacesd.
    pub screenshot: bool,
    /// Publishes guest ports other than cua-spacesd's.
    pub ports: bool,
    /// Can cut guest egress (`restrict_network`).
    pub restrict_network: bool,
    /// The most vCPUs an instance may have.
    pub max_cpus: Option<u32>,
    /// The most memory (MiB) an instance may have.
    pub max_memory_mb: Option<u64>,
}

/// An image a provider offers, for image catalogs and the New Space wizard.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogImage {
    /// The reference to pass to `start`.
    pub reference: String,
    /// A short title.
    pub title: String,
    /// Whether it is the provider's default image.
    pub default: bool,
}

/// What `info` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The protocol version the provider speaks.
    pub protocol: u32,
    /// The provider's name (the `<name>` of `cua-runtime-<name>`).
    pub name: String,
    /// The name to show people.
    #[serde(default)]
    pub display_name: String,
    /// One line about it.
    #[serde(default)]
    pub description: String,
    /// The kind its Spaces present as: `vm`, `container` or `microvm`.
    pub kind: String,
    /// The guest operating systems it runs (`macos`, `linux`, `windows`).
    #[serde(default)]
    pub os: Vec<String>,
    /// The guest architectures it runs (`arm64`, `amd64`).
    #[serde(default)]
    pub arch: Vec<String>,
    /// The provider resolves and pulls its own images: cua passes the
    /// reference through untouched.
    #[serde(default)]
    pub handles_images: bool,
    /// The guest port cua-spacesd listens on.
    #[serde(default = "default_spacesd_port")]
    pub spacesd_port: u16,
    /// Optional verbs and limits.
    #[serde(default)]
    pub capabilities: ProviderCapabilities,
    /// Images it offers.
    #[serde(default)]
    pub images: Vec<CatalogImage>,
}

fn default_spacesd_port() -> u16 {
    DEFAULT_SPACESD_PORT
}

impl Manifest {
    /// The display name, or the name when the provider gave none.
    pub fn label(&self) -> &str {
        if self.display_name.trim().is_empty() {
            &self.name
        } else {
            &self.display_name
        }
    }
}

/// What `doctor` answers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DoctorStatus {
    /// Whether the provider can start a Space now.
    pub ready: bool,
    /// One line about its state.
    pub detail: String,
    /// What is missing for it to be ready.
    pub missing: Vec<String>,
}

/// The image as a provider that handles its own images ran it (`start`'s
/// optional `image` field).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportedImage {
    /// The reference as the provider normalised it.
    pub reference: Option<String>,
    /// The digest that runs, when the provider pinned one.
    pub digest: Option<String>,
    /// Guest OS.
    pub os: Option<String>,
    /// Guest architecture.
    pub arch: Option<String>,
    /// Whether the image runs cua-spacesd.
    pub spacesd: Option<bool>,
}

/// A discovered provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalProvider {
    /// What `info` said.
    pub manifest: Manifest,
    /// The executable.
    pub path: PathBuf,
    /// The kind its Spaces present as.
    pub kind: Kind,
}

impl ExternalProvider {
    /// The provider's name (its runtime word).
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// The runtime word as a [`Runtime`].
    pub fn runtime(&self) -> Runtime {
        Runtime::Other(self.manifest.name.clone())
    }
}

/// A `cua-runtime-*` executable that was found but is not used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The name from the file name.
    pub name: String,
    /// The executable.
    pub path: PathBuf,
    /// Why it is not used.
    pub reason: String,
}

/// One discovery pass: the providers in use and the executables skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discovery {
    /// Providers in use, by name.
    pub providers: Vec<Arc<ExternalProvider>>,
    /// Executables that are not used, and why.
    pub skipped: Vec<Skipped>,
}

/// Where providers are looked for.
#[derive(Clone, Debug)]
enum Dirs {
    /// [`PROVIDERS_DIR`] in the cua home, then `PATH` (read at each scan).
    Default,
    /// These directories, in order.
    Fixed(Vec<PathBuf>),
}

#[derive(Default)]
struct Cache {
    /// The fingerprint the last scan saw: each directory's and each
    /// executable's modification time.
    fingerprint: Vec<(PathBuf, Option<SystemTime>, u64)>,
    discovery: Arc<Discovery>,
    /// `info` answers by executable and modification time, so an
    /// unchanged executable is not asked again.
    manifests: HashMap<(PathBuf, Option<SystemTime>, u64), Result<Manifest, String>>,
    scanned: bool,
}

/// Discovers providers (see the module docs) and caches what it found.
pub struct Registry {
    dirs: Dirs,
    cache: Mutex<Cache>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("dirs", &self.dirs)
            .finish_non_exhaustive()
    }
}

impl Registry {
    /// The process-wide registry: `$CUA_HOME/runtime-providers`, then `PATH`.
    pub fn global() -> &'static Arc<Registry> {
        static R: OnceLock<Arc<Registry>> = OnceLock::new();
        R.get_or_init(|| {
            Arc::new(Registry {
                dirs: Dirs::Default,
                cache: Mutex::new(Cache::default()),
            })
        })
    }

    /// A registry over `dirs` only, searched in order (no `PATH`).
    pub fn with_dirs(dirs: Vec<PathBuf>) -> Arc<Registry> {
        Arc::new(Registry {
            dirs: Dirs::Fixed(dirs),
            cache: Mutex::new(Cache::default()),
        })
    }

    fn search_dirs(&self) -> Vec<PathBuf> {
        match &self.dirs {
            Dirs::Fixed(d) => d.clone(),
            Dirs::Default => {
                let mut v = vec![cua_home::cua_home().join(PROVIDERS_DIR)];
                if let Some(path) = std::env::var_os("PATH") {
                    v.extend(std::env::split_paths(&path).filter(|p| !p.as_os_str().is_empty()));
                }
                let mut seen = HashSet::new();
                v.retain(|p| seen.insert(p.clone()));
                v
            }
        }
    }

    /// The providers in use (cached; checked again when a searched
    /// directory or a provider changes).
    pub fn providers(&self) -> Vec<Arc<ExternalProvider>> {
        self.discover().providers.clone()
    }

    /// The provider named `name`.
    pub fn get(&self, name: &str) -> Option<Arc<ExternalProvider>> {
        let name = name.trim().to_ascii_lowercase();
        if !valid_name(&name) || is_reserved(&name) {
            return None;
        }
        self.discover()
            .providers
            .iter()
            .find(|p| p.name() == name)
            .cloned()
    }

    /// The last discovery, scanning again when something changed.
    pub fn discover(&self) -> Arc<Discovery> {
        let dirs = self.search_dirs();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut fingerprint: Vec<(PathBuf, Option<SystemTime>, u64)> = dirs
            .iter()
            .map(|d| {
                let m = std::fs::metadata(d).ok();
                (d.clone(), m.as_ref().and_then(|m| m.modified().ok()), 0)
            })
            .collect();
        // The executables the last scan used: a provider replaced in place
        // does not change its directory.
        for p in cache
            .discovery
            .providers
            .iter()
            .map(|p| p.path.clone())
            .chain(cache.discovery.skipped.iter().map(|s| s.path.clone()))
        {
            fingerprint.push(file_stamp(&p));
        }
        if cache.scanned && cache.fingerprint == fingerprint {
            return cache.discovery.clone();
        }
        let discovery = Arc::new(scan(&dirs, &mut cache.manifests));
        // Fingerprint again over what this scan found.
        let mut fp: Vec<_> = fingerprint.into_iter().take(dirs.len()).collect();
        for p in discovery
            .providers
            .iter()
            .map(|p| p.path.clone())
            .chain(discovery.skipped.iter().map(|s| s.path.clone()))
        {
            fp.push(file_stamp(&p));
        }
        cache.fingerprint = fp;
        cache.discovery = discovery.clone();
        cache.scanned = true;
        discovery
    }
}

fn file_stamp(p: &Path) -> (PathBuf, Option<SystemTime>, u64) {
    let m = std::fs::metadata(p).ok();
    (
        p.to_path_buf(),
        m.as_ref().and_then(|m| m.modified().ok()),
        m.map(|m| m.len()).unwrap_or(0),
    )
}

/// The `<name>` of a `cua-runtime-<name>` file name.
fn name_of(file: &str) -> Option<&str> {
    let rest = file.strip_prefix(EXECUTABLE_PREFIX)?;
    if cfg!(windows) {
        rest.strip_suffix(".exe")
            .or_else(|| rest.strip_suffix(".EXE"))
    } else {
        Some(rest)
    }
}

fn is_executable(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else {
        return false;
    };
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn scan(
    dirs: &[PathBuf],
    manifests: &mut HashMap<(PathBuf, Option<SystemTime>, u64), Result<Manifest, String>>,
) -> Discovery {
    let mut out = Discovery::default();
    let mut taken: HashSet<String> = HashSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let file = e.file_name().to_str()?.to_string();
                let name = name_of(&file)?.to_string();
                Some((name, e.path()))
            })
            .collect();
        found.sort();
        for (name, path) in found {
            if !is_executable(&path) || taken.contains(&name) {
                // An earlier directory's provider of the same name wins.
                continue;
            }
            let skip = |reason: String| {
                tracing::warn!(provider = %name, path = %path.display(), "{reason}");
                Skipped {
                    name: name.clone(),
                    path: path.clone(),
                    reason,
                }
            };
            if !valid_name(&name) {
                out.skipped.push(skip(format!(
                    "{name:?} is not a valid runtime name ([a-z0-9][a-z0-9-]{{0,31}}); ignored"
                )));
                taken.insert(name);
                continue;
            }
            if is_reserved(&name) {
                out.skipped.push(skip(format!(
                    "{name:?} is a built-in runtime or kind word; the built-in wins and this \
                     provider is ignored"
                )));
                taken.insert(name);
                continue;
            }
            taken.insert(name.clone());
            let stamp = file_stamp(&path);
            let manifest = manifests
                .entry(stamp)
                .or_insert_with(|| info_blocking(&path))
                .clone();
            match manifest.and_then(|m| check_manifest(&name, m)) {
                Ok((manifest, kind)) => out.providers.push(Arc::new(ExternalProvider {
                    manifest,
                    path,
                    kind,
                })),
                Err(e) => out.skipped.push(skip(e)),
            }
        }
    }
    out
}

fn check_manifest(name: &str, m: Manifest) -> Result<(Manifest, Kind), String> {
    if m.protocol != PROTOCOL_VERSION {
        return Err(format!(
            "speaks protocol {}, this cua speaks {PROTOCOL_VERSION}; ignored",
            m.protocol
        ));
    }
    if m.name != name {
        return Err(format!(
            "info names it {:?}, but its file name says {name:?}; ignored",
            m.name
        ));
    }
    let kind = match Kind::parse(&m.kind) {
        Ok(k) if k != Kind::Auto => k,
        _ => {
            return Err(format!(
                "kind {:?} is not vm, container or microvm; ignored",
                m.kind
            ));
        }
    };
    Ok((m, kind))
}

/// Runs `info` synchronously (discovery is synchronous: it backs the
/// placement checks), with [`INFO_TIMEOUT`].
fn info_blocking(path: &Path) -> Result<Manifest, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let mut child = Command::new(path)
        .arg("info")
        .env(PROTOCOL_ENV, PROTOCOL_VERSION.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run info: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"{}");
    }
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(o) = stdout.as_mut() {
            let _ = o.read_to_end(&mut s);
        }
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.read_to_end(&mut s);
        }
        s
    });
    let deadline = std::time::Instant::now() + INFO_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("info timed out after {}s", INFO_TIMEOUT.as_secs()));
            }
            Err(e) => return Err(format!("info: {e}")),
        }
    };
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    let tail = stderr_tail(&String::from_utf8_lossy(&err));
    if !status.success() {
        let why = protocol_error(&out)
            .map(|(_, m)| m)
            .unwrap_or_else(|| format!("exited with {status}"));
        return Err(with_tail(format!("info failed: {why}"), &tail));
    }
    serde_json::from_slice(&out).map_err(|e| with_tail(format!("info answered {e}"), &tail))
}

fn stderr_tail(s: &str) -> Vec<String> {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(STDERR_TAIL)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

fn with_tail(msg: String, tail: &[String]) -> String {
    if tail.is_empty() {
        msg
    } else {
        format!("{msg}\n{}", tail.join("\n"))
    }
}

/// The `error` object of a failed call's stdout.
fn protocol_error(stdout: &[u8]) -> Option<(String, String)> {
    let v: Value = serde_json::from_slice(stdout).ok()?;
    let e = v.get("error")?;
    Some((
        e.get("kind")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .to_string(),
        e.get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ))
}

/// One sentence naming `providers` as local runtimes, for tool and help
/// text (`None` when there are none, so nothing changes without them).
pub fn runtimes_hint(providers: &[Arc<ExternalProvider>]) -> Option<String> {
    if providers.is_empty() {
        return None;
    }
    let list: Vec<String> = providers
        .iter()
        .map(|p| {
            let label = p.manifest.label();
            if label == p.name() {
                format!("{} ({})", p.name(), p.kind)
            } else {
                format!("{} ({}, {})", p.name(), label, p.kind)
            }
        })
        .collect();
    Some(format!(
        "Installed local runtime providers: {}.",
        list.join(", ")
    ))
}

/// Appends [`runtimes_hint`] to the description of the `runtime` property
/// of a tool's JSON input schema. Returns whether it changed anything.
pub fn annotate_runtime_schema(schema: &mut Value, providers: &[Arc<ExternalProvider>]) -> bool {
    let Some(hint) = runtimes_hint(providers) else {
        return false;
    };
    let Some(Value::Object(runtime)) = schema.pointer_mut("/properties/runtime") else {
        return false;
    };
    let d = runtime
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end()
        .to_string();
    let d = if d.is_empty() {
        hint
    } else if d.ends_with('.') {
        format!("{d} {hint}")
    } else {
        format!("{d}. {hint}")
    };
    runtime.insert("description".into(), Value::String(d));
    true
}

/// One provider behind [`LocalRuntime`]. Its backend label (persisted as
/// `runtime_type`) is the provider's name.
#[derive(Clone, Debug)]
pub struct ExternalRuntime {
    provider: Arc<ExternalProvider>,
    call_timeout: Duration,
}

impl ExternalRuntime {
    /// The runtime of `provider`.
    pub fn new(provider: Arc<ExternalProvider>) -> Self {
        Self {
            provider,
            call_timeout: CALL_TIMEOUT,
        }
    }

    /// How long a call other than `start` may take (default
    /// [`CALL_TIMEOUT`]).
    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    /// The provider.
    pub fn provider(&self) -> &Arc<ExternalProvider> {
        &self.provider
    }

    fn name(&self) -> &str {
        self.provider.name()
    }

    fn spacesd_port(&self) -> u16 {
        self.provider.manifest.spacesd_port
    }

    /// Runs `verb` with `request` and parses the answer. `instance` names
    /// the instance a `not_found` refers to.
    async fn call<T: DeserializeOwned>(
        &self,
        verb: &str,
        request: &Value,
        timeout: Duration,
        instance: Option<&str>,
    ) -> RuntimeResult<T> {
        let out = self.call_raw(verb, request, timeout, instance).await?;
        // An empty answer is `{}`.
        let out = if out.iter().all(u8::is_ascii_whitespace) {
            b"{}".to_vec()
        } else {
            out
        };
        serde_json::from_slice(&out).map_err(|e| {
            RuntimeError::Other(format!(
                "runtime {} answered {verb} with invalid JSON: {e}",
                self.name()
            ))
        })
    }

    async fn call_raw(
        &self,
        verb: &str,
        request: &Value,
        timeout: Duration,
        instance: Option<&str>,
    ) -> RuntimeResult<Vec<u8>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let name = self.name().to_string();
        // Never log the request: `start` may carry registry credentials.
        tracing::debug!(provider = %name, verb, "external runtime call");
        let mut child = tokio::process::Command::new(&self.provider.path)
            .arg(verb)
            .env(PROTOCOL_ENV, PROTOCOL_VERSION.to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                RuntimeError::Other(format!("runtime {name}: could not run {verb}: {e}"))
            })?;
        let body = serde_json::to_vec(request).map_err(|e| RuntimeError::Other(e.to_string()))?;
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let run = async {
            let write = async {
                if let Some(mut s) = stdin.take() {
                    // A provider that does not read its request may close
                    // stdin early: not an error.
                    let _ = s.write_all(&body).await;
                    let _ = s.shutdown().await;
                }
            };
            let read_out = async {
                let mut v = Vec::new();
                if let Some(o) = stdout.as_mut() {
                    let _ = o.read_to_end(&mut v).await;
                }
                v
            };
            let read_err = async {
                let mut v = Vec::new();
                if let Some(e) = stderr.as_mut() {
                    let _ = e.read_to_end(&mut v).await;
                }
                v
            };
            let ((), out, err) = tokio::join!(write, read_out, read_err);
            let status = child.wait().await;
            (status, out, err)
        };
        let (status, out, err) = match tokio::time::timeout(timeout, run).await {
            Ok(r) => r,
            Err(_) => {
                // The child is killed when `child` drops (kill_on_drop).
                return Err(RuntimeError::Other(format!(
                    "runtime {name}: {verb} timed out after {}s",
                    timeout.as_secs()
                )));
            }
        };
        let err = String::from_utf8_lossy(&err);
        for line in err.lines().filter(|l| !l.trim().is_empty()) {
            tracing::debug!(provider = %name, verb, "{line}");
        }
        let status =
            status.map_err(|e| RuntimeError::Other(format!("runtime {name}: {verb}: {e}")))?;
        if status.success() {
            return Ok(out);
        }
        let tail = stderr_tail(&err);
        let (kind, message) = protocol_error(&out)
            .unwrap_or_else(|| ("other".into(), format!("{verb} exited with {status}")));
        Err(match kind.as_str() {
            "not_found" => RuntimeError::NotFound(
                instance
                    .map(str::to_string)
                    .unwrap_or_else(|| message.clone()),
            ),
            "unsupported" => RuntimeError::Unsupported {
                backend: name,
                op: if message.is_empty() {
                    verb.to_string()
                } else {
                    message
                },
            },
            "invalid" => RuntimeError::InvalidPlacement(crate::placement::PlacementError::new(
                crate::placement::Axis::Runtime,
                &name,
                format!("runtime {name} refused {verb}: {message}"),
                vec![],
            )),
            "unavailable" => RuntimeError::Other(with_tail(
                format!("runtime {name} is unavailable: {message}"),
                &tail,
            )),
            _ => RuntimeError::Other(with_tail(
                format!("runtime {name}: {verb} failed: {message}"),
                &tail,
            )),
        })
    }

    /// Asks the provider whether it can start a Space now.
    pub async fn doctor(&self) -> RuntimeResult<DoctorStatus> {
        self.call("doctor", &json!({}), self.call_timeout, None)
            .await
    }

    fn unsupported(&self, op: String) -> RuntimeError {
        RuntimeError::Unsupported {
            backend: self.name().to_string(),
            op,
        }
    }

    /// Checks `spec` against the manifest before asking the provider.
    fn check_spec(&self, spec: &LocalStartSpec) -> RuntimeResult<()> {
        let caps = &self.provider.manifest.capabilities;
        if !spec.sidecars.is_empty() {
            return Err(self.unsupported("sidecars".into()));
        }
        if spec.gpu.is_some() {
            return Err(self.unsupported("GPU options".into()));
        }
        if !spec.mounts.is_empty() {
            return Err(self.unsupported("host directories mounted into the guest".into()));
        }
        if spec.restrict_network && !caps.restrict_network {
            return Err(self.unsupported("a restricted network".into()));
        }
        if !spec.egress.is_empty() {
            // A provider's runtime is not a host-side point cua controls.
            return Err(self.unsupported(
                "an egress policy (cua enforces one only with its own runtimes, outside the \
                 guest)"
                    .into(),
            ));
        }
        if let Some(max) = caps.max_cpus
            && spec.cpus > max
        {
            return Err(self.invalid(format!("{} vCPUs (at most {max})", spec.cpus)));
        }
        if let Some(max) = caps.max_memory_mb
            && spec.memory_mb > max
        {
            return Err(self.invalid(format!("{} MiB of memory (at most {max})", spec.memory_mb)));
        }
        if !caps.ports
            && let Some(p) = spec
                .ports
                .iter()
                .find(|p| **p != crate::ENV_PORT && **p != self.spacesd_port())
        {
            return Err(self.unsupported(format!("publishing guest port {p}")));
        }
        Ok(())
    }

    fn invalid(&self, what: String) -> RuntimeError {
        let name = self.name();
        RuntimeError::InvalidPlacement(crate::placement::PlacementError::new(
            crate::placement::Axis::Runtime,
            name,
            format!("runtime {name} cannot run {what}"),
            vec![],
        ))
    }

    /// cua's spacesd port in the guest is the provider's: `ports` and
    /// probes ask for [`crate::ENV_PORT`], the provider gets its own.
    fn to_guest_port(&self, port: u16) -> u16 {
        if port == crate::ENV_PORT {
            self.spacesd_port()
        } else {
            port
        }
    }

    /// The other way: the provider's spacesd port is published as
    /// [`crate::ENV_PORT`] too, so the sandbox layer finds it.
    fn published_endpoints(&self, mut e: LocalEndpoints) -> LocalEndpoints {
        let sp = self.spacesd_port();
        if sp != crate::ENV_PORT
            && let Some(h) = e.ports.get(&sp).copied()
        {
            e.ports.entry(crate::ENV_PORT).or_insert(h);
        }
        if e.host.trim().is_empty() {
            e.host = "127.0.0.1".into();
        }
        e
    }

    fn instance(&self, i: WireInstance) -> LocalInstance {
        LocalInstance {
            name: i.name,
            // Persisted as `runtime_type`: later calls route back here.
            backend: self.name().to_string(),
            status: i.status.0,
            endpoints: self.published_endpoints(i.endpoints),
        }
    }
}

/// [`InstanceStatus`] on the wire: `running`, `paused`, `stopped`,
/// `provisioning` (any case), or anything else as unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WireStatus(InstanceStatus);

impl<'de> Deserialize<'de> for WireStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(WireStatus(match &v {
            Value::String(s) => match s.to_ascii_lowercase().as_str() {
                "running" => InstanceStatus::Running,
                "paused" | "suspended" => InstanceStatus::Paused,
                "stopped" => InstanceStatus::Stopped,
                "provisioning" | "starting" | "creating" => InstanceStatus::Provisioning,
                _ => InstanceStatus::Unknown(s.clone()),
            },
            // `{"Unknown": "..."}`, serde's own form.
            Value::Object(o) => InstanceStatus::Unknown(
                o.get("Unknown")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            other => InstanceStatus::Unknown(other.to_string()),
        }))
    }
}

#[derive(Deserialize)]
struct WireInstance {
    name: String,
    status: WireStatus,
    #[serde(default)]
    endpoints: LocalEndpoints,
    #[serde(default)]
    image: Option<ReportedImage>,
}

#[derive(Deserialize)]
struct WireSummary {
    name: String,
    status: WireStatus,
}

#[derive(Deserialize)]
struct WireStatusAnswer {
    status: WireStatus,
}

#[derive(Deserialize)]
struct WireExec {
    #[serde(default)]
    exit_code: i64,
    #[serde(default)]
    stdout_b64: String,
    #[serde(default)]
    stderr_b64: String,
}

#[derive(Deserialize)]
struct WireScreenshot {
    png_b64: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

fn b64(s: &str) -> RuntimeResult<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| RuntimeError::Other(format!("invalid base64 from runtime: {e}")))
}

/// `start`'s request: the spec, credentials only when present.
fn start_request(spec: &LocalStartSpec) -> RuntimeResult<Value> {
    let mut v = serde_json::to_value(spec).map_err(|e| RuntimeError::Other(e.to_string()))?;
    if let Value::Object(o) = &mut v {
        o.insert(
            "ready_timeout_secs".into(),
            json!(spec.ready_timeout.as_secs()),
        );
        if let Some(c) = &spec.registry_credentials {
            o.insert(
                "registry_credentials".into(),
                json!({
                    "registry": c.registry,
                    "username": c.username,
                    "password": c.password,
                }),
            );
        }
    }
    Ok(v)
}

#[async_trait]
impl LocalRuntime for ExternalRuntime {
    fn backend(&self) -> String {
        self.name().to_string()
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Ok(self.start_resolved(spec).await?.0)
    }

    async fn start_resolved(
        &self,
        spec: &LocalStartSpec,
    ) -> RuntimeResult<(LocalInstance, Option<ImageInfo>)> {
        self.check_spec(spec)?;
        let mut spec = spec.clone();
        spec.ports = spec.ports.iter().map(|p| self.to_guest_port(*p)).collect();
        spec.ports.sort_unstable();
        spec.ports.dedup();
        for p in &mut spec.probes {
            p.port = self.to_guest_port(p.port);
        }
        // A reference the provider resolves itself was never read from the
        // registry, so its OS is only the caller's default: use the one the
        // provider runs when it runs exactly one.
        let m = &self.provider.manifest;
        if m.handles_images && m.os.len() == 1 && !m.os.contains(&spec.os) {
            spec.os = m.os[0].clone();
        }
        spec.runtime = Some(self.name().to_string());
        spec.kind = Some(self.provider.kind.to_string());
        // Not a built-in container engine option.
        spec.container_runtime = None;
        let req = start_request(&spec)?;
        let timeout = spec.ready_timeout + START_MARGIN;
        let wire: WireInstance = self.call("start", &req, timeout, Some(&spec.name)).await?;
        let image = wire.image.clone().map(|i| {
            let reference = i.reference.unwrap_or_else(|| spec.image.clone());
            let digest = i.digest.unwrap_or_default();
            ImageInfo {
                pinned_ref: if digest.is_empty() {
                    reference.clone()
                } else {
                    format!("{reference}@{digest}")
                },
                reference,
                digest,
                variant: "external".into(),
                arch: i.arch,
                os: i.os.unwrap_or_else(|| spec.os.clone()),
                emulated: false,
                spacesd: i.spacesd,
                injected: false,
            }
        });
        Ok((self.instance(wire), image))
    }

    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        self.call::<Value>(
            "stop",
            &json!({ "name": name }),
            self.call_timeout,
            Some(name),
        )
        .await
        .map(drop)
    }

    async fn suspend(&self, name: &str) -> RuntimeResult<()> {
        if !self.provider.manifest.capabilities.suspend {
            return Err(self.unsupported(format!("suspend {name}")));
        }
        self.call::<Value>(
            "suspend",
            &json!({ "name": name }),
            self.call_timeout,
            Some(name),
        )
        .await
        .map(drop)
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        let wire: WireInstance = self
            .call(
                "resume",
                &json!({ "name": name }),
                self.call_timeout,
                Some(name),
            )
            .await?;
        Ok(self.instance(wire))
    }

    fn power_control(&self, _runtime_type: &str) -> Option<crate::PowerControl> {
        Some(if self.provider.manifest.capabilities.suspend {
            crate::PowerControl::Suspend
        } else {
            crate::PowerControl::Stop
        })
    }

    async fn fork(&self, source: &str, new_name: &str) -> RuntimeResult<()> {
        if !self.provider.manifest.capabilities.fork {
            return Err(self.unsupported(format!("fork {source} -> {new_name}")));
        }
        self.call::<Value>(
            "fork",
            &json!({ "source": source, "new_name": new_name }),
            self.call_timeout,
            Some(source),
        )
        .await
        .map(drop)
    }

    async fn checkpoint(&self, name: &str, checkpoint: &str) -> RuntimeResult<()> {
        if !self.provider.manifest.capabilities.checkpoint {
            return Err(self.unsupported(format!("checkpoint {name} -> {checkpoint}")));
        }
        self.call::<Value>(
            "checkpoint",
            &json!({ "name": name, "checkpoint": checkpoint }),
            self.call_timeout,
            Some(name),
        )
        .await
        .map(drop)
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        let rows: Vec<WireSummary> = self
            .call("list", &json!({}), self.call_timeout, None)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| LocalSummary {
                name: r.name,
                backend: self.name().to_string(),
                status: r.status.0,
            })
            .collect())
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        let a: WireStatusAnswer = self
            .call(
                "status",
                &json!({ "name": name }),
                self.call_timeout,
                Some(name),
            )
            .await?;
        Ok(a.status.0)
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.call::<Value>(
            "delete",
            &json!({ "name": name }),
            self.call_timeout,
            Some(name),
        )
        .await
        .map(drop)
    }

    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        let e: LocalEndpoints = self
            .call(
                "endpoints",
                &json!({ "name": name }),
                self.call_timeout,
                Some(name),
            )
            .await?;
        Ok(self.published_endpoints(e))
    }

    async fn guest_exec(
        &self,
        name: &str,
        script: &str,
        timeout: Option<Duration>,
        sink: tokio::sync::mpsc::Sender<GuestOutput>,
    ) -> RuntimeResult<i64> {
        if !self.provider.manifest.capabilities.exec {
            return Err(self.unsupported(format!("agentless exec in {name}")));
        }
        let req = json!({
            "name": name,
            "argv": ["/bin/sh", "-c", script],
            "env": BTreeMap::<String, String>::new(),
        });
        let budget = timeout.map_or(self.call_timeout, |t| t + Duration::from_secs(10));
        let out: WireExec = self.call("exec", &req, budget, Some(name)).await?;
        let stdout = b64(&out.stdout_b64)?;
        let stderr = b64(&out.stderr_b64)?;
        if !stdout.is_empty() {
            let _ = sink.send(GuestOutput::Stdout(stdout)).await;
        }
        if !stderr.is_empty() {
            let _ = sink.send(GuestOutput::Stderr(stderr)).await;
        }
        Ok(out.exit_code)
    }

    async fn guest_screenshot(&self, name: &str) -> RuntimeResult<GuestScreenshot> {
        if !self.provider.manifest.capabilities.screenshot {
            return Err(self.unsupported(format!("agentless screenshots of {name}")));
        }
        let s: WireScreenshot = self
            .call(
                "screenshot",
                &json!({ "name": name }),
                self.call_timeout,
                Some(name),
            )
            .await?;
        Ok(GuestScreenshot {
            png: b64(&s.png_b64)?,
            width: s.width,
            height: s.height,
            via: self.name().to_string(),
        })
    }
}

/// The providers of a [`Registry`] in front of another local runtime (see
/// the module docs). With no provider installed every call goes straight
/// to `inner`.
pub struct WithExternal {
    inner: Arc<dyn LocalRuntime>,
    registry: Arc<Registry>,
    /// Instance → provider, learned from starts and listings.
    owners: Mutex<HashMap<String, String>>,
}

impl WithExternal {
    /// `inner` behind `registry`'s providers.
    pub fn new(inner: Arc<dyn LocalRuntime>, registry: Arc<Registry>) -> Self {
        Self {
            inner,
            registry,
            owners: Mutex::new(HashMap::new()),
        }
    }

    /// `inner` behind the process-wide [`Registry::global`].
    pub fn global(inner: Arc<dyn LocalRuntime>) -> Self {
        Self::new(inner, Registry::global().clone())
    }

    fn remember(&self, name: &str, provider: &str) {
        self.owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), provider.to_string());
    }

    fn forget(&self, name: &str) {
        self.owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
    }

    /// The provider whose runtime word is `runtime_type`.
    fn by_type(&self, runtime_type: &str) -> Option<ExternalRuntime> {
        self.registry.get(runtime_type).map(ExternalRuntime::new)
    }

    /// The provider that owns the instance `name`: the one it was started
    /// on, else the one that lists it. `None` (the built-in engines) when
    /// no provider has it, and always when none is installed.
    async fn owner(&self, name: &str) -> Option<ExternalRuntime> {
        let providers = self.registry.providers();
        if providers.is_empty() {
            return None;
        }
        let known = self
            .owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned();
        if let Some(p) = known.and_then(|k| providers.iter().find(|p| p.name() == k).cloned()) {
            return Some(ExternalRuntime::new(p));
        }
        let asks = providers.into_iter().map(|p| async move {
            let rt = ExternalRuntime::new(p);
            let rows = rt.list().await.unwrap_or_else(|e| {
                tracing::debug!(provider = %rt.name(), error = %e, "list failed");
                vec![]
            });
            (rt, rows)
        });
        let mut found = None;
        for (rt, rows) in futures_util::future::join_all(asks).await {
            for r in &rows {
                self.remember(&r.name, rt.name());
            }
            if found.is_none() && rows.iter().any(|r| r.name == name) {
                found = Some(rt);
            }
        }
        found
    }

    /// The provider a start spec's runtime names.
    fn for_spec(&self, spec: &LocalStartSpec) -> Option<ExternalRuntime> {
        let r = spec.runtime.as_deref()?;
        match Runtime::parse(r).ok()? {
            Runtime::Other(name) => self.by_type(&name),
            _ => None,
        }
    }
}

#[async_trait]
impl LocalRuntime for WithExternal {
    fn backend(&self) -> String {
        self.inner.backend()
    }

    async fn reclaim(&self, need: crate::Shortfall) -> crate::Reclaimed {
        self.inner.reclaim(need).await
    }

    async fn warm_held(&self) -> Vec<crate::WarmHeld> {
        self.inner.warm_held().await
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Ok(self.start_resolved(spec).await?.0)
    }

    async fn start_resolved(
        &self,
        spec: &LocalStartSpec,
    ) -> RuntimeResult<(LocalInstance, Option<ImageInfo>)> {
        match self.for_spec(spec) {
            Some(rt) => {
                self.remember(&spec.name, rt.name());
                rt.start_resolved(spec).await
            }
            None => self.inner.start_resolved(spec).await,
        }
    }

    async fn gpu_support(&self) -> Vec<crate::gpu::GpuSupport> {
        self.inner.gpu_support().await
    }

    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        match self.owner(name).await {
            Some(rt) => rt.stop(name).await,
            None => self.inner.stop(name).await,
        }
    }

    async fn set_egress(
        &self,
        name: &str,
        policy: &cua_vmm::egress::EgressPolicy,
    ) -> RuntimeResult<cua_vmm::egress::Enforcement> {
        match self.owner(name).await {
            Some(rt) => rt.set_egress(name, policy).await,
            None => self.inner.set_egress(name, policy).await,
        }
    }

    async fn egress(&self, name: &str) -> RuntimeResult<cua_vmm::egress::EgressPolicy> {
        match self.owner(name).await {
            Some(rt) => rt.egress(name).await,
            None => self.inner.egress(name).await,
        }
    }

    async fn suspend(&self, name: &str) -> RuntimeResult<()> {
        match self.owner(name).await {
            Some(rt) => rt.suspend(name).await,
            None => self.inner.suspend(name).await,
        }
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        match self.owner(name).await {
            Some(rt) => rt.resume(name).await,
            None => self.inner.resume(name).await,
        }
    }

    async fn resize(&self, name: &str, shape: &cua_vmm::Resize) -> RuntimeResult<LocalInstance> {
        match self.owner(name).await {
            // The external runtime protocol has no resize.
            Some(rt) => rt.resize(name, shape).await,
            None => self.inner.resize(name, shape).await,
        }
    }

    fn power_control(&self, runtime_type: &str) -> Option<crate::PowerControl> {
        match self.by_type(runtime_type) {
            Some(rt) => rt.power_control(runtime_type),
            None => self.inner.power_control(runtime_type),
        }
    }

    async fn fork(&self, source: &str, new_name: &str) -> RuntimeResult<()> {
        match self.owner(source).await {
            Some(rt) => {
                rt.fork(source, new_name).await?;
                self.remember(new_name, rt.name());
                Ok(())
            }
            None => self.inner.fork(source, new_name).await,
        }
    }

    async fn checkpoint(&self, name: &str, checkpoint: &str) -> RuntimeResult<()> {
        match self.owner(name).await {
            Some(rt) => rt.checkpoint(name, checkpoint).await,
            None => self.inner.checkpoint(name, checkpoint).await,
        }
    }

    async fn checkpoint_shared(&self, name: &str, checkpoint: &str) -> RuntimeResult<bool> {
        match self.owner(name).await {
            Some(rt) => rt.checkpoint_shared(name, checkpoint).await,
            None => self.inner.checkpoint_shared(name, checkpoint).await,
        }
    }

    async fn pin_checkpoint(
        &self,
        runtime_type: &str,
        checkpoint: &str,
        pinned: bool,
    ) -> RuntimeResult<()> {
        match self.by_type(runtime_type) {
            Some(rt) => rt.pin_checkpoint(runtime_type, checkpoint, pinned).await,
            None => {
                self.inner
                    .pin_checkpoint(runtime_type, checkpoint, pinned)
                    .await
            }
        }
    }

    async fn fork_on(&self, runtime_type: &str, source: &str, new_name: &str) -> RuntimeResult<()> {
        match self.by_type(runtime_type) {
            Some(rt) => {
                rt.fork(source, new_name).await?;
                self.remember(new_name, rt.name());
                Ok(())
            }
            None => self.inner.fork_on(runtime_type, source, new_name).await,
        }
    }

    async fn delete_checkpoint(&self, runtime_type: &str, checkpoint: &str) -> RuntimeResult<()> {
        match self.by_type(runtime_type) {
            Some(rt) => rt.delete(checkpoint).await,
            None => self.inner.delete_checkpoint(runtime_type, checkpoint).await,
        }
    }

    async fn export_checkpoint_on(
        &self,
        runtime_type: &str,
        checkpoint: &str,
    ) -> RuntimeResult<cua_vmm::CheckpointExport> {
        self.inner
            .export_checkpoint_on(runtime_type, checkpoint)
            .await
    }

    async fn import_dir_on(&self, runtime_type: &str, digest: &str) -> Option<std::path::PathBuf> {
        self.inner.import_dir_on(runtime_type, digest).await
    }

    async fn import_checkpoint_on(
        &self,
        runtime_type: &str,
        checkpoint: &str,
        state: serde_json::Value,
        qcow2_rebase: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    ) -> RuntimeResult<()> {
        self.inner
            .import_checkpoint_on(runtime_type, checkpoint, state, qcow2_rebase)
            .await
    }

    async fn snapshot_abis(&self) -> Vec<String> {
        self.inner.snapshot_abis().await
    }

    async fn start_fork(
        &self,
        runtime_type: &str,
        spec: &LocalStartSpec,
    ) -> RuntimeResult<LocalInstance> {
        match self.by_type(runtime_type) {
            Some(rt) => rt.resume(&spec.name).await,
            None => self.inner.start_fork(runtime_type, spec).await,
        }
    }

    fn fork_has_own_identity(&self, runtime_type: &str) -> bool {
        match self.by_type(runtime_type) {
            Some(_) => false,
            None => self.inner.fork_has_own_identity(runtime_type),
        }
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        let mut out = self.inner.list().await?;
        for p in self.registry.providers() {
            let rt = ExternalRuntime::new(p);
            match rt.list().await {
                Ok(rows) => {
                    for r in &rows {
                        self.remember(&r.name, rt.name());
                    }
                    out.extend(rows);
                }
                Err(e) => {
                    tracing::warn!(provider = %rt.name(), error = %e, "could not list a runtime's instances")
                }
            }
        }
        Ok(out)
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        match self.owner(name).await {
            Some(rt) => rt.status(name).await,
            None => self.inner.status(name).await,
        }
    }

    async fn status_on(&self, name: &str, runtime_type: &str) -> RuntimeResult<InstanceStatus> {
        match self.by_type(runtime_type) {
            Some(rt) => {
                self.remember(name, rt.name());
                rt.status(name).await
            }
            None => self.inner.status_on(name, runtime_type).await,
        }
    }

    async fn stop_reason(
        &self,
        name: &str,
        runtime_type: &str,
    ) -> RuntimeResult<Option<crate::StopReason>> {
        match self.by_type(runtime_type) {
            Some(rt) => rt.stop_reason(name, runtime_type).await,
            None => self.inner.stop_reason(name, runtime_type).await,
        }
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        match self.owner(name).await {
            Some(rt) => {
                let r = rt.delete(name).await;
                if matches!(r, Ok(()) | Err(RuntimeError::NotFound(_))) {
                    self.forget(name);
                }
                r
            }
            None => self.inner.delete(name).await,
        }
    }

    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        match self.owner(name).await {
            Some(rt) => rt.endpoints(name).await,
            None => self.inner.endpoints(name).await,
        }
    }

    async fn guest_tcp_listening(
        &self,
        name: &str,
        guest_port: u16,
    ) -> RuntimeResult<Option<bool>> {
        match self.owner(name).await {
            Some(rt) => rt.guest_tcp_listening(name, guest_port).await,
            None => self.inner.guest_tcp_listening(name, guest_port).await,
        }
    }

    async fn guest_exec(
        &self,
        name: &str,
        script: &str,
        timeout: Option<Duration>,
        sink: tokio::sync::mpsc::Sender<GuestOutput>,
    ) -> RuntimeResult<i64> {
        match self.owner(name).await {
            Some(rt) => rt.guest_exec(name, script, timeout, sink).await,
            None => self.inner.guest_exec(name, script, timeout, sink).await,
        }
    }

    async fn guest_screenshot(&self, name: &str) -> RuntimeResult<GuestScreenshot> {
        match self.owner(name).await {
            Some(rt) => rt.guest_screenshot(name).await,
            None => self.inner.guest_screenshot(name).await,
        }
    }

    async fn guest_display(&self, name: &str) -> RuntimeResult<crate::GuestDisplay> {
        match self.owner(name).await {
            Some(rt) => rt.guest_display(name).await,
            None => self.inner.guest_display(name).await,
        }
    }

    async fn build_image(
        &self,
        spec: &cua_image::build::BuildSpec,
        creds: Option<&cua_image::RegistryCredentials>,
    ) -> RuntimeResult<String> {
        self.inner.build_image(spec, creds).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_runtime_word_rule() {
        for ok in ["acme", "a", "macos-vms", "x1", "0abc"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", "Acme", "a_b", "a.b", &"a".repeat(33)] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_name(&"a".repeat(32)));
    }

    #[test]
    fn every_builtin_runtime_and_kind_word_is_reserved() {
        for r in Runtime::BUILTIN {
            assert!(is_reserved(r.as_str()), "{r}");
        }
        for k in ["auto", "container", "vm", "microvm"] {
            assert!(is_reserved(k), "{k}");
        }
        assert!(!is_reserved("acme"));
    }

    #[test]
    fn the_file_name_gives_the_runtime_name() {
        assert_eq!(name_of("cua-runtime-acme"), Some("acme"));
        assert_eq!(name_of("cua-runtimes"), None);
        assert_eq!(name_of("cua"), None);
    }

    #[test]
    fn runtime_schemas_name_providers_only_when_installed() {
        let mut schema =
            json!({"properties": {"runtime": {"type": "string", "description": "Which engine"}}});
        let before = schema.clone();
        assert!(!annotate_runtime_schema(&mut schema, &[]));
        assert_eq!(schema, before);
        let p = Arc::new(ExternalProvider {
            manifest: serde_json::from_value(json!({
                "protocol": 1, "name": "acme", "display_name": "macOS VMs", "kind": "vm"
            }))
            .unwrap(),
            path: PathBuf::from("/x/cua-runtime-acme"),
            kind: Kind::Vm,
        });
        assert!(annotate_runtime_schema(&mut schema, &[p]));
        assert_eq!(
            schema["properties"]["runtime"]["description"],
            "Which engine. Installed local runtime providers: acme (macOS VMs, vm)."
        );
        let mut none = json!({"properties": {"kind": {}}});
        let p2 = Arc::new(ExternalProvider {
            manifest: serde_json::from_value(json!({"protocol": 1, "name": "x", "kind": "vm"}))
                .unwrap(),
            path: PathBuf::from("/x"),
            kind: Kind::Vm,
        });
        assert!(!annotate_runtime_schema(&mut none, &[p2]));
    }

    #[test]
    fn wire_status_accepts_lowercase_and_serde_forms() {
        let s: WireStatus = serde_json::from_value(json!("running")).unwrap();
        assert_eq!(s.0, InstanceStatus::Running);
        let s: WireStatus = serde_json::from_value(json!("Stopped")).unwrap();
        assert_eq!(s.0, InstanceStatus::Stopped);
        let s: WireStatus = serde_json::from_value(json!({"Unknown": "booting"})).unwrap();
        assert_eq!(s.0, InstanceStatus::Unknown("booting".into()));
        let s: WireStatus = serde_json::from_value(json!("weird")).unwrap();
        assert_eq!(s.0, InstanceStatus::Unknown("weird".into()));
    }

    #[test]
    fn a_manifest_needs_protocol_1_its_own_name_and_a_kind() {
        let m = |protocol: u32, name: &str, kind: &str| Manifest {
            protocol,
            name: name.into(),
            display_name: String::new(),
            description: String::new(),
            kind: kind.into(),
            os: vec![],
            arch: vec![],
            handles_images: false,
            spacesd_port: DEFAULT_SPACESD_PORT,
            capabilities: ProviderCapabilities::default(),
            images: vec![],
        };
        assert_eq!(
            check_manifest("acme", m(1, "acme", "vm")).unwrap().1,
            Kind::Vm
        );
        assert!(check_manifest("acme", m(2, "acme", "vm")).is_err());
        assert!(check_manifest("acme", m(1, "other", "vm")).is_err());
        assert!(check_manifest("acme", m(1, "acme", "auto")).is_err());
        assert!(check_manifest("acme", m(1, "acme", "qemu")).is_err());
        assert_eq!(m(1, "acme", "vm").label(), "acme");
    }

    #[test]
    fn start_requests_carry_credentials_only_when_present() {
        let mut spec = LocalStartSpec {
            name: "s".into(),
            image: "registry.example/acme/image:1".into(),
            os: "macos".into(),
            cpus: 2,
            memory_mb: 4096,
            disk_size_gb: None,
            ports: vec![3211],
            env: BTreeMap::new(),
            command: None,
            ready_timeout: Duration::from_secs(90),
            firmware: None,
            sidecars: vec![],
            registry_credentials: None,
            container_runtime: None,
            kind: None,
            runtime: None,
            restrict_network: false,
            headless: false,
            spacesd: None,
            egress: Default::default(),
            gpu: None,
            probes: vec![],
            ephemeral: false,
            warm: None,
            mounts: vec![],
            forkable: false,
        };
        let v = start_request(&spec).unwrap();
        assert!(v.get("registry_credentials").is_none());
        assert_eq!(v["ready_timeout_secs"], 90);
        spec.registry_credentials = Some(crate::RegistryCredentials::new("u", "p"));
        let v = start_request(&spec).unwrap();
        assert_eq!(v["registry_credentials"]["username"], "u");
        assert_eq!(v["registry_credentials"]["password"], "p");
    }
}
