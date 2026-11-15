//! Lume backend: macOS and Linux arm64 guests on Apple Virtualization.framework
//! through `lume serve` (port of `cua_sandbox/runtime/lume.py`).
//!
//! * **macOS / OCI images**: pull once into a stopped `cua-base-<sha12>` VM
//!   (same naming as the Python runtime, so bases are shared), then clone
//!   (APFS clonefile, instant) and run.
//! * **Linux disk images** ([`ImageSource::Disk`]): the qcow2/raw disk is
//!   converted to a raw image in `~/.cua/vmm/lume/<name>/disk.img`, a Linux VM
//!   shell is created through the API, and the VM is launched with
//!   `lume run --detach --disk-path <disk> --mount <cloud-init seed>` so the
//!   same NoCloud seed as the QEMU backend provisions SSH access.
//!
//! [`LumeRuntime::ensure_serving`] makes the backend zero-setup: if the API is
//! not reachable it starts `lume serve`, and when the binary is missing it sets
//! up the built-in Lume ([`builtin`]) unless `runtime.lume` is `system`.

pub mod builtin;
pub mod client;
pub mod derive;
pub mod gpu;
pub mod lease;
pub mod owned;
pub mod ssh;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_home::channel::CuaChannel;

use crate::error::{Result, VmmError};
use crate::exec::{GuestExec, PrefixExec};
use crate::host;
use crate::runtime::Runtime;
use crate::types::*;

pub use builtin::LumeCmd;
pub use client::{CreateVm, LumeClient, RunVm, SetVm, SharedDir, VmDetails};
pub use owned::{OwnedKind, OwnedVm, OwnedVms};
pub use ssh::LumeSshExec;

/// File name of the env token in the macOS setup share (guest:
/// `/Volumes/My Shared Files/env-token`).
pub const MACOS_SETUP_TOKEN_FILE: &str = "env-token";

/// Where a macOS guest on Lume sees shared directories: each one under its
/// name (the host directory's last path component). The setup share is
/// `setup`; [`crate::StartSpec::mounts`] add more.
pub const MACOS_SHARE_ROOT: &str = "/Volumes/My Shared Files";

/// macOS VMs Apple's macOS license and Virtualization framework allow at
/// once on one Mac.
pub const MACOS_VMS_PER_MAC: u32 = 2;

/// The capacity error for a macOS VM past [`MACOS_VMS_PER_MAC`].
pub fn macos_limit_message(others: &[String]) -> String {
    format!(
        "this Mac already runs {} macOS VMs ({}); Apple allows {MACOS_VMS_PER_MAC} at once. \
         Stop or delete one (`cua spaces ls`, `cua spaces delete`) and try again",
        others.len(),
        others.join(", ")
    )
}

/// The capacity error when Apple Virtualization refused the boot for the
/// limit and the VMs holding it are not ones this Lume lists (another Cua
/// Spaces, another Lume, any app running macOS VMs).
pub fn macos_limit_unlisted_message() -> String {
    format!(
        "this Mac is already running its limit of macOS VMs; Apple allows {MACOS_VMS_PER_MAC} at \
         once, and others outside this Space (another Cua Spaces, Lume or a virtualization app) \
         hold them. Stop or delete one of them and try again"
    )
}

/// `secs` since the Unix epoch as Lume stamps its log lines
/// (`2026-11-07T04:07:15Z`, UTC), so stamps compare as text.
pub fn utc_stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// The stamp of the log line that holds byte `at` of `log` (the last
/// `[2026-11-07T04:07:15Z]` before it), if there is one close by.
fn line_stamp(log: &[u8], at: usize) -> Option<String> {
    let start = at.saturating_sub(600);
    let head = &log[start..at];
    (0..head.len().saturating_sub(22)).rev().find_map(|i| {
        let w = &head[i..i + 22];
        let ok = w[0] == b'['
            && w[21] == b']'
            && w[5] == b'-'
            && w[11] == b'T'
            && w[20] == b'Z'
            && w[1..5].iter().all(u8::is_ascii_digit);
        ok.then(|| String::from_utf8_lossy(&w[1..21]).into_owned())
    })
}

/// Whether `log` (bytes of a `lume serve` log) says, in a line stamped
/// `since` or later, that Apple Virtualization refused to run VM `name` for
/// the limit on active virtual machines. Lume's log has lines of many MB
/// (whole API responses) and several `lume serve` write into one file at
/// their own offsets, so this looks for the refusal itself, reads a little
/// around it and trusts its stamp rather than its place in the file.
pub fn log_says_vm_limit(log: &[u8], name: &str, since: &str) -> bool {
    const NEEDLE: &[u8] = b"number of virtual machines exceeds the limit";
    const AROUND: usize = 700;
    let tag = format!("name={name}");
    let mut from = 0;
    while let Some(at) = log[from..]
        .windows(NEEDLE.len())
        .position(|w| w == NEEDLE)
        .map(|p| p + from)
    {
        let near = &log[at.saturating_sub(AROUND)..(at + AROUND).min(log.len())];
        if String::from_utf8_lossy(near)
            .split_whitespace()
            .any(|t| t == tag)
            && line_stamp(log, at).is_some_and(|stamp| stamp.as_str() >= since)
        {
            return true;
        }
        from = at + NEEDLE.len();
    }
    false
}

/// The host a macOS guest's published ports are dialed on once they are
/// forwarded over vsock ([`client::VmDetails::forwards`]).
pub const VSOCK_HOST: &str = "127.0.0.1";

/// How long a start keeps waiting for a guest's vsock path after its
/// network address turned out unreachable (Local Network privacy or a VPN)
/// before it reports that: the agent starts with the guest's services, so
/// a guest that has one answers well within this.
pub const VSOCK_GRACE: Duration = Duration::from_secs(30);

/// Where a macOS guest sees the shared host directory `host`.
pub fn macos_share_path(host: &std::path::Path) -> String {
    format!(
        "{MACOS_SHARE_ROOT}/{}",
        host.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    )
}

/// The shared directories for `mounts` of a macOS guest: each one's guest
/// path must be where Lume shares it ([`macos_share_path`]), and no name
/// may repeat (or be the setup share's).
fn macos_mount_shares(mounts: &[crate::Mount]) -> Result<Vec<SharedDir>> {
    crate::types::check_mounts(BackendKind::Lume, mounts)?;
    let mut names = std::collections::BTreeSet::from(["setup".to_string()]);
    let mut out = Vec::new();
    for m in mounts {
        let at = macos_share_path(&m.host);
        if m.guest.trim_end_matches('/') != at {
            return Err(VmmError::invalid(format!(
                "a macOS guest sees the shared directory {} at {at:?}, not {:?}",
                m.host.display(),
                m.guest
            )));
        }
        let name = at.rsplit('/').next().unwrap_or_default().to_string();
        if !names.insert(name.clone()) {
            return Err(VmmError::invalid(format!(
                "two shared directories named {name:?} (a macOS guest tells them apart by name)"
            )));
        }
        out.push(SharedDir {
            host_path: m.host.display().to_string(),
            read_only: m.read_only,
        });
    }
    Ok(out)
}

/// Official installer (same one the docs and the Python SDK point to).
pub const INSTALL_URL: &str =
    "https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh";

/// Configuration for [`LumeRuntime`].
#[derive(Clone, Debug)]
pub struct LumeConfig {
    /// The Cua channel this Lume belongs to (default: this process's,
    /// [`CuaChannel::current`]). It decides the default URL and port, the
    /// VM directory, the binary name and the fallback ports: Stable's
    /// `lume` on `:7777` with VMs in `~/.lume`, Local's `lume-local` on
    /// `:7787` with VMs in `~/.lume-local`.
    pub channel: CuaChannel,
    /// API base URL (default `$LUME_API`, else the channel's:
    /// `http://127.0.0.1:7777` for Stable, `http://127.0.0.1:7787` for Local).
    pub url: String,
    /// Start `lume serve` when the API is unreachable.
    pub spawn_serve: bool,
    /// Run the official installer when the `lume` binary is missing.
    pub allow_install: bool,
    /// State for Linux disk conversions and seeds (default `~/.cua/vmm/lume`).
    pub root: PathBuf,
    /// How long to wait for a booted VM to report an IP.
    pub ip_timeout: Duration,
    /// How long a registry pull may take.
    pub pull_timeout: Duration,
    /// Host DHCP lease file consulted (read-only) for guest IPs.
    pub leases_path: String,
}

impl Default for LumeConfig {
    fn default() -> Self {
        let channel = CuaChannel::current();
        Self {
            channel,
            url: serving_url(&crate::local_network::lume_base_with(
                channel,
                std::env::var("LUME_API").ok().as_deref(),
            )),
            spawn_serve: true,
            allow_install: false,
            root: default_root(),
            ip_timeout: Duration::from_secs(300),
            pull_timeout: Duration::from_secs(3600),
            leases_path: lease::LEASES_PATH.into(),
        }
    }
}

/// The Lume backend.
/// A protected VM's current gateway ([`LumeRuntime::gateway_network`]).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct LumeGateway {
    /// `gateway:<socket>`, for Lume's run.
    network: String,
    /// Guest port -> the host loopback port it is forwarded on.
    ports: std::collections::BTreeMap<u16, u16>,
}

const GATEWAY_FILE: &str = "gateway.json";

/// A Lume VM's key in the tree book ([`crate::tree`]).
fn tree_key(name: &str) -> String {
    crate::tree::key("lume", name)
}
const GATEWAY_SUBNET: &str = "10.0.3.0/24";
const GATEWAY_IP: &str = "10.0.3.1";
const GATEWAY_HOST_IP: &str = "10.0.3.2";
const GATEWAY_MAC: &str = "5a:94:ef:e4:0c:de";
/// The address the gateway's DHCP gives the guest (by its MAC).
const GUEST_IP: &str = "10.0.3.15";

/// Whether a VM with `settings` runs behind a gateway: under the secure
/// default, unless the user chose No protection.
fn protected(settings: &crate::network::NetworkSettings) -> bool {
    crate::network::Mode::from_env() == crate::network::Mode::Secure
        && settings.preset != crate::network::NetworkPreset::Unprotected
}

/// What a macOS VM's gateway knows: the guest's resolver is the gateway
/// (open on 53); the host is the gateway's host address, opened only by
/// the localhost preset's ports; the Mac's own addresses and the daemon's
/// ports stay closed.
fn gateway_context() -> crate::network::NetworkContext {
    let ip = |s: &str| -> std::net::IpAddr { s.parse().expect("a valid address") };
    crate::network::NetworkContext {
        resolvers: vec![ip(GATEWAY_IP)],
        host_aliases: vec![ip(GATEWAY_HOST_IP)],
        host_addrs: crate::network::host_addresses(),
        // The tree range: the gateway carries it to the tree's running
        // members only ([`crate::tree`]).
        tree: crate::tree::context_tree(),
        other_trees: vec![],
        daemon_ports: crate::network::daemon_ports(),
    }
}

/// A short private folder for one VM's gateway sockets (a unix socket path
/// is at most 104 bytes): under the temporary directory, named after the
/// VM's directory.
fn gateway_run_dir(vm_dir: &std::path::Path) -> PathBuf {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(vm_dir.display().to_string().as_bytes());
    let hex: String = h.iter().take(6).map(|b| format!("{b:02x}")).collect();
    std::env::temp_dir().join(format!("cua-lgw-{hex}"))
}

pub struct LumeRuntime {
    cfg: LumeConfig,
    client: LumeClient,
    /// The host preference GPU acceleration sets ([`gpu`]).
    gpu_pref: Arc<dyn gpu::GpuPreference>,
    /// The `lume serve` URL already checked ([`builtin::judge_server`]).
    accepted: std::sync::Mutex<Option<String>>,
    /// The version of a too-old Lume `runtime.lume = system` made cua use,
    /// named in a failed start.
    old_lume: std::sync::Mutex<Option<String>>,
    /// One restart of a VM whose gateway ended at a time
    /// ([`LumeRuntime::mend_gateway`]).
    mending: tokio::sync::Mutex<()>,
    /// VMs restarted behind a new gateway whose callers have not been told
    /// yet ([`Runtime::mend`] says so once): their open connections are
    /// stale whoever noticed first.
    mended: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

/// Configured `lume serve` URL -> the one this process moved to because an
/// old Lume answers at the configured one ([`LumeRuntime::ensure_serving`]).
static MOVED: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// The `lume serve` URL to use for `configured`: where this process moved
/// to, if it did.
pub fn serving_url(configured: &str) -> String {
    let configured = configured.trim_end_matches('/');
    MOVED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(from, _)| from == configured)
        .map_or_else(|| configured.to_string(), |(_, to)| to.clone())
}

fn remember_move(from: &str, to: &str) {
    let from = from.trim_end_matches('/').to_string();
    let mut moved = MOVED.lock().unwrap_or_else(|e| e.into_inner());
    moved.retain(|(f, _)| *f != from);
    moved.push((from, to.trim_end_matches('/').to_string()));
}

/// The version of the `lume serve` at `client`: what it reports, else (on
/// this Mac) its executable's `--version`.
async fn server_version(
    channel: CuaChannel,
    config_root: &std::path::Path,
    client: &LumeClient,
) -> Option<String> {
    if let Some(v) = client.reported_version().await {
        return Some(v);
    }
    let url = client.base();
    if !builtin::is_loopback(&url) {
        return None;
    }
    let port = url_port(&url)?;
    let config_root = config_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        // Only a Lume (`lume`, or a channel's own such as `lume-local`) is
        // asked for its version, never, say, a test's own fake server
        // process.
        builtin::serving_binary(port)
            .filter(|b| {
                b.file_name()
                    .is_some_and(|n| CuaChannel::ALL.iter().any(|c| n == c.lume_bin_name()))
            })
            .and_then(|b| builtin::binary_version_in(channel, &config_root, &b))
    })
    .await
    .ok()
    .flatten()
}

fn url_port(url: &str) -> Option<u16> {
    url.trim_end_matches('/')
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
}

/// The reference `lume pull` takes: `tag-ref@sha256:…` (how the daemon
/// names a Lume image so its cached base follows the digest) pulls by tag,
/// since lume cannot pull by digest; anything else is used as given.
pub fn lume_pull_ref(reference: &str) -> &str {
    match reference.rsplit_once('@') {
        Some((tag_ref, digest)) if digest.starts_with("sha256:") && tag_ref.contains(':') => {
            tag_ref
        }
        _ => reference,
    }
}

/// Stable base-VM name for an OCI reference: `cua-base-<sha256[:12]>`
/// (identical to `_base_vm_name` in the Python runtime). Pass
/// `tag-ref@sha256:…` so a moved tag (`macos:26`) gets a fresh base
/// instead of cloning the stale one cached under the tag.
pub fn base_vm_name(reference: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(reference.as_bytes());
    let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
    format!("cua-base-{}", &hex[..12])
}

/// The warm VMs of base VM `base` (`cua-base-<sha12>` →
/// `cua-warm-<sha12>` and `cua-warm-<sha12>-2`): the image booted to
/// readiness with no identity and saved ([`LumeRuntime::prepare_warm`]).
/// Two, each with its own MAC address, because macOS runs at most two
/// macOS guests per host and every resume of one state has its address.
pub fn warm_vm_names(base: &str) -> [String; 2] {
    let first = match base.strip_prefix("cua-base-") {
        Some(id) => format!("cua-warm-{id}"),
        None => format!("{base}-warm"),
    };
    let second = format!("{first}-2");
    [first, second]
}

/// Sets the macOS guest `vm`'s host names (computer, local and DNS) to
/// `name` ([`macos_hostname`]) in the background, through `lume ssh` and
/// the image's `lume` account: a VM resumed from a state other VMs share
/// (a warm state, a checkpoint) or a claimed pool member would otherwise
/// keep the name it was saved with. Bounded retries while SSH comes up;
/// failures are logged, never fatal.
fn set_macos_hostname_later(lume: Option<LumeCmd>, vm: &str, name: &str) {
    let Some(lume) = lume else { return };
    let host = macos_hostname(name);
    let exec = lume_ssh_prefix(&lume, vm);
    let vm = vm.to_string();
    crate::cleanup::spawn(async move {
        let script = format!(
            "for k in ComputerName LocalHostName HostName; do \
             printf '%s\\n' lume | sudo -S -p '' scutil --set $k {host}; done"
        );
        // A restore can take tens of seconds before SSH answers.
        for attempt in 0..20 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            let req = crate::exec::ExecRequest::sh(script.clone()).timeout(Duration::from_secs(30));
            match exec.exec(req).await {
                Ok(out) if out.exit_code == 0 => {
                    tracing::info!(%vm, %host, "set the guest's hostname");
                    return;
                }
                Ok(out) => tracing::debug!(%vm, code = out.exit_code, "hostname not set yet"),
                Err(e) => tracing::debug!(%vm, error = %e, "hostname not set yet"),
            }
        }
        tracing::warn!(%vm, %host, "could not set the guest's hostname");
    });
}

/// `name` as a macOS host name: letters, digits and `-` (what
/// `LocalHostName` allows), at most 63 characters, never empty.
pub fn macos_hostname(name: &str) -> String {
    let mut h: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(63)
        .collect();
    h = h.trim_matches('-').to_string();
    if h.is_empty() { "space".into() } else { h }
}

/// A guest command prefix: `lume ssh <vm>`, run with `lume`'s environment.
fn lume_ssh_prefix(lume: &LumeCmd, vm: &str) -> PrefixExec {
    let (program, mut prefix) = lume.wrapped();
    prefix.extend(["ssh".to_string(), vm.to_string()]);
    PrefixExec { program, prefix }
}

/// `lume --version`'s output, run as `lume` runs (its channel's
/// environment), for a report; `None` when it fails or takes over 5 s.
pub async fn version_text(lume: &LumeCmd) -> Option<String> {
    let run = lume
        .tokio_command()
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .ok()?
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// [`lume_cmd_for`] this process's channel.
pub fn lume_cmd() -> Result<Option<LumeCmd>> {
    lume_cmd_for(CuaChannel::current())
}

/// The Lume for `channel` to run a process from, with the environment that
/// keeps it in the channel's VM directory ([`LumeCmd`]); `None` when there
/// is no Lume yet. For a caller that hands a command to another process
/// (`lume attach`: [`LumeCmd::argv`]).
pub fn lume_cmd_for(channel: CuaChannel) -> Result<Option<LumeCmd>> {
    match lume_bin_for(channel) {
        Some(bin) => Ok(Some(LumeCmd::new(channel, &default_root(), bin)?)),
        None => Ok(None),
    }
}

/// State for Linux disk conversions and seeds, and the private config and
/// temp directories of Local's Lume: `<cua home>/vmm/lume`.
pub(crate) fn default_root() -> PathBuf {
    root_in(&host::cua_home())
}

/// [`default_root`] under the cua home `home`.
pub(crate) fn root_in(home: &std::path::Path) -> PathBuf {
    home.join("vmm").join("lume")
}

/// Locate the `lume` binary for the `runtime.lume` setting: this Mac's
/// own, or the built-in one ([`builtin`]) when installed. For this
/// process's channel ([`lume_bin_for`]).
pub fn lume_bin() -> Option<PathBuf> {
    lume_bin_for(CuaChannel::current())
}

/// [`lume_bin`] for `channel`: Stable's own Lume is `lume`, Local's
/// `lume-local` (Local never uses Stable's).
pub fn lume_bin_for(channel: CuaChannel) -> Option<PathBuf> {
    builtin::resolve_for(channel, builtin::LumeSource::current())
}

/// Deletes a clone whose create was cut off (dropped) before its first
/// boot was requested: Lume finishes a clone whose request was dropped, so
/// the delete waits (bounded) for the VM to appear.
struct CloneGuard {
    client: LumeClient,
    name: String,
    root: PathBuf,
    finished: bool,
}

impl CloneGuard {
    fn new(client: LumeClient, name: &str, root: &std::path::Path) -> Self {
        Self {
            client,
            name: name.into(),
            root: root.to_path_buf(),
            finished: false,
        }
    }

    fn done(&mut self) {
        self.finished = true;
    }
}

impl Drop for CloneGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let (client, name, dir) = (
            self.client.clone(),
            self.name.clone(),
            self.root.join(&self.name),
        );
        crate::cleanup::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                match client.get(&name).await {
                    Ok(Some(vm)) if vm.status != "provisioning" => {
                        if vm.status == "running" {
                            let _ = client.stop_for_delete(&name).await;
                        }
                        match client.delete(&name).await {
                            Ok(()) => tracing::info!(vm = %name, "deleted a cancelled clone"),
                            Err(e) => {
                                tracing::warn!(vm = %name, error = %e, "could not delete a cancelled clone")
                            }
                        }
                        break;
                    }
                    _ if tokio::time::Instant::now() >= deadline => break,
                    _ => tokio::time::sleep(Duration::from_millis(250)).await,
                }
            }
            OwnedVms::default().forget(&name);
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

impl LumeRuntime {
    pub fn new(cfg: LumeConfig) -> Self {
        let client = LumeClient::new(serving_url(&cfg.url));
        Self {
            cfg,
            client,
            gpu_pref: Arc::new(gpu::Defaults),
            accepted: Default::default(),
            old_lume: Default::default(),
            mending: Default::default(),
            mended: Default::default(),
        }
    }

