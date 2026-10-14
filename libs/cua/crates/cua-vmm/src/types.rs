//! Backend-neutral data types: what to start, and how to reach it afterwards.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::VmmError;

/// CPU architecture of a guest or host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// Architecture of the machine this process runs on.
    pub fn host() -> Self {
        if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::X86_64
        }
    }

    /// OCI platform name (`amd64` / `arm64`).
    pub fn oci(self) -> &'static str {
        match self {
            Arch::X86_64 => "amd64",
            Arch::Aarch64 => "arm64",
        }
    }

    /// QEMU system-emulator suffix (`qemu-system-<this>`).
    pub fn qemu(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.qemu())
    }
}

impl FromStr for Arch {
    type Err = VmmError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "x86_64" | "x86-64" | "amd64" | "x64" => Ok(Arch::X86_64),
            "aarch64" | "arm64" | "arm64e" => Ok(Arch::Aarch64),
            other => Err(VmmError::invalid(format!("unknown architecture '{other}'"))),
        }
    }
}

/// Operating system inside the guest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOs {
    #[default]
    Linux,
    Macos,
    Windows,
}

/// Boot firmware preference for VM guests (a Fleet template's
/// `vmTemplate.firmware`). `None` in a [`StartSpec`] lets the backend decide
/// (QEMU: UEFI on aarch64, for Windows and for GPT disks without a BIOS boot
/// partition; SeaBIOS otherwise, as KubeVirt does).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootFirmware {
    /// Legacy BIOS (SeaBIOS). Ignored on aarch64, which has no BIOS.
    Bios,
    /// UEFI (edk2/OVMF), secure boot off.
    Efi,
}

impl FromStr for BootFirmware {
    type Err = VmmError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "bios" | "seabios" | "legacy" => Ok(BootFirmware::Bios),
            "efi" | "uefi" | "ovmf" => Ok(BootFirmware::Efi),
            other => Err(VmmError::invalid(format!("unknown firmware '{other}'"))),
        }
    }
}

/// Which backend manages an instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// Apple Virtualization.framework through `lume serve`.
    Lume,
    /// `qemu-system-*` launched directly on the host.
    Qemu,
    /// QEMU inside a `trycua/cua-qemu-*` container.
    QemuDocker,
    /// OCI container (gVisor `runsc` when available, else `runc`).
    Container,
    /// The SDK-managed "cua-runtime" VM that hosts containerd + runsc.
    Managed,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Lume => "lume",
            BackendKind::Qemu => "qemu",
            BackendKind::QemuDocker => "qemu-docker",
            BackendKind::Container => "container",
            BackendKind::Managed => "managed",
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where the guest's root filesystem / disk comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// A local disk file (`.qcow2`, `.img`/`.raw`, `.vhdx`, or an installer `.iso`).
    Disk { path: PathBuf },
    /// An OCI reference. Interpreted per backend: a container image for
    /// `container`, a lume VM image for `lume`, a KubeVirt containerDisk for
    /// `qemu` (resolved to a local qcow2 through an [`crate::DiskResolver`]).
    Oci { reference: String },
    /// Reuse an instance that already exists under this name (created by
    /// [`crate::Runtime::fork`] or by an earlier `start` that was stopped).
    Existing,
}

impl ImageSource {
    pub fn disk(path: impl Into<PathBuf>) -> Self {
        Self::Disk { path: path.into() }
    }
    pub fn oci(reference: impl Into<String>) -> Self {
        Self::Oci {
            reference: reference.into(),
        }
    }
}

/// A readiness check. Readiness is always "the backend reports running" plus
/// every probe in [`StartSpec::probes`]; nothing assumes a daemon in the guest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Probe {
    /// A guest TCP port accepts a connection *and keeps it open* (user-mode
    /// NAT forwarders accept and immediately close when nothing listens in the
    /// guest; that does not count).
    Tcp { port: u16 },
    /// `GET http://<host>:<port><path>` returns a 2xx/3xx status.
    Http { port: u16, path: String },
    /// No probe; the instance is ready as soon as the backend says it runs.
    None,
}

