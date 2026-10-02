// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `SystemService`: capabilities, `Init`, health, metrics and shutdown.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cua_proto::env::v1::system_service_server::{SystemService, SystemServiceServer};
use cua_proto::env::v1::*;
use futures_util::StreamExt as _;
use tonic::{Code, Request, Response, Status};

use crate::context::ServerContext;
use crate::error::status;
use crate::provider::ServiceProvider;
use crate::services::diagnose::DiagnoseState;
use crate::util::{proto_duration, system_time, timestamp};

/// Everything `GetCapabilities` and `Health` report.
pub struct SystemServiceImpl {
    ctx: ServerContext,
    providers: Arc<Vec<Arc<dyn ServiceProvider>>>,
    core_features: Vec<Feature>,
    diagnose: DiagnoseState,
}

impl SystemServiceImpl {
    /// How the guest is hosted (the configured override, else detected).
    fn runtime(&self) -> Runtime {
        self.ctx
            .config()
            .runtime_override
            .as_deref()
            .and_then(parse_runtime)
            .unwrap_or_else(|| {
                detect_runtime(&sysinfo::System::kernel_version().unwrap_or_default()).0
            })
    }

    /// Creates the service. `core_features` are the features implemented by
    /// this crate; provider features override them by name.
    pub fn new(
        ctx: ServerContext,
        providers: Arc<Vec<Arc<dyn ServiceProvider>>>,
        core_features: Vec<Feature>,
    ) -> Self {
        Self {
            diagnose: DiagnoseState::new(ctx.clone(), None),
            ctx,
            providers,
            core_features,
        }
    }

    /// Serves `Diagnose` / `DiagnoseOnce` from `state`.
    pub fn with_diagnose(mut self, state: DiagnoseState) -> Self {
        self.diagnose = state;
        self
    }

    /// Tonic server.
    pub fn into_server(self) -> SystemServiceServer<Self> {
        SystemServiceServer::new(self)
    }

    /// The merged feature list (core, then desktop defaults, then providers).
    pub fn features(&self) -> Vec<Feature> {
        let mut merged: BTreeMap<String, Feature> = BTreeMap::new();
        for feature in &self.core_features {
            merged.insert(feature.name.clone(), feature.clone());
        }
        for name in crate::provider::DESKTOP_FEATURES {
            merged.entry((*name).to_owned()).or_insert_with(|| {
                crate::provider::feature(name, false, crate::provider::NO_DESKTOP_PROVIDER)
            });
        }
        for provider in self.providers.iter() {
            for feature in provider.capabilities() {
                merged.insert(feature.name.clone(), feature);
            }
        }
        merged.into_values().collect()
    }

    fn health_components(&self) -> Vec<ComponentHealth> {
        let mut components = vec![ComponentHealth {
            name: "process".into(),
            status: HealthStatus::Serving as i32,
            detail: String::new(),
        }];
        let data_dir = &self.ctx.config().data_dir;
        let fs_ok = std::fs::create_dir_all(data_dir).is_ok();
        components.push(ComponentHealth {
            name: "filesystem".into(),
            status: if fs_ok {
                HealthStatus::Serving
            } else {
                HealthStatus::Degraded
            } as i32,
            detail: if fs_ok {
                String::new()
            } else {
                format!("data directory {} is not writable", data_dir.display())
            },
        });
        if let Some(path) = &self.ctx.config().await_token_file {
            // Always SERVING: readiness probes must pass before a claim can
            // bind and write the token.
            components.push(ComponentHealth {
                name: "auth".into(),
                status: HealthStatus::Serving as i32,
                detail: if self.ctx.awaiting_token() {
                    format!("awaiting token file {}", path.display())
                } else {
                    format!("token from file {}", path.display())
                },
            });
        }
        for provider in self.providers.iter() {
            components.extend(provider.health());
        }
        components
    }
}

fn os_family() -> OsFamily {
    if cfg!(target_os = "linux") {
        OsFamily::Linux
    } else if cfg!(target_os = "macos") {
        OsFamily::Macos
    } else if cfg!(windows) {
        OsFamily::Windows
    } else {
        OsFamily::Unspecified
    }
}

fn os_family_named(name: &str) -> OsFamily {
    match name {
        "linux" => OsFamily::Linux,
        "macos" => OsFamily::Macos,
        "windows" => OsFamily::Windows,
        _ => OsFamily::Unspecified,
    }
}

fn architecture() -> Architecture {
    match std::env::consts::ARCH {
        "x86_64" => Architecture::X8664,
        "aarch64" => Architecture::Arm64,
        _ => Architecture::Unspecified,
    }
}

