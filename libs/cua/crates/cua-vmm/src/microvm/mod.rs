//! microVM backend: Firecracker on Linux/KVM, Virtualization.framework on
//! macOS ([`vz`]), booting OCI rootfs images. What follows describes the
//! Linux host; [`vz`] says how a Mac differs.
//!
//! Each Space is one `firecracker` process inside its own user + network
//! namespace created by `pasta` (passt): unprivileged, with NAT egress and
//! guest ports forwarded to host loopback. Every VM sees the same guest
//! address, so a snapshot restores into any namespace.
//!
//! Disks: the image's rootfs is one read-only ext4 per image digest,
//! attached over virtio-pmem and mounted with DAX, so every VM of that
//! image reads the host's page cache (a browser binary is in memory once
//! per host, not once per VM); `CUA_MICROVM_BASE=squashfs` uses a
//! compressed squashfs block device instead. A writable overlay over it
//! has the VM's own sparse ext4 scratch disk as its upper layer. The init
//! (`cua-microvm-init`, from a tiny initramfs) sets that up and runs the
//! image's ENTRYPOINT/CMD with its environment, as a container would.
//!
//! Snapshots: `checkpoint` pauses the VM, writes its memory and device
//! state (to tmpfs, then moved to disk) and copies its scratch disk, and
//! resumes it. `fork` makes a new VM that restores from such a snapshot
//! (or a copy of a stopped VM's disk); forks of one snapshot map its memory
//! file privately, sharing its pages.
//!
//! Warm starts: the first boot of a configuration with a readiness probe
//! (image, CPUs, memory, command, environment) is snapshotted once its
//! probes pass, before it gets an identity. Later starts of the same
//! configuration restore that snapshot and then receive their own identity
//! over vsock (Spaces token, hostname, entropy, clock), which is how a
//! Space comes up in a fraction of its boot time. `CUA_MICROVM_WARM=0`
//! turns this off. The snapshot is taken once the guest's CPUs have gone
//! quiet, so restored VMs do not finish the desktop's start-up themselves.
//!
//! Warm pools: a pool member ([`StartSpec::pool_member`]) is restored from
//! the golden snapshot like a warm start and then paused: it costs no CPU
//! and little memory (the snapshot's pages are shared through the page
//! cache). [`Runtime::claim`] renames it (its directory moves with the
//! running VMM), resumes it and sends its identity over vsock. Every
//! restore, fork and claim gets a new token (or none), hostname, machine
//! id, clock and entropy; the guest address is the same in every VM, each
//! in its own network namespace, so there is no lease to renew.
//!
//! Layout under `~/.cua/vmm/microvm`: `bases/<digest>/rootfs.{ext4,sqfs}`,
//! `kernel/`, `initrd/<init-hash>.cpio`, `templates/scratch-<n>g.ext4`,
//! `golden/<key>/`, `checkpoints/<name>/` and `instances/<name>/`.
// Off Linux/macOS only the parts the rest of cua names are used.
#![cfg_attr(not(unix), allow(dead_code))]

pub mod agent;
pub mod assets;
pub mod fcapi;
pub mod rootfs;
pub mod vz;

/// Firecracker runs on Linux only; the rest of cua builds this module on
/// every OS (the daemon names the runtime everywhere), so the few calls into
/// unix-only APIs fail with this on other systems.
#[cfg(not(unix))]
pub(crate) fn unix_only(what: &str) -> VmmError {
    VmmError::other(format!("{what}: Firecracker microVMs run on Linux only"))
}

/// A unix socket stream (the Firecracker API and the guest's vsock).
#[cfg(unix)]
pub(crate) type UnixSock = tokio::net::UnixStream;
#[cfg(not(unix))]
pub(crate) type UnixSock = tokio::io::DuplexStream;

/// Connects to the unix socket at `path`.
pub(crate) async fn connect_unix(path: &Path) -> std::io::Result<UnixSock> {
    #[cfg(unix)]
    {
        tokio::net::UnixStream::connect(path).await
    }
    #[cfg(not(unix))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "{}: unix sockets (Firecracker microVMs run on Linux only)",
                path.display()
            ),
        ))
    }
}

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_microvm_init::{BootConfig, Identity, Request};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::error::{Result, VmmError};
use crate::exec::GuestExec;
use crate::host;
use crate::runtime::{RootfsResolver, Runtime};
use crate::types::*;

pub use agent::{Agent, VsockExec};
pub use assets::MicroVmReport;
pub use fcapi::FcApi;
pub use vz::VzApi;

/// The monitor a microVM runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vmm {
    /// Firecracker (Linux/KVM).
    Firecracker,
    /// `cua-vz`, Virtualization.framework (macOS on Apple silicon).
    Vz,
}

impl Vmm {
    /// This host's.
    pub fn host() -> Self {
        if cfg!(target_os = "macos") {
            Vmm::Vz
        } else {
            Vmm::Firecracker
        }
    }

    /// The backend its VMs are listed under.
    pub fn backend(self) -> BackendKind {
        match self {
            Vmm::Firecracker => BackendKind::Firecracker,
            Vmm::Vz => BackendKind::Vz,
        }
    }

    fn accel(self) -> &'static str {
        match self {
            Vmm::Firecracker => "kvm",
            Vmm::Vz => "vz",
        }
    }
}

/// A VM's monitor control: Firecracker's API or `cua-vz`'s.
enum Ctl {
    Fc(FcApi),
    Vz(VzApi),
}

impl Ctl {
    async fn pause(&self) -> Result<()> {
        match self {
            Ctl::Fc(a) => a.pause().await,
            Ctl::Vz(a) => a.pause().await,
        }
    }

    async fn resume(&self) -> Result<()> {
        match self {
            Ctl::Fc(a) => a.resume().await,
            Ctl::Vz(a) => a.resume().await,
        }
    }
}

/// Environment variables that carry the Spaces token; the token goes to the
/// guest's token file instead of the environment.
const TOKEN_VARS: [&str; 3] = ["CUA_SPACESD_TOKEN", "CUA_ENV_TOKEN", "CUA_GUESTD_TOKEN"];

/// Default size of a VM's (sparse) scratch disk.
const DEFAULT_DISK_GB: u32 = 16;

/// A guest counts as settled when its CPUs are busy less than this share
/// of the time...
const QUIET_BUSY: f64 = 0.05;
/// ...over two consecutive windows this long.
const QUIET_WINDOW: Duration = Duration::from_millis(150);

/// `(busy, total)` jiffies from the `cpu` line of `/proc/stat`.
fn cpu_jiffies(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal ...: idle and iowait
    // are not busy; guest time is already counted in user.
    let total: u64 = v.iter().take(8).sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

/// Golden snapshots kept (oldest removed first).
const MAX_GOLDEN: usize = 8;

/// Configuration for [`MicroVmRuntime`].
#[derive(Clone)]
pub struct MicroVmConfig {
    /// State root (default `~/.cua/vmm/microvm`).
    pub root: PathBuf,
    /// Pulls OCI images' layers (implemented by `cua-image`).
    pub resolver: Option<Arc<dyn RootfsResolver>>,
    /// Snapshot the first boot of a configuration and restore later starts
    /// from it (`CUA_MICROVM_WARM=0` turns it off).
    pub warm: bool,
    /// Where snapshots are written before they move to disk (default a
    /// directory in `/dev/shm`, when it has room).
    pub stage: Option<PathBuf>,
    /// How long `stop` waits for the guest to power off.
    pub graceful_stop: Duration,
    /// How new VMs attach their image (squashfs under `cua-vz`, which has
    /// no virtio-pmem).
    pub base_format: BaseFormat,
    /// The monitor ([`Vmm::host`] by default).
    pub vmm: Vmm,
}

impl Default for MicroVmConfig {
    fn default() -> Self {
        let vmm = Vmm::host();
        Self {
            root: host::cua_home().join("vmm").join("microvm"),
            resolver: None,
            warm: std::env::var("CUA_MICROVM_WARM").map_or(true, |v| v != "0"),
            stage: None,
            graceful_stop: Duration::from_secs(10),
            base_format: match vmm {
                Vmm::Vz => BaseFormat::Squashfs,
                Vmm::Firecracker => BaseFormat::from_env(),
            },
            vmm,
        }
    }
}

/// What a state entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Instance,
    /// A fork source created by `ensure_base` (a rootfs, no disk).
    Base,
}

/// Persisted per-VM state (`instances/<name>/state.json`, 0600: it holds
/// the Spaces token).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VmState {
    pub name: String,
    pub kind: EntryKind,
    pub image: String,
    /// The rootfs (`bases/<base>/rootfs.{sqfs,ext4}`).
    pub base: String,
    #[serde(default)]
    pub base_format: BaseFormat,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk_gb: u32,
    /// Guest ports to publish.
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Guest → host ports of the current run.
    #[serde(default)]
    pub host_ports: BTreeMap<u16, u16>,
    /// The workload (without identity).
    pub boot: BootConfig,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub probes: Vec<Probe>,
    #[serde(default)]
    pub restrict_network: bool,
    /// The egress policy ([`crate::egress`]; `None`: never set), enforced in
    /// the VM's network namespace from its first instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress: Option<crate::egress::EgressPolicy>,
    /// The network's process: `pasta` (Linux) or the network gateway
    /// (macOS, [`crate::netgw`]).
    #[serde(default)]
    pub pasta_pid: Option<u32>,
    /// The monitor's process: `firecracker` or `cua-vz`.
    #[serde(default)]
    pub fc_pid: Option<u32>,
    /// `cua-vz`'s socket directory (`<root>/run/<id>`: short, as a Unix
    /// socket path must be), for the run in progress.
    #[serde(default)]
    pub sock_id: Option<String>,
    #[serde(default)]
    pub paused: bool,
    /// A snapshot directory the next start restores (a fork).
    #[serde(default)]
    pub restore_from: Option<PathBuf>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// `cold`, `warm` (a golden snapshot) or `restore` (a fork).
    #[serde(default)]
    pub last_start: Option<String>,
    pub created_at: u64,
    /// A warm-pool member: running (paused) with no identity of its own
    /// until [`Runtime::claim`] binds it to a Space.
    #[serde(default)]
    pub pool_member: bool,
    /// A memory file that holds exactly this running VM's memory as of
    /// its last load or snapshot (it was restored with dirty-page tracking):
    /// its next snapshot writes only the pages changed since, on top of a
    /// copy of this file.
    #[serde(default)]
    pub mem_base: Option<MemBase>,
}

/// A memory file, and the inode it had, so a file replaced or deleted
/// since is never mistaken for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemBase {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
}

impl MemBase {
    fn of(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::metadata(path).ok()?;
            Some(Self {
                path: path.to_path_buf(),
                dev: m.dev(),
                ino: m.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }

    /// Whether the file is still the one recorded.
    fn intact(&self) -> bool {
        Self::of(&self.path).is_some_and(|now| now == *self)
    }
}

/// `meta.json` of a snapshot directory (a checkpoint, a golden snapshot or
/// a fork's own snapshot).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub name: String,
    #[serde(default)]
    pub source: Option<String>,
    pub image: String,
    pub base: String,
    #[serde(default)]
    pub base_format: BaseFormat,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk_gb: u32,
    #[serde(default)]
    pub ports: Vec<u16>,
    pub boot: BootConfig,
    #[serde(default)]
    pub probes: Vec<Probe>,
    #[serde(default)]
    pub restrict_network: bool,
    /// The source's egress policy: forks inherit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress: Option<crate::egress::EgressPolicy>,
    /// Memory and device state were saved (else only the disk: a fork
    /// boots it cold).
    pub memory: bool,
    pub created_at: u64,
    /// A fork source to keep: [`Runtime::delete_checkpoint`] refuses it
    /// until it is unpinned ([`MicroVmRuntime::pin_checkpoint`]).
    #[serde(default)]
    pub pinned: bool,
}

/// Image config the workload comes from.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct BaseMeta {
    reference: String,
    run: crate::runtime::OciRunConfig,
    /// Digest-pinned references known to name this rootfs.
    #[serde(default)]
    pinned: Vec<String>,
    /// Content id of the static cua-spacesd built into this rootfs
    /// ([`crate::inject`]); `None` for the image as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spacesd: Option<String>,
}

/// The microVM (Firecracker) backend.
pub struct MicroVmRuntime {
    cfg: MicroVmConfig,
    locks: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Host ports recently handed to VMs (see [`MicroVmRuntime::pick_ports`]).
    recent_ports: std::sync::Mutex<HashMap<u16, std::time::Instant>>,
    /// vCPUs of boots in progress (counted against [`vcpu_limit`]).
    booting_vcpus: std::sync::atomic::AtomicU32,
}

/// The most microVM vCPUs this host runs at once: `CUA_MICROVM_MAX_VCPUS`,
/// else 256 inside a VM (nested KVM on an AWS c8i host runs at most 256
/// nested vCPUs at once: the 257th fails with KVM_EXIT_FAIL_ENTRY, and
/// with 2-vCPU guests running ones went down too), else no limit.
pub fn vcpu_limit() -> u32 {
    if let Some(n) = std::env::var("CUA_MICROVM_MAX_VCPUS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
    {
        return n;
    }
    let nested = std::fs::read_to_string("/proc/cpuinfo")
        .map(|c| {
            c.lines()
                .any(|l| l.starts_with("flags") && l.split_whitespace().any(|f| f == "hypervisor"))
        })
        .unwrap_or(false);
    if nested { 256 } else { u32::MAX }
}

/// Releases a boot's vCPU reservation.
struct VcpuReservation<'a>(&'a std::sync::atomic::AtomicU32, u32);

