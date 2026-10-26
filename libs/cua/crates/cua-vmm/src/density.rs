//! VM density on one host: what running VMs really use, and whether one
//! more fits.
//!
//! A VM's guest RAM is reserved (`memory_mb`) but used lazily, and VMs
//! restored from one memory snapshot (golden warm starts, forks of a
//! checkpoint) map its RAM file copy-on-write: the pages none of them wrote
//! are one copy in the host page cache. So reservations overstate what a
//! host of such VMs uses, often several times over, and admission counts
//! what is actually resident instead:
//!
//! * each VM's **private** memory: its process's own pages (macOS
//!   `phys_footprint`, which leaves out clean file-backed pages; Linux
//!   `Anonymous` + `Swap` of `smaps_rollup`): its guest's writes and
//!   QEMU's own memory;
//! * each **shared RAM file** a running VM maps, counted once: the part of
//!   it that is resident (`mincore`);
//! * the host's own **available** memory and **pressure**, so other
//!   programs on the machine count too.
//!
//! A new VM is admitted when, with what it is expected to need (the median
//! private memory of the running VMs it most resembles), VMs stay under
//! [`DensityConfig::max_rss_fraction`] of the host's memory, the host keeps
//! [`DensityConfig::min_available_mb`] available, and the vCPUs of all VMs
//! stay within [`DensityConfig::vcpu_overcommit`] times the host's CPUs.
//! Rollout VMs mostly wait (on a trainer, a model, the network), so vCPUs
//! are overcommitted several times by default; memory is not overcommitted
//! beyond what is measured.
//!
//! A VM that was admitted but whose launch has not finished (it is booting,
//! or its RAM is still arriving from a snapshot stream) is **pending**: it
//! counts as at least what it is expected to end up using, so a burst of
//! creates cannot all be admitted against memory none of them holds yet. It
//! stops being pending the moment its launch finishes ([`Pending`]), is
//! deleted or fails, and from then on only what it really holds counts.
//! What a new VM is expected to need comes from VMs that have finished
//! launching (a VM halfway through its boot would make the estimate too
//! small), else its guest RAM.
//!
//! [`decide`] is pure (unit tested); [`measure`], [`host_memory`] and
//! [`file_resident_mb`] read the OS.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default share of the host's physical memory running VMs may use.
pub const DEFAULT_MAX_RSS_FRACTION: f64 = 0.8;
/// Default vCPUs per host CPU.
pub const DEFAULT_VCPU_OVERCOMMIT: f64 = 8.0;
/// How long an admitted VM counts as at least its estimate (its memory
/// grows while it restores or boots).
pub const SETTLE: Duration = Duration::from_secs(45);

/// Admission limits for VMs on this host.
#[derive(Clone, Debug, PartialEq)]
pub struct DensityConfig {
    /// The share of physical memory (0..=1) running VMs may use, counting
    /// shared snapshot pages once (`CUA_VM_MAX_RSS_FRACTION`).
    pub max_rss_fraction: f64,
    /// vCPUs per host CPU (at least 1; `CUA_VM_VCPU_OVERCOMMIT`).
    pub vcpu_overcommit: f64,
    /// Memory the host keeps available for everything else, in MiB
    /// (`CUA_VM_MIN_AVAILABLE_MB`; default an eighth of physical memory,
    /// between 1 and 8 GiB).
    pub min_available_mb: u64,
    /// Off: no admission check (`CUA_VM_ADMISSION=0`).
    pub enabled: bool,
}

impl Default for DensityConfig {
    fn default() -> Self {
        Self {
            max_rss_fraction: DEFAULT_MAX_RSS_FRACTION,
            vcpu_overcommit: DEFAULT_VCPU_OVERCOMMIT,
            min_available_mb: 0,
            enabled: true,
        }
    }
}

impl DensityConfig {
    /// The defaults with the `CUA_VM_*` environment applied; invalid values
    /// are ignored with a warning.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// [`Self::from_env`] over any lookup (tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut c = Self::default();
        let num = |k: &str| -> Option<f64> {
            let v = get(k)?;
            match v.trim().parse::<f64>() {
                Ok(f) if f.is_finite() => Some(f),
                _ => {
                    tracing::warn!(key = k, value = v, "ignoring a non-numeric setting");
                    None
                }
            }
        };
        if let Some(f) = num("CUA_VM_MAX_RSS_FRACTION") {
            if f > 0.0 && f <= 1.0 {
                c.max_rss_fraction = f;
            } else {
                tracing::warn!(
                    value = f,
                    "CUA_VM_MAX_RSS_FRACTION must be in (0, 1]; ignored"
                );
            }
        }
        if let Some(f) = num("CUA_VM_VCPU_OVERCOMMIT") {
            if f >= 1.0 {
                c.vcpu_overcommit = f;
            } else {
                tracing::warn!(
                    value = f,
                    "CUA_VM_VCPU_OVERCOMMIT must be at least 1; ignored"
                );
            }
        }
        if let Some(f) = num("CUA_VM_MIN_AVAILABLE_MB")
            && f >= 0.0
        {
            c.min_available_mb = f as u64;
        }
        if let Some(v) = get("CUA_VM_ADMISSION") {
            c.enabled = !matches!(v.trim(), "0" | "off" | "false" | "no");
        }
        c
    }

    /// The available-memory floor for a host of `total_mb`.
    pub fn floor_mb(&self, total_mb: u64) -> u64 {
        if self.min_available_mb > 0 {
            self.min_available_mb
        } else {
            (total_mb / 8).clamp(1024, 8192)
        }
    }

    /// Most vCPUs all VMs may have on a host of `host_cpus` CPUs.
    pub fn vcpu_limit(&self, host_cpus: u32) -> u32 {
        (f64::from(host_cpus) * self.vcpu_overcommit).floor() as u32
    }

    /// Most memory (MiB) running VMs may use on a host of `total_mb`.
    pub fn rss_cap_mb(&self, total_mb: u64) -> u64 {
        (total_mb as f64 * self.max_rss_fraction) as u64
    }
}

/// How hard the OS is working to find memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    #[default]
    Normal,
    /// Compressing or swapping (macOS `warn`; Linux PSI some > 10%).
    Warn,
    /// About to kill processes (macOS `critical`; Linux PSI full > 10%).
    Critical,
}

impl Pressure {
    /// `normal`, `warn` or `critical`.
    pub fn as_str(self) -> &'static str {
        match self {
            Pressure::Normal => "normal",
            Pressure::Warn => "warn",
            Pressure::Critical => "critical",
        }
    }
    /// 0, 1 or 2 (the wire form).
    pub fn level(self) -> u32 {
        self as u32
    }
}

