//! Persistent sandbox state: `~/.cua/sandboxes/<name>.json`, in exactly the
//! JSON shape cua-sandbox's `sandbox_state.py` writes, so the Python CLI and
//! this crate can read each other's files.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Fields of a local sandbox state file (`sandbox_state.save`), in the same
/// order and with the same `null`s as the Python writer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LocalState {
    /// Sandbox name.
    pub name: String,
    /// Runtime identifier (`qemu`, `lume`, `docker`, ...).
    pub runtime_type: String,
    /// `Image.to_dict()`.
    pub image: Value,
    /// Host to dial.
    pub host: String,
    /// Host port of the sandbox's primary API.
    pub api_port: u16,
    /// Guest port → host port (JSON object with string keys).
    pub exposed_ports: Option<BTreeMap<String, u16>>,
    /// VNC port.
    pub vnc_port: Option<u16>,
    /// QMP port.
    pub qmp_port: Option<u16>,
    /// gRPC port (Android emulator).
    pub grpc_port: Option<u16>,
    /// ADB serial.
    pub adb_serial: Option<String>,
    /// Android SDK root.
    pub sdk_root: Option<String>,
    /// Disk path.
    pub disk_path: Option<String>,
    /// Guest OS type.
    pub os_type: Option<String>,
    /// VNC display number.
    pub vnc_display: Option<i64>,
    /// Memory in MiB.
    pub memory_mb: Option<u64>,
    /// vCPU count.
    pub cpu_count: Option<u32>,
    /// Guest architecture.
    pub arch: Option<String>,
    /// `running` or `stopped`, ...
    pub status: String,
    /// ISO-8601 creation time with `+00:00`.
    pub created_at: String,
    /// Fields other writers added (preserved on rewrite).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A named Fleet claim (`sandbox_state.save_fleet_claim`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FleetState {
    /// Claim / sandbox name.
    pub name: String,
    /// Always `"fleet"`.
    pub runtime_type: String,
    /// Pool the claim belongs to.
    pub pool_name: String,
    /// Status.
    pub status: String,
    /// Creation time.
    pub created_at: String,
    /// Other fields (for example `namespace`, `service`).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One state file.
// Plain data read and written a handful of times; not worth boxing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SandboxState {
    /// Fleet claim.
    Fleet(FleetState),
    /// Local runtime (or direct connection, `runtime_type: "direct"`).
    Local(LocalState),
}

impl SandboxState {
    /// Name.
    pub fn name(&self) -> &str {
        match self {
            SandboxState::Fleet(f) => &f.name,
            SandboxState::Local(l) => &l.name,
        }
    }

    /// `runtime_type`.
    pub fn runtime_type(&self) -> &str {
        match self {
            SandboxState::Fleet(f) => &f.runtime_type,
            SandboxState::Local(l) => &l.runtime_type,
        }
    }

    /// `status`.
    pub fn status(&self) -> &str {
        match self {
            SandboxState::Fleet(f) => &f.status,
            SandboxState::Local(l) => &l.status,
        }
    }

    fn set_status(&mut self, status: &str) {
        match self {
            SandboxState::Fleet(f) => f.status = status.into(),
            SandboxState::Local(l) => l.status = status.into(),
        }
    }
}

/// `datetime.now(timezone.utc).isoformat()`.
pub fn python_utc_now() -> String {
    let s = humantime::format_rfc3339_micros(SystemTime::now()).to_string();
    match s.strip_suffix('Z') {
        Some(base) => format!("{base}+00:00"),
        None => s,
    }
}

/// `Image.from_registry(ref, os_type=..., kind=...).to_dict()`.
pub fn registry_image_dict(reference: &str, os_type: &str, kind: Option<&str>) -> Value {
    serde_json::json!({
        "os_type": os_type,
        "distro": "registry",
        "version": "latest",
        "kind": kind,
        "layers": [],
        "registry": reference,
    })
}

/// Writes `data` to `path` through a temp file created `0600` (Unix), then
/// renamed, so a token in it is never readable by others, even briefly.
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write as _;
    // A test must never write the user's real sandbox state.
    cua_home::guard_write(path)?;
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(data)?;
    drop(f);
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// The state directory (`~/.cua/sandboxes` by default).
#[derive(Clone, Debug)]
pub struct StateStore {
    dir: PathBuf,
}

impl Default for StateStore {
    fn default() -> Self {
        Self::new(default_state_dir())
    }
}

/// `~/.cua/sandboxes` (`$CUA_HOME/sandboxes` when `CUA_HOME` is set).
pub fn default_state_dir() -> PathBuf {
    cua_home::cua_home().join("sandboxes")
}

