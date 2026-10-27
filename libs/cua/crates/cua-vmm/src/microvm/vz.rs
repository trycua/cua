//! The macOS half of the microVM backend: Apple's Virtualization.framework
//! through `cua-vz` (libs/cua/vz), cua's own monitor, one process per VM
//! the way `firecracker` is on Linux.
//!
//! Everything above the monitor is the Linux backend's: the same guest
//! kernel and init, the image as a read-only squashfs (written in process,
//! [`super::rootfs::build_squashfs_native`]) under an overlay whose upper
//! layer is the VM's own ext4 scratch disk, identity over vsock after a
//! restore, warm starts, warm pools, checkpoints and forks.
//!
//! What differs:
//! * **Snapshots** are the framework's saved machine state (macOS 14+,
//!   `saveMachineStateTo`), one file holding the guest's memory and devices
//!   (`vzstate`), written while the VM is paused. A restore must use the
//!   saved VM's machine identifier (kept next to it, `machine-id`) and MAC
//!   address, so every fork of one snapshot has both; each has a network
//!   of its own, so they never meet. A fork's disk is an APFS clone.
//! * **Memory is not shared between forks.** A restore reads the state into
//!   the new VM's own memory (only the pages the guest had touched are in
//!   the file; the rest stay untouched), so each fork costs what its guest
//!   uses, and restoring takes time in proportion to it.
//! * **Network**: a user-mode stack per VM, the Cua network gateway
//!   ([`crate::netgw`], shipped next to `cua-vz`), on a file-handle network
//!   device, with the same guest address in every VM (as each Firecracker
//!   VM has its own namespace). The gateway checks every connection the
//!   guest opens against the Space's egress rules, never dials the host's
//!   loopback (its host address opens only for a rule naming it) and drops
//!   DNS answers the rules refuse. A Space without network gets an
//!   unconnected device.
//! * **Published ports** are host loopback ports `cua-vz` carries over vsock
//!   to the guest init ([`cua_microvm_init::Request::Forward`]), so they work
//!   with or without a network.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::{Result, VmmError};
use crate::host;

/// The oldest macOS that saves and restores VMs.
pub const MIN_MACOS: u32 = 14;

/// The guest's network: the Linux backend's addresses (in a /29, so the
/// gateway has a host address, [`HOST_IP`]).
const SUBNET: &str = "172.16.0.0/29";
/// The gateway's address for the host: its loopback, but only on the ports
/// a rule naming this address opens (none unless the policy says so).
pub const HOST_IP: &str = "172.16.0.6";
/// The gateway's MAC address.
const GATEWAY_MAC: &str = "5a:94:ef:e4:0c:dd";

/// Why this host cannot run macOS microVMs, if it cannot.
pub fn unsupported_reason() -> Option<String> {
    if host::HostOs::current() != host::HostOs::Macos
        || crate::types::Arch::host() != crate::types::Arch::Aarch64
    {
        return Some("macOS microVMs need an Apple silicon Mac".into());
    }
    match host::macos_major() {
        Some(m) if m >= MIN_MACOS => None,
        _ => Some(format!(
            "macOS microVMs need macOS {MIN_MACOS} or later (this Mac runs {})",
            host::macos_version().unwrap_or_else(|| "an unknown version".into())
        )),
    }
}

/// `cua-vz`: `$CUA_VZ`, next to the running executable, `~/.cua/bin`, then
/// `PATH`.
pub fn vz_bin() -> Option<PathBuf> {
    const NAME: &str = "cua-vz";
    if let Some(p) = std::env::var_os("CUA_VZ").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        && dir.join(NAME).is_file()
    {
        return Some(dir.join(NAME));
    }
    let own = host::cua_home().join("bin").join(NAME);
    if own.is_file() {
        return Some(own);
    }
    host::which(NAME)
}

/// The kernel command line: the init's network fixed by `ip=` (the same
/// address in every VM), the console on the framework's virtio console,
/// and reboot = power off (the monitor ends when the guest does).
pub fn boot_args() -> String {
    use cua_microvm_init::{GATEWAY_IP, GUEST_IP};
    format!(
        "console=hvc0 reboot=k panic=1 quiet loglevel=3 random.trust_cpu=on \
         ip={GUEST_IP}::{GATEWAY_IP}:255.255.255.252::eth0:off"
    )
}

