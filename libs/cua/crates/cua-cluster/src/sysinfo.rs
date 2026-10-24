//! This machine's memory, for the capacity a host offers.

/// Total physical memory in MiB, when the OS says.
pub fn total_memory_mb() -> Option<u64> {
    imp()
}

/// Memory available for new work in MiB, when the OS says: Linux
/// `MemAvailable`; macOS `kern.memorystatus_level` (the share of memory the
/// kernel counts as available, file cache and compressible idle memory
/// included; what `memory_pressure` prints and its pressure levels follow).
pub fn available_memory_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024)
    }
    #[cfg(target_os = "macos")]
    {
        mac::available_mb()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Memory pressure: 0 normal, 1 warn (compressing or swapping), 2 critical
/// (about to kill processes). macOS `kern.memorystatus_vm_pressure_level`;
/// Linux PSI (`some` / `full` avg10 over 10%).
pub fn memory_pressure() -> u32 {
    #[cfg(target_os = "macos")]
    {
        match mac::sysctl_u64(c"kern.memorystatus_vm_pressure_level") {
            Some(4) => 2,
            Some(2) => 1,
            _ => 0,
        }
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(t) = std::fs::read_to_string("/proc/pressure/memory") else {
            return 0;
        };
        let avg10 = |kind: &str| -> f64 {
            t.lines()
                .find(|l| l.starts_with(kind))
                .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10=")))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0)
        };
        if avg10("full") > 10.0 {
            2
        } else if avg10("some") > 10.0 {
            1
        } else {
            0
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

/// What this machine's VM runtime measures ([`set_vm_capacity_report`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VmCapacity {
    /// VM memory it takes now (MiB), shared snapshot memory counted once.
    pub memory_free_mb: u64,
    /// The median memory of a running VM (MiB); 0: none runs.
    pub typical_mb: u64,
    /// vCPUs per host CPU.
    pub vcpu_overcommit: f32,
}

type VmReport = Box<dyn Fn() -> Option<VmCapacity> + Send + Sync>;
static VM_CAPACITY: std::sync::OnceLock<(f32, VmReport)> = std::sync::OnceLock::new();

/// Sets this machine's VM runtime: its vCPU overcommit and what it
/// measures now (`None`: not measured), for [`fill_measured`] and for what
/// `vm` work reserves here. The daemon does, once.
pub fn set_vm_capacity_report(
    vcpu_overcommit: f32,
    f: impl Fn() -> Option<VmCapacity> + Send + Sync + 'static,
) {
    let _ = VM_CAPACITY.set((vcpu_overcommit.max(1.0), Box::new(f)));
}

/// What this machine's VM runtime measures now, if it reports.
pub fn vm_capacity() -> Option<VmCapacity> {
    VM_CAPACITY.get().and_then(|(_, f)| f())
}

/// The vCPU overcommit of this machine's VM runtime, when one reports (a
/// constant: what `vm` work reserves is the same when accepted and when
/// released).
pub fn vm_overcommit() -> Option<f32> {
    VM_CAPACITY.get().map(|(o, _)| *o)
}

/// What a `vm` Space of `millicpus` (its vCPUs) reserves of this
/// machine's CPU: a share by the VM overcommit when a VM runtime reports.
pub fn vm_millicpus(millicpus: u32) -> u32 {
    match vm_overcommit() {
        Some(o) => (f64::from(millicpus) / f64::from(o)).ceil() as u32,
        None => millicpus,
    }
}

/// Adds what is measured rather than reserved to `st`: the OS's available
/// memory and pressure, and the VM runtime's capacity.
pub fn fill_measured(st: &mut cua_proto::env::v1::HostNodeStatus) {
    let m = st.measured.get_or_insert_with(Default::default);
    m.memory_available_mb = available_memory_mb().unwrap_or(0);
    m.memory_pressure = memory_pressure();
    if let Some(v) = vm_capacity() {
        m.vm_memory_known = true;
        m.vm_memory_free_mb = v.memory_free_mb;
        m.vm_typical_mb = v.typical_mb;
        m.vcpu_overcommit = v.vcpu_overcommit;
    }
}

/// The 1-minute load average (runnable threads), when the OS says.
pub fn load_average() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut v = [0f64; 3];
        // SAFETY: an out array of three doubles and its length.
        let n = unsafe { libc::getloadavg(v.as_mut_ptr(), 3) };
        (n >= 1).then_some(v[0])
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// How many container vCPUs share one host CPU: containers of agents and
/// rollouts mostly wait (`CUA_CONTAINER_CPU_OVERCOMMIT`, default 4). The
/// elastic pool books containers with the same factor.
pub fn container_overcommit() -> u32 {
    std::env::var("CUA_CONTAINER_CPU_OVERCOMMIT")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|v| (1..=64).contains(v))
        .unwrap_or(4)
}

/// What a container Space of `millicpus` (its vCPU limit) reserves of this
/// machine's CPU: its share by [`container_overcommit`].
pub fn container_millicpus(millicpus: u32) -> u32 {
    millicpus.div_ceil(container_overcommit()).max(1)
}

/// This machine as a placement candidate sees it when it has no job runner
/// report: every CPU, and its memory, less what is in use (Linux;
/// elsewhere the total). The CPU is free until Spaces reserve it
/// ([`apply_reservations`]): the load average is never a reason to refuse
/// work (other programs' busy CPUs share), only to rank this machine lower
/// ([`load_share`]).
pub fn this_machine_status() -> cua_proto::env::v1::HostNodeStatus {
    let total_cpu = cpu_count() * 1000;
    let mem = total_memory_mb().unwrap_or(4096);
    let mut st = cua_proto::env::v1::HostNodeStatus {
        arch: std::env::consts::ARCH.into(),
        kinds: vec!["container".into(), "vm".into()],
        millicpus_total: total_cpu,
        millicpus_free: total_cpu,
        memory_total_mb: mem,
        memory_free_mb: available_memory_mb().unwrap_or(mem),
        ..Default::default()
    };
    fill_measured(&mut st);
    st
}

/// This machine's CPUs.
pub fn cpu_count() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

/// The share (0..1) of `cpus` the 1-minute `load` keeps busy. It only
/// lowers this machine's rank among candidates.
pub fn load_share(load: Option<f64>, cpus: u32) -> f64 {
    match load {
        Some(l) if l.is_finite() && cpus > 0 => (l / f64::from(cpus)).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// One local Space (or sandbox) that holds capacity on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalReservation {
    /// A container (its vCPUs share CPUs by [`container_overcommit`]), not
    /// a VM (each vCPU books a CPU).
    pub container: bool,
    /// Its vCPUs (thousandths).
    pub millicpus: u32,
    /// Its memory (MiB).
    pub memory_mb: u64,
    /// Still starting: its memory is not in the OS's figure yet.
    pub starting: bool,
}

impl LocalReservation {
    /// What this Space books of the machine's CPU (thousandths).
    pub fn booked_millicpus(&self) -> u32 {
        if self.container {
            container_millicpus(self.millicpus)
        } else {
            // A VM's vCPUs share CPUs by the VM overcommit when a VM
            // runtime reports one.
            vm_millicpus(self.millicpus)
        }
    }
}

/// What the local Spaces hold of this machine, for the refusal message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reserved {
    /// CPU they book (thousandths).
    pub millicpus: u32,
    /// Memory they reserve (MiB).
    pub memory_mb: u64,
    /// How many of them.
    pub spaces: u32,
    /// This machine's CPU (thousandths), as the CPUs they book count it.
    pub total_millicpus: u32,
}

/// Takes what `reservations` hold out of `st` (this machine's status from
/// [`this_machine_status`]): their CPU, always; their memory only for
/// Spaces still starting when `memory_measured` (the OS's available memory
/// already lacks what running ones use), else all of it. Nothing is counted
/// twice.
pub fn apply_reservations(
    st: &mut cua_proto::env::v1::HostNodeStatus,
    reservations: &[LocalReservation],
    memory_measured: bool,
) -> Reserved {
    let mut held = Reserved {
        total_millicpus: st.millicpus_total,
        ..Default::default()
    };
    let mut unmeasured_mb = 0u64;
    for r in reservations {
        held.millicpus = held.millicpus.saturating_add(r.booked_millicpus());
        held.memory_mb = held.memory_mb.saturating_add(r.memory_mb);
        held.spaces += 1;
        if !memory_measured || r.starting {
            unmeasured_mb = unmeasured_mb.saturating_add(r.memory_mb);
        }
    }
    st.millicpus_free = st.millicpus_total.saturating_sub(held.millicpus);
    st.memory_free_mb = st.memory_free_mb.saturating_sub(unmeasured_mb);
    held
}

/// This machine's status with its local Spaces' reservations taken out,
/// its CPU counted in the vCPUs of a new Space: `factor` of them share a
/// CPU ([`container_overcommit`] for a container, the VM overcommit for a
/// VM, else 1), so the
/// status compares with the Space's own vCPUs.
pub fn this_machine_status_reserving(
    reservations: &[LocalReservation],
    factor: f64,
) -> (cua_proto::env::v1::HostNodeStatus, Reserved) {
    let mut st = this_machine_status();
    let held = apply_reservations(&mut st, reservations, available_memory_mb().is_some());
    let f = if factor.is_finite() {
        factor.max(1.0)
    } else {
        1.0
    };
    st.millicpus_total = (f64::from(st.millicpus_total) * f).round() as u32;
    st.millicpus_free = (f64::from(st.millicpus_free) * f).round() as u32;
    (st, held)
}

/// What an hour of this host costs in US dollars (`CUA_HOST_USD_PER_HOUR`;
/// 0 for machines you own).
pub fn usd_per_hour() -> f64 {
    std::env::var("CUA_HOST_USD_PER_HOUR")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0)
}

/// This machine's accelerators (`HostNodeStatus.accelerators`), detected
/// once: Apple silicon's GPU (`apple:M2 Max:38-core GPU:96 GB`, from
/// `sysctl` and the GPU's IORegistry entry) and NVIDIA GPUs (`nvidia:L4:23
/// GB`, one per GPU, from `nvidia-smi` when it is installed).
pub fn accelerators() -> Vec<String> {
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(detect_accelerators).clone()
}

fn command_output(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn detect_accelerators() -> Vec<String> {
    let mut out = Vec::new();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if let Some(a) = apple_gpu() {
        out.push(a);
    }
    if !cfg!(target_os = "macos")
        && let Some(text) = command_output(
            "nvidia-smi",
            &[
                "--query-gpu=name,memory.total",
                "--format=csv,noheader,nounits",
            ],
        )
    {
        out.extend(parse_nvidia_smi(&text));
    }
    out
}

/// `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`
/// lines (`NVIDIA L4, 23034`) as accelerator words.
pub fn parse_nvidia_smi(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (name, mib) = line.rsplit_once(',')?;
            let name = name.trim();
            let name = name.strip_prefix("NVIDIA ").unwrap_or(name).trim();
            if name.is_empty() {
                return None;
            }
            let mib: u64 = mib.trim().parse().ok()?;
            Some(format!("nvidia:{name}:{} GB", mib.div_ceil(1024)))
        })
        .collect()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn apple_gpu() -> Option<String> {
    let chip = sysctl_string(c"machdep.cpu.brand_string")?;
    let chip = chip
        .trim()
        .strip_prefix("Apple ")
        .unwrap_or(chip.trim())
        .to_string();
    let mem_gb = total_memory_mb().map(|mb| (mb + 512) / 1024);
    let cores = command_output(
        "/usr/sbin/ioreg",
        &["-r", "-c", "AGXAccelerator", "-d", "1"],
    )
    .and_then(|t| parse_gpu_cores(&t));
    let mut word = format!("apple:{chip}");
    if let Some(c) = cores {
        word.push_str(&format!(":{c}-core GPU"));
    }
    if let Some(gb) = mem_gb {
        word.push_str(&format!(":{gb} GB"));
    }
    Some(word)
}

/// The `"gpu-core-count" = N` of an `ioreg -r -c AGXAccelerator` listing.
pub fn parse_gpu_cores(ioreg: &str) -> Option<u32> {
    ioreg.lines().find_map(|l| {
        let (_, rest) = l.split_once("\"gpu-core-count\"")?;
        rest.trim().trim_start_matches('=').trim().parse().ok()
    })
}

#[cfg(target_os = "macos")]
fn sysctl_string(name: &std::ffi::CStr) -> Option<String> {
    let mut len: usize = 0;
    // SAFETY: a valid NUL-terminated name; a null buffer asks for the size.
    let r = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: a buffer of `len` bytes and its length.
    let r = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    buf.truncate(len);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf8(buf).ok()
}

/// This host's IPv4 addresses for native jobs' peers
/// (`HostNodeStatus.addresses`, `ip/prefix` with the interface's netmask): LAN (private) first, then tailnet
/// (100.64.0.0/10), then public. Loopback, link-local, and container or VM
/// bridges (`docker*`, `br-*`, `veth*`, `bridge*`, `vmnet*`, `virbr*`, ...)
/// are left out, and so are tunnels (`utun*`, `tun*`, `wg*`) unless their
/// address is a tailnet one. Re-read at most every 30 s.
pub fn addresses() -> Vec<String> {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, Vec<String>)>> =
        std::sync::Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = g.as_ref()
        && at.elapsed() < std::time::Duration::from_secs(30)
    {
        return v.clone();
    }
    let v = order_addresses(interfaces());
    *g = Some((std::time::Instant::now(), v.clone()));
    v
}

