//! Where a sandbox runs, what kind of machine it is, and which engine runs
//! it: three independent axes, each `auto` by default.
//!
//! | axis | values | default |
//! |---|---|---|
//! | [`On`] (where) | `local`, `cloud`, `direct:<addr>`, `relay:<id>`, or a registered provider | `local` (see [`crate::settings`]) |
//! | [`Kind`] (what) | `auto`, `container`, `vm`, `microvm` | `auto`: from the image |
//! | [`Runtime`] (engine) | `auto`, `gvisor`, `runc`, `qemu`, `lume`, `firecracker`, `containerization`, `kubevirt`, or a provider's own | `auto`: the safest available |
//!
//! Each location advertises the kinds it runs and the runtimes for each
//! kind ([`Capabilities`]); [`validate`] and [`select`] check a request
//! against them and fail with an [`PlacementError`] that lists the valid
//! options. The built-in locations:
//!
//! | location | container | vm | microvm |
//! |---|---|---|---|
//! | `local` | `gvisor` (auto when available), `runc` | `qemu` (auto), `lume` (auto for macOS guests) | `firecracker` (Linux with KVM), `containerization` (macOS 26, Apple silicon) |
//! | `cloud` | `gvisor` | `kubevirt` | |
//! | `direct:` / `relay:` | an existing machine: kind and runtime stay `auto` | |
//!
//! Third-party providers ([`register_provider`]) add locations with their
//! own kinds and runtimes (`--on e2b`, ...), validated the same way.
//! External runtime providers installed on this machine
//! ([`crate::external`]) add `local` runtimes under the kind they present
//! as; `auto` never picks them.
//!
//! `auto` kind comes from the image: macOS and Windows images are VMs; an
//! image with a container rootfs is a container; an image that only offers
//! a disk (a containerDisk or a Lume image) is a VM. `auto` runtime is the
//! first runtime the location lists for that kind that fits the image and
//! the host (gVisor before runc; Lume for a macOS guest).
//!
//! A `microvm` is never chosen by `auto`: it boots the image's container
//! rootfs with its own kernel (locally Firecracker on Linux and Apple's
//! Containerization on a Mac; Modal's VM runtime with `--on modal --runtime
//! microvm`), asked for with `--kind microvm`.

use cua_image::resolve::Variant;
use std::fmt;
use std::sync::{OnceLock, RwLock};

/// Where a sandbox runs (the `--on` flag, the `on` argument).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum On {
    /// This machine: a container, QEMU or Lume.
    Local,
    /// Cua cloud.
    Cloud,
    /// An existing machine running cua-spacesd, by address (`host:port`).
    Direct(String),
    /// A machine of the account, reached through the cua.ai relay.
    Relay(String),
    /// A new Space on one of your machines that provides Spaces (`cua host
    /// setup --provide-spaces`), by machine id or name ("spare mac mini"
    /// resolves fuzzily). Reached through the relay.
    Host(String),
    /// A registered third-party provider, by name.
    Provider(String),
    /// The least-loaded of your machines that can take it: this one and
    /// the ones that provide Spaces (yours, and those shared with you to
    /// use), and Modal only when you connected it and no machine has room.
    /// Never the default: an omitted `on` is [`On::Local`].
    Auto,
}

impl On {
    /// The location word: `local`, `cloud`, `direct`, `relay`, or the
    /// provider's name.
    pub fn location(&self) -> &str {
        match self {
            On::Local => "local",
            On::Cloud => "cloud",
            On::Direct(_) => "direct",
            On::Relay(_) => "relay",
            On::Host(_) => "host",
            On::Provider(p) => p,
            On::Auto => "auto",
        }
    }

    /// Whether this names an existing machine (kind and runtime do not
    /// apply).
    pub fn is_existing_machine(&self) -> bool {
        matches!(self, On::Direct(_) | On::Relay(_))
    }

    /// Parses `local`, `cloud`, `direct:<addr>`, `relay:<id>` or a
    /// registered provider name (case-insensitive). Engine and kind words
    /// (`qemu`, `docker`, `vm`, ...) are refused with a pointer to
    /// `--runtime` / `--kind`.
    pub fn parse(s: &str) -> Result<Self, PlacementError> {
        let t = s.trim();
        let lower = t.to_ascii_lowercase();
        for (prefix, make) in [
            ("direct:", On::Direct as fn(String) -> On),
            ("relay:", On::Relay as fn(String) -> On),
            ("host:", On::Host as fn(String) -> On),
        ] {
            if lower.starts_with(prefix) {
                let rest = t[prefix.len()..].trim();
                if rest.is_empty() {
                    return Err(PlacementError::new(
                        Axis::On,
                        t,
                        format!(
                            "{prefix} needs an address, for example {prefix}{}",
                            match prefix {
                                "direct:" => "127.0.0.1:3211",
                                "host:" => "<computer name or id>",
                                _ => "<computer-id>",
                            }
                        ),
                        valid_on(),
                    ));
                }
                return Ok(make(rest.to_string()));
            }
        }
        match lower.as_str() {
            "local" => return Ok(On::Local),
            "cloud" => return Ok(On::Cloud),
            "auto" => return Ok(On::Auto),
            _ => {}
        }
        if let Some(p) = provider(&lower) {
            return Ok(On::Provider(p.name));
        }
        // A reserved contrib location (`e2b`, `daytona`, ...) parses even when
        // this build did not register the provider, so the create path can
        // say how to get it instead of "unknown location".
        if crate::provider::is_contrib_location(&lower)
            || crate::provider::is_cloud_location(&lower)
        {
            return Ok(On::Provider(lower));
        }
        let hint = match lower.as_str() {
            "fleet" => " (Cua Cloud has ended)".to_string(),
            "docker" | "container" => {
                " (`--on` says where it runs; use `--kind container`)".to_string()
            }
            "vm" => " (`--on` says where it runs; use `--kind vm`)".to_string(),
            "microvm" => " (`--on` says where it runs; use `--kind microvm`)".to_string(),
            other if Runtime::parse(other).is_ok_and(|r| r != Runtime::Auto) => {
                format!(" (`--on` says where it runs; use `--runtime {other}`)")
            }
            _ => String::new(),
        };
        Err(PlacementError::new(
            Axis::On,
            t,
            format!("unknown location {t:?}{hint}"),
            valid_on(),
        ))
    }
}

impl On {
    /// [`On::parse`], and any other plain word is one of your machines
    /// (`--on mac-mini` is `host:mac-mini`): a word that names no location
    /// and is not an engine or kind word. Used where Spaces are created, so
    /// a person or an agent can name the machine directly.
    pub fn parse_or_host(s: &str) -> Result<Self, PlacementError> {
        match On::parse(s) {
            Ok(on) => Ok(on),
            Err(e) => {
                let t = s.trim();
                let lower = t.to_ascii_lowercase();
                let reserved = matches!(
                    lower.as_str(),
                    "fleet" | "docker" | "container" | "vm" | "microvm"
                ) || Runtime::parse(&lower)
                    .is_ok_and(|r| !matches!(r, Runtime::Other(_)));
                if t.is_empty() || t.contains(':') || reserved {
                    Err(e)
                } else {
                    Ok(On::Host(t.to_string()))
                }
            }
        }
    }
}

impl fmt::Display for On {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            On::Direct(a) => write!(f, "direct:{a}"),
            On::Relay(a) => write!(f, "relay:{a}"),
            On::Host(a) => write!(f, "host:{a}"),
            other => f.write_str(other.location()),
        }
    }
}

impl std::str::FromStr for On {
    type Err = PlacementError;
    fn from_str(s: &str) -> Result<Self, PlacementError> {
        On::parse(s)
    }
}

