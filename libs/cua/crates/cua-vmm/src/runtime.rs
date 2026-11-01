//! The [`Runtime`] trait every local backend implements.
//!
//! Port of `cua_sandbox/runtime/base.py`, with one deliberate difference:
//! readiness is daemon-agnostic. `start` returns once the backend reports the
//! instance running **and** every user-declared [`crate::Probe`] passes. No
//! runtime waits for computer-server, cua-spacesd or any other agent.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{Result, VmmError};
use crate::exec::GuestExec;
use crate::types::{
    Arch, BackendKind, CheckpointInfo, ClaimIdentity, Endpoints, ImageSource, Instance,
    InstanceSummary, StartSpec, Status,
};

/// A local sandbox backend (Lume, QEMU, OCI container, ...).
///
/// Names are the primary key. `ensure_base` and `fork` create *stopped*
/// instances; `start` creates-if-absent and boots. Every method is idempotent
/// where that is meaningful (`stop` on a stopped instance is `Ok`).
#[async_trait]
pub trait Runtime: Send + Sync {
    /// Which backend this is.
    fn kind(&self) -> BackendKind;

    /// Make sure a stopped, never-started base instance named `base_name`
    /// exists for `image` (pulling or importing it the first time). Bases are
    /// fork sources only.
    async fn ensure_base(&self, image: &ImageSource, base_name: &str) -> Result<CheckpointInfo>;

    /// Create (if needed) and boot an instance, then wait until it is ready.
    async fn start(&self, spec: &StartSpec) -> Result<Instance>;

    /// Stop an instance, keeping its disk/state so it can be started again.
    async fn stop(&self, name: &str) -> Result<()>;

    /// Pause an instance in memory.
    async fn suspend(&self, name: &str) -> Result<()> {
        let _ = name;
        Err(VmmError::Unsupported {
            backend: self.kind().as_str(),
            op: "suspend",
        })
    }

    /// Resume a paused instance.
    async fn resume(&self, name: &str) -> Result<Instance> {
        let _ = name;
        Err(VmmError::Unsupported {
            backend: self.kind().as_str(),
            op: "resume",
        })
    }

    /// Gives an instance another shape (vCPUs, memory, a larger disk): it
    /// is stopped if it runs and boots again from its own disk. Only VMs
    /// that can start again from their disk offer it.
    async fn resize(&self, name: &str, shape: &crate::Resize) -> Result<Instance> {
        let _ = (name, shape);
        Err(VmmError::Unsupported {
            backend: self.kind().as_str(),
            op: "resize",
        })
    }

    /// Bind a running warm-pool member (started with
    /// [`crate::StartSpec::pool_member`]) to a Space: it is renamed
    /// `new_name`, receives `identity` (token, hostname; fresh entropy,
    /// clock and network lease where memory was restored) and is returned
    /// running and ready. A backend without warm pools says so.
    async fn claim(
        &self,
        member: &str,
        new_name: &str,
        identity: &ClaimIdentity,
    ) -> Result<Instance> {
        let _ = (member, new_name, identity);
        Err(VmmError::Unsupported {
            backend: self.kind().as_str(),
            op: "claim",
        })
    }

    /// Create a new stopped instance `new_name` from a stopped base, checkpoint
    /// or instance `source`, copy-on-write where the backend can.
    async fn fork(&self, source: &str, new_name: &str) -> Result<()>;

    /// Capture the current state of `name` as `checkpoint`. The checkpoint can
    /// later be passed to [`Runtime::fork`].
    async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo>;