/// The full OS product string (`OperatingSystem.pretty_name`): Linux's
/// `/etc/os-release` `PRETTY_NAME`, macOS's product version and build, or
/// Windows' product name and build; `name version` when none is known.
fn os_pretty_name(name: &str, version: &str) -> String {
    #[cfg(target_os = "linux")]
    if let Some(p) =
        os_release_pretty(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())
    {
        return p;
    }
    #[cfg(target_os = "macos")]
    {
        let p = macos_pretty(
            &sysctl_string(c"kern.osproductversion"),
            &sysctl_string(c"kern.osversion"),
        );
        if !p.is_empty() {
            return p;
        }
    }
    #[cfg(windows)]
    {
        const KEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
        let p = windows_pretty(
            &windows_bios::value_at(KEY, "ProductName"),
            &windows_bios::value_at(KEY, "CurrentBuild"),
        );
        if !p.is_empty() {
            return p;
        }
    }
    format!("{name} {version}").trim().to_owned()
}

/// `PRETTY_NAME` of an os-release file ("Ubuntu 24.04.3 LTS").
pub fn os_release_pretty(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| {
            v.trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .trim()
                .to_owned()
        })
        .filter(|v| !v.is_empty())
}

/// "macOS 26.5.2 (25F84)" from the product version and build.
pub fn macos_pretty(version: &str, build: &str) -> String {
    match (version.trim(), build.trim()) {
        ("", _) => String::new(),
        (v, "") => format!("macOS {v}"),
        (v, b) => format!("macOS {v} ({b})"),
    }
}

/// "Windows Server 2022 Datacenter 10.0.20348" from the registry's product
/// name and build. Windows 11 still says "Windows 10" in `ProductName`; the
/// build (22000 and later) tells them apart.
pub fn windows_pretty(product: &str, build: &str) -> String {
    let product = product.trim();
    if product.is_empty() {
        return String::new();
    }
    let build = build.trim();
    let product = match build.parse::<u32>() {
        Ok(b) if b >= 22_000 && product.starts_with("Windows 10") => {
            product.replacen("Windows 10", "Windows 11", 1)
        }
        _ => product.to_owned(),
    };
    if build.is_empty() {
        product
    } else {
        format!("{product} 10.0.{build}")
    }
}

/// Whether the guest is a virtual machine (its memory and disk are its own).
fn is_vm(runtime: Runtime) -> bool {
    matches!(
        runtime,
        Runtime::Kubevirt | Runtime::Lume | Runtime::Qemu | Runtime::Hyperv
    )
}

/// A cgroup memory limit file's value (`memory.max`, v1
/// `memory.limit_in_bytes`): `None` for "max" or v1's "no limit" sentinel.
pub fn parse_cgroup_limit(text: &str) -> Option<u64> {
    let v: u64 = text.trim().parse().ok()?;
    (v > 0 && v < (1u64 << 60)).then_some(v)
}

/// The guest's cgroup memory limit and usage, when it runs under one:
/// cgroup v2 (`memory.max`, `memory.current`), then v1.
#[cfg(target_os = "linux")]
fn cgroup_memory() -> Option<(u64, u64)> {
    const FILES: [(&str, &str); 2] = [
        ("/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory.current"),
        (
            "/sys/fs/cgroup/memory/memory.limit_in_bytes",
            "/sys/fs/cgroup/memory/memory.usage_in_bytes",
        ),
    ];
    FILES.iter().find_map(|(max, current)| {
        let limit = parse_cgroup_limit(&std::fs::read_to_string(max).ok()?)?;
        let used = read_trimmed(current).parse().unwrap_or(0);
        Some((limit, used))
    })
}

#[cfg(not(target_os = "linux"))]
fn cgroup_memory() -> Option<(u64, u64)> {
    None
}

fn read_trimmed(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// Whether the kernel is gVisor's synthetic one: older releases (still
/// what Fleet's runsc reports) say exactly "4.4.0", current ones name
/// themselves ("4.19.0-gvisor", and "gVisor" in /proc/version). A real
/// "4.4.0-<abi>-generic" kernel (Ubuntu 16.04) is not gVisor.
pub fn is_gvisor(kernel: &str, proc_version: &str) -> bool {
    kernel == "4.4.0"
        || kernel.to_ascii_lowercase().contains("gvisor")
        || proc_version.to_ascii_lowercase().contains("gvisor")
}

/// Parses a runtime name (`--runtime`, `CUA_ENV_RUNTIME`).
pub fn parse_runtime(name: &str) -> Option<Runtime> {
    Some(match name.to_ascii_lowercase().as_str() {
        "kubevirt" => Runtime::Kubevirt,
        "gvisor" | "runsc" => Runtime::Gvisor,
        "lume" => Runtime::Lume,
        "qemu" => Runtime::Qemu,
        "container" | "docker" | "runc" => Runtime::Container,
        "bare" | "host" => Runtime::Bare,
        "hyperv" | "hyper-v" => Runtime::Hyperv,
        "unknown" => Runtime::Unknown,
        _ => return None,
    })
}

/// The container agent a microVM container runtime runs as PID 1, when
/// `pid1_comm` is one (`runch-agent`: Modal's VM runtime).
pub fn microvm_container_agent(pid1_comm: &str) -> Option<&'static str> {
    match pid1_comm {
        "runch-agent" => Some("runch (microVM)"),
        "kata-agent" => Some("kata (microVM)"),
        _ => None,
    }
}