/// What kind of machine a sandbox is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    /// From the image (see the module docs).
    #[default]
    Auto,
    /// A container: shares the host (or engine VM) kernel.
    Container,
    /// A virtual machine with its own kernel.
    Vm,
    /// A lightweight VM with its own kernel that boots the image's
    /// container rootfs (Firecracker): VM isolation, container images,
    /// snapshot-fast starts.
    MicroVm,
}

impl Kind {
    /// `auto`, `container`, `vm` or `microvm`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Auto => "auto",
            Kind::Container => "container",
            Kind::Vm => "vm",
            Kind::MicroVm => "microvm",
        }
    }

    /// Parses [`Kind::as_str`] (case-insensitive; empty is `auto`).
    pub fn parse(s: &str) -> Result<Self, PlacementError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Kind::Auto),
            "container" => Ok(Kind::Container),
            "vm" => Ok(Kind::Vm),
            "microvm" | "micro-vm" => Ok(Kind::MicroVm),
            other => {
                let hint = match other {
                    "docker" | "rootfs" => " (a container is `container`)",
                    "qemu" | "lume" | "kubevirt" | "gvisor" | "runc" | "runsc" | "firecracker"
                    | "containerization" => " (that is a runtime: use `--runtime`)",
                    _ => "",
                };
                Err(PlacementError::new(
                    Axis::Kind,
                    s.trim(),
                    format!("unknown kind {:?}{hint}", s.trim()),
                    ["auto", "container", "vm", "microvm"]
                        .map(String::from)
                        .to_vec(),
                ))
            }
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Kind {
    type Err = PlacementError;
    fn from_str(s: &str) -> Result<Self, PlacementError> {
        Kind::parse(s)
    }
}

/// Which engine runs a sandbox.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Runtime {
    /// The safest one available (see the module docs).
    #[default]
    Auto,
    /// gVisor (`runsc`): a container with a user-space kernel.
    Gvisor,
    /// runc: a plain OCI container.
    Runc,
    /// QEMU: a VM from a containerDisk or a disk file.
    Qemu,
    /// Lume (Apple Virtualization): macOS and Linux VMs on Apple silicon.
    Lume,
    /// KubeVirt: a VM in Cua cloud.
    Kubevirt,
    /// Firecracker: a microVM on a Linux host with KVM.
    Firecracker,
    /// Virtualization.framework through cua's own monitor (`cua-vz`): a
    /// microVM on an Apple silicon Mac (macOS 14+), with checkpoints and
    /// forks.
    Vz,
    /// Apple's Containerization: a microVM per Space on a Mac with macOS 26
    /// (Apple silicon), through cua's own copy of the `container` CLI.
    Containerization,
    /// A third-party provider's own runtime (Modal's `microvm`, ...).
    Other(String),
}

impl Runtime {
    /// The built-in runtimes, in display order.
    pub const BUILTIN: [Runtime; 8] = [
        Runtime::Gvisor,
        Runtime::Runc,
        Runtime::Qemu,
        Runtime::Lume,
        Runtime::Firecracker,
        Runtime::Vz,
        Runtime::Containerization,
        Runtime::Kubevirt,
    ];

    /// The runtime's name.
    pub fn as_str(&self) -> &str {
        match self {
            Runtime::Auto => "auto",
            Runtime::Gvisor => "gvisor",
            Runtime::Runc => "runc",
            Runtime::Qemu => "qemu",
            Runtime::Lume => "lume",
            Runtime::Kubevirt => "kubevirt",
            Runtime::Firecracker => "firecracker",
            Runtime::Vz => "vz",
            Runtime::Containerization => "containerization",
            Runtime::Other(s) => s,
        }
    }

    /// Parses a runtime name (case-insensitive; empty is `auto`; `runsc`
    /// is gVisor's binary). Any other word is [`Runtime::Other`], valid
    /// only where a provider advertises it.
    pub fn parse(s: &str) -> Result<Self, PlacementError> {
        let t = s.trim().to_ascii_lowercase();
        Ok(match t.as_str() {
            "" | "auto" => Runtime::Auto,
            "gvisor" | "runsc" => Runtime::Gvisor,
            "runc" => Runtime::Runc,
            "qemu" => Runtime::Qemu,
            "lume" => Runtime::Lume,
            "kubevirt" => Runtime::Kubevirt,
            "firecracker" => Runtime::Firecracker,
            "vz" | "virtualization" => Runtime::Vz,
            "containerization" | "apple" | "apple-container" => Runtime::Containerization,
            "container" | "vm" => {
                return Err(PlacementError::new(
                    Axis::Runtime,
                    &t,
                    format!("{t:?} is a kind, not a runtime: use `--kind {t}`"),
                    Runtime::BUILTIN.iter().map(|r| r.to_string()).collect(),
                ));
            }
            other => {
                if other.is_empty()
                    || !other
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(PlacementError::new(
                        Axis::Runtime,
                        &t,
                        format!("invalid runtime name {t:?}"),
                        Runtime::BUILTIN.iter().map(|r| r.to_string()).collect(),
                    ));
                }
                Runtime::Other(other.to_string())
            }
        })
    }

    /// The kind a built-in runtime runs (`None` for `auto` and third-party
    /// runtimes, whose kind comes from their provider).
    pub fn builtin_kind(&self) -> Option<Kind> {
        match self {
            Runtime::Gvisor | Runtime::Runc => Some(Kind::Container),
            Runtime::Qemu | Runtime::Lume | Runtime::Kubevirt => Some(Kind::Vm),
            Runtime::Firecracker | Runtime::Vz | Runtime::Containerization => Some(Kind::MicroVm),
            Runtime::Auto | Runtime::Other(_) => None,
        }
    }
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Runtime {
    type Err = PlacementError;
    fn from_str(s: &str) -> Result<Self, PlacementError> {
        Runtime::parse(s)
    }
}

/// Which axis a [`PlacementError`] is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// `--on` / `on` / `local=`.
    On,
    /// `--kind` / `kind`.
    Kind,
    /// `--runtime` / `runtime`.
    Runtime,
    /// The image does not fit the chosen kind or runtime.
    Image,
}

impl Axis {
    /// `on`, `kind`, `runtime` or `image`.
    pub fn as_str(self) -> &'static str {
        match self {
            Axis::On => "on",
            Axis::Kind => "kind",
            Axis::Runtime => "runtime",
            Axis::Image => "image",
        }
    }
}

/// An invalid location, kind or runtime, or a combination that does not
/// exist. `valid` lists what would be accepted in its place.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PlacementError {
    /// The axis at fault.
    pub axis: Axis,
    /// The value given.
    pub given: String,
    /// The full message, ending with the valid options.
    pub message: String,
    /// The accepted values for `axis` in this context.
    pub valid: Vec<String>,
}

impl PlacementError {
    /// An error about `axis`: `why`, then the `valid` values.
    pub fn new(axis: Axis, given: &str, why: String, valid: Vec<String>) -> Self {
        let message = if valid.is_empty() {
            why
        } else {
            format!("{why}; valid {}: {}", axis.as_str(), valid.join(", "))
        };
        Self {
            axis,
            given: given.to_string(),
            message,
            valid,
        }
    }
}

/// The kinds and runtimes of one kind a location offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindSupport {
    /// The kind.
    pub kind: Kind,
    /// Its runtimes, in `auto` preference order.
    pub runtimes: Vec<Runtime>,
}

/// What a location can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The location's name (`local`, `cloud`, `e2b`, ...).
    pub name: String,
    /// One line for help and errors.
    pub description: String,
    /// The kinds it runs, in `auto` preference order. Empty for an
    /// existing machine (`direct:`, `relay:`).
    pub kinds: Vec<KindSupport>,
}

