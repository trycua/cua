//! The local-runtime interface `cua-sandbox-core` needs.
//!
//! It mirrors `cua-vmm`'s `Runtime` trait (start / stop / suspend / resume /
//! fork / checkpoint / list / status / delete / endpoints) with the subset of
//! types the sandbox layer uses, so the orchestrator can plug `cua-vmm` in
//! with a thin adapter without this crate depending on it.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

/// Result of a runtime call; errors are plain strings (the adapter maps
/// `cua_vmm::VmmError` into this).
pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

/// A local-runtime failure.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    /// No such instance.
    #[error("instance {0} not found")]
    NotFound(String),
    /// The backend cannot do this.
    #[error("{backend} does not support {op}")]
    Unsupported {
        /// Backend name.
        backend: String,
        /// Operation.
        op: String,
    },
    /// The image cannot run on any local backend (for example a macOS image
    /// in no Lume format). The message says what to use instead.
    #[error("{0}")]
    UnsupportedImage(String),
    /// A pull, build or VM create would leave less than the configured
    /// minimum free disk space. The message says how much and how to free
    /// space (`cua cache prune`).
    #[error("{0}")]
    InsufficientDisk(String),
    /// The kind or runtime asked for does not fit the image or this host;
    /// the error lists what would.
    #[error("{0}")]
    InvalidPlacement(crate::placement::PlacementError),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

/// Instance status (mirror of `cua_vmm::Status`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceStatus {
    /// Running.
    Running,
    /// Paused in memory.
    Paused,
    /// Stopped (disk kept).
    Stopped,
    /// Being created or pulled.
    Provisioning,
    /// Backend-specific.
    Unknown(String),
}

/// What to start (subset of `cua_vmm::StartSpec`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalStartSpec {
    /// Instance name.
    pub name: String,
    /// Image reference (OCI ref, Lume image, local path).
    pub image: String,
    /// Guest OS (`linux`, `macos`, `windows`).
    pub os: String,
    /// vCPUs.
    pub cpus: u32,
    /// Memory (MiB).
    pub memory_mb: u64,
    /// Grow the VM's disk to at least this many GiB (VM backends; container
    /// backends refuse it). `None`: the image's size.
    #[serde(default)]
    pub disk_size_gb: Option<u32>,
    /// Guest TCP ports to publish on the host.
    pub ports: Vec<u16>,
    /// Environment for the guest (containers) or cloud-init.
    pub env: BTreeMap<String, String>,
    /// The sandbox's command (argv), replacing the image's entrypoint
    /// (containers) or run by cloud-init after boot (Linux VMs). `None`: the
    /// image decides.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// How long the backend may take to report running.
    pub ready_timeout: Duration,
    /// Boot firmware for VM backends (`bios` / `efi`); `None` lets the
    /// backend decide. Set from a Fleet template's `firmware`.
    #[serde(default)]
    pub firmware: Option<String>,
    /// Extra containers sharing the instance's network namespace
    /// (container backends; VM backends refuse them).
    #[serde(default)]
    pub sidecars: Vec<crate::Sidecar>,
    /// Credentials for pulling a private image. Never serialized.
    #[serde(skip)]
    pub registry_credentials: Option<crate::RegistryCredentials>,
    /// Container backends: `runc` or `gvisor`; `None` = gVisor when
    /// available. Sidecars where gVisor would run need an explicit `runc`.
    #[serde(default)]
    pub container_runtime: Option<String>,
    /// The kind asked for (`container`, `vm`); `None` or `auto` decides
    /// from the image (see [`crate::placement`]).
    #[serde(default)]
    pub kind: Option<String>,
    /// The runtime asked for (`gvisor`, `runc`, `qemu`, `lume`); `None` or
    /// `auto` picks the safest one for the kind.
    #[serde(default)]
    pub runtime: Option<String>,
    /// Cut guest egress (QEMU `restrict=on`, published ports still work).
    /// `false` (the default): outbound network. Backends that cannot honour
    /// it refuse it.
    #[serde(default)]
    pub restrict_network: bool,
    /// A GPU option of the runtime (see [`crate::gpu`]); `None`: no GPU.
    #[serde(default)]
    pub gpu: Option<String>,
    /// Readiness probes the backend should wait for before `start`
    /// returns. Backends that can see the guest side of a forwarded port
    /// (QEMU user networking) check them more precisely than the caller can.
    #[serde(default)]
    pub probes: Vec<LocalProbe>,
    /// An ephemeral sandbox (deleted with its handle; reaped when its
    /// process dies). Container backends label it `ai.cua.ephemeral=true`.
    #[serde(default)]
    pub ephemeral: bool,
}