/// This machine's own address on the network `guest` is on (a VM's NAT,
/// a container bridge): where the guest reaches this machine.
pub fn gateway_for(guest: std::net::Ipv4Addr) -> Option<std::net::Ipv4Addr> {
    gateway_among(&interfaces(), guest)
}

/// [`gateway_for`] among `(interface, address, prefix length)` triples:
/// the narrowest network holding `guest` (not `guest` itself).
pub fn gateway_among(
    ifaces: &[(String, std::net::Ipv4Addr, u8)],
    guest: std::net::Ipv4Addr,
) -> Option<std::net::Ipv4Addr> {
    ifaces
        .iter()
        .filter(|(_, ip, prefix)| {
            let p = u32::from((*prefix).min(32));
            let mask = if p == 0 { 0 } else { u32::MAX << (32 - p) };
            *ip != guest
                && !ip.is_loopback()
                && *prefix > 0
                && (u32::from(*ip) & mask) == (u32::from(guest) & mask)
        })
        .max_by_key(|(_, _, prefix)| *prefix)
        .map(|(_, ip, _)| *ip)
}

/// Whether `ip` is in the tailnet range 100.64.0.0/10.
pub fn is_tailnet(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (o[1] & 0xc0) == 64
}

fn bridge_like(name: &str) -> bool {
    const PREFIXES: [&str; 14] = [
        "docker", "br-", "veth", "bridge", "vmnet", "virbr", "lxc", "lxd", "cni", "flannel",
        "cali", "podman", "vboxnet", "anpi",
    ];
    PREFIXES.iter().any(|p| name.starts_with(p))
}

