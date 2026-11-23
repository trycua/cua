//! The built-in Linux runtime on Windows as a Hyper-V virtual machine: the
//! second engine next to [`super::wsl`], for PCs and servers where Hyper-V is
//! already on and WSL 2 is not (Windows Server has no WSL 2), and for anyone
//! who sets `runtime.windows_backend = "hyperv"`.
//!
//! # How it works
//!
//! It is the Mac's runtime ([`super`]: Colima on Virtualization.framework)
//! moved to Hyper-V, with the same disk.
//!
//! * **The disk.** `ghcr.io/trycua/runtime-gvisor` (the same pinned layer the
//!   Mac downloads: Ubuntu 24.04 with Docker and gVisor's `runsc` baked in). Its
//!   amd64 build carries a small initramfs with the Hyper-V storage driver
//!   (`libs/images/runtime-gvisor/build.sh`): the Colima base image has none and
//!   its kernel finds the root disk only through built-in virtio drivers, so on
//!   Hyper-V's synthetic SCSI controller (a module, `hv_storvsc`) it panics with
//!   "Cannot open root device". (A generation 1 machine does not help: the
//!   kernel's `ata_piix` ignores emulated IDE disks under Hyper-V on purpose.) It unpacks to a raw disk; cua writes it as a fixed VHD
//!   (the raw bytes plus the 512-byte footer, [`vhd_fixed_footer`]) and
//!   `Convert-VHD` turns that into a dynamic VHDX, which Hyper-V boots as a
//!   generation 2 (UEFI) machine. No `qemu-img` is involved, and the VHDX is
//!   not a sparse file (Hyper-V refuses those).
//! * **The first boot** is configured by a cloud-init seed cua writes itself
//!   ([`crate::cloudinit::Seed`]): the `cua` user with this home's own SSH
//!   key, Docker's `daemon.json` (gVisor the default runtime, as on the Mac;
//!   bridge networks away from the subnets Hyper-V and WSL pick) and a small
//!   service that pings the switch's gateway at every boot so the host learns
//!   the VM's address.
//! * **The network** is Hyper-V's Default Switch (NAT and DHCP, no setup). The
//!   VM's MAC is fixed ([`mac`]); the host finds its address from the VM's
//!   integration reports when it has them, else from the neighbor table by
//!   that MAC.
//! * **A pipe, not a TCP port.** As for WSL, Windows reaches the engine at the
//!   named pipe `\\.\pipe\cua-engine-<tag>`; every connection runs
//!   `ssh cua@<vm> docker system dial-stdio` with this home's key
//!   ([`ssh_args`]). Docker is never published on a TCP port.
//! * **Lifecycle.** Created on the first Linux Space that needs it, started on
//!   demand, powered off when no Linux Space runs ([`stop`]); [`remove`]
//!   deletes the VM, its disk and the key.
//! * **Permission.** Hyper-V is managed by administrators and the
//!   `Hyper-V Administrators` group. [`probe`] says plainly when this account
//!   is in neither, and when the feature is off.
//!
//! Differences from the WSL engine: a VM of its own (the Spaces share nothing
//! with the user's other WSL distributions), memory and processors set by cua
//! rather than `.wslconfig`, and gVisor isolation as on the Mac.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;

use super::wsl;
use crate::error::{Result, VmmError};
use crate::host;

/// The VM's name is this prefix and the home's tag ([`vm_name`]).
pub const VM_PREFIX: &str = "cua-runtime-";

/// Hyper-V's built-in NAT switch (Windows 10 and 11 have it, Windows Server does not).
pub const SWITCH: &str = "Default Switch";

/// Without a Default Switch the VM gets a switch, NAT and addresses of its own
/// (this takes an administrator once). The subnet is away from Docker's pools
/// ([`wsl::BRIDGE_CIDR`], [`wsl::ADDRESS_POOL`]) and the Default Switch's.
pub const OWN_SWITCH: &str = "cua-runtime";
pub const OWN_PREFIX: &str = "10.232.0.0/24";
pub const OWN_GATEWAY: &str = "10.232.0.1";
pub const OWN_GUEST: &str = "10.232.0.2";

/// The Linux user cua logs in as (SSH key only, passwordless sudo).
pub const GUEST_USER: &str = "cua";

/// The VM's disk can grow to this many GiB (a dynamic VHDX: it takes what
/// the images and Spaces use).
pub const DISK_GIB: u64 = 60;

/// Bumped when a setup change needs the VM made again.
pub const SETUP_VERSION: u32 = 1;

/// Which engine the Windows runtime uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    /// WSL 2 when it works, else Hyper-V when it does, and whichever this
    /// home already set up.
    #[default]
    Auto,
    /// Only WSL 2.
    Wsl,
    /// Only Hyper-V.
    Hyperv,
}