/// Linux memory pressure from PSI (`/proc/pressure/memory` avg10, percent)
/// and `MemAvailable`. PSI "full" also counts stalls on page-cache refaults:
/// after a big build or copy it passes 10% with most memory still available.
/// That is busy, not about to kill processes, so it is only critical while
/// less than a tenth of memory is available too.
pub fn linux_pressure(full: f64, some: f64, available_mb: u64, total_mb: u64) -> Pressure {
    let scarce = available_mb.saturating_mul(10) < total_mb;
    if full > 10.0 && scarce {
        Pressure::Critical
    } else if full > 10.0 || some > 10.0 {
        Pressure::Warn
    } else {
        Pressure::Normal
    }
}

/// The host's memory now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostMemory {
    /// Physical memory (MiB).
    pub total_mb: u64,
    /// What new work can use (MiB): Linux `MemAvailable`; macOS
    /// `kern.memorystatus_level` of physical memory (file cache and idle
    /// memory the kernel can compress count as available; what
    /// `memory_pressure` prints).
    pub available_mb: u64,
    /// Compressed memory, in physical MiB (macOS; Linux 0).
    pub compressed_mb: u64,
    pub pressure: Pressure,
}

/// One running VM as measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VmUse {
    pub name: String,
    pub vcpus: u32,
    /// Its configured guest RAM (MiB).
    pub memory_mb: u64,
    /// Its own resident memory (MiB), shared snapshot pages excluded.
    pub private_mb: u64,
    /// The snapshot RAM file it maps copy-on-write, if any.
    pub shared_file: Option<PathBuf>,
}

/// Everything admission looks at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub vms: Vec<VmUse>,
    /// Resident pages of each distinct shared RAM file in use (MiB).
    pub shared: Vec<(PathBuf, u64)>,
    /// `None` when the OS does not say.
    pub host: Option<HostMemory>,
    /// Logical CPUs.
    pub host_cpus: u32,
    /// The most VMs the hypervisor runs at once on this host, and how many
    /// it runs now, of any program (macOS: `kern.hv.max_address_spaces`,
    /// 128, every QEMU and Apple Virtualization VM counted). `None`: no
    /// such limit known.
    pub hypervisor: Option<HypervisorSlots>,
}

/// The hypervisor's VM slots (macOS Hypervisor.framework address spaces).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HypervisorSlots {
    pub max: u32,
    pub used: u32,
}

impl Usage {
    /// Shared snapshot memory, each file once (MiB).
    pub fn shared_mb(&self) -> u64 {
        self.shared.iter().map(|(_, mb)| mb).sum()
    }
    /// What the running VMs use together (MiB).
    pub fn vm_rss_mb(&self) -> u64 {
        self.vms.iter().map(|v| v.private_mb).sum::<u64>() + self.shared_mb()
    }
    /// vCPUs of the running VMs.
    pub fn vcpus(&self) -> u32 {
        self.vms.iter().map(|v| v.vcpus).sum()
    }
}

/// A VM about to start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub name: String,
    pub vcpus: u32,
    pub memory_mb: u64,
    /// The snapshot RAM file it will map copy-on-write.
    pub shared_file: Option<PathBuf>,
    /// The size of that file (MiB), counted when no running VM maps it yet.
    pub shared_file_mb: u64,
}

/// A VM admitted whose launch has not finished (booting, or restoring a
/// snapshot): its memory is still arriving. It counts as at least
/// `expected_mb` until its launch finishes, it is deleted or fails, or
/// [`SETTLE`] has passed (a launch that never ends must not hold memory for
/// ever).
#[derive(Clone, Debug)]
pub struct Pending {
    pub name: String,
    pub vcpus: u32,
    pub expected_mb: u64,
    pub at: Instant,
    /// The snapshot RAM file it maps copy-on-write, if any, and its size
    /// (MiB): counted once for all pending VMs of that file, until a
    /// running VM's measurement shows it.
    pub shared_file: Option<PathBuf>,
    pub shared_file_mb: u64,
}

/// What [`decide`] says yes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admit {
    /// The private memory the new VM is expected to need (MiB).
    pub expected_mb: u64,
}

/// Why a VM does not fit, for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal(pub String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn median(mut v: Vec<u64>) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

fn gib(mb: u64) -> String {
    format!("{:.1} GiB", mb as f64 / 1024.0)
}

/// The private memory `req` is expected to need: the median of the settled
/// VMs mapping the same snapshot, else of those of its size, else its whole
/// guest RAM; never more than its RAM (plus QEMU's own ~10%), never under
/// 64 MiB. VMs in `pending` are not settled: they are mid-boot or
/// mid-restore, small now and bigger later, so they say nothing about what
/// a VM ends up using.
pub fn estimate_mb(usage: &Usage, pending: &[Pending], req: &Request) -> u64 {
    let like = |f: &dyn Fn(&VmUse) -> bool| {
        median(
            usage
                .vms
                .iter()
                .filter(|v| !pending.iter().any(|p| p.name == v.name) && f(v))
                .map(|v| v.private_mb)
                .collect(),
        )
    };
    let est = req
        .shared_file
        .as_ref()
        .and_then(|f| like(&|v: &VmUse| v.shared_file.as_ref() == Some(f)))
        .or_else(|| like(&|v: &VmUse| v.memory_mb == req.memory_mb))
        .unwrap_or(req.memory_mb);
    est.clamp(64, req.memory_mb + req.memory_mb / 10 + 64)
}

/// What pending VMs hold that `usage` does not show yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PendingTotals {
    /// Private memory still to arrive (MiB): for a pending VM that is
    /// measured, what it is expected to end at less what it has; for one
    /// that is not (its process has not started), all of it. Plus the
    /// part of each shared RAM file only pending VMs map that is not
    /// resident yet, once per file.
    pub mb: u64,
    /// vCPUs of the pending VMs that are not measured yet.
    pub vcpus: u32,
    /// Pending VMs whose process does not exist yet.
    pub launching: u32,
}

/// Sums what `pending` VMs add on top of `usage`. A pending VM and its
/// measurement count once (the larger of the two), and a shared RAM file
/// counts once however many VMs map it, pending or running.
pub fn pending_totals(usage: &Usage, pending: &[Pending]) -> PendingTotals {
    let mut t = PendingTotals::default();
    let mut files: Vec<(&Path, u64)> = vec![];
    for p in pending {
        match usage.vms.iter().find(|v| v.name == p.name) {
            Some(v) => t.mb += p.expected_mb.saturating_sub(v.private_mb),
            None => {
                t.mb += p.expected_mb;
                t.vcpus += p.vcpus;
                t.launching += 1;
            }
        }
        if let Some(f) = &p.shared_file
            && !files.iter().any(|(g, _)| *g == f.as_path())
        {
            files.push((f, p.shared_file_mb));
        }
    }
    for (f, size) in files {
        let resident = usage
            .shared
            .iter()
            .find(|(g, _)| g == f)
            .map_or(0, |(_, mb)| *mb);
        t.mb += size.saturating_sub(resident);
    }
    t
}