/// A readiness probe handed to the local backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalProbe {
    /// Guest port.
    pub port: u16,
    /// `GET` this path for a 2xx/3xx; `None` = TCP only.
    #[serde(default)]
    pub http_path: Option<String>,
}

/// How to reach a running instance (subset of `cua_vmm::Endpoints`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalEndpoints {
    /// Host to dial for published ports.
    pub host: String,
    /// Guest port → host port.
    pub ports: BTreeMap<u16, u16>,
    /// VNC `host:port`, when the VMM exposes one (agentless access).
    pub vnc: Option<String>,
    /// QMP `host:port` or socket path.
    pub qmp: Option<String>,
    /// SSH `host:port`, when the image has it.
    pub ssh: Option<String>,
    /// Host file the guest serial console is logged to (QEMU).
    #[serde(default)]
    pub serial_log: Option<String>,
    /// Container id (container backends), for the engine's logs.
    #[serde(default)]
    pub container_id: Option<String>,
}

/// A started instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalInstance {
    /// Name.
    pub name: String,
    /// Backend identifier, persisted as `runtime_type` (`qemu`, `lume`,
    /// `docker`, `gvisor`, ...).
    pub backend: String,
    /// Status.
    pub status: InstanceStatus,
    /// Endpoints.
    pub endpoints: LocalEndpoints,
}

/// The image a sandbox runs, as the one resolver ([`cua_image::resolve`])
/// pinned it at create time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInfo {
    /// The reference as requested, normalised
    /// (`docker.io/library/python:3.12-slim`).
    pub reference: String,
    /// `registry/repo@sha256:...` of the variant that runs.
    pub pinned_ref: String,
    /// The digest in `pinned_ref`.
    pub digest: String,
    /// `rootfs`, `containerdisk` or `lume`.
    pub variant: String,
    /// Architecture that runs (`amd64`/`arm64`), when known.
    #[serde(default)]
    pub arch: Option<String>,
    /// Guest OS: `linux`, `windows` or `macos`.
    #[serde(default)]
    pub os: String,
    /// Whether it runs emulated (no build for the host's architecture).
    #[serde(default)]
    pub emulated: bool,
    /// Whether the image declares cua-spacesd (`ai.cua.spacesd` label or
    /// annotation, or a published 3211); `None` when it does not say.
    /// Readiness waits for spacesd only when this is `Some(true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacesd: Option<bool>,
}

impl From<&cua_image::ResolvedImage> for ImageInfo {
    fn from(r: &cua_image::ResolvedImage) -> Self {
        Self {
            reference: r.reference.clone(),
            pinned_ref: r.pinned_ref.clone(),
            digest: r.digest.clone(),
            variant: r.variant.as_str().into(),
            arch: r.arch.clone(),
            os: r.os.clone(),
            emulated: r.emulated,
            spacesd: r.spacesd,
        }
    }
}

/// Output of a guest command run without an in-guest agent
/// ([`LocalRuntime::guest_exec`]), forwarded as it arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuestOutput {
    /// Bytes the command wrote to stdout.
    Stdout(Vec<u8>),
    /// Bytes the command wrote to stderr.
    Stderr(Vec<u8>),
}

/// A guest framebuffer captured without an in-guest agent
/// ([`LocalRuntime::guest_screenshot`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestScreenshot {
    /// PNG bytes.
    pub png: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How it was captured (`vnc`).
    pub via: String,
}