impl Backend {
    /// `auto`, `wsl` or `hyperv`.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Self::Auto),
            "wsl" | "wsl2" | "wsl 2" => Some(Self::Wsl),
            "hyperv" | "hyper-v" => Some(Self::Hyperv),
            _ => None,
        }
    }

    /// The setting's word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Wsl => "wsl",
            Self::Hyperv => "hyperv",
        }
    }

    /// The setting now (the environment, else the config file, else auto).
    pub fn current() -> Self {
        crate::kinds::windows_backend_word()
            .and_then(|w| Self::parse(&w))
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Names and paths
// ---------------------------------------------------------------------------

/// This home's VM.
pub fn vm_name() -> String {
    format!("{VM_PREFIX}{}", wsl::tag())
}

/// The VM's fixed MAC (Hyper-V's `00-15-5D` prefix and the home's tag), 12 hex
/// digits: it is how the host finds the VM's address.
pub fn mac() -> String {
    format!("00155D{}", wsl::tag()[..6].to_ascii_uppercase())
}

/// `<runtime root>/hyperv`: the VM's disk, seed and key.
pub fn dir() -> PathBuf {
    super::root().join("hyperv")
}

/// The VM's disk.
pub fn disk_path() -> PathBuf {
    dir().join("disk.vhdx")
}

/// The cloud-init seed the VM boots with.
pub fn seed_path() -> PathBuf {
    dir().join("seed.iso")
}

/// The private key cua logs into the VM with (this user's: no one else can read it).
pub fn key_path() -> PathBuf {
    dir().join("ssh").join("id_ed25519")
}

fn marker() -> PathBuf {
    dir().join("provisioned")
}

/// Whether this home's VM is on a switch of its own (written by setup).
pub fn own_network() -> bool {
    std::fs::read_to_string(dir().join("network"))
        .map(|s| s.trim() == "own")
        .unwrap_or(false)
}

/// The VM's network configuration when it has addresses of its own: a static
/// address on the NIC with [`mac`].
pub fn network_config(dns: &[String]) -> String {
    let m = mac();
    let colons = m
        .as_bytes()
        .chunks(2)
        .map(|c| String::from_utf8_lossy(c).to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(":");
    let servers = dns.join(", ");
    format!(
        "version: 2\nethernets:\n  cua0:\n    match:\n      macaddress: \"{colons}\"\n    set-name: cua0\n    addresses: [{OWN_GUEST}/24]\n    routes:\n      - to: default\n        via: {OWN_GATEWAY}\n    nameservers:\n      addresses: [{servers}]\n"
    )
}

/// The name servers the VM uses: the host's own (so a network that only lets its
/// own DNS through works), then public ones, at most four. The runtime image has no
/// systemd-resolved, so `/etc/resolv.conf` is written from this list.
pub fn dns_servers(host: &[String]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for s in host
        .iter()
        .map(|s| s.trim().to_string())
        .chain(["1.1.1.1".to_string(), "8.8.8.8".to_string()])
    {
        let usable = s
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified())
            && s != OWN_GATEWAY;
        if usable && !v.contains(&s) {
            v.push(s);
        }
    }
    v.truncate(4);
    v
}

/// The host's IPv4 name servers (what Windows uses now).
async fn host_dns() -> Vec<String> {
    let out = ps(
        "Get-DnsClientServerAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue | \
         ForEach-Object { $_.ServerAddresses } | Where-Object { $_ } | Select-Object -Unique | \
         ForEach-Object { $_ }",
        60,
    )
    .await
    .unwrap_or_default();
    out.lines().map(|l| l.trim().to_string()).collect()
}

/// Whether setup finished for this home (the VM may be off).
pub fn provisioned() -> bool {
    std::fs::read_to_string(marker())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(|v| v == SETUP_VERSION)
}

// ---------------------------------------------------------------------------
// The disk: raw bytes as a fixed VHD
// ---------------------------------------------------------------------------

/// The CHS geometry the VHD specification derives from a size.
pub fn vhd_geometry(size: u64) -> (u16, u8, u8) {
    let sectors = (size / 512).min(65535 * 16 * 255);
    let (spt, heads, cth);
    if sectors >= 65535 * 16 * 63 {
        spt = 255u64;
        heads = 16u64;
        cth = sectors / spt;
    } else {
        let mut s = 17u64;
        let mut c = sectors / s;
        let mut h = c.div_ceil(1024).max(4);
        if c >= h * 1024 || h > 16 {
            s = 31;
            h = 16;
            c = sectors / s;
        }
        if c >= h * 1024 {
            s = 63;
            h = 16;
            c = sectors / s;
        }
        spt = s;
        heads = h;
        cth = c;
    }
    ((cth / heads) as u16, heads as u8, spt as u8)
}

/// The 512-byte footer that makes `size` bytes of raw disk a fixed VHD:
/// `Convert-VHD` reads that and writes a VHDX. `timestamp` counts seconds since
/// 2000-01-01.
pub fn vhd_fixed_footer(size: u64, id: [u8; 16], timestamp: u32) -> [u8; 512] {
    let mut f = [0u8; 512];
    f[0..8].copy_from_slice(b"conectix");
    f[8..12].copy_from_slice(&2u32.to_be_bytes()); // features: reserved bit
    f[12..16].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // file format version
    f[16..24].copy_from_slice(&u64::MAX.to_be_bytes()); // data offset: fixed disk
    f[24..28].copy_from_slice(&timestamp.to_be_bytes());
    f[28..32].copy_from_slice(b"cua ");
    f[32..36].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    f[36..40].copy_from_slice(b"Wi2k");
    f[40..48].copy_from_slice(&size.to_be_bytes()); // original size
    f[48..56].copy_from_slice(&size.to_be_bytes()); // current size
    let (cylinders, heads, sectors) = vhd_geometry(size);
    f[56..58].copy_from_slice(&cylinders.to_be_bytes());
    f[58] = heads;
    f[59] = sectors;
    f[60..64].copy_from_slice(&2u32.to_be_bytes()); // disk type: fixed
    f[68..84].copy_from_slice(&id);
    let sum: u32 = f.iter().map(|b| *b as u32).fold(0u32, u32::wrapping_add);
    f[64..68].copy_from_slice(&(!sum).to_be_bytes());
    f
}

/// Unpacks the gzipped raw disk at `gz` into `out` with its VHD footer; `done`
/// hears the compressed bytes read so far. Returns the virtual size.
#[cfg(feature = "lume")]
pub fn gz_to_fixed_vhd(
    gz: &std::path::Path,
    out: &std::path::Path,
    done: &dyn Fn(u64),
) -> Result<u64> {
    use std::io::{BufReader, BufWriter, Read, Write};
    struct Counting<'a, R> {
        inner: R,
        read: u64,
        done: &'a dyn Fn(u64),
    }
    impl<R: Read> Read for Counting<'_, R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read += n as u64;
            (self.done)(self.read);
            Ok(n)
        }
    }
    let file = std::fs::File::open(gz)?;
    let counting = Counting {
        inner: BufReader::new(file),
        read: 0,
        done,
    };
    let mut decoder = flate2::read::GzDecoder::new(counting);
    let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(out)?);
    let mut size = std::io::copy(&mut decoder, &mut w)?;
    // A raw disk is whole sectors; pad in case it is not.
    if size % 512 != 0 {
        let pad = 512 - size % 512;
        w.write_all(&vec![0u8; pad as usize])?;
        size += pad;
    }
    let id = disk_id(gz, size);
    let since_2000 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(946_684_800) as u32)
        .unwrap_or(0);
    w.write_all(&vhd_fixed_footer(size, id, since_2000))?;
    w.flush()?;
    Ok(size)
}

/// 16 stable bytes for a disk's unique id.
#[cfg(feature = "lume")]
fn disk_id(gz: &std::path::Path, size: u64) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(format!("{}:{size}:{}", gz.display(), wsl::tag()).as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&d[..16]);
    id
}