impl Capabilities {
    /// The runtimes for `kind` (every runtime for [`Kind::Auto`]).
    pub fn runtimes(&self, kind: Kind) -> Vec<Runtime> {
        let mut out: Vec<Runtime> = vec![];
        for k in &self.kinds {
            if kind == Kind::Auto || k.kind == kind {
                for r in &k.runtimes {
                    if !out.contains(r) {
                        out.push(r.clone());
                    }
                }
            }
        }
        out
    }

    /// The kinds this location runs.
    pub fn kinds(&self) -> Vec<Kind> {
        self.kinds.iter().map(|k| k.kind).collect()
    }

    /// The kind that `runtime` runs here.
    pub fn kind_of(&self, runtime: &Runtime) -> Option<Kind> {
        self.kinds
            .iter()
            .find(|k| k.runtimes.contains(runtime))
            .map(|k| k.kind)
    }
}

/// The `local` capabilities: the built-in runtimes, then the installed
/// external runtime providers ([`crate::external`]), each under the kind it
/// presents as. With no provider installed, exactly
/// [`builtin_local_capabilities`].
pub fn local_capabilities() -> Capabilities {
    local_capabilities_with(&crate::external::Registry::global().providers())
}

/// [`builtin_local_capabilities`] plus `providers` (never chosen by `auto`:
/// they come after the built-ins of their kind).
pub fn local_capabilities_with(
    providers: &[std::sync::Arc<crate::external::ExternalProvider>],
) -> Capabilities {
    let mut caps = builtin_local_capabilities();
    for p in providers {
        let runtime = p.runtime();
        match caps.kinds.iter_mut().find(|k| k.kind == p.kind) {
            Some(k) if !k.runtimes.contains(&runtime) => k.runtimes.push(runtime),
            Some(_) => {}
            None => caps.kinds.push(KindSupport {
                kind: p.kind,
                runtimes: vec![runtime],
            }),
        }
    }
    caps
}

/// The `local` capabilities of the runtimes built into cua.
pub fn builtin_local_capabilities() -> Capabilities {
    Capabilities {
        name: "local".into(),
        description: "this computer: a container (gVisor or runc), a VM (QEMU or Lume) or a \
                      microVM (Firecracker on Linux with KVM; on a Mac, cua's own \
                      Virtualization.framework monitor, or Apple's Containerization on \
                      macOS 26)"
            .into(),
        kinds: vec![
            KindSupport {
                kind: Kind::Container,
                runtimes: vec![Runtime::Gvisor, Runtime::Runc],
            },
            KindSupport {
                kind: Kind::Vm,
                runtimes: vec![Runtime::Qemu, Runtime::Lume],
            },
            KindSupport {
                kind: Kind::MicroVm,
                runtimes: vec![Runtime::Firecracker, Runtime::Vz, Runtime::Containerization],
            },
        ],
    }
}

/// The built-in `cloud` capabilities.
pub fn cloud_capabilities() -> Capabilities {
    Capabilities {
        name: "cloud".into(),
        description: "Cua cloud: a gVisor container or a KubeVirt VM".into(),
        kinds: vec![
            KindSupport {
                kind: Kind::Container,
                runtimes: vec![Runtime::Gvisor],
            },
            KindSupport {
                kind: Kind::Vm,
                runtimes: vec![Runtime::Kubevirt],
            },
        ],
    }
}

fn providers_lock() -> &'static RwLock<Vec<Capabilities>> {
    static P: OnceLock<RwLock<Vec<Capabilities>>> = OnceLock::new();
    P.get_or_init(|| RwLock::new(vec![]))
}

/// Registers a third-party location (`--on <name>`). A later registration
/// of the same name replaces the earlier one. `local`, `cloud`, `direct`
/// and `relay` are reserved.
pub fn register_provider(caps: Capabilities) -> Result<(), PlacementError> {
    let name = caps.name.trim().to_ascii_lowercase();
    if ["local", "cloud", "direct", "relay", "fleet", "auto"].contains(&name.as_str())
        || name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(PlacementError::new(
            Axis::On,
            &caps.name,
            format!("{:?} cannot name a provider", caps.name),
            vec![],
        ));
    }
    let caps = Capabilities { name, ..caps };
    let mut p = providers_lock().write().unwrap_or_else(|e| e.into_inner());
    p.retain(|c| c.name != caps.name);
    p.push(caps);
    Ok(())
}

/// The registered third-party provider named `name`.
pub fn provider(name: &str) -> Option<Capabilities> {
    let name = name.trim().to_ascii_lowercase();
    providers_lock()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|c| c.name == name)
        .cloned()
}

/// Every location: `local`, then the registered providers. Cua Cloud
/// (`cloud`) has ended and is no longer listed; it still parses, and a
/// create there fails with [`crate::CLOUD_ENDED`].
pub fn locations() -> Vec<Capabilities> {
    let mut v = vec![local_capabilities()];
    v.extend(
        providers_lock()
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned(),
    );
    v
}

fn valid_on() -> Vec<String> {
    let mut v: Vec<String> = locations().into_iter().map(|c| c.name).collect();
    // Reserved contrib provider names, whether or not this build registered
    // them, so the error points at a real option (`--on e2b`, ...).
    for name in crate::provider::CONTRIB_LOCATIONS
        .iter()
        .chain(crate::provider::CLOUD_LOCATIONS)
    {
        if !v.iter().any(|n| n == name) {
            v.push((*name).to_string());
        }
    }
    v.push("auto".into());
    v.push("direct:<addr>".into());
    v.push("host:<computer>".into());
    v
}

/// The capabilities of `on` (an existing machine offers none).
pub fn capabilities(on: &On) -> Capabilities {
    match on {
        On::Local => local_capabilities(),
        On::Cloud => cloud_capabilities(),
        On::Provider(p) => provider(p).unwrap_or(Capabilities {
            name: p.clone(),
            description: String::new(),
            kinds: vec![],
        }),
        // A host runs what this build runs locally (it is a cua machine);
        // the host itself refuses what its runtimes cannot do.
        On::Host(_) | On::Auto => Capabilities {
            name: on.location().into(),
            description: "one of your computers that provides Spaces".into(),
            ..builtin_local_capabilities()
        },
        On::Direct(_) | On::Relay(_) => Capabilities {
            name: on.location().into(),
            description: "an existing computer".into(),
            kinds: vec![],
        },
    }
}

/// What is known about the image before a placement is chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageFacts {
    /// Guest OS (`linux`, `windows`, `macos`), when known.
    pub os: Option<String>,
    /// The variants the image offers. Empty: unknown (the backend decides
    /// from the registry at start).
    pub variants: Vec<Variant>,
}

impl ImageFacts {
    fn os_is(&self, os: &str) -> bool {
        self.os
            .as_deref()
            .is_some_and(|o| o.eq_ignore_ascii_case(os))
    }

    fn macos(&self) -> bool {
        self.os_is("macos") || self.os_is("darwin")
    }

    fn only_vm(&self) -> bool {
        self.macos() || self.os_is("windows")
    }

    fn known(&self) -> bool {
        !self.variants.is_empty()
    }

    fn has(&self, v: Variant) -> bool {
        self.variants.contains(&v)
    }