/// Runtime from the SMBIOS system vendor and product (Linux DMI, the
/// Windows BIOS registry key).
pub fn classify_firmware(vendor: &str, product: &str) -> (Runtime, String) {
    if vendor.contains("KubeVirt") || product.contains("KubeVirt") {
        return (
            Runtime::Kubevirt,
            format!("{vendor} {product}").trim().to_owned(),
        );
    }
    if vendor.contains("QEMU") || product.contains("QEMU") {
        return (Runtime::Qemu, product.to_owned());
    }
    if vendor.contains("Microsoft") && product.contains("Virtual Machine") {
        return (Runtime::Hyperv, product.to_owned());
    }
    if vendor.contains("Apple") {
        return (Runtime::Lume, product.to_owned());
    }
    (Runtime::Bare, String::new())
}

#[cfg(windows)]
mod windows_bios {
    //! `HKLM\HARDWARE\DESCRIPTION\System\BIOS` string values.
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002_u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            hkey: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut core::ffi::c_void,
            len: *mut u32,
        ) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// The value, or "" when it is missing.
    pub fn value(name: &str) -> String {
        value_at("HARDWARE\\DESCRIPTION\\System\\BIOS", name)
    }

    /// A string value under `HKLM\<key>`, or "" when it is missing.
    pub fn value_at(key: &str, name: &str) -> String {
        let key = wide(key);
        let name = wide(name);
        let mut buf = [0u16; 256];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: NUL-terminated key and value names; `buf` holds `len` bytes.
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return String::new();
        }
        let chars = (len as usize / 2).min(buf.len());
        String::from_utf16_lossy(&buf[..chars])
            .trim_end_matches('\0')
            .trim()
            .to_owned()
    }
}

/// Best-effort runtime detection.
pub fn detect_runtime(kernel: &str) -> (Runtime, String) {
    if cfg!(target_os = "linux") {
        // gVisor's synthetic kernel is only ever seen inside runsc, and a
        // Kubernetes pod under containerd (Fleet) has none of docker's or
        // podman's marker files, so the kernel decides on its own.
        if is_gvisor(kernel, &read_trimmed("/proc/version")) {
            return (Runtime::Gvisor, "runsc".into());
        }
        let in_container =
            crate::token_file::in_container() || std::env::var_os("container").is_some();
        if in_container {
            return (Runtime::Container, String::new());
        }
        // A container runtime whose isolation is a microVM boots the
        // rootfs as the VM's root, with its agent as PID 1 (Modal's VM
        // runtime: runch-agent in Cloud Hypervisor): a container, not a
        // bare host.
        if let Some(agent) = microvm_container_agent(&read_trimmed("/proc/1/comm")) {
            return (Runtime::Container, agent.into());
        }
        let vendor = read_trimmed("/sys/class/dmi/id/sys_vendor");
        let product = read_trimmed("/sys/class/dmi/id/product_name");
        return classify_firmware(&vendor, &product);
    }
    #[cfg(windows)]
    {
        // The same SMBIOS strings, from the registry copy Windows keeps.
        let vendor = windows_bios::value("SystemManufacturer");
        let product = windows_bios::value("SystemProductName");
        return classify_firmware(&vendor, &product);
    }
    #[cfg(target_os = "macos")]
    {
        let model = macos_model();
        if model.starts_with("VirtualMac") {
            return (Runtime::Lume, model);
        }
        return (Runtime::Bare, model);
    }
    #[allow(unreachable_code)]
    (Runtime::Unknown, String::new())
}

#[cfg(target_os = "macos")]
fn macos_model() -> String {
    sysctl_string(c"hw.model")
}

/// A string sysctl, or "" when it is missing.
#[cfg(target_os = "macos")]
fn sysctl_string(name: &std::ffi::CStr) -> String {
    let mut size: libc::size_t = 0;
    // SAFETY: size query with a null buffer.
    unsafe {
        if libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return String::new();
        }
    }
    let mut buf = vec![0u8; size];
    // SAFETY: buffer of the reported size.
    unsafe {
        if libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return String::new();
        }
    }
    String::from_utf8_lossy(&buf[..size.saturating_sub(1)]).into_owned()
}

fn display_server() -> DisplayServer {
    if cfg!(target_os = "macos") {
        return DisplayServer::Quartz;
    }
    if cfg!(windows) {
        return DisplayServer::Win32;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        DisplayServer::Wayland
    } else if std::env::var_os("DISPLAY").is_some() {
        DisplayServer::X11
    } else {
        DisplayServer::None
    }
}