/// Whether `req` may start now. `pending` are VMs admitted whose launch has
/// not finished (each counts as at least its estimate; those already
/// measured in `usage` count as the larger of the two).
pub fn decide(
    cfg: &DensityConfig,
    usage: &Usage,
    pending: &[Pending],
    req: &Request,
) -> Result<Admit, Refusal> {
    // A shared RAM file already counted (mapped by a running VM, or reserved
    // by a pending one) costs nothing more.
    let new_shared = match &req.shared_file {
        Some(f)
            if !usage.shared.iter().any(|(p, _)| p == f)
                && !pending.iter().any(|p| p.shared_file.as_ref() == Some(f)) =>
        {
            req.shared_file_mb
        }
        _ => 0,
    };
    let expected = fork_private_mb(estimate_mb(usage, pending, req), new_shared, req);
    if !cfg.enabled {
        return Ok(Admit {
            expected_mb: expected,
        });
    }
    // Pending VMs: whatever their measurement does not show yet.
    let totals = pending_totals(usage, pending);
    let (pending_mb, pending_vcpus) = (totals.mb, totals.vcpus);
    let need = expected + new_shared;

    if let Some(hv) = usage.hypervisor {
        // VMs admitted but not yet launched hold no address space yet.
        let launching = totals.launching;
        if hv.used + launching + 1 > hv.max {
            return Err(Refusal(format!(
                "this host's hypervisor runs at most {} VMs at once and {} run or are starting \
                 (every QEMU and Apple Virtualization VM on the machine counts; macOS \
                 kern.hv.max_address_spaces), so '{}' does not fit; delete a VM or create it \
                 on another machine (on=\"auto\")",
                hv.max,
                hv.used + launching,
                req.name
            )));
        }
    }
    let limit = cfg.vcpu_limit(usage.host_cpus.max(1));
    let vcpus = usage.vcpus() + pending_vcpus;
    if vcpus + req.vcpus > limit {
        return Err(Refusal(format!(
            "this host is at its VM CPU limit: its VMs have {vcpus} vCPUs on {} CPUs \
             (vcpu_overcommit {}x allows {limit}), so '{}' with {} more does not fit; \
             delete a Space, give it fewer CPUs, raise CUA_VM_VCPU_OVERCOMMIT, or create \
             it on another machine (on=\"auto\")",
            usage.host_cpus, cfg.vcpu_overcommit, req.name, req.vcpus
        )));
    }
    let Some(host) = usage.host else {
        // The OS does not say how much memory there is: CPUs only.
        return Ok(Admit {
            expected_mb: expected,
        });
    };
    if host.pressure == Pressure::Critical {
        return Err(Refusal(format!(
            "this host's memory pressure is critical ({} available of {}), so '{}' was not \
             started; delete a Space or create it on another machine (on=\"auto\")",
            gib(host.available_mb),
            gib(host.total_mb),
            req.name
        )));
    }
    let cap = cfg.rss_cap_mb(host.total_mb);
    // One VM bigger than everything VMs may use here never fits, however
    // many VMs are deleted: say what it asked for and the cap.
    if need > cap {
        return Err(Refusal(format!(
            "'{}' needs about {} of memory ({} asked), more than this host's VM memory \
             limit of {} (max_rss_fraction {} of {}); give it less memory (--memory), \
             raise CUA_VM_MAX_RSS_FRACTION on this host, or create it on a machine with \
             more memory (on=\"auto\")",
            req.name,
            gib(need),
            gib(req.memory_mb),
            gib(cap),
            cfg.max_rss_fraction,
            gib(host.total_mb)
        )));
    }
    let used = usage.vm_rss_mb() + pending_mb;
    if used + need > cap {
        return Err(Refusal(format!(
            "this host is at its VM memory limit: its VMs use {} (shared snapshot pages \
             counted once) of the {} they may use (max_rss_fraction {} of {}), and '{}' \
             is expected to need {} more; delete a Space, give it less memory, raise \
             CUA_VM_MAX_RSS_FRACTION, or create it on another machine (on=\"auto\")",
            gib(used),
            gib(cap),
            cfg.max_rss_fraction,
            gib(host.total_mb),
            req.name,
            gib(need)
        )));
    }
    let floor = cfg.floor_mb(host.total_mb);
    let avail = host.available_mb.saturating_sub(pending_mb);
    if avail < need + floor {
        return Err(Refusal(format!(
            "this host has {} of memory available and keeps {} free for everything else, \
             so '{}', expected to need {}, does not fit; delete a Space, close programs, \
             or create it on another machine (on=\"auto\")",
            gib(avail),
            gib(floor),
            req.name,
            gib(need)
        )));
    }
    Ok(Admit {
        expected_mb: expected,
    })
}

/// The private memory a VM is expected to need when it also brings a shared
/// RAM file nobody maps yet (`new_shared_mb`, the first fork of a checkpoint
/// or the first restore of a snapshot): each of its guest pages is either
/// still the file's page or its own private copy, never both, so together
/// they are at most its guest RAM plus QEMU's own (the same bound
/// [`estimate_mb`] applies). Charging the full guest RAM as private on top
/// of the whole file counted one VM's memory twice. Later VMs of the same
/// file still get the full estimate until a sibling has settled, so a burst
/// of forks is charged its honest worst case.
fn fork_private_mb(estimate: u64, new_shared_mb: u64, req: &Request) -> u64 {
    if new_shared_mb == 0 {
        return estimate;
    }
    let ceiling = req.memory_mb + req.memory_mb / 10 + 64;
    estimate.min(ceiling.saturating_sub(new_shared_mb)).max(64)
}

/// How much more VM memory (MiB) this host takes now: the least of what
/// the cap and the available-memory floor leave.
pub fn headroom_mb(cfg: &DensityConfig, usage: &Usage, pending: &[Pending]) -> Option<u64> {
    let host = usage.host?;
    let totals = pending_totals(usage, pending);
    let pending_mb = totals.mb;
    let by_cap = cfg
        .rss_cap_mb(host.total_mb)
        .saturating_sub(usage.vm_rss_mb() + pending_mb);
    let by_floor = host
        .available_mb
        .saturating_sub(pending_mb + cfg.floor_mb(host.total_mb));
    let hv_full = usage
        .hypervisor
        .is_some_and(|h| h.used + totals.launching >= h.max);
    Some(if host.pressure == Pressure::Critical || hv_full {
        0
    } else {
        by_cap.min(by_floor)
    })
}

/// The median private memory of the settled running VMs (MiB), what one
/// more is expected to need.
pub fn typical_vm_mb(usage: &Usage, pending: &[Pending]) -> Option<u64> {
    median(
        usage
            .vms
            .iter()
            .filter(|v| !pending.iter().any(|p| p.name == v.name))
            .map(|v| v.private_mb)
            .collect(),
    )
}

/// Logical CPUs of this host.
pub fn host_cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

/// The private resident memory of process `pid` (MiB), or `None` when it is
/// gone or the OS does not say.
pub fn process_private_mb(pid: u32) -> Option<u64> {
    imp::process_private_bytes(pid).map(|b| b >> 20)
}

/// How much of `path` is in memory (MiB), by `mincore` over a read-only
/// mapping of it.
pub fn file_resident_mb(path: &Path) -> Option<u64> {
    imp::file_resident_bytes(path).map(|b| b >> 20)
}