// ---------------------------------------------------------------------------
// What the VM boots with
// ---------------------------------------------------------------------------

/// Docker's `daemon.json` in the VM: gVisor the default runtime as in the
/// Mac's profile ([`super::profile_yaml`]), and bridge networks away from the
/// subnets Hyper-V's Default Switch and WSL pick.
pub fn daemon_json() -> String {
    serde_json::json!({
        "bip": wsl::BRIDGE_CIDR,
        "default-address-pools": [{"base": wsl::ADDRESS_POOL, "size": 24}],
        "default-runtime": "runsc",
        "runtimes": {
            "runsc": {"path": "/usr/local/bin/runsc", "runtimeArgs": ["--allow-suid"]},
            "cua-runsc": {"path": "/usr/local/lib/cua/cua-runsc", "runtimeArgs": ["--allow-suid"]}
        },
        "log-driver": "local",
        "log-opts": {"max-size": "20m", "max-file": "3"}
    })
    .to_string()
}

/// A small program, run at every boot, that pings the switch's gateway until it
/// answers: the host's neighbor table then has the VM's address.
const ANNOUNCE: &str = "#!/bin/sh\nfor i in $(seq 1 60); do\n  g=$(ip route | awk '/default/ {print $3; exit}')\n  if [ -n \"$g\" ] && ping -c1 -W1 \"$g\" >/dev/null 2>&1; then exit 0; fi\n  sleep 2\ndone\nexit 0\n";

const ANNOUNCE_UNIT: &str = "[Unit]\nDescription=Tell the host this VM's address\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=oneshot\nExecStart=/usr/local/bin/cua-announce\n\n[Install]\nWantedBy=multi-user.target\n";