/// The gateway's configuration for one VM ([`crate::netgw::CONFIG_FILE`]):
/// the guest's subnet, DNS on the gateway, the host address, and the
/// file-handle device's socket (`gv.sock` in `sockets`). Published ports
/// go over vsock, not through the gateway.
pub fn gateway_config(sockets: &Path) -> Value {
    use cua_microvm_init::GATEWAY_IP;
    crate::netgw::config(&crate::netgw::Network {
        subnet: SUBNET,
        gateway_ip: GATEWAY_IP,
        gateway_mac: GATEWAY_MAC,
        host_ip: Some(HOST_IP),
        dhcp: false,
        static_leases: Default::default(),
        vfkit: Some(format!("unixgram://{}", sockets.join("gv.sock").display())),
        qemu: None,
    })
}

/// What [`vz_config`] needs.
pub struct VzLaunch<'a> {
    pub kernel: &'a Path,
    pub initrd: &'a Path,
    pub cpus: u32,
    pub memory_mb: u64,
    /// The image (`base.sqfs`) and the scratch disk, in the VM's directory.
    pub base: &'a str,
    pub sockets: &'a Path,
    /// The network gateway (`None`: no network).
    pub gateway: Option<&'a Path>,
    pub forwards: &'a [(u16, u16)],
    pub restore: Option<&'a Path>,
}

/// `vz.json` for `cua-vz` (libs/cua/vz/Sources/CuaVZCore/Config.swift).
pub fn vz_config(l: &VzLaunch<'_>) -> Value {
    let mut v = json!({
        "kernel": l.kernel,
        "initrd": l.initrd,
        "cmdline": boot_args(),
        "cpus": l.cpus.max(1),
        "memory_mb": l.memory_mb.max(256),
        "disks": [
            {"path": l.base, "read_only": true},
            {"path": "scratch.ext4", "read_only": false},
        ],
        "mac": cua_microvm_init::GUEST_MAC,
        "machine_id": "machine-id",
        "sockets": l.sockets,
        "console": "console.log",
        "forwards": l.forwards.iter().map(|(h, g)| json!({"host": h, "guest": g})).collect::<Vec<_>>(),
    });
    if let Some(gw) = l.gateway {
        v["network"] = json!({"gateway": gw, "config": crate::netgw::CONFIG_FILE});
    }
    if let Some(r) = l.restore {
        v["restore"] = json!(r);
    }
    v
}

/// `cua-vz`'s control socket (`api.sock`): one JSON line each way per
/// connection.
#[derive(Clone, Debug)]
pub struct VzApi {
    sock: PathBuf,
}

impl VzApi {
    pub fn new(sock: impl Into<PathBuf>) -> Self {
        Self { sock: sock.into() }
    }

    pub fn path(&self) -> &Path {
        &self.sock
    }

    /// Sends `{"op": op, ...}`; the reply on success, its error otherwise.
    pub async fn call(&self, op: &str, path: Option<&Path>, timeout: Duration) -> Result<Value> {
        let fut = async {
            let mut s = super::connect_unix(&self.sock)
                .await
                .map_err(|e| VmmError::other(format!("cua-vz {}: {e}", self.sock.display())))?;
            let mut req = json!({"op": op});
            if let Some(p) = path {
                req["path"] = json!(p);
            }
            let mut line = serde_json::to_vec(&req)?;
            line.push(b'\n');
            s.write_all(&line).await?;
            let mut r = BufReader::new(s);
            let mut reply = String::new();
            if r.read_line(&mut reply).await? == 0 {
                return Err(VmmError::other(format!("cua-vz: no answer to {op}")));
            }
            let v: Value = serde_json::from_str(reply.trim())?;
            if v["ok"].as_bool() != Some(true) {
                return Err(VmmError::other(format!(
                    "cua-vz {op}: {}",
                    v["error"].as_str().unwrap_or("failed")
                )));
            }
            Ok(v)
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| VmmError::other(format!("cua-vz: no answer to {op} within {timeout:?}")))?
    }

    pub async fn state(&self) -> Result<String> {
        let v = self.call("state", None, Duration::from_secs(5)).await?;
        Ok(v["state"].as_str().unwrap_or_default().to_string())
    }

    pub async fn pause(&self) -> Result<()> {
        self.call("pause", None, Duration::from_secs(30))
            .await
            .map(drop)
    }

    pub async fn resume(&self) -> Result<()> {
        self.call("resume", None, Duration::from_secs(30))
            .await
            .map(drop)
    }

    /// Saves the paused VM's memory and devices to `file` (which must not
    /// exist).
    pub async fn save(&self, file: &Path) -> Result<()> {
        self.call("save", Some(file), Duration::from_secs(600))
            .await
            .map(drop)
    }

    /// Stops the VM at once (no guest shutdown); the monitor exits.
    pub async fn stop(&self) -> Result<()> {
        self.call("stop", None, Duration::from_secs(30))
            .await
            .map(drop)
    }
}