fn tunnel_like(name: &str) -> bool {
    [
        "utun",
        "tun",
        "wg",
        "tailscale",
        "ipsec",
        "ppp",
        "gif",
        "stf",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// Keeps and orders `(interface, address, prefix length)` triples as
/// [`addresses`] does, as `ip/prefix` (the interface's own netmask).
pub fn order_addresses(ifaces: Vec<(String, std::net::Ipv4Addr, u8)>) -> Vec<String> {
    let mut keep: Vec<(u8, String)> = Vec::new();
    for (name, ip, prefix) in ifaces {
        if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || bridge_like(&name) {
            continue;
        }
        let tailnet = is_tailnet(ip);
        if tunnel_like(&name) && !tailnet {
            continue;
        }
        let rank = if ip.is_private() {
            0
        } else if tailnet {
            1
        } else {
            2
        };
        let s = format!("{ip}/{prefix}");
        if !keep.iter().any(|(_, k)| *k == s) {
            keep.push((rank, s));
        }
    }
    keep.sort_by_key(|(r, _)| *r);
    keep.into_iter().map(|(_, s)| s).collect()
}

#[cfg(unix)]
fn interfaces() -> Vec<(String, std::net::Ipv4Addr, u8)> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list freed by freeifaddrs.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list getifaddrs returned.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || (ifa.ifa_flags & libc::IFF_UP as libc::c_uint) == 0 {
            continue;
        }
        // SAFETY: a non-null sockaddr of the list.
        let family = i32::from(unsafe { (*ifa.ifa_addr).sa_family });
        if family != libc::AF_INET {
            continue;
        }
        // SAFETY: an AF_INET address is a sockaddr_in.
        let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
        let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
        let prefix = if ifa.ifa_netmask.is_null() {
            32
        } else {
            // SAFETY: an AF_INET address's netmask is a sockaddr_in.
            let m = unsafe { &*(ifa.ifa_netmask as *const libc::sockaddr_in) };
            u32::from_be(m.sin_addr.s_addr).count_ones() as u8
        };
        // SAFETY: ifa_name is a NUL-terminated string of the list.
        let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }
            .to_string_lossy()
            .into_owned();
        out.push((name, ip, prefix));
    }
    // SAFETY: the list getifaddrs returned, freed once.
    unsafe { libc::freeifaddrs(head) };
    out
}