/// The VM's cloud-config: the `cua` user with `public_key`, Docker's
/// configuration and the address announcement.
pub fn guest_user_data(public_key: &str, dns: &[String]) -> String {
    let resolv: String = dns.iter().map(|d| format!("nameserver {d}\\n")).collect();
    let indent = |s: &str| {
        s.lines()
            .map(|l| format!("      {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "#cloud-config\n\
         users:\n\
         \x20 - default\n\
         \x20 - name: {GUEST_USER}\n\
         \x20   groups: [docker]\n\
         \x20   sudo: ALL=(ALL) NOPASSWD:ALL\n\
         \x20   shell: /bin/bash\n\
         \x20   lock_passwd: true\n\
         \x20   ssh_authorized_keys:\n\
         \x20     - {key}\n\
         ssh_pwauth: false\n\
         disable_root: true\n\
         manage_etc_hosts: localhost\n\
         bootcmd:\n\
         \x20 - |\n\
         \x20   rm -f /etc/resolv.conf\n\
         \x20   printf '{resolv}' > /etc/resolv.conf\n\
         write_files:\n\
         \x20 - path: /etc/docker/daemon.json\n\
         \x20   permissions: '0644'\n\
         \x20   content: |\n\
         {daemon}\n\
         \x20 - path: /usr/local/bin/cua-announce\n\
         \x20   permissions: '0755'\n\
         \x20   content: |\n\
         {announce}\n\
         \x20 - path: /etc/systemd/system/cua-announce.service\n\
         \x20   permissions: '0644'\n\
         \x20   content: |\n\
         {unit}\n\
         runcmd:\n\
         \x20 - [systemctl, daemon-reload]\n\
         \x20 - [systemctl, enable, --now, cua-announce.service]\n\
         \x20 - [systemctl, restart, docker]\n",
        key = public_key.trim(),
        daemon = indent(&daemon_json()),
        announce = indent(ANNOUNCE),
        unit = indent(ANNOUNCE_UNIT),
    )
}

/// The arguments of the `ssh` that runs `remote` in the VM at `ip` with `key`.
/// The link is a private NAT on this PC; the host key is not pinned.
pub fn ssh_args(ip: &str, key: &std::path::Path, remote: &str) -> Vec<String> {
    let mut v: Vec<String> = [
        "-i",
        &key.display().to_string(),
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=NUL",
        "-o",
        "LogLevel=ERROR",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=20",
        "-T",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.push(format!("{GUEST_USER}@{ip}"));
    v.push(remote.to_string());
    v
}

/// What the pipe's clients run in the VM: Docker's socket on stdin and stdout.
pub const DIAL_COMMAND: &str = "docker system dial-stdio";

/// The arguments of the `ssh` that carries one TCP connection to `127.0.0.1:port`
/// in the VM (`-W`: its stdin and stdout are the connection).
pub fn ssh_forward_args(ip: &str, key: &std::path::Path, port: u16) -> Vec<String> {
    let mut v = ssh_args(ip, key, "");
    // ssh_args ends with the destination and the (empty) command: replace both.
    v.truncate(v.len() - 2);
    v.push("-W".into());
    v.push(format!("127.0.0.1:{port}"));
    v.push(format!("{GUEST_USER}@{ip}"));
    v
}

/// Run in the VM (as one long-lived `ssh`): every second, the ports Docker publishes
/// there (each is held by a `docker-proxy` on the VM's loopback), as one line of numbers.
pub const PORTS_SCRIPT: &str = "while true; do sudo ss -ltnHp 2>/dev/null | awk '/docker-proxy/ {n = split($4, a, \":\"); print a[n]}' | sort -un | tr '\\n' ' '; echo; sleep 1; done";

/// The ports on one line of [`PORTS_SCRIPT`]'s output.
pub fn parse_ports(line: &str) -> Vec<u16> {
    let mut v: Vec<u16> = line
        .split_whitespace()
        .filter_map(|w| w.parse::<u16>().ok())
        .filter(|p| *p >= 1024)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

// ---------------------------------------------------------------------------
// What is there
// ---------------------------------------------------------------------------

/// What `doctor` and `ensure` find out about Hyper-V.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Hyper-V is on and this account may manage it. `vm` is this home's VM
    /// state (`Off`, `Running`, `Saved`) when it exists.
    Available { vm: Option<String> },
    /// What to turn on or fix.
    Unavailable(String),
}

/// Hyper-V is not on.
pub const NOT_ENABLED: &str = "Hyper-V is not turned on. Cua Spaces can run Linux in a Hyper-V \
    virtual machine on Windows Pro, Enterprise, Education and Server: turn on the Hyper-V feature \
    (an administrator runs `Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V \
    -All` on Windows, or `Install-WindowsFeature Hyper-V -IncludeManagementTools` on Windows \
    Server) and restart, or use WSL 2";

/// This account may not manage Hyper-V.
pub const NOT_PERMITTED: &str = "This account cannot manage Hyper-V. Add it to the Hyper-V \
    Administrators group (an administrator runs `net localgroup \"Hyper-V Administrators\" \
    <user> /add`), then sign out and in again";

/// Windows Server (no Default Switch) and this session is not elevated.
pub const NO_SWITCH_NOT_ADMIN: &str = "This PC has no Hyper-V Default Switch (Windows Server), so \
    the Linux runtime's virtual machine needs a private network that only an administrator can \
    make: run Cua from an elevated (administrator) PowerShell once to set it up";

#[derive(Deserialize, Default)]
struct Facts {
    #[serde(default)]
    role: bool,
    #[serde(default)]
    vmms: String,
    #[serde(default)]
    can: bool,
    #[serde(default)]
    vm: String,
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    switch: bool,
}

/// The PowerShell that prints the facts [`classify_probe`] reads, as JSON.
pub fn probe_script() -> String {
    format!(
        "$ErrorActionPreference = 'SilentlyContinue'\n\
         $role = [bool](Get-Service -Name vmms) -and [bool](Get-Module -ListAvailable -Name Hyper-V)\n\
         $svc = (Get-Service -Name vmms).Status\n\
         $can = $false\n\
         $vm = ''\n\
         $switch = $false\n\
         $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)\n\
         if ($role) {{\n\
           try {{ Get-VM -ErrorAction Stop | Out-Null; $can = $true }} catch {{ }}\n\
           if ($can) {{ $v = Get-VM -Name '{name}' -ErrorAction SilentlyContinue; if ($v) {{ $vm = $v.State.ToString() }} }}\n\
           if ($can) {{ $switch = [bool](Get-VMSwitch -Name '{default_switch}' -ErrorAction SilentlyContinue) }}\n\
         }}\n\
         [pscustomobject]@{{ role = $role; vmms = \"$svc\"; can = $can; vm = $vm; admin = $admin; switch = $switch }} | ConvertTo-Json -Compress\n",
        default_switch = SWITCH,
        name = vm_name()
    )
}

/// What the facts mean.
pub fn classify_probe(json: &str) -> Probe {
    let Some(start) = json.find('{') else {
        return Probe::Unavailable(NOT_ENABLED.into());
    };
    let f: Facts = serde_json::from_str(&json[start..]).unwrap_or_default();
    if !f.role {
        return Probe::Unavailable(NOT_ENABLED.into());
    }
    if !f.vmms.eq_ignore_ascii_case("running") {
        return Probe::Unavailable(
            "The Hyper-V Virtual Machine Management service is not running: restart this PC \
             if Hyper-V was just turned on"
                .into(),
        );
    }
    if !f.can {
        return Probe::Unavailable(NOT_PERMITTED.into());
    }
    // No Default Switch (Windows Server): the VM's private network is made once,
    // which takes an administrator; a VM that already has it needs none.
    if !f.switch && !f.admin && f.vm.is_empty() && !own_network() {
        return Probe::Unavailable(NO_SWITCH_NOT_ADMIN.into());
    }
    Probe::Available {
        vm: (!f.vm.is_empty()).then_some(f.vm),
    }
}

/// Asks Windows (read-only, no elevation).
pub async fn probe() -> Probe {
    if !cfg!(windows) {
        return Probe::Unavailable("Hyper-V runs on Windows".into());
    }
    match ps(&probe_script(), 60).await {
        Ok(out) => classify_probe(&out),
        Err(e) => Probe::Unavailable(format!("could not ask Windows about Hyper-V: {e}")),
    }
}

// ---------------------------------------------------------------------------
// PowerShell
// ---------------------------------------------------------------------------

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs `script` in Windows PowerShell and returns what it printed. A script
/// that throws (or exits non-zero) is an error carrying its message.
pub async fn ps(script: &str, secs: u64) -> Result<String> {
    use base64::Engine as _;
    let wrapped = format!(
        "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'\n\
         try {{\n{script}\n}} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}\n"
    );
    let utf16: Vec<u8> = wrapped
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    let mut c = tokio::process::Command::new("powershell.exe");
    c.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-EncodedCommand",
        &encoded,
    ])
    .stdin(std::process::Stdio::null())
    .kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    let out = tokio::time::timeout(std::time::Duration::from_secs(secs), c.output())
        .await
        .map_err(|_| VmmError::other(format!("PowerShell did not finish in {secs} s")))?
        .map_err(|e| VmmError::other(format!("starting powershell.exe: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no message");
        Err(VmmError::other(line.trim().to_string()))
    }
}

/// Single-quotes `s` for PowerShell.
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Memory for the VM in bytes and its processors.
fn vm_size() -> (u64, u32) {
    let (cpus, gib) = super::vm_size();
    (u64::from(gib) << 30, cpus)
}

/// The PowerShell that makes the VM from the fixed VHD at `fixed`: converts it
/// to the dynamic VHDX, makes a generation 2 machine on the Default Switch
/// with the seed in its DVD drive. An earlier setup's VM of this name goes first.
pub fn create_script(fixed: &std::path::Path, memory: u64, cpus: u32, own: bool) -> String {
    let name = q(&vm_name());
    let (switch, network) = if own {
        (
            OWN_SWITCH,
            format!(
                "if (-not (Get-VMSwitch -Name {sw} -ErrorAction SilentlyContinue)) {{ New-VMSwitch -SwitchName {sw} -SwitchType Internal | Out-Null }}\n\
                 $nic = 'vEthernet ({raw})'\n\
                 if (-not (Get-NetIPAddress -InterfaceAlias $nic -IPAddress {gw} -ErrorAction SilentlyContinue)) {{ New-NetIPAddress -InterfaceAlias $nic -IPAddress {gw} -PrefixLength 24 | Out-Null }}\n\
                 if (-not (Get-NetNat -Name {sw} -ErrorAction SilentlyContinue)) {{ New-NetNat -Name {sw} -InternalIPInterfaceAddressPrefix {prefix} | Out-Null }}\n",
                sw = q(OWN_SWITCH),
                raw = OWN_SWITCH,
                gw = OWN_GATEWAY,
                prefix = OWN_PREFIX
            ),
        )
    } else {
        (SWITCH, String::new())
    };
    format!(
        "$name = {name}\n\
         {network}\
         $v = Get-VM -Name $name -ErrorAction SilentlyContinue\n\
         if ($v) {{ Stop-VM -Name $name -TurnOff -Force -ErrorAction SilentlyContinue; Remove-VM -Name $name -Force }}\n\
         Remove-Item -LiteralPath {vhdx} -Force -ErrorAction SilentlyContinue\n\
         Convert-VHD -Path {fixed} -DestinationPath {vhdx} -VHDType Dynamic\n\
         Resize-VHD -Path {vhdx} -SizeBytes {disk}GB\n\
         New-VM -Name $name -Generation 2 -MemoryStartupBytes {memory} -VHDPath {vhdx} -SwitchName {switch} | Out-Null\n\
         Set-VM -Name $name -ProcessorCount {cpus} -CheckpointType Disabled -StaticMemory\n\
         Set-VMFirmware -VMName $name -EnableSecureBoot On -SecureBootTemplate MicrosoftUEFICertificateAuthority\n\
         Set-VMNetworkAdapter -VMName $name -StaticMacAddress {mac}\n\
         Add-VMDvdDrive -VMName $name -Path {seed}\n\
         $dvd = Get-VMDvdDrive -VMName $name\n\
         $disk = Get-VMHardDiskDrive -VMName $name\n\
         Set-VMFirmware -VMName $name -BootOrder $disk, $dvd\n",
        vhdx = q(&disk_path().display().to_string()),
        fixed = q(&fixed.display().to_string()),
        disk = DISK_GIB,
        switch = q(switch),
        mac = q(&mac()),
        seed = q(&seed_path().display().to_string()),
    )
}

/// The PowerShell that prints the VM's IPv4 address (nothing while it has
/// none): the integration services' report, else the neighbor table by MAC.
pub fn ip_script() -> String {
    format!(
        "$name = {name}\n\
         $ip = (Get-VMNetworkAdapter -VMName $name).IPAddresses | Where-Object {{ $_ -match '^\\d+\\.\\d+\\.\\d+\\.\\d+$' }} | Select-Object -First 1\n\
         if (-not $ip) {{\n\
           $m = '{mac}' -replace '(..)(?!$)', '$1-'\n\
           $ip = (Get-NetNeighbor -AddressFamily IPv4 -ErrorAction SilentlyContinue | Where-Object {{ $_.LinkLayerAddress -eq $m -and $_.State -ne 'Unreachable' }} | Select-Object -First 1).IPAddress\n\
         }}\n\
         if ($ip) {{ $ip }}\n",
        name = q(&vm_name()),
        mac = mac()
    )
}

// ---------------------------------------------------------------------------
// Setting up, starting, stopping
// ---------------------------------------------------------------------------

static IP: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn cached_ip() -> Option<String> {
    IP.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn remember_ip(ip: &str) {
    *IP.lock().unwrap_or_else(|e| e.into_inner()) = Some(ip.to_string());
}

/// The `ssh` program Windows ships (an optional feature on some editions).
fn ssh_bin() -> Result<PathBuf> {
    host::which("ssh").ok_or_else(|| {
        VmmError::missing(
            "OpenSSH client",
            "turn on the Windows feature OpenSSH Client (Settings, Optional features), which the \
             Hyper-V runtime uses to reach Docker in its VM",
        )
    })
}

fn ssh_command(ip: &str, remote: &str) -> Result<tokio::process::Command> {
    let mut c = tokio::process::Command::new(ssh_bin()?);
    c.args(ssh_args(ip, &key_path(), remote));
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    Ok(c)
}

/// Runs `remote` in the VM; stdout when it succeeded.
async fn ssh_run(ip: &str, remote: &str, secs: u64) -> Result<String> {
    let mut c = ssh_command(ip, remote)?;
    c.stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(std::time::Duration::from_secs(secs), c.output())
        .await
        .map_err(|_| VmmError::other(format!("`{remote}` in the VM did not finish in {secs} s")))?
        .map_err(|e| VmmError::other(format!("starting ssh: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(VmmError::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// The VM's state (`Off`, `Running`, ...); `None` when it does not exist.
pub async fn vm_state() -> Result<Option<String>> {
    let s = ps(
        &format!(
            "$v = Get-VM -Name {} -ErrorAction SilentlyContinue; if ($v) {{ $v.State.ToString() }}",
            q(&vm_name())
        ),
        60,
    )
    .await?;
    Ok((!s.is_empty()).then_some(s))
}

/// Makes this home's VM: the disk (the Mac's, downloaded once), the key, the
/// seed, then the machine. `step` reports what it is doing.
#[cfg(feature = "lume")]
pub async fn provision(step: &(dyn Fn(&str, f64) + Send + Sync)) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    crate::disk::ensure_space(
        &dir(),
        super::disk_pin().size + (14 << 30),
        "set up the built-in Linux runtime",
    )?;
    step("downloading the Linux runtime", 0.05);
    let mut dl = super::Downloads::new(super::disk_pin().size);
    let gz = super::ensure_disk(&mut dl).await?;

    step("preparing the disk", 0.45);
    let fixed = dir().join("disk-fixed.vhd");
    let (gz2, fixed2) = (gz.clone(), fixed.clone());
    tokio::task::spawn_blocking(move || gz_to_fixed_vhd(&gz2, &fixed2, &|_| {}))
        .await
        .map_err(|e| VmmError::other(format!("preparing the disk: {e}")))??;

    step("setting up the virtual machine", 0.7);
    let key = crate::cloudinit::ensure_ssh_key(&key_path()).await?;
    let public = std::fs::read_to_string(format!("{}.pub", key.display()))?;
    let own = !ps(
        &format!(
            "[bool](Get-VMSwitch -Name {} -ErrorAction SilentlyContinue)",
            q(SWITCH)
        ),
        60,
    )
    .await?
    .eq_ignore_ascii_case("true");
    let dns = dns_servers(&host_dns().await);
    let mut seed = crate::cloudinit::Seed::raw("cua-runtime", &guest_user_data(&public, &dns));
    if own {
        seed.network_config = Some(network_config(&dns));
    }
    seed.write_iso(&seed_path())?;
    std::fs::write(
        dir().join("network"),
        if own { "own\n" } else { "default\n" },
    )?;
    let (memory, cpus) = vm_size();
    let made = ps(&create_script(&fixed, memory, cpus, own), 900).await;
    let _ = std::fs::remove_file(&fixed);
    made.map_err(|e| VmmError::other(format!("making the Hyper-V virtual machine: {e}")))?;
    std::fs::write(marker(), format!("{SETUP_VERSION}\n"))?;
    Ok(())
}

/// Starts the VM when it is not running, waits until Docker answers in it and
/// serves the engine's pipe.
pub async fn start_engine(step: &(dyn Fn(&str, f64) + Send + Sync)) -> Result<()> {
    match vm_state().await?.as_deref() {
        Some("Running") => {}
        Some(_) => {
            step("starting the virtual machine", 0.75);
            ps(&format!("Start-VM -Name {}", q(&vm_name())), 120)
                .await
                .map_err(|e| VmmError::other(format!("starting the Hyper-V VM: {e}")))?;
        }
        None => {
            return Err(VmmError::other(
                "the built-in Linux runtime's Hyper-V virtual machine is gone; \
                 remove the runtime's setup and create the Space again",
            ));
        }
    }
    step("waiting for its address", 0.8);
    let ip = wait_ip(180).await?;
    remember_ip(&ip);
    step("starting Docker", 0.9);
    wait_docker(&ip, 240).await?;
    serve_pipe()
}

async fn wait_ip(secs: u64) -> Result<String> {
    if own_network() {
        return Ok(OWN_GUEST.to_string());
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if let Ok(ip) = ps(&ip_script(), 60).await
            && !ip.is_empty()
        {
            return Ok(ip.lines().next().unwrap_or_default().trim().to_string());
        }
        if tokio::time::Instant::now() > deadline {
            return Err(VmmError::other(format!(
                "the Hyper-V virtual machine has no address after {secs} s: its network (the \
                 Default Switch) may be down"
            )));
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

async fn wait_docker(ip: &str, secs: u64) -> Result<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut last = String::new();
    loop {
        match ssh_run(ip, "docker info --format '{{.ServerVersion}}'", 30).await {
            Ok(v) if !v.is_empty() => return Ok(()),
            Ok(_) => {}
            Err(e) => last = e.to_string(),
        }
        if tokio::time::Instant::now() > deadline {
            return Err(VmmError::other(format!(
                "Docker did not answer in the Hyper-V virtual machine after {secs} s{}",
                if last.is_empty() {
                    String::new()
                } else {
                    format!(" ({last})")
                }
            )));
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// The command each pipe client runs: `ssh` into the VM at its current address.
fn dial() -> wsl::Dial {
    Arc::new(|| {
        let ip = cached_ip().unwrap_or_default();
        ssh_command(&ip, DIAL_COMMAND).unwrap_or_else(|_| tokio::process::Command::new("ssh"))
    })
}

/// Serves the engine's pipe from this process: every client is an `ssh` that
/// runs `docker system dial-stdio` in the VM. Also starts the port forwarder.
pub fn serve_pipe() -> Result<()> {
    start_forwarder();
    wsl::serve_pipe_with(dial())
}

// ---------------------------------------------------------------------------
// Published ports
// ---------------------------------------------------------------------------

/// A container's published port is bound on the VM's loopback. The VM has an
/// address of its own (WSL's localhost forwarding and the Mac's Lima forwarder do
/// not exist here), so cua opens each of those ports on this PC's loopback under the
/// same number and carries every connection into the VM over `ssh -W`. The container
/// layer picks host port numbers this PC has free for this engine
/// (`forwards_ports_by_number`).
static FORWARDER: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>> =
    std::sync::Mutex::new(None);

/// Starts the forwarder in this process when it is not running (a no-op without a
/// tokio runtime).
pub fn start_forwarder() {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let mut slot = FORWARDER.lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_ref().is_some_and(|h| !h.is_finished()) {
        return;
    }
    *slot = Some(rt.spawn(forward_loop()));
}

/// Stops forwarding (the VM is stopping).
pub fn stop_forwarder() {
    if let Some(h) = FORWARDER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        h.abort();
    }
}

/// Watches the VM's published ports and keeps a listener on this PC's loopback for
/// each; ends when aborted. Reconnects when the watching `ssh` ends.
async fn forward_loop() {
    use tokio::io::{AsyncBufReadExt as _, BufReader};
    let mut listeners: std::collections::HashMap<u16, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();
    loop {
        if let Some(ip) = cached_ip()
            && let Ok(mut c) = ssh_command(&ip, PORTS_SCRIPT)
        {
            c.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            if let Ok(mut child) = c.spawn()
                && let Some(out) = child.stdout.take()
            {
                let mut lines = BufReader::new(out).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let want = parse_ports(&line);
                    listeners.retain(|p, h| {
                        let keep = want.contains(p) && !h.is_finished();
                        if !keep {
                            h.abort();
                        }
                        keep
                    });
                    for p in want {
                        if let std::collections::hash_map::Entry::Vacant(e) = listeners.entry(p)
                            && let Some(ip) = cached_ip()
                        {
                            e.insert(tokio::spawn(listen(p, ip)));
                        }
                    }
                }
            }
        }
        for (_, h) in listeners.drain() {
            h.abort();
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// This PC's `127.0.0.1:port`, every connection carried to the same port in the VM.
async fn listen(port: u16, ip: String) {
    let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await else {
        // Something else here holds the number: its owner is who the client reaches.
        return;
    };
    loop {
        let Ok((conn, _)) = l.accept().await else {
            return;
        };
        let ip = ip.clone();
        tokio::spawn(async move {
            let mut c = tokio::process::Command::new(ssh_bin().unwrap_or_else(|_| "ssh".into()));
            c.args(ssh_forward_args(&ip, &key_path(), port));
            #[cfg(windows)]
            c.creation_flags(CREATE_NO_WINDOW);
            wsl::bridge_with(conn, c).await;
        });
    }
}

/// A daemon that restarted finds the VM still running and serves the pipe
/// again; never blocks the caller.
pub fn reattach() {
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        static BUSY: AtomicBool = AtomicBool::new(false);
        static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
        if !provisioned() {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        {
            let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(15)) {
                return;
            }
            if BUSY.swap(true, SeqCst) {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        rt.spawn(async {
            let look = tokio::time::timeout(std::time::Duration::from_secs(20), async {
                if vm_state().await.ok().flatten().as_deref() == Some("Running")
                    && let Ok(ip) = wait_ip(10).await
                {
                    remember_ip(&ip);
                    let _ = serve_pipe();
                }
            })
            .await;
            let _ = look;
            BUSY.store(false, SeqCst);
        });
    }
}

/// Powers the VM off (its Spaces stop with it): the guest shuts down
/// cleanly first, then Hyper-V turns it off if it has not in a minute.
pub async fn stop() -> Result<()> {
    wsl::stop_serving();
    stop_forwarder();
    if vm_state().await?.as_deref() != Some("Running") {
        return Ok(());
    }
    if let Some(ip) = cached_ip() {
        let _ = ssh_run(&ip, "sudo systemctl poweroff", 20).await;
    }
    let name = q(&vm_name());
    ps(
        &format!(
            "$name = {name}\n\
             for ($i = 0; $i -lt 30; $i++) {{ if ((Get-VM -Name $name).State -eq 'Off') {{ break }}; Start-Sleep -Seconds 2 }}\n\
             if ((Get-VM -Name $name).State -ne 'Off') {{ Stop-VM -Name $name -TurnOff -Force }}"
        ),
        120,
    )
    .await
    .map(drop)
}

/// Deletes the VM and its disk, seed and key. Nothing else is touched.
pub async fn remove() -> Result<()> {
    wsl::stop_serving();
    stop_forwarder();
    if provisioned() || dir().exists() {
        let name = q(&vm_name());
        let _ = ps(
            &format!(
                "$name = {name}\n\
                 if (Get-VM -Name $name -ErrorAction SilentlyContinue) {{ Stop-VM -Name $name -TurnOff -Force -ErrorAction SilentlyContinue; Remove-VM -Name $name -Force }}"
            ),
            120,
        )
        .await;
    }
    if own_network() {
        let _ = ps(
            &format!(
                "Remove-NetNat -Name {sw} -Confirm:$false -ErrorAction SilentlyContinue\n\
                 Remove-VMSwitch -Name {sw} -Force -ErrorAction SilentlyContinue",
                sw = q(OWN_SWITCH)
            ),
            120,
        )
        .await;
    }
    match std::fs::remove_dir_all(dir()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_reads_its_words() {
        assert_eq!(Backend::parse("hyperv"), Some(Backend::Hyperv));
        assert_eq!(Backend::parse(" Hyper-V "), Some(Backend::Hyperv));
        assert_eq!(Backend::parse("WSL2"), Some(Backend::Wsl));
        assert_eq!(Backend::parse(""), Some(Backend::Auto));
        assert_eq!(Backend::parse("vmware"), None);
        assert_eq!(Backend::Hyperv.as_str(), "hyperv");
    }

    #[test]
    fn a_fixed_vhd_footer_is_what_hyper_v_reads() {
        let size = 3 * 1024 * 1024 * 1024u64 + 512 * 7;
        let f = vhd_fixed_footer(size, [7; 16], 123_456);
        assert_eq!(&f[0..8], b"conectix");
        assert_eq!(u64::from_be_bytes(f[16..24].try_into().unwrap()), u64::MAX);
        assert_eq!(u64::from_be_bytes(f[40..48].try_into().unwrap()), size);
        assert_eq!(u64::from_be_bytes(f[48..56].try_into().unwrap()), size);
        assert_eq!(u32::from_be_bytes(f[60..64].try_into().unwrap()), 2);
        assert_eq!(&f[68..84], &[7u8; 16]);
        // The checksum is the one's complement of the sum of every other byte.
        let mut g = f;
        g[64..68].fill(0);
        let sum: u32 = g.iter().map(|b| *b as u32).fold(0u32, u32::wrapping_add);
        assert_eq!(u32::from_be_bytes(f[64..68].try_into().unwrap()), !sum);
    }

    #[test]
    fn the_geometry_follows_the_vhd_specification() {
        // The specification's own largest disk.
        assert_eq!(vhd_geometry(65535 * 16 * 255 * 512), (65535, 16, 255));
        // A 3.5 GiB disk: 17 sectors a track is too few cylinders, so 63.
        let (c, h, s) = vhd_geometry(3_758_096_384);
        assert_eq!((h, s), (16, 63));
        assert_eq!(c as u64, 3_758_096_384 / 512 / 63 / 16);
    }

    #[test]
    fn the_vm_has_a_fixed_hyper_v_mac() {
        let m = mac();
        assert_eq!(m.len(), 12);
        assert!(m.starts_with("00155D"));
        assert!(m.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(vm_name().starts_with(VM_PREFIX));
    }

    #[test]
    fn docker_in_the_vm_runs_gvisor_away_from_the_switch_subnets() {
        let v: serde_json::Value = serde_json::from_str(&daemon_json()).unwrap();
        assert_eq!(v["default-runtime"], "runsc");
        assert_eq!(v["runtimes"]["runsc"]["path"], "/usr/local/bin/runsc");
        assert_eq!(v["bip"], wsl::BRIDGE_CIDR);
        assert_eq!(v["default-address-pools"][0]["base"], wsl::ADDRESS_POOL);
    }

    #[test]
    fn the_cloud_config_logs_cua_in_with_the_homes_key() {
        let c = guest_user_data(
            "ssh-ed25519 AAAAC3Nza cua-vmm\n",
            &["172.31.0.2".to_string(), "1.1.1.1".to_string()],
        );
        assert!(c.starts_with("#cloud-config\n"));
        assert!(c.contains("- ssh-ed25519 AAAAC3Nza cua-vmm\n"));
        assert!(c.contains("name: cua\n"));
        assert!(c.contains("ssh_pwauth: false"));
        assert!(c.contains("/etc/docker/daemon.json"));
        assert!(c.contains("cua-announce.service"));
        // The image has no systemd-resolved: resolv.conf is written at every boot.
        assert!(c.contains("manage_etc_hosts: localhost"));
        assert!(
            c.contains("printf 'nameserver 172.31.0.2\\nnameserver 1.1.1.1\\n' > /etc/resolv.conf")
        );
        // The files' bodies are indented under their keys: the document parses.
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&c).expect("valid YAML");
        let files = doc["write_files"].as_sequence().unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0]["content"].as_str().unwrap().trim(), daemon_json());
        assert!(
            files[1]["content"]
                .as_str()
                .unwrap()
                .starts_with("#!/bin/sh")
        );
    }

    #[test]
    fn the_pipes_clients_ssh_in_with_the_key_and_no_tty() {
        let a = ssh_args(
            "172.19.167.122",
            std::path::Path::new("C:\\k\\id"),
            DIAL_COMMAND,
        );
        assert_eq!(a[0], "-i");
        assert_eq!(a[1], "C:\\k\\id");
        assert!(a.contains(&"BatchMode=yes".to_string()));
        assert!(a.contains(&"-T".to_string()));
        assert_eq!(a[a.len() - 2], "cua@172.19.167.122");
        assert_eq!(a[a.len() - 1], "docker system dial-stdio");
    }

    #[test]
    fn what_hyper_v_says_becomes_what_to_do() {
        let on = r#"{"role":true,"vmms":"Running","can":true,"vm":"Off"}"#;
        assert_eq!(
            classify_probe(on),
            Probe::Available {
                vm: Some("Off".into())
            }
        );
        assert_eq!(
            classify_probe(r#"{"role":true,"vmms":"Running","can":true,"vm":"","switch":true}"#),
            Probe::Available { vm: None }
        );
        // Windows Server (no Default Switch): an administrator sets the private network up.
        assert_eq!(
            classify_probe(
                r#"{"role":true,"vmms":"Running","can":true,"vm":"","switch":false,"admin":false}"#
            ),
            Probe::Unavailable(NO_SWITCH_NOT_ADMIN.into())
        );
        assert_eq!(
            classify_probe(
                r#"{"role":true,"vmms":"Running","can":true,"vm":"","switch":false,"admin":true}"#
            ),
            Probe::Available { vm: None }
        );
        assert_eq!(
            classify_probe(r#"{"role":false,"vmms":"","can":false,"vm":""}"#),
            Probe::Unavailable(NOT_ENABLED.into())
        );
        assert_eq!(
            classify_probe(r#"{"role":true,"vmms":"Running","can":false,"vm":""}"#),
            Probe::Unavailable(NOT_PERMITTED.into())
        );
        assert!(matches!(
            classify_probe(r#"{"role":true,"vmms":"Stopped","can":false,"vm":""}"#),
            Probe::Unavailable(m) if m.contains("restart this PC")
        ));
        // Noise before the JSON (a banner) and garbage are both fine.
        assert!(matches!(
            classify_probe(&format!("banner\n{on}")),
            Probe::Available { .. }
        ));
        assert_eq!(
            classify_probe("nothing"),
            Probe::Unavailable(NOT_ENABLED.into())
        );
    }

    #[test]
    fn the_forwarder_carries_published_ports_through_ssh() {
        assert_eq!(
            parse_ports("32768 8080 80 32768 notaport\n"),
            vec![8080, 32768]
        );
        assert_eq!(parse_ports(""), Vec::<u16>::new());
        let a = ssh_forward_args("10.232.0.2", std::path::Path::new("C:\\k\\id"), 40123);
        assert_eq!(a[a.len() - 3], "-W");
        assert_eq!(a[a.len() - 2], "127.0.0.1:40123");
        assert_eq!(a[a.len() - 1], "cua@10.232.0.2");
        assert!(a.contains(&"-T".to_string()));
        // The script prints one line of numbers a second, from docker-proxy only.
        assert!(PORTS_SCRIPT.contains("docker-proxy"));
        assert!(PORTS_SCRIPT.contains("sleep 1"));
    }

    #[test]
    fn the_vm_resolves_names_through_the_hosts_servers_then_public_ones() {
        let host = [
            "172.31.0.2",
            "127.0.0.1",
            "169.254.1.1",
            "10.232.0.1",
            "172.31.0.2",
            "bogus",
        ]
        .map(String::from);
        assert_eq!(
            dns_servers(&host),
            vec!["172.31.0.2".to_string(), "1.1.1.1".into(), "8.8.8.8".into()]
        );
        assert_eq!(
            dns_servers(&[]),
            vec!["1.1.1.1".to_string(), "8.8.8.8".into()]
        );
    }

    #[test]
    fn a_vm_with_its_own_network_gets_a_static_address_on_its_mac() {
        let c = network_config(&["172.31.0.2".to_string(), "1.1.1.1".to_string()]);
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&c).expect("valid YAML");
        let nic = &doc["ethernets"]["cua0"];
        assert_eq!(nic["addresses"][0].as_str(), Some("10.232.0.2/24"));
        assert_eq!(
            nic["nameservers"]["addresses"][0].as_str(),
            Some("172.31.0.2")
        );
        assert_eq!(nic["routes"][0]["via"].as_str(), Some("10.232.0.1"));
        let m = nic["match"]["macaddress"].as_str().unwrap();
        assert_eq!(m.len(), 17);
        assert!(m.starts_with("00:15:5d:"));
        assert_eq!(m.replace(':', "").to_ascii_uppercase(), mac());
    }

    #[test]
    fn scripts_quote_their_paths_and_name_the_machine() {
        assert_eq!(q("it's"), "'it''s'");
        let s = create_script(std::path::Path::new("C:\\a b\\d.vhd"), 4 << 30, 4, false);
        assert!(s.contains("Convert-VHD -Path 'C:\\a b\\d.vhd'"));
        assert!(s.contains("-Generation 2"));
        assert!(s.contains("MicrosoftUEFICertificateAuthority"));
        assert!(s.contains(&format!("-StaticMacAddress '{}'", mac())));
        assert!(s.contains(&vm_name()));
        assert!(!s.contains("New-NetNat"));
        let own = create_script(std::path::Path::new("C:\\d.vhd"), 4 << 30, 4, true);
        assert!(own.contains("New-VMSwitch -SwitchName 'cua-runtime' -SwitchType Internal"));
        assert!(own.contains(
            "New-NetNat -Name 'cua-runtime' -InternalIPInterfaceAddressPrefix 10.232.0.0/24"
        ));
        assert!(own.contains("-SwitchName 'cua-runtime'"));
        assert!(ip_script().contains("Get-NetNeighbor"));
        assert!(probe_script().contains(&vm_name()));
    }
}