/// Filesystem capacity of `path`: (total, used).
fn disk_usage(path: &std::path::Path) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let Ok(cpath) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return (0, 0);
        };
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: valid C string and out-pointer.
        if unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) } != 0 {
            return (0, 0);
        }
        let frag = stat.f_frsize as u64;
        let total = stat.f_blocks as u64 * frag;
        let free = stat.f_bfree as u64 * frag;
        (total, total.saturating_sub(free))
    }
    #[cfg(not(unix))]
    {
        let disks = sysinfo::Disks::new_with_refreshed_list();
        disks
            .iter()
            .filter(|d| path.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .map(|d| (d.total_space(), d.total_space() - d.available_space()))
            .unwrap_or((0, 0))
    }
}

async fn run_power_command(mode: ShutdownMode) {
    let args: &[&str] = match (mode, cfg!(windows)) {
        (ShutdownMode::GuestPoweroff, false) => &["shutdown", "-h", "now"],
        (ShutdownMode::GuestReboot, false) => &["shutdown", "-r", "now"],
        (ShutdownMode::GuestPoweroff, true) => &["shutdown", "/s", "/t", "0"],
        (ShutdownMode::GuestReboot, true) => &["shutdown", "/r", "/t", "0"],
        _ => return,
    };
    match tokio::process::Command::new(args[0])
        .args(&args[1..])
        .status()
        .await
    {
        Ok(status) => tracing::warn!(?status, ?mode, "guest power command finished"),
        Err(error) => tracing::error!(%error, ?mode, "guest power command failed"),
    }
}

/// Only the root-token holder (the Space's owner through its own
/// credential) may attach or detach a relay: never a relay caller, a view
/// share or a viewer ticket, and never an unauthenticated loopback caller
/// when a token exists.
fn require_root(caller: &crate::auth::CallerIdentity, what: &str) -> Result<(), Status> {
    if caller.asserted || caller.viewer.is_some() {
        return Err(status(
            Code::PermissionDenied,
            ErrorReason::PermissionDenied,
            format!("only the Space's owner (its root token) may {what}"),
        ));
    }
    Ok(())
}