    /// Uses `pref` for the GPU preference instead of the host's `defaults`
    /// (tests).
    pub fn with_gpu_preference(mut self, pref: Arc<dyn gpu::GpuPreference>) -> Self {
        self.gpu_pref = pref;
        self
    }

    fn gpu(&self) -> gpu::Gpu<'_> {
        gpu::Gpu::new(&self.cfg.root, self.gpu_pref.as_ref())
    }

    /// GPU acceleration on this host ([`crate::gpu::lume_support`]).
    pub fn gpu_support(&self) -> crate::gpu::GpuSupport {
        crate::gpu::lume_support(self.bin().is_some())
    }

    /// After `name` was deleted: GPU acceleration's host preference goes
    /// when cua set it and no VM needs it any more.
    fn forget_gpu(&self, name: &str) {
        if let Err(e) = self.gpu().after_delete(name) {
            tracing::warn!(vm = name, error = %e, "could not update the GPU preference");
        }
    }

    /// Before a macOS VM boots: GPU acceleration's host preference, when
    /// the VM has it.
    async fn before_macos_boot(&self, name: &str) -> Result<()> {
        self.ensure_macos_slot(name).await?;
        self.gpu()
            .before_start(name)
            .map_err(|e| VmmError::other(format!("GPU acceleration for {name}: {e}")))
    }

    /// Whether VM `name` (as Lume reports it) runs behind a network gateway
    /// that has ended: a protected macOS VM, running, whose gateway process
    /// is gone. Its network cable is cut for good (a gateway cannot be
    /// attached to a VM that already runs), so nothing reaches its agent.
    fn gateway_lost(&self, name: &str, vm: &VmDetails) -> bool {
        vm.os.eq_ignore_ascii_case("macos")
            && map_status(&vm.status) == Status::Running
            && self.kept_network(name).is_some()
            && self.load_gateway(name).is_some()
            && !crate::netgw::gateway_running(&self.cfg.root.join(name))
    }

    /// Makes a running protected macOS VM whose gateway ended reachable
    /// again by restarting it behind a new gateway (the gateway dies with
    /// a signal that reaches it, as when an app update stops everything of
    /// the old bundle; the VM, run by `lume serve`, does not). Unsaved work
    /// in the guest is lost, as in any restart; left as it is the VM would
    /// run without a network and without its agent.
    async fn mend_gateway(&self, name: &str, vm: VmDetails) -> Result<VmDetails> {
        if !self.gateway_lost(name, &vm) {
            return Ok(vm);
        }
        let _one_at_a_time = self.mending.lock().await;
        // Another caller may have mended it while this one waited.
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        if !self.gateway_lost(name, &vm) {
            return Ok(vm);
        }
        tracing::warn!(
            vm = name,
            "the VM's network gateway ended while it kept running, so it has no network and \
             no agent; restarting it behind a new gateway"
        );
        self.stop_vm(name).await?;
        self.resume(name).await?;
        self.mended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string());
        self.client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))
    }

    /// The macOS VMs Lume runs on this Mac other than `name` (Spaces or
    /// not, any owner of this Lume); `None` when Lume cannot list them.
    async fn macos_vms_running_besides(&self, name: &str) -> Option<Vec<String>> {
        let vms = self.client.list().await.ok()?;
        Some(
            vms.into_iter()
                .filter(|v| v.name != name && v.os.eq_ignore_ascii_case("macos"))
                .filter(|v| matches!(v.status.as_str(), "running" | "starting" | "provisioning"))
                .map(|v| v.name)
                .collect(),
        )
    }

    /// Refuses to boot a third macOS VM up front with a capacity error
    /// that says why, instead of letting Apple Virtualization stop it while
    /// it boots ("stopped while booting").
    async fn ensure_macos_slot(&self, name: &str) -> Result<()> {
        match self.macos_vms_running_besides(name).await {
            Some(others) if others.len() as u32 >= MACOS_VMS_PER_MAC => {
                Err(VmmError::HostCapacity(macos_limit_message(&others)))
            }
            _ => Ok(()),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(LumeConfig::default())
    }

    pub fn client(&self) -> &LumeClient {
        &self.client
    }

    /// The Lume this runtime runs: this Mac's own or the built-in one, for
    /// its channel ([`lume_bin_for`]).
    fn bin(&self) -> Option<PathBuf> {
        lume_bin_for(self.cfg.channel)
    }

    /// The Lume this runtime runs, with the environment its channel needs
    /// ([`LumeCmd`]). Every Lume process this runtime starts, `serve`
    /// included, is made from this, so a Local Lume never runs without its
    /// own VM directory.
    fn lume(&self) -> Result<Option<LumeCmd>> {
        match self.bin() {
            Some(bin) => Ok(Some(LumeCmd::new(self.cfg.channel, &self.cfg.root, bin)?)),
            None => Ok(None),
        }
    }

    /// [`LumeRuntime::lume`] for work that is best effort: a failure is
    /// logged and the work skipped.
    fn lume_or_log(&self) -> Option<LumeCmd> {
        self.lume().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not prepare the Lume environment");
            None
        })
    }

    /// Where this runtime's Lume keeps VMs by default (`~/.lume`, or
    /// Local's `~/.lume-local` / `$LUME_HOME`).
    fn vm_dir(&self) -> PathBuf {
        crate::storage::default_lume_dir(
            self.cfg.channel,
            &host::home_dir(),
            crate::storage::lume_home_env().as_deref(),
        )
    }

    /// The port of the `lume serve` in use (moved off the configured one
    /// when an old Lume holds it).
    fn port(&self) -> u16 {
        url_port(&self.client.base()).unwrap_or_else(|| self.cfg.channel.lume_port())
    }

    /// Make sure a current `lume serve` answers, starting (and, if allowed,
    /// installing) it as needed. Never restarts or stops a server that is
    /// already running: when the one answering is older than
    /// [`builtin::MIN_VERSION`] (say a user's own LaunchAgent), `auto` and
    /// `builtin` run a current Lume on the next free port instead and use
    /// that from then on; `system` uses it and names it in a failed start.
    pub async fn ensure_serving(&self) -> Result<()> {
        if self.client.reachable().await {
            let url = self.client.base();
            if self
                .accepted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_deref()
                == Some(url.as_str())
            {
                return Ok(());
            }
            let source = builtin::LumeSource::current();
            let version = server_version(self.cfg.channel, &self.cfg.root, &self.client).await;
            match builtin::judge_server(source, version.as_deref()) {
                builtin::ServerVerdict::Use => {}
                builtin::ServerVerdict::UseOld(v) => {
                    tracing::warn!(
                        url = %url,
                        version = %v,
                        minimum = builtin::MIN_VERSION,
                        "lume serve is older than cua needs; using it because runtime.lume is system"
                    );
                    *self.old_lume.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
                }
                builtin::ServerVerdict::Avoid(v) => return self.serve_elsewhere(&url, &v).await,
            }
            *self.accepted.lock().unwrap_or_else(|e| e.into_inner()) = Some(url);
            return Ok(());
        }
        self.spawn_serve().await
    }

    /// The `lume serve` at `old_url` is Lume `old` (too old): use a current
    /// one on the first of [`builtin::fallback_ports_for`] that has one or is
    /// free, starting it there. The old server keeps running untouched.
    async fn serve_elsewhere(&self, old_url: &str, old: &str) -> Result<()> {
        let too_old = format!(
            "lume serve at {old_url} is Lume {old}, and macOS Spaces need Lume {} or newer",
            builtin::MIN_VERSION
        );
        if !self.cfg.spawn_serve {
            return Err(VmmError::missing(
                too_old,
                "update it (or stop it and let cua run its built-in Lume)",
            ));
        }
        let port = url_port(old_url).unwrap_or_else(|| self.cfg.channel.lume_port());
        let ports: Vec<u16> = builtin::fallback_ports_for(self.cfg.channel, port).collect();
        for &p in &ports {
            let url = builtin::with_port(old_url, p);
            let candidate = LumeClient::new(url.clone());
            if candidate.reachable().await {
                // Only a Lume known to be current (cua's own from an earlier
                // run); anything else on the port is left alone.
                if server_version(self.cfg.channel, &self.cfg.root, &candidate)
                    .await
                    .is_some_and(|v| builtin::new_enough(&v))
                {
                    tracing::info!(%url, "using the current lume serve next to an old one");
                    self.client.set_base(&url);
                    remember_move(&self.cfg.url, &url);
                    *self.accepted.lock().unwrap_or_else(|e| e.into_inner()) = Some(url);
                    return Ok(());
                }
                continue;
            }
            if std::net::TcpListener::bind(("127.0.0.1", p)).is_err() {
                continue; // something else holds it
            }
            tracing::warn!(
                old = %old_url,
                version = %old,
                %url,
                "lume serve is older than cua needs; leaving it running and starting a current Lume on another port"
            );
            self.client.set_base(&url);
            remember_move(&self.cfg.url, &url);
            return self.spawn_serve().await;
        }
        Err(VmmError::missing(
            too_old,
            format!(
                "ports {}-{} are taken too; update or stop that Lume",
                ports.first().copied().unwrap_or(port),
                ports.last().copied().unwrap_or(port)
            ),
        ))
    }

    /// Starts `lume serve` on [`Self::port`] (installing Lume as allowed)
    /// and waits for it to answer.
    async fn spawn_serve(&self) -> Result<()> {
        if !cfg!(target_os = "macos") {
            return Err(VmmError::missing(
                "Lume",
                "Lume runs on macOS (Apple Silicon) hosts only",
            ));
        }
        if !self.cfg.spawn_serve {
            return Err(VmmError::missing(
                format!("lume serve at {}", self.client.base()),
                "start it with `lume serve`",
            ));
        }
        let source = builtin::LumeSource::current();
        let bin = match self.bin() {
            Some(b) => b,
            // Nothing to ask: the pinned, signed Lume, into cua's own dir.
            None if source.allows_builtin() => builtin::ensure().await?,
            None if source == builtin::LumeSource::System && !self.cfg.allow_install => {
                return Err(VmmError::missing(
                    "Lume",
                    "macOS Spaces are set to use this Mac's own Lume, and it is not installed. \
                     Choose Built-in for macOS VMs in Settings (or `cua config set runtime.lume builtin`)",
                ));
            }
            // The official installer is Stable's (`lume`, its LaunchAgent and
            // port): Local never runs it over a Stable install.
            None if self.cfg.allow_install && !self.cfg.channel.is_local() => {
                install_lume().await?;
                if self.client.reachable().await {
                    return Ok(()); // installer's LaunchAgent started it
                }
                self.bin().ok_or_else(|| {
                    VmmError::missing(
                        self.cfg.channel.lume_bin_name(),
                        format!(
                            "installer finished but `{}` is not on PATH",
                            self.cfg.channel.lume_bin_name()
                        ),
                    )
                })?
            }
            None if self.cfg.channel.is_local() => {
                return Err(VmmError::missing(
                    self.cfg.channel.lume_bin_name(),
                    format!(
                        "install it with Lume's install-local.sh, or let cua use its built-in Lume (`cua config set runtime.lume builtin`); `{}` is not on PATH",
                        self.cfg.channel.lume_bin_name()
                    ),
                ));
            }
            None => {
                return Err(VmmError::missing(
                    "lume",
                    format!(
                        "install it with `/bin/bash -c \"$(curl -fsSL {INSTALL_URL})\"` or enable allow_install to let cua do it"
                    ),
                ));
            }
        };
        std::fs::create_dir_all(&self.cfg.root)?;
        let lume = LumeCmd::new(self.cfg.channel, &self.cfg.root, &bin)?;
        if source == builtin::LumeSource::System
            && let Some(v) = builtin::binary_version(&lume).filter(|v| !builtin::new_enough(v))
        {
            tracing::warn!(
                bin = %bin.display(),
                version = %v,
                minimum = builtin::MIN_VERSION,
                "this Mac's Lume is older than cua needs; using it because runtime.lume is system"
            );
            *self.old_lume.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        }
        // No `lume serve` answers, so none of its pulls runs: a pull a
        // killed serve left half-made in its temp dir is only taking space.
        sweep_pull_dirs_for(&lume, &lume_temp_dir(), STALE_PULL_DIR_IDLE);
        let log = std::fs::File::create(self.cfg.root.join("serve.log"))?;
        let mut cmd = lume.command();
        cmd.args(["serve", "--port", &self.port().to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0); // outlive this process; not tied to our terminal
        }
        let child = cmd.spawn()?;
        tracing::info!(pid = child.id(), "started lume serve");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while tokio::time::Instant::now() < deadline {
            if self.client.reachable().await {
                *self.accepted.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.client.base());
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(VmmError::other(format!(
            "started `lume serve` but {} did not answer within 30s (see {})",
            self.client.base(),
            self.cfg.root.join("serve.log").display()
        )))
    }

    /// Wait until the guest has an IP. Each poll asks `lume serve` and, when
    /// it has no address yet, looks the VM's MAC up in the host's vmnet DHCP
    /// leases and ARP table ([`lease`]); `lume serve` can lag the lease by
    /// many seconds or miss it. Polls back off from 0.5 s to 3 s. A VM that
    /// never reaches `running`, or stops after it did, fails fast with the
    /// tail of its run log instead of waiting out the timeout; one Lume no
    /// longer knows (deleted outside the SDK) fails within a few seconds.
    async fn wait_for_ip(&self, name: &str) -> Result<VmDetails> {
        let start = tokio::time::Instant::now();
        let deadline = start + self.cfg.ip_timeout;
        let never_ran_after = Duration::from_secs(45);
        let mut saw_running = false;
        let mut delay = Duration::from_millis(500);
        let mut last_state = String::new();
        let leases_path = std::path::PathBuf::from(&self.cfg.leases_path);
        let before = lease::read_leases(&leases_path).await;
        // The boot began just now (the run request went out a moment ago):
        // a refusal `lume serve` logs from a few seconds before counts.
        let since = utc_stamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .saturating_sub(5),
        );
        let mut missing_since: Option<tokio::time::Instant> = None;
        // Bounded by the deadline; at most ip_timeout / 0.5 s iterations.
        while tokio::time::Instant::now() < deadline {
            let vm = self.client.get(name).await?;
            if vm.is_none() {
                let since = *missing_since.get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() >= VM_GONE_GRACE {
                    return Err(VmmError::other(format!(
                        "lume VM '{name}' disappeared while booting (it was deleted outside Cua)"
                    )));
                }
            } else {
                missing_since = None;
            }
            if let Some(mut vm) = vm {
                let state = format!(
                    "status={} api_ip={:?} mac={:?}",
                    vm.status, vm.ip_address, vm.mac_address
                );
                if state != last_state {
                    tracing::debug!(vm = name, elapsed = ?start.elapsed(), %state, "lume VM state");
                    last_state = state;
                }
                match vm.status.as_str() {
                    "running" => saw_running = true,
                    "stopped" if saw_running => {
                        // Apple Virtualization stops a macOS VM past its
                        // limit while it boots: say so when that is why.
                        if let Some(others) = self.macos_vms_running_besides(name).await
                            && others.len() as u32 >= MACOS_VMS_PER_MAC
                        {
                            return Err(VmmError::HostCapacity(macos_limit_message(&others)));
                        }
                        // The VMs holding the limit may be another Lume's,
                        // which this one does not list: its own log says so.
                        if self.serve_log_says_vm_limit_soon(name, &since).await {
                            return Err(VmmError::HostCapacity(macos_limit_unlisted_message()));
                        }
                        return Err(VmmError::other(format!(
                            "lume VM '{name}' stopped while booting{}",
                            self.run_log_tail(name)
                        )));
                    }
                    "stopped" if start.elapsed() > never_ran_after => {
                        return Err(VmmError::other(format!(
                            "lume VM '{name}' never started ({}s after `lume run`){}",
                            never_ran_after.as_secs(),
                            self.run_log_tail(name)
                        )));
                    }
                    _ => {}
                }
                if vm.ip().is_some() {
                    tracing::debug!(vm = name, elapsed = ?start.elapsed(), ip = ?vm.ip_address, "IP from lume serve");
                    return Ok(vm);
                }
                let mac = vm
                    .mac_address
                    .clone()
                    .or_else(|| lease::config_mac(&host::home_dir(), name));
                if saw_running
                    && let Some((ip, source)) =
                        lease::lookup(mac.as_deref(), name, &before, &leases_path).await
                {
                    tracing::info!(vm = name, elapsed = ?start.elapsed(), %ip, source, "guest IP found on the host before lume serve reported it");
                    vm.ip_address = Some(ip);
                    return Ok(vm);
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 3 / 2).min(Duration::from_secs(3));
        }
        Err(VmmError::Timeout {
            name: name.into(),
            secs: self.cfg.ip_timeout.as_secs(),
            detail: format!(
                "VM never reported an IP address (lume serve, {} and ARP checked; last: {last_state}){}",
                self.cfg.leases_path,
                self.run_log_tail(name)
            ),
        })
    }

    /// Whether `lume serve`'s log says VM `name` was refused for the limit
    /// on active virtual machines since `since` (a [`utc_stamp`]).
    fn serve_log_says_vm_limit(&self, name: &str, since: &str) -> bool {
        use std::io::{Read as _, Seek as _, SeekFrom};
        // The log is large; its recent part is enough.
        const TAIL: u64 = 96 * 1024 * 1024;
        let Ok(mut f) = std::fs::File::open(self.cfg.root.join("serve.log")) else {
            return false;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let mut bytes = Vec::new();
        if f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).is_err()
            || f.read_to_end(&mut bytes).is_err()
        {
            return false;
        }
        log_says_vm_limit(&bytes, name, since)
    }

    /// [`Self::serve_log_says_vm_limit`], looked at again for up to 10
    /// seconds: Lume writes the refusal to its log a moment after the VM
    /// has stopped, and cua notices the stop first.
    async fn serve_log_says_vm_limit_soon(&self, name: &str, since: &str) -> bool {
        for _ in 0..20 {
            if self.serve_log_says_vm_limit(name, since) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        false
    }

    fn run_log_tail(&self, name: &str) -> String {
        let log = self.cfg.root.join(name).join("run.log");
        match std::fs::read_to_string(&log) {
            Ok(s) if !s.trim().is_empty() => {
                format!("; {}: {}", log.display(), crate::exec::tail(&s, 800))
            }
            _ => String::new(),
        }
    }

    fn endpoints_from(vm: &VmDetails, ssh: Option<&SshAccess>, ports: &[u16]) -> Endpoints {
        let ip = vm.ip().unwrap_or("").to_string();
        let vnc =
            vm.vnc_url
                .as_deref()
                .and_then(client::parse_vnc_url)
                .map(|(host, port, password)| VncEndpoint {
                    host,
                    port,
                    password,
                });
        let ssh = match ssh {
            Some(a) => Some(SshEndpoint {
                host: ip.clone(),
                port: 22,
                user: a.user.clone(),
                private_key: Some(a.private_key.clone()),
                password: a.password.clone(),
            }),
            None if vm.os == "macOS" || vm.os == "macos" => Some(SshEndpoint {
                host: ip.clone(),
                port: 22,
                user: "lume".into(),
                private_key: None,
                password: Some("lume".into()),
            }),
            None => None,
        };
        // A macOS guest whose vsock agent answers: its published ports are
        // host loopback ports Lume carries over vsock, which neither macOS
        // Local Network privacy nor a VPN's routes touch.
        if let Some(forwarded) = vm.vsock_ports(ports) {
            return Endpoints {
                host: VSOCK_HOST.into(),
                ports: forwarded,
                vnc,
                ssh,
                ..Default::default()
            };
        }
        Endpoints {
            host: ip,
            // Lume VMs sit on a host-reachable NAT network: guest ports are
            // reachable on the VM IP directly, so the mapping is the identity.
            ports: ports
                .iter()
                .map(|p| (*p, *p))
                .chain(std::iter::once((22, 22)))
                .collect(),
            vnc,
            ssh,
            ..Default::default()
        }
    }

    /// `<root>/<name>/ports.json`: the guest ports `start` published.
    fn ports_file(&self, name: &str) -> PathBuf {
        self.cfg.root.join(name).join("ports.json")
    }

    fn save_ports(&self, name: &str, ports: &[u16]) {
        let path = self.ports_file(name);
        let saved = path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&path, serde_json::to_vec(ports).unwrap_or_default()));
        if let Err(e) = saved {
            tracing::warn!(name, error = %e, "could not record the published ports");
        }
    }

    fn load_ports(&self, name: &str) -> Vec<u16> {
        std::fs::read(self.ports_file(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// [`Self::instance`], reached through the VM's network gateway when it
    /// has one.
    fn instance_gw(&self, vm: &VmDetails, ssh: Option<&SshAccess>, ports: &[u16]) -> Instance {
        let mut i = Self::instance(vm, ssh, ports);
        i.endpoints = self.with_gateway(&vm.name, i.endpoints);
        i
    }

    /// `<root>/<name>/network.json`: the settings a protected VM's gateway
    /// enforces (written at start, read at every later run).
    fn network_file(&self, name: &str) -> PathBuf {
        self.cfg.root.join(name).join("network.json")
    }

    /// `<root>/<name>/gateway.json`: the current run's gateway.
    fn gateway_file(&self, name: &str) -> PathBuf {
        self.cfg.root.join(name).join(GATEWAY_FILE)
    }

    fn keep_network(&self, name: &str, settings: &crate::network::NetworkSettings) -> Result<()> {
        let path = self.network_file(name);
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&path, serde_json::to_vec_pretty(settings)?)?;
        Ok(())
    }

    fn forget_network(&self, name: &str) {
        let _ = std::fs::remove_file(self.network_file(name));
        let _ = std::fs::remove_file(self.gateway_file(name));
    }

    fn kept_network(&self, name: &str) -> Option<crate::network::NetworkSettings> {
        std::fs::read(self.network_file(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn load_gateway(&self, name: &str) -> Option<LumeGateway> {
        std::fs::read(self.gateway_file(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    }

    /// `ep` as the host reaches a VM behind its gateway: on loopback,
    /// through the gateway's forwards. Unchanged for a VM on Apple's NAT.
    fn with_gateway(&self, name: &str, mut ep: Endpoints) -> Endpoints {
        let Some(gw) = self.load_gateway(name) else {
            return ep;
        };
        ep.host = "127.0.0.1".into();
        ep.ports = ep
            .ports
            .keys()
            .filter_map(|g| gw.ports.get(g).map(|h| (*g, *h)))
            .collect();
        if let Some(ssh) = ep.ssh.as_mut() {
            ssh.host = "127.0.0.1".into();
            if let Some(h) = gw.ports.get(&22) {
                ssh.port = *h;
            }
        }
        ep
    }

    /// [`Self::macos_run`], behind the VM's gateway when its network is
    /// protected ([`Self::gateway_network`]).
    async fn protected_run(&self, name: &str, spec: Option<&StartSpec>) -> Result<RunVm> {
        let mut run = self.macos_run(name, spec)?;
        run.network = self.gateway_network(name).await?;
        Ok(run)
    }

    /// The guest ports a VM's gateway forwards: the ports its start
    /// published ([`Self::save_ports`], recorded before the gateway starts)
    /// and SSH.
    fn gateway_guest_ports(&self, name: &str) -> Vec<u16> {
        let mut guest = self.load_ports(name);
        guest.push(22);
        guest.sort_unstable();
        guest.dedup();
        guest
    }

    /// Starts the gateway of VM `name` for one run and returns its
    /// `gateway:<socket>` network, when the VM's network is protected
    /// ([`Self::keep_network`]); `None` for a VM on Apple's NAT. The gateway
    /// enforces the VM's compiled settings, hands the guest a fixed address
    /// by DHCP, and forwards the VM's published ports (and SSH) from the
    /// host's loopback; it ends when the VM disconnects.
    async fn gateway_network(&self, name: &str) -> Result<Option<String>> {
        let Some(settings) = self.kept_network(name) else {
            let _ = std::fs::remove_file(self.gateway_file(name));
            return Ok(None);
        };
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        let mac = vm
            .mac_address
            .as_deref()
            .and_then(lease::normalize_mac)
            .or_else(|| lease::config_mac(&host::home_dir(), name))
            .ok_or_else(|| {
                VmmError::other(format!(
                    "{name}: Lume did not report the VM's MAC address, which its network \
                     gateway needs (Lume 0.6 or later)"
                ))
            })?;
        let bin = crate::netgw::bin().ok_or_else(crate::netgw::missing)?;
        let dir = self.cfg.root.join(name);
        std::fs::create_dir_all(&dir)?;
        let (rules, domains) = crate::network::compile_named(
            &settings,
            &gateway_context(),
            crate::network::Mode::from_env(),
        )
        .map_err(VmmError::from)?;
        crate::netgw::write_rules_named(&dir, &rules, &domains)?;
        let run = gateway_run_dir(&dir);
        let _ = std::fs::remove_dir_all(&run);
        std::fs::create_dir_all(&run)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700))?;
        }
        // Lume finds the control socket beside the network socket.
        let sock = run.join("net.sock");
        let control = run.join("control.sock");
        let guest = self.gateway_guest_ports(name);
        let hosts = host::free_ports(guest.len())?;
        let ports: std::collections::BTreeMap<u16, u16> =
            guest.iter().copied().zip(hosts).collect();
        let net = crate::netgw::Network {
            subnet: GATEWAY_SUBNET,
            gateway_ip: GATEWAY_IP,
            gateway_mac: GATEWAY_MAC,
            host_ip: Some(GATEWAY_HOST_IP),
            dhcp: true,
            static_leases: [(GUEST_IP.to_string(), mac)].into(),
            vfkit: Some(format!("unixgram://{}", sock.display())),
            qemu: None,
            // The gateway answers DNS on its own address (the DHCP lease names it);
            // `dns_ips` is for more addresses, and refuses the gateway's own.
            dns_ips: vec![],
            forwards: ports
                .iter()
                .map(|(g, h)| (format!("127.0.0.1:{h}"), format!("{GUEST_IP}:{g}")))
                .collect(),
            control: Some(format!("unix://{}", control.display())),
        };
        // Its tree: the gateway carries connections to the tree's running
        // members and takes theirs ([`crate::tree`]).
        let joined = crate::tree::join(
            &tree_key(name),
            None,
            &dir,
            crate::tree::ingress_for(Some(&settings), false),
        )?;
        let mut config = crate::netgw::config(&net);
        config["tree"] = crate::tree::GatewayTree {
            sock: joined.sock,
            guest_ip: GUEST_IP.to_string(),
        }
        .config();
        std::fs::write(
            dir.join(crate::netgw::CONFIG_FILE),
            serde_json::to_vec_pretty(&config)?,
        )?;
        crate::netgw::start(&bin, &dir, Duration::from_secs(15)).await?;
        let gw = LumeGateway {
            network: format!("gateway:{}", sock.display()),
            ports,
        };
        std::fs::write(self.gateway_file(name), serde_json::to_vec_pretty(&gw)?)?;
        Ok(Some(gw.network))
    }

    fn instance(vm: &VmDetails, ssh: Option<&SshAccess>, ports: &[u16]) -> Instance {
        Instance {
            name: vm.name.clone(),
            backend: BackendKind::Lume,
            status: map_status(&vm.status),
            endpoints: Self::endpoints_from(vm, ssh, ports),
            isolation: Isolation::Vm { accel: "vz".into() },
            arch: Some(Arch::Aarch64),
        }
    }

    /// Prepare the raw disk + seed for a Linux disk-image VM.
    async fn prepare_linux(
        &self,
        spec: &StartSpec,
        src: &std::path::Path,
    ) -> Result<(PathBuf, Option<PathBuf>)> {
        let dir = self.cfg.root.join(&spec.name);
        std::fs::create_dir_all(&dir)?;
        let raw = dir.join("disk.img");
        if !raw.exists() {
            // The raw copy is sparse; the guest's first writes land on top.
            crate::disk::ensure_space(
                &dir,
                std::fs::metadata(src)
                    .map(|m| m.len())
                    .unwrap_or(0)
                    .saturating_add(crate::disk::VM_CREATE_ESTIMATE),
                &format!("create VM {}", spec.name),
            )?;
            let qemu_img = host::which("qemu-img").ok_or_else(|| {
                VmmError::missing(
                    "qemu-img",
                    "needed to convert Linux disks for Lume (`brew install qemu`)",
                )
            })?;
            let (s, d) = (src.display().to_string(), raw.display().to_string());
            host::run(&qemu_img, &["convert", "-q", "-O", "raw", &s, &d]).await?;
            if let Some(gb) = spec.disk_size_gb {
                host::run(
                    &qemu_img,
                    &["resize", "-q", "-f", "raw", &d, &format!("{gb}G")],
                )
                .await?;
            }
        }
        // SSH key / raw user-data plus the guest env (the spacesd token).
        let seed = crate::cloudinit::seed_for_spec(&spec.name, spec)?;
        let seed_path = match seed {
            Some(s) => {
                let p = dir.join("seed.iso");
                s.write_iso(&p)?;
                Some(p)
            }
            None => None,
        };
        Ok((raw, seed_path))
    }

    async fn run_linux_detached(
        &self,
        name: &str,
        disk: &std::path::Path,
        seed: Option<&std::path::Path>,
    ) -> Result<()> {
        let lume = self
            .lume()?
            .ok_or_else(|| VmmError::missing("lume", "binary needed for Linux disk VMs"))?;
        let log = self.cfg.root.join(name).join("run.log");
        let mut args = vec![
            "run".to_string(),
            name.to_string(),
            "--detach".into(),
            "--display".into(),
            "none".into(),
            "--disk-path".into(),
            disk.display().to_string(),
            "--log-file".into(),
            log.display().to_string(),
        ];
        if let Some(s) = seed {
            args.push("--mount".into());
            args.push(s.display().to_string());
        }
        if let Some(network) = self.gateway_network(name).await? {
            args.push("--network".into());
            args.push(network);
        }
        cli_run(&lume, &args).await
    }

    /// The macOS setup share: a read-only host directory mounted in the guest
    /// at `/Volumes/My Shared Files`, holding `env-token` (0600) when the start
    /// spec carries `CUA_ENV_TOKEN`. The golden's spacesd launcher copies
    /// it to `~/.cua/spacesd/token` before serving, so the driver starts
    /// with this token instead of in bootstrap mode. The directory is
    /// shared on every run, empty when there is no token: a saved machine
    /// state restores only into the same shared directories (by name), so
    /// a VM suspended without a token resumes with one and the other way
    /// round. `spec` is `None` for a restart, which keeps what is there.
    fn macos_setup_share(&self, name: &str, spec: Option<&StartSpec>) -> Result<Vec<SharedDir>> {
        let dir = self.cfg.root.join(name).join("setup");
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        if let Some(spec) = spec {
            match spec
                .env
                .get(crate::cloudinit::ENV_TOKEN_VAR)
                .filter(|t| !t.is_empty())
            {
                // Owner-only from creation (never briefly world-readable).
                Some(token) => {
                    crate::host::write_private(&dir.join(MACOS_SETUP_TOKEN_FILE), token.as_bytes())?
                }
                // No token in this spec: never share a stale one.
                None => {
                    let _ = std::fs::remove_file(dir.join(MACOS_SETUP_TOKEN_FILE));
                }
            }
        }
        Ok(vec![SharedDir {
            host_path: dir.display().to_string(),
            read_only: true,
        }])
    }

    fn macos_run(&self, name: &str, spec: Option<&StartSpec>) -> Result<RunVm> {
        if spec.is_some_and(|s| s.command.as_ref().is_some_and(|c| !c.is_empty())) {
            return Err(VmmError::Unsupported {
                backend: "lume",
                op: "a sandbox command on a macOS guest (bake it into the image)",
            });
        }
        let mut shared_directories = self.macos_setup_share(name, spec)?;
        if let Some(spec) = spec {
            shared_directories.extend(macos_mount_shares(&spec.mounts)?);
        }
        Ok(RunVm {
            no_display: true,
            shared_directories,
            vnc: (!builtin::vnc_enabled()).then(|| "disabled".to_string()),
            // The published ports also over vsock (the IP stays the
            // fallback for a guest without the agent). A restart publishes
            // what the start did, so a state saved with the socket device
            // restores into one.
            forwards: match spec {
                Some(s) => s.ports.clone(),
                None => self.load_ports(name),
            },
            ..Default::default()
        })
    }

    /// [`Runtime::ensure_base`] with registry credentials for a private
    /// image (used when they apply to the image's registry).
    pub async fn ensure_base_with(
        &self,
        image: &ImageSource,
        base_name: &str,
        creds: Option<&RegistryCredentials>,
    ) -> Result<CheckpointInfo> {
        self.ensure_base_sized(image, base_name, creds, None).await
    }

    /// [`Self::ensure_base_with`], checking the disk for `pull_bytes` (what
    /// the pull writes, when known) before a pull instead of
    /// [`crate::disk::LUME_PULL_ESTIMATE`].
    pub async fn ensure_base_sized(
        &self,
        image: &ImageSource,
        base_name: &str,
        creds: Option<&RegistryCredentials>,
        pull_bytes: Option<u64>,
    ) -> Result<CheckpointInfo> {
        self.ensure_serving().await?;
        let ImageSource::Oci { reference } = image else {
            return Err(VmmError::invalid(
                "lume bases are pulled from OCI references",
            ));
        };
        // The macOS Space with developer tools: the published image is pulled
        // like any other; building it here is the fallback.
        if let Some(parent) = derive::dev_parent(reference) {
            return self
                .ensure_dev_base(reference, base_name, &parent, creds, pull_bytes)
                .await;
        }
        self.pull_base(image, base_name, creds, pull_bytes).await
    }

    /// Pulls `image` into the stopped base VM `base_name` (the pull half of
    /// [`Self::ensure_base_sized`]).
    async fn pull_base(
        &self,
        image: &ImageSource,
        base_name: &str,
        creds: Option<&RegistryCredentials>,
        pull_bytes: Option<u64>,
    ) -> Result<CheckpointInfo> {
        let ImageSource::Oci { reference } = image else {
            return Err(VmmError::invalid(
                "lume bases are pulled from OCI references",
            ));
        };
        let pinned = reference;
        let reference = &lume_pull_ref(reference).to_string();
        if let Some(vm) = self.client.get(base_name).await? {
            if vm.status == "running" {
                return Err(VmmError::invalid(format!(
                    "base VM '{base_name}' is running; stop it before using it as a clone source"
                )));
            }
            if vm.status != "pulling" {
                OwnedVms::default().touch(base_name);
                return Ok(CheckpointInfo::now(
                    base_name,
                    BackendKind::Lume,
                    Some(reference.clone()),
                ));
            }
        }
        crate::disk::ensure_space(
            &self.vm_dir(),
            lume_pull_need(pull_bytes),
            &format!("pull {reference}"),
        )?;
        tracing::info!(
            reference,
            base_name,
            "pulling lume base image (first time only)"
        );
        crate::progress::report(
            crate::progress::Progress::phase(crate::progress::Phase::Pulling).detail(reference),
        );
        // Recorded before the pull: a pull an older Lume finishes in the
        // background after a cancelled create is still the SDK's base (the
        // cache GC can remove it); a cancelled pull's record goes with it.
        OwnedVms::default().mark(base_name, OwnedKind::Base, Some(pinned));
        let pulled = match creds.filter(|c| c.applies_to(crate::registry_of(reference))) {
            // `lume serve`'s pull API takes no credentials: the `lume pull`
            // CLI reads them from its environment (never argv).
            Some(c) => match self.lume()? {
                Some(lume) => cli_pull(&lume, reference, base_name, c, self.cfg.pull_timeout).await,
                None => Err(VmmError::missing(
                    "lume",
                    "binary needed to pull a private image",
                )),
            },
            None => {
                self.client
                    .pull(reference, base_name, self.cfg.pull_timeout)
                    .await
            }
        };
        if let Err(e) = pulled {
            if !matches!(self.client.get(base_name).await, Ok(Some(_))) {
                OwnedVms::default().forget(base_name);
            }
            return Err(e);
        }
        OwnedVms::default().mark(base_name, OwnedKind::Base, Some(pinned));
        Ok(CheckpointInfo::now(
            base_name,
            BackendKind::Lume,
            Some(reference.clone()),
        ))
    }

    /// The base VM of the macOS Space with developer tools `reference`
    /// (`ghcr.io/trycua/macos:26-xcode`, [`derive`]). The default is to pull
    /// the published image, with the usual progress and disk check. When it
    /// is not available (not published yet, no network to it), or the user
    /// prefers the Xcode already on this Mac (`CUA_DEV_IMAGE=local`), it is
    /// built here as a clone of the full image's base plus that Xcode. Either
    /// way every Space is a clone of the one base.
    async fn ensure_dev_base(
        &self,
        reference: &str,
        base_name: &str,
        parent: &str,
        creds: Option<&RegistryCredentials>,
        pull_bytes: Option<u64>,
    ) -> Result<CheckpointInfo> {
        derive::require_apple_hardware()?;
        // One at a time in this process: two Spaces asked for together wait
        // for the first, then both clone it.
        static BUILD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _one_at_a_time = BUILD.lock().await;
        let info =
            || CheckpointInfo::now(base_name, BackendKind::Lume, Some(reference.to_string()));
        if let Some(vm) = self.client.get(base_name).await? {
            if vm.status == "running" {
                return Err(VmmError::invalid(format!(
                    "base VM '{base_name}' is running; stop it before using it as a clone source"
                )));
            }
            if vm.status != "pulling" {
                OwnedVms::default().touch(base_name);
                return Ok(info());
            }
        }
        if !derive::prefer_local() {
            let image = ImageSource::Oci {
                reference: reference.to_string(),
            };
            match self.pull_base(&image, base_name, creds, pull_bytes).await {
                Ok(done) => return Ok(done),
                // Not enough disk is the same for a build: say so.
                Err(e @ VmmError::InsufficientDisk(_)) => return Err(e),
                Err(e) => {
                    tracing::warn!(error = %e, "the published macOS Space with developer tools is not available; building it on this Mac");
                    crate::progress::report(
                        crate::progress::Progress::phase(crate::progress::Phase::Creating).detail(
                            "The published image is not available; building it on this Mac",
                        ),
                    );
                }
            }
        }
        // The full image the build starts from: a base the SDK already holds
        // for this macOS version (no second download), else the image itself.
        let version = parent
            .split('@')
            .next()
            .and_then(|t| t.rsplit_once(':'))
            .map(|(_, v)| v)
            .unwrap_or("26");
        let owned = OwnedVms::default();
        let mut held = owned
            .list()
            .into_iter()
            .filter(|o| {
                o.kind == OwnedKind::Base
                    && o.source
                        .as_deref()
                        .is_some_and(|s| derive::is_full_macos_base(s, version))
            })
            .collect::<Vec<_>>();
        held.sort_by_key(|o| std::cmp::Reverse(o.last_used));
        let mut parent_base = None;
        for o in &held {
            if matches!(self.client.get(&o.name).await?, Some(vm) if vm.status == "stopped") {
                parent_base = Some(o.name.clone());
                break;
            }
        }
        let parent_base = match parent_base {
            Some(b) => b,
            None => {
                let b = base_vm_name(parent);
                Box::pin(self.ensure_base_sized(
                    &ImageSource::Oci {
                        reference: parent.to_string(),
                    },
                    &b,
                    creds,
                    pull_bytes,
                ))
                .await?;
                b
            }
        };
        crate::disk::ensure_space(
            &self.vm_dir(),
            derive::DEV_IMAGE_ESTIMATE,
            "build the macOS Space with developer tools",
        )?;
        let lume = self.lume()?.ok_or_else(|| {
            VmmError::missing(
                "lume",
                "binary needed to build the macOS Space with developer tools",
            )
        })?;
        crate::progress::report(
            crate::progress::Progress::phase(crate::progress::Phase::Creating)
                .detail("Building the macOS Space with developer tools (first time only)"),
        );
        // Recorded before the build, as for a pull: a build cut off still
        // leaves a VM the cache GC may remove; a failed one's record goes.
        owned.mark(base_name, OwnedKind::Base, Some(reference));
        let work = host::cua_home().join("vmm").join("lume").join("derive");
        let built = derive::Derive {
            lume: &lume,
            storage: &self.vm_dir(),
            base_vm: &parent_base,
            name: base_name,
            work: &work,
        }
        .run()
        .await;
        if let Err(e) = built {
            if !matches!(self.client.get(base_name).await, Ok(Some(_))) {
                owned.forget(base_name);
            }
            return Err(e);
        }
        owned.mark(base_name, OwnedKind::Base, Some(reference));
        owned.touch(&parent_base);
        Ok(info())
    }

    /// Installs the start spec's workload environment in a booted macOS
    /// guest ([`apply_macos_env`]), or clears one an earlier start installed.
    /// Nothing happens (no SSH round trip) when neither applies. `exec` is
    /// `None` when the `lume` binary is missing.
    async fn sync_macos_env(
        &self,
        name: &str,
        env: &std::collections::BTreeMap<String, String>,
        exec: Option<&dyn GuestExec>,
    ) -> Result<()> {
        let env = crate::cloudinit::workload_env(env);
        let marker = self.cfg.root.join(name).join(MACOS_ENV_MARKER);
        if env.is_empty() && !marker.exists() {
            return Ok(());
        }
        let exec = exec.ok_or_else(|| {
            VmmError::missing(
                "lume",
                "needed to set a macOS guest's environment (`lume ssh`)",
            )
        })?;
        apply_macos_env(exec, name, &env, MACOS_ENV_BUDGET, MACOS_ENV_RETRY).await?;
        if env.is_empty() {
            let _ = std::fs::remove_file(&marker);
        } else {
            std::fs::create_dir_all(self.cfg.root.join(name))?;
            std::fs::write(&marker, b"")?;
        }
        Ok(())
    }

    /// `lume set` the stopped clone `spec.name` to the spec's CPU, memory and
    /// (grow-only) disk size; a no-op when it already matches.
    async fn apply_clone_resources(&self, spec: &StartSpec) -> Result<()> {
        let vm = self
            .client
            .get(&spec.name)
            .await?
            .ok_or_else(|| VmmError::NotFound(spec.name.clone()))?;
        let set = clone_resources(&vm, spec);
        if set.is_empty() {
            return Ok(());
        }
        tracing::info!(vm = %spec.name, ?set, "applying requested resources to the lume clone");
        self.client.set(&spec.name, &set).await
    }

    fn linux_state(&self, name: &str) -> Option<(PathBuf, Option<PathBuf>)> {
        let dir = self.cfg.root.join(name);
        let disk = dir.join("disk.img");
        disk.exists().then(|| {
            let seed = dir.join("seed.iso");
            (disk, seed.exists().then_some(seed))
        })
    }
}

/// The `lume set` a fresh clone `vm` needs to match `spec`: CPU and memory
/// when they differ, and the disk only when the spec asks for more than the
/// clone has (lume grows disks but never shrinks them). The clone is new, so
/// lume's pre-resize backup is skipped.
pub fn clone_resources(vm: &VmDetails, spec: &StartSpec) -> client::SetVm {
    let mut set = client::SetVm::default();
    if spec.cpus > 0 && spec.cpus != vm.cpu_count {
        set.cpu = Some(spec.cpus);
    }
    if spec.memory_mb > 0 && spec.memory_mb.saturating_mul(1 << 20) != vm.memory_size {
        set.memory = Some(format!("{}MB", spec.memory_mb));
    }
    if let Some(gb) = spec.disk_size_gb.filter(|g| *g > 0) {
        let want = u64::from(gb) << 30;
        match client::disk_total_bytes(&vm.disk_size) {
            Some(have) if want <= have => {
                if want < have {
                    tracing::warn!(
                        vm = %vm.name,
                        requested_gb = gb,
                        current_gb = have >> 30,
                        "lume cannot shrink a cloned disk; keeping the base image's size"
                    );
                }
            }
            _ => {
                set.disk_size = Some(format!("{gb}GB"));
                set.no_backup = Some(true);
            }
        }
    }
    set
}

/// `lume pull` arguments for `reference` into VM `name` (same split as
/// [`client::pull_body`]).
pub fn cli_pull_args(reference: &str, name: &str) -> Vec<String> {
    let b = client::pull_body(reference, name);
    let mut args = vec![
        "pull".to_string(),
        b["image"].as_str().unwrap_or(reference).to_string(),
        name.to_string(),
    ];
    for key in ["registry", "organization"] {
        if let Some(v) = b[key].as_str() {
            args.push(format!("--{key}"));
            args.push(v.to_string());
        }
    }
    args
}

/// Pulls a private image with the `lume` CLI, which reads registry
/// credentials from `GITHUB_USERNAME` / `GITHUB_TOKEN` in its environment.
/// The credentials never go on argv or into a log line or error.
pub async fn cli_pull(
    lume: &LumeCmd,
    reference: &str,
    name: &str,
    creds: &RegistryCredentials,
    timeout: Duration,
) -> Result<()> {
    let args = cli_pull_args(reference, name);
    let mut cmd = lume.tokio_command();
    cmd.args(&args)
        .env("GITHUB_USERNAME", &creds.username)
        .env("GITHUB_TOKEN", &creds.password)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    tracing::info!(
        reference,
        name,
        "pulling private lume image with credentials"
    );
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| VmmError::Timeout {
            name: name.to_string(),
            secs: timeout.as_secs(),
            detail: format!("lume pull of {reference}"),
        })??;
    if out.status.success() {
        return Ok(());
    }
    let mut stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if stderr.trim().is_empty() {
        stderr = String::from_utf8_lossy(&out.stdout).into_owned();
    }
    // Belt and braces: never echo the secret back.
    let stderr = stderr.replace(&creds.password, "***");
    Err(VmmError::Command {
        cmd: format!("lume pull {reference}"),
        code: out.status.code(),
        stderr: crate::exec::tail(&stderr, 2000),
    })
}

/// Runs the `lume` CLI to completion with `args` (the VM-directory
/// environment of its channel applies), for what has no `lume serve` call.
async fn cli_run(lume: &LumeCmd, args: &[String]) -> Result<()> {
    let (program, lead) = lume.wrapped();
    let argv: Vec<&str> = lead
        .iter()
        .map(String::as_str)
        .chain(args.iter().map(String::as_str))
        .collect();
    host::run(program, &argv).await?;
    Ok(())
}

/// Host-side marker: this VM got a workload environment from cua-vmm (so a
/// later start without one clears it).
const MACOS_ENV_MARKER: &str = "env-applied";
/// How long a pull's temp dir must have gone unwritten before
/// [`sweep_stale_pull_dirs`] removes it: a pull that still runs (a `lume
/// pull` in another process) writes to it all the time.
const STALE_PULL_DIR_IDLE: Duration = Duration::from_secs(30);

/// Where Lume pulls an image before moving the VM into place
/// (`FileManager.temporaryDirectory`: the per-user temp dir on macOS).
fn lume_temp_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; 1024];
        // SAFETY: `buf` is writable for its length; confstr writes at most
        // that many bytes, NUL-terminated.
        let n = unsafe {
            libc::confstr(
                libc::_CS_DARWIN_USER_TEMP_DIR,
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        };
        if n > 0 && n <= buf.len() {
            buf.truncate(n - 1);
            if let Ok(s) = String::from_utf8(buf) {
                return PathBuf::from(s);
            }
        }
    }
    std::env::temp_dir()
}