/// The guest display, reachable without an in-guest agent
/// ([`LocalRuntime::guest_display`]).
#[derive(Clone, PartialEq, Eq)]
pub struct GuestDisplay {
    /// `vnc://[:PASSWORD@]HOST:PORT`. May carry the VNC password: never log
    /// it; [`GuestDisplay::redacted_url`] is safe to show.
    pub url: String,
    /// How the display is reached (`vnc`).
    pub via: String,
    /// A host command that opens a viewer on it (`lume attach <vm>`), when
    /// the runtime has one.
    pub open_command: Option<Vec<String>>,
}

impl GuestDisplay {
    /// [`GuestDisplay::url`] with any password replaced by `****`.
    pub fn redacted_url(&self) -> String {
        match (self.url.split_once("://"), self.url.rfind('@')) {
            (Some((scheme, _)), Some(at)) => format!("{scheme}://****{}", &self.url[at..]),
            _ => self.url.clone(),
        }
    }
}

impl std::fmt::Debug for GuestDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestDisplay")
            .field("url", &self.redacted_url())
            .field("via", &self.via)
            .field("open_command", &self.open_command)
            .finish()
    }
}

/// One row of [`LocalRuntime::list`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSummary {
    /// Name.
    pub name: String,
    /// Backend.
    pub backend: String,
    /// Status.
    pub status: InstanceStatus,
}

/// A local VM / container runtime.
#[async_trait]
pub trait LocalRuntime: Send + Sync {
    /// Backend identifier (persisted as `runtime_type`).
    fn backend(&self) -> String;

    /// Create (if needed) and boot an instance. Returns once the backend
    /// reports it running; guest readiness is the sandbox layer's job.
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance>;

    /// [`LocalRuntime::start`], plus the image as the runtime resolved and
    /// pinned it (`None`: not resolved from a registry). The default
    /// resolves nothing.
    async fn start_resolved(
        &self,
        spec: &LocalStartSpec,
    ) -> RuntimeResult<(LocalInstance, Option<ImageInfo>)> {
        Ok((self.start(spec).await?, None))
    }

    /// The GPU options of each runtime this can start (Lume, QEMU,
    /// containers), for [`LocalStartSpec::gpu`]. Default: none.
    async fn gpu_support(&self) -> Vec<crate::gpu::GpuSupport> {
        Vec::new()
    }

    /// Stops, keeping disk and state.
    async fn stop(&self, name: &str) -> RuntimeResult<()>;

    /// Pauses in memory.
    async fn suspend(&self, name: &str) -> RuntimeResult<()> {
        Err(RuntimeError::Unsupported {
            backend: self.backend(),
            op: format!("suspend {name}"),
        })
    }

    /// Resumes a paused (or starts a stopped) instance.
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance>;

    /// How an instance of `runtime_type` (the state file's engine word:
    /// `lume`, `runc`, `gvisor`, `qemu`, ...) turns off and on again
    /// (`None`: it cannot). Default: none.
    fn power_control(&self, runtime_type: &str) -> Option<crate::PowerControl> {
        let _ = runtime_type;
        None
    }

    /// Copy-on-write clone of a stopped instance or checkpoint.
    async fn fork(&self, source: &str, new_name: &str) -> RuntimeResult<()> {
        Err(RuntimeError::Unsupported {
            backend: self.backend(),
            op: format!("fork {source} -> {new_name}"),
        })
    }

    /// Captures a checkpoint.
    async fn checkpoint(&self, name: &str, checkpoint: &str) -> RuntimeResult<()> {
        Err(RuntimeError::Unsupported {
            backend: self.backend(),
            op: format!("checkpoint {name} -> {checkpoint}"),
        })
    }

    /// All managed instances.
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>>;

    /// Status of one instance.
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus>;