#[tonic::async_trait]
impl SystemService for SystemServiceImpl {
    async fn get_capabilities(
        &self,
        _request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<GetCapabilitiesResponse>, Status> {
        let kernel = sysinfo::System::kernel_version().unwrap_or_default();
        let (runtime, runtime_detail) = match self
            .ctx
            .config()
            .runtime_override
            .as_deref()
            .and_then(parse_runtime)
        {
            Some(runtime) => (runtime, "configured".to_owned()),
            None => detect_runtime(&kernel),
        };
        let display_server = self
            .providers
            .iter()
            .find_map(|p| p.display_server())
            .unwrap_or_else(display_server);
        let displays = self.providers.iter().flat_map(|p| p.displays()).collect();
        let config = self.ctx.config();
        let has_mcp = self
            .core_features
            .iter()
            .any(|f| f.name == "driver" && f.supported)
            && config.enable_mcp;
        Ok(Response::new(GetCapabilitiesResponse {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: cua_proto::ENV_PROTOCOL_VERSION,
            protocol_revision: cua_proto::ENV_PROTOCOL_REVISION,
            os: Some({
                let name = sysinfo::System::name().unwrap_or_default();
                let version = sysinfo::System::os_version().unwrap_or_default();
                OperatingSystem {
                    family: config
                        .os_override
                        .as_deref()
                        .map_or_else(os_family, os_family_named) as i32,
                    pretty_name: os_pretty_name(&name, &version),
                    name,
                    version,
                    kernel,
                }
            }),
            runtime: runtime as i32,
            runtime_detail,
            arch: architecture() as i32,
            display_server: display_server as i32,
            displays,
            features: self.features(),
            hostname: sysinfo::System::host_name().unwrap_or_default(),
            // Await-token-file mode: initialized once the file holds a token
            // (Init cannot and need not install one).
            initialized: self.ctx.init_state().initialized
                || (self.ctx.access_mode() == crate::auth::AccessMode::AwaitTokenFile
                    && !self.ctx.awaiting_token()),
            boot_time: Some(timestamp(
                std::time::UNIX_EPOCH + Duration::from_secs(sysinfo::System::boot_time()),
            )),
            side_channels: Some(SideChannels {
                media_ws_path: cua_proto::metadata::MEDIA_WS_PATH.into(),
                media_quic_port: config.media_quic_port as u32,
                tunnel_ws_path: cua_proto::metadata::TUNNEL_WS_PATH.into(),
                files_http_path: cua_proto::metadata::FILES_PATH.into(),
                mcp_http_path: if has_mcp {
                    cua_proto::metadata::MCP_PATH.into()
                } else {
                    String::new()
                },
            }),
            limits: Some(Limits {
                max_chunk_bytes: crate::config::MAX_CHUNK_BYTES,
                max_message_bytes: crate::config::MAX_MESSAGE_BYTES,
                preferred_chunk_bytes: crate::config::PREFERRED_CHUNK_BYTES,
                default_scrollback_bytes: config.default_scrollback_bytes,
            }),
            machine_seal_public_key: self.ctx.machine_seal_public_key(),
        }))
    }

    async fn init(&self, request: Request<InitRequest>) -> Result<Response<InitResponse>, Status> {
        // Gateway bootstrap: an Init that reached us without a token (the
        // driver has none yet on a non-loopback bind) must carry one.
        let bootstrapping = self.ctx.access_mode() == crate::auth::AccessMode::Bootstrap
            && !request
                .extensions()
                .get::<crate::auth::CallerIdentity>()
                .is_some_and(|c| c.token_verified);
        let body = request.into_inner();
        // Await-token-file mode: the token is never accepted over the
        // network. Presenting the current one is a no-op.
        if self.ctx.access_mode() == crate::auth::AccessMode::AwaitTokenFile
            && !body.token.is_empty()
            && self.ctx.auth().token().as_deref() != Some(body.token.as_str())
        {
            return Err(status(
                Code::PermissionDenied,
                ErrorReason::PermissionDenied,
                "the access token is managed by the claim's token file; Init cannot set or rotate it",
            ));
        }
        if bootstrapping && body.token.is_empty() {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::NotInitialized,
                "uninitialized: the first Init must carry a token (gateway bootstrap)",
            ));
        }
        // Validate everything before changing anything.
        if !body.default_user.is_empty() {
            crate::process::spawn::lookup_user(&body.default_user).map_err(|e| {
                crate::error::invalid(format!("default_user {:?}: {e}", body.default_user))
            })?;
        }
        let workdir = if body.default_workdir.is_empty() {
            None
        } else {
            let path = PathBuf::from(&body.default_workdir);
            if !path.is_absolute() || !path.is_dir() {
                return Err(crate::error::invalid(format!(
                    "default_workdir {:?} must be an existing absolute directory",
                    body.default_workdir
                )));
            }
            Some(path)
        };
        let mut bootstrap_claimed = false;
        if bootstrapping {
            // First Init wins atomically; anyone later must present its token.
            if !self.ctx.auth().claim_token(&body.token) {
                return Err(crate::error::unauthenticated(
                    "driver already initialized: present its token",
                ));
            }
            bootstrap_claimed = true;
            tracing::info!(
                "gateway bootstrap: token installed by the first Init; bootstrap mode ended"
            );
            if let Some(path) = &self.ctx.config().bootstrap_token_file {
                match persist_token(path, &body.token) {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), "bootstrap token persisted (0600)")
                    }
                    Err(error) => {
                        tracing::warn!(path = %path.display(), %error, "could not persist the bootstrap token; it lives in memory only")
                    }
                }
            }
        }
        if !body.ca_bundle.is_empty() {
            install_ca_bundle(&self.ctx, &body.ca_bundle)?;
        }
        let mut clock_adjusted = false;
        if let Some(now) = &body.now {
            clock_adjusted = maybe_step_clock(system_time(now));
        }
        let token_changed = bootstrap_claimed || self.ctx.auth().set_token(&body.token);
        if let Some(access) = &body.audio_uplink {
            self.ctx.set_audio_uplink(access);
        }
        if let Some(presence) = &body.presence {
            self.ctx.set_presence_settings(presence);
        }
        self.ctx.update_init(|state| {
            for (key, value) in &body.env {
                state.env.insert(key.clone(), value.clone());
            }
            if !body.default_user.is_empty() {
                state.default_user = Some(body.default_user.clone());
            }
            if let Some(workdir) = workdir {
                state.default_workdir = Some(workdir);
            }
            for (key, value) in &body.labels {
                state.labels.insert(key.clone(), value.clone());
            }
            state.initialized = true;
        });
        if token_changed {
            tracing::info!("access token installed or rotated by Init");
            self.ctx.notify_token_rotated();
        }
        Ok(Response::new(InitResponse {
            token_changed,
            clock_adjusted,
        }))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let components = self.health_components();
        let worst = components
            .iter()
            .map(|c| c.status)
            .max()
            .unwrap_or(HealthStatus::Serving as i32);
        Ok(Response::new(HealthResponse {
            status: worst,
            components,
            uptime: Some(proto_duration(self.ctx.uptime())),
        }))
    }

    async fn metrics(
        &self,
        request: Request<MetricsRequest>,
    ) -> Result<Response<MetricsResponse>, Status> {
        let body = request.into_inner();
        let disk_path = if body.disk_path.is_empty() {
            self.ctx
                .init_state()
                .default_workdir
                .or_else(crate::config::home_dir)
                .unwrap_or_else(|| PathBuf::from("/"))
        } else {
            PathBuf::from(body.disk_path)
        };
        let vm = is_vm(self.runtime());
        let sample = tokio::task::spawn_blocking(move || {
            use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
            let mut system = System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                    .with_memory(MemoryRefreshKind::everything()),
            );
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            system.refresh_cpu_usage();
            let networks = sysinfo::Networks::new_with_refreshed_list();
            let (rx, tx) = networks.iter().fold((0u64, 0u64), |(rx, tx), (_, data)| {
                (rx + data.total_received(), tx + data.total_transmitted())
            });
            let (disk_total, disk_used) = disk_usage(&disk_path);
            // A VM's memory is its own; a container's is its cgroup limit
            // when it has one, else only the host's.
            let (mem_total, mem_used, mem_limited) = match cgroup_memory() {
                Some((limit, used)) if !vm && limit <= system.total_memory().max(limit) => {
                    (limit, used.min(limit), true)
                }
                _ => (system.total_memory(), system.used_memory(), vm),
            };
            (
                system.cpus().len() as u32,
                system.global_cpu_usage() as f64,
                System::load_average().one,
                (mem_total, mem_used, mem_limited),
                (disk_total, disk_used, vm),
                rx,
                tx,
            )
        })
        .await
        .map_err(|e| crate::error::internal(e.to_string()))?;
        let (cpu_count, cpu, load, mem, disk, rx, tx) = sample;
        let (mem_total, mem_used, memory_limited) = mem;
        let (disk_total, disk_used, disk_limited) = disk;
        Ok(Response::new(MetricsResponse {
            sampled_at: Some(crate::util::now_ts()),
            cpu_count,
            cpu_percent: cpu,
            load_average_1m: load,
            memory_total_bytes: mem_total,
            memory_used_bytes: mem_used,
            disk_total_bytes: disk_total,
            disk_used_bytes: disk_used,
            memory_limited,
            disk_limited,
            managed_process_count: self
                .ctx
                .managed_processes()
                .load(std::sync::atomic::Ordering::SeqCst),
            media_session_count: self
                .ctx
                .media_sessions()
                .load(std::sync::atomic::Ordering::SeqCst),
            network_rx_bytes: rx,
            network_tx_bytes: tx,
        }))
    }

    async fn shutdown(
        &self,
        request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        let body = request.into_inner();
        let mode = ShutdownMode::try_from(body.mode)
            .ok()
            .filter(|m| *m != ShutdownMode::Unspecified)
            .ok_or_else(|| crate::error::invalid("mode is required"))?;
        if matches!(
            mode,
            ShutdownMode::GuestPoweroff | ShutdownMode::GuestReboot
        ) && !self.ctx.config().allow_guest_power
        {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::PermissionDenied,
                "guest power control is disabled (start the driver with --allow-guest-power)",
            ));
        }
        let grace =
            crate::util::duration(body.grace_period.as_ref()).unwrap_or(Duration::from_secs(5));
        tracing::warn!(?mode, reason = %body.reason, "shutdown requested");
        let ctx = self.ctx.clone();
        tokio::spawn(async move {
            // Let the response reach the caller first.
            tokio::time::sleep(Duration::from_millis(100)).await;
            match mode {
                ShutdownMode::DriverExit => ctx.request_shutdown(false),
                ShutdownMode::DriverRestart => ctx.request_shutdown(true),
                ShutdownMode::GuestPoweroff | ShutdownMode::GuestReboot => {
                    ctx.request_shutdown(false);
                    tokio::time::sleep(grace.min(Duration::from_secs(60))).await;
                    run_power_command(mode).await;
                }
                ShutdownMode::Unspecified => {}
            }
        });
        Ok(Response::new(ShutdownResponse {}))
    }

    type DiagnoseStream = std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<DiagnoseResponse, Status>> + Send + 'static>,
    >;

    async fn diagnose(
        &self,
        request: Request<DiagnoseRequest>,
    ) -> Result<Response<Self::DiagnoseStream>, Status> {
        let options = request.into_inner().options.unwrap_or_default();
        let receiver = self.diagnose.start(options)?;
        let stream = tokio_stream::wrappers::ReceiverStream::new(receiver).map(Ok);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn diagnose_once(
        &self,
        request: Request<DiagnoseOnceRequest>,
    ) -> Result<Response<DiagnoseOnceResponse>, Status> {
        let options = request.into_inner().options.unwrap_or_default();
        let report = self.diagnose.run_once(options).await?;
        Ok(Response::new(DiagnoseOnceResponse {
            report: Some(report),
        }))
    }

    async fn attach_relay(
        &self,
        request: Request<AttachRelayRequest>,
    ) -> Result<Response<AttachRelayResponse>, Status> {
        require_root(&crate::auth::caller(&request), "attach a relay")?;
        let body = request.into_inner();
        self.ctx
            .attach_relay(
                &body.relay_url,
                &body.machine_token,
                &body.machine_id,
                &body.relay_jwks_json,
                &body.owner,
                &body.owner_email,
            )
            .map_err(|e| status(Code::FailedPrecondition, ErrorReason::Unspecified, e))?;
        Ok(Response::new(AttachRelayResponse {
            machine_id: body.machine_id,
        }))
    }

    async fn detach_relay(
        &self,
        request: Request<DetachRelayRequest>,
    ) -> Result<Response<DetachRelayResponse>, Status> {
        require_root(&crate::auth::caller(&request), "detach the relay")?;
        Ok(Response::new(DetachRelayResponse {
            detached: self.ctx.detach_relay(),
        }))
    }

    async fn create_viewer_ticket(
        &self,
        request: Request<CreateViewerTicketRequest>,
    ) -> Result<Response<CreateViewerTicketResponse>, Status> {
        let caller = crate::auth::caller(&request);
        if caller.viewer.is_some() {
            return Err(status(
                Code::PermissionDenied,
                ErrorReason::PermissionDenied,
                "a viewer ticket cannot mint viewer tickets",
            ));
        }
        let body = request.into_inner();
        let ttl = crate::util::duration(body.ttl.as_ref())
            .filter(|ttl| !ttl.is_zero())
            .unwrap_or(DEFAULT_VIEWER_TTL)
            .min(crate::auth::MAX_TICKET_TTL);
        let policy = match SessionPolicy::try_from(body.policy) {
            Ok(SessionPolicy::Unspecified) | Err(_) => SessionPolicy::AllowActivation,
            Ok(policy) => policy,
        };
        let files_root = if body.files_root.is_empty() {
            None
        } else {
            let resolver = crate::filesystem::PathResolver::new(self.ctx.clone());
            let path = resolver.resolve(&body.files_root)?;
            tokio::fs::create_dir_all(&path)
                .await
                .map_err(|e| crate::error::io_status(&e, &path))?;
            let canonical = tokio::fs::canonicalize(&path)
                .await
                .map_err(|e| crate::error::io_status(&e, &path))?;
            Some(canonical.display().to_string())
        };
        let base = body.principal.or(caller.principal).unwrap_or_default();
        let id = if base.id.is_empty() {
            crate::util::random_id(6)
        } else {
            base.id.clone()
        };
        let grant = crate::auth::ViewerGrant {
            policy: policy as i32,
            clipboard: body.clipboard,
            files_root: files_root.clone(),
            audio_uplink: body.audio_uplink,
            principal_id: if id.starts_with("viewer:") {
                id
            } else {
                format!("viewer:{id}")
            },
            display_name: if base.display_name.is_empty() {
                "Web viewer".into()
            } else {
                base.display_name
            },
        };
        let resource = serde_json::to_string(&grant).map_err(|e| {
            status(
                Code::Internal,
                ErrorReason::Internal,
                format!("viewer grant: {e}"),
            )
        })?;
        let principal = grant.principal();
        let (ticket, expires_at) = self.ctx.mint_ticket(
            crate::auth::TicketScope::Viewer,
            &resource,
            Some(&principal),
            ttl,
        );
        // Hints for the page (the server enforces the grant either way).
        let mut viewer_path = format!("{}/#ticket={ticket}", cua_proto::metadata::VIEWER_PATH);
        if let Some(root) = &files_root {
            viewer_path.push_str("&files=");
            viewer_path.extend(percent_encoding::utf8_percent_encode(
                root,
                percent_encoding::NON_ALPHANUMERIC,
            ));
        }
        if !grant.clipboard {
            viewer_path.push_str("&clipboard=0");
        }
        Ok(Response::new(CreateViewerTicketResponse {
            viewer_path,
            ticket,
            expires_at: Some(timestamp(expires_at)),
            files_root: files_root.unwrap_or_default(),
        }))
    }
}