impl Probe {
    pub fn tcp(port: u16) -> Self {
        Probe::Tcp { port }
    }
    pub fn http(port: u16, path: impl Into<String>) -> Self {
        Probe::Http {
            port,
            path: path.into(),
        }
    }
    /// Guest port this probe targets, if any.
    pub fn port(&self) -> Option<u16> {
        match self {
            Probe::Tcp { port } | Probe::Http { port, .. } => Some(*port),
            Probe::None => None,
        }
    }
}

/// How to get an SSH login into a VM guest. For QEMU the runtime forwards guest
/// port 22 and, when no user-data is given, seeds `authorized_keys` through a
/// cloud-init NoCloud ISO.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshAccess {
    pub user: String,
    /// Private key used to log in. Its `.pub` sibling is injected via cloud-init.
    pub private_key: PathBuf,
    /// Password, for images that allow password auth (e.g. lume `lume`/`lume`).
    #[serde(default)]
    pub password: Option<String>,
}

/// Everything needed to start (or create-and-start) one sandbox.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StartSpec {
    pub name: String,
    pub image: ImageSource,
    #[serde(default)]
    pub os: GuestOs,
    /// Guest architecture. `None` = host architecture (or whatever the image is).
    #[serde(default)]
    pub arch: Option<Arch>,
    pub cpus: u32,
    pub memory_mb: u64,
    /// Boot firmware (VM backends). `None` = the backend decides.
    #[serde(default)]
    pub firmware: Option<BootFirmware>,
    /// Grow the instance disk to at least this many GiB (VM backends).
    #[serde(default)]
    pub disk_size_gb: Option<u32>,
    /// Guest TCP ports to publish on the host (loopback).
    #[serde(default)]
    pub ports: Vec<u16>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The sandbox's command (argv), replacing the image's entrypoint:
    /// containers get it as ENTRYPOINT (with an empty CMD); Linux VMs run it
    /// from cloud-init as the `cua-command` unit with `env` exported. Other
    /// VM guests reject it.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub probes: Vec<Probe>,
    #[serde(with = "duration_secs")]
    pub ready_timeout: Duration,
    #[serde(default)]
    pub ssh: Option<SshAccess>,
    /// Raw cloud-init user-data (VM backends). Overrides the generated one.
    #[serde(default)]
    pub cloud_init_user_data: Option<String>,
    /// Block guest egress (QEMU `restrict=on`; loopback host forwards
    /// still work). Off by default: guests get outbound network, like a
    /// Docker container. Backends that cannot isolate the guest while still
    /// publishing its ports (containers, Lume) refuse it
    /// ([`reject_restricted_network`]).
    #[serde(default)]
    pub restrict_network: bool,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Backend-specific extra arguments (appended to the qemu command line).
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Extra containers sharing the instance's network namespace (container
    /// backend only; VM backends reject them). They start before `start`
    /// returns and are removed with the instance.
    #[serde(default)]
    pub sidecars: Vec<SidecarSpec>,
    /// Credentials for pulling `image` (and sidecar images on the same
    /// registry). Never serialized or logged.
    #[serde(skip)]
    pub registry_auth: Option<RegistryCredentials>,
    /// Container backend: the OCI runtime, `runc` or `gvisor`/`runsc`.
    /// `None`: gVisor when the engine has it. Sidecars need an explicit
    /// `runc` where gVisor would be chosen.
    #[serde(default)]
    pub container_runtime: Option<String>,
    /// A GPU option this backend offers ([`crate::gpu`]): `paravirtual`
    /// (Lume, macOS guests), `virgl` (QEMU), `nvidia` (containers). `None`:
    /// no GPU. A backend refuses one it does not offer.
    #[serde(default)]
    pub gpu: Option<String>,
    /// The image's layers, `(digest, size)`, when the caller read its
    /// manifest: a pull's progress knows its total and the layers the
    /// engine already has from the start, instead of learning sizes layer
    /// by layer.
    #[serde(default)]
    pub pull_layers: Vec<(String, u64)>,
}