    /// Status of one instance that a state file says `runtime_type`
    /// (`qemu`, `lume`, `gvisor`, ...) runs. A backend that routes by
    /// runtime asks only that engine, so [`RuntimeError::NotFound`] means
    /// the instance is gone rather than that some engine was unreachable.
    async fn status_on(&self, name: &str, runtime_type: &str) -> RuntimeResult<InstanceStatus> {
        let _ = runtime_type;
        self.status(name).await
    }

    /// Stops and removes an instance.
    async fn delete(&self, name: &str) -> RuntimeResult<()>;

    /// Endpoints of a running instance.
    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints>;

    /// Whether something listens on TCP `guest_port` inside the guest, seen
    /// from the guest side of the host forwarder (container netns, QEMU
    /// slirp). `None`: the backend cannot tell, and the sandbox layer falls
    /// back to a host-side check. Docker's userland proxy and slirp accept on
    /// the host before the guest listens, so a bare connect proves nothing.
    async fn guest_tcp_listening(
        &self,
        name: &str,
        guest_port: u16,
    ) -> RuntimeResult<Option<bool>> {
        let _ = (name, guest_port);
        Ok(None)
    }

    /// Runs `script` with `/bin/sh -c` in the guest without an in-guest
    /// agent (for example over `lume ssh` for a macOS Lume VM), forwarding
    /// its output to `sink` as it arrives, and returns its exit code. The
    /// fallback for sandboxes without cua-spacesd; backends without such a
    /// path return [`RuntimeError::Unsupported`].
    async fn guest_exec(
        &self,
        name: &str,
        script: &str,
        timeout: Option<Duration>,
        sink: tokio::sync::mpsc::Sender<GuestOutput>,
    ) -> RuntimeResult<i64> {
        let _ = (script, timeout, sink);
        Err(RuntimeError::Unsupported {
            backend: self.backend(),
            op: format!("agentless exec in {name}"),
        })
    }

    /// Captures the guest framebuffer without an in-guest agent (the VMM's
    /// VNC endpoint). [`RuntimeError::Unsupported`] when there is none.
    async fn guest_screenshot(&self, name: &str) -> RuntimeResult<GuestScreenshot> {
        Err(RuntimeError::Unsupported {
            backend: self.backend(),
            op: format!("agentless screenshots of {name}"),
        })
    }

    /// The guest display (the VMM's VNC endpoint) and how to open it.
    /// [`RuntimeError::Unsupported`] when there is none.
    async fn guest_display(&self, name: &str) -> RuntimeResult<GuestDisplay> {
        Err(RuntimeError::Unsupported {
            backend: self.backend(),
            op: format!("an agentless display for {name}"),
        })
    }

    /// Builds `spec` (image layers on the container image `spec.from`)
    /// into this machine's container engine and returns the reference to
    /// run (`container:<ref>`). Cached by the same content hash as remote
    /// builds (`cua-b-<hash>`), so an identical spec is not built again.
    /// VM images cannot take container layers
    /// ([`RuntimeError::UnsupportedImage`]).
    async fn build_image(
        &self,
        spec: &cua_fleet::BuildSpec,
        creds: Option<&cua_fleet::RegistryCredentials>,
    ) -> RuntimeResult<String> {
        let _ = (spec, creds);
        Err(RuntimeError::Unsupported {
            backend: self.backend(),
            op: "image builds".into(),
        })
    }
}

#[cfg(test)]
mod guest_display_tests {
    use super::GuestDisplay;

    #[test]
    fn debug_and_redacted_url_never_show_the_password() {
        let d = GuestDisplay {
            url: "vnc://:s3cret@127.0.0.1:5901".into(),
            via: "vnc".into(),
            open_command: Some(vec!["lume".into(), "attach".into(), "vm".into()]),
        };
        assert_eq!(d.redacted_url(), "vnc://****@127.0.0.1:5901");
        for shown in [format!("{d:?}"), format!("{d:#?}")] {
            assert!(!shown.contains("s3cret"), "leaked in {shown}");
        }
        let plain = GuestDisplay {
            url: "vnc://127.0.0.1:5901".into(),
            ..d
        };
        assert_eq!(plain.redacted_url(), "vnc://127.0.0.1:5901");
    }
}