/// Lifetime of a viewer ticket when the request leaves it unset.
pub const DEFAULT_VIEWER_TTL: Duration = Duration::from_secs(3600);

/// Writes the bootstrap token to `path` with mode 0600 (atomic rename).
fn persist_token(path: &std::path::Path, token: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        std::io::Write::write_all(&mut file, format!("{token}\n").as_bytes())?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    std::fs::rename(&tmp, path)
}

/// Stores the CA bundle for new processes and, as root on Linux, adds it to
/// the system trust store.
fn install_ca_bundle(ctx: &ServerContext, pem: &[u8]) -> Result<(), Status> {
    if !pem.windows(27).any(|w| w == b"-----BEGIN CERTIFICATE-----") {
        return Err(crate::error::invalid("ca_bundle is not PEM"));
    }
    let dir = &ctx.config().data_dir;
    std::fs::create_dir_all(dir).map_err(|e| crate::error::io_status(&e, dir))?;
    let path = dir.join("ca-bundle.pem");
    std::fs::write(&path, pem).map_err(|e| crate::error::io_status(&e, &path))?;
    ctx.update_init(|state| {
        state
            .env
            .insert("NODE_EXTRA_CA_CERTS".into(), path.display().to_string());
    });
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        let root = unsafe { libc::geteuid() } == 0;
        let store = std::path::Path::new("/usr/local/share/ca-certificates");
        if root && store.is_dir() && std::fs::write(store.join("cua-env-init.crt"), pem).is_ok() {
            let _ = std::process::Command::new("update-ca-certificates").status();
        }
    }
    Ok(())
}

