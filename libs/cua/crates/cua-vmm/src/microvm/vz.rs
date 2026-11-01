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
//! * **Network**: a user-mode stack per VM (gvproxy, Apache-2.0, pinned
//!   and downloaded on first use) on a file-handle network device, with the
//!   same guest address in every VM (as each Firecracker VM has its own
//!   namespace); its host address leads nowhere, so a guest cannot reach the
//!   host's loopback services. A Space without network gets an unconnected
//!   device.
//! * **Published ports** are host loopback ports `cua-vz` carries over vsock
//!   to the guest init ([`cua_microvm_init::Request::Forward`]), so they work
//!   with or without a network.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::{Result, VmmError};
use crate::host;

/// The pinned gvproxy release (containers/gvisor-tap-vsock).
pub const GVPROXY_VERSION: &str = "v0.9.0";

/// SHA-256 of `gvproxy-darwin` (a universal binary) of [`GVPROXY_VERSION`].
pub const GVPROXY_SHA256: &str = "e2a6825ba789fe588828b418c09aaf16525dec6d8cb2d16c94e72535dd4606be";

fn gvproxy_url() -> String {
    format!(
        "https://github.com/containers/gvisor-tap-vsock/releases/download/{GVPROXY_VERSION}/gvproxy-darwin"
    )
}

/// The oldest macOS that saves and restores VMs.
pub const MIN_MACOS: u32 = 14;

/// The guest's network: the Linux backend's addresses (in a /29, so the
/// stack has a host address, [`HOST_IP`], that leads nowhere).
const SUBNET: &str = "172.16.0.0/29";
/// gvproxy's host address, mapped to [`BLACKHOLE`] instead of the host's
/// loopback (gvproxy's default).
const HOST_IP: &str = "172.16.0.6";
/// TEST-NET-1 (RFC 5737): never routed.
const BLACKHOLE: &str = "192.0.2.1";

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

/// gvproxy: `$CUA_GVPROXY`, then cua's pinned copy in `~/.cua/bin`.
pub fn gvproxy_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUA_GVPROXY").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let own = gvproxy_path();
    own.is_file().then_some(own)
}

fn gvproxy_path() -> PathBuf {
    host::cua_home()
        .join("bin")
        .join(format!("gvproxy-{GVPROXY_VERSION}"))
}

/// gvproxy, downloaded (pinned by SHA-256) into `~/.cua/bin` the first time.
pub async fn ensure_gvproxy() -> Result<PathBuf> {
    if let Some(p) = gvproxy_bin() {
        return Ok(p);
    }
    let dest = gvproxy_path();
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let url = gvproxy_url();
    tracing::info!(url, dest = %dest.display(), "downloading gvproxy");
    let bytes = reqwest::get(&url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| VmmError::other(format!("download {url}: {e}")))?
        .bytes()
        .await
        .map_err(|e| VmmError::other(format!("download {url}: {e}")))?;
    let got = format!("{:x}", Sha256::digest(&bytes));
    if got != GVPROXY_SHA256 {
        return Err(VmmError::other(format!(
            "download {url}: SHA-256 {got} does not match the pinned {GVPROXY_SHA256}"
        )));
    }
    let partial = dest.with_extension("partial");
    tokio::fs::write(&partial, &bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    tokio::fs::rename(&partial, &dest).await?;
    Ok(dest)
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

/// gvproxy's configuration for one VM: the guest's subnet, DNS on the
/// gateway, no forwards (published ports go over vsock), and the stack's
/// host address mapped nowhere.
pub fn gvproxy_config(sockets: &Path, pid_file: &Path) -> String {
    use cua_microvm_init::{GATEWAY_IP, GUEST_IP};
    format!(
        "log-level: warning\n\
         pid-file: {pid}\n\
         stack:\n  mtu: 1500\n  subnet: {SUBNET}\n  gatewayIP: {GATEWAY_IP}\n  deviceIP: {GUEST_IP}\n  \
         hostIP: {HOST_IP}\n  gatewayMacAddress: 5a:94:ef:e4:0c:dd\n  nat:\n    {HOST_IP}: {BLACKHOLE}\n  \
         gatewayVirtualIPs:\n    - {HOST_IP}\n\
         interfaces:\n  vfkit: unixgram://{gv}\n",
        pid = pid_file.display(),
        gv = sockets.join("gv.sock").display(),
    )
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
    pub gvproxy: Option<&'a Path>,
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
    if let Some(gv) = l.gvproxy {
        v["network"] = json!({"gvproxy": gv, "config": "gvproxy.yaml"});
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
        let c = gvproxy_config(Path::new("/r/run/ab12"), Path::new("/v/gvproxy.pid"));
        assert!(c.contains("subnet: 172.16.0.0/29"));
        assert!(c.contains("gatewayIP: 172.16.0.1"));
        assert!(c.contains("deviceIP: 172.16.0.2"));
        // gvproxy maps its host address to the host's loopback by default:
        // here it leads to an unrouted address instead.
        assert!(c.contains("nat:\n    172.16.0.6: 192.0.2.1"), "{c}");
        assert!(!c.contains("127.0.0.1"));
        assert!(!c.contains("forwards"), "ports go over vsock");
        assert!(c.contains("vfkit: unixgram:///r/run/ab12/gv.sock"));
        assert!(c.contains("pid-file: /v/gvproxy.pid"));
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
            gvproxy: Some(Path::new("/b/gvproxy")),
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
            json!({"gvproxy": "/b/gvproxy", "config": "gvproxy.yaml"})
        );
        assert_eq!(v["forwards"], json!([{"host": 50001, "guest": 3211}]));
        assert_eq!(v["restore"], "/c/snap/vzstate");
        let none = vz_config(&VzLaunch {
            gvproxy: None,
            restore: None,
            ..VzLaunch {
                kernel: Path::new("/k"),
                initrd: Path::new("/i"),
                cpus: 2,
                memory_mb: 1024,
                base: "base.sqfs",
                sockets: Path::new("/s"),
                gvproxy: None,
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