/// Refuses a GPU option `backend` does not offer (`offered`: the ones it
/// does for this spec).
pub fn reject_gpu(backend: BackendKind, spec: &StartSpec, offered: &[&str]) -> crate::Result<()> {
    match spec.gpu.as_deref() {
        None => Ok(()),
        Some(g) if offered.contains(&g) => Ok(()),
        Some(g) => Err(crate::VmmError::Unsupported {
            backend: backend.as_str(),
            op: match (backend, g) {
                (BackendKind::Lume, _) => {
                    "a GPU other than `paravirtual` (GPU acceleration is for macOS guests)"
                }
                (BackendKind::Qemu, _) => "a GPU other than `virgl`",
                _ => "a GPU other than `nvidia`",
            },
        }),
    }
}

/// VM backends have no network namespace to share: sidecars are a
/// container-backend feature.
pub fn reject_sidecars(backend: BackendKind, spec: &StartSpec) -> crate::Result<()> {
    if spec.sidecars.is_empty() {
        return Ok(());
    }
    Err(crate::VmmError::Unsupported {
        backend: backend.as_str(),
        op: "sidecars (extra containers need a container sandbox; a VM has no network \
             namespace to share)",
    })
}

/// Backends that cannot cut a guest's egress while keeping its published
/// ports refuse [`StartSpec::restrict_network`] instead of ignoring it.
pub fn reject_restricted_network(backend: BackendKind, spec: &StartSpec) -> crate::Result<()> {
    if !spec.restrict_network {
        return Ok(());
    }
    Err(crate::VmmError::Unsupported {
        backend: backend.as_str(),
        op: match backend {
            BackendKind::Container => {
                "network=\"none\" (a container without a network cannot publish the ports \
                 the SDK reaches it on; use a QEMU VM sandbox for a guest without egress)"
            }
            _ => "network=\"none\" (only QEMU guests can run without outbound network)",
        },
    })
}

/// Registry credentials for one pull. `Debug` redacts the password, and the
/// type is never serialized.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistryCredentials {
    /// Registry host the credentials are for (`ghcr.io`, `localhost:5000`,
    /// `docker.io`). `None`: whichever registry the image names.
    pub registry: Option<String>,
    pub username: String,
    pub password: String,
}

impl RegistryCredentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            registry: None,
            username: username.into(),
            password: password.into(),
        }
    }

    /// Scopes the credentials to one registry host.
    pub fn for_registry(mut self, registry: impl Into<String>) -> Self {
        self.registry = Some(registry.into());
        self
    }

    /// Whether these credentials apply to `registry` (a host as image refs
    /// spell it; Docker Hub spellings are equivalent).
    pub fn applies_to(&self, registry: &str) -> bool {
        match &self.registry {
            None => true,
            Some(r) => normalize_registry(r) == normalize_registry(registry),
        }
    }
}

impl std::fmt::Debug for RegistryCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryCredentials")
            .field("registry", &self.registry)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Docker Hub has several spellings; everything else compares as given.
pub fn normalize_registry(host: &str) -> &str {
    match host {
        "docker.io" | "index.docker.io" | "registry-1.docker.io" => "docker.io",
        other => other,
    }
}

/// The registry host of an image reference, docker-CLI style (`python` and
/// `library/python` are `docker.io`).
pub fn registry_of(reference: &str) -> &str {
    let reference = reference.split('@').next().unwrap_or(reference);
    match reference.split_once('/') {
        Some((first, _)) if first.contains('.') || first.contains(':') || first == "localhost" => {
            normalize_registry(first)
        }
        _ => "docker.io",
    }
}

/// An extra container that shares a sandbox's network namespace. The
/// sandbox reaches it at its name (or `localhost`) and it reaches the
/// sandbox at `main`, as on Fleet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarSpec {
    /// Name, unique within the sandbox (a DNS label other than `main`);
    /// also its hostname.
    pub name: String,
    /// OCI image reference.
    pub image: String,
    /// argv replacing the image's ENTRYPOINT (with an empty CMD).
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Ports the sidecar listens on (reachable from the sandbox on
    /// `localhost`, and published like the sandbox's own ports).
    #[serde(default)]
    pub ports: Vec<u16>,
}