/// [`sweep_stale_pull_dirs`] in the temp directory `lume` pulls into: its
/// private one (Local's), else `shared`, the user's. Local never looks in
/// the shared one, where Stable's running Lume may be mid-pull.
fn sweep_pull_dirs_for(lume: &LumeCmd, shared: &std::path::Path, idle: Duration) -> usize {
    sweep_stale_pull_dirs(lume.temp_dir().unwrap_or(shared), idle)
}

/// Removes the `lume_vm_*` dirs in `temp` (a pull's half-made VM, left by a
/// `lume serve` that was killed mid-pull: it cleans up only when it can)
/// that nothing wrote for `idle`. Returns how many it removed.
fn sweep_stale_pull_dirs(temp: &std::path::Path, idle: Duration) -> usize {
    fn newest(path: &std::path::Path, depth: u8) -> Option<std::time::SystemTime> {
        let mut t = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
        if depth > 0
            && let Ok(entries) = std::fs::read_dir(path)
        {
            for e in entries.flatten() {
                if let Some(m) = newest(&e.path(), depth - 1) {
                    t = t.max(m);
                }
            }
        }
        Some(t)
    }
    let Ok(entries) = std::fs::read_dir(temp) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let path = e.path();
        let ours = e.file_name().to_string_lossy().starts_with("lume_vm_")
            && e.file_type().is_ok_and(|t| t.is_dir());
        let stale = newest(&path, 3)
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= idle);
        if ours && stale && std::fs::remove_dir_all(&path).is_ok() {
            tracing::info!(dir = %path.display(), "removed a pull a stopped lume serve left");
            removed += 1;
        }
    }
    removed
}