    /// Delete a checkpoint created by [`Runtime::checkpoint`].
    async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
        let _ = checkpoint;
        Err(VmmError::Unsupported {
            backend: self.kind().as_str(),
            op: "delete_checkpoint",
        })
    }

    /// Checkpoint `checkpoint` as files, to copy it to another host
    /// (snapshot sync). A backend that cannot says so.
    async fn export_checkpoint(&self, checkpoint: &str) -> Result<CheckpointExport> {
        let _ = checkpoint;
        Err(VmmError::Unsupported {
            backend: self.kind().as_str(),
            op: "export_checkpoint",
        })
    }

    /// Where the files of a copied checkpoint of manifest `digest` go
    /// before [`Runtime::import_checkpoint`] (`None`: no imports).
    fn import_dir(&self, digest: &str) -> Option<std::path::PathBuf> {
        let _ = digest;
        None
    }

    /// Makes a copied checkpoint `checkpoint` of this backend's: `state` is
    /// its [`CheckpointExport::state`] with paths of this host (files laid
    /// out under [`Runtime::import_dir`]), and each `(overlay, backing)` in
    /// `qcow2_rebase` a disk layer to point at its backing file's copy.
    async fn import_checkpoint(
        &self,
        checkpoint: &str,
        state: serde_json::Value,
        qcow2_rebase: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    ) -> Result<()> {
        let _ = (checkpoint, state, qcow2_rebase);
        Err(VmmError::Unsupported {
            backend: self.kind().as_str(),
            op: "import_checkpoint",
        })
    }

    /// The checkpoint ABIs this backend restores on this host
    /// ([`CheckpointExport::abi`]); empty: none.
    fn snapshot_abis(&self) -> Vec<String> {
        Vec::new()
    }

    /// Pins (`pinned`) or unpins checkpoint `checkpoint`: a pinned one is
    /// a fork source to keep (a world forks are made from), so
    /// [`Runtime::delete_checkpoint`] refuses it until it is unpinned.
    async fn pin_checkpoint(&self, checkpoint: &str, pinned: bool) -> Result<()> {
        let _ = (checkpoint, pinned);
        Err(VmmError::Unsupported {
            backend: self.kind().as_str(),
            op: "pin_checkpoint",
        })
    }

    /// All instances this backend manages (including bases and checkpoints
    /// where the backend stores them as instances).
    async fn list(&self) -> Result<Vec<InstanceSummary>>;

    /// Current status of one instance.
    async fn status(&self, name: &str) -> Result<Status>;

    /// Stop (if running) and permanently remove an instance and its state.
    async fn delete(&self, name: &str) -> Result<()>;

    /// How to reach a running instance.
    async fn endpoints(&self, name: &str) -> Result<Endpoints>;

    /// Whether a TCP listener on `guest_port` exists *inside* the guest, as
    /// seen from the guest side of any host forwarder. `None` when the
    /// backend has no such view (callers then fall back to a host-side
    /// check). Docker's userland proxy and QEMU slirp both accept on the host
    /// before the guest listens, so a host-side connect alone proves nothing.
    async fn guest_tcp_listening(&self, name: &str, guest_port: u16) -> Result<Option<bool>> {
        let _ = (name, guest_port);
        Ok(None)
    }

    /// Agentless command execution inside the guest, when the backend has a
    /// path for it (container exec, SSH, `lume ssh`). `None` otherwise.
    async fn guest_exec(&self, name: &str) -> Result<Option<Arc<dyn GuestExec>>> {
        let _ = name;
        Ok(None)
    }

    /// Replaces instance `name`'s whole egress policy ([`crate::egress`];
    /// empty removes it), enforced by the host outside the guest. A running
    /// instance gets it at once; a stopped one at its next start. The
    /// backend keeps it with the instance (restarts, resumes and forks keep
    /// it). Backends with no host-side point refuse it
    /// ([`crate::egress::support`]); none filters inside the guest.
    async fn set_egress(
        &self,
        name: &str,
        policy: &crate::egress::EgressPolicy,
    ) -> Result<crate::egress::Enforcement> {
        let _ = (name, policy);
        Err(
            crate::egress::refusal(self.kind()).unwrap_or(VmmError::Unsupported {
                backend: self.kind().as_str(),
                op: "an egress policy (this backend has no host-side point to enforce one)",
            }),
        )
    }

    /// The egress policy kept with instance `name` (empty: none).
    async fn egress(&self, name: &str) -> Result<crate::egress::EgressPolicy> {
        let _ = name;
        Ok(Default::default())
    }
}