#[cfg(not(unix))]
fn interfaces() -> Vec<(String, std::net::Ipv4Addr, u8)> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn imp() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

#[cfg(target_os = "macos")]
fn imp() -> Option<u64> {
    mac::sysctl_u64(c"hw.memsize")
        .filter(|b| *b > 0)
        .map(|b| b / (1024 * 1024))
}

/// macOS memory figures (the same as `cua_vmm::density::host_memory`,
/// which this crate does not depend on).
#[cfg(target_os = "macos")]
mod mac {
    pub fn sysctl_u64(name: &std::ffi::CStr) -> Option<u64> {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: a valid NUL-terminated name, an out pointer to a u64 and
        // its length; sysctlbyname writes at most `len` bytes.
        let r = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut v as *mut u64).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        // A 4-byte value lands in the low half (little endian).
        (r == 0).then_some(if len == 4 { v & 0xffff_ffff } else { v })
    }

    /// The classic `vm_statistics64` every macOS fills.
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

    pub fn available_mb() -> Option<u64> {
        let total = sysctl_u64(c"hw.memsize")?;
        let mut s = VmStats::default();
        let mut count = (std::mem::size_of::<VmStats>() / std::mem::size_of::<i32>()) as u32;
        // SAFETY: `s` is a buffer of `count` integers, which
        // host_statistics64 fills at most.
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
        // The kernel's own figure, the one its pressure levels follow (and
        // `memory_pressure` prints): idle app memory it can compress counts
        // as available.
        if let Some(level) = sysctl_u64(c"kern.memorystatus_level").filter(|l| *l <= 100) {
            return Some((total >> 20) * level / 100);
        }
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        let app = u64::from(s.internal_page_count).saturating_sub(u64::from(s.purgeable_count));
        let used = (app + u64::from(s.wire_count) + u64::from(s.compressor_page_count)) * page;
        Some(total.saturating_sub(used) >> 20)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn imp() -> Option<u64> {
    None
}

/// Why this machine cannot start another job for lack of open-file
/// capacity, when the system file table has less than
/// [`cua_home::filetable::MIN_FREE_ENV`] percent (default 10) free: a
/// sentence and its fix, naming the processes holding the most
/// descriptors. `None` when there is room or the OS does not say.
pub fn file_table_shortfall() -> Option<String> {
    let table = cua_home::filetable::read()?;
    shortfall(table, cua_home::filetable::min_free(), || {
        cua_home::filetable::top_holders(3)
    })
}

fn shortfall(
    table: cua_home::filetable::FileTable,
    min_free: f64,
    holders: impl FnOnce() -> Vec<(String, u32, u64)>,
) -> Option<String> {
    cua_home::filetable::check(table, min_free)
        .err()
        .map(|t| cua_home::filetable::refusal(t, "another job", &holders()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nearly_full_file_table_refuses_jobs_with_a_fix() {
        use cua_home::filetable::FileTable;
        let full = FileTable {
            open: 250_000,
            max: 262_144,
        };
        let who = || {
            vec![(
                "com.apple.Virtualization.VirtualMachine".to_string(),
                7,
                151_000,
            )]
        };
        let why = shortfall(full, 0.10, who).expect("refused");
        assert!(why.contains("12144 of 262144"), "{why}");
        assert!(why.contains("pid 7"), "{why}");
        assert!(why.contains("cua daemon stop"), "{why}");
        let roomy = FileTable {
            open: 5_000,
            max: 262_144,
        };
        assert_eq!(shortfall(roomy, 0.10, Vec::new), None);
        assert_eq!(shortfall(full, 0.0, Vec::new), None);
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn memory_is_known() {
        assert!(super::total_memory_mb().unwrap() > 256);
    }

    #[test]
    #[cfg(unix)]
    fn this_machine_reports_what_it_has() {
        let s = super::this_machine_status();
        assert!(s.millicpus_total >= 1000);
        assert!(s.millicpus_free <= s.millicpus_total);
        assert!(s.memory_free_mb <= s.memory_total_mb);
        assert!(super::load_average().is_some());
        assert!(s.measured.unwrap().memory_pressure <= 2);
        // The load average never takes CPU away.
        assert_eq!(s.millicpus_free, s.millicpus_total);
    }

    fn mach(cpus: u32, mem: u64) -> cua_proto::env::v1::HostNodeStatus {
        cua_proto::env::v1::HostNodeStatus {
            millicpus_total: cpus * 1000,
            millicpus_free: cpus * 1000,
            memory_total_mb: mem,
            memory_free_mb: mem,
            ..Default::default()
        }
    }

    fn res(container: bool, vcpus: u32, mem: u64, starting: bool) -> super::LocalReservation {
        super::LocalReservation {
            container,
            millicpus: vcpus * 1000,
            memory_mb: mem,
            starting,
        }
    }

    #[test]
    fn containers_book_a_share_of_cpu_and_vms_their_vcpus() {
        // Default overcommit 4 (the env var is not set in tests).
        assert_eq!(res(true, 2, 0, false).booked_millicpus(), 500);
        // A VM books its vCPUs by the VM overcommit when a VM runtime reports
        // one (a process-wide setting other tests may have made).
        assert_eq!(
            res(false, 2, 0, false).booked_millicpus(),
            super::vm_millicpus(2000)
        );
        assert_eq!(super::container_millicpus(1), 1);
    }

    #[test]
    fn reservations_come_off_free_cpu_and_unmeasured_memory_only() {
        // Measured memory: running Spaces are already out of the OS's
        // figure; only the one still starting comes off it.
        let mut st = mach(12, 32768);
        st.memory_free_mb = 20000;
        let held = super::apply_reservations(
            &mut st,
            &[res(true, 16, 8192, false), res(true, 8, 4096, true)],
            true,
        );
        assert_eq!(st.millicpus_free, 12000 - 4000 - 2000);
        assert_eq!(st.memory_free_mb, 20000 - 4096);
        assert_eq!(
            (
                held.millicpus,
                held.memory_mb,
                held.spaces,
                held.total_millicpus
            ),
            (6000, 12288, 2, 12000)
        );
        // Unmeasured memory: all of it comes off the total.
        let mut st = mach(4, 8192);
        super::apply_reservations(&mut st, &[res(false, 2, 4096, false)], false);
        assert_eq!(st.memory_free_mb, 4096);
        // Never below zero.
        super::apply_reservations(&mut st, &[res(false, 64, 1 << 20, false)], false);
        assert_eq!((st.millicpus_free, st.memory_free_mb), (0, 0));
    }

    #[test]
    fn load_is_a_share_of_the_cpus_for_ranking() {
        assert_eq!(super::load_share(Some(23.0), 12), 1.0);
        assert_eq!(super::load_share(Some(3.0), 12), 0.25);
        assert_eq!(super::load_share(None, 12), 0.0);
        assert_eq!(super::load_share(Some(f64::NAN), 12), 0.0);
    }

    #[test]
    fn measured_vm_capacity_reaches_the_status() {
        super::set_vm_capacity_report(8.0, || {
            Some(super::VmCapacity {
                memory_free_mb: 7000,
                typical_mb: 300,
                vcpu_overcommit: 8.0,
            })
        });
        let mut st = cua_proto::env::v1::HostNodeStatus::default();
        super::fill_measured(&mut st);
        let m = st.measured.unwrap();
        assert!(m.vm_memory_known);
        assert_eq!(
            (m.vm_memory_free_mb, m.vm_typical_mb, m.vcpu_overcommit),
            (7000, 300, 8.0)
        );
        assert_eq!(super::vm_overcommit(), Some(8.0));
        assert_eq!(super::vm_millicpus(2000), 250);
        assert_eq!(super::vm_millicpus(1), 1);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert!(m.memory_available_mb > 0);
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn available_memory_is_at_most_the_total() {
        let avail = super::available_memory_mb().unwrap();
        assert!(avail > 0 && avail <= super::total_memory_mb().unwrap());
    }

    #[test]
    fn addresses_put_lan_first_and_leave_bridges_out() {
        use std::net::Ipv4Addr;
        let v = super::order_addresses(vec![
            ("lo0".into(), Ipv4Addr::new(127, 0, 0, 1), 8),
            ("utun4".into(), Ipv4Addr::new(100, 101, 7, 9), 32),
            ("utun5".into(), Ipv4Addr::new(10, 8, 0, 2), 24),
            ("en0".into(), Ipv4Addr::new(203, 0, 113, 5), 24),
            ("bridge100".into(), Ipv4Addr::new(192, 168, 64, 1), 24),
            ("docker0".into(), Ipv4Addr::new(172, 17, 0, 1), 16),
            ("en1".into(), Ipv4Addr::new(172, 31, 20, 9), 20),
            ("en2".into(), Ipv4Addr::new(169, 254, 3, 3), 16),
        ]);
        assert_eq!(v, ["172.31.20.9/20", "100.101.7.9/32", "203.0.113.5/24"]);
        assert!(super::is_tailnet(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(super::is_tailnet(Ipv4Addr::new(100, 127, 255, 1)));
        assert!(!super::is_tailnet(Ipv4Addr::new(100, 128, 0, 1)));
    }

    #[test]
    fn a_guests_gateway_is_this_machines_address_on_its_network() {
        use std::net::Ipv4Addr;
        let ifaces = vec![
            ("lo0".into(), Ipv4Addr::new(127, 0, 0, 1), 8),
            ("en0".into(), Ipv4Addr::new(192, 168, 1, 188), 24),
            ("bridge100".into(), Ipv4Addr::new(192, 168, 64, 1), 24),
            ("docker0".into(), Ipv4Addr::new(172, 17, 0, 1), 16),
        ];
        assert_eq!(
            super::gateway_among(&ifaces, Ipv4Addr::new(192, 168, 64, 172)),
            Some(Ipv4Addr::new(192, 168, 64, 1))
        );
        assert_eq!(
            super::gateway_among(&ifaces, Ipv4Addr::new(172, 17, 0, 3)),
            Some(Ipv4Addr::new(172, 17, 0, 1))
        );
        assert_eq!(
            super::gateway_among(&ifaces, Ipv4Addr::new(10, 9, 9, 9)),
            None
        );
    }

    #[test]
    fn this_host_reports_some_address_or_none_quickly() {
        let v = super::addresses();
        assert!(v.iter().all(|a| {
            let (ip, len) = a.split_once('/').unwrap();
            ip.parse::<std::net::Ipv4Addr>().is_ok() && len.parse::<u8>().is_ok_and(|l| l <= 32)
        }));
        assert!(!v.iter().any(|a| a.starts_with("127.")));
    }

    #[test]
    fn nvidia_smi_and_ioreg_parse() {
        assert_eq!(
            super::parse_nvidia_smi("NVIDIA L4, 23034\nNVIDIA A100-SXM4-40GB, 40960\n"),
            ["nvidia:L4:23 GB", "nvidia:A100-SXM4-40GB:40 GB"]
        );
        assert_eq!(
            super::parse_gpu_cores("  |   \"gpu-core-count\" = 38\n  \"x\" = 1"),
            Some(38)
        );
        assert_eq!(super::parse_gpu_cores("nothing"), None);
    }

    #[test]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn apple_silicon_reports_its_gpu() {
        let a = super::accelerators();
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(a[0].starts_with("apple:") && a[0].ends_with(" GB"), "{a:?}");
    }
}