impl Drop for VcpuReservation<'_> {
    fn drop(&mut self) {
        self.0
            .fetch_sub(self.1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn random_hex(n: usize) -> String {
    use std::io::Read;
    let mut b = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    } else {
        let seed = now_nanos().to_le_bytes();
        let h = Sha256::digest(seed);
        for (i, x) in b.iter_mut().enumerate() {
            *x = h[i % 32];
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unsupported_host(vmm: Vmm) -> Option<VmmError> {
    if vmm == Vmm::Vz {
        if let Some(why) = vz::unsupported_reason() {
            return Some(VmmError::other(why));
        }
        if vz::vz_bin().is_none() {
            return Some(VmmError::missing(
                "cua-vz",
                "the macOS microVM monitor ships next to `cua`; set CUA_VZ to its path or put it \
                 in ~/.cua/bin (libs/cua/vz/build.sh builds it)",
            ));
        }
        return None;
    }
    if std::env::consts::OS != "linux" {
        return Some(VmmError::Unsupported {
            backend: "firecracker",
            op: "a host other than Linux (microVMs need Linux with KVM)",
        });
    }
    if host::kvm_usable().is_err() {
        return Some(VmmError::missing(
            "/dev/kvm",
            "microVMs need KVM: enable virtualization (or nested virtualization) and add \
             this user to the kvm group",
        ));
    }
    None
}

/// The kernel command line: no PCI, the init's network fixed by `ip=`,
/// quiet, and reboot = exit (Firecracker ends when the guest reboots).
fn boot_args() -> String {
    use cua_microvm_init::{GATEWAY_IP, GUEST_IP};
    format!(
        "console=ttyS0 reboot=k panic=1 pci=off quiet loglevel=3 i8042.noaux i8042.nomux \
         i8042.nopnp i8042.dumbkbd random.trust_cpu=on \
         ip={GUEST_IP}::{GATEWAY_IP}:255.255.255.252::eth0:off"
    )
}

/// The script run inside the VM's namespace (by `pasta`): the tap device,
/// NAT, port forwards and DNS, then `exec firecracker`.
fn net_script(
    ports: &[u16],
    restrict: bool,
    config: bool,
    egress: &crate::egress::Rules,
) -> String {
    use cua_microvm_init::{GATEWAY_IP, GUEST_IP, PREFIX_LEN};
    let mut nat = String::from("*nat\n");
    for p in ports {
        nat.push_str(&format!(
            "-A OUTPUT -d 127.0.0.1/32 -p tcp --dport {p} -j DNAT --to-destination {GUEST_IP}:{p}\n"
        ));
    }
    nat.push_str(&format!(
        "-A POSTROUTING -d {GUEST_IP}/32 -j MASQUERADE\n\
         -A PREROUTING -d {GATEWAY_IP}/32 -p udp --dport 53 -j DNAT --to-destination {DNS_FWD}:53\n\
         -A PREROUTING -d {GATEWAY_IP}/32 -p tcp --dport 53 -j DNAT --to-destination {DNS_FWD}:53\n"
    ));
    if !restrict {
        nat.push_str(&format!(
            "-A POSTROUTING -s {GUEST_IP}/32 ! -d {GATEWAY_IP}/{PREFIX_LEN} -j MASQUERADE\n"
        ));
    }
    nat.push_str("COMMIT\n");
    // pasta answers some segments of a forwarded connection without the
    // ACK flag (window updates); conntrack calls them invalid, they skip the
    // reverse NAT, reach this namespace's own stack, and it resets the
    // guest's connection: every TCP download stalled after the request.
    // Drop what conntrack cannot place instead (and accept window updates as
    // valid, `nf_conntrack_tcp_be_liberal`).
    let mut filter = format!(
        "*filter\n:{chain} - [0:0]\n-A INPUT -m conntrack --ctstate INVALID -j DROP\n",
        chain = crate::egress::NS_CHAIN
    );
    if restrict {
        filter.push_str(&format!(
            "-A FORWARD -s {GUEST_IP}/32 -j DROP\n-A INPUT -s {GUEST_IP}/32 -p udp --dport 53 -j DROP\n"
        ));
    }
    // The egress policy: everything the guest sends goes through its chain
    // (empty without one, so `set_egress` only refills it). Destinations
    // are judged as the guest sent them: its DNS to the gateway is the
    // gateway's address, not pasta's forward.
    filter.push_str(&crate::egress::ns_jumps(crate::egress::NsHook::Tap(TAP)));
    filter.push_str(&crate::egress::ns_rules(egress, false, None));
    filter.push_str("COMMIT\n");
    format!(
        "set -e\n\
         printf 'tuntap add tap0 mode tap\\naddr add {GATEWAY_IP}/{PREFIX_LEN} dev tap0\\nlink set tap0 up\\n' | ip -batch -\n\
         echo 1 > /proc/sys/net/ipv4/ip_forward\n\
         echo 1 > /proc/sys/net/ipv4/conf/all/route_localnet\n\
         echo 1 > /proc/sys/net/netfilter/nf_conntrack_tcp_be_liberal 2>/dev/null || true\n\
         iptables-restore <<'EOF'\n{nat}{filter}EOF\n\
         exec \"$CUA_FIRECRACKER_BIN\" --api-sock api.sock{}\n",
        if config { " --config-file vm.json" } else { "" }
    )
}

/// How a VM's read-only image is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BaseFormat {
    /// A compressed squashfs on a virtio block device (each VM caches what
    /// it reads in its own memory).
    #[default]
    Squashfs,
    /// An ext4 image on virtio-pmem, mapped with DAX: every VM reads the
    /// host's page cache, so shared files (a browser) cost memory once per
    /// host. The default.
    Ext4Dax,
}

impl BaseFormat {
    /// `CUA_MICROVM_BASE=squashfs` picks squashfs; ext4 + DAX otherwise.
    pub fn from_env() -> Self {
        match std::env::var("CUA_MICROVM_BASE").as_deref() {
            Ok("squashfs") => BaseFormat::Squashfs,
            _ => BaseFormat::Ext4Dax,
        }
    }

    fn file(self) -> &'static str {
        match self {
            BaseFormat::Squashfs => "rootfs.sqfs",
            BaseFormat::Ext4Dax => "rootfs.ext4",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            BaseFormat::Squashfs => ".complete",
            BaseFormat::Ext4Dax => ".complete-ext4",
        }
    }

    /// The name of the VM directory's link to the image (recorded in
    /// snapshots).
    fn link(self) -> &'static str {
        match self {
            BaseFormat::Squashfs => "base.sqfs",
            BaseFormat::Ext4Dax => "base.img",
        }
    }
}

/// Removes the memory-base links in VM directory `dir` but `keep`.
fn drop_bases(dir: &Path, keep: Option<&Path>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_name().to_string_lossy().starts_with("mem-base-") && Some(p.as_path()) != keep {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Opens `path` and takes an exclusive `flock` on it (blocking); the lock
/// lasts as long as the returned file.
fn lock_exclusive(path: &Path) -> Result<std::fs::File> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(f)
}

/// The VM namespace's own address on pasta's link, and its gateway (from
/// the benchmarking range 198.18.0.0/15, which no LAN or VPN uses).
const NS_IP: &str = "198.18.0.2";
const NS_GATEWAY: &str = "198.18.0.1";

/// The guest's link MTU (virtio-net's default).
const GUEST_MTU: u32 = 1500;

/// Where `pasta` forwards the guest's DNS (to the host's resolver).
const DNS_FWD: &str = "169.254.1.53";

/// The guest's tap device in the VM's namespace.
const TAP: &str = "tap0";

impl MicroVmRuntime {
    pub fn new(cfg: MicroVmConfig) -> Self {
        Self {
            cfg,
            locks: std::sync::Mutex::new(HashMap::new()),
            recent_ports: std::sync::Mutex::new(HashMap::new()),
            booting_vcpus: std::sync::atomic::AtomicU32::new(0),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(MicroVmConfig::default())
    }

    pub fn config(&self) -> &MicroVmConfig {
        &self.cfg
    }

    /// Host checks for `cua runtime doctor`.
    pub async fn report(&self) -> MicroVmReport {
        assets::report(&self.cfg.root).await
    }

    fn lock_for(&self, key: &str) -> Arc<Mutex<()>> {
        let mut m = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn instances(&self) -> PathBuf {
        self.cfg.root.join("instances")
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.instances().join(name)
    }

    fn checkpoint_dir(&self, name: &str) -> PathBuf {
        self.cfg.root.join("checkpoints").join(name)
    }

    fn base_dir(&self, base: &str) -> PathBuf {
        self.cfg.root.join("bases").join(base)
    }

    fn golden_dir(&self, key: &str) -> PathBuf {
        self.cfg.root.join("golden").join(key)
    }

    /// Persisted state of `name`.
    pub fn load(&self, name: &str) -> Result<VmState> {
        let p = self.dir(name).join("state.json");
        let raw = std::fs::read(&p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VmmError::NotFound(name.to_string())
            } else {
                e.into()
            }
        })?;
        serde_json::from_slice(&raw).map_err(|e| VmmError::State {
            path: p,
            detail: e.to_string(),
        })
    }

    fn save(&self, st: &VmState) -> Result<()> {
        let dir = self.dir(&st.name);
        std::fs::create_dir_all(&dir)?;
        host::write_private(&dir.join("state.json"), &serde_json::to_vec_pretty(st)?)?;
        Ok(())
    }

    fn all_states(&self) -> Vec<VmState> {
        let Ok(rd) = std::fs::read_dir(self.instances()) else {
            return vec![];
        };
        let mut v: Vec<VmState> = rd
            .flatten()
            .filter_map(|e| self.load(&e.file_name().to_string_lossy()).ok())
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Whether `pid` is this VM's `firecracker` (pids are reused: its
    /// working directory must be the VM's).
    fn owns(&self, name: &str, pid: Option<u32>) -> bool {
        let Some(pid) = pid else { return false };
        if !host::pid_alive(pid) {
            return false;
        }
        match vz::pid_cwd(pid) {
            Some(cwd) => {
                let dir = self.dir(name);
                cwd == dir || std::fs::canonicalize(&dir).is_ok_and(|d| d == cwd)
            }
            // Not readable (another user, or no procfs): trust the pid.
            None => true,
        }
    }

    fn running(&self, st: &VmState) -> bool {
        st.kind == EntryKind::Instance && self.owns(&st.name, st.fc_pid)
    }

    fn status_of(&self, st: &VmState) -> Status {
        if self.running(st) {
            if st.paused {
                Status::Paused
            } else {
                Status::Running
            }
        } else {
            Status::Stopped
        }
    }

    fn endpoints_of(&self, st: &VmState) -> Endpoints {
        Endpoints {
            host: "127.0.0.1".into(),
            ports: st.host_ports.clone(),
            serial_log: Some(self.dir(&st.name).join("console.log")),
            ..Default::default()
        }
    }

    fn instance(&self, st: &VmState) -> Instance {
        Instance {
            name: st.name.clone(),
            backend: self.cfg.vmm.backend(),
            status: self.status_of(st),
            endpoints: self.endpoints_of(st),
            isolation: Isolation::Vm {
                accel: self.cfg.vmm.accel().into(),
            },
            arch: Some(Arch::host()),
        }
    }

    /// `cua-vz`'s socket directory for a run.
    fn run_dir(&self, sock_id: &str) -> PathBuf {
        self.cfg.root.join("run").join(sock_id)
    }

    /// Where VM `name`'s monitor sockets are: its directory (Firecracker),
    /// or its run's socket directory (`cua-vz`).
    fn sock(&self, name: &str, file: &str) -> PathBuf {
        if self.cfg.vmm == Vmm::Vz
            && let Some(id) = self.load(name).ok().and_then(|s| s.sock_id)
        {
            return self.run_dir(&id).join(file);
        }
        self.dir(name).join(file)
    }

    fn agent(&self, name: &str) -> Agent {
        Agent::new(self.sock(name, "v.sock"))
    }

    fn api(&self, name: &str) -> FcApi {
        FcApi::new(self.dir(name).join("api.sock"))
    }

    fn vz_api(&self, name: &str) -> VzApi {
        VzApi::new(self.sock(name, "api.sock"))
    }

    fn ctl(&self, name: &str) -> Ctl {
        match self.cfg.vmm {
            Vmm::Firecracker => Ctl::Fc(self.api(name)),
            Vmm::Vz => Ctl::Vz(self.vz_api(name)),
        }
    }

    /// The image format new VMs use.
    fn base_format(&self) -> BaseFormat {
        match self.cfg.vmm {
            Vmm::Vz => BaseFormat::Squashfs,
            Vmm::Firecracker => self.cfg.base_format,
        }
    }

    // ------------------------------------------------------------ images

    /// A rootfs of `format` built for exactly `reference`.
    fn built_base(
        &self,
        reference: &str,
        format: BaseFormat,
        spacesd: Option<&str>,
    ) -> Option<(String, crate::runtime::OciRunConfig)> {
        let rd = std::fs::read_dir(self.cfg.root.join("bases")).ok()?;
        for e in rd.flatten() {
            let dir = e.path();
            if !dir.join(format.marker()).exists() {
                continue;
            }
            let Ok(raw) = std::fs::read(dir.join("image.json")) else {
                continue;
            };
            let Ok(meta) = serde_json::from_slice::<BaseMeta>(&raw) else {
                continue;
            };
            if meta.spacesd.as_deref() == spacesd
                && (meta.reference == reference || meta.pinned.iter().any(|p| p == reference))
            {
                crate::disk::mark_used(&dir);
                return Some((e.file_name().to_string_lossy().into_owned(), meta.run));
            }
        }
        None
    }

    /// The read-only rootfs of `reference` in `format` (built once per
    /// image digest) and its run config.
    ///
    /// `inject`: the static cua-spacesd to build into the rootfs as one more
    /// layer ([`crate::inject`]), for an image that has none. Such a rootfs
    /// is a base of its own, named by the image digest and the binary.
    async fn ensure_rootfs(
        &self,
        reference: &str,
        creds: Option<&RegistryCredentials>,
        format: BaseFormat,
        inject: Option<&Path>,
    ) -> Result<(String, crate::runtime::OciRunConfig)> {
        let extra = match inject {
            Some(binary) => Some(self.spacesd_layer(binary).await?),
            None => None,
        };
        let sd_id = extra.as_ref().map(|(_, id)| id.as_str());
        // A digest-pinned reference never moves: once built, its rootfs is
        // found without asking the registry.
        if reference.contains("@sha256:")
            && let Some(hit) = self.built_base(reference, format, sd_id)
        {
            return Ok(hit);
        }
        let resolver = self.cfg.resolver.as_ref().ok_or_else(|| {
            VmmError::other("microVM images need an OCI resolver (cua-image); none is configured")
        })?;
        let layers = resolver
            .resolve_rootfs(reference, Arch::host(), creds)
            .await?;
        let mut base = layers
            .digest
            .strip_prefix("sha256:")
            .unwrap_or(&layers.digest)
            .to_string();
        if let Some(id) = sd_id {
            base = format!("{base}-sd{id}");
        }
        let dir = self.base_dir(&base);
        // Remember a pinned spelling so the next start skips the registry.
        let remember = |dir: &Path| -> Result<()> {
            if reference.contains("@sha256:")
                && let Ok(raw) = std::fs::read(dir.join("image.json"))
                && let Ok(mut m) = serde_json::from_slice::<BaseMeta>(&raw)
                && m.reference != reference
                && !m.pinned.iter().any(|p| p == reference)
            {
                m.pinned.push(reference.to_string());
                std::fs::write(dir.join("image.json"), serde_json::to_vec_pretty(&m)?)?;
            }
            Ok(())
        };
        if dir.join(format.marker()).exists() {
            crate::disk::mark_used(&dir);
            remember(&dir)?;
            return Ok((base, layers.config));
        }
        let lock = self.lock_for(&format!("base:{base}"));
        let _g = lock.lock().await;
        // Other processes (another daemon, a test) may build it too.
        std::fs::create_dir_all(&dir)?;
        let lock_path = dir.join(".build.lock");
        let _flock = tokio::task::spawn_blocking(move || lock_exclusive(&lock_path))
            .await
            .map_err(|e| VmmError::other(e.to_string()))??;
        if dir.join(format.marker()).exists() {
            remember(&dir)?;
            return Ok((base, layers.config));
        }
        let mut image_layers = layers.layers.clone();
        if let Some((tar, _)) = &extra {
            image_layers.push(tar.clone());
        }
        let total: u64 = image_layers
            .iter()
            .filter_map(|l| std::fs::metadata(l).ok())
            .map(|m| m.len())
            .sum();
        crate::disk::ensure_space(
            &dir,
            // The in-process squashfs writer also keeps the merged tarball.
            total.saturating_mul(match format {
                BaseFormat::Ext4Dax => 6,
                BaseFormat::Squashfs if self.cfg.vmm == Vmm::Vz => 5,
                BaseFormat::Squashfs => 2,
            }),
            &format!("microVM rootfs of {reference}"),
        )?;
        crate::progress::report(
            crate::progress::Progress::phase(crate::progress::Phase::Creating)
                .detail("building the microVM root filesystem"),
        );
        let out = dir.join(format.file());
        let files = image_layers;
        let started = std::time::Instant::now();
        let native = self.cfg.vmm == Vmm::Vz;
        tokio::task::spawn_blocking(move || match format {
            BaseFormat::Squashfs if native => rootfs::build_squashfs_native(&files, &out),
            BaseFormat::Squashfs => rootfs::build_squashfs(&files, &out),
            BaseFormat::Ext4Dax => rootfs::build_ext4(&files, &out),
        })
        .await
        .map_err(|e| VmmError::other(e.to_string()))??;
        if !dir.join("image.json").exists() {
            let meta = BaseMeta {
                reference: reference.to_string(),
                run: layers.config.clone(),
                pinned: vec![],
                spacesd: sd_id.map(str::to_string),
            };
            std::fs::write(dir.join("image.json"), serde_json::to_vec_pretty(&meta)?)?;
        } else {
            remember(&dir)?;
        }
        std::fs::write(dir.join(format.marker()), &layers.digest)?;
        tracing::info!(
            reference,
            base,
            ?format,
            secs = started.elapsed().as_secs_f64(),
            "built microVM rootfs"
        );
        Ok((base, layers.config))
    }

    /// The layer tar that adds the static cua-spacesd to a rootfs, and the
    /// binary's content id (cached under the microVM root by that id).
    async fn spacesd_layer(&self, binary: &Path) -> Result<(PathBuf, String)> {
        let binary = binary.to_path_buf();
        let root = self.cfg.root.join("spacesd");
        tokio::task::spawn_blocking(move || {
            let id = crate::inject::content_id(&binary)?;
            let tar = root.join(format!("{id}.tar"));
            if !tar.is_file() {
                std::fs::create_dir_all(&root)?;
                let tmp = tar.with_extension("partial");
                std::fs::write(&tmp, crate::inject::layer_tar(&binary)?)?;
                std::fs::rename(&tmp, &tar)?;
            }
            Ok((tar, id))
        })
        .await
        .map_err(|e| VmmError::other(e.to_string()))?
    }

    /// The initramfs holding the guest init (cached by the init's hash).
    async fn ensure_initrd(&self) -> Result<PathBuf> {
        let init = assets::init_bin().ok_or_else(|| {
            VmmError::missing(
                "cua-microvm-init",
                "the microVM guest init ships next to `cua`; set CUA_MICROVM_INIT to its path \
                 or put it in ~/.cua/bin",
            )
        })?;
        let bytes = tokio::fs::read(&init).await?;
        // The initramfs holds nothing but the init: a dynamically linked
        // one panics the guest kernel ("No working init found").
        match rootfs::elf_interpreter(&bytes) {
            Some(None) => {}
            Some(Some(interp)) => {
                return Err(VmmError::invalid(format!(
                    "the microVM guest init {} is dynamically linked (it asks for {interp}); \
                     it must be static: cargo build --release -p cua-microvm-init --target \
                     x86_64-unknown-linux-musl (or aarch64-unknown-linux-musl)",
                    init.display()
                )));
            }
            None => {
                return Err(VmmError::invalid(format!(
                    "the microVM guest init {} is not a Linux executable",
                    init.display()
                )));
            }
        }
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let p = self
            .cfg
            .root
            .join("initrd")
            .join(format!("{}.cpio", &hash[..16]));
        if !p.is_file() {
            std::fs::create_dir_all(p.parent().unwrap_or(&self.cfg.root))?;
            let tmp = p.with_extension("partial");
            std::fs::write(&tmp, rootfs::initramfs(&bytes))?;
            std::fs::rename(&tmp, &p)?;
        }
        Ok(p)
    }

    /// A fresh scratch disk at `dest` (a sparse copy of a formatted
    /// template).
    async fn fresh_scratch(&self, dest: &Path, gib: u32) -> Result<()> {
        if self.cfg.vmm == Vmm::Vz {
            // No mkfs.ext4 here: a small formatted ext4 in a file of the
            // asked size, which the guest init grows at boot.
            let (dest, size) = (dest.to_path_buf(), u64::from(gib.max(1)) << 30);
            return tokio::task::spawn_blocking(move || {
                rootfs::expand_template(rootfs::SCRATCH_TEMPLATE, &dest, size)
            })
            .await
            .map_err(|e| VmmError::other(e.to_string()))?;
        }
        let tmpl = self
            .cfg
            .root
            .join("templates")
            .join(format!("scratch-{gib}g.ext4"));
        if !tmpl.is_file() {
            let lock = self.lock_for(&format!("tmpl:{gib}"));
            let _g = lock.lock().await;
            if !tmpl.is_file() {
                std::fs::create_dir_all(tmpl.parent().unwrap_or(&self.cfg.root))?;
                let t = tmpl.clone();
                tokio::task::spawn_blocking(move || {
                    let _flock = lock_exclusive(&t.with_extension("lock"))?;
                    if t.is_file() {
                        return Ok(());
                    }
                    rootfs::make_ext4(&t, gib)
                })
                .await
                .map_err(|e| VmmError::other(e.to_string()))??;
            }
        }
        rootfs::copy_sparse(&tmpl, dest).await
    }

    // ------------------------------------------------------------ processes

    #[cfg(not(unix))]
    async fn launch(&self, _st: &mut VmState, _config: Option<serde_json::Value>) -> Result<()> {
        Err(unix_only("start"))
    }

    /// Starts `pasta` + `firecracker` for `st` (cold boot when `config` is
    /// given, else an empty VMM for a snapshot load) and waits for the API.
    #[cfg(unix)]
    async fn launch(&self, st: &mut VmState, config: Option<serde_json::Value>) -> Result<()> {
        let dir = self.dir(&st.name);
        for f in ["api.sock", "v.sock", "fc.pid"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
        // Firecracker installs on first use; passt is the host's to install.
        let pasta = assets::pasta_bin().ok_or_else(assets::pasta_missing)?;
        let fc = assets::ensure_firecracker().await?;
        let link = dir.join(st.base_format.link());
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(self.base_dir(&st.base).join(st.base_format.file()), &link)?;
        if let Some(c) = &config {
            std::fs::write(dir.join("vm.json"), serde_json::to_vec_pretty(c)?)?;
        }
        std::fs::write(
            dir.join("net.sh"),
            net_script(
                &st.ports,
                st.restrict_network,
                config.is_some(),
                &st.egress
                    .as_ref()
                    .map(|p| p.parse())
                    .transpose()?
                    .unwrap_or_default(),
            ),
        )?;
        let host_ports = self.pick_ports(st.ports.len())?;
        st.host_ports = st.ports.iter().copied().zip(host_ports).collect();
        let mut args: Vec<String> = vec![
            "--config-net".into(),
            "-q".into(),
            "-f".into(),
            "-u".into(),
            "none".into(),
            "-T".into(),
            "none".into(),
            "-U".into(),
            "none".into(),
            "--dns-forward".into(),
            DNS_FWD.into(),
            // The guest must not reach the host's loopback services (other
            // Spaces' ports, the daemon) through the gateway address.
            "--no-map-gw".into(),
            // The guest's link is 1500 bytes: pasta must not offer TCP
            // peers a larger MSS (with its default 65520 MTU it offers
            // 61440), or replies too big for the guest's link are dropped
            // and every TCP download stalls after the handshake.
            "--mtu".into(),
            GUEST_MTU.to_string(),
            // Not a copy of the host's address (pasta's default): in the
            // namespace that address would be local, and a guest could not
            // reach services on the host's own LAN/VPC address.
            "--address".into(),
            NS_IP.into(),
            "--netmask".into(),
            "30".into(),
            "--gateway".into(),
            NS_GATEWAY.into(),
        ];
        if st.host_ports.is_empty() {
            args.extend(["-t".into(), "none".into()]);
        }
        for (g, h) in &st.host_ports {
            args.extend(["-t".into(), format!("127.0.0.1/{h}:{g}")]);
        }
        args.extend(["--".into(), "/bin/sh".into(), "net.sh".into()]);
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("console.log"))?;
        let mut cmd = tokio::process::Command::new(&pasta);
        cmd.args(&args)
            .current_dir(&dir)
            .env("CUA_FIRECRACKER_BIN", &fc)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .kill_on_drop(false);
        let mut child = cmd
            .spawn()
            .map_err(|e| VmmError::other(format!("pasta: {e}")))?;
        st.pasta_pid = child.id();
        // Reap it whenever it exits (the VM outlives this handle).
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        let api = self.api(&st.name);
        let alive = || st.pasta_pid.is_some_and(host::pid_alive);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if dir.join("api.sock").exists()
                && api.wait_ready(Duration::from_millis(50)).await.is_ok()
            {
                break;
            }
            if !alive() {
                return Err(VmmError::other(format!(
                    "the microVM's network namespace (pasta) exited; see {}",
                    dir.join("console.log").display()
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(VmmError::other("firecracker did not start within 20 s"));
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        // pasta runs its command in new PID namespace: the VMM is pasta's
        // child (the script execs it), seen from here by its host pid.
        st.fc_pid = st.pasta_pid.and_then(|p| {
            std::fs::read_to_string(format!("/proc/{p}/task/{p}/children"))
                .ok()
                .and_then(|s| s.split_whitespace().next().and_then(|c| c.parse().ok()))
        });
        if st.fc_pid.is_none() {
            return Err(VmmError::other(
                "cannot find the firecracker process under pasta",
            ));
        }
        st.paused = false;
        self.save(st)?;
        Ok(())
    }

    #[cfg(not(unix))]
    async fn launch_vz(
        &self,
        _st: &mut VmState,
        _kernel: &Path,
        _initrd: &Path,
        _restore: Option<&Path>,
        _timeout: Duration,
    ) -> Result<()> {
        Err(unix_only("start"))
    }

    /// Starts `cua-vz` (and the VM's network gateway) for `st`: a cold boot of
    /// `kernel` and `initrd`, or a restore of the saved state `restore`
    /// (the monitor resumes it). Returns once the VM runs.
    #[cfg(unix)]
    async fn launch_vz(
        &self,
        st: &mut VmState,
        kernel: &Path,
        initrd: &Path,
        restore: Option<&Path>,
        timeout: Duration,
    ) -> Result<()> {
        let dir = self.dir(&st.name);
        let bin = vz::vz_bin().ok_or_else(|| {
            VmmError::missing(
                "cua-vz",
                "the macOS microVM monitor ships next to `cua`; set CUA_VZ to its path or put \
                 it in ~/.cua/bin (libs/cua/vz/build.sh builds it)",
            )
        })?;
        let gateway = if st.restrict_network {
            None
        } else {
            Some(crate::netgw::bin().ok_or_else(crate::netgw::missing)?)
        };
        let id = random_hex(4);
        let run = self.run_dir(&id);
        let _ = std::fs::remove_dir_all(&run);
        std::fs::create_dir_all(&run)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700))?;
        }
        st.sock_id = Some(id);
        let link = dir.join(st.base_format.link());
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(self.base_dir(&st.base).join(st.base_format.file()), &link)?;
        let _ = std::fs::remove_file(dir.join(crate::netgw::PID_FILE));
        if gateway.is_some() {
            // The rules first: the gateway does not start without them.
            crate::netgw::write_rules(
                &dir,
                &st.egress
                    .as_ref()
                    .map(|p| p.parse())
                    .transpose()?
                    .unwrap_or_default(),
            )?;
            std::fs::write(
                dir.join(crate::netgw::CONFIG_FILE),
                serde_json::to_vec_pretty(&vz::gateway_config(&run))?,
            )?;
        }
        let host_ports = self.pick_ports(st.ports.len())?;
        st.host_ports = st.ports.iter().copied().zip(host_ports).collect();
        let forwards: Vec<(u16, u16)> = st.host_ports.iter().map(|(g, h)| (*h, *g)).collect();
        let config = vz::vz_config(&vz::VzLaunch {
            kernel,
            initrd,
            cpus: st.cpus,
            memory_mb: st.memory_mb,
            base: st.base_format.link(),
            sockets: &run,
            gateway: gateway.as_deref(),
            forwards: &forwards,
            restore,
        });
        std::fs::write(dir.join("vz.json"), serde_json::to_vec_pretty(&config)?)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("console.log"))?;
        let mut child = tokio::process::Command::new(&bin)
            .arg(&dir)
            .current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .kill_on_drop(false)
            .spawn()
            .map_err(|e| VmmError::other(format!("cua-vz: {e}")))?;
        st.fc_pid = child.id();
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = exited.clone();
        // Reap it whenever it exits (the VM outlives this handle).
        tokio::spawn(async move {
            let _ = child.wait().await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let api = VzApi::new(run.join("api.sock"));
        // A restore reads the guest's memory before the monitor answers.
        let deadline = tokio::time::Instant::now() + timeout.max(Duration::from_secs(60));
        loop {
            if run.join("api.sock").exists() && api.state().await.is_ok() {
                break;
            }
            if exited.load(std::sync::atomic::Ordering::SeqCst) {
                let tail = std::fs::read_to_string(dir.join("console.log")).unwrap_or_default();
                let why: Vec<&str> = tail.lines().filter(|l| l.starts_with("cua-vz:")).collect();
                st.fc_pid = None;
                return Err(VmmError::other(format!(
                    "the microVM monitor exited: {}",
                    why.last().copied().unwrap_or("see console.log")
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(VmmError::other("cua-vz did not start the VM in time"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        st.pasta_pid = std::fs::read_to_string(dir.join(crate::netgw::PID_FILE))
            .ok()
            .and_then(|p| p.trim().parse().ok());
        st.paused = false;
        self.save(st)?;
        Ok(())
    }

    fn vm_config(&self, st: &VmState, kernel: &Path, initrd: &Path) -> serde_json::Value {
        let mut c = self.vm_config_blocks(st, kernel, initrd);
        if st.base_format == BaseFormat::Ext4Dax {
            // The image on virtio-pmem (DAX in the guest); the scratch
            // disk is then the only block device.
            c["drives"] = json!([
                {"drive_id": "scratch", "path_on_host": "scratch.ext4", "is_root_device": false, "is_read_only": false},
            ]);
            c["pmem"] = json!([
                {"id": "base", "path_on_host": BaseFormat::Ext4Dax.link(), "root_device": false, "read_only": true},
            ]);
        }
        c
    }

    fn vm_config_blocks(&self, st: &VmState, kernel: &Path, initrd: &Path) -> serde_json::Value {
        json!({
            "boot-source": {
                "kernel_image_path": kernel,
                "initrd_path": initrd,
                "boot_args": boot_args(),
            },
            "drives": [
                {"drive_id": "base", "path_on_host": "base.sqfs", "is_root_device": false, "is_read_only": true},
                {"drive_id": "scratch", "path_on_host": "scratch.ext4", "is_root_device": false, "is_read_only": false},
            ],
            "network-interfaces": [
                {"iface_id": "eth0", "guest_mac": cua_microvm_init::GUEST_MAC, "host_dev_name": "tap0"}
            ],
            "machine-config": {"vcpu_count": st.cpus.max(1), "mem_size_mib": st.memory_mb.max(256)},
            "vsock": {"guest_cid": 3, "uds_path": "v.sock"},
            // Free page reporting: memory the guest frees goes back to the
            // host (density); the balloon itself stays empty.
            "balloon": {
                "amount_mib": 0,
                "deflate_on_oom": true,
                "stats_polling_interval_s": 0,
                "free_page_reporting": true,
            },
        })
    }

    /// `n` free loopback ports no other VM of this process was handed in
    /// the last minute (a port is free between the pick and pasta's bind).
    fn pick_ports(&self, n: usize) -> Result<Vec<u16>> {
        let mut recent = self.recent_ports.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        recent.retain(|_, at| now.duration_since(*at) < Duration::from_secs(60));
        let mut out = Vec::with_capacity(n);
        let mut tries = 0;
        while out.len() < n {
            tries += 1;
            if tries > 64 * n.max(1) {
                return Err(VmmError::other("no free loopback ports"));
            }
            for p in host::free_ports(n - out.len())? {
                if !recent.contains_key(&p) && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        for p in &out {
            recent.insert(*p, now);
        }
        Ok(out)
    }

    /// Makes `file` (a memory file VM `name` now equals) the VM's own base:
    /// a hard link in its directory, so the base outlives the snapshot,
    /// fork or golden snapshot it came from; older bases are dropped.
    fn own_base(&self, name: &str, file: &Path) -> Option<MemBase> {
        let dir = self.dir(name);
        let link = dir.join(format!("mem-base-{}", random_hex(4)));
        let kept = match std::fs::hard_link(file, &link) {
            Ok(()) => link.clone(),
            // Another filesystem: the file itself, while it lasts.
            Err(_) => file.to_path_buf(),
        };
        drop_bases(&dir, Some(&kept));
        MemBase::of(&kept)
    }

    /// Kills the VM's processes (no clean shutdown).
    fn kill(&self, st: &mut VmState) {
        if self.owns(&st.name, st.fc_pid)
            && let Some(p) = st.fc_pid
        {
            host::signal(p, "KILL");
        }
        if let Some(p) = st.pasta_pid.filter(|p| host::pid_alive(*p)) {
            // The gateway outlives a killed cua-vz: only ever signal it if
            // the pid is still a gateway.
            let ours = self.cfg.vmm == Vmm::Firecracker
                || vz::pid_path(p).is_some_and(|e| crate::netgw::is_gateway(&e));
            if ours {
                host::signal(p, "TERM");
            }
        }
        if let Some(id) = st.sock_id.take() {
            let _ = std::fs::remove_dir_all(self.run_dir(&id));
        }
        st.fc_pid = None;
        st.pasta_pid = None;
        st.paused = false;
        st.host_ports.clear();
        st.mem_base = None;
        drop_bases(&self.dir(&st.name), None);
    }

    async fn wait_exit(&self, st: &VmState, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if !self.owns(&st.name, st.fc_pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        !self.owns(&st.name, st.fc_pid)
    }

    // ------------------------------------------------------------ boot paths

    fn identity(&self, st: &VmState) -> Identity {
        Identity {
            hostname: Some(st.name.clone()),
            token: st.token.clone(),
            token_group: None,
            entropy: Some(random_hex(64)),
            unix_nanos: Some(now_nanos() as u64),
            machine_id: Some(random_hex(16)),
            env: BTreeMap::new(),
        }
    }

    /// The configuration a golden snapshot is keyed by: everything that
    /// shapes the guest before it gets an identity.
    fn golden_key(&self, st: &VmState, initrd: &Path) -> String {
        let monitor = match self.cfg.vmm {
            Vmm::Firecracker => assets::firecracker_bin(),
            Vmm::Vz => vz::vz_bin(),
        };
        let fc = monitor
            .and_then(|p| std::fs::metadata(&p).ok().map(|m| (p, m.len())))
            .map(|(p, n)| format!("{}:{n}", p.display()))
            .unwrap_or_default();
        let mut probe_ports: Vec<u16> = st.probes.iter().filter_map(Probe::port).collect();
        probe_ports.sort_unstable();
        let doc = json!({
            "v": 4,
            "base": st.base,
            "format": st.base_format,
            "cpus": st.cpus,
            "memory_mb": st.memory_mb,
            "disk_gb": st.disk_gb,
            "boot": st.boot,
            "probes": probe_ports,
            "restrict": st.restrict_network,
            "kernel": assets::KERNEL_VERSION,
            "vmm": self.cfg.vmm,
            "initrd": initrd.file_name().map(|n| n.to_string_lossy().into_owned()),
            "fc": fc,
        });
        let h = Sha256::digest(doc.to_string().as_bytes());
        h.iter().take(12).map(|b| format!("{b:02x}")).collect()
    }

    /// Boots `st` (new or stopped): a pending fork snapshot, else a golden
    /// snapshot of the same configuration (a fresh VM only), else a cold
    /// boot (which records the golden snapshot when warm starts are on).
    async fn boot(&self, st: &mut VmState, fresh: bool, timeout: Duration) -> Result<()> {
        if let Some(snap) = st.restore_from.clone() {
            self.restore(st, &snap, timeout, "restore").await?;
            st.restore_from = None;
            self.save(st)?;
            return Ok(());
        }
        let initrd = self.ensure_initrd().await?;
        let warm = self.cfg.warm && fresh && !st.probes.is_empty();
        let key = warm.then(|| self.golden_key(st, &initrd));
        if let Some(k) = &key {
            let g = self.golden_dir(k);
            if g.join("meta.json").is_file() {
                crate::disk::mark_used(&g);
                match self.restore(st, &g, timeout, "warm").await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        // A snapshot this host cannot load (a new kernel,
                        // a changed CPU): drop it and boot cold.
                        tracing::warn!(golden = %g.display(), error = %e, "golden snapshot did not restore; booting cold");
                        self.kill(st);
                        let _ = std::fs::remove_dir_all(&g);
                        let _ = std::fs::remove_file(self.dir(&st.name).join("scratch.ext4"));
                    }
                }
            }
        }
        // A golden restore brings its own disk; only a cold boot needs a
        // fresh one.
        let scratch = self.dir(&st.name).join("scratch.ext4");
        if !scratch.is_file() {
            self.fresh_scratch(&scratch, st.disk_gb).await?;
        }
        self.cold_boot(st, &initrd, key, timeout).await
    }

    async fn cold_boot(
        &self,
        st: &mut VmState,
        initrd: &Path,
        golden: Option<String>,
        timeout: Duration,
    ) -> Result<()> {
        // What the host must have comes first, before anything downloads:
        // a host without passt is told what to install, not a raw error.
        if self.cfg.vmm == Vmm::Firecracker {
            assets::pasta_bin().ok_or_else(assets::pasta_missing)?;
            assets::ensure_firecracker().await?;
        }
        let kernel = assets::ensure_kernel(&self.cfg.root).await?;
        crate::progress::report(crate::progress::Progress::phase(
            crate::progress::Phase::Booting,
        ));
        let started = std::time::Instant::now();
        match self.cfg.vmm {
            Vmm::Firecracker => {
                let config = self.vm_config(st, &kernel, initrd);
                self.launch(st, Some(config)).await?;
            }
            Vmm::Vz => self.launch_vz(st, &kernel, initrd, None, timeout).await?,
        }
        let agent = self.agent(&st.name);
        let (name, fc) = (st.name.clone(), st.fc_pid);
        agent
            .wait(timeout.min(Duration::from_secs(60)), || {
                self.owns(&name, fc)
            })
            .await?;
        // A golden snapshot must not hold this VM's identity: it is applied
        // after the snapshot.
        let mut boot = st.boot.clone();
        if golden.is_none() {
            boot.identity = Some(self.identity(st));
        }
        agent
            .request(&Request::Boot(boot), Duration::from_secs(30))
            .await?;
        self.wait_probes(st, timeout).await?;
        tracing::info!(
            name = st.name,
            ms = started.elapsed().as_millis() as u64,
            "microVM booted cold"
        );
        if let Some(k) = golden {
            // Every VM restored from the snapshot inherits whatever the
            // guest was still doing: let the desktop finish starting first.
            let settled = self.quiesce(st, Duration::from_secs(20)).await;
            tracing::debug!(
                name = st.name,
                ms = started.elapsed().as_millis() as u64,
                settled,
                "guest settled before its golden snapshot"
            );
            if let Err(e) = self.record_golden(st, &k).await {
                tracing::warn!(error = %e, "could not record a golden snapshot");
            }
            agent
                .request(
                    &Request::Identity(self.identity(st)),
                    Duration::from_secs(10),
                )
                .await?;
        }
        st.last_start = Some("cold".into());
        self.save(st)?;
        Ok(())
    }

    /// Restores `snap` into `st` (whose scratch disk is replaced by the
    /// snapshot's), then gives it its identity.
    async fn restore(
        &self,
        st: &mut VmState,
        snap: &Path,
        timeout: Duration,
        how: &str,
    ) -> Result<()> {
        let started = std::time::Instant::now();
        let dir = self.dir(&st.name);
        let scratch = dir.join("scratch.ext4");
        // A fork's own snapshot is used once: its disk moves into place.
        if snap.starts_with(&dir) {
            if snap.join("scratch.ext4").is_file() {
                std::fs::rename(snap.join("scratch.ext4"), &scratch)?;
            }
        } else {
            rootfs::copy_sparse(&snap.join("scratch.ext4"), &scratch).await?;
        }
        let t_disk = started.elapsed();
        let t_launch;
        match self.cfg.vmm {
            Vmm::Firecracker => {
                self.launch(st, None).await?;
                t_launch = started.elapsed();
                let api = self.api(&st.name);
                if let Err(e) = api.load(&snap.join("vmstate"), &snap.join("mem")).await {
                    self.kill(st);
                    self.save(st)?;
                    return Err(e);
                }
                st.mem_base = self.own_base(&st.name, &snap.join("mem"));
            }
            Vmm::Vz => {
                // The saved VM's machine identifier: the framework restores
                // into no other.
                std::fs::copy(snap.join("machine-id"), dir.join("machine-id"))?;
                let kernel = assets::ensure_kernel(&self.cfg.root).await?;
                let initrd = self.ensure_initrd().await?;
                if let Err(e) = self
                    .launch_vz(st, &kernel, &initrd, Some(&snap.join("vzstate")), timeout)
                    .await
                {
                    self.kill(st);
                    self.save(st)?;
                    return Err(e);
                }
                // Restoring is the launch; the guest's memory is its own.
                t_launch = t_disk;
                st.mem_base = None;
                // A fork's own snapshot is spent.
                if snap.starts_with(&dir) {
                    let _ = std::fs::remove_dir_all(snap);
                }
            }
        }
        let agent = self.agent(&st.name);
        let (name, fc) = (st.name.clone(), st.fc_pid);
        // A restored guest answers in milliseconds, but on a slow nested
        // host its first page faults can take seconds: allow what a boot
        // gets.
        agent
            .wait(timeout.min(Duration::from_secs(60)), || {
                self.owns(&name, fc)
            })
            .await?;
        // The guest holds its snapshot's token file: a VM without a token
        // of its own empties it, so it never opens with another's.
        let mut id = self.identity(st);
        id.token.get_or_insert_with(String::new);
        agent
            .request(&Request::Identity(id), Duration::from_secs(10))
            .await?;
        let t_identity = started.elapsed();
        self.wait_probes(st, timeout).await?;
        tracing::info!(
            name = st.name,
            how,
            ms = started.elapsed().as_millis() as u64,
            disk_ms = t_disk.as_millis() as u64,
            vmm_ms = (t_launch - t_disk).as_millis() as u64,
            load_identity_ms = (t_identity - t_launch).as_millis() as u64,
            "microVM restored"
        );
        st.last_start = Some(how.into());
        self.save(st)?;
        Ok(())
    }

    /// Waits (at most `limit`) until the guest's CPUs are nearly idle:
    /// busy under [`QUIET_BUSY`] of the time over two consecutive
    /// [`QUIET_WINDOW`]s. Whether it settled.
    async fn quiesce(&self, st: &VmState, limit: Duration) -> bool {
        let exec = VsockExec {
            agent: self.agent(&st.name),
        };
        let sample = || async {
            let out = exec
                .exec(crate::exec::ExecRequest::sh("head -n1 /proc/stat"))
                .await
                .ok()?;
            cpu_jiffies(&String::from_utf8_lossy(&out.stdout))
        };
        let deadline = tokio::time::Instant::now() + limit;
        let mut quiet = 0;
        let Some(mut last) = sample().await else {
            return false;
        };
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(QUIET_WINDOW).await;
            let Some(now) = sample().await else {
                return false;
            };
            let (busy, total) = (now.0 - last.0, now.1 - last.1);
            last = now;
            if total > 0 && (busy as f64) < QUIET_BUSY * total as f64 {
                quiet += 1;
                if quiet >= 2 {
                    return true;
                }
            } else {
                quiet = 0;
            }
        }
        false
    }

    async fn wait_probes(&self, st: &VmState, timeout: Duration) -> Result<()> {
        if !st.probes.is_empty() {
            crate::progress::report(crate::progress::Progress::phase(
                crate::progress::Phase::WaitingForServices,
            ));
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let agent = self.agent(&st.name);
        let ep = self.endpoints_of(st);
        for probe in &st.probes {
            loop {
                let ok = match probe {
                    Probe::Tcp { port } => agent.listening(*port).await.unwrap_or(false),
                    other => crate::probe::check(&ep, other).await.unwrap_or(false),
                };
                if ok {
                    break;
                }
                if !self.owns(&st.name, st.fc_pid) {
                    return Err(VmmError::other(format!(
                        "microVM '{}' exited before {probe:?} passed; see {}",
                        st.name,
                        self.dir(&st.name).join("console.log").display()
                    )));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(VmmError::Timeout {
                        name: st.name.clone(),
                        secs: timeout.as_secs(),
                        detail: format!("probe {probe:?} never passed"),
                    });
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ snapshots

    /// A tmpfs directory with room for `bytes`, where snapshots are written
    /// first (Firecracker syncs its memory file; on a network disk that is
    /// slow, on tmpfs it is a memory copy).
    fn stage_dir(&self, bytes: u64) -> Option<PathBuf> {
        let base = self.cfg.stage.clone().unwrap_or_else(|| {
            #[cfg(unix)]
            let uid = unsafe { libc::getuid() };
            #[cfg(not(unix))]
            let uid = 0;
            PathBuf::from(format!("/dev/shm/cua-microvm-{uid}"))
        });
        std::fs::create_dir_all(&base).ok()?;
        let free = crate::disk::space(&base).map(|s| s.available).unwrap_or(0);
        (free > bytes.saturating_add(bytes / 4)).then_some(base)
    }

    /// Writes a snapshot of running `st` into `target` (memory, device
    /// state, a copy of its disk and `meta.json`). The VM is paused for the
    /// snapshot and the disk copy, and resumed after unless it was paused
    /// already.
    ///
    /// A VM whose memory is known to equal a file ([`VmState::mem_base`])
    /// writes only the pages changed since (a diff snapshot, while paused),
    /// and the full memory file is that file's copy with them laid over it
    /// (after it resumes). Returns the memory file the VM now equals, when
    /// it tracks its writes: the caller records it as its new base.
    async fn snapshot_to(
        &self,
        st: &VmState,
        target: &Path,
        meta_name: &str,
    ) -> Result<Option<MemBase>> {
        if self.cfg.vmm == Vmm::Vz {
            return self
                .snapshot_to_vz(st, target, meta_name)
                .await
                .map(|()| None);
        }
        let agent = self.agent(&st.name);
        let api = self.api(&st.name);
        let partial = target.with_extension(format!("partial-{}", random_hex(4)));
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial)?;
        let tracks = st.mem_base.is_some();
        let base = st.mem_base.clone().filter(MemBase::intact);
        let mem_bytes = st.memory_mb << 20;
        let stage = self
            .stage_dir(mem_bytes)
            .map(|d| d.join(format!("{}-{}", st.name, random_hex(4))));
        let write_dir = stage.clone().unwrap_or_else(|| partial.clone());
        std::fs::create_dir_all(&write_dir)?;
        let cleanup = |e: VmmError| {
            let _ = std::fs::remove_dir_all(&partial);
            if let Some(s) = &stage {
                let _ = std::fs::remove_dir_all(s);
            }
            e
        };
        let started = std::time::Instant::now();
        if !st.paused {
            // Flush the guest's dirty pages: the disk copy is then usable
            // on its own (a cold fork).
            let _ = agent.request(&Request::Sync, Duration::from_secs(20)).await;
        }
        let t_sync = started.elapsed();
        let was_paused = st.paused;
        if !was_paused {
            api.pause().await?;
        }
        let mut t_mem = t_sync;
        let snap = async {
            api.snapshot(
                &write_dir.join("vmstate"),
                &write_dir.join("mem"),
                base.is_some(),
            )
            .await?;
            t_mem = started.elapsed();
            rootfs::copy_sparse(
                &self.dir(&st.name).join("scratch.ext4"),
                &partial.join("scratch.ext4"),
            )
            .await
        }
        .await;
        if !was_paused {
            api.resume().await?;
        }
        let t_paused = started.elapsed();
        snap.map_err(cleanup)?;
        let built = async {
            if let Some(b) = &base {
                // The full memory: the base's copy with the changed pages
                // laid over it.
                rootfs::copy_sparse(&b.path, &partial.join("mem")).await?;
                let (diff, full) = (write_dir.join("mem"), partial.join("mem"));
                tokio::task::spawn_blocking(move || rootfs::overlay_data(&diff, &full))
                    .await
                    .map_err(|e| VmmError::other(e.to_string()))??;
                if write_dir != partial {
                    rootfs::copy_sparse(&write_dir.join("vmstate"), &partial.join("vmstate"))
                        .await?;
                }
            } else if write_dir != partial {
                for f in ["vmstate", "mem"] {
                    rootfs::copy_sparse(&write_dir.join(f), &partial.join(f)).await?;
                }
            }
            Ok::<_, VmmError>(())
        }
        .await;
        if let Some(s) = &stage {
            let _ = std::fs::remove_dir_all(s);
        }
        built.map_err(cleanup)?;
        tracing::debug!(
            name = st.name,
            diff = base.is_some(),
            sync_ms = t_sync.as_millis() as u64,
            memory_ms = (t_mem - t_sync).as_millis() as u64,
            paused_ms = (t_paused - t_sync).as_millis() as u64,
            build_ms = (started.elapsed() - t_paused).as_millis() as u64,
            "microVM snapshot"
        );
        let meta = SnapshotMeta {
            name: meta_name.to_string(),
            source: Some(st.name.clone()),
            image: st.image.clone(),
            base: st.base.clone(),
            base_format: st.base_format,
            cpus: st.cpus,
            memory_mb: st.memory_mb,
            disk_gb: st.disk_gb,
            ports: st.ports.clone(),
            boot: st.boot.clone(),
            probes: st.probes.clone(),
            restrict_network: st.restrict_network,
            egress: st.egress.clone(),
            memory: true,
            created_at: host::now_secs(),
            pinned: false,
        };
        std::fs::write(partial.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
        // Never replace a snapshot another process finished first (VMs may
        // be restoring from it): keep the one that is there. The VM then
        // has no file it equals: its next snapshot is a full one.
        if target.join("meta.json").is_file() {
            let _ = std::fs::remove_dir_all(&partial);
            return Ok(None);
        }
        let _ = std::fs::remove_dir_all(target);
        if let Err(e) = std::fs::rename(&partial, target) {
            let _ = std::fs::remove_dir_all(&partial);
            if !target.join("meta.json").is_file() {
                return Err(e.into());
            }
            return Ok(None);
        }
        Ok(if tracks {
            self.own_base(&st.name, &target.join("mem"))
        } else {
            None
        })
    }

    /// [`Self::snapshot_to`] under `cua-vz`: the framework's saved state of
    /// the paused VM (`vzstate`), an APFS clone of its disk and its machine
    /// identifier. The VM is paused for the save and the clone.
    async fn snapshot_to_vz(&self, st: &VmState, target: &Path, meta_name: &str) -> Result<()> {
        let agent = self.agent(&st.name);
        let api = self.vz_api(&st.name);
        let dir = self.dir(&st.name);
        let partial = target.with_extension(format!("partial-{}", random_hex(4)));
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial)?;
        let started = std::time::Instant::now();
        if !st.paused {
            // Flush the guest's dirty pages: the disk clone is then usable
            // on its own (a cold fork).
            let _ = agent.request(&Request::Sync, Duration::from_secs(20)).await;
        }
        let t_sync = started.elapsed();
        let was_paused = st.paused;
        if !was_paused {
            api.pause().await?;
        }
        let snap = async {
            api.save(&partial.join("vzstate")).await?;
            rootfs::copy_sparse(&dir.join("scratch.ext4"), &partial.join("scratch.ext4")).await?;
            std::fs::copy(dir.join("machine-id"), partial.join("machine-id"))?;
            Ok::<_, VmmError>(())
        }
        .await;
        let t_paused = started.elapsed();
        if !was_paused {
            api.resume().await?;
        }
        if let Err(e) = snap {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(e);
        }
        let state_bytes = std::fs::metadata(partial.join("vzstate"))
            .map(|m| m.len())
            .unwrap_or(0);
        tracing::info!(
            name = st.name,
            sync_ms = t_sync.as_millis() as u64,
            paused_ms = (t_paused - t_sync).as_millis() as u64,
            state_mib = state_bytes >> 20,
            "microVM snapshot"
        );
        let meta = self.meta_of(st, meta_name, true);
        std::fs::write(partial.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
        // Never replace a snapshot another process finished first.
        if target.join("meta.json").is_file() {
            let _ = std::fs::remove_dir_all(&partial);
            return Ok(());
        }
        let _ = std::fs::remove_dir_all(target);
        if let Err(e) = std::fs::rename(&partial, target) {
            let _ = std::fs::remove_dir_all(&partial);
            if !target.join("meta.json").is_file() {
                return Err(e.into());
            }
        }
        Ok(())
    }

    async fn record_golden(&self, st: &VmState, key: &str) -> Result<()> {
        let lock = self.lock_for(&format!("golden:{key}"));
        let Ok(_g) = lock.try_lock() else {
            return Ok(());
        };
        let target = self.golden_dir(key);
        if target.join("meta.json").is_file() {
            return Ok(());
        }
        std::fs::create_dir_all(target.parent().unwrap_or(&self.cfg.root))?;
        // Every VM restored from it would share its cua-spacesd's
        // per-machine secrets unless that cua-spacesd renews them
        // ([`crate::identity_gate`]); a guest awaiting its token still
        // answers `GetCapabilities`.
        if let Some(port) = st.host_ports.get(&crate::identity_gate::SPACESD_PORT) {
            let rev =
                crate::identity_gate::revision_at(&format!("http://127.0.0.1:{port}"), None).await;
            if !crate::identity_gate::renews_identity(rev) {
                return Err(VmmError::other(format!(
                    "this image's cua-spacesd (protocol revision {}) cannot become a new \
                     machine, so its VMs boot cold rather than share one snapshot's keys",
                    rev.map_or("unknown".to_string(), |r| r.to_string())
                )));
            }
        }
        let started = std::time::Instant::now();
        // A golden snapshot comes from a cold boot, which tracks no writes.
        self.snapshot_to(st, &target, key).await?;
        tracing::info!(
            key,
            ms = started.elapsed().as_millis() as u64,
            "recorded a golden microVM snapshot"
        );
        self.gc_golden();
        Ok(())
    }

    fn gc_golden(&self) {
        let Ok(rd) = std::fs::read_dir(self.cfg.root.join("golden")) else {
            return;
        };
        let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = rd
            .flatten()
            .filter(|e| e.path().join("meta.json").is_file())
            .map(|e| {
                let t = crate::disk::last_used(&e.path())
                    .or_else(|| e.metadata().and_then(|m| m.modified()).ok())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (t, e.path())
            })
            .collect();
        dirs.sort();
        while dirs.len() > MAX_GOLDEN {
            let (_, p) = dirs.remove(0);
            let _ = std::fs::remove_dir_all(p);
        }
    }

    /// Drops every golden snapshot (the next start of each configuration
    /// boots cold).
    pub fn clear_golden(&self) -> Result<()> {
        let g = self.cfg.root.join("golden");
        if g.exists() {
            std::fs::remove_dir_all(g)?;
        }
        Ok(())
    }

    /// Reserves `cpus` vCPUs for a boot, or says why the host is full.
    fn reserve_vcpus(&self, cpus: u32) -> Result<VcpuReservation<'_>> {
        use std::sync::atomic::Ordering;
        let limit = vcpu_limit();
        let running: u32 = self
            .all_states()
            .iter()
            .filter(|s| self.running(s))
            .map(|s| s.cpus)
            .sum();
        let booting = self.booting_vcpus.fetch_add(cpus, Ordering::SeqCst);
        let r = VcpuReservation(&self.booting_vcpus, cpus);
        if running + booting + cpus > limit {
            return Err(VmmError::invalid(format!(
                "this host already runs {} microVM vCPUs and {limit} is its limit \
                 (CUA_MICROVM_MAX_VCPUS); stop a Space or give Spaces fewer CPUs",
                running + booting
            )));
        }
        Ok(r)
    }

    fn new_state(&self, name: &str, meta: &SnapshotMeta) -> VmState {
        VmState {
            name: name.to_string(),
            kind: EntryKind::Instance,
            image: meta.image.clone(),
            base: meta.base.clone(),
            base_format: meta.base_format,
            cpus: meta.cpus,
            memory_mb: meta.memory_mb,
            disk_gb: meta.disk_gb,
            ports: meta.ports.clone(),
            host_ports: BTreeMap::new(),
            boot: meta.boot.clone(),
            token: None,
            probes: meta.probes.clone(),
            restrict_network: meta.restrict_network,
            egress: meta.egress.clone(),
            pasta_pid: None,
            fc_pid: None,
            sock_id: None,
            paused: false,
            restore_from: None,
            source: Some(meta.name.clone()),
            labels: BTreeMap::new(),
            last_start: None,
            pool_member: false,
            mem_base: None,
            created_at: host::now_secs(),
        }
    }

    fn meta_of(&self, st: &VmState, name: &str, memory: bool) -> SnapshotMeta {
        SnapshotMeta {
            name: name.to_string(),
            source: Some(st.name.clone()),
            image: st.image.clone(),
            base: st.base.clone(),
            base_format: st.base_format,
            cpus: st.cpus,
            memory_mb: st.memory_mb,
            disk_gb: st.disk_gb,
            ports: st.ports.clone(),
            boot: st.boot.clone(),
            probes: st.probes.clone(),
            restrict_network: st.restrict_network,
            egress: st.egress.clone(),
            memory,
            created_at: host::now_secs(),
            pinned: false,
        }
    }

    /// Builds the boot config of a new VM from its image and spec.
    fn workload(
        spec: &StartSpec,
        run: &crate::runtime::OciRunConfig,
    ) -> Result<(BootConfig, Option<String>)> {
        let injected = spec.inject_spacesd.is_some();
        let argv = if injected {
            // The injected cua-spacesd runs the image's command (or the
            // Space's own) with the image's user and working directory.
            crate::inject::wrapper_command(run, spec.command.as_deref())
        } else {
            match spec.command.as_ref().filter(|c| !c.is_empty()) {
                Some(c) => c.clone(),
                None => run
                    .entrypoint
                    .iter()
                    .chain(run.cmd.iter())
                    .cloned()
                    .collect(),
            }
        };
        if argv.is_empty() {
            return Err(VmmError::invalid(
                "the image has no ENTRYPOINT or CMD; give the Space a command",
            ));
        }
        let mut env: Vec<(String, String)> = run
            .env
            .iter()
            .filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect();
        let mut token = None;
        for (k, v) in &spec.env {
            if TOKEN_VARS.contains(&k.as_str()) {
                if token.is_none() || k == "CUA_SPACESD_TOKEN" {
                    token = Some(v.clone());
                }
                continue;
            }
            env.retain(|(key, _)| key != k);
            env.push((k.clone(), v.clone()));
        }
        if token.is_some() || spec.pool_member {
            // cua-spacesd follows the token file the init writes (and
            // rewrites after every restore). A pool member has no token
            // yet but boots the same way, so it shares the golden snapshot
            // of Spaces of its shape and its claim's token takes effect.
            env.retain(|(k, _)| k != "CUA_ENV_AWAIT_TOKEN_FILE");
            env.push(("CUA_ENV_AWAIT_TOKEN_FILE".into(), "1".into()));
        }
        Ok((
            BootConfig {
                argv,
                env,
                workdir: run.working_dir.clone().filter(|w| !w.is_empty()),
                // The injected supervisor starts as root and drops to the
                // image's user for the command.
                user: run
                    .user
                    .clone()
                    .filter(|u| !u.is_empty())
                    .filter(|_| !injected),
                identity: None,
            },
            token,
        ))
    }
}

#[async_trait]
impl Runtime for MicroVmRuntime {
    fn kind(&self) -> BackendKind {
        self.cfg.vmm.backend()
    }

    async fn ensure_base(&self, image: &ImageSource, base_name: &str) -> Result<CheckpointInfo> {
        validate_name(base_name)?;
        let ImageSource::Oci { reference } = image else {
            return Err(VmmError::invalid("microVM bases are OCI images"));
        };
        let (base, run) = self
            .ensure_rootfs(reference, None, self.base_format(), None)
            .await?;
        if self.load(base_name).is_err() {
            let spec = StartSpec::new(base_name, image.clone());
            let (boot, _) = Self::workload(&spec, &run)?;
            let meta = SnapshotMeta {
                name: base_name.into(),
                source: None,
                image: reference.clone(),
                base,
                base_format: self.base_format(),
                cpus: spec.cpus,
                memory_mb: spec.memory_mb,
                disk_gb: DEFAULT_DISK_GB,
                ports: vec![],
                boot,
                probes: vec![],
                restrict_network: false,
                egress: None,
                memory: false,
                created_at: host::now_secs(),
                pinned: false,
            };
            let mut st = self.new_state(base_name, &meta);
            st.kind = EntryKind::Base;
            st.source = Some(reference.clone());
            self.save(&st)?;
        }
        Ok(CheckpointInfo::now(
            base_name,
            self.cfg.vmm.backend(),
            Some(reference.clone()),
        ))
    }

    async fn start(&self, spec: &StartSpec) -> Result<Instance> {
        validate_name(&spec.name)?;
        if let Some(e) = unsupported_host(self.cfg.vmm) {
            return Err(e);
        }
        reject_sidecars(self.cfg.vmm.backend(), spec)?;
        crate::types::reject_mounts(self.cfg.vmm.backend(), spec)?;
        crate::egress::check(self.cfg.vmm.backend(), &spec.egress)?;
        reject_gpu(self.cfg.vmm.backend(), spec, &[])?;
        if spec.arch.is_some_and(|a| a != Arch::host()) {
            return Err(VmmError::invalid(format!(
                "a microVM runs the host's architecture ({}); this image is {}",
                Arch::host().oci(),
                spec.effective_arch().oci()
            )));
        }
        let lock = self.lock_for(&spec.name);
        let _g = lock.lock().await;
        if let Ok(mut st) = self.load(&spec.name) {
            if st.kind == EntryKind::Base {
                return Err(VmmError::invalid(format!(
                    "{} is a base, not a VM",
                    spec.name
                )));
            }
            if self.running(&st) {
                if st.paused {
                    self.ctl(&st.name).resume().await?;
                    st.paused = false;
                    self.save(&st)?;
                }
                return Ok(self.instance(&st));
            }
            // Ports and probes asked for now apply from this boot on.
            for p in &spec.ports {
                if !st.ports.contains(p) {
                    st.ports.push(*p);
                }
            }
            if !spec.probes.is_empty() {
                st.probes = spec.probes.clone();
            }
            if let Some(t) = TOKEN_VARS.iter().find_map(|k| spec.env.get(*k)) {
                st.token = Some(t.clone());
            }
            // A kept policy wins (`set_egress` changed it since).
            if st.egress.is_none() && !spec.egress.is_empty() {
                st.egress = Some(spec.egress.clone());
            }
            self.kill(&mut st);
            let _vcpus = self.reserve_vcpus(st.cpus)?;
            if let Err(e) = self.boot(&mut st, false, spec.ready_timeout).await {
                self.kill(&mut st);
                let _ = self.save(&st);
                return Err(e);
            }
            return Ok(self.instance(&st));
        }
        let reference = match &spec.image {
            ImageSource::Oci { reference } => reference.clone(),
            ImageSource::Existing => return Err(VmmError::NotFound(spec.name.clone())),
            ImageSource::Disk { .. } => {
                return Err(VmmError::invalid(
                    "a microVM boots an OCI rootfs image, not a disk file (use --kind vm)",
                ));
            }
        };
        let (base, run) = self
            .ensure_rootfs(
                &reference,
                spec.registry_auth.as_ref(),
                self.base_format(),
                spec.inject_spacesd.as_deref(),
            )
            .await?;
        let (boot, token) = Self::workload(spec, &run)?;
        let mut st = VmState {
            name: spec.name.clone(),
            kind: EntryKind::Instance,
            image: reference.clone(),
            base,
            base_format: self.base_format(),
            cpus: spec.cpus.max(1),
            memory_mb: spec.memory_mb.max(256),
            disk_gb: spec.disk_size_gb.unwrap_or(DEFAULT_DISK_GB).max(1),
            ports: spec.ports.clone(),
            host_ports: BTreeMap::new(),
            boot,
            token,
            probes: spec.probes.clone(),
            restrict_network: spec.restrict_network,
            egress: (!spec.egress.is_empty()).then(|| spec.egress.clone()),
            pasta_pid: None,
            fc_pid: None,
            sock_id: None,
            paused: false,
            restore_from: None,
            source: Some(reference),
            labels: spec.labels.clone(),
            last_start: None,
            pool_member: false,
            mem_base: None,
            created_at: host::now_secs(),
        };
        let _vcpus = self.reserve_vcpus(st.cpus)?;
        let dir = self.dir(&st.name);
        std::fs::create_dir_all(&dir)?;
        st.pool_member = spec.pool_member;
        let result = async {
            self.save(&st)?;
            self.boot(&mut st, true, spec.ready_timeout).await?;
            if st.pool_member {
                // A member waits paused: no CPU, and no memory beyond the
                // pages its restore touched (the rest is the golden
                // snapshot's file, shared by every member in the page cache).
                self.ctl(&st.name).pause().await?;
                st.paused = true;
                self.save(&st)?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            self.kill(&mut st);
            // Keep the console for `why did it fail`.
            let logs = self.cfg.root.join("logs");
            let kept = logs.join(format!("{}.log", st.name));
            let saved = std::fs::create_dir_all(&logs).is_ok()
                && std::fs::rename(dir.join("console.log"), &kept).is_ok();
            let _ = std::fs::remove_dir_all(&dir);
            return Err(if saved {
                VmmError::other(format!("{e} (console: {})", kept.display()))
            } else {
                e
            });
        }
        Ok(self.instance(&st))
    }

    async fn stop(&self, name: &str) -> Result<()> {
        let lock = self.lock_for(name);
        let _g = lock.lock().await;
        let mut st = self.load(name)?;
        if self.running(&st) {
            let api = self.ctl(name);
            if st.paused {
                let _ = api.resume().await;
            }
            let grace = self.cfg.graceful_stop;
            let _ = self
                .agent(name)
                .request(
                    &Request::Shutdown {
                        grace_secs: grace.as_secs(),
                    },
                    Duration::from_secs(3),
                )
                .await;
            if !self.wait_exit(&st, grace + Duration::from_secs(5)).await {
                tracing::warn!(name, "microVM did not power off; killing it");
            }
        }
        self.kill(&mut st);
        self.save(&st)
    }

    async fn suspend(&self, name: &str) -> Result<()> {
        let lock = self.lock_for(name);
        let _g = lock.lock().await;
        let mut st = self.load(name)?;
        if !self.running(&st) {
            return Err(VmmError::invalid(format!("{name} is not running")));
        }
        if !st.paused {
            self.ctl(name).pause().await?;
            st.paused = true;
            self.save(&st)?;
        }
        Ok(())
    }

    async fn resume(&self, name: &str) -> Result<Instance> {
        {
            let lock = self.lock_for(name);
            let _g = lock.lock().await;
            let mut st = self.load(name)?;
            if self.running(&st) {
                if st.paused {
                    self.ctl(name).resume().await?;
                    st.paused = false;
                    self.save(&st)?;
                }
                return Ok(self.instance(&st));
            }
        }
        // Stopped: boot it again.
        self.start(&StartSpec::new(name, ImageSource::Existing))
            .await
    }

    async fn claim(
        &self,
        member: &str,
        new_name: &str,
        identity: &ClaimIdentity,
    ) -> Result<Instance> {
        validate_name(new_name)?;
        // Both names locked, in a fixed order.
        let mut names = [member, new_name];
        names.sort_unstable();
        let first = self.lock_for(names[0]);
        let _g1 = first.lock().await;
        let second = (names[0] != names[1]).then(|| self.lock_for(names[1]));
        let _g2 = match &second {
            Some(l) => Some(l.lock().await),
            None => None,
        };
        let mut st = self.load(member)?;
        if st.kind != EntryKind::Instance || !st.pool_member || !self.running(&st) {
            return Err(VmmError::invalid(format!(
                "{member} is not a running warm-pool member"
            )));
        }
        if member != new_name {
            if self.dir(new_name).exists() {
                return Err(VmmError::AlreadyExists(new_name.into()));
            }
            // The VMM's working directory, sockets and console move with
            // the directory: it keeps running under its new name.
            std::fs::rename(self.dir(member), self.dir(new_name))?;
            st.name = new_name.to_string();
            if let Some(b) = &mut st.mem_base
                && let Ok(rest) = b.path.strip_prefix(self.dir(member))
            {
                b.path = self.dir(new_name).join(rest);
            }
        }
        st.pool_member = false;
        if identity.token.is_some() {
            st.token = identity.token.clone();
        }
        self.save(&st)?;
        let bound = async {
            let api = self.ctl(&st.name);
            if st.paused {
                api.resume().await?;
                st.paused = false;
            }
            // Its own token, hostname, clock, entropy and machine id. The
            // guest's address is the same in every VM (each has its own
            // network namespace), so there is no lease to renew.
            let mut id = self.identity(&st);
            if let Some(h) = identity.hostname.clone().filter(|h| !h.trim().is_empty()) {
                id.hostname = Some(h);
            }
            self.agent(&st.name)
                .request(&Request::Identity(id), Duration::from_secs(10))
                .await?;
            self.save(&st)
        }
        .await;
        if let Err(e) = bound {
            // Never hand out a member that may still hold another identity.
            self.kill(&mut st);
            let _ = self.save(&st);
            return Err(e);
        }
        Ok(self.instance(&st))
    }

    async fn fork(&self, source: &str, new_name: &str) -> Result<()> {
        validate_name(new_name)?;
        if self.dir(new_name).join("state.json").exists() {
            return Err(VmmError::AlreadyExists(new_name.into()));
        }
        let new_dir = self.dir(new_name);
        let ckpt = self.checkpoint_dir(source);
        let made = async {
            if ckpt.join("meta.json").is_file() {
                let meta: SnapshotMeta =
                    serde_json::from_slice(&std::fs::read(ckpt.join("meta.json"))?)?;
                let mut st = self.new_state(new_name, &meta);
                std::fs::create_dir_all(&new_dir)?;
                if meta.memory {
                    st.restore_from = Some(ckpt.clone());
                } else {
                    rootfs::copy_sparse(&ckpt.join("scratch.ext4"), &new_dir.join("scratch.ext4"))
                        .await?;
                }
                return self.save(&st);
            }
            let src_lock = self.lock_for(source);
            let _g = src_lock.lock().await;
            let src = self.load(source)?;
            match src.kind {
                EntryKind::Base => {
                    let meta = self.meta_of(&src, source, false);
                    let st = self.new_state(new_name, &meta);
                    std::fs::create_dir_all(&new_dir)?;
                    self.fresh_scratch(&new_dir.join("scratch.ext4"), st.disk_gb)
                        .await?;
                    self.save(&st)
                }
                EntryKind::Instance if self.running(&src) => {
                    std::fs::create_dir_all(&new_dir)?;
                    let snap = new_dir.join("snap");
                    let mut src = src;
                    src.mem_base = self.snapshot_to(&src, &snap, source).await?;
                    self.save(&src)?;
                    // No token of the source's: the fork gets its own when
                    // it starts, and until then none opens it.
                    let mut st = self.new_state(new_name, &self.meta_of(&src, source, true));
                    st.restore_from = Some(snap);
                    self.save(&st)
                }
                EntryKind::Instance => {
                    std::fs::create_dir_all(&new_dir)?;
                    rootfs::copy_sparse(
                        &self.dir(source).join("scratch.ext4"),
                        &new_dir.join("scratch.ext4"),
                    )
                    .await?;
                    let st = self.new_state(new_name, &self.meta_of(&src, source, false));
                    self.save(&st)
                }
            }
        }
        .await;
        if made.is_err() {
            let _ = std::fs::remove_dir_all(&new_dir);
        }
        made
    }

    /// In the VM's network namespace: at once while it runs (the chain is
    /// refilled, `nsenter` into pasta's user and network namespaces), at
    /// its next boot otherwise.
    async fn set_egress(
        &self,
        name: &str,
        policy: &crate::egress::EgressPolicy,
    ) -> Result<crate::egress::Enforcement> {
        let rules = crate::egress::check(self.cfg.vmm.backend(), policy)?;
        let lock = self.lock_for(name);
        let _g = lock.lock().await;
        let mut st = self.load(name)?;
        if st.kind != EntryKind::Instance {
            return Err(VmmError::invalid(format!("{name} is not a VM")));
        }
        if self.cfg.vmm == Vmm::Vz {
            // The gateway reloads its rules file (at once while the VM
            // runs; it is read at the next start otherwise). A VM without
            // network has no gateway and gets none.
            if !st.restrict_network {
                crate::netgw::write_rules(&self.dir(name), &rules)?;
            }
        } else if self.running(&st) {
            let pid = st
                .fc_pid
                .ok_or_else(|| VmmError::other(format!("{name}: no firecracker process")))?;
            crate::egress::ns_apply(
                pid,
                &crate::egress::ns_script(&rules, crate::egress::NsHook::Tap(TAP), None),
            )
            .await?;
        }
        st.egress = Some(policy.clone());
        self.save(&st)?;
        Ok(crate::egress::Enforcement::Host)
    }

    async fn egress(&self, name: &str) -> Result<crate::egress::EgressPolicy> {
        Ok(self.load(name)?.egress.unwrap_or_default())
    }

    async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
        validate_name(checkpoint)?;
        let target = self.checkpoint_dir(checkpoint);
        if target.exists() {
            return Err(VmmError::AlreadyExists(checkpoint.into()));
        }
        std::fs::create_dir_all(target.parent().unwrap_or(&self.cfg.root))?;
        let lock = self.lock_for(name);
        let _g = lock.lock().await;
        let st = self.load(name)?;
        if st.kind != EntryKind::Instance {
            return Err(VmmError::invalid(format!("{name} is a base, not a VM")));
        }
        if self.running(&st) {
            let mut st = st;
            st.mem_base = self.snapshot_to(&st, &target, checkpoint).await?;
            self.save(&st)?;
        } else {
            let partial = target.with_extension("partial");
            std::fs::create_dir_all(&partial)?;
            rootfs::copy_sparse(
                &self.dir(name).join("scratch.ext4"),
                &partial.join("scratch.ext4"),
            )
            .await?;
            std::fs::write(
                partial.join("meta.json"),
                serde_json::to_vec_pretty(&self.meta_of(&st, checkpoint, false))?,
            )?;
            std::fs::rename(&partial, &target)?;
        }
        Ok(CheckpointInfo::now(
            checkpoint,
            self.cfg.vmm.backend(),
            Some(name.into()),
        ))
    }

    async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
        validate_name(checkpoint)?;
        let d = self.checkpoint_dir(checkpoint);
        if !d.exists() {
            return Err(VmmError::NotFound(checkpoint.into()));
        }
        if self.read_meta(&d).is_some_and(|m| m.pinned) {
            return Err(VmmError::invalid(format!(
                "checkpoint {checkpoint} is pinned (a fork source to keep); unpin it first"
            )));
        }
        // Running forks keep the memory file they mapped (or, under
        // cua-vz, the memory they restored).
        std::fs::remove_dir_all(d)?;
        Ok(())
    }

    async fn pin_checkpoint(&self, checkpoint: &str, pinned: bool) -> Result<()> {
        MicroVmRuntime::pin_checkpoint(self, checkpoint, pinned)
    }

    async fn list(&self) -> Result<Vec<InstanceSummary>> {
        Ok(self
            .all_states()
            .into_iter()
            .filter(|s| s.kind == EntryKind::Instance)
            .map(|s| InstanceSummary {
                status: self.status_of(&s),
                name: s.name,
                backend: self.cfg.vmm.backend(),
            })
            .collect())
    }

    async fn status(&self, name: &str) -> Result<Status> {
        let st = self.load(name)?;
        Ok(self.status_of(&st))
    }

    async fn delete(&self, name: &str) -> Result<()> {
        let lock = self.lock_for(name);
        let _g = lock.lock().await;
        let mut st = self.load(name)?;
        self.kill(&mut st);
        let _ = self.wait_exit(&st, Duration::from_secs(5)).await;
        std::fs::remove_dir_all(self.dir(name))?;
        Ok(())
    }

    async fn endpoints(&self, name: &str) -> Result<Endpoints> {
        let st = self.load(name)?;
        Ok(self.endpoints_of(&st))
    }

    async fn guest_tcp_listening(&self, name: &str, guest_port: u16) -> Result<Option<bool>> {
        let st = self.load(name)?;
        if !self.running(&st) || st.paused {
            return Ok(Some(false));
        }
        Ok(self.agent(name).listening(guest_port).await.ok())
    }

    async fn guest_exec(&self, name: &str) -> Result<Option<Arc<dyn GuestExec>>> {
        self.load(name)?;
        Ok(Some(Arc::new(VsockExec {
            agent: self.agent(name),
        })))
    }
}

impl MicroVmRuntime {
    fn read_meta(&self, dir: &Path) -> Option<SnapshotMeta> {
        serde_json::from_slice(&std::fs::read(dir.join("meta.json")).ok()?).ok()
    }

    /// Pins (`pinned`) or unpins checkpoint `checkpoint`: a pinned one is a
    /// fork source to keep, which [`Runtime::delete_checkpoint`] refuses
    /// until it is unpinned.
    pub fn pin_checkpoint(&self, checkpoint: &str, pinned: bool) -> Result<()> {
        validate_name(checkpoint)?;
        let d = self.checkpoint_dir(checkpoint);
        let mut meta = self
            .read_meta(&d)
            .ok_or_else(|| VmmError::NotFound(checkpoint.into()))?;
        meta.pinned = pinned;
        let tmp = d.join("meta.json.partial");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&meta)?)?;
        std::fs::rename(tmp, d.join("meta.json"))?;
        Ok(())
    }
}

/// Checkpoints stored by the microVM backend (name, source, has memory).
pub fn list_checkpoints(rt: &MicroVmRuntime) -> Vec<SnapshotMeta> {
    let Ok(rd) = std::fs::read_dir(rt.cfg.root.join("checkpoints")) else {
        return vec![];
    };
    rd.flatten()
        .filter_map(|e| std::fs::read(e.path().join("meta.json")).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> crate::runtime::OciRunConfig {
        crate::runtime::OciRunConfig {
            entrypoint: vec![],
            cmd: vec!["/opt/cua/desktop/entrypoint.sh".into()],
            env: vec!["PATH=/usr/bin:/bin".into(), "DISPLAY=:1".into()],
            working_dir: None,
            user: None,
        }
    }

    #[test]
    fn the_token_leaves_the_environment_for_the_token_file() {
        let spec = StartSpec::new("s", ImageSource::oci("x"))
            .env("CUA_ENV_TOKEN", "tok")
            .env("DISPLAY", ":2");
        let (boot, token) = MicroVmRuntime::workload(&spec, &run()).unwrap();
        assert_eq!(token.as_deref(), Some("tok"));
        assert_eq!(boot.argv, ["/opt/cua/desktop/entrypoint.sh"]);
        assert!(
            boot.env
                .iter()
                .all(|(k, _)| !TOKEN_VARS.contains(&k.as_str()))
        );
        assert!(
            boot.env.contains(&("DISPLAY".into(), ":2".into())),
            "the spec wins"
        );
        assert!(
            boot.env
                .contains(&("CUA_ENV_AWAIT_TOKEN_FILE".into(), "1".into()))
        );
        let plain = StartSpec::new("s", ImageSource::oci("x")).command(["/bin/sleep", "1"]);
        let (boot, token) = MicroVmRuntime::workload(&plain, &run()).unwrap();
        assert!(token.is_none());
        assert_eq!(boot.argv, ["/bin/sleep", "1"]);
        assert!(
            boot.env
                .iter()
                .all(|(k, _)| k != "CUA_ENV_AWAIT_TOKEN_FILE")
        );
        let none = crate::runtime::OciRunConfig::default();
        assert!(
            MicroVmRuntime::workload(&StartSpec::new("s", ImageSource::oci("x")), &none).is_err()
        );
    }

    #[test]
    fn an_injected_spacesd_runs_the_images_command_under_the_supervisor() {
        let image = crate::runtime::OciRunConfig {
            entrypoint: vec!["docker-entrypoint.sh".into()],
            cmd: vec!["node".into()],
            env: vec!["PATH=/usr/bin".into()],
            working_dir: Some("/app".into()),
            user: Some("node".into()),
        };
        let mut spec = StartSpec::new("s", ImageSource::oci("node:20")).env("CUA_ENV_TOKEN", "tok");
        spec.inject_spacesd = Some("/host/cua-spacesd-linux-aarch64".into());
        let (boot, token) = MicroVmRuntime::workload(&spec, &image).unwrap();
        assert_eq!(token.as_deref(), Some("tok"));
        assert_eq!(&boot.argv[..2], [crate::inject::GUEST_PATH, "supervise"]);
        assert_eq!(
            &boot.argv[boot.argv.len() - 3..],
            ["--", "docker-entrypoint.sh", "node"]
        );
        assert!(boot.argv.windows(2).any(|w| w == ["--user", "node"]));
        assert_eq!(
            boot.user, None,
            "the supervisor starts as root and drops itself"
        );
        assert_eq!(boot.workdir.as_deref(), Some("/app"));
        assert!(
            boot.env
                .iter()
                .all(|(k, _)| !TOKEN_VARS.contains(&k.as_str()))
        );
        // The Space's own command is served too, and ends the Space.
        let mut own = StartSpec::new("s", ImageSource::oci("node:20")).command(["sleep", "5"]);
        own.inject_spacesd = spec.inject_spacesd.clone();
        let (boot, _) = MicroVmRuntime::workload(&own, &image).unwrap();
        assert!(boot.argv.windows(2).any(|w| w == ["--on-exit", "exit"]));
        assert_eq!(&boot.argv[boot.argv.len() - 3..], ["--", "sleep", "5"]);
        // An image with no command at all still serves.
        let none = crate::runtime::OciRunConfig::default();
        let (boot, _) = MicroVmRuntime::workload(&spec, &none).unwrap();
        assert_eq!(boot.argv.last().map(String::as_str), Some("serve"));
    }

    #[test]
    fn the_namespace_script_forwards_ports_and_nats_egress() {
        let none = crate::egress::Rules::default();
        let s = net_script(&[3211, 8080], false, true, &none);
        assert!(s.contains("--dport 3211 -j DNAT --to-destination 172.16.0.2:3211"));
        assert!(s.contains("--dport 8080 -j DNAT"));
        assert!(s.contains("-s 172.16.0.2/32 ! -d 172.16.0.1/30 -j MASQUERADE"));
        assert!(s.contains("--config-file vm.json"));
        assert!(!s.contains("-j DROP\n-A INPUT -s"), "no restriction");
        assert!(s.contains("-A INPUT -m conntrack --ctstate INVALID -j DROP"));
        assert!(s.contains("nf_conntrack_tcp_be_liberal"));
        // The egress chain and its jumps are always there (empty without
        // a policy), so a policy set later only refills the chain.
        assert!(s.contains(":CUA_EGRESS - [0:0]\n"), "{s}");
        assert!(s.contains("-A FORWARD -i tap0 -j CUA_EGRESS\n-A INPUT -i tap0 -j CUA_EGRESS\n"));
        assert!(!s.contains("-A CUA_EGRESS"), "{s}");
        let r = net_script(&[], true, false, &none);
        assert!(!r.contains("-s 172.16.0.2/32 ! -d"), "no egress NAT");
        assert!(r.contains("-A FORWARD -s 172.16.0.2/32 -j DROP"));
        // network=none drops before the egress chain could accept.
        assert!(
            r.find("-A FORWARD -s 172.16.0.2/32 -j DROP").unwrap()
                < r.find("-A FORWARD -i tap0 -j CUA_EGRESS").unwrap()
        );
        // A policy is in the chain before firecracker runs (the first
        // instruction of the guest), judged by the original destination.
        let p = crate::egress::EgressPolicy {
            allow: vec!["1.1.1.1".into()],
            deny: vec!["all".into()],
        };
        let e = net_script(&[3211], false, true, &p.parse().unwrap());
        assert!(
            e.contains("-A CUA_EGRESS -m conntrack --ctdir REPLY -j ACCEPT"),
            "{e}"
        );
        assert!(
            e.contains("-A CUA_EGRESS -m conntrack --ctorigdst 1.1.1.1/32 -j ACCEPT"),
            "{e}"
        );
        assert!(
            e.contains("-A CUA_EGRESS -m conntrack --ctorigdst 0.0.0.0/0 -j DROP"),
            "{e}"
        );
        assert!(e.find("CUA_EGRESS").unwrap() < e.find("exec \"$CUA_FIRECRACKER_BIN\"").unwrap());
        assert!(!r.contains("--config-file"));
        assert!(r.contains("exec \"$CUA_FIRECRACKER_BIN\" --api-sock api.sock\n"));
    }

    #[test]
    fn busy_time_comes_from_proc_stat() {
        let s = "cpu  100 5 50 1000 20 3 2 0 0 0\ncpu0 1 2 3 4\n";
        assert_eq!(cpu_jiffies(s), Some((160, 1180)));
        assert_eq!(cpu_jiffies("intr 1 2"), None);
    }

    #[test]
    fn boot_args_configure_the_guest_network() {
        let a = boot_args();
        assert!(
            a.contains("ip=172.16.0.2::172.16.0.1:255.255.255.252::eth0:off"),
            "{a}"
        );
        assert!(a.contains("reboot=k"));
    }

    /// A resolver that hands out one prepared layer.
    struct OneLayer {
        layer: PathBuf,
    }

    #[async_trait::async_trait]
    impl RootfsResolver for OneLayer {
        async fn resolve_rootfs(
            &self,
            _reference: &str,
            _arch: Arch,
            _creds: Option<&RegistryCredentials>,
        ) -> Result<crate::runtime::RootfsLayers> {
            Ok(crate::runtime::RootfsLayers {
                digest: "sha256:0123456789abcdef".into(),
                layers: vec![self.layer.clone()],
                config: crate::runtime::OciRunConfig {
                    cmd: vec!["/bin/sh".into()],
                    ..Default::default()
                },
            })
        }
    }

    /// An image without cua-spacesd gets the binary as one more layer of
    /// its rootfs, in a base of its own; the image as it is keeps its base.
    #[tokio::test]
    async fn an_injected_rootfs_holds_the_binary_in_a_base_of_its_own() {
        use backhand::{FilesystemReader, InnerNode};
        let d = tempfile::tempdir().unwrap();
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Regular);
        h.set_size(5);
        h.set_mode(0o644);
        b.append_data(&mut h, "etc/os-release", &b"alpine"[..5])
            .unwrap();
        let layer = d.path().join("l0.tar");
        std::fs::write(&layer, b.into_inner().unwrap()).unwrap();
        let binary = d.path().join("cua-spacesd-linux-x");
        std::fs::write(&binary, b"static-spacesd").unwrap();
        let rt = MicroVmRuntime::new(MicroVmConfig {
            root: d.path().join("root"),
            resolver: Some(Arc::new(OneLayer { layer })),
            // The in-process squashfs writer, as on a Mac.
            vmm: Vmm::Vz,
            base_format: BaseFormat::Squashfs,
            ..Default::default()
        });
        let (plain, _) = rt
            .ensure_rootfs("alpine:3.20", None, BaseFormat::Squashfs, None)
            .await
            .unwrap();
        let (injected, run) = rt
            .ensure_rootfs("alpine:3.20", None, BaseFormat::Squashfs, Some(&binary))
            .await
            .unwrap();
        assert_eq!(plain, "0123456789abcdef");
        assert!(injected.starts_with("0123456789abcdef-sd"), "{injected}");
        assert_eq!(run.cmd, ["/bin/sh"]);
        // Built once: asking again finds it.
        let (again, _) = rt
            .ensure_rootfs("alpine:3.20", None, BaseFormat::Squashfs, Some(&binary))
            .await
            .unwrap();
        assert_eq!(again, injected);

        let fs = FilesystemReader::from_reader(std::io::BufReader::new(
            std::fs::File::open(rt.base_dir(&injected).join(BaseFormat::Squashfs.file())).unwrap(),
        ))
        .unwrap();
        let node = |p: &str| {
            fs.files()
                .find(|n| n.fullpath == Path::new(p))
                .unwrap_or_else(|| panic!("{p} is missing"))
        };
        let InnerNode::File(f) = &node("/.cua/bin/cua-spacesd").inner else {
            panic!("not a file")
        };
        let mut got = Vec::new();
        std::io::Read::read_to_end(&mut fs.file(f).reader(), &mut got).unwrap();
        assert_eq!(got, b"static-spacesd");
        assert_eq!(node("/.cua/bin/cua-spacesd").header.permissions, 0o555);
        assert_eq!(node("/.cua/bin").header.permissions, 0o755);
        assert!(
            fs.files()
                .any(|n| n.fullpath == Path::new("/etc/os-release"))
        );
        // The image as it is has no such file.
        let plain_fs = FilesystemReader::from_reader(std::io::BufReader::new(
            std::fs::File::open(rt.base_dir(&plain).join(BaseFormat::Squashfs.file())).unwrap(),
        ))
        .unwrap();
        assert!(!plain_fs.files().any(|n| n.fullpath.starts_with("/.cua")));
    }

    #[tokio::test]
    async fn state_round_trips_and_missing_names_are_not_found() {
        let d = tempfile::tempdir().unwrap();
        let rt = MicroVmRuntime::new(MicroVmConfig {
            root: d.path().to_path_buf(),
            ..Default::default()
        });
        assert!(matches!(
            rt.status("nope").await,
            Err(VmmError::NotFound(_))
        ));
        let meta = SnapshotMeta {
            name: "c".into(),
            source: None,
            image: "img".into(),
            base: "abc".into(),
            base_format: BaseFormat::Squashfs,
            cpus: 2,
            memory_mb: 1024,
            disk_gb: 4,
            ports: vec![3211],
            boot: BootConfig {
                argv: vec!["/bin/true".into()],
                ..Default::default()
            },
            probes: vec![Probe::tcp(3211)],
            restrict_network: false,
            egress: None,
            memory: false,
            created_at: 0,
            pinned: false,
        };
        let st = rt.new_state("vm1", &meta);
        rt.save(&st).unwrap();
        assert_eq!(rt.status("vm1").await.unwrap(), Status::Stopped);
        let list = rt.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].backend, rt.kind());
        let ep = rt.endpoints("vm1").await.unwrap();
        assert!(ep.serial_log.unwrap().ends_with("vm1/console.log"));
        assert!(matches!(
            rt.fork("missing", "vm2").await,
            Err(VmmError::NotFound(_))
        ));
        // A golden key depends on what shapes the guest, not on its name.
        let initrd = PathBuf::from("/x/0123.cpio");
        let mut other = rt.new_state("vm9", &meta);
        assert_eq!(rt.golden_key(&st, &initrd), rt.golden_key(&other, &initrd));
        other.memory_mb = 2048;
        assert_ne!(rt.golden_key(&st, &initrd), rt.golden_key(&other, &initrd));
        rt.delete("vm1").await.unwrap();
        assert!(rt.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_pinned_checkpoint_is_kept_until_unpinned() {
        let d = tempfile::tempdir().unwrap();
        let rt = MicroVmRuntime::new(MicroVmConfig {
            root: d.path().to_path_buf(),
            ..Default::default()
        });
        let st = rt.new_state(
            "vm1",
            &SnapshotMeta {
                name: "c".into(),
                source: None,
                image: "img".into(),
                base: "abc".into(),
                base_format: BaseFormat::Squashfs,
                cpus: 1,
                memory_mb: 512,
                disk_gb: 1,
                ports: vec![],
                boot: BootConfig::default(),
                probes: vec![],
                restrict_network: false,
                egress: None,
                memory: false,
                created_at: 0,
                pinned: false,
            },
        );
        rt.save(&st).unwrap();
        std::fs::write(rt.dir("vm1").join("scratch.ext4"), b"disk").unwrap();
        // A stopped VM's checkpoint is its disk.
        rt.checkpoint("vm1", "world").await.unwrap();
        assert!(matches!(
            rt.pin_checkpoint("missing", true),
            Err(VmmError::NotFound(_))
        ));
        rt.pin_checkpoint("world", true).unwrap();
        let e = rt.delete_checkpoint("world").await.unwrap_err();
        assert!(matches!(e, VmmError::Invalid(_)), "{e}");
        assert!(e.to_string().contains("pinned"), "{e}");
        assert!(list_checkpoints(&rt)[0].pinned);
        rt.pin_checkpoint("world", false).unwrap();
        rt.delete_checkpoint("world").await.unwrap();
        assert!(list_checkpoints(&rt).is_empty());
    }

    #[tokio::test]
    async fn only_a_running_pool_member_can_be_claimed() {
        let d = tempfile::tempdir().unwrap();
        let rt = MicroVmRuntime::new(MicroVmConfig {
            root: d.path().to_path_buf(),
            ..Default::default()
        });
        let id = ClaimIdentity {
            token: Some("t".repeat(32)),
            hostname: None,
        };
        assert!(matches!(
            rt.claim("missing", "space", &id).await,
            Err(VmmError::NotFound(_))
        ));
        let meta = SnapshotMeta {
            name: "c".into(),
            source: None,
            image: "img".into(),
            base: "abc".into(),
            base_format: BaseFormat::Ext4Dax,
            cpus: 1,
            memory_mb: 512,
            disk_gb: 1,
            ports: vec![],
            boot: BootConfig::default(),
            probes: vec![],
            restrict_network: false,
            egress: None,
            memory: false,
            created_at: 0,
            pinned: false,
        };
        // A Space (not a member), and a stopped member: both refused, and
        // neither is renamed.
        let mut st = rt.new_state("plain", &meta);
        rt.save(&st).unwrap();
        assert!(matches!(
            rt.claim("plain", "space", &id).await,
            Err(VmmError::Invalid(_))
        ));
        st.name = "member".into();
        st.pool_member = true;
        rt.save(&st).unwrap();
        assert!(matches!(
            rt.claim("member", "space", &id).await,
            Err(VmmError::Invalid(_))
        ));
        assert!(rt.load("member").is_ok());
        assert!(matches!(rt.load("space"), Err(VmmError::NotFound(_))));
    }

    #[test]
    fn a_pool_member_boots_awaiting_its_token() {
        let mut spec = StartSpec::new("m", ImageSource::oci("x"));
        spec.pool_member = true;
        let (boot, token) = MicroVmRuntime::workload(&spec, &run()).unwrap();
        assert!(token.is_none());
        // The same boot as a Space with a token: they share golden
        // snapshots, and the claim's token file is what cua-spacesd follows.
        let with_token = StartSpec::new("s", ImageSource::oci("x")).env("CUA_ENV_TOKEN", "tok");
        assert_eq!(
            MicroVmRuntime::workload(&with_token, &run()).unwrap().0,
            boot
        );
    }
}