/// Resolves an OCI reference to a local bootable disk (qcow2). Implemented by
/// `cua-image` for KubeVirt containerDisks; kept as a trait here so `cua-vmm`
/// does not depend on the registry client.
#[async_trait]
pub trait DiskResolver: Send + Sync {
    /// Return a local disk for `reference` (pulled and cached as needed).
    async fn resolve(&self, reference: &str, arch: Arch) -> Result<PathBuf>;

    /// [`Self::resolve`] with registry credentials for a private image. The
    /// default refuses credentials rather than pulling without them; a
    /// resolver that can authenticate overrides it.
    async fn resolve_with_credentials(
        &self,
        reference: &str,
        arch: Arch,
        creds: Option<&crate::RegistryCredentials>,
    ) -> Result<PathBuf> {
        match creds {
            None => self.resolve(reference, arch).await,
            Some(_) => Err(crate::VmmError::Unsupported {
                backend: "qemu",
                op: "registry credentials with this disk resolver (use cua-image's \
                     ContainerDiskResolver)",
            }),
        }
    }
}

/// Gives a guest restored from a memory snapshot that other guests share
/// its own identity, through the guest's own agent (cua-spacesd). Kept as a
/// trait so `cua-vmm` does not depend on that agent's client: a backend
/// that restores shared memory only does so when one is configured.
#[async_trait]
pub trait GuestIdentity: Send + Sync {
    /// The guest reachable at `endpoints` accepts `current` as its token.
    /// Rotate it to `identity.token`, set its hostname and step its clock
    /// to now; with `new_machine` also re-seed its random state and make it
    /// a new machine (its own machine-id and per-machine keys). Retries
    /// until `timeout` while the guest's agent is not answering yet.
    async fn renew(
        &self,
        endpoints: &Endpoints,
        current: &str,
        identity: &ClaimIdentity,
        new_machine: bool,
        timeout: std::time::Duration,
    ) -> Result<()>;

    /// Steps the clock of a guest that was paused (the source of a memory
    /// snapshot) to the host's. Its token is `token`.
    async fn resync_clock(&self, endpoints: &Endpoints, token: &str) -> Result<()> {
        let _ = (endpoints, token);
        Ok(())
    }
}

/// What an OCI image runs (its config's `Entrypoint`, `Cmd`, `Env`,
/// `WorkingDir` and `User`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OciRunConfig {
    #[serde(default)]
    pub entrypoint: Vec<String>,
    #[serde(default)]
    pub cmd: Vec<String>,
    /// `KEY=value` entries.
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
}

/// An OCI image's layers, pulled to local files.
#[derive(Clone, Debug)]
pub struct RootfsLayers {
    /// The platform manifest's digest (`sha256:...`): the rootfs's identity.
    pub digest: String,
    /// Layer blobs (gzip or plain tar), bottom first.
    pub layers: Vec<PathBuf>,
    pub config: OciRunConfig,
}

/// Pulls an OCI image's layers and config (implemented by `cua-image`; a
/// trait here so `cua-vmm` does not depend on the registry client). The
/// microVM backend builds its root filesystem from them.
#[async_trait]
pub trait RootfsResolver: Send + Sync {
    async fn resolve_rootfs(
        &self,
        reference: &str,
        arch: Arch,
        creds: Option<&crate::RegistryCredentials>,
    ) -> Result<RootfsLayers>;
}

/// A checkpoint as files, for copying it to another host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckpointExport {
    /// The backend's state for it (JSON), with paths as they are here.
    pub state: serde_json::Value,
    /// Its files (each once), in a stable order.
    pub files: Vec<ExportFile>,
    /// Directories named in `state` whose files the backend finds by name.
    pub dirs: Vec<std::path::PathBuf>,
    /// Which hosts can restore it: `<backend>/<accelerator>/<arch>`.
    pub abi: String,
}

/// One file of a [`CheckpointExport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportFile {
    /// Where it is.
    pub path: std::path::PathBuf,
    /// The directory of [`CheckpointExport::dirs`] it is in, if any.
    pub dir: Option<std::path::PathBuf>,
    /// A qcow2 image: its backing file (another file of the export).
    pub qcow2_backing: Option<std::path::PathBuf>,
}