/// This host's memory now.
pub fn host_memory() -> Option<HostMemory> {
    imp::host_memory()
}

/// Measures running VMs and the shared files they map: `vms` are
/// `(name, pid, vcpus, memory_mb, shared_file)`; one whose process is gone
/// is left out.
pub fn measure(vms: &[(String, u32, u32, u64, Option<PathBuf>)]) -> Usage {
    let mut out = Usage {
        host: host_memory(),
        host_cpus: host_cpus(),
        hypervisor: imp::hypervisor_slots(),
        ..Default::default()
    };
    for (name, pid, vcpus, memory_mb, shared) in vms {
        let Some(private_mb) = process_private_mb(*pid) else {
            continue;
        };
        if let Some(f) = shared
            && !out.shared.iter().any(|(p, _)| p == f)
        {
            out.shared
                .push((f.clone(), file_resident_mb(f).unwrap_or(0)));
        }
        out.vms.push(VmUse {
            name: name.clone(),
            vcpus: *vcpus,
            memory_mb: *memory_mb,
            private_mb,
            shared_file: shared.clone(),
        });
    }
    out
}

#[cfg(unix)]
mod imp_unix {
    use std::path::Path;

    /// Resident bytes of a file, by `mincore` over a private read-only map.
    pub fn file_resident_bytes(path: &Path) -> Option<u64> {
        use std::os::fd::AsRawFd;
        let f = std::fs::File::open(path).ok()?;
        let len = f.metadata().ok()?.len() as usize;
        if len == 0 {
            return Some(0);
        }
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as usize;
        // SAFETY: a read-only shared mapping of an open file of `len` bytes;
        // unmapped below on every path.
        let addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                f.as_raw_fd(),
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            return None;
        }
        let pages = len.div_ceil(page);
        let mut vec = vec![0u8; pages];
        // SAFETY: `addr`/`len` is the mapping above and `vec` has one byte
        // per page of it.
        let r = unsafe { libc::mincore(addr, len, vec.as_mut_ptr().cast()) };
        // SAFETY: the mapping made above.
        unsafe { libc::munmap(addr, len) };
        if r != 0 {
            return None;
        }
        let resident = vec.iter().filter(|b| **b & 1 != 0).count() as u64;
        Some(resident * page as u64)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub use super::imp_unix::file_resident_bytes;
    use super::{HostMemory, Pressure};

    /// `phys_footprint`: dirty private and compressed pages, IOKit and
    /// Hypervisor.framework memory; clean file-backed pages (a snapshot
    /// RAM file's unwritten pages) are not in it.
    pub fn process_private_bytes(pid: u32) -> Option<u64> {
        // SAFETY: a zeroed out struct of the V4 flavor, which proc_pid_rusage
        // fills.
        let mut ri: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        // SAFETY: `ri` is a valid rusage_info_v4 for RUSAGE_INFO_V4.
        let r = unsafe {
            libc::proc_pid_rusage(
                pid as i32,
                libc::RUSAGE_INFO_V4,
                (&mut ri as *mut libc::rusage_info_v4).cast(),
            )
        };
        (r == 0).then_some(ri.ri_phys_footprint)
    }

    /// The classic `vm_statistics64` (what every macOS fills).
    #[repr(C)]
    #[derive(Default)]
    struct VmStats {
        free_count: u32,
        active_count: u32,
        inactive_count: u32,
        wire_count: u32,
        zero_fill_count: u64,
        reactivations: u64,
        pageins: u64,
        pageouts: u64,
        faults: u64,
        cow_faults: u64,
        lookups: u64,
        hits: u64,
        purges: u64,
        purgeable_count: u32,
        speculative_count: u32,
        decompressions: u64,
        compressions: u64,
        swapins: u64,
        swapouts: u64,
        compressor_page_count: u32,
        throttled_count: u32,
        external_page_count: u32,
        internal_page_count: u32,
        total_uncompressed_pages_in_compressor: u64,
    }