impl StartSpec {
    pub fn new(name: impl Into<String>, image: ImageSource) -> Self {
        Self {
            name: name.into(),
            image,
            os: GuestOs::Linux,
            arch: None,
            firmware: None,
            cpus: 2,
            memory_mb: 2048,
            disk_size_gb: None,
            ports: Vec::new(),
            env: BTreeMap::new(),
            command: None,
            probes: Vec::new(),
            ready_timeout: Duration::from_secs(300),
            ssh: None,
            cloud_init_user_data: None,
            restrict_network: false,
            labels: BTreeMap::new(),
            extra_args: Vec::new(),
            sidecars: Vec::new(),
            registry_auth: None,
            container_runtime: None,
            gpu: None,
            pull_layers: vec![],
        }
    }

    pub fn os(mut self, os: GuestOs) -> Self {
        self.os = os;
        self
    }
    pub fn arch(mut self, arch: Arch) -> Self {
        self.arch = Some(arch);
        self
    }
    pub fn firmware(mut self, firmware: BootFirmware) -> Self {
        self.firmware = Some(firmware);
        self
    }
    pub fn cpus(mut self, cpus: u32) -> Self {
        self.cpus = cpus;
        self
    }
    pub fn memory_mb(mut self, mb: u64) -> Self {
        self.memory_mb = mb;
        self
    }
    pub fn port(mut self, port: u16) -> Self {
        if !self.ports.contains(&port) {
            self.ports.push(port);
        }
        self
    }
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
    pub fn command<I, S>(mut self, argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.command = Some(argv.into_iter().map(Into::into).collect());
        self
    }
    /// Add a readiness probe; its port is published automatically.
    pub fn probe(mut self, probe: Probe) -> Self {
        if let Some(p) = probe.port() {
            self = self.port(p);
        }
        self.probes.push(probe);
        self
    }
    pub fn ready_timeout(mut self, t: Duration) -> Self {
        self.ready_timeout = t;
        self
    }
    pub fn ssh(mut self, access: SshAccess) -> Self {
        self.ssh = Some(access);
        self
    }
    pub fn label(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.labels.insert(k.into(), v.into());
        self
    }
    pub fn sidecar(mut self, sidecar: SidecarSpec) -> Self {
        self.sidecars.push(sidecar);
        self
    }
    pub fn registry_auth(mut self, creds: RegistryCredentials) -> Self {
        self.registry_auth = Some(creds);
        self
    }

    /// Guest architecture after defaulting to the host.
    pub fn effective_arch(&self) -> Arch {
        self.arch.unwrap_or_else(Arch::host)
    }
}

/// A host:port pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

/// VNC access to the guest framebuffer (VMM-level; no guest agent needed).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VncEndpoint {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub password: Option<String>,
}

/// SSH access into the guest, when the image has sshd.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshEndpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub private_key: Option<PathBuf>,
    #[serde(default)]
    pub password: Option<String>,
}

/// How to reach a running instance from the host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoints {
    /// Host to dial for published ports (`127.0.0.1` for forwarded ports, the
    /// VM IP for bridged/NAT backends such as Lume).
    pub host: String,
    /// Guest port → host port for every published port.
    #[serde(default)]
    pub ports: BTreeMap<u16, u16>,
    #[serde(default)]
    pub vnc: Option<VncEndpoint>,
    /// QEMU Machine Protocol socket (screendump / input / power / snapshots).
    #[serde(default)]
    pub qmp: Option<HostPort>,
    /// File the guest serial console is logged to.
    #[serde(default)]
    pub serial_log: Option<PathBuf>,
    #[serde(default)]
    pub ssh: Option<SshEndpoint>,
    #[serde(default)]
    pub container_id: Option<String>,
}

impl Endpoints {
    /// Host port that reaches `guest_port`, if it is published.
    pub fn host_port(&self, guest_port: u16) -> Option<u16> {
        self.ports.get(&guest_port).copied()
    }
    /// `host:port` string for a guest port.
    pub fn addr(&self, guest_port: u16) -> Option<String> {
        self.host_port(guest_port)
            .map(|p| format!("{}:{}", self.host, p))
    }
}

/// Lifecycle state as reported by the backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    /// Paused in memory (QMP `stop`, `docker pause`).
    Paused,
    Stopped,
    /// Being created or pulled.
    Provisioning,
    Unknown(String),
}

