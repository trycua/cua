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
    Arch, BackendKind, CheckpointInfo, Endpoints, ImageSource, Instance, InstanceSummary,
    StartSpec, Status,
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