    fn offers(&self) -> String {
        self.variants
            .iter()
            .map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What is known about this host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostFacts {
    /// Whether the container engine has gVisor (`None`: unknown, the
    /// engine decides at start: gVisor when available, else runc).
    pub gvisor: Option<bool>,
    /// An Apple silicon Mac (Lume runs).
    pub apple_silicon: bool,
    /// Linux with a usable `/dev/kvm` (Firecracker runs).
    pub kvm: bool,
    /// An Apple silicon Mac on macOS 26 or later (Apple's Containerization
    /// runs).
    pub apple_containers: bool,
    /// An Apple silicon Mac on macOS 14 or later with cua's microVM monitor
    /// (`cua-vz`) installed: microVMs with checkpoints and forks.
    pub vz: bool,
}

impl HostFacts {
    /// This host, without probing the container engine.
    pub fn current() -> Self {
        Self {
            gvisor: None,
            apple_silicon: cfg!(all(target_os = "macos", target_arch = "aarch64")),
            kvm: cfg!(target_os = "linux")
                && std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("/dev/kvm")
                    .is_ok(),
            apple_containers: cfg!(all(target_os = "macos", target_arch = "aarch64"))
                && cua_vmm::host::macos_major().is_some_and(|m| m >= 26),
            vz: vz_installed(),
        }
    }
}

/// Whether macOS microVMs under `cua-vz` can run here: an Apple silicon
/// Mac on macOS 14+ with the monitor and the guest init installed.
fn vz_installed() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        cua_vmm::microvm::vz::unsupported_reason().is_none()
            && cua_vmm::microvm::vz::vz_bin().is_some()
            && cua_vmm::microvm::assets::init_bin().is_some()
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        false
    }
}

/// A checked placement. `kind` and `runtime` stay [`Kind::Auto`] /
/// [`Runtime::Auto`] only when the facts do not decide them (the backend
/// then applies the same rule at start).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Where.
    pub on: On,
    /// What kind of machine.
    pub kind: Kind,
    /// Which engine.
    pub runtime: Runtime,
    /// Why, for `--explain`-style output and logs.
    pub reason: String,
}

/// Checks a request against the location's capabilities alone (no image,
/// no host): a known location, a kind it runs, a runtime it offers for
/// that kind. Returns the kind the request implies ([`Kind::Auto`] when
/// neither the kind nor the runtime says).
pub fn validate(on: &On, kind: Kind, runtime: &Runtime) -> Result<Kind, PlacementError> {
    if on.is_existing_machine() {
        if kind != Kind::Auto {
            return Err(PlacementError::new(
                Axis::Kind,
                kind.as_str(),
                format!(
                    "--kind {kind} does not apply to {on}: it connects to an existing computer"
                ),
                vec!["auto".into()],
            ));
        }
        if *runtime != Runtime::Auto {
            return Err(PlacementError::new(
                Axis::Runtime,
                runtime.as_str(),
                format!(
                    "--runtime {runtime} does not apply to {on}: it connects to an existing computer"
                ),
                vec!["auto".into()],
            ));
        }
        return Ok(Kind::Auto);
    }
    let caps = capabilities(on);
    if caps.kinds.is_empty() {
        // A registered location (local, cloud, a registered provider) always
        // has capabilities. An empty one here is a reserved contrib provider
        // this build did not register: defer its kind/runtime validation to
        // the provider (the daemon that runs it has it registered).
        if let On::Provider(_) = on {
            return Ok(kind);
        }
        return Err(PlacementError::new(
            Axis::On,
            &on.to_string(),
            format!("unknown location {:?}", on.to_string()),
            valid_on(),
        ));
    }
    if kind != Kind::Auto && !caps.kinds().contains(&kind) {
        return Err(PlacementError::new(
            Axis::Kind,
            kind.as_str(),
            format!("{on} does not run {kind} sandboxes"),
            with_auto(caps.kinds().iter().map(|k| k.to_string())),
        ));
    }
    if *runtime == Runtime::Auto {
        return Ok(kind);
    }
    // Another machine's external runtime providers are its own: that
    // machine checks the runtime when it creates the Space.
    if matches!(on, On::Host(_)) && matches!(runtime, Runtime::Other(_)) {
        return Ok(kind);
    }
    let Some(runtime_kind) = caps.kind_of(runtime) else {
        return Err(PlacementError::new(
            Axis::Runtime,
            runtime.as_str(),
            format!("runtime {runtime} is not available {}", where_phrase(on)),
            with_auto(caps.runtimes(kind).iter().map(|r| r.to_string())),
        ));
    };
    if kind != Kind::Auto && kind != runtime_kind {
        return Err(PlacementError::new(
            Axis::Runtime,
            runtime.as_str(),
            format!(
                "runtime {runtime} runs {} sandboxes, not {} ones {}",
                runtime_kind,
                kind,
                where_phrase(on)
            ),
            with_auto(caps.runtimes(kind).iter().map(|r| r.to_string())),
        ));
    }
    Ok(runtime_kind)
}

fn where_phrase(on: &On) -> String {
    match on {
        On::Local => "locally".into(),
        On::Cloud => "in the cloud".into(),
        other => format!("on {other}"),
    }
}

fn with_auto(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut v = vec!["auto".to_string()];
    v.extend(items);
    v
}