/// Steps the clock when it is off by more than a second and the driver may
/// (root on Linux). Returns whether it did.
fn maybe_step_clock(now: std::time::SystemTime) -> bool {
    let skew = match now.duration_since(std::time::SystemTime::now()) {
        Ok(ahead) => ahead,
        Err(behind) => behind.duration(),
    };
    if skew <= Duration::from_secs(1) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            return false;
        }
        let since = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let ts = libc::timespec {
            tv_sec: since.as_secs() as libc::time_t,
            tv_nsec: since.subsec_nanos() as _,
        };
        // SAFETY: valid timespec.
        return unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) } == 0;
    }
    #[allow(unreachable_code)]
    false
}

#[cfg(test)]
mod gvisor_tests {
    #[test]
    fn a_microvm_container_agent_as_pid1_is_a_container() {
        assert_eq!(
            super::microvm_container_agent("runch-agent"),
            Some("runch (microVM)")
        );
        assert_eq!(super::microvm_container_agent("systemd"), None);
        assert_eq!(super::microvm_container_agent("bash"), None);
    }

    #[test]
    fn gvisor_kernels_are_recognised() {
        assert!(super::is_gvisor("4.4.0", ""));
        assert!(super::is_gvisor("4.19.0-gvisor", ""));
        assert!(super::is_gvisor(
            "6.1",
            "Linux version 4.19.0 #1 SMP Sun Jan 10 15:06:54 PST 2016 gVisor"
        ));
        // Ubuntu 16.04's real 4.4.0 kernels are not gVisor.
        assert!(!super::is_gvisor(
            "4.4.0-210-generic",
            "Linux version 4.4.0-210-generic (buildd@lgw01-amd64-009)"
        ));
        assert!(!super::is_gvisor(
            "6.8.0-45-generic",
            "Linux version 6.8.0-45-generic (buildd@lcy02)"
        ));
    }
}