/// Directory of the ephemeral-sandbox leases, inside the state directory
/// (`.ephemeral/`, invisible to state listings, which read `*.json`).
pub const LEASE_DIR: &str = ".ephemeral";

/// The lease of an ephemeral local sandbox: which process owns it. Written
/// before the instance is created and removed when it is deleted, so an
/// instance whose owner died (a crash, a kill) is found and reaped by the
/// daemon or the next CLI run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EphemeralLease {
    /// Instance name (`cua-eph-<hex>`).
    pub name: String,
    /// Owning process.
    pub pid: u32,
    /// Unix seconds the lease was taken.
    pub created_at: u64,
}

impl StateStore {
    /// A store rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Root directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(Error::InvalidArgument(format!(
                "invalid sandbox name {name:?}"
            )));
        }
        Ok(self.dir.join(format!("{name}.json")))
    }

    /// Writes (or overwrites) a state file (`indent=2`, like Python).
    /// Owner-only (`0600`): a state file can hold a spacesd token.
    pub fn save(&self, state: &SandboxState) -> Result<()> {
        cua_home::guard_write(&self.dir)?;
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(state.name())?;
        write_private(&path, serde_json::to_string_pretty(state)?.as_bytes())
    }

    /// Makes a state file readable by its owner only (it holds a token).
    pub fn restrict(&self, name: &str) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let path = self.path(name)?;
            if path.exists() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        #[cfg(not(unix))]
        let _ = name;
        Ok(())
    }

    /// `save_fleet_claim(name, pool_name)`.
    pub fn save_fleet_claim(&self, name: &str, pool_name: &str) -> Result<()> {
        self.save(&SandboxState::Fleet(FleetState {
            name: name.into(),
            runtime_type: "fleet".into(),
            pool_name: pool_name.into(),
            status: "running".into(),
            created_at: python_utc_now(),
            extra: Map::new(),
        }))
    }

    /// Loads a state file; `None` when missing or unreadable (as Python).
    pub fn load(&self, name: &str) -> Option<SandboxState> {
        let text = std::fs::read_to_string(self.path(name).ok()?).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Loads the raw JSON object.
    pub fn load_raw(&self, name: &str) -> Option<Map<String, Value>> {
        let text = std::fs::read_to_string(self.path(name).ok()?).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Merges `fields` into an existing file (`sandbox_state.update`).
    pub fn update(&self, name: &str, fields: Map<String, Value>) -> Result<()> {
        let Some(mut raw) = self.load_raw(name) else {
            return Ok(());
        };
        raw.extend(fields);
        write_private(
            &self.path(name)?,
            serde_json::to_string_pretty(&raw)?.as_bytes(),
        )
    }

    /// Sets `status`.
    pub fn set_status(&self, name: &str, status: &str) -> Result<()> {
        if let Some(mut s) = self.load(name) {
            s.set_status(status);
            self.save(&s)?;
        }
        Ok(())
    }

    /// Removes a state file (missing is fine).
    pub fn delete(&self, name: &str) -> Result<()> {
        match std::fs::remove_file(self.path(name)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn lease_path(&self, name: &str) -> Result<PathBuf> {
        let p = self.path(name)?;
        Ok(self.dir.join(LEASE_DIR).join(p.file_name().expect("named")))
    }

    /// Records that process `pid` owns ephemeral instance `name`.
    pub fn write_lease(&self, name: &str, pid: u32) -> Result<()> {
        let path = self.lease_path(name)?;
        cua_home::guard_write(&path)?;
        std::fs::create_dir_all(path.parent().expect("in the lease dir"))?;
        let lease = EphemeralLease {
            name: name.into(),
            pid,
            created_at: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        write_private(&path, serde_json::to_string_pretty(&lease)?.as_bytes())
    }

    /// Drops the lease of `name` (missing is fine).
    pub fn remove_lease(&self, name: &str) -> Result<()> {
        match std::fs::remove_file(self.lease_path(name)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Every readable lease.
    pub fn leases(&self) -> Vec<EphemeralLease> {
        let Ok(rd) = std::fs::read_dir(self.dir.join(LEASE_DIR)) else {
            return vec![];
        };
        let mut out: Vec<EphemeralLease> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|b| serde_json::from_slice(&b).ok())
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Every readable state file.
    pub fn list_all(&self) -> Vec<SandboxState> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        let mut out: Vec<SandboxState> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| serde_json::from_str(&t).ok())
            .collect();
        out.sort_by(|a, b| a.name().cmp(b.name()));
        out
    }
}
