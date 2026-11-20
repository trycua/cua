//! The Spaces calls the reconciler makes: the seam between the pool and
//! Cua Spaces. `SdkSpaces` in cua-cli implements it over the cua SDK
//! (the cua daemon, or a runtime in this process); the tests use an
//! in-memory fake.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

use crate::egress::EgressPolicy;

/// A Space to create.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpaceSpec {
    /// Space name.
    pub name: String,
    /// Image.
    pub image: String,
    /// `local`, `auto` or `host:<computer>`.
    pub on: String,
    /// vCPUs.
    pub cpus: Option<u32>,
    /// Memory in MiB.
    pub memory_mb: Option<u64>,
    /// Disk in GiB.
    pub disk_gb: Option<u32>,
    /// Labels kept with the Space.
    pub labels: BTreeMap<String, String>,
    /// Egress policy, enforced by the host from the guest's first
    /// instruction; `None` for none.
    pub egress: Option<EgressPolicy>,
    /// How long the create may take until the Space is ready.
    pub timeout: Duration,
    /// A key that cancels the create while it runs.
    pub create_id: String,
    /// Host folders shared into the Space (the cache and repo mirror
    /// mounts). Only where [`Spaces::supports_mounts`].
    pub mounts: Vec<HostMount>,
}

/// A host folder shared into a Space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostMount {
    /// The folder on the host.
    pub host: std::path::PathBuf,
    /// Where the guest sees it (a macOS guest on Lume sees a share under
    /// `/Volumes/My Shared Files/<folder name>`).
    pub guest: String,
    /// The guest cannot write to it. Always true for the CI caches.
    pub read_only: bool,
}

/// A Space as listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpaceRow {
    /// Id (`local:<name>`, `relay:<computer>/<name>`, ...).
    pub id: String,
    /// Name.
    pub name: String,
    /// Labels.
    pub labels: BTreeMap<String, String>,
    /// `macos`, `linux`, `windows` or empty.
    pub os: String,
    /// `arm64`, `amd64` or empty.
    pub arch: String,
    /// `local`, `relay`, `direct`, `cloud`.
    pub provider: String,
    /// `running`, `stopped`, `suspended` or empty.
    pub power_state: String,
}

impl SpaceRow {
    /// A label's value.
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }

    /// A macOS VM on this computer that is not known to be off.
    pub fn is_local_macos_vm(&self) -> bool {
        self.os == "macos"
            && self.provider == "local"
            && !matches!(self.power_state.as_str(), "stopped" | "suspended")
    }
}

/// A shell command's result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShOutput {
    /// Stdout.
    pub stdout: String,
    /// Stderr.
    pub stderr: String,
    /// Exit code (`None` when killed or timed out).
    pub exit_code: Option<i32>,
}

impl ShOutput {
    /// Exited 0.
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Why a Spaces call failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SpacesError {
    /// The computer is at a capacity limit (two macOS VMs per Mac, the
    /// host's Space limit, its memory): wait for a slot, this is not a
    /// failure.
    #[error("no room for another Space: {0}")]
    LimitExceeded(String),
    /// No such Space.
    #[error("not found: {0}")]
    NotFound(String),
    /// The runtime cannot enforce an egress policy (macOS VMs, ...).
    #[error("{0}")]
    EgressUnsupported(String),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

/// The Spaces operations of a pool.
#[async_trait]
pub trait Spaces: Send + Sync {
    /// Creates a Space and waits until it is ready.
    async fn create(&self, spec: &SpaceSpec) -> Result<SpaceRow, SpacesError>;
    /// Deletes a Space (a missing one is [`SpacesError::NotFound`]).
    async fn delete(&self, id: &str) -> Result<(), SpacesError>;
    /// Every registered Space.
    async fn list(&self) -> Result<Vec<SpaceRow>, SpacesError>;
    /// Runs `script` with `/bin/sh -c` in the Space, as its user.
    async fn sh(&self, id: &str, script: &str, timeout: Duration) -> Result<ShOutput, SpacesError>;
    /// Writes `data` to the guest path `path` (parents created) with Unix
    /// `mode`, through the Space's file API (never a command line).
    async fn write_file(
        &self,
        id: &str,
        path: &str,
        data: &[u8],
        mode: u32,
    ) -> Result<(), SpacesError>;
    /// Copies the host file `local` to the guest path `path`.
    async fn upload_file(&self, id: &str, local: &Path, path: &str) -> Result<(), SpacesError>;
    /// Copies the guest file or folder `remote` into the host folder
    /// `local_dir`.
    async fn download(&self, id: &str, remote: &str, local_dir: &Path) -> Result<(), SpacesError>;
    /// Starts `program args` in the Space, detached, tagged `tag`.
    async fn spawn(
        &self,
        id: &str,
        program: &str,
        args: &[String],
        cwd: &str,
        tag: &str,
    ) -> Result<(), SpacesError>;
    /// Replaces the Space's egress policy.
    async fn set_egress(&self, id: &str, policy: &EgressPolicy) -> Result<(), SpacesError>;
    /// Whether [`SpaceSpec::mounts`] reach the guest. A backend that cannot
    /// share host folders returns false and the pool restores caches
    /// another way (an upload) or skips them.
    fn supports_mounts(&self) -> bool {
        false
    }
    /// Checkpoints Space `id` as `name` while it keeps running (memory
    /// and disk), pinned so pruning leaves it. `Spaces::create` with image
    /// `checkpoint:<name>` then starts forks of it.
    async fn checkpoint(&self, _id: &str, _name: &str) -> Result<(), SpacesError> {
        Err(SpacesError::Other(
            "this Spaces backend cannot take checkpoints".into(),
        ))
    }
    /// The names of the checkpoints this computer holds.
    async fn checkpoints(&self) -> Result<Vec<String>, SpacesError> {
        Ok(Vec::new())
    }
    /// Deletes a checkpoint (unpinning it first); a missing one is fine.
    async fn delete_checkpoint(&self, _name: &str) -> Result<(), SpacesError> {
        Ok(())
    }
    /// The macOS VMs running on this computer, Spaces or not (Apple allows
    /// two); `None` when it cannot be told.
    async fn macos_vms_running(&self) -> Option<u32> {
        None
    }
}