/// Isolation the instance actually got. Reported so callers can surface a
/// weaker-than-requested sandbox (e.g. `runc` when gVisor is unavailable).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Isolation {
    /// Hardware virtual machine with the named accelerator (`hvf`, `kvm`,
    /// `whpx`, `tcg`, `vz`).
    Vm { accel: String },
    /// gVisor user-space kernel (`runsc`).
    Gvisor,
    /// Plain Linux namespaces (`runc`) — shares the engine VM's kernel.
    Runc,
}

/// A started instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub backend: BackendKind,
    pub status: Status,
    pub endpoints: Endpoints,
    pub isolation: Isolation,
    #[serde(default)]
    pub arch: Option<Arch>,
}

/// One row of [`crate::Runtime::list`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceSummary {
    pub name: String,
    pub backend: BackendKind,
    pub status: Status,
}

/// A saved state that can seed new instances.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointInfo {
    pub name: String,
    pub backend: BackendKind,
    /// Seconds since the Unix epoch.
    pub created_at: u64,
    #[serde(default)]
    pub source: Option<String>,
}

impl CheckpointInfo {
    pub fn now(name: impl Into<String>, backend: BackendKind, source: Option<String>) -> Self {
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            name: name.into(),
            backend,
            created_at,
            source,
        }
    }
}

/// Validate a sandbox name: it becomes a directory, a container name and a
/// QEMU `-name`, so keep it to a conservative DNS-label-like alphabet.
pub fn validate_name(name: &str) -> crate::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        Err(VmmError::invalid(format!(
            "sandbox name '{name}' must be 1-63 chars of [A-Za-z0-9._-] starting with a letter or digit"
        )))
    }
}

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_secs(u64::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egress_is_on_by_default_and_only_qemu_can_cut_it() {
        let mut spec = StartSpec::new("x", ImageSource::Existing);
        assert!(
            !spec.restrict_network,
            "guests get outbound network by default"
        );
        for b in [BackendKind::Container, BackendKind::Lume] {
            assert!(reject_restricted_network(b, &spec).is_ok());
        }
        spec.restrict_network = true;
        for b in [BackendKind::Container, BackendKind::Lume] {
            let e = reject_restricted_network(b, &spec).unwrap_err().to_string();
            assert!(e.contains("network=\"none\""), "{e}");
        }
    }

    #[test]
    fn arch_parsing_accepts_oci_and_uname_spellings() {
        assert_eq!("amd64".parse::<Arch>().unwrap(), Arch::X86_64);
        assert_eq!("x86_64".parse::<Arch>().unwrap(), Arch::X86_64);
        assert_eq!("arm64".parse::<Arch>().unwrap(), Arch::Aarch64);
        assert_eq!("aarch64".parse::<Arch>().unwrap(), Arch::Aarch64);
        assert!("riscv".parse::<Arch>().is_err());
    }

    #[test]
    fn firmware_parses_template_spellings() {
        assert_eq!("efi".parse::<BootFirmware>().unwrap(), BootFirmware::Efi);
        assert_eq!("UEFI".parse::<BootFirmware>().unwrap(), BootFirmware::Efi);
        assert_eq!("bios".parse::<BootFirmware>().unwrap(), BootFirmware::Bios);
        assert!("coreboot".parse::<BootFirmware>().is_err());
    }

    #[test]
    fn probe_publishes_its_port() {
        let spec = StartSpec::new("x", ImageSource::Existing)
            .probe(Probe::tcp(22))
            .probe(Probe::http(8080, "/"))
            .port(22);
        assert_eq!(spec.ports, vec![22, 8080]);
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("cua-e2e-1").is_ok());
        assert!(validate_name("-bad").is_err());
        assert!(validate_name("has space").is_err());
        assert!(validate_name(&"a".repeat(64)).is_err());
    }

    #[test]
    fn spec_round_trips_through_json() {
        let spec = StartSpec::new("x", ImageSource::oci("alpine:3"))
            .probe(Probe::http(80, "/health"))
            .env("A", "b");
        let json = serde_json::to_string(&spec).unwrap();
        let back: StartSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.probes, spec.probes);
        assert_eq!(back.image, spec.image);
        assert_eq!(back.ready_timeout, spec.ready_timeout);
    }
}