    fn sysctl_u64(name: &std::ffi::CStr) -> Option<u64> {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: a NUL-terminated name and an out buffer of `len` bytes.
        let r = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut v as *mut u64).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        // 4-byte values land in the low half (little endian).
        (r == 0).then_some(if len == 4 { v & 0xffff_ffff } else { v })
    }

    /// Hypervisor.framework's VM limit and the VMs using it now: every
    /// QEMU (`hvf`) and Apple Virtualization VM process on the machine.
    pub fn hypervisor_slots() -> Option<super::HypervisorSlots> {
        let max = sysctl_u64(c"kern.hv.max_address_spaces")? as u32;
        let mut pids = vec![0i32; 8192];
        // SAFETY: a buffer of `pids.len()` ints and its size in bytes.
        let n = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr().cast(),
                (pids.len() * std::mem::size_of::<i32>()) as i32,
            )
        };
        if n <= 0 {
            return None;
        }
        let mut used = 0;
        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        for pid in &pids[..(n as usize).min(pids.len())] {
            // SAFETY: a buffer of its own size.
            let len =
                unsafe { libc::proc_pidpath(*pid, path.as_mut_ptr().cast(), path.len() as u32) };
            if len <= 0 {
                continue;
            }
            let p = &path[..len as usize];
            let name = p.rsplit(|b| *b == b'/').next().unwrap_or(p);
            if name.starts_with(b"qemu-system-")
                || name == b"com.apple.Virtualization.VirtualMachine"
            {
                used += 1;
            }
        }
        Some(super::HypervisorSlots { max, used })
    }

    pub fn host_memory() -> Option<HostMemory> {
        let total = sysctl_u64(c"hw.memsize")?;
        let mut s = VmStats::default();
        let mut count = (std::mem::size_of::<VmStats>() / std::mem::size_of::<i32>()) as u32;
        // SAFETY: mach_host_self has no preconditions; `s` is a buffer of
        // `count` integers, which host_statistics64 fills at most.
        #[allow(deprecated)]
        let r = unsafe {
            libc::host_statistics64(
                libc::mach_host_self(),
                libc::HOST_VM_INFO64,
                (&mut s as *mut VmStats).cast(),
                &mut count,
            )
        };
        if r != 0 {
            return None;
        }
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        let app = u64::from(s.internal_page_count).saturating_sub(u64::from(s.purgeable_count));
        let used = (app + u64::from(s.wire_count) + u64::from(s.compressor_page_count)) * page;
        // The kernel's own figure, the one its pressure levels follow (and
        // `memory_pressure` prints): idle memory it can compress counts as
        // available. Else physical memory less app, wired and compressed.
        let available = match sysctl_u64(c"kern.memorystatus_level").filter(|l| *l <= 100) {
            Some(level) => (total >> 20) * level / 100,
            None => total.saturating_sub(used) >> 20,
        };
        let pressure = match sysctl_u64(c"kern.memorystatus_vm_pressure_level") {
            Some(4) => Pressure::Critical,
            Some(2) => Pressure::Warn,
            _ => Pressure::Normal,
        };
        Some(HostMemory {
            total_mb: total >> 20,
            available_mb: available,
            compressed_mb: (u64::from(s.compressor_page_count) * page) >> 20,
            pressure,
        })
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::HostMemory;
    pub use super::imp_unix::file_resident_bytes;

    fn kb(text: &str, key: &str) -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(key))?;
        line.split_whitespace().nth(1)?.parse().ok()
    }

    #[cfg(test)]
    pub fn kb_for_test(text: &str, key: &str) -> Option<u64> {
        kb(text, key)
    }

    /// Anonymous memory (what the guest wrote, including copy-on-write
    /// copies of a snapshot file's pages) and swap.
    ///
    /// Read from `/proc/<pid>/status` (`RssAnon`, `VmSwap`), which the kernel
    /// keeps as counters. `smaps_rollup` gives the same figure but walks the
    /// page tables of the whole guest RAM (about 5 ms for a 4 GiB VM), and
    /// admission measures every running VM for each new one: with hundreds
    /// of VMs that walk made creates serial and quadratic (256 forks: create
    /// p50 31 s, max 133 s). Kernels before 4.5 have no `RssAnon`; they use
    /// `smaps_rollup`.
    pub fn process_private_bytes(pid: u32) -> Option<u64> {
        if let Ok(t) = std::fs::read_to_string(format!("/proc/{pid}/status"))
            && let Some(anon) = kb(&t, "RssAnon:")
        {
            return Some((anon + kb(&t, "VmSwap:").unwrap_or(0)) * 1024);
        }
        let t = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
        Some((kb(&t, "Anonymous:")? + kb(&t, "Swap:").unwrap_or(0)) * 1024)
    }

    fn psi(text: &str, kind: &str) -> f64 {
        text.lines()
            .find(|l| l.starts_with(kind))
            .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    }

    /// KVM has no small fixed VM limit.
    pub fn hypervisor_slots() -> Option<super::HypervisorSlots> {
        None
    }

    pub fn host_memory() -> Option<HostMemory> {
        let t = std::fs::read_to_string("/proc/meminfo").ok()?;
        let total_mb = kb(&t, "MemTotal:")? / 1024;
        let available_mb = kb(&t, "MemAvailable:")? / 1024;
        let (full, some) = match std::fs::read_to_string("/proc/pressure/memory") {
            Ok(p) => (psi(&p, "full"), psi(&p, "some")),
            Err(_) => (0.0, 0.0),
        };
        Some(HostMemory {
            total_mb,
            available_mb,
            compressed_mb: 0,
            pressure: super::linux_pressure(full, some, available_mb, total_mb),
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use super::HostMemory;
    use std::path::Path;
    pub fn process_private_bytes(_: u32) -> Option<u64> {
        None
    }
    pub fn file_resident_bytes(_: &Path) -> Option<u64> {
        None
    }
    pub fn host_memory() -> Option<HostMemory> {
        None
    }
    pub fn hypervisor_slots() -> Option<super::HypervisorSlots> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psi_full_is_critical_only_while_memory_is_scarce() {
        // The CI runner after a build: 6.4 of 7.8 GiB available, full 12%.
        assert_eq!(linux_pressure(12.0, 30.0, 6_550, 7_990), Pressure::Warn);
        assert_eq!(linux_pressure(12.0, 30.0, 700, 7_990), Pressure::Critical);
        assert_eq!(linux_pressure(0.0, 11.0, 6_550, 7_990), Pressure::Warn);
        assert_eq!(linux_pressure(1.0, 2.0, 100, 7_990), Pressure::Normal);
    }

    fn vm(name: &str, private_mb: u64, shared: Option<&str>) -> VmUse {
        VmUse {
            name: name.into(),
            vcpus: 1,
            memory_mb: 1024,
            private_mb,
            shared_file: shared.map(PathBuf::from),
        }
    }

    fn host(total_mb: u64, available_mb: u64) -> Option<HostMemory> {
        Some(HostMemory {
            total_mb,
            available_mb,
            compressed_mb: 0,
            pressure: Pressure::Normal,
        })
    }

    fn req(name: &str, shared: Option<&str>) -> Request {
        Request {
            name: name.into(),
            vcpus: 1,
            memory_mb: 1024,
            shared_file: shared.map(PathBuf::from),
            shared_file_mb: 1024,
        }
    }

    fn cfg() -> DensityConfig {
        DensityConfig {
            max_rss_fraction: 0.5,
            vcpu_overcommit: 4.0,
            min_available_mb: 1024,
            enabled: true,
        }
    }

    /// The CI runner: 7.8 GiB, nothing running, default fraction (6.2 GiB
    /// for VMs).
    fn runner() -> (DensityConfig, Usage) {
        let cfg = DensityConfig {
            min_available_mb: 0,
            ..DensityConfig::default()
        };
        let u = Usage {
            host: host(7_990, 7_200),
            host_cpus: 4,
            ..Default::default()
        };
        (cfg, u)
    }

    fn fresh(name: &str, memory_mb: u64) -> Request {
        Request {
            name: name.into(),
            vcpus: 2,
            memory_mb,
            shared_file: None,
            shared_file_mb: 0,
        }
    }

    #[test]
    fn a_vm_under_the_cap_is_admitted_on_an_empty_host() {
        let (cfg, u) = runner();
        // A 4 GiB bake on the 7.8 GiB runner, nothing else running.
        let a = decide(&cfg, &u, &[], &fresh("bake", 4096)).unwrap();
        assert!(a.expected_mb >= 4096);
        // Right at the cap still fits.
        let cap = cfg.rss_cap_mb(7_990);
        let at_cap = cap - cap / 11 - 64;
        assert!(decide(&cfg, &u, &[], &fresh("edge", at_cap)).is_ok());
    }

    #[test]
    fn a_vm_bigger_than_the_cap_says_what_it_asked_for_and_the_cap() {
        let (cfg, u) = runner();
        let e = decide(&cfg, &u, &[], &fresh("big", 8192)).unwrap_err().0;
        assert!(
            e.starts_with("'big' needs about 8.0 GiB of memory (8.0 GiB asked)"),
            "{e}"
        );
        assert!(
            e.contains("more than this host's VM memory limit of 6.2 GiB"),
            "{e}"
        );
        assert!(e.contains("CUA_VM_MAX_RSS_FRACTION"), "{e}");
        // Not the "its VMs use 0.0 GiB" message for a host that is empty.
        assert!(!e.contains("its VMs use"), "{e}");
    }

    fn fork_of(name: &str, memory_mb: u64) -> Request {
        Request {
            name: name.into(),
            vcpus: 2,
            memory_mb,
            shared_file: Some(PathBuf::from("/ck/mem.ram")),
            shared_file_mb: memory_mb,
        }
    }

    #[test]
    fn the_first_fork_of_a_checkpoint_counts_its_ram_once() {
        // The bake's fork: a 4 GiB checkpoint, nothing running, 7.8 GiB host.
        let (cfg, u) = runner();
        let a = decide(&cfg, &u, &[], &fork_of("fork", 4096)).unwrap();
        // The file (4 GiB) plus what fits under guest RAM + QEMU's own, not
        // the file plus another full 4 GiB.
        assert_eq!(a.expected_mb, 4096 / 10 + 64);
        assert!(4096 + a.expected_mb <= 4096 + 4096 / 10 + 64);
    }

    #[test]
    fn a_burst_of_forks_is_still_charged_its_worst_case() {
        let (cfg, u) = runner();
        let first = decide(&cfg, &u, &[], &fork_of("f0", 4096)).unwrap();
        let pending = [Pending {
            name: "f0".into(),
            vcpus: 2,
            expected_mb: first.expected_mb,
            at: Instant::now(),
            shared_file: Some(PathBuf::from("/ck/mem.ram")),
            shared_file_mb: 4096,
        }];
        // The file is counted once (by f0); a second fork whose sibling has
        // not settled yet may still write all its RAM, so it is charged the
        // full estimate, and on this host it does not fit.
        let e = decide(&cfg, &u, &pending, &fork_of("f1", 4096))
            .unwrap_err()
            .0;
        assert!(e.contains("VM memory limit"), "{e}");
        // On a host with room it fits, charged the full guest RAM.
        let big = Usage {
            host: host(32_768, 30_000),
            host_cpus: 8,
            ..Default::default()
        };
        let a = decide(&cfg, &big, &pending, &fork_of("f1", 4096)).unwrap();
        assert_eq!(a.expected_mb, 4096);
    }

    #[test]
    fn later_forks_are_estimated_from_settled_siblings() {
        // Once a fork has settled, the next ones are expected to use what it
        // uses (its private pages), and the file is already counted.
        let (cfg, mut u) = runner();
        u.vms.push(VmUse {
            name: "f0".into(),
            vcpus: 2,
            memory_mb: 4096,
            private_mb: 300,
            shared_file: Some(PathBuf::from("/ck/mem.ram")),
        });
        u.shared.push((PathBuf::from("/ck/mem.ram"), 1_500));
        let a = decide(&cfg, &u, &[], &fork_of("f1", 4096)).unwrap();
        assert_eq!(a.expected_mb, 300);
    }

    #[test]
    fn shared_snapshot_pages_count_once() {
        let u = Usage {
            vms: (0..10)
                .map(|i| vm(&format!("v{i}"), 100, Some("/g/mem.ram")))
                .collect(),
            shared: vec![("/g/mem.ram".into(), 700)],
            host: host(16384, 12000),
            host_cpus: 8,
            hypervisor: None,
        };
        // 10 x 100 private + 700 shared once, not 10 x 800.
        assert_eq!(u.vm_rss_mb(), 1700);
        assert_eq!(u.vcpus(), 10);
    }

    #[test]
    fn estimate_prefers_siblings_then_same_size_then_reservation() {
        let mut u = Usage {
            vms: vec![
                vm("a", 300, Some("/g/1")),
                vm("b", 320, Some("/g/1")),
                vm("c", 900, None),
            ],
            ..Default::default()
        };
        assert_eq!(estimate_mb(&u, &[], &req("n", Some("/g/1"))), 320);
        // Same size, no siblings: all three of 1 GiB.
        assert_eq!(estimate_mb(&u, &[], &req("n", Some("/g/2"))), 320);
        u.vms.clear();
        assert_eq!(estimate_mb(&u, &[], &req("n", None)), 1024);
        // Never under 64 MiB.
        u.vms = vec![vm("tiny", 3, None)];
        assert_eq!(estimate_mb(&u, &[], &req("n", None)), 64);
    }

    #[test]
    fn admits_until_the_rss_cap_then_says_why() {
        let c = cfg();
        // Cap 8 GiB of 16; 7 VMs at 1 GiB each use 7 GiB.
        let mut u = Usage {
            vms: (0..7).map(|i| vm(&format!("v{i}"), 1024, None)).collect(),
            host: host(16384, 9000),
            host_cpus: 8,
            ..Default::default()
        };
        assert_eq!(
            decide(&c, &u, &[], &req("n", None)).unwrap().expected_mb,
            1024
        );
        u.vms.push(vm("v7", 1024, None));
        let e = decide(&c, &u, &[], &req("n", None)).unwrap_err().0;
        assert!(e.contains("VM memory limit"), "{e}");
        assert!(e.contains("max_rss_fraction 0.5"), "{e}");
        assert!(e.contains("CUA_VM_MAX_RSS_FRACTION"), "{e}");
        assert!(e.contains("'n'"), "{e}");
    }

    #[test]
    fn copy_on_write_siblings_fit_many_more_than_reservations() {
        let c = cfg();
        // 8 GiB cap: 60 forks of one 1 GiB world, each 100 MiB private,
        // share 600 MiB: 6.6 GiB, and one more fits.
        let u = Usage {
            vms: (0..60)
                .map(|i| vm(&format!("f{i}"), 100, Some("/w/mem.ram")))
                .collect(),
            shared: vec![("/w/mem.ram".into(), 600)],
            host: host(16384, 9000),
            host_cpus: 16,
            hypervisor: None,
        };
        assert!(decide(&c, &u, &[], &req("n", Some("/w/mem.ram"))).is_ok());
        // A new world's file is counted in full until something maps it.
        let mut r = req("n", Some("/w2/mem.ram"));
        r.shared_file_mb = 4096;
        assert!(decide(&c, &u, &[], &r).is_err());
    }

    #[test]
    fn vcpu_overcommit_limits_vcpus() {
        let c = cfg();
        let u = Usage {
            vms: (0..31).map(|i| vm(&format!("v{i}"), 10, None)).collect(),
            host: host(1 << 20, 1 << 20),
            host_cpus: 8,
            ..Default::default()
        };
        // 4x of 8 CPUs: 32 vCPUs.
        assert!(decide(&c, &u, &[], &req("n", None)).is_ok());
        let mut two = req("n", None);
        two.vcpus = 2;
        let e = decide(&c, &u, &[], &two).unwrap_err().0;
        assert!(e.contains("31 vCPUs on 8 CPUs"), "{e}");
        assert!(e.contains("vcpu_overcommit 4x allows 32"), "{e}");
    }

    #[test]
    fn pending_vms_count_until_they_are_measured() {
        let c = cfg();
        let u = Usage {
            vms: vec![vm("a", 200, None)],
            host: host(16384, 15000),
            host_cpus: 2,
            ..Default::default()
        };
        let p = |name: &str| Pending {
            name: name.into(),
            vcpus: 1,
            expected_mb: 1024,
            at: Instant::now(),
            shared_file: None,
            shared_file_mb: 0,
        };
        // 8 vCPUs allowed: 1 running + 7 pending = 8, so a 9th is refused
        // even though none of the pending ones is measured yet.
        let pending: Vec<Pending> = (0..7).map(|i| p(&format!("p{i}"))).collect();
        assert!(
            decide(&c, &u, &pending, &req("n", None))
                .unwrap_err()
                .0
                .contains("vCPU")
        );
        // A pending VM that is measured counts once (the larger of the two).
        let pending = vec![p("a")];
        assert!(decide(&c, &u, &pending, &req("n", None)).is_ok());
        assert_eq!(headroom_mb(&c, &u, &pending), Some(8192 - 1024));
    }

    fn pend(name: &str, expected_mb: u64, shared: Option<(&str, u64)>) -> Pending {
        Pending {
            name: name.into(),
            vcpus: 1,
            expected_mb,
            at: Instant::now(),
            shared_file: shared.map(|(f, _)| PathBuf::from(f)),
            shared_file_mb: shared.map_or(0, |(_, mb)| mb),
        }
    }

    /// A 32 GiB host that allows its VMs 24.6 GiB, as in the failing run.
    fn small_host() -> (DensityConfig, Option<HostMemory>) {
        (
            DensityConfig {
                max_rss_fraction: 0.8,
                vcpu_overcommit: 8.0,
                min_available_mb: 1024,
                enabled: true,
            },
            host(31_539, 28_000),
        )
    }

    #[test]
    fn settled_vms_count_what_they_use_not_their_guest_size() {
        let (c, h) = small_host();
        // 20 VMs of 2 GiB guest RAM that use 600 MiB each: 12 GiB, not 40.
        let mut r = req("n", None);
        r.memory_mb = 2048;
        let u = Usage {
            vms: (0..20)
                .map(|i| VmUse {
                    memory_mb: 2048,
                    ..vm(&format!("v{i}"), 600, None)
                })
                .collect(),
            host: h,
            host_cpus: 16,
            ..Default::default()
        };
        assert_eq!(u.vm_rss_mb(), 12_000);
        let a = decide(&c, &u, &[], &r).unwrap();
        // The next one is expected to need what its settled peers use.
        assert_eq!(a.expected_mb, 600);
        // And 22 more fit, up to the cap (24.6 GiB = 42 x 600 MiB), not one
        // fewer.
        let u40 = Usage {
            vms: (0..42)
                .map(|i| VmUse {
                    memory_mb: 2048,
                    ..vm(&format!("v{i}"), 600, None)
                })
                .collect(),
            ..u.clone()
        };
        let u41 = Usage {
            vms: u40.vms[..41].to_vec(),
            ..u40.clone()
        };
        assert!(decide(&c, &u41, &[], &r).is_ok());
        let e = decide(&c, &u40, &[], &r).unwrap_err().0;
        assert!(e.contains("its VMs use 24.6 GiB"), "{e}");
    }

    #[test]
    fn booting_vms_count_at_guest_size_until_their_launch_finishes() {
        let (c, h) = small_host();
        let mut r = req("n", None);
        r.memory_mb = 2048;
        // Nothing has settled, so there is nothing to estimate from: each
        // booting VM is expected to need its guest RAM (2 GiB), whatever it
        // holds so far.
        let u = Usage {
            vms: (0..8)
                .map(|i| VmUse {
                    memory_mb: 2048,
                    ..vm(&format!("b{i}"), 150, None)
                })
                .collect(),
            host: h,
            host_cpus: 16,
            ..Default::default()
        };
        let booting: Vec<Pending> = (0..8).map(|i| pend(&format!("b{i}"), 2048, None)).collect();
        assert_eq!(estimate_mb(&u, &booting, &r), 2048);
        // 8 x 2 GiB = 16 GiB reserved, a 9th and 10th fit under 24.6 ...
        assert!(decide(&c, &u, &booting, &r).is_ok());
        // ... and once their launch finished (pending dropped) they are
        // counted at the 150 MiB they hold, and the estimate follows.
        assert_eq!(estimate_mb(&u, &[], &r), 150);
        let e = {
            let more: Vec<Pending> = (8..12)
                .map(|i| pend(&format!("c{i}"), 2048, None))
                .collect();
            let mut all = booting.clone();
            all.extend(more);
            decide(&c, &u, &all, &r).unwrap_err().0
        };
        // 8 x 2 GiB + 4 x 2 GiB booting leaves no room for another 2 GiB.
        assert!(e.contains("VM memory limit"), "{e}");
        assert!(e.contains("its VMs use 24.0 GiB"), "{e}");
    }

    #[test]
    fn a_pending_vm_and_its_measurement_count_once() {
        let (c, h) = small_host();
        let u = Usage {
            vms: vec![vm("a", 700, None)],
            host: h,
            host_cpus: 16,
            ..Default::default()
        };
        // Expected 2048, holds 700: 2048 in all, not 2748.
        let t = pending_totals(&u, &[pend("a", 2048, None)]);
        assert_eq!(t.mb, 2048 - 700);
        assert_eq!(t.launching, 0);
        // Holds more than expected: what it holds, nothing added.
        let t = pending_totals(&u, &[pend("a", 500, None)]);
        assert_eq!(t.mb, 0);
        // Not started yet: all of it, and its vCPUs.
        let t = pending_totals(&u, &[pend("ghost", 2048, None)]);
        assert_eq!((t.mb, t.vcpus, t.launching), (2048, 1, 1));
        let _ = c;
    }

    #[test]
    fn pending_vms_of_one_snapshot_count_its_ram_file_once() {
        let (c, h) = small_host();
        let mut r = req("n", Some("/ck/mem.ram"));
        r.memory_mb = 2048;
        r.shared_file_mb = 2048;
        // 30 forks admitted a moment ago, none measured yet, all mapping one
        // 2 GiB file: the file once (2 GiB) plus 30 x 100 MiB of their own.
        let pending: Vec<Pending> = (0..30)
            .map(|i| pend(&format!("f{i}"), 100, Some(("/ck/mem.ram", 2048))))
            .collect();
        let u = Usage {
            host: h,
            host_cpus: 64,
            ..Default::default()
        };
        assert_eq!(pending_totals(&u, &pending).mb, 30 * 100 + 2048);
        let a = decide(&c, &u, &pending, &r);
        assert!(a.is_ok(), "{a:?}");
        // When a running fork shows 700 MiB of the file resident, only the
        // rest of it is still reserved.
        let u = Usage {
            vms: vec![vm("f0", 100, Some("/ck/mem.ram"))],
            shared: vec![("/ck/mem.ram".into(), 700)],
            host: h,
            host_cpus: 64,
            ..Default::default()
        };
        // 29 unmeasured pending VMs at 100 MiB each, f0 already measured
        // at what it was expected to use (adds nothing), and the part of the
        // 2048 MiB RAM file not resident yet (700 MiB is).
        let f0_rest = 0;
        assert_eq!(
            pending_totals(&u, &pending).mb,
            29 * 100 + f0_rest + (2048 - 700)
        );
        // Another snapshot's file is a new cost, and is refused when it does
        // not fit.
        let mut big = req("n", Some("/other/mem.ram"));
        big.memory_mb = 2048;
        big.shared_file_mb = 30_000;
        assert!(decide(&c, &u, &pending, &big).is_err());
    }

    #[test]
    fn the_estimate_ignores_vms_still_booting() {
        // A burst: 3 settled VMs of 800 MiB, 40 just started with 100 MiB
        // each. The median over all would be 100; a new VM is expected to
        // end like the settled ones.
        let mut vms: Vec<VmUse> = (0..3).map(|i| vm(&format!("s{i}"), 800, None)).collect();
        vms.extend((0..40).map(|i| vm(&format!("b{i}"), 100, None)));
        let u = Usage {
            vms,
            ..Default::default()
        };
        let booting: Vec<Pending> = (0..40).map(|i| pend(&format!("b{i}"), 800, None)).collect();
        assert_eq!(estimate_mb(&u, &[], &req("n", None)), 100);
        assert_eq!(estimate_mb(&u, &booting, &req("n", None)), 800);
        assert_eq!(typical_vm_mb(&u, &booting), Some(800));
    }

    /// macOS runs at most `kern.hv.max_address_spaces` (128) VMs at once,
    /// whatever the memory: past it QEMU fails with HV_NO_RESOURCES
    /// (measured on an M2 Max: the 129th VM).
    #[test]
    fn the_hypervisor_vm_limit_refuses_with_its_reason() {
        let c = cfg();
        let mut u = Usage {
            host: host(1 << 20, 1 << 20),
            host_cpus: 1024,
            hypervisor: Some(HypervisorSlots {
                max: 128,
                used: 127,
            }),
            ..Default::default()
        };
        assert!(decide(&c, &u, &[], &req("n", None)).is_ok());
        // A VM admitted but not launched yet takes the last slot.
        let p = Pending {
            name: "p".into(),
            vcpus: 1,
            expected_mb: 64,
            at: Instant::now(),
            shared_file: None,
            shared_file_mb: 0,
        };
        let e = decide(&c, &u, &[p], &req("n", None)).unwrap_err().0;
        assert!(
            e.contains("at most 128 VMs") && e.contains("kern.hv.max_address_spaces"),
            "{e}"
        );
        u.hypervisor = Some(HypervisorSlots {
            max: 128,
            used: 128,
        });
        assert!(decide(&c, &u, &[], &req("n", None)).is_err());
        assert_eq!(headroom_mb(&c, &u, &[]), Some(0));
    }

    #[test]
    fn the_available_floor_and_critical_pressure_refuse() {
        let c = cfg();
        let mut u = Usage {
            host: host(16384, 1500),
            host_cpus: 8,
            ..Default::default()
        };
        let e = decide(&c, &u, &[], &req("n", None)).unwrap_err().0;
        assert!(e.contains("available"), "{e}");
        u.host = Some(HostMemory {
            total_mb: 16384,
            available_mb: 16000,
            compressed_mb: 0,
            pressure: Pressure::Critical,
        });
        let e = decide(&c, &u, &[], &req("n", None)).unwrap_err().0;
        assert!(e.contains("critical"), "{e}");
        assert_eq!(headroom_mb(&c, &u, &[]), Some(0));
    }

    #[test]
    fn disabled_or_unknown_memory_admits() {
        let mut c = cfg();
        c.enabled = false;
        let u = Usage {
            host: host(1024, 0),
            host_cpus: 1,
            ..Default::default()
        };
        assert!(decide(&c, &u, &[], &req("n", None)).is_ok());
        c.enabled = true;
        let u = Usage {
            host: None,
            host_cpus: 1,
            ..Default::default()
        };
        assert!(decide(&c, &u, &[], &req("n", None)).is_ok());
    }

    #[test]
    fn config_from_env() {
        let c = DensityConfig::from_lookup(|k| match k {
            "CUA_VM_MAX_RSS_FRACTION" => Some("0.6".into()),
            "CUA_VM_VCPU_OVERCOMMIT" => Some("6".into()),
            "CUA_VM_MIN_AVAILABLE_MB" => Some("2048".into()),
            _ => None,
        });
        assert_eq!(
            (
                c.max_rss_fraction,
                c.vcpu_overcommit,
                c.min_available_mb,
                c.enabled
            ),
            (0.6, 6.0, 2048, true)
        );
        // Out of range values keep the defaults.
        let c = DensityConfig::from_lookup(|k| match k {
            "CUA_VM_MAX_RSS_FRACTION" => Some("1.5".into()),
            "CUA_VM_VCPU_OVERCOMMIT" => Some("0.5".into()),
            "CUA_VM_ADMISSION" => Some("off".into()),
            _ => None,
        });
        assert_eq!(c.max_rss_fraction, DEFAULT_MAX_RSS_FRACTION);
        assert_eq!(c.vcpu_overcommit, DEFAULT_VCPU_OVERCOMMIT);
        assert!(!c.enabled);
        assert_eq!(DensityConfig::default().floor_mb(98304), 8192);
        assert_eq!(DensityConfig::default().floor_mb(8192), 1024);
        assert_eq!(DensityConfig::default().vcpu_limit(12), 96);
    }

    /// The cheap `/proc/<pid>/status` counters report what `smaps_rollup`
    /// does (which walks the page tables), and see touched memory.
    #[test]
    #[cfg(target_os = "linux")]
    fn private_memory_from_status_matches_smaps() {
        let mut ballast = vec![0u8; 96 << 20];
        for p in ballast.iter_mut().step_by(4096) {
            *p = 1;
        }
        let pid = std::process::id();
        let fast = process_private_mb(pid).unwrap();
        let t = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap();
        let slow = (imp::kb_for_test(&t, "Anonymous:").unwrap()
            + imp::kb_for_test(&t, "Swap:").unwrap_or(0))
            / 1024;
        assert!(fast >= 90, "touched 96 MiB, measured {fast}");
        assert!(
            fast.abs_diff(slow) <= 24,
            "status {fast} MiB vs smaps_rollup {slow} MiB"
        );
        std::hint::black_box(&ballast);
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn measures_this_process_and_host() {
        let h = host_memory().unwrap();
        assert!(h.total_mb > 256 && h.available_mb <= h.total_mb, "{h:?}");
        let me = process_private_mb(std::process::id()).unwrap();
        assert!(me < h.total_mb);
        // A file just written is (mostly) in the page cache.
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("ram");
        std::fs::write(&f, vec![7u8; 8 << 20]).unwrap();
        let r = file_resident_mb(&f).unwrap();
        assert!(r <= 8, "{r}");
        assert_eq!(process_private_mb(u32::MAX >> 1), None);
        #[cfg(target_os = "macos")]
        {
            let hv = imp::hypervisor_slots().unwrap();
            assert!(hv.max >= 1 && hv.used <= hv.max + 64, "{hv:?}");
        }
        let u = measure(&[("me".into(), std::process::id(), 2, 1024, Some(f.clone()))]);
        assert_eq!(u.vms.len(), 1);
        assert_eq!(u.shared.len(), 1);
        assert!(u.host_cpus >= 1);
    }
}