/// Chooses the kind and runtime for a request: [`validate`], then `auto`
/// from the image and the host, then a check that the image fits.
pub fn select(
    on: &On,
    kind: Kind,
    runtime: &Runtime,
    image: &ImageFacts,
    host: &HostFacts,
) -> Result<Selection, PlacementError> {
    let implied = validate(on, kind, runtime)?;
    if on.is_existing_machine() {
        return Ok(Selection {
            on: on.clone(),
            kind: Kind::Auto,
            runtime: Runtime::Auto,
            reason: "an existing computer".into(),
        });
    }
    let caps = capabilities(on);
    let mut why: Vec<String> = vec![];

    // The kind.
    let kind = if implied != Kind::Auto {
        if kind == Kind::Auto {
            why.push(format!("runtime {runtime} runs {implied}s"));
        }
        implied
    } else if caps.kinds.len() == 1 {
        let k = caps.kinds[0].kind;
        why.push(format!("{on} runs only {k}s"));
        k
    } else if image.only_vm() {
        why.push(format!(
            "{} images run as VMs",
            if image.macos() { "macOS" } else { "Windows" }
        ));
        Kind::Vm
    } else if image.has(Variant::Rootfs) {
        why.push("the image has a container rootfs".into());
        Kind::Container
    } else if image.has(Variant::Containerdisk) || image.has(Variant::Lume) {
        why.push(format!("the image offers only a disk ({})", image.offers()));
        Kind::Vm
    } else {
        Kind::Auto
    };
    if kind == Kind::Auto {
        return Ok(Selection {
            on: on.clone(),
            kind,
            runtime: runtime.clone(),
            reason: "decided from the image at start".into(),
        });
    }
    if !caps.kinds().contains(&kind) {
        return Err(PlacementError::new(
            Axis::Kind,
            kind.as_str(),
            format!("{on} does not run {kind} sandboxes ({})", why.join("; ")),
            with_auto(caps.kinds().iter().map(|k| k.to_string())),
        ));
    }

    // The image must offer that kind.
    if matches!(kind, Kind::Container | Kind::MicroVm) && image.only_vm() {
        return Err(PlacementError::new(
            Axis::Kind,
            kind.as_str(),
            format!(
                "{} images run only as VMs",
                if image.macos() { "macOS" } else { "Windows" }
            ),
            vec!["auto".into(), "vm".into()],
        ));
    }
    if image.known() {
        let fits = match kind {
            // A microVM boots the container rootfs.
            Kind::Container | Kind::MicroVm => image.has(Variant::Rootfs),
            _ => image.has(Variant::Containerdisk) || image.has(Variant::Lume),
        };
        if !fits {
            let other = if kind == Kind::Vm { "container" } else { "vm" };
            return Err(PlacementError::new(
                Axis::Image,
                kind.as_str(),
                format!(
                    "the image has no {kind} variant (it offers: {}); use --kind {other} or --kind auto",
                    image.offers()
                ),
                vec!["auto".into(), other.into()],
            ));
        }
    }

    // The runtime.
    let offered = caps.runtimes(kind);
    let runtime = if *runtime != Runtime::Auto {
        runtime.clone()
    } else {
        match (on, kind) {
            (On::Local, Kind::Container) => match host.gvisor {
                Some(true) => {
                    why.push("gVisor is available".into());
                    Runtime::Gvisor
                }
                Some(false) => {
                    why.push("gVisor is not installed in the container engine".into());
                    Runtime::Runc
                }
                None => Runtime::Auto,
            },
            (On::Local, Kind::Vm) => {
                let lume_only =
                    image.known() && image.has(Variant::Lume) && !image.has(Variant::Containerdisk);
                if image.macos() || lume_only {
                    why.push("macOS guests run on Lume".into());
                    Runtime::Lume
                } else {
                    Runtime::Qemu
                }
            }
            // A Mac's microVMs run under cua's own monitor (checkpoints
            // and forks), else on Apple's Containerization.
            (On::Local, Kind::MicroVm) if host.vz => {
                why.push(
                    "microVMs on a Mac run on Virtualization.framework (cua-vz, with \
                     checkpoints and forks)"
                        .into(),
                );
                Runtime::Vz
            }
            (On::Local, Kind::MicroVm) if host.apple_containers => {
                why.push("microVMs on a Mac run on Apple's Containerization".into());
                Runtime::Containerization
            }
            _ => offered.first().cloned().unwrap_or(Runtime::Auto),
        }
    };

    // The runtime must fit the image and the host.
    if runtime == Runtime::Containerization && !host.apple_containers {
        return Err(PlacementError::new(
            Axis::Runtime,
            "containerization",
            "Apple's Containerization runs only on Apple silicon Macs with macOS 26 or later"
                .into(),
            with_auto(
                offered
                    .iter()
                    .filter(|r| match r {
                        Runtime::Containerization => false,
                        Runtime::Firecracker => host.kvm,
                        Runtime::Vz => host.vz,
                        _ => true,
                    })
                    .map(|r| r.to_string()),
            ),
        ));
    }
    if runtime == Runtime::Vz && !host.vz {
        return Err(PlacementError::new(
            Axis::Runtime,
            "vz",
            "macOS microVMs (vz) run on an Apple silicon Mac with macOS 14 or later, with cua's \
             monitor (cua-vz) installed next to cua"
                .into(),
            with_auto(
                offered
                    .iter()
                    .filter(|r| match r {
                        Runtime::Vz => false,
                        Runtime::Firecracker => host.kvm,
                        Runtime::Containerization => host.apple_containers,
                        _ => true,
                    })
                    .map(|r| r.to_string()),
            ),
        ));
    }
    if runtime == Runtime::Firecracker && !host.kvm {
        return Err(PlacementError::new(
            Axis::Runtime,
            "firecracker",
            "Firecracker microVMs run only on Linux with KVM (/dev/kvm); use --kind container \
             or --kind vm here (on a Mac, --runtime vz or containerization)"
                .into(),
            with_auto(
                offered
                    .iter()
                    .filter(|r| match r {
                        Runtime::Firecracker => false,
                        Runtime::Vz => host.vz,
                        Runtime::Containerization => host.apple_containers,
                        _ => true,
                    })
                    .map(|r| r.to_string()),
            ),
        ));
    }
    if runtime == Runtime::Lume && !host.apple_silicon {
        return Err(PlacementError::new(
            Axis::Runtime,
            "lume",
            "Lume runs only on an Apple silicon Mac".into(),
            with_auto(
                offered
                    .iter()
                    .filter(|r| **r != Runtime::Lume)
                    .map(|r| r.to_string()),
            ),
        ));
    }
    if image.macos() && on == &On::Cloud {
        return Err(PlacementError::new(
            Axis::On,
            "cloud",
            cua_image::resolve::CLOUD_MACOS_UNSUPPORTED.into(),
            vec!["local".into()],
        ));
    }
    if image.macos() && matches!(runtime, Runtime::Qemu | Runtime::Kubevirt) {
        return Err(PlacementError::new(
            Axis::Runtime,
            runtime.as_str(),
            format!("macOS guests do not run on {runtime}"),
            vec!["auto".into(), "lume".into()],
        ));
    }
    if image.known() {
        let needs: &[Variant] = match runtime {
            Runtime::Gvisor
            | Runtime::Runc
            | Runtime::Firecracker
            | Runtime::Vz
            | Runtime::Containerization => &[Variant::Rootfs],
            Runtime::Qemu | Runtime::Kubevirt => &[Variant::Containerdisk],
            Runtime::Lume => &[Variant::Lume],
            Runtime::Auto | Runtime::Other(_) => &[],
        };
        if !needs.is_empty() && !needs.iter().any(|v| image.has(*v)) {
            let fitting: Vec<String> = offered
                .iter()
                .filter(|r| match r {
                    Runtime::Gvisor | Runtime::Runc => image.has(Variant::Rootfs),
                    Runtime::Firecracker => image.has(Variant::Rootfs) && host.kvm,
                    Runtime::Vz => image.has(Variant::Rootfs) && host.vz,
                    Runtime::Containerization => {
                        image.has(Variant::Rootfs) && host.apple_containers
                    }
                    Runtime::Qemu | Runtime::Kubevirt => image.has(Variant::Containerdisk),
                    Runtime::Lume => image.has(Variant::Lume) && host.apple_silicon,
                    _ => true,
                })
                .map(|r| r.to_string())
                .collect();
            return Err(PlacementError::new(
                Axis::Runtime,
                runtime.as_str(),
                format!(
                    "runtime {runtime} needs a {} image; this one offers: {}",
                    needs[0].as_str(),
                    image.offers()
                ),
                with_auto(fitting.into_iter()),
            ));
        }
    }
    if runtime != Runtime::Auto {
        why.push(format!("runtime {runtime}"));
    }
    Ok(Selection {
        on: on.clone(),
        kind,
        runtime,
        reason: why.join("; "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Kind::*;
    use Variant::*;

    fn img(os: Option<&str>, variants: &[Variant]) -> ImageFacts {
        ImageFacts {
            os: os.map(String::from),
            variants: variants.to_vec(),
        }
    }

    const MAC: HostFacts = HostFacts {
        gvisor: Some(true),
        apple_silicon: true,
        kvm: false,
        apple_containers: true,
        vz: true,
    };
    /// macOS 26 without cua-vz (an install from before it shipped).
    const MAC_26_NO_VZ: HostFacts = HostFacts {
        gvisor: Some(true),
        apple_silicon: true,
        kvm: false,
        apple_containers: true,
        vz: false,
    };
    /// An Apple silicon Mac before macOS 26, without cua-vz.
    const MAC_15: HostFacts = HostFacts {
        gvisor: Some(true),
        apple_silicon: true,
        kvm: false,
        apple_containers: false,
        vz: false,
    };
    const LINUX_NO_GVISOR: HostFacts = HostFacts {
        gvisor: Some(false),
        apple_silicon: false,
        kvm: false,
        apple_containers: false,
        vz: false,
    };
    const LINUX_KVM: HostFacts = HostFacts {
        gvisor: Some(true),
        apple_silicon: false,
        kvm: true,
        apple_containers: false,
        vz: false,
    };

    fn linux() -> ImageFacts {
        img(Some("linux"), &[Rootfs, Containerdisk])
    }

    fn rt(s: &str) -> Runtime {
        Runtime::parse(s).unwrap()
    }

    #[test]
    fn a_host_is_named_with_host_or_by_a_plain_word_where_spaces_are_created() {
        let spare = On::parse("host:spare mac mini").unwrap();
        assert_eq!(spare, On::Host("spare mac mini".into()));
        assert_eq!(spare.to_string(), "host:spare mac mini");
        assert_eq!(spare.location(), "host");
        assert!(!spare.is_existing_machine());
        assert!(On::parse("host:").is_err());
        // A plain word names a machine only where Spaces are created.
        assert!(On::parse("mac-mini").is_err());
        assert_eq!(
            On::parse_or_host("mac-mini").unwrap(),
            On::Host("mac-mini".into())
        );
        assert_eq!(On::parse_or_host("local").unwrap(), On::Local);
        // `auto` is the least-loaded of your machines, never a machine
        // called "auto", and never what an omitted `on` means.
        assert_eq!(On::parse("auto").unwrap(), On::Auto);
        assert_eq!(On::parse(" AUTO ").unwrap(), On::Auto);
        assert_eq!(On::parse_or_host("auto").unwrap(), On::Auto);
        assert_eq!(On::Auto.to_string(), "auto");
        assert!(!On::Auto.is_existing_machine());
        assert!(validate(&On::Auto, Kind::Vm, &Runtime::Lume).is_ok());
        // `cluster` is no location (it was never released).
        assert_eq!(
            On::parse_or_host("cluster").unwrap(),
            On::Host("cluster".into())
        );
        // Engine and kind words keep their pointer to --runtime / --kind.
        for word in ["docker", "vm", "qemu", "lume", "fleet", "x:y"] {
            assert!(On::parse_or_host(word).is_err(), "{word}");
        }
        // A host runs what a local machine runs.
        assert!(validate(&spare, Kind::Vm, &Runtime::Lume).is_ok());
        assert!(validate(&spare, Kind::Container, &Runtime::Kubevirt).is_err());
    }

    #[test]
    fn parses_the_three_axes() {
        assert_eq!(On::parse("local").unwrap(), On::Local);
        assert_eq!(On::parse(" Cloud ").unwrap(), On::Cloud);
        assert_eq!(
            On::parse("direct:10.0.0.5:3211").unwrap(),
            On::Direct("10.0.0.5:3211".into())
        );
        assert_eq!(
            On::parse("relay:abcd1234").unwrap(),
            On::Relay("abcd1234".into())
        );
        assert_eq!(On::Direct("h:1".into()).to_string(), "direct:h:1");
        assert_eq!(Kind::parse("").unwrap(), Auto);
        assert_eq!(Kind::parse("VM").unwrap(), Vm);
        assert_eq!(rt("runsc"), Runtime::Gvisor);
        assert_eq!(rt(""), Runtime::Auto);
        assert_eq!(rt("firecracker"), Runtime::Firecracker);
        assert_eq!(Kind::parse("microvm").unwrap(), MicroVm);
        // Modal's VM runtime keeps its wire word.
        assert_eq!(rt("microvm"), Runtime::Other("microvm".into()));
        assert_eq!(rt("youki"), Runtime::Other("youki".into()));
    }

    #[test]
    fn engine_and_kind_words_are_refused_as_locations_with_a_pointer() {
        for (word, hint) in [
            ("qemu", "--runtime qemu"),
            ("lume", "--runtime lume"),
            ("gvisor", "--runtime gvisor"),
            ("docker", "--kind container"),
            ("vm", "--kind vm"),
            ("fleet", "Cua Cloud has ended"),
        ] {
            let e = On::parse(word).unwrap_err();
            assert_eq!(e.axis, Axis::On, "{word}");
            assert!(e.message.contains(hint), "{word}: {}", e.message);
            assert!(e.valid.contains(&"local".to_string()), "{word}");
            assert!(!e.valid.contains(&"cloud".to_string()), "{word}");
        }
        assert!(
            On::parse("direct:")
                .unwrap_err()
                .message
                .contains("needs an address")
        );
        let e = Kind::parse("qemu").unwrap_err();
        assert!(e.message.contains("runtime"), "{}", e.message);
        assert_eq!(e.valid, ["auto", "container", "vm", "microvm"]);
        assert!(
            Kind::parse("firecracker")
                .unwrap_err()
                .message
                .contains("runtime")
        );
        let e = Runtime::parse("vm").unwrap_err();
        assert!(e.message.contains("--kind vm"), "{}", e.message);
    }

    /// Every location x kind x built-in runtime: valid exactly when the
    /// location lists that runtime for that kind.
    #[test]
    fn the_static_matrix() {
        let table: &[(On, Kind, &str, Result<Kind, Axis>)] = &[
            // local
            (On::Local, Auto, "auto", Ok(Auto)),
            (On::Local, Container, "auto", Ok(Container)),
            (On::Local, Vm, "auto", Ok(Vm)),
            (On::Local, Auto, "gvisor", Ok(Container)),
            (On::Local, Auto, "runc", Ok(Container)),
            (On::Local, Auto, "qemu", Ok(Vm)),
            (On::Local, Auto, "lume", Ok(Vm)),
            (On::Local, Auto, "kubevirt", Err(Axis::Runtime)),
            (On::Local, Container, "gvisor", Ok(Container)),
            (On::Local, Container, "runc", Ok(Container)),
            (On::Local, Container, "qemu", Err(Axis::Runtime)),
            (On::Local, Container, "lume", Err(Axis::Runtime)),
            (On::Local, Container, "kubevirt", Err(Axis::Runtime)),
            (On::Local, Vm, "qemu", Ok(Vm)),
            (On::Local, Vm, "lume", Ok(Vm)),
            (On::Local, Vm, "gvisor", Err(Axis::Runtime)),
            (On::Local, Vm, "runc", Err(Axis::Runtime)),
            (On::Local, Vm, "kubevirt", Err(Axis::Runtime)),
            (On::Local, Auto, "firecracker", Ok(MicroVm)),
            (On::Local, MicroVm, "auto", Ok(MicroVm)),
            (On::Local, MicroVm, "firecracker", Ok(MicroVm)),
            (On::Local, MicroVm, "qemu", Err(Axis::Runtime)),
            (On::Local, MicroVm, "gvisor", Err(Axis::Runtime)),
            (On::Local, Vm, "firecracker", Err(Axis::Runtime)),
            (On::Local, Container, "firecracker", Err(Axis::Runtime)),
            // cloud
            (On::Cloud, Auto, "auto", Ok(Auto)),
            (On::Cloud, Container, "auto", Ok(Container)),
            (On::Cloud, Vm, "auto", Ok(Vm)),
            (On::Cloud, Auto, "gvisor", Ok(Container)),
            (On::Cloud, Auto, "kubevirt", Ok(Vm)),
            (On::Cloud, Auto, "runc", Err(Axis::Runtime)),
            (On::Cloud, Auto, "qemu", Err(Axis::Runtime)),
            (On::Cloud, Auto, "lume", Err(Axis::Runtime)),
            (On::Cloud, Container, "gvisor", Ok(Container)),
            (On::Cloud, Container, "kubevirt", Err(Axis::Runtime)),
            (On::Cloud, Container, "runc", Err(Axis::Runtime)),
            (On::Cloud, Vm, "kubevirt", Ok(Vm)),
            (On::Cloud, Vm, "gvisor", Err(Axis::Runtime)),
            (On::Cloud, Vm, "qemu", Err(Axis::Runtime)),
            (On::Cloud, Vm, "lume", Err(Axis::Runtime)),
            (On::Cloud, MicroVm, "auto", Err(Axis::Kind)),
            (On::Cloud, Auto, "firecracker", Err(Axis::Runtime)),
            // an existing machine
            (On::Direct("h:1".into()), Auto, "auto", Ok(Auto)),
            (On::Direct("h:1".into()), Container, "auto", Err(Axis::Kind)),
            (On::Direct("h:1".into()), Vm, "auto", Err(Axis::Kind)),
            (On::Direct("h:1".into()), Auto, "gvisor", Err(Axis::Runtime)),
            (
                On::Relay("abcd1234".into()),
                Auto,
                "qemu",
                Err(Axis::Runtime),
            ),
            (On::Relay("abcd1234".into()), Auto, "auto", Ok(Auto)),
        ];
        for (on, kind, runtime, want) in table {
            let got = validate(on, *kind, &rt(runtime)).map_err(|e| e.axis);
            assert_eq!(&got, want, "{on} / {kind} / {runtime}");
        }
    }

    #[test]
    fn errors_list_the_valid_options_for_the_context() {
        let e = validate(&On::Local, Container, &Runtime::Qemu).unwrap_err();
        assert_eq!(e.valid, ["auto", "gvisor", "runc"]);
        assert!(
            e.message
                .contains("runs vm sandboxes, not container ones locally"),
            "{}",
            e.message
        );
        assert!(
            e.message.ends_with("valid runtime: auto, gvisor, runc"),
            "{}",
            e.message
        );
        let e = validate(&On::Cloud, Auto, &Runtime::Qemu).unwrap_err();
        assert_eq!(e.valid, ["auto", "gvisor", "kubevirt"]);
        assert!(
            e.message.contains("not available in the cloud"),
            "{}",
            e.message
        );
        let e = validate(&On::Cloud, Vm, &Runtime::Gvisor).unwrap_err();
        assert_eq!(e.valid, ["auto", "kubevirt"]);
        let e = validate(&On::Local, Vm, &Runtime::Firecracker).unwrap_err();
        assert_eq!(e.valid, ["auto", "qemu", "lume"]);
        assert!(
            e.message.contains("runs microvm sandboxes, not vm ones"),
            "{}",
            e.message
        );
        let e = validate(&On::Local, Vm, &Runtime::Other("youki".into())).unwrap_err();
        assert_eq!(e.valid, ["auto", "qemu", "lume"]);
    }

    /// `auto` kind and runtime for every built-in location and image shape.
    #[test]
    fn the_auto_matrix() {
        let cases: &[(On, ImageFacts, HostFacts, Kind, &str)] = &[
            // local: canonical linux (rootfs + disk) is a container on gVisor
            (On::Local, linux(), MAC, Container, "gvisor"),
            (On::Local, linux(), LINUX_NO_GVISOR, Container, "runc"),
            (On::Local, linux(), HostFacts::default(), Container, "auto"),
            // a plain container image
            (On::Local, img(None, &[Rootfs]), MAC, Container, "gvisor"),
            // disk only: a VM on QEMU
            (
                On::Local,
                img(Some("linux"), &[Containerdisk]),
                MAC,
                Vm,
                "qemu",
            ),
            (
                On::Local,
                img(Some("windows"), &[Containerdisk]),
                MAC,
                Vm,
                "qemu",
            ),
            // macOS: Lume
            (On::Local, img(Some("macos"), &[Lume]), MAC, Vm, "lume"),
            (On::Local, img(None, &[Lume]), MAC, Vm, "lume"),
            // cloud
            (On::Cloud, linux(), MAC, Container, "gvisor"),
            (On::Cloud, img(None, &[Rootfs]), MAC, Container, "gvisor"),
            (
                On::Cloud,
                img(Some("linux"), &[Containerdisk]),
                MAC,
                Vm,
                "kubevirt",
            ),
            (
                On::Cloud,
                img(Some("windows"), &[Containerdisk]),
                MAC,
                Vm,
                "kubevirt",
            ),
            // unknown image: left to the backend
            (On::Local, ImageFacts::default(), MAC, Auto, "auto"),
            (On::Cloud, ImageFacts::default(), MAC, Auto, "auto"),
        ];
        for (on, image, host, kind, runtime) in cases {
            let s = select(on, Auto, &Runtime::Auto, image, host)
                .unwrap_or_else(|e| panic!("{on} {image:?}: {e}"));
            assert_eq!(
                (s.kind, s.runtime.as_str()),
                (*kind, *runtime),
                "{on} {image:?}: {}",
                s.reason
            );
        }
    }

    #[test]
    fn an_explicit_kind_picks_the_matching_variant_and_runtime() {
        let s = select(&On::Local, Vm, &Runtime::Auto, &linux(), &MAC).unwrap();
        assert_eq!((s.kind, s.runtime), (Vm, Runtime::Qemu));
        let s = select(&On::Cloud, Vm, &Runtime::Auto, &linux(), &MAC).unwrap();
        assert_eq!((s.kind, s.runtime), (Vm, Runtime::Kubevirt));
        let s = select(&On::Cloud, Auto, &Runtime::Kubevirt, &linux(), &MAC).unwrap();
        assert_eq!(s.kind, Vm);
        let s = select(&On::Local, Auto, &Runtime::Runc, &linux(), &MAC).unwrap();
        assert_eq!((s.kind, s.runtime), (Container, Runtime::Runc));
    }

    #[test]
    fn the_image_must_offer_the_kind_and_runtime() {
        let disk = img(Some("linux"), &[Containerdisk]);
        let e = select(&On::Local, Container, &Runtime::Auto, &disk, &MAC).unwrap_err();
        assert_eq!(e.axis, Axis::Image);
        assert!(e.message.contains("offers: containerdisk"), "{}", e.message);
        assert_eq!(e.valid, ["auto", "vm"]);
        let e = select(&On::Cloud, Auto, &Runtime::Gvisor, &disk, &MAC).unwrap_err();
        assert_eq!(e.axis, Axis::Image);
        let rootfs = img(None, &[Rootfs]);
        let e = select(&On::Cloud, Vm, &Runtime::Auto, &rootfs, &MAC).unwrap_err();
        assert!(e.message.contains("no vm variant"), "{}", e.message);
        // A Lume image does not boot in QEMU.
        let mac = img(Some("macos"), &[Lume]);
        let e = select(&On::Local, Vm, &Runtime::Qemu, &mac, &MAC).unwrap_err();
        assert_eq!(e.valid, ["auto", "lume"]);
        let e = select(&On::Local, Container, &Runtime::Auto, &mac, &MAC).unwrap_err();
        assert!(
            e.message.contains("macOS images run only as VMs"),
            "{}",
            e.message
        );
        let win = img(Some("windows"), &[Containerdisk]);
        let e = select(&On::Local, Auto, &Runtime::Gvisor, &win, &MAC).unwrap_err();
        assert!(e.message.contains("Windows"), "{}", e.message);
        // Lume needs a Lume image.
        let e = select(&On::Local, Auto, &Runtime::Lume, &linux(), &MAC).unwrap_err();
        assert!(e.message.contains("needs a lume image"), "{}", e.message);
        assert_eq!(e.valid, ["auto", "qemu"]);
    }

    #[test]
    fn a_microvm_boots_the_rootfs_on_linux_with_kvm() {
        let s = select(&On::Local, MicroVm, &Runtime::Auto, &linux(), &LINUX_KVM).unwrap();
        assert_eq!((s.kind, s.runtime), (MicroVm, Runtime::Firecracker));
        let s = select(
            &On::Local,
            Auto,
            &Runtime::Firecracker,
            &linux(),
            &LINUX_KVM,
        )
        .unwrap();
        assert_eq!(s.kind, MicroVm);
        // auto never picks a microVM.
        let s = select(&On::Local, Auto, &Runtime::Auto, &linux(), &LINUX_KVM).unwrap();
        assert_eq!(s.kind, Container);
        // No KVM and no Containerization (a Mac before macOS 26): refused
        // with the alternatives.
        let e = select(&On::Local, MicroVm, &Runtime::Auto, &linux(), &MAC_15).unwrap_err();
        assert_eq!(e.axis, Axis::Runtime);
        assert!(e.message.contains("Linux with KVM"), "{}", e.message);
        // A disk-only image has no rootfs to boot.
        let disk = img(Some("linux"), &[Containerdisk]);
        let e = select(&On::Local, MicroVm, &Runtime::Auto, &disk, &LINUX_KVM).unwrap_err();
        assert_eq!(e.axis, Axis::Image);
        assert_eq!(e.valid, ["auto", "vm"]);
        let mac = img(Some("macos"), &[Lume]);
        assert!(select(&On::Local, MicroVm, &Runtime::Auto, &mac, &LINUX_KVM).is_err());
    }

    #[test]
    fn a_microvm_on_a_mac_runs_under_cua_vz_else_apples_containerization() {
        // `--kind microvm` on a Mac with cua-vz: Virtualization.framework,
        // whose microVMs checkpoint and fork.
        let s = select(&On::Local, MicroVm, &Runtime::Auto, &linux(), &MAC).unwrap();
        assert_eq!((s.kind, s.runtime), (MicroVm, Runtime::Vz));
        assert!(
            s.reason.contains("Virtualization.framework"),
            "{}",
            s.reason
        );
        for w in ["vz", "virtualization"] {
            let s = select(&On::Local, Auto, &rt(w), &linux(), &MAC).unwrap();
            assert_eq!((s.kind, s.runtime), (MicroVm, Runtime::Vz));
        }
        // Without cua-vz (an older install) on macOS 26: Apple's
        // Containerization, as before.
        let s = select(&On::Local, MicroVm, &Runtime::Auto, &linux(), &MAC_26_NO_VZ).unwrap();
        assert_eq!((s.kind, s.runtime), (MicroVm, Runtime::Containerization));
        assert!(s.reason.contains("Containerization"), "{}", s.reason);
        // Containerization by name (or its short words) where it runs.
        for w in ["containerization", "apple", "apple-container"] {
            let s = select(&On::Local, Auto, &rt(w), &linux(), &MAC).unwrap();
            assert_eq!((s.kind, s.runtime), (MicroVm, Runtime::Containerization));
        }
        // `auto` keeps a Mac's Linux Spaces on their default (a container).
        let s = select(&On::Local, Auto, &Runtime::Auto, &linux(), &MAC).unwrap();
        assert_eq!(s.kind, Container);
        // Containerization: not before macOS 26, nor on Linux.
        for host in [&MAC_15, &LINUX_KVM] {
            let e = select(
                &On::Local,
                MicroVm,
                &Runtime::Containerization,
                &linux(),
                host,
            )
            .unwrap_err();
            assert_eq!(e.axis, Axis::Runtime);
            assert!(e.message.contains("macOS 26"), "{}", e.message);
        }
        // vz: not without cua-vz, nor on Linux; the alternatives offered
        // are the ones that run there.
        let e = select(&On::Local, MicroVm, &Runtime::Vz, &linux(), &MAC_26_NO_VZ).unwrap_err();
        assert_eq!(e.axis, Axis::Runtime);
        assert!(e.message.contains("macOS 14"), "{}", e.message);
        assert_eq!(e.valid, ["auto", "containerization"]);
        let e = select(&On::Local, MicroVm, &Runtime::Vz, &linux(), &LINUX_KVM).unwrap_err();
        assert_eq!(e.valid, ["auto", "firecracker"]);
        let e = select(&On::Local, MicroVm, &Runtime::Firecracker, &linux(), &MAC).unwrap_err();
        assert!(e.message.contains("--runtime vz"), "{}", e.message);
        assert_eq!(e.valid, ["auto", "vz", "containerization"]);
        // A disk-only or macOS image has no rootfs to boot.
        let disk = img(Some("linux"), &[Containerdisk]);
        let e = select(&On::Local, MicroVm, &Runtime::Auto, &disk, &MAC).unwrap_err();
        assert_eq!(e.axis, Axis::Image);
        let e = select(&On::Local, MicroVm, &Runtime::Vz, &disk, &MAC).unwrap_err();
        assert_eq!(e.axis, Axis::Image, "{}", e.message);
        let mac = img(Some("macos"), &[Lume]);
        assert!(select(&On::Local, MicroVm, &Runtime::Auto, &mac, &MAC).is_err());
        // They are microVM runtimes, not VM or container ones.
        for r in [Runtime::Containerization, Runtime::Vz] {
            assert!(validate(&On::Local, Vm, &r).is_err());
            assert!(validate(&On::Local, Container, &r).is_err());
            assert_eq!(r.builtin_kind(), Some(MicroVm));
        }
    }

    #[test]
    fn host_limits() {
        let mac = img(Some("macos"), &[Lume]);
        let e = select(&On::Local, Auto, &Runtime::Auto, &mac, &LINUX_NO_GVISOR).unwrap_err();
        assert!(e.message.contains("Apple silicon"), "{}", e.message);
        let e = select(&On::Cloud, Auto, &Runtime::Auto, &mac, &MAC).unwrap_err();
        assert_eq!(e.axis, Axis::On);
        assert_eq!(e.valid, ["local"]);
    }

    #[test]
    fn every_own_cloud_word_parses_as_a_location() {
        for w in ["aws", "gcp", "hetzner", "modal", "HETZNER"] {
            assert_eq!(
                On::parse(w).unwrap(),
                On::Provider(w.to_ascii_lowercase()),
                "{w}"
            );
        }
        assert!(crate::provider::is_cloud_location("hetzner"));
        assert!(crate::provider::is_provider_location("hetzner"));
        // The error for an unknown word lists hetzner among the valid ones.
        let e = On::parse("hetznr").unwrap_err();
        assert!(e.valid.iter().any(|v| v == "hetzner"), "{:?}", e.valid);
    }

    #[test]
    fn providers_are_registered_and_validated_by_their_capabilities() {
        register_provider(Capabilities {
            name: "Testbox".into(),
            description: "a test provider".into(),
            kinds: vec![KindSupport {
                kind: Container,
                runtimes: vec![Runtime::Other("youki".into())],
            }],
        })
        .unwrap();
        let on = On::parse("testbox").unwrap();
        assert_eq!(on, On::Provider("testbox".into()));
        assert!(locations().iter().any(|c| c.name == "testbox"));
        assert_eq!(validate(&on, Auto, &rt("youki")).unwrap(), Container);
        let e = validate(&on, Vm, &Runtime::Auto).unwrap_err();
        assert_eq!(
            (e.axis, e.valid.clone()),
            (Axis::Kind, vec!["auto".into(), "container".into()])
        );
        let e = validate(&on, Auto, &Runtime::Gvisor).unwrap_err();
        assert_eq!(e.valid, ["auto", "youki"]);
        let s = select(&on, Auto, &Runtime::Auto, &ImageFacts::default(), &MAC).unwrap();
        assert_eq!((s.kind, s.runtime), (Container, rt("youki")));
        assert!(
            register_provider(Capabilities {
                name: "cloud".into(),
                description: String::new(),
                kinds: vec![],
            })
            .is_err()
        );
    }
}