#[cfg(test)]
mod firmware_tests {
    use super::{classify_firmware, Runtime};

    #[test]
    fn smbios_strings_name_the_hypervisor() {
        // QEMU q35 as a Windows guest reports it (BIOS registry key).
        assert_eq!(
            classify_firmware("QEMU", "Standard PC (Q35 + ICH9, 2009)").0,
            Runtime::Qemu
        );
        assert_eq!(classify_firmware("KubeVirt", "None").0, Runtime::Kubevirt);
        assert_eq!(
            classify_firmware("Microsoft Corporation", "Virtual Machine").0,
            Runtime::Hyperv
        );
        assert_eq!(classify_firmware("Dell Inc.", "XPS 13").0, Runtime::Bare);
        assert_eq!(classify_firmware("", "").0, Runtime::Bare);
    }
}

#[cfg(test)]
mod os_info_tests {
    use super::{macos_pretty, os_release_pretty, parse_cgroup_limit, windows_pretty};

    #[test]
    fn pretty_names() {
        let os_release = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\nID=ubuntu\n";
        assert_eq!(
            os_release_pretty(os_release).as_deref(),
            Some("Ubuntu 24.04.3 LTS")
        );
        assert_eq!(
            os_release_pretty("PRETTY_NAME='Arch Linux'").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(os_release_pretty("NAME=x"), None);
        assert_eq!(macos_pretty("26.5.2", "25F84"), "macOS 26.5.2 (25F84)");
        assert_eq!(macos_pretty("26.5.2", ""), "macOS 26.5.2");
        assert_eq!(macos_pretty("", "25F84"), "");
        assert_eq!(
            windows_pretty("Windows Server 2022 Datacenter", "20348"),
            "Windows Server 2022 Datacenter 10.0.20348"
        );
        assert_eq!(
            windows_pretty("Windows 10 Pro", "26100"),
            "Windows 11 Pro 10.0.26100"
        );
        assert_eq!(
            windows_pretty("Windows 10 Pro", "19045"),
            "Windows 10 Pro 10.0.19045"
        );
        assert_eq!(windows_pretty("", "1"), "");
    }

    #[test]
    fn cgroup_limits() {
        assert_eq!(parse_cgroup_limit("4294967296\n"), Some(4 << 30));
        assert_eq!(parse_cgroup_limit("max\n"), None);
        assert_eq!(parse_cgroup_limit("9223372036854771712"), None); // v1: no limit
        assert_eq!(parse_cgroup_limit("0"), None);
    }
}

#[cfg(test)]
mod os_override_tests {
    use super::{os_family_named, OsFamily};

    #[test]
    fn os_override_names_map_to_families() {
        assert_eq!(os_family_named("linux"), OsFamily::Linux);
        assert_eq!(os_family_named("macos"), OsFamily::Macos);
        assert_eq!(os_family_named("windows"), OsFamily::Windows);
        assert_eq!(os_family_named("plan9"), OsFamily::Unspecified);
    }
}