/// The working directory of process `pid` (`None` when it cannot be read).
pub fn pid_cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a zeroed out-struct of the size the call is told.
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        let n = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                (&mut info as *mut libc::proc_vnodepathinfo).cast(),
                size,
            )
        };
        if n != size {
            return None;
        }
        let raw = &info.pvi_cdir.vip_path;
        let bytes: Vec<u8> = raw
            .iter()
            .flatten()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
}

/// The executable of process `pid` (`None` when it cannot be read).
pub fn pid_path(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; 4 * 1024];
        // SAFETY: the buffer is as large as the call is told.
        let n = unsafe {
            libc::proc_pidpath(
                pid as libc::c_int,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
            )
        };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guest_network_matches_the_linux_backend_and_hides_the_host() {
        let a = boot_args();
        assert!(
            a.contains("ip=172.16.0.2::172.16.0.1:255.255.255.252::eth0:off"),
            "{a}"
        );
        assert!(a.contains("console=hvc0"));
        assert!(
            !a.contains("pci=off"),
            "Virtualization.framework devices are PCI"
        );
        let c = gateway_config(Path::new("/r/run/ab12"));
        assert_eq!(c["subnet"], "172.16.0.0/29");
        assert_eq!(c["gateway_ip"], "172.16.0.1");
        // The host address stands for the host's loopback, which only a
        // rule naming it opens (the gateway's own guarantee).
        assert_eq!(c["host_ip"], HOST_IP);
        assert!(
            c.get("dhcp").is_none(),
            "the kernel command line sets the address"
        );
        assert_eq!(
            c["listen"],
            json!({"vfkit": "unixgram:///r/run/ab12/gv.sock"})
        );
        assert_eq!(c["rules"], crate::netgw::RULES_FILE);
    }

    #[test]
    fn vz_json_is_what_cua_vz_reads() {
        let v = vz_config(&VzLaunch {
            kernel: Path::new("/k/vmlinux"),
            initrd: Path::new("/k/i.cpio"),
            cpus: 0,
            memory_mb: 64,
            base: "base.sqfs",
            sockets: Path::new("/r/run/ab12"),
            gateway: Some(Path::new("/b/cua-netgw")),
            forwards: &[(50001, 3211)],
            restore: Some(Path::new("/c/snap/vzstate")),
        });
        assert_eq!(v["cpus"], 1, "at least one");
        assert_eq!(v["memory_mb"], 256, "at least 256 MiB");
        assert_eq!(
            v["disks"][0],
            json!({"path": "base.sqfs", "read_only": true})
        );
        assert_eq!(
            v["disks"][1],
            json!({"path": "scratch.ext4", "read_only": false})
        );
        assert_eq!(v["mac"], cua_microvm_init::GUEST_MAC);
        assert_eq!(v["machine_id"], "machine-id");
        assert_eq!(
            v["network"],
            json!({"gateway": "/b/cua-netgw", "config": "netgw.json"})
        );
        assert_eq!(v["forwards"], json!([{"host": 50001, "guest": 3211}]));
        assert_eq!(v["restore"], "/c/snap/vzstate");
        let none = vz_config(&VzLaunch {
            gateway: None,
            restore: None,
            ..VzLaunch {
                kernel: Path::new("/k"),
                initrd: Path::new("/i"),
                cpus: 2,
                memory_mb: 1024,
                base: "base.sqfs",
                sockets: Path::new("/s"),
                gateway: None,
                forwards: &[],
                restore: None,
            }
        });
        assert!(
            none.get("network").is_none(),
            "no network: an unconnected device"
        );
        assert!(none.get("restore").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn this_process_reports_its_own_working_directory_and_path() {
        let me = std::process::id();
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            pid_cwd(me).map(|p| std::fs::canonicalize(p).unwrap()),
            Some(std::fs::canonicalize(cwd).unwrap())
        );
        assert!(pid_path(me).is_some());
        assert_eq!(pid_cwd(u32::MAX - 7), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_control_client_reads_one_json_line() {
        let d = tempfile::tempdir().unwrap();
        let sock = d.path().join("api.sock");
        let l = tokio::net::UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            for answer in [
                r#"{"ok":true,"state":"paused"}"#,
                r#"{"ok":false,"error":"save needs a paused VM"}"#,
            ] {
                let (s, _) = l.accept().await.unwrap();
                let mut r = BufReader::new(s);
                let mut line = String::new();
                r.read_line(&mut line).await.unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                assert!(req["op"].is_string());
                r.get_mut()
                    .write_all(format!("{answer}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let api = VzApi::new(&sock);
        assert_eq!(api.state().await.unwrap(), "paused");
        let e = api.save(Path::new("/x")).await.unwrap_err().to_string();
        assert!(e.contains("save needs a paused VM"), "{e}");
    }
}