/// How long a booting VM may be unknown to Lume before the wait fails.
const VM_GONE_GRACE: Duration = Duration::from_secs(3);

/// How long to keep retrying the environment write while the guest's sshd
/// comes up after boot.
const MACOS_ENV_BUDGET: Duration = Duration::from_secs(180);
const MACOS_ENV_RETRY: Duration = Duration::from_secs(3);

/// The script that installs a workload environment in a macOS guest, run as
/// the login user through `lume ssh` (no agent in the image needed):
///
/// - `~/.cua/env.sh` (`0600`, `export K='v'` lines), sourced from
///   `~/.zshenv` and `~/.profile`, so every shell (Terminal, `ssh <cmd>`,
///   `zsh -c`) sees it;
/// - `launchctl setenv` for each variable (apps launched afterwards inherit
///   it), re-applied at every login by the `ai.cua.env` LaunchAgent;
/// - the previous variables are unset first, so an empty `env` clears them.
///
/// The contents travel base64-encoded inside the script (`lume ssh` does not
/// forward stdin); values are plain environment, not secrets.
pub fn macos_env_script(env: &std::collections::BTreeMap<String, String>) -> Result<String> {
    use base64::Engine as _;
    crate::cloudinit::validate_guest_env(env)?;
    let q = crate::exec::shell_quote;
    let mut env_sh = String::from("# Written by cua-vmm: the sandbox environment.\n");
    let mut setenv = String::new();
    let mut unset = String::new();
    for (k, v) in env {
        env_sh.push_str(&format!("export {k}={}\n", q(v)));
        setenv.push_str(&format!("launchctl setenv {k} {}\n", q(v)));
        unset.push_str(&format!("launchctl unsetenv {k}\n"));
    }
    let plist = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>ai.cua.env</string>\n  \
<key>ProgramArguments</key>\n  <array>\n    <string>/bin/sh</string>\n    <string>-c</string>\n    \
<string>[ -r \"$HOME/.cua/env-launchctl.sh\" ] &amp;&amp; /bin/sh \"$HOME/.cua/env-launchctl.sh\"</string>\n  \
</array>\n  <key>RunAtLoad</key><true/>\n</dict>\n</plist>\n";
    let b = |t: &str| base64::engine::general_purpose::STANDARD.encode(t);
    let line = "[ -r \"$HOME/.cua/env.sh\" ] && . \"$HOME/.cua/env.sh\" # cua-vmm env";
    Ok(format!(
        "set -e; d=\"$HOME/.cua\"; mkdir -p \"$d\" \"$HOME/Library/LaunchAgents\"; umask 077; \
         if [ -f \"$d/env-unset.sh\" ]; then /bin/sh \"$d/env-unset.sh\" || true; fi; \
         printf %s '{env}' | base64 --decode > \"$d/env.sh\"; \
         printf %s '{setenv}' | base64 --decode > \"$d/env-launchctl.sh\"; \
         printf %s '{unset}' | base64 --decode > \"$d/env-unset.sh\"; \
         /bin/sh \"$d/env-launchctl.sh\"; \
         for f in \"$HOME/.zshenv\" \"$HOME/.profile\"; do \
         grep -qs 'cua-vmm env' \"$f\" || printf '\\n%s\\n' {line} >> \"$f\"; done; \
         printf %s '{plist}' | base64 --decode > \"$HOME/Library/LaunchAgents/ai.cua.env.plist\"; \
         echo cua-env-ok",
        env = b(&env_sh),
        setenv = b(&setenv),
        unset = b(&unset),
        line = q(line),
        plist = b(plist),
    ))
}

/// Runs [`macos_env_script`] through `exec`, retrying (bounded by `budget`)
/// while the guest's sshd is not up yet. Fails loudly: an environment the
/// caller asked for is never dropped.
pub async fn apply_macos_env(
    exec: &dyn GuestExec,
    name: &str,
    env: &std::collections::BTreeMap<String, String>,
    budget: Duration,
    retry: Duration,
) -> Result<()> {
    let script = macos_env_script(env)?;
    let deadline = tokio::time::Instant::now() + budget;
    let max_attempts = (budget.as_millis() / retry.as_millis().max(1)).max(1) as usize + 1;
    let mut last = String::new();
    for _ in 0..max_attempts {
        let req = crate::exec::ExecRequest::sh(script.clone()).timeout(Duration::from_secs(60));
        match exec.exec(req).await {
            Ok(out) if out.success() && out.stdout_str().contains("cua-env-ok") => return Ok(()),
            Ok(out) => last = crate::exec::tail(&out.stderr_str(), 500),
            Err(e) => last = e.to_string(),
        }
        if tokio::time::Instant::now() + retry > deadline {
            break;
        }
        tokio::time::sleep(retry).await;
    }
    Err(VmmError::Timeout {
        name: name.to_string(),
        secs: budget.as_secs(),
        detail: format!("could not set the sandbox environment over `lume ssh`: {last}"),
    })
}

/// How long `lume get` may take when no `lume serve` answers.
const CLI_STATUS_TIMEOUT: Duration = Duration::from_secs(15);

/// A VM's status from `lume get <name> --format json`, for when `lume serve`
/// is not running.
async fn cli_status(lume: &LumeCmd, name: &str) -> Result<Status> {
    let mut cmd = lume.tokio_command();
    cmd.args(["get", name, "--format", "json"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(CLI_STATUS_TIMEOUT, cmd.output())
        .await
        .map_err(|_| VmmError::Timeout {
            name: name.to_string(),
            secs: CLI_STATUS_TIMEOUT.as_secs(),
            detail: format!("lume get {name}"),
        })??;
    parse_cli_get(
        name,
        out.status.success(),
        out.status.code(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

/// The status in `lume get --format json` output (a one-element array, or
/// an object), or not found when Lume says it has no such VM.
fn parse_cli_get(
    name: &str,
    success: bool,
    code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> Result<Status> {
    if !success {
        if format!("{stdout}\n{stderr}").contains("not found") {
            return Err(VmmError::NotFound(name.into()));
        }
        return Err(VmmError::Command {
            cmd: format!("lume get {name}"),
            code,
            stderr: crate::exec::tail(stderr, 2000),
        });
    }
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| VmmError::other(format!("lume get {name}: unreadable output: {e}")))?;
    let vm = match &v {
        serde_json::Value::Array(a) => a.iter().find(|vm| vm["name"] == name).or(a.first()),
        o => Some(o),
    };
    vm.and_then(|vm| vm["status"].as_str())
        .map(map_status)
        .ok_or_else(|| VmmError::other(format!("lume get {name}: no status in its output")))
}

fn map_status(s: &str) -> Status {
    match s {
        "running" => Status::Running,
        "stopped" | "stop" => Status::Stopped,
        "pulling" | "provisioning" | "starting" => Status::Provisioning,
        "suspended" | "paused" => Status::Paused,
        other => Status::Unknown(other.into()),
    }
}

/// Run the official Lume installer (explicit opt-in only).
pub async fn install_lume() -> Result<()> {
    let curl = host::which("curl")
        .ok_or_else(|| VmmError::missing("curl", "needed to download the Lume installer"))?;
    let script = host::run(curl, &["-fsSL", INSTALL_URL]).await?;
    tracing::info!("running the Lume installer");
    let out = crate::exec::run_with_stdin(
        "/bin/bash",
        &["-s".into()],
        Some(script.as_bytes()),
        Some(Duration::from_secs(600)),
    )
    .await?;
    out.check("lume installer")?;
    // The installer's LaunchAgent logs to /tmp; record that cua installed
    // it, so `cua cache prune` caps those logs (a user's own install is
    // left alone).
    let marker = lume_installed_marker();
    if let Some(d) = marker.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&marker, host::now_secs().to_string());
    Ok(())
}

/// Marker file: this cua home installed Lume ([`install_lume`]).
pub fn lume_installed_marker() -> PathBuf {
    host::cua_home()
        .join("vmm")
        .join("lume")
        .join("installed-by-cua")
}

/// Logs the Lume installer's LaunchAgent writes (`lume serve` as a daemon).
pub const LUME_DAEMON_LOGS: [&str; 2] = ["/tmp/lume_daemon.log", "/tmp/lume_daemon.error.log"];

/// The GPU options Lume offers `spec`: GPU acceleration for a macOS guest.
/// An OCI image is one (Lume pulls and clones macOS bases; a Linux guest
/// boots from a disk image), even when the caller left `os` at its Linux
/// default, as the Spaces app's create does (CUA-1778).
/// Whether `spec` boots a macOS guest: Lume pulls macOS images from OCI
/// registries (a Linux guest boots from a disk), whatever `os` says (the
/// Spaces app's create leaves it at its default).
fn macos_guest(spec: &StartSpec) -> bool {
    spec.os == GuestOs::Macos || matches!(spec.image, ImageSource::Oci { .. })
}

fn gpu_offered(spec: &StartSpec) -> &'static [&'static str] {
    if macos_guest(spec) {
        &[crate::gpu::PARAVIRTUAL]
    } else {
        &[]
    }
}

/// What a base pull is checked for: the caller's size when it knows it,
/// else [`crate::disk::LUME_PULL_ESTIMATE`].
fn lume_pull_need(pull_bytes: Option<u64>) -> u64 {
    pull_bytes
        .filter(|b| *b > 0)
        .unwrap_or(crate::disk::LUME_PULL_ESTIMATE)
}

impl LumeRuntime {
    /// Starts `spec`; sets `created` once a new VM exists in Lume (a clone
    /// or a Linux VM shell), so a failure after that deletes it.
    async fn start_vm(&self, spec: &StartSpec, created: &mut bool) -> Result<Instance> {
        validate_name(&spec.name)?;
        crate::types::reject_sidecars(BackendKind::Lume, spec)?;
        crate::types::reject_gpu(BackendKind::Lume, spec, gpu_offered(spec))?;
        // Lume answers, so only the host (Apple silicon) can say no.
        if spec.gpu.is_some()
            && let Err(why) = crate::gpu::lume_support(true).pick(crate::gpu::PARAVIRTUAL)
        {
            return Err(VmmError::invalid(why));
        }
        // Under the secure default the VM runs behind its own Cua network
        // gateway, which enforces its settings ("none" too: the gateway
        // drops everything the guest opens, while cua still reaches it
        // through the gateway's forwards). No protection runs it on Apple's
        // NAT as before.
        let settings = crate::network::spec_settings(spec);
        match crate::network::gate(BackendKind::Lume, &spec.name, &settings)? {
            Some(crate::network::NetworkPlan::Open { .. }) => self.forget_network(&spec.name),
            Some(_) => return Err(VmmError::Other(crate::network::WHY_LUME.into())),
            None if protected(&settings) => self.keep_network(&spec.name, &settings)?,
            None => {
                self.forget_network(&spec.name);
                crate::types::reject_restricted_network(BackendKind::Lume, spec)?;
                crate::egress::check(BackendKind::Lume, &spec.egress)?;
            }
        }
        // Record the published guest ports before anything starts: the VM's
        // gateway forwards exactly the ports recorded here, so recording
        // them only after boot (`finish_start`) left a first start's
        // gateway with SSH alone and the host with no route to cua-spacesd.
        self.save_ports(&spec.name, &spec.ports);
        crate::types::reject_injection(BackendKind::Lume, spec)?;
        if macos_guest(spec) {
            macos_mount_shares(&spec.mounts)?;
        } else {
            // Linux guests on Lume boot from a disk with no shared folders.
            crate::types::reject_mounts(BackendKind::Lume, spec)?;
        }
        self.ensure_serving().await?;
        // Linux disk VMs always get SSH access: the caller's or a managed
        // per-instance key (seeded through cloud-init).
        let owned;
        let spec = if spec.os == GuestOs::Linux
            && spec.ssh.is_none()
            && spec.cloud_init_user_data.is_none()
            && (matches!(spec.image, ImageSource::Disk { .. })
                || self.linux_state(&spec.name).is_some())
        {
            let dir = self.cfg.root.join(&spec.name);
            std::fs::create_dir_all(&dir)?;
            let mut s = spec.clone();
            s.ssh = crate::cloudinit::managed_ssh(&dir).await;
            owned = s;
            &owned
        } else {
            spec
        };
        let existing = self.client.get(&spec.name).await?;
        // A stopped VM with a saved state (a fork of a checkpoint, a
        // suspended Space) resumes with the host name it was saved with.
        let resumes_state = existing
            .as_ref()
            .is_some_and(|vm| vm.saved_state && vm.status != "running");
        let mut macos_booted = false;
        match (&existing, &spec.image) {
            (Some(vm), _) if vm.status == "running" => {}
            (Some(_), _) => {
                // Stopped VM (fork/clone or earlier run): boot it again.
                match self.linux_state(&spec.name) {
                    Some((disk, mut seed)) => {
                        // A new start spec may carry a new token: reseed
                        // (a Linux guest always gets a seed).
                        if let Some(s) = crate::cloudinit::seed_for_spec(&spec.name, spec)? {
                            let p = self.cfg.root.join(&spec.name).join("seed.iso");
                            s.write_iso(&p)?;
                            seed = Some(p);
                        }
                        self.run_linux_detached(&spec.name, &disk, seed.as_deref())
                            .await?
                    }
                    None => {
                        let run = self.protected_run(&spec.name, Some(spec)).await?;
                        self.before_macos_boot(&spec.name).await?;
                        self.client.run(&spec.name, &run).await?;
                        if resumes_state {
                            self.wait_restored(&spec.name).await;
                        }
                        macos_booted = true;
                    }
                }
            }
            (None, ImageSource::Existing) => return Err(VmmError::NotFound(spec.name.clone())),
            (None, ImageSource::Oci { reference }) => {
                let base = base_vm_name(reference);
                self.ensure_base_sized(
                    &spec.image,
                    &base,
                    spec.registry_auth.as_ref(),
                    spec.pull_bytes,
                )
                .await?;
                crate::disk::ensure_space(
                    &self.vm_dir(),
                    crate::disk::VM_CREATE_ESTIMATE,
                    &format!("create VM {}", spec.name),
                )?;
                // A warm state of this image (booted to readiness, no
                // identity, saved; [`LumeRuntime::prepare_warm`]) resumes in
                // seconds instead of booting macOS: use it when it fits.
                if let Some(warm) = self.usable_warm(&base, spec).await? {
                    crate::progress::report(crate::progress::Progress::phase(
                        crate::progress::Phase::Creating,
                    ));
                    let mut made = CloneGuard::new(self.client.clone(), &spec.name, &self.cfg.root);
                    self.client
                        .clone_vm_with_state(&warm, &spec.name)
                        .await
                        .inspect_err(|_| made.done())?;
                    *created = true;
                    OwnedVms::default().mark(&spec.name, OwnedKind::Instance, Some(&base));
                    OwnedVms::default().touch(&base);
                    let run = self
                        .protected_run(&spec.name, Some(spec))
                        .await
                        .inspect_err(|_| made.done())?;
                    crate::progress::report(crate::progress::Progress::phase(
                        crate::progress::Phase::Booting,
                    ));
                    self.client
                        .run(&spec.name, &run)
                        .await
                        .inspect_err(|_| made.done())?;
                    made.done();
                    self.wait_restored(&spec.name).await;
                    tracing::info!(vm = %spec.name, warm = %warm, "resumed a new Space from the image's warm state");
                    let inst = self.finish_start(spec, true).await?;
                    set_macos_hostname_later(self.lume_or_log(), &spec.name, &spec.name);
                    return Ok(inst);
                }
                // APFS clonefile: copy-on-write, so the clone costs only what
                // the guest writes.
                crate::progress::report(crate::progress::Progress::phase(
                    crate::progress::Phase::Creating,
                ));
                // From here the VM exists in Lume (a clone Lume finishes even
                // when the request is dropped): a create cut off before it
                // booted deletes it again.
                let mut made = CloneGuard::new(self.client.clone(), &spec.name, &self.cfg.root);
                // An error is Lume's answer (no clone made); only a create
                // cut off mid-request leaves the guard to clean up.
                self.client
                    .clone_vm(&base, &spec.name)
                    .await
                    .inspect_err(|_| made.done())?;
                *created = true;
                OwnedVms::default().mark(&spec.name, OwnedKind::Instance, Some(&base));
                OwnedVms::default().touch(&base);
                // A clone inherits the base's CPU, memory and disk: apply the
                // requested ones before the first boot.
                if let Err(e) = self.apply_clone_resources(spec).await {
                    let _ = self.client.delete(&spec.name).await;
                    OwnedVms::default().forget(&spec.name);
                    made.done();
                    return Err(e);
                }
                if spec.gpu.is_some() {
                    self.gpu().enable(&spec.name).inspect_err(|_| made.done())?;
                }
                let run = self
                    .protected_run(&spec.name, Some(spec))
                    .await
                    .inspect_err(|_| made.done())?;
                crate::progress::report(crate::progress::Progress::phase(
                    crate::progress::Phase::Booting,
                ));
                self.before_macos_boot(&spec.name)
                    .await
                    .inspect_err(|_| made.done())?;
                self.client
                    .run(&spec.name, &run)
                    .await
                    .inspect_err(|_| made.done())?;
                made.done();
                macos_booted = true;
            }
            (None, ImageSource::Disk { path }) => {
                if spec.os != GuestOs::Linux {
                    return Err(VmmError::invalid(
                        "lume boots disk images for Linux guests only",
                    ));
                }
                let (disk, seed) = self.prepare_linux(spec, path).await?;
                let size_gb = std::fs::metadata(&disk)?.len().div_ceil(1 << 30).max(1);
                self.client
                    .create(&CreateVm {
                        name: spec.name.clone(),
                        os: "linux".into(),
                        cpu: spec.cpus,
                        memory: format!("{}MB", spec.memory_mb),
                        disk_size: format!("{size_gb}GB"),
                        display: "1024x768".into(),
                        ipsw: None,
                        storage: None,
                    })
                    .await?;
                *created = true;
                OwnedVms::default().mark(&spec.name, OwnedKind::Instance, None);
                // Creation is asynchronous; wait until the VM shell exists.
                let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
                loop {
                    match self.client.get(&spec.name).await? {
                        Some(vm) if vm.status != "provisioning" => break,
                        _ if tokio::time::Instant::now() > deadline => {
                            return Err(VmmError::Timeout {
                                name: spec.name.clone(),
                                secs: 120,
                                detail: "lume create did not finish".into(),
                            });
                        }
                        _ => tokio::time::sleep(Duration::from_millis(500)).await,
                    }
                }
                self.run_linux_detached(&spec.name, &disk, seed.as_deref())
                    .await?;
            }
        }
        let inst = self.finish_start(spec, macos_booted).await?;
        if resumes_state && macos_booted {
            set_macos_hostname_later(self.lume_or_log(), &spec.name, &spec.name);
        }
        Ok(inst)
    }

    /// The rest of a start once the VM runs: its address, a macOS guest's
    /// environment, the published ports and the readiness probes.
    async fn finish_start(&self, spec: &StartSpec, macos_booted: bool) -> Result<Instance> {
        crate::progress::report(crate::progress::Progress::phase(
            crate::progress::Phase::Booting,
        ));
        let vm = self.wait_for_ip(&spec.name).await?;
        if macos_booted {
            let exec = self.lume()?.map(|l| lume_ssh_prefix(&l, &spec.name));
            self.sync_macos_env(
                &spec.name,
                &spec.env,
                exec.as_ref().map(|e| e as &dyn GuestExec),
            )
            .await?;
        }
        // Remember the published guest ports: a later process reattaching
        // by name (`endpoints`) must still see e.g. cua-spacesd's 3211.
        self.save_ports(&spec.name, &spec.ports);
        let inst = self.instance_gw(&vm, spec.ssh.as_ref(), &spec.ports);
        if !spec.probes.is_empty() {
            crate::progress::report(crate::progress::Progress::phase(
                crate::progress::Phase::WaitingForServices,
            ));
        }
        self.wait_services(
            &spec.name,
            inst,
            spec.ssh.as_ref(),
            &spec.ports,
            &spec.probes,
            spec.ready_timeout,
        )
        .await
    }

    /// Waits for `probes` on the VM `name` (started with `ports` forwarded
    /// over vsock, [`Self::macos_run`]) and returns the instance to use:
    /// the vsock path (host loopback ports) once the guest agent answers,
    /// else the VM's address (`ip_inst`). Both are tried together; a VM
    /// address that has no route (macOS Local Network privacy, a VPN) only
    /// fails the wait when the vsock path also fails to come up within
    /// [`VSOCK_GRACE`] (a guest without the agent, or a Lume without vsock).
    async fn wait_services(
        &self,
        name: &str,
        ip_inst: Instance,
        ssh: Option<&SshAccess>,
        ports: &[u16],
        probes: &[Probe],
        timeout: Duration,
    ) -> Result<Instance> {
        let deadline = tokio::time::Instant::now() + timeout;
        if ip_inst.endpoints.host == VSOCK_HOST || ports.is_empty() {
            crate::probe::wait_all(name, &ip_inst.endpoints, probes, timeout).await?;
            return Ok(ip_inst);
        }
        let via_vsock = async {
            let inst = self.vsock_instance(name, ssh, ports, deadline).await?;
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            crate::probe::wait_all(name, &inst.endpoints, probes, left)
                .await
                .ok()?;
            Some(inst)
        };
        let ip_endpoints = ip_inst.endpoints.clone();
        let via_ip = crate::probe::wait_all(name, &ip_endpoints, probes, timeout);
        tokio::pin!(via_vsock, via_ip);
        let mut vsock_over = false;
        loop {
            tokio::select! {
                biased;
                r = &mut via_vsock, if !vsock_over => match r {
                    Some(inst) => return Ok(inst),
                    None => vsock_over = true,
                },
                ip = &mut via_ip => {
                    // Both work: prefer the path a VPN connecting later
                    // cannot break, when it is already up. No route: give
                    // the vsock path its grace before saying so.
                    let tail = match &ip {
                        Ok(()) => Duration::from_secs(2),
                        Err(_) => VSOCK_GRACE,
                    };
                    if !vsock_over
                        && let Ok(Some(inst)) = tokio::time::timeout(tail, &mut via_vsock).await
                    {
                        if let Err(e) = &ip {
                            tracing::info!(vm = name, error = %e, "reached the guest over vsock; its address did not answer");
                        }
                        return Ok(inst);
                    }
                    return ip.map(|()| ip_inst);
                }
            }
        }
    }

    /// The instance on the vsock path once Lume reports `ports` forwarded
    /// with the guest agent answering, or `None` when it never will (Lume
    /// runs the VM without vsock) or `deadline` passes.
    async fn vsock_instance(
        &self,
        name: &str,
        ssh: Option<&SshAccess>,
        ports: &[u16],
        deadline: tokio::time::Instant,
    ) -> Option<Instance> {
        // Lume records the forwards right after the VM starts: a VM seen
        // running without them this many times in a row has none.
        const NO_VSOCK_POLLS: u32 = 4;
        let mut running_without = 0;
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(vm)) = self.client.get(name).await {
                if vm.vsock_ports(ports).is_some() {
                    return Some(Self::instance(&vm, ssh, ports));
                }
                // An older Lume, or a resume of a state saved without the
                // socket device.
                if map_status(&vm.status) == Status::Running && !vm.vsock {
                    running_without += 1;
                    if running_without >= NO_VSOCK_POLLS {
                        return None;
                    }
                } else {
                    running_without = 0;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        None
    }

    async fn stop_vm(&self, name: &str) -> Result<()> {
        self.ensure_serving().await?;
        match self.client.get(name).await? {
            None => Err(VmmError::NotFound(name.into())),
            Some(vm) if vm.status == "stopped" => Ok(()),
            Some(_) => self.client.stop(name).await,
        }
    }

    /// Deletes `name`, which this start created and failed to bring up
    /// with `why`. Best effort: a failure to delete is logged.
    async fn discard_failed(&self, name: &str, why: &VmmError) {
        match Runtime::delete(self, name).await {
            Ok(()) | Err(VmmError::NotFound(_)) => {
                tracing::info!(vm = name, error = %why, "deleted a VM whose create failed")
            }
            Err(e) => {
                tracing::warn!(vm = name, error = %e, "could not delete a VM whose create failed")
            }
        }
    }

    /// A warm VM of base `base` a start of `spec` can resume from: it
    /// exists, holds a saved state, has the CPU, memory and disk the spec
    /// asks for (a saved state restores only into the configuration it was
    /// saved from) and its MAC address is not in use by a running VM (every
    /// resume of one state has that state's address). `None` when there is
    /// none, or the spec needs a GPU (a warm state is saved without one).
    async fn usable_warm(&self, base: &str, spec: &StartSpec) -> Result<Option<String>> {
        // A saved state restores only into the shared directories it was
        // saved with: a start with mounts boots cold.
        if spec.gpu.is_some() || !spec.mounts.is_empty() {
            return Ok(None);
        }
        let mut fits = Vec::new();
        for warm in warm_vm_names(base) {
            if let Some(vm) = self.client.get(&warm).await?
                && vm.saved_state
                && vm.status == "stopped"
                && clone_resources(&vm, spec).is_empty()
            {
                fits.push(vm);
            }
        }
        if fits.is_empty() {
            return Ok(None);
        }
        let in_use: Vec<String> = self
            .client
            .list()
            .await?
            .iter()
            .filter(|v| v.status == "running")
            .filter_map(|v| v.mac_address.as_deref().map(str::to_ascii_lowercase))
            .collect();
        Ok(fits
            .into_iter()
            .find(|vm| {
                vm.mac_address
                    .as_deref()
                    .is_some_and(|m| !in_use.contains(&m.to_ascii_lowercase()))
            })
            .map(|vm| vm.name))
    }

    /// Makes the warm states of `spec.image` (an OCI macOS image): for each
    /// of [`warm_vm_names`], a clone of its base VM with `spec`'s CPU,
    /// memory and disk, booted with no identity (no Spaces token: a Spaces
    /// image's cua-spacesd waits for one in bootstrap mode), waited on until
    /// `spec.probes` pass and `settle` more, then saved and stopped. From
    /// then on a start of that image with the same resources resumes a
    /// copy-on-write clone of one instead of booting macOS, and the SDK
    /// installs the Space's token with its first `Init`, as for any driver
    /// in bootstrap mode. Every resume of a state has the MAC address it
    /// was saved with, so there are as many states as macOS lets a host run
    /// guests (two), each with its own address. Existing warm states are
    /// replaced. It runs one macOS VM at a time while it works.
    pub async fn prepare_warm(&self, spec: &StartSpec, settle: Duration) -> Result<Vec<String>> {
        let ImageSource::Oci { reference } = &spec.image else {
            return Err(VmmError::invalid(
                "a warm state is made from an OCI macOS image",
            ));
        };
        if spec.os != GuestOs::Macos {
            return Err(VmmError::invalid("warm states are for macOS images"));
        }
        self.ensure_serving().await?;
        let base = base_vm_name(reference);
        self.ensure_base_sized(
            &spec.image,
            &base,
            spec.registry_auth.as_ref(),
            spec.pull_bytes,
        )
        .await?;
        let mut made = Vec::new();
        for warm in warm_vm_names(&base) {
            self.prepare_one_warm(spec, reference, &base, &warm, settle)
                .await?;
            made.push(warm);
        }
        Ok(made)
    }

    /// One warm state of [`Self::prepare_warm`], built under a temporary
    /// name and cloned into place only once saved, so a start never resumes
    /// a half-made one.
    async fn prepare_one_warm(
        &self,
        spec: &StartSpec,
        reference: &str,
        base: &str,
        warm: &str,
        settle: Duration,
    ) -> Result<()> {
        let building = format!("{warm}-new");
        for stale in [building.as_str(), warm] {
            if self.client.get(stale).await?.is_some() {
                let _ = self.client.stop_for_delete(stale).await;
                self.client.delete(stale).await?;
                OwnedVms::default().forget(stale);
            }
        }
        let started = std::time::Instant::now();
        self.client.clone_vm(base, &building).await?;
        OwnedVms::default().mark(&building, OwnedKind::Base, Some(reference));
        let made = async {
            let mut boot = spec.clone();
            boot.name = building.clone();
            boot.env.clear();
            self.apply_clone_resources(&boot).await?;
            self.before_macos_boot(&building).await?;
            // The same run options as a Space's (the setup share, empty):
            // the state restores only into the devices it was saved with.
            let run = self.macos_run(&building, Some(&boot))?;
            self.client.run(&building, &run).await?;
            let vm = self.wait_for_ip(&building).await?;
            let inst = Self::instance(&vm, None, &spec.ports);
            self.wait_services(
                &building,
                inst,
                None,
                &spec.ports,
                &spec.probes,
                spec.ready_timeout,
            )
            .await?;
            tokio::time::sleep(settle).await;
            let ready = started.elapsed();
            self.client.suspend(&building).await?;
            tracing::info!(
                vm = %building,
                ready_ms = ready.as_millis() as u64,
                total_ms = started.elapsed().as_millis() as u64,
                "saved a warm state"
            );
            self.client.clone_vm_with_state(&building, warm).await
        }
        .await;
        let _ = self.client.stop_for_delete(&building).await;
        let _ = self.client.delete(&building).await;
        OwnedVms::default().forget(&building);
        let _ = std::fs::remove_dir_all(self.cfg.root.join(&building));
        made?;
        OwnedVms::default().mark(warm, OwnedKind::Base, Some(reference));
        Ok(())
    }

    /// After `lume run` of a VM with a saved state: waits (bounded) until
    /// Lume has restored it, which it reports by dropping the state (or
    /// booting instead, when the state did not restore). Until then the
    /// guest is paused, so probes against it would only time out.
    async fn wait_restored(&self, name: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        while tokio::time::Instant::now() < deadline {
            match self.client.get(name).await {
                Ok(Some(vm)) if vm.saved_state && vm.status != "stopped" => {}
                _ => return,
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// [`Runtime::checkpoint`] of the running macOS VM `name` with its
    /// memory: Lume saves its state and stops it (`suspend`), the state is
    /// cloned with the disk (copy-on-write) into `checkpoint`, and `name`
    /// resumes from its own state. `name` keeps running whatever happens
    /// after the save: it is resumed even when the clone fails.
    /// [`VmmError::Unsupported`] (nothing done) from a Lume without saved
    /// states.
    async fn checkpoint_running(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
        if self.client.get(checkpoint).await?.is_some() {
            return Err(VmmError::AlreadyExists(checkpoint.into()));
        }
        let started = std::time::Instant::now();
        self.client.suspend(name).await?;
        let saved = started.elapsed();
        let cloned = self.client.clone_vm_with_state(name, checkpoint).await;
        if cloned.is_ok() {
            OwnedVms::default().mark(checkpoint, OwnedKind::Checkpoint, Some(name));
        }
        let run = self.protected_run(name, None).await?;
        self.before_macos_boot(name).await?;
        self.client.run(name, &run).await?;
        cloned?;
        tracing::info!(
            vm = name,
            checkpoint,
            save_ms = saved.as_millis() as u64,
            total_ms = started.elapsed().as_millis() as u64,
            "checkpointed a running VM with its memory"
        );
        Ok(CheckpointInfo::now(
            checkpoint,
            BackendKind::Lume,
            Some(name.into()),
        ))
    }

    /// `e`, naming the too-old Lume `runtime.lume = system` made cua use.
    fn name_old_lume(&self, e: VmmError) -> VmmError {
        let old = self
            .old_lume
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match (old, e) {
            (
                Some(v),
                e @ (VmmError::Lume { .. } | VmmError::Timeout { .. } | VmmError::Other(_)),
            ) => VmmError::other(format!(
                "{e}. This Mac's Lume is {v}, older than the {} macOS Spaces need: update it, \
                     or choose Built-in for macOS VMs in Settings (`cua config set runtime.lume builtin`)",
                builtin::MIN_VERSION
            )),
            (_, e) => e,
        }
    }
}

#[async_trait]
impl Runtime for LumeRuntime {
    fn kind(&self) -> BackendKind {
        BackendKind::Lume
    }

    async fn ensure_base(&self, image: &ImageSource, base_name: &str) -> Result<CheckpointInfo> {
        self.ensure_base_with(image, base_name, None).await
    }

    async fn start(&self, spec: &StartSpec) -> Result<Instance> {
        // Its tree (the start spec's), for its tree address and members.
        crate::tree::assign(&tree_key(&spec.name), spec.tree.as_deref())?;
        let mut created = false;
        match self.start_vm(spec, &mut created).await {
            Ok(inst) => Ok(inst),
            Err(e) => {
                // A VM this start made and could not bring up (it stopped
                // while booting, never got an address, its services never
                // answered) is deleted again: left behind it holds tens of
                // GB and its name, so the next create would boot the
                // broken one instead of a new one.
                if created {
                    self.discard_failed(&spec.name, &e).await;
                }
                Err(self.name_old_lume(e))
            }
        }
    }

    async fn stop(&self, name: &str) -> Result<()> {
        let stopped = self.stop_vm(name).await;
        // Its tree stops sending it connections.
        let _ = crate::tree::leave(&tree_key(name));
        stopped
    }

    /// A running macOS VM is suspended to disk: Lume saves its memory and
    /// device state next to its disk and stops it, and [`Runtime::resume`]
    /// restores it where it was (no RAM is held meanwhile). Where that is
    /// not possible (a Linux disk VM, which `lume run` hosts, or a Lume
    /// without saved states) suspending stops the VM (its disk and setup
    /// share stay) and resume boots it again, so `cua sb suspend` / `resume`
    /// work for macOS Spaces as they do for the others.
    async fn suspend(&self, name: &str) -> Result<()> {
        self.ensure_serving().await?;
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        if vm.status == "running" && self.linux_state(name).is_none() {
            match self.client.suspend(name).await {
                Err(VmmError::Unsupported { .. }) => {}
                other => return other,
            }
        }
        self.stop(name).await
    }

    /// Boots a stopped VM again, or resumes it from the state
    /// [`Runtime::suspend`] saved (Lume restores a saved state on run): the
    /// way `start` boots an existing VM, with its setup share (the env token the last
    /// start wrote) for a macOS guest and its disk and seed for a Linux disk
    /// VM. Returns once the guest has an IP. A running VM is returned as is.
    async fn resume(&self, name: &str) -> Result<Instance> {
        self.ensure_serving().await?;
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        if map_status(&vm.status) == Status::Stopped {
            match self.linux_state(name) {
                Some((disk, seed)) => {
                    self.run_linux_detached(name, &disk, seed.as_deref())
                        .await?
                }
                None => {
                    let run = self.protected_run(name, None).await?;
                    self.before_macos_boot(name).await?;
                    self.client.run(name, &run).await?;
                    if vm.saved_state {
                        self.wait_restored(name).await;
                    }
                }
            }
        }
        let vm = self.wait_for_ip(name).await?;
        Ok(self.instance_gw(&vm, None, &self.load_ports(name)))
    }

    /// Binds the running pool member `member` to a Space: Lume renames
    /// the running VM to `new_name` (the guest keeps running; only its
    /// directory moves), cua's own state for it moves along, and a macOS
    /// guest gets `identity.hostname` (default `new_name`) in the
    /// background through `lume ssh` (the image's `lume` account). The
    /// Spaces token, machine-seal keys and entropy reach cua-spacesd with
    /// the caller's `Init{new_machine}`: a member runs with no token (its
    /// driver in bootstrap mode). [`VmmError::Unsupported`] from a Lume
    /// without renames.
    async fn claim(
        &self,
        member: &str,
        new_name: &str,
        identity: &ClaimIdentity,
    ) -> Result<Instance> {
        validate_name(new_name)?;
        self.ensure_serving().await?;
        let vm = self
            .client
            .get(member)
            .await?
            .ok_or_else(|| VmmError::NotFound(member.into()))?;
        if vm.status != "running" {
            return Err(VmmError::invalid(format!(
                "pool member '{member}' is not running ({})",
                vm.status
            )));
        }
        if self.client.get(new_name).await?.is_some() {
            return Err(VmmError::AlreadyExists(new_name.into()));
        }
        self.client.rename(member, new_name).await?;
        let (from, to) = (self.cfg.root.join(member), self.cfg.root.join(new_name));
        if from.exists() {
            std::fs::rename(&from, &to)?;
        }
        let owned = OwnedVms::default();
        let source = owned.get(member).and_then(|r| r.source);
        owned.forget(member);
        owned.mark(new_name, OwnedKind::Instance, source.as_deref());
        let vm = self
            .client
            .get(new_name)
            .await?
            .ok_or_else(|| VmmError::NotFound(new_name.into()))?;
        if vm.os.eq_ignore_ascii_case("macos") {
            set_macos_hostname_later(
                self.lume_or_log(),
                new_name,
                identity.hostname.as_deref().unwrap_or(new_name),
            );
        }
        Ok(self.instance_gw(&vm, None, &self.load_ports(new_name)))
    }

    async fn fork(&self, source: &str, new_name: &str) -> Result<()> {
        validate_name(new_name)?;
        // `lume serve` may have exited (an update swapped its binary, a
        // crash): start it again rather than fail "not reachable".
        self.ensure_serving().await?;
        let vm = self
            .client
            .get(source)
            .await?
            .ok_or_else(|| VmmError::NotFound(source.into()))?;
        if vm.status == "running" {
            return Err(VmmError::invalid(format!(
                "'{source}' is running; stop it before cloning"
            )));
        }
        if self.client.get(new_name).await?.is_some() {
            return Err(VmmError::AlreadyExists(new_name.into()));
        }
        // A source with a saved machine state (a checkpoint of a running
        // VM) forks with it: the fork's first run resumes the saved guest.
        if vm.saved_state {
            self.client.clone_vm_with_state(source, new_name).await?;
        } else {
            self.client.clone_vm(source, new_name).await?;
        }
        OwnedVms::default().mark(new_name, OwnedKind::Checkpoint, Some(source));
        // Linux disk VMs keep their disk outside lume: copy it (APFS clonefile
        // via `cp -c` keeps this instant and copy-on-write).
        if let Some((disk, seed)) = self.linux_state(source) {
            let dir = self.cfg.root.join(new_name);
            std::fs::create_dir_all(&dir)?;
            let (s, d) = (
                disk.display().to_string(),
                dir.join("disk.img").display().to_string(),
            );
            if host::run("cp", &["-c", &s, &d]).await.is_err() {
                std::fs::copy(&disk, dir.join("disk.img"))?;
            }
            if let Some(seed) = seed {
                std::fs::copy(seed, dir.join("seed.iso"))?;
            }
        }
        Ok(())
    }

    /// A running macOS VM: save its machine state (memory included) →
    /// clone it with that state into `checkpoint` → resume it from its own
    /// state, so the checkpoint and every fork of it start where the guest
    /// was ([`Self::checkpoint_running`]). Otherwise (a stopped VM, a Linux
    /// disk VM `lume run` hosts, or a Lume without saved states): stop →
    /// clone → restart, a clean stopped-state clone, the same contract as
    /// cloud snapshots.
    async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
        self.ensure_serving().await?;
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        let was_running = vm.status == "running";
        if was_running && self.linux_state(name).is_none() {
            validate_name(checkpoint)?;
            match self.checkpoint_running(name, checkpoint).await {
                Err(VmmError::Unsupported { .. }) => {
                    tracing::info!(
                        vm = name,
                        "this Lume cannot save VM states; checkpointing the stopped disk"
                    );
                }
                other => return other,
            }
        }
        if was_running {
            self.client.stop(name).await?;
        }
        let res = self.fork(name, checkpoint).await;
        if was_running {
            match self.linux_state(name) {
                Some((disk, seed)) => {
                    self.run_linux_detached(name, &disk, seed.as_deref())
                        .await?
                }
                None => {
                    let run = self.protected_run(name, None).await?;
                    self.before_macos_boot(name).await?;
                    self.client.run(name, &run).await?
                }
            }
        }
        res?;
        Ok(CheckpointInfo::now(
            checkpoint,
            BackendKind::Lume,
            Some(name.into()),
        ))
    }

    async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
        self.delete(checkpoint).await
    }

    async fn list(&self) -> Result<Vec<InstanceSummary>> {
        self.ensure_serving().await?;
        Ok(self
            .client
            .list()
            .await?
            .into_iter()
            .map(|vm| InstanceSummary {
                status: map_status(&vm.status),
                name: vm.name,
                backend: BackendKind::Lume,
            })
            .collect())
    }

    async fn status(&self, name: &str) -> Result<Status> {
        match self.client.get(name).await {
            Ok(Some(vm)) => Ok(map_status(&vm.status)),
            Ok(None) => Err(VmmError::NotFound(name.into())),
            // No `lume serve` answers (after a reboot it is not running until
            // something starts it). The `lume` CLI reads the VM's state from
            // its directory without a server, so a status check does not have
            // to start one.
            Err(VmmError::Missing { .. }) => match self.lume()? {
                Some(lume) => cli_status(&lume, name).await,
                // No Lume on this machine at all (the built-in one is set
                // up on first use): it runs no VM, so none has this name.
                None => Err(VmmError::NotFound(name.into())),
            },
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, name: &str) -> Result<()> {
        // `lume serve` may have exited (a crash, an update, a reboot without
        // its login item): start it rather than leave a Space that cannot
        // be deleted.
        self.ensure_serving().await?;
        let dir = self.cfg.root.join(name);
        let Some(vm) = self.client.get(name).await? else {
            // A create cut off before Lume had the VM may have left cua's
            // own state for it.
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                self.forget_gpu(name);
            }
            return Err(VmmError::NotFound(name.into()));
        };
        if vm.status == "running" {
            let _ = self.client.stop_for_delete(name).await;
        }
        self.client.delete(name).await?;
        OwnedVms::default().forget(name);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        self.forget_gpu(name);
        Ok(())
    }

    async fn network_events(&self, name: &str) -> Result<Vec<crate::egress::NetworkEvent>> {
        // A VM behind its gateway: the gateway logs every refused
        // connection beside the VM's other files. On Apple's NAT nothing is
        // counted.
        Ok(crate::netgw::read_events(&self.cfg.root.join(name)))
    }

    async fn mend(&self, name: &str) -> Result<Option<Instance>> {
        // Restarted by whoever noticed first (an endpoints read): tell the
        // daemon once, so it drops what it holds for the old run.
        let was_mended = self
            .mended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
        if was_mended && let Some(vm) = self.client.get(name).await.ok().flatten() {
            return Ok(Some(self.instance_gw(&vm, None, &self.load_ports(name))));
        }
        // Cheap first: only a protected VM whose gateway process is gone
        // (files on this Mac) is worth asking Lume about.
        if self.kept_network(name).is_none()
            || self.load_gateway(name).is_none()
            || crate::netgw::gateway_running(&self.cfg.root.join(name))
        {
            return Ok(None);
        }
        let Some(vm) = self.client.get(name).await.ok().flatten() else {
            return Ok(None);
        };
        if !self.gateway_lost(name, &vm) {
            return Ok(None);
        }
        let vm = self.mend_gateway(name, vm).await?;
        Ok(Some(self.instance_gw(&vm, None, &self.load_ports(name))))
    }

    async fn endpoints(&self, name: &str) -> Result<Endpoints> {
        // As in `delete`: a `lume serve` that exited (a crash, an update,
        // `lume stop` on a VM it hosted) is started again rather than leave a
        // running Space that cannot be opened.
        self.ensure_serving().await?;
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        let vm = self.mend_gateway(name, vm).await?;
        Ok(self.with_gateway(
            name,
            Self::endpoints_from(&vm, None, &self.load_ports(name)),
        ))
    }

    /// [`LumeSshExec`] (`lume ssh <vm>`) for macOS guests (password auth
    /// handled by lume; stdout and stderr kept apart, no stdin);
    /// Linux disk VMs use the SSH endpoint from their spec, so callers should
    /// build an [`crate::SshExec`] from `Instance::endpoints.ssh`.
    async fn guest_exec(&self, name: &str) -> Result<Option<Arc<dyn GuestExec>>> {
        self.ensure_serving().await?;
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        if vm.os.eq_ignore_ascii_case("macos") {
            let lume = self
                .lume()?
                .ok_or_else(|| VmmError::missing("lume", "needed for `lume ssh`"))?;
            return Ok(Some(Arc::new(LumeSshExec::from_lume(&lume, name))));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_base_pull_checks_the_disk_for_the_images_real_size() {
        let gib = 1u64 << 30;
        // The catalog's macOS 26: 24.2 GB download + 34.9 GB unpacked.
        assert_eq!(super::lume_pull_need(Some(59_142_929_031)), 59_142_929_031);
        assert!(super::lume_pull_need(Some(59_142_929_031)) > 50 * gib);
        // Unknown (or a zero size): the estimate.
        assert_eq!(super::lume_pull_need(None), crate::disk::LUME_PULL_ESTIMATE);
        assert_eq!(
            super::lume_pull_need(Some(0)),
            crate::disk::LUME_PULL_ESTIMATE
        );
    }

    use super::*;

    fn runtime_for(channel: CuaChannel, url: &str) -> LumeRuntime {
        LumeRuntime::new(LumeConfig {
            channel,
            url: url.into(),
            spawn_serve: false,
            ..LumeConfig::default()
        })
    }

    /// Stable is unchanged (`:7777`); Local serves on its own port, and a
    /// URL without one falls back to the channel's.
    #[test]
    fn each_channel_serves_on_its_own_port() {
        for (channel, port) in [(CuaChannel::Stable, 7777), (CuaChannel::Local, 7787)] {
            assert_eq!(channel.lume_port(), port);
            assert_eq!(runtime_for(channel, channel.lume_api()).port(), port);
            assert_eq!(runtime_for(channel, "http://localhost").port(), port);
        }
        // `LUME_API` (here, the configured URL) still decides when it has a port.
        assert_eq!(
            runtime_for(CuaChannel::Local, "http://127.0.0.1:7900").port(),
            7900
        );
        assert_eq!(
            runtime_for(CuaChannel::Stable, "http://127.0.0.1:7900").port(),
            7900
        );
    }

    /// A runtime keeps its VMs where its channel does.
    #[test]
    fn each_channel_keeps_its_vms_in_its_own_directory() {
        let home = host::home_dir();
        let stable = runtime_for(CuaChannel::Stable, "http://127.0.0.1:1").vm_dir();
        let local = runtime_for(CuaChannel::Local, "http://127.0.0.1:1").vm_dir();
        // Stable ignores LUME_HOME: the pinned Lume does too.
        assert_eq!(stable, home.join(".lume"));
        if crate::storage::lume_home_env().is_none() {
            assert_eq!(local, home.join(".lume-local"));
            assert_ne!(stable, local);
        }
    }

    fn pull_dir(parent: &std::path::Path, name: &str) {
        let d = parent.join(name);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/disk.img"), b"x").unwrap();
    }

    /// Starting Local's Lume sweeps its own temp directory, never the
    /// shared one where Stable's running Lume may be mid-pull. Stable's
    /// sweep is as it was.
    #[test]
    fn local_s_start_up_sweep_leaves_stable_s_pull_dirs_alone() {
        let t = tempfile::tempdir().unwrap();
        let shared = t.path().join("shared-tmp");
        let root = t.path().join("cua-local/vmm/lume");
        let vms = t.path().join(".lume-local");
        std::fs::create_dir_all(&shared).unwrap();
        // Stable's Lume is pulling (quiet for longer than the sweep's idle
        // time, as a pull waiting on the network can be).
        pull_dir(&shared, "lume_vm_stable-pull");
        let local = LumeCmd::with_vm_dir(CuaChannel::Local, &root, "/opt/lume", &vms).unwrap();
        let stable = LumeCmd::with_vm_dir(CuaChannel::Stable, &root, "/opt/lume", &vms).unwrap();
        // Local's Lume pulls into a temp directory of its own, under its home.
        let private = local.temp_dir().expect("Local has its own temp dir");
        assert!(private.starts_with(&root) && private.is_dir());
        assert!(
            local
                .env()
                .iter()
                .any(|(k, v)| *k == "TMPDIR" && v == private.as_os_str())
        );
        assert_eq!(stable.temp_dir(), None);
        assert!(stable.env().iter().all(|(k, _)| *k != "TMPDIR"));
        pull_dir(private, "lume_vm_local-pull");

        // Local's sweep (everything counts as stale) removes its own only.
        assert_eq!(sweep_pull_dirs_for(&local, &shared, Duration::ZERO), 1);
        assert!(
            shared.join("lume_vm_stable-pull").exists(),
            "Stable's pull was swept"
        );
        assert!(!private.join("lume_vm_local-pull").exists());

        // Stable's sweep still covers the shared directory.
        assert_eq!(sweep_pull_dirs_for(&stable, &shared, Duration::ZERO), 1);
        assert!(!shared.join("lume_vm_stable-pull").exists());
    }

    /// A stand-in Lume that finds VMs the way Lume does: the default
    /// location in `$XDG_CONFIG_HOME/lume/config.yaml`, else `stable_dir`.
    /// Named `lume` (the bundled one, Stable's; 0.6.0 and newer), it ignores
    /// `LUME_HOME` for VMs; named `lume-local` it follows it (#5804).
    #[cfg(unix)]
    fn fake_lume_named(name: &str, dir: &std::path::Path, stable_dir: &std::path::Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let follows_lume_home = name == CuaChannel::Local.lume_bin_name();
        let bin = dir.join(name);
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\n\
                 vms='{stable}'\n\
                 cfg=\"${{XDG_CONFIG_HOME:+$XDG_CONFIG_HOME/lume/config.yaml}}\"\n\
                 if [ -n \"$cfg\" ] && [ -f \"$cfg\" ]; then\n\
                   vms=$(sed -n 's/^ *path: *\"\\(.*\\)\"$/\\1/p' \"$cfg\" | head -n 1)\n\
                 fi\n\
                 {home_rule}\
                 sub=\"$1\"; shift\n\
                 case \"$sub\" in\n\
                   get) [ -f \"$vms/$1/config.json\" ] || {{ echo \"Error: Virtual machine not found: $1\"; exit 1; }}\n\
                        echo \"[{{\\\"name\\\":\\\"$1\\\",\\\"status\\\":\\\"stopped\\\"}}]\" ;;\n\
                   pull) mkdir -p \"$vms/$2\" && echo '{{}}' > \"$vms/$2/config.json\" ;;\n\
                   run) [ -f \"$vms/$1/config.json\" ] || {{ echo \"Error: Virtual machine not found: $1\" >&2; exit 1; }}\n\
                        touch \"$vms/$1/ran\" ;;\n\
                   ssh) if [ \"$1\" = --timeout ]; then shift 2; fi\n\
                        [ -f \"$vms/$1/config.json\" ] || {{ echo \"Error: VM '$1' not found\"; exit 1; }}\n\
                        /bin/sh -c \"$2\" 2>&1; rc=$?; echo; exit $rc ;;\n\
                   *) exit 64 ;;\n\
                 esac\n",
                stable = stable_dir.display(),
                home_rule = if follows_lume_home {
                    "[ -n \"$LUME_HOME\" ] && vms=\"$LUME_HOME\"\n"
                } else {
                    ""
                }
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    /// A Lume named `lume-local` (an installed one, which Local prefers)
    /// follows `LUME_HOME`; the same environment keeps it in Local's
    /// directory too (#5804).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_lume_local_named_binary_stays_in_local_s_vm_directory() {
        let t = tempfile::tempdir().unwrap();
        let stable_dir = t.path().join(".lume");
        let local_dir = t.path().join(".lume-local");
        for (dir, vm) in [(&stable_dir, "stable-vm"), (&local_dir, "local-vm")] {
            std::fs::create_dir_all(dir.join(vm)).unwrap();
            std::fs::write(dir.join(vm).join("config.json"), "{}").unwrap();
        }
        let bin = fake_lume_named(CuaChannel::Local.lume_bin_name(), t.path(), &stable_dir);
        let local = LumeCmd::with_vm_dir(
            CuaChannel::Local,
            &t.path().join("cua-root"),
            &bin,
            &local_dir,
        )
        .unwrap();
        assert_eq!(
            cli_status(&local, "local-vm").await.unwrap(),
            Status::Stopped
        );
        assert!(matches!(
            cli_status(&local, "stable-vm").await,
            Err(VmmError::NotFound(_))
        ));
        // Without the environment it would use Stable's, as a `lume` does.
        let bare = LumeCmd::plain(&bin);
        assert_eq!(
            cli_status(&bare, "stable-vm").await.unwrap(),
            Status::Stopped
        );
    }

    /// Both channels' VM directories hold a VM. Every Lume CLI path (`get`,
    /// `pull`, `run`, `ssh` as a guest command prefix and as `LumeSshExec`,
    /// and the `serve` command) sees only its own channel's, even though
    /// the Lume ignores `LUME_HOME` for VMs.
    #[cfg(unix)]
    #[tokio::test]
    async fn every_lume_cli_path_stays_in_its_own_channel_s_vm_directory() {
        use crate::exec::ExecRequest;
        let t = tempfile::tempdir().unwrap();
        let stable_dir = t.path().join(".lume");
        let local_dir = t.path().join(".lume-local");
        for (dir, vm) in [(&stable_dir, "stable-vm"), (&local_dir, "local-vm")] {
            std::fs::create_dir_all(dir.join(vm)).unwrap();
            std::fs::write(dir.join(vm).join("config.json"), "{}").unwrap();
        }
        let bin = fake_lume_named("lume", t.path(), &stable_dir);
        let root = t.path().join("cua-root");
        let stable = LumeCmd::with_vm_dir(CuaChannel::Stable, &root, &bin, &stable_dir).unwrap();
        let local = LumeCmd::with_vm_dir(CuaChannel::Local, &root, &bin, &local_dir).unwrap();
        let timeout = Duration::from_secs(20);
        let own = [
            (&stable, "stable-vm", "local-vm"),
            (&local, "local-vm", "stable-vm"),
        ];

        // The stand-in does what Lume does without the environment: Local
        // would land in Stable's directory.
        let bare = LumeCmd::plain(&bin);
        assert_eq!(
            cli_status(&bare, "stable-vm").await.unwrap(),
            Status::Stopped
        );
        assert!(matches!(
            cli_status(&bare, "local-vm").await,
            Err(VmmError::NotFound(_))
        ));
        // The contract (#5804): a Lume named `lume` takes `LUME_HOME` for
        // its release-channel file only, so `LUME_HOME` alone does not move
        // its VMs. The config under `XDG_CONFIG_HOME` is what does.
        let only_home = std::process::Command::new(&bin)
            .args(["get", "local-vm", "--format", "json"])
            .env("LUME_HOME", &local_dir)
            .output()
            .unwrap();
        assert!(
            !only_home.status.success(),
            "LUME_HOME alone isolated a `lume`"
        );
        assert!(local.env().iter().any(|(k, _)| *k == "XDG_CONFIG_HOME"));

        for (lume, mine, other) in own {
            // `lume get`
            assert_eq!(cli_status(lume, mine).await.unwrap(), Status::Stopped);
            assert!(matches!(
                cli_status(lume, other).await,
                Err(VmmError::NotFound(_))
            ));
            // `lume run`
            cli_run(lume, &["run".into(), mine.into()]).await.unwrap();
            assert!(cli_run(lume, &["run".into(), other.into()]).await.is_err());
            // `lume ssh` as a guest command prefix (hostname, environment)
            let out = lume_ssh_prefix(lume, mine)
                .exec(ExecRequest::sh("echo hi").timeout(timeout))
                .await
                .unwrap();
            assert_eq!((out.exit_code, out.stdout_str().trim()), (0, "hi"));
            let out = lume_ssh_prefix(lume, other)
                .exec(ExecRequest::sh("echo hi").timeout(timeout))
                .await
                .unwrap();
            assert_eq!(out.exit_code, 1);
            assert!(out.stdout_str().contains("not found"));
            // `lume ssh` as `LumeSshExec` (guest exec)
            let ssh = LumeSshExec::from_lume(lume, mine);
            let out = ssh
                .exec(ExecRequest::sh("echo hi").timeout(timeout))
                .await
                .unwrap();
            assert_eq!((out.exit_code, out.stdout_str().trim()), (0, "hi"));
            let ssh = LumeSshExec::from_lume(lume, other);
            let out = ssh
                .exec(ExecRequest::sh("echo hi").timeout(timeout))
                .await
                .unwrap();
            assert_eq!(out.exit_code, 1);
            // the command `serve` is started from (and `lume attach` is
            // handed to another process as)
            let mut cmd = lume.tokio_command();
            let got = cmd.args(["get", other]).output().await.unwrap();
            assert!(!got.status.success());
            let argv = lume.argv(["get", mine]);
            let got = tokio::process::Command::new(&argv[0])
                .args(&argv[1..])
                .output()
                .await
                .unwrap();
            assert!(got.status.success(), "{argv:?}");
        }

        // `lume pull` makes a base in its own directory, and only there.
        let creds = RegistryCredentials::new("me", "s3cret");
        for (lume, dir, elsewhere, base) in [
            (&stable, &stable_dir, &local_dir, "stable-base"),
            (&local, &local_dir, &stable_dir, "local-base"),
        ] {
            cli_pull(lume, "ghcr.io/me/img:1", base, &creds, timeout)
                .await
                .unwrap();
            assert!(dir.join(base).join("config.json").is_file(), "{base}");
            assert!(!elsewhere.join(base).exists(), "{base} leaked");
        }
        // Each `run` only touched its own VM.
        assert!(stable_dir.join("stable-vm/ran").exists());
        assert!(local_dir.join("local-vm/ran").exists());
        assert!(!stable_dir.join("local-vm").exists() && !local_dir.join("stable-vm").exists());
    }

    /// A killed serve's half-made pull is removed; a pull still being
    /// written, and anything not Lume's, stays.
    #[test]
    fn a_stale_pull_dir_is_swept_and_a_live_one_stays() {
        let t = tempfile::tempdir().unwrap();
        for d in ["lume_vm_dead", "lume_vm_live", "other"] {
            std::fs::create_dir_all(t.path().join(d).join("sub")).unwrap();
            std::fs::write(t.path().join(d).join("sub/disk.img"), b"x").unwrap();
        }
        std::fs::write(t.path().join("lume_vm_file"), b"x").unwrap();
        assert_eq!(
            sweep_stale_pull_dirs(t.path(), Duration::from_secs(3600)),
            0,
            "written just now: still pulling"
        );
        assert_eq!(sweep_stale_pull_dirs(t.path(), Duration::ZERO), 2);
        assert!(!t.path().join("lume_vm_dead").exists());
        assert!(t.path().join("other").exists());
        assert!(t.path().join("lume_vm_file").exists());
    }

    #[test]
    fn cli_get_reads_the_status_lume_reports() {
        let stopped = r#"[{"name":"cua-box","status":"stopped","os":"macOS"}]"#;
        assert_eq!(
            parse_cli_get("cua-box", true, Some(0), stopped, "").unwrap(),
            Status::Stopped
        );
        let running = r#"{"name":"cua-box","status":"running"}"#;
        assert_eq!(
            parse_cli_get("cua-box", true, Some(0), running, "").unwrap(),
            Status::Running
        );
        let gone = "Error: Virtual machine not found: cua-box";
        assert!(matches!(
            parse_cli_get("cua-box", false, Some(1), gone, ""),
            Err(VmmError::NotFound(_))
        ));
        assert!(matches!(
            parse_cli_get("cua-box", false, Some(1), "", "permission denied"),
            Err(VmmError::Command { .. })
        ));
        assert!(parse_cli_get("cua-box", true, Some(0), "[]", "").is_err());
    }

    /// A VM stopped by a reboot, asked through the `lume` CLI (a stand-in
    /// script) because no `lume serve` runs.
    #[cfg(unix)]
    #[tokio::test]
    async fn cli_status_runs_lume_get() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("lume");
        std::fs::write(
            &bin,
            "#!/bin/sh\n[ \"$1 $2 $3 $4\" = 'get cua-box --format json' ] || exit 2\n\
             echo '[{\"name\":\"cua-box\",\"status\":\"stopped\"}]'\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            cli_status(&LumeCmd::plain(&bin), "cua-box").await.unwrap(),
            Status::Stopped
        );
    }

    #[test]
    fn base_names_match_the_python_runtime() {
        // hashlib.sha256(b"ghcr.io/trycua/macos-sequoia-cua:latest").hexdigest()[:12]
        assert_eq!(
            lume_pull_ref(
                "ghcr.io/trycua/macos:26@sha256:b03ff1154dc80a7dadef5397eaca395c1e6cb64093a29bd53ca5916127816a8e"
            ),
            "ghcr.io/trycua/macos:26"
        );
        assert_eq!(
            lume_pull_ref("ghcr.io/trycua/macos:26"),
            "ghcr.io/trycua/macos:26"
        );
        // A bare digest ref is not a tag: left alone.
        assert_eq!(
            lume_pull_ref("ghcr.io/trycua/macos@sha256:ab"),
            "ghcr.io/trycua/macos@sha256:ab"
        );
        // A moved tag names a different base.
        assert_ne!(
            base_vm_name("ghcr.io/trycua/macos:26@sha256:aa"),
            base_vm_name("ghcr.io/trycua/macos:26@sha256:bb")
        );
        let n = base_vm_name("ghcr.io/trycua/macos-sequoia-cua:latest");
        assert_eq!(n, "cua-base-3de9994dac39");
        assert_ne!(n, base_vm_name("ghcr.io/trycua/macos-tahoe-cua:latest"));
    }

    #[test]
    fn statuses_map() {
        assert_eq!(map_status("running"), Status::Running);
        assert_eq!(map_status("stopped"), Status::Stopped);
        assert_eq!(map_status("pulling"), Status::Provisioning);
    }

    fn base_vm() -> VmDetails {
        VmDetails {
            name: "clone".into(),
            os: "macOS".into(),
            cpu_count: 4,
            memory_size: 8 << 30,
            disk_size: serde_json::json!({"allocated": 1u64 << 30, "total": 150u64 << 30}),
            ..Default::default()
        }
    }

    #[test]
    fn clone_resources_follow_the_spec() {
        let img = ImageSource::Oci {
            reference: "ghcr.io/trycua/macos:26".into(),
        };
        let spec = StartSpec::new("clone", img.clone()).cpus(2).memory_mb(4096);
        let set = clone_resources(&base_vm(), &spec);
        assert_eq!(
            set,
            SetVm {
                cpu: Some(2),
                memory: Some("4096MB".into()),
                ..Default::default()
            }
        );

        // Already matching: nothing to set.
        let same = StartSpec::new("clone", img.clone()).cpus(4).memory_mb(8192);
        assert!(clone_resources(&base_vm(), &same).is_empty());

        // Disk grows only; a smaller request keeps the base size.
        let mut grow = same.clone();
        grow.disk_size_gb = Some(200);
        let set = clone_resources(&base_vm(), &grow);
        assert_eq!(set.disk_size.as_deref(), Some("200GB"));
        assert_eq!(set.no_backup, Some(true));
        let mut shrink = same;
        shrink.disk_size_gb = Some(64);
        assert!(clone_resources(&base_vm(), &shrink).is_empty());
    }

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A `GuestExec` that fails `fail_first` times, then succeeds, recording
    /// every script.
    struct FakeExec {
        fail_first: Mutex<u32>,
        scripts: Mutex<Vec<String>>,
    }

    impl FakeExec {
        fn new(fail_first: u32) -> Self {
            Self {
                fail_first: Mutex::new(fail_first),
                scripts: Mutex::default(),
            }
        }
    }

    #[async_trait]
    impl GuestExec for FakeExec {
        async fn exec(&self, req: crate::exec::ExecRequest) -> Result<crate::exec::ExecOutput> {
            self.scripts.lock().unwrap().push(req.script);
            let mut f = self.fail_first.lock().unwrap();
            if *f > 0 {
                *f -= 1;
                return Ok(crate::exec::ExecOutput {
                    exit_code: 255,
                    stderr: b"Connection refused".to_vec(),
                    ..Default::default()
                });
            }
            Ok(crate::exec::ExecOutput {
                exit_code: 0,
                stdout: b"cua-env-ok\n".to_vec(),
                ..Default::default()
            })
        }
    }

    fn decoded_parts(script: &str) -> Vec<String> {
        use base64::Engine as _;
        script
            .split("printf %s '")
            .skip(1)
            .map(|p| {
                let b = &p[..p.find('\'').unwrap()];
                String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b).unwrap())
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn macos_env_script_writes_shell_launchctl_and_agent_files() {
        let env: BTreeMap<String, String> = [
            ("FOO".to_string(), "bar baz".to_string()),
            ("Q".to_string(), "it's".to_string()),
        ]
        .into();
        let script = macos_env_script(&env).unwrap();
        // Values never appear in the clear (base64 inside the script).
        assert!(!script.contains("bar baz"));
        let parts = decoded_parts(&script);
        assert_eq!(parts.len(), 4, "env.sh, launchctl, unset, plist");
        assert!(parts[0].contains("export FOO='bar baz'\n"));
        assert!(parts[0].contains("export Q='it'\\''s'\n"));
        assert!(parts[1].contains("launchctl setenv FOO 'bar baz'\n"));
        assert_eq!(parts[2], "launchctl unsetenv FOO\nlaunchctl unsetenv Q\n");
        assert!(parts[3].contains("<string>ai.cua.env</string>"));
        assert!(parts[3].contains("<key>RunAtLoad</key><true/>"));
        assert!(script.contains(".zshenv") && script.contains(".profile"));
        assert!(script.ends_with("echo cua-env-ok"));
        // Empty env: a clearing script (unsets the previous variables).
        let clear = decoded_parts(&macos_env_script(&BTreeMap::new()).unwrap());
        assert!(!clear[0].contains("export"));
        assert!(clear[1].is_empty());
        // Bad names and control characters are refused.
        let bad: BTreeMap<String, String> = [("1X".to_string(), "v".to_string())].into();
        assert!(macos_env_script(&bad).is_err());
        let ctl: BTreeMap<String, String> = [("X".to_string(), "a\nb".to_string())].into();
        assert!(macos_env_script(&ctl).is_err());
    }

    #[tokio::test]
    async fn macos_env_retries_until_ssh_answers_and_fails_loudly() {
        let env: BTreeMap<String, String> = [("FOO".to_string(), "1".to_string())].into();
        let exec = FakeExec::new(2);
        apply_macos_env(
            &exec,
            "m",
            &env,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await
        .unwrap();
        assert_eq!(exec.scripts.lock().unwrap().len(), 3);
        let never = FakeExec::new(u32::MAX);
        let err = apply_macos_env(
            &never,
            "m",
            &env,
            Duration::from_millis(50),
            Duration::from_millis(10),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, VmmError::Timeout { .. }), "{err}");
        assert!(err.to_string().contains("Connection refused"));
        assert!(never.scripts.lock().unwrap().len() <= 7, "bounded retries");
    }

    /// A fake `lume serve`: answers GET /lume/vms/<vm> with `status` and
    /// records every request line.
    async fn fake_lume(status: &'static str) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let line = req.lines().next().unwrap_or("").to_string();
                    log.lock().unwrap().push(line.clone());
                    let body = if line.starts_with("GET ") {
                        format!(r#"{{"name":"m","os":"macOS","status":"{status}"}}"#)
                    } else {
                        "{}".to_string()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (url, seen)
    }

    /// A fake `lume serve` with VMs: GET `/lume/vms[/:name]` answers from
    /// `vms` (a missing VM is Lume's JSON not-found error), a POST to a path
    /// ending in one of `missing` gets the router's bare "Not found" (an
    /// older Lume), every other request 200 `{}`; a stop or suspend marks
    /// the VM stopped and a run running. Records "METHOD PATH BODY" for
    /// each request.
    async fn fake_lume_vms(
        vms: Vec<serde_json::Value>,
        missing: &'static [&'static str],
    ) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        super::client::tests::temp_home();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let vms = Arc::new(std::sync::Mutex::new(vms));
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (log, vms) = (log.clone(), vms.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16384];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let line = req.lines().next().unwrap_or("").to_string();
                    let mut parts = line.split_whitespace();
                    let (method, path) = (
                        parts.next().unwrap_or("").to_string(),
                        parts.next().unwrap_or("").to_string(),
                    );
                    let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                    log.lock()
                        .unwrap()
                        .push(format!("{method} {path} {body}").trim().to_string());
                    let (code, out) = {
                        let mut vms = vms.lock().unwrap();
                        if method == "GET" && path == "/lume/vms" {
                            (200, serde_json::Value::Array(vms.to_vec()).to_string())
                        } else if method == "GET" && path.starts_with("/lume/vms/") {
                            let name = &path["/lume/vms/".len()..];
                            match vms.iter().find(|v| v["name"] == name) {
                                Some(v) => (200, v.to_string()),
                                None => (
                                    400,
                                    format!(r#"{{"message":"Virtual machine not found: {name}"}}"#),
                                ),
                            }
                        } else if method == "POST" && missing.iter().any(|m| path.ends_with(m)) {
                            (404, "Not found".to_string())
                        } else {
                            let set = |vms: &mut Vec<serde_json::Value>, verb: &str, to: &str| {
                                if let Some(name) = path
                                    .strip_prefix("/lume/vms/")
                                    .and_then(|p| p.strip_suffix(verb))
                                    && let Some(v) = vms.iter_mut().find(|v| v["name"] == name)
                                {
                                    v["status"] = to.into();
                                }
                            };
                            set(&mut vms, "/stop", "stopped");
                            set(&mut vms, "/suspend", "stopped");
                            set(&mut vms, "/run", "running");
                            (200, "{}".to_string())
                        }
                    };

                    let resp = format!(
                        "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
                        out.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (url, seen)
    }

    fn mac_vm(name: &str, status: &str, saved: bool, mac: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name, "os": "macOS", "status": status, "savedState": saved,
            "macAddress": mac, "cpuCount": 4, "memorySize": 8u64 << 30,
            "diskSize": {"total": 160u64 << 30, "allocated": 1}
        })
    }

    fn rt_at(url: String, root: &std::path::Path) -> LumeRuntime {
        LumeRuntime::new(LumeConfig {
            url,
            spawn_serve: false,
            root: root.to_path_buf(),
            ..LumeConfig::default()
        })
    }

    fn posts(seen: &Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("POST") || l.starts_with("DELETE"))
            .cloned()
            .collect()
    }

    /// Suspending a running macOS Space saves its memory (Lume's suspend,
    /// which also stops it); a Lume without saved states gets a stop, and
    /// a stopped VM nothing.
    #[tokio::test]
    async fn suspend_saves_a_running_vm_and_falls_back_to_a_stop() {
        let dir = tempfile::tempdir().unwrap();
        let (url, seen) = fake_lume_vms(
            vec![mac_vm("m", "running", false, "aa:00:00:00:00:01")],
            &[],
        )
        .await;
        Runtime::suspend(&rt_at(url, dir.path()), "m")
            .await
            .unwrap();
        assert_eq!(posts(&seen), ["POST /lume/vms/m/suspend {}"]);
        let (url, seen) = fake_lume_vms(
            vec![mac_vm("m", "running", false, "aa:00:00:00:00:01")],
            &["/suspend"],
        )
        .await;
        Runtime::suspend(&rt_at(url, dir.path()), "m")
            .await
            .unwrap();
        assert_eq!(
            posts(&seen),
            ["POST /lume/vms/m/suspend {}", "POST /lume/vms/m/stop {}"]
        );
    }

    /// A checkpoint of a running macOS VM keeps its memory: save, clone
    /// with the state, resume the source from its own state. A fork of a
    /// checkpoint with a state carries the state.
    #[tokio::test]
    async fn checkpoint_of_a_running_vm_saves_its_memory_and_forks_carry_it() {
        let dir = tempfile::tempdir().unwrap();
        let (url, seen) = fake_lume_vms(
            vec![
                mac_vm("m", "running", false, "aa:00:00:00:00:01"),
                mac_vm("ck", "stopped", true, "aa:00:00:00:00:01"),
            ],
            &[],
        )
        .await;
        let rt = rt_at(url, dir.path());
        // "ck" exists in the fake, so checkpoint into a fresh name.
        Runtime::checkpoint(&rt, "m", "ck2").await.unwrap();
        let p = posts(&seen);
        assert_eq!(p[0], "POST /lume/vms/m/suspend {}");
        assert!(p[1].starts_with("POST /lume/vms/clone"), "{p:?}");
        assert!(p[1].contains(r#""withSavedState":true"#), "{p:?}");
        assert!(p[1].contains(r#""newName":"ck2""#), "{p:?}");
        assert!(p[2].starts_with("POST /lume/vms/m/run"), "{p:?}");
        assert_eq!(p.len(), 3, "{p:?}");
        seen.lock().unwrap().clear();
        Runtime::fork(&rt, "ck", "f1").await.unwrap();
        let p = posts(&seen);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains(r#""withSavedState":true"#), "{p:?}");
    }

    /// An older Lume (no saved states): the checkpoint stops, clones and
    /// restarts the VM as before.
    #[tokio::test]
    async fn checkpoint_falls_back_to_a_stopped_clone_on_an_older_lume() {
        let dir = tempfile::tempdir().unwrap();
        let (url, seen) = fake_lume_vms(
            vec![mac_vm("m", "running", false, "aa:00:00:00:00:01")],
            &["/suspend"],
        )
        .await;
        Runtime::checkpoint(&rt_at(url, dir.path()), "m", "ck2")
            .await
            .unwrap();
        let p = posts(&seen);
        assert_eq!(p[0], "POST /lume/vms/m/suspend {}");
        assert_eq!(p[1], "POST /lume/vms/m/stop {}");
        assert!(p[2].starts_with("POST /lume/vms/clone"), "{p:?}");
        assert!(!p[2].contains("withSavedState"), "{p:?}");
        assert!(p[3].starts_with("POST /lume/vms/m/run"), "{p:?}");
    }

    /// A start resumes a warm state only when one fits: saved, stopped,
    /// the spec's resources, and a MAC address no running VM has (each
    /// resume of a state has the state's address).
    #[tokio::test]
    async fn warm_states_are_used_only_when_they_fit_and_their_address_is_free() {
        let dir = tempfile::tempdir().unwrap();
        let [a, b] = warm_vm_names("cua-base-0123456789ab");
        assert_eq!(a, "cua-warm-0123456789ab");
        assert_eq!(b, "cua-warm-0123456789ab-2");
        let spec = StartSpec::new("s1", ImageSource::oci("ghcr.io/trycua/macos:26"))
            .cpus(4)
            .memory_mb(8192);
        let (url, _) = fake_lume_vms(
            vec![
                mac_vm(&a, "stopped", true, "aa:00:00:00:00:0a"),
                mac_vm(&b, "stopped", true, "aa:00:00:00:00:0b"),
            ],
            &[],
        )
        .await;
        let rt = rt_at(url, dir.path());
        let base = "cua-base-0123456789ab";
        assert_eq!(rt.usable_warm(base, &spec).await.unwrap(), Some(a.clone()));
        // Other resources: no warm state fits.
        let big = spec.clone().memory_mb(16384);
        assert_eq!(rt.usable_warm(base, &big).await.unwrap(), None);
        // A GPU: never warm.
        let mut gpu = spec.clone();
        gpu.gpu = Some(crate::gpu::PARAVIRTUAL.into());
        assert_eq!(rt.usable_warm(base, &gpu).await.unwrap(), None);
        // The first state's address is in use: the second one.
        let (url, _) = fake_lume_vms(
            vec![
                mac_vm(&a, "stopped", true, "aa:00:00:00:00:0a"),
                mac_vm(&b, "stopped", true, "aa:00:00:00:00:0b"),
                mac_vm("s0", "running", false, "AA:00:00:00:00:0A"),
            ],
            &[],
        )
        .await;
        let rt = rt_at(url, dir.path());
        assert_eq!(rt.usable_warm(base, &spec).await.unwrap(), Some(b.clone()));
        // Both in use, or no saved state: none.
        let (url, _) = fake_lume_vms(
            vec![
                mac_vm(&a, "stopped", true, "aa:00:00:00:00:0a"),
                mac_vm(&b, "stopped", false, "aa:00:00:00:00:0b"),
                mac_vm("s0", "running", false, "aa:00:00:00:00:0a"),
            ],
            &[],
        )
        .await;
        assert_eq!(
            rt_at(url, dir.path())
                .usable_warm(base, &spec)
                .await
                .unwrap(),
            None
        );
    }

    /// Claiming a running pool member renames the running VM in Lume and
    /// moves cua's own state for it; a Lume without renames says so.
    #[tokio::test]
    async fn claim_renames_the_running_member_and_moves_its_state() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pool-1")).unwrap();
        std::fs::write(dir.path().join("pool-1").join("ports.json"), b"[3211]").unwrap();
        let (url, seen) = fake_lume_vms(
            vec![
                mac_vm("pool-1", "running", false, "aa:00:00:00:00:01"),
                mac_vm("space-1", "running", false, "aa:00:00:00:00:01"),
            ],
            &[],
        )
        .await;
        let rt = rt_at(url, dir.path());
        // The fake already has "space-1": a name in use is refused.
        let err = Runtime::claim(&rt, "pool-1", "space-1", &ClaimIdentity::default())
            .await
            .unwrap_err();
        assert!(matches!(err, VmmError::AlreadyExists(_)), "{err}");
        let (url, seen2) = fake_lume_vms(
            vec![mac_vm("pool-1", "running", false, "aa:00:00:00:00:01")],
            &[],
        )
        .await;
        let rt = rt_at(url, dir.path());
        // The fake never shows the new name, so the final lookup fails
        // after the rename; what matters is the requests and the move.
        let _ = Runtime::claim(&rt, "pool-1", "space-2", &ClaimIdentity::default()).await;
        let p = posts(&seen2);
        assert_eq!(p, [r#"POST /lume/vms/pool-1/rename {"newName":"space-2"}"#]);
        assert!(dir.path().join("space-2").join("ports.json").exists());
        assert!(!dir.path().join("pool-1").exists());
        drop(seen);
        let (url, _) = fake_lume_vms(
            vec![mac_vm("pool-1", "running", false, "aa:00:00:00:00:01")],
            &["/rename"],
        )
        .await;
        let err = Runtime::claim(
            &rt_at(url, dir.path()),
            "pool-1",
            "space-3",
            &ClaimIdentity::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, VmmError::Unsupported { .. }), "{err}");
    }

    #[test]
    fn macos_hostnames_are_valid_labels() {
        assert_eq!(macos_hostname("my_space.1"), "my-space-1");
        assert_eq!(macos_hostname("--"), "space");
        assert_eq!(macos_hostname(&"a".repeat(80)).len(), 63);
    }

    /// Suspending a stopped VM does nothing, after making sure `lume
    /// serve` answers (it would start it).
    #[tokio::test]
    async fn suspend_is_a_no_op_when_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let (url, seen) = fake_lume("stopped").await;
        Runtime::suspend(&rt_at(url, dir.path()), "m")
            .await
            .unwrap();
        let seen = seen.lock().unwrap();
        assert!(!seen.iter().any(|l| l.starts_with("POST")), "{seen:?}");
        assert_eq!(
            seen.first().map(String::as_str),
            Some("GET /lume/host/status HTTP/1.1")
        );
    }

    /// Deleting a macOS Space when `lume serve` is gone starts it (here,
    /// with starting it off, says how) instead of failing on a raw
    /// connection error and leaving the Space behind.
    #[tokio::test]
    async fn delete_makes_sure_lume_serve_answers_first() {
        let dir = tempfile::tempdir().unwrap();
        let (url, seen) = fake_lume("stopped").await;
        let rt = LumeRuntime::new(LumeConfig {
            url,
            spawn_serve: false,
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        Runtime::delete(&rt, "m").await.unwrap();
        let lines = seen.lock().unwrap().clone();
        assert_eq!(
            lines.first().map(String::as_str),
            Some("GET /lume/host/status HTTP/1.1")
        );
        assert!(
            lines.iter().any(|l| l.starts_with("DELETE /lume/vms/m")),
            "{lines:?}"
        );

        // (Elsewhere Lume is refused before: it runs on macOS only.)
        if !cfg!(target_os = "macos") {
            return;
        }
        let gone = LumeRuntime::new(LumeConfig {
            url: "http://127.0.0.1:9".into(),
            spawn_serve: false,
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        let e = Runtime::delete(&gone, "m").await.unwrap_err();
        assert!(e.to_string().contains("start it with `lume serve`"), "{e}");
    }

    /// A fork or checkpoint after `lume serve` exited (an update swapped
    /// its binary) goes through the same check as delete: Lume answers
    /// first (or is started), not a raw "not reachable".
    #[tokio::test]
    async fn fork_and_checkpoint_make_sure_lume_serve_answers_first() {
        let dir = tempfile::tempdir().unwrap();
        for op in ["fork", "checkpoint"] {
            let (url, seen) = fake_lume("stopped").await;
            let rt = LumeRuntime::new(LumeConfig {
                url,
                spawn_serve: false,
                root: dir.path().to_path_buf(),
                ..LumeConfig::default()
            });
            let _ = match op {
                "fork" => Runtime::fork(&rt, "m", "n").await,
                _ => Runtime::checkpoint(&rt, "m", "c").await.map(|_| ()),
            };
            let lines = seen.lock().unwrap().clone();
            assert_eq!(
                lines.first().map(String::as_str),
                Some("GET /lume/host/status HTTP/1.1"),
                "{op}: {lines:?}"
            );
        }
    }

    #[test]
    fn published_ports_survive_a_new_process() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = || LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        };
        assert!(LumeRuntime::new(cfg()).load_ports("m").is_empty());
        LumeRuntime::new(cfg()).save_ports("m", &[3211, 8080]);
        // A fresh runtime (another CLI process) reads them back.
        assert_eq!(LumeRuntime::new(cfg()).load_ports("m"), vec![3211, 8080]);
    }

    #[test]
    fn a_running_protected_vm_without_its_gateway_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        let vm = |os: &str, status: &str| -> VmDetails {
            serde_json::from_value(serde_json::json!({
                "name": "m", "os": os, "status": status, "cpuCount": 2,
                "memorySize": 4u64 << 30, "display": "1024x768",
                "diskSize": {"allocated": 1u64, "total": 2u64}
            }))
            .unwrap()
        };
        let mac = vm("macOS", "running");
        // No network settings kept: not behind a gateway at all.
        assert!(!rt.gateway_lost("m", &mac));
        rt.keep_network("m", &crate::network::NetworkSettings::internet())
            .unwrap();
        // Settings but no gateway ever started: nothing to mend.
        assert!(!rt.gateway_lost("m", &mac));
        let gw = LumeGateway {
            network: "gateway:/x".into(),
            ports: [(22, 1), (3211, 2)].into(),
        };
        std::fs::write(rt.gateway_file("m"), serde_json::to_vec(&gw).unwrap()).unwrap();
        // A gateway that ran, whose process is gone (no pid file): lost.
        #[cfg(unix)]
        assert!(rt.gateway_lost("m", &mac));
        // Only a running macOS VM is concerned.
        assert!(!rt.gateway_lost("m", &vm("macOS", "stopped")));
        assert!(!rt.gateway_lost("m", &vm("linux", "running")));
    }

    #[test]
    fn stamps_are_written_the_way_lume_writes_them() {
        assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp(1_794_024_435), "2026-11-07T04:07:15Z");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn the_vm_limit_is_read_from_lume_serves_own_log() {
        let line = |stamp: &str, tail: &str| {
            format!(
                "[{stamp}] ERROR: Failed in VM.run error=The number of virtual machines exceeds the limit. The maximum supported number of active virtual machines has been reached. {tail}\n"
            )
        };
        let since = "2026-11-07T04:07:10Z";
        let log = line(
            "2026-11-07T04:07:15Z",
            "name=child-safari errorType=NSError",
        );
        assert!(log_says_vm_limit(log.as_bytes(), "child-safari", since));
        // Lume orders the fields as it likes: the name may come last.
        let last = line("2026-11-07T04:07:15Z", "errorType=NSError name=hold-one");
        assert!(log_says_vm_limit(last.as_bytes(), "hold-one", since));
        assert!(!log_says_vm_limit(last.as_bytes(), "hold", since));
        assert!(!log_says_vm_limit(log.as_bytes(), "other-vm", since));
        // An older refusal (an earlier boot of the same name) is not this one.
        let old = line("2026-11-07T03:00:00Z", "name=child-safari");
        assert!(!log_says_vm_limit(old.as_bytes(), "child-safari", since));
        assert!(!log_says_vm_limit(
            b"[2026-11-07T04:07:15Z] INFO: all fine name=child-safari",
            "child-safari",
            since
        ));
        let msg = macos_limit_unlisted_message();
        assert!(msg.contains("Apple allows 2"), "{msg}");
        assert!(!msg.contains("stopped while booting"), "{msg}");
    }

    #[test]
    fn the_serve_log_is_read_wherever_the_refusal_sits_among_huge_lines() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        // Lume logs whole API responses (single lines of many MB), and more
        // than one `lume serve` writes into the file at its own offset, so
        // a refusal can sit far before the end of a file that is longer.
        let huge = format!(
            "[2026-11-07T04:07:20Z] INFO: response {}\n",
            "x".repeat(3 * 1024 * 1024)
        );
        let refusal = "[2026-11-07T04:07:15Z] ERROR: Failed in VM.run error=The number of virtual machines exceeds the limit. name=w errorType=NSError\n";
        std::fs::write(
            dir.path().join("serve.log"),
            format!("{refusal}{huge}{huge}{huge}"),
        )
        .unwrap();
        assert!(rt.serve_log_says_vm_limit("w", "2026-11-07T04:07:10Z"));
        assert!(!rt.serve_log_says_vm_limit("w", "2026-11-07T04:07:16Z"));
        assert!(!rt.serve_log_says_vm_limit("z", "2026-11-07T04:07:10Z"));
    }

    #[test]
    fn a_gateway_forwards_the_ports_a_start_recorded_so_the_probe_port_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        // Nothing recorded yet: the gateway would forward SSH alone, and
        // cua-spacesd's port would have no route (the bug).
        assert_eq!(rt.gateway_guest_ports("m"), vec![22]);
        rt.save_ports("m", &[3211]);
        assert_eq!(rt.gateway_guest_ports("m"), vec![22, 3211]);
        // The gateway's forwards decide the endpoints the probes dial.
        let gw = LumeGateway {
            network: "gateway:/x".into(),
            ports: [(22, 50022), (3211, 50211)].into(),
        };
        std::fs::create_dir_all(dir.path().join("m")).unwrap();
        std::fs::write(rt.gateway_file("m"), serde_json::to_vec(&gw).unwrap()).unwrap();
        let ep = rt.with_gateway(
            "m",
            Endpoints {
                host: "192.168.64.5".into(),
                ports: [(22, 22), (3211, 3211)].into(),
                ..Default::default()
            },
        );
        assert_eq!(ep.addr(3211).as_deref(), Some("127.0.0.1:50211"));
    }

    #[tokio::test]
    async fn macos_env_sync_skips_clean_guests_and_clears_after_env() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        let exec = FakeExec::new(0);
        // Token-only env: nothing to do, no SSH round trip.
        let token: BTreeMap<String, String> =
            [(crate::cloudinit::ENV_TOKEN_VAR.to_string(), "t".to_string())].into();
        rt.sync_macos_env("m", &token, Some(&exec)).await.unwrap();
        assert!(exec.scripts.lock().unwrap().is_empty());
        // Workload env: applied (the token stays out of it), marker written.
        let mut env = token.clone();
        env.insert("FOO".into(), "1".into());
        rt.sync_macos_env("m", &env, Some(&exec)).await.unwrap();
        {
            let scripts = exec.scripts.lock().unwrap();
            assert_eq!(scripts.len(), 1);
            let parts = decoded_parts(&scripts[0]);
            assert!(parts[0].contains("export FOO='1'"));
            assert!(!parts[0].contains("CUA_ENV_TOKEN"));
        }
        assert!(dir.path().join("m").join(MACOS_ENV_MARKER).exists());
        // A later start without env clears it once.
        rt.sync_macos_env("m", &token, Some(&exec)).await.unwrap();
        assert_eq!(exec.scripts.lock().unwrap().len(), 2);
        assert!(!dir.path().join("m").join(MACOS_ENV_MARKER).exists());
        rt.sync_macos_env("m", &token, Some(&exec)).await.unwrap();
        assert_eq!(exec.scripts.lock().unwrap().len(), 2);
        // No lume binary: a clear error, never a silent drop.
        let err = rt.sync_macos_env("m", &env, None).await.unwrap_err();
        assert!(matches!(err, VmmError::Missing { .. }), "{err}");
    }

    #[test]
    fn cli_pull_args_split_like_the_api() {
        assert_eq!(
            cli_pull_args("ghcr.io/me/macos-private:26", "cua-base-x"),
            vec![
                "pull",
                "macos-private:26",
                "cua-base-x",
                "--registry",
                "ghcr.io",
                "--organization",
                "me"
            ]
        );
        assert_eq!(
            cli_pull_args("me/img:1", "b"),
            vec!["pull", "img:1", "b", "--organization", "me"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_pull_passes_credentials_in_the_environment_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let bin = dir.path().join("lume");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" \"$GITHUB_USERNAME\" \"$GITHUB_TOKEN\" > {}\n\
                 [ \"$2\" = fail:1 ] && {{ echo \"denied for $GITHUB_TOKEN\" >&2; exit 3; }}\nexit 0\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let creds = RegistryCredentials::new("me", "s3cret");
        cli_pull(
            &LumeCmd::plain(&bin),
            "ghcr.io/me/img:1",
            "base",
            &creds,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        let seen = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = seen.lines().collect();
        assert_eq!(
            lines,
            vec![
                "pull img:1 base --registry ghcr.io --organization me",
                "me",
                "s3cret"
            ]
        );
        let err = cli_pull(
            &LumeCmd::plain(&bin),
            "ghcr.io/me/fail:1",
            "base",
            &creds,
            Duration::from_secs(20),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("denied for ***"), "{err}");
        assert!(!err.contains("s3cret"));
    }

    #[test]
    fn gpu_acceleration_is_offered_for_a_macos_image_whatever_os_says() {
        let oci = ImageSource::Oci {
            reference: "ghcr.io/trycua/macos:26".into(),
        };
        // The Spaces app's create leaves `os` at its default (Linux).
        let mut spec = StartSpec::new("gpu-mac", oci.clone());
        assert_eq!(spec.os, GuestOs::Linux);
        spec.gpu = Some(crate::gpu::PARAVIRTUAL.into());
        crate::types::reject_gpu(BackendKind::Lume, &spec, gpu_offered(&spec))
            .expect("a macOS image on Lume takes GPU acceleration");
        spec.os = GuestOs::Macos;
        assert_eq!(gpu_offered(&spec), [crate::gpu::PARAVIRTUAL]);
        // A Linux disk VM has none, and another GPU kind is still refused.
        let disk = StartSpec::new(
            "gpu-linux",
            ImageSource::Disk {
                path: "/tmp/linux.qcow2".into(),
            },
        );
        assert!(gpu_offered(&disk).is_empty());
        spec.gpu = Some(crate::gpu::VIRGL.into());
        assert!(crate::types::reject_gpu(BackendKind::Lume, &spec, gpu_offered(&spec)).is_err());
    }

    #[test]
    fn an_oci_image_on_lume_is_a_macos_guest_whatever_os_says() {
        let oci = StartSpec::new(
            "m",
            ImageSource::Oci {
                reference: "ghcr.io/trycua/macos:26".into(),
            },
        );
        assert_eq!(oci.os, GuestOs::Linux);
        assert!(macos_guest(&oci));
        let disk = StartSpec::new(
            "l",
            ImageSource::Disk {
                path: "/tmp/linux.qcow2".into(),
            },
        );
        assert!(!macos_guest(&disk));
    }

    #[test]
    fn a_macos_guest_gets_its_mounts_as_named_shares_next_to_the_setup_share() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().join("lume"),
            ..LumeConfig::default()
        });
        let job = dir.path().join("cj-0123-1");
        let data = dir.path().join("run-data");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let mut spec = StartSpec::new("cj-0123-1", ImageSource::Existing);
        spec.os = GuestOs::Macos;
        spec.mounts = vec![
            crate::Mount {
                host: job.clone(),
                guest: macos_share_path(&job),
                read_only: false,
            },
            crate::Mount {
                host: data.clone(),
                guest: macos_share_path(&data),
                read_only: true,
            },
        ];
        assert_eq!(spec.mounts[0].guest, "/Volumes/My Shared Files/cj-0123-1");
        let run = rt.macos_run("cj-0123-1", Some(&spec)).unwrap();
        let shares: Vec<(String, bool)> = run
            .shared_directories
            .iter()
            .map(|d| (d.host_path.clone(), d.read_only))
            .collect();
        assert_eq!(shares.len(), 3, "{shares:?}");
        assert!(shares[0].0.ends_with("/setup"));
        assert_eq!(shares[1], (job.display().to_string(), false));
        assert_eq!(shares[2], (data.display().to_string(), true));
        // A guest path Lume does not share it at, a repeated name, and a
        // missing directory are refused before anything starts.
        let mut wrong = spec.clone();
        wrong.mounts[0].guest = "/cua/job".into();
        let e = rt.macos_run("x", Some(&wrong)).unwrap_err().to_string();
        assert!(e.contains("/Volumes/My Shared Files/cj-0123-1"), "{e}");
        let setup = dir.path().join("setup");
        std::fs::create_dir_all(&setup).unwrap();
        let mut clash = spec.clone();
        clash.mounts[1] = crate::Mount {
            host: setup.clone(),
            guest: macos_share_path(&setup),
            read_only: false,
        };
        let e = rt.macos_run("x", Some(&clash)).unwrap_err().to_string();
        assert!(e.contains("two shared directories named \"setup\""), "{e}");
        let mut gone = spec.clone();
        gone.mounts[0].host = dir.path().join("missing");
        gone.mounts[0].guest = macos_share_path(&gone.mounts[0].host);
        assert!(rt.macos_run("x", Some(&gone)).is_err());
    }

    #[test]
    fn macos_setup_share_carries_the_env_token_only_when_given() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        let mut spec = StartSpec::new("cua-e2e-mac", ImageSource::Existing);
        spec.os = GuestOs::Macos;
        // No token: the share is there (a saved state needs the same
        // shares on every run), and empty.
        let run = rt.macos_run("cua-e2e-mac", Some(&spec)).unwrap();
        assert_eq!(run.shared_directories.len(), 1);
        let share = std::path::PathBuf::from(&run.shared_directories[0].host_path);
        assert_eq!(share.file_name().unwrap(), "setup");
        assert_eq!(std::fs::read_dir(&share).unwrap().count(), 0);

        spec.env
            .insert(crate::cloudinit::ENV_TOKEN_VAR.into(), "tok-1".into());
        let run = rt.macos_run("cua-e2e-mac", Some(&spec)).unwrap();
        assert_eq!(run.shared_directories.len(), 1);
        assert!(run.shared_directories[0].read_only);
        let token = share.join(MACOS_SETUP_TOKEN_FILE);
        assert_eq!(std::fs::read_to_string(&token).unwrap(), "tok-1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&token).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&share).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let body = serde_json::to_value(&run).unwrap();
        assert_eq!(body["sharedDirectories"][0]["readOnly"], true);

        // A restart without a spec reuses the share and its token.
        assert_eq!(
            rt.macos_run("cua-e2e-mac", None)
                .unwrap()
                .shared_directories
                .len(),
            1
        );
        assert!(token.exists());
        // A later start without a token never shares the stale one.
        spec.env.clear();
        assert_eq!(
            rt.macos_run("cua-e2e-mac", Some(&spec))
                .unwrap()
                .shared_directories
                .len(),
            1
        );
        assert!(!token.exists());
    }

    #[test]
    fn macos_endpoints_default_to_lume_credentials() {
        let vm = VmDetails {
            name: "m".into(),
            os: "macOS".into(),
            status: "running".into(),
            ip_address: Some("192.168.64.9".into()),
            vnc_url: Some("vnc://:pw@127.0.0.1:5905".into()),
            ..Default::default()
        };
        let ep = LumeRuntime::endpoints_from(&vm, None, &[8080]);
        assert_eq!(ep.host, "192.168.64.9");
        assert_eq!(ep.addr(8080).as_deref(), Some("192.168.64.9:8080"));
        assert_eq!(ep.ssh.as_ref().unwrap().user, "lume");
        assert_eq!(ep.vnc.as_ref().unwrap().port, 5905);
        assert_eq!(ep.vnc.as_ref().unwrap().password.as_deref(), Some("pw"));
    }

    #[test]
    fn forwarded_ports_are_dialed_on_loopback_once_every_one_is_there() {
        let vm: VmDetails = serde_json::from_value(serde_json::json!({
            "name": "m", "os": "macOS", "status": "running",
            "ipAddress": "192.168.64.9",
            "vsock": true, "forwards": { "3211": 50123, "8080": 50124 }
        }))
        .unwrap();
        let ep = LumeRuntime::endpoints_from(&vm, None, &[3211, 8080]);
        assert_eq!(ep.host, VSOCK_HOST);
        assert_eq!(ep.addr(3211).as_deref(), Some("127.0.0.1:50123"));
        assert_eq!(ep.host_port(22), None);
        // SSH stays on the VM's address.
        assert_eq!(ep.ssh.as_ref().unwrap().host, "192.168.64.9");
        // A port Lume does not forward: the VM's address for all of them.
        let ep = LumeRuntime::endpoints_from(&vm, None, &[3211, 9000]);
        assert_eq!(ep.host, "192.168.64.9");
        assert_eq!(ep.host_port(9000), Some(9000));
    }

    #[test]
    fn forwards_count_only_once_the_agent_answers() {
        // Lume reports the forwards only then; before, `vsock` alone.
        let vm: VmDetails = serde_json::from_value(serde_json::json!({
            "name": "m", "status": "running", "ipAddress": "192.168.64.9", "vsock": true
        }))
        .unwrap();
        assert!(vm.vsock && vm.vsock_ports(&[3211]).is_none());
        // An older Lume reports neither.
        let old: VmDetails =
            serde_json::from_value(serde_json::json!({ "name": "m", "status": "running" }))
                .unwrap();
        assert!(!old.vsock && old.forwards.is_empty());
    }

    #[test]
    fn a_macos_run_forwards_its_published_ports() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        let spec = StartSpec::new("m", ImageSource::Existing)
            .os(GuestOs::Macos)
            .port(3211);
        let run = rt.macos_run("m", Some(&spec)).unwrap();
        assert_eq!(run.forwards, vec![3211]);
        assert_eq!(
            serde_json::to_value(&run).unwrap()["forwards"],
            serde_json::json!([3211])
        );
        // A restart forwards what the start published.
        rt.save_ports("m", &[3211]);
        assert_eq!(rt.macos_run("m", None).unwrap().forwards, vec![3211]);
        // Nothing published: the field is left out (an older Lume never sees it).
        let bare = StartSpec::new("n", ImageSource::Existing).os(GuestOs::Macos);
        let run = rt.macos_run("n", Some(&bare)).unwrap();
        assert!(
            serde_json::to_value(&run)
                .unwrap()
                .get("forwards")
                .is_none()
        );
    }
}
