//! Where local sandboxes keep their disks, and how much room is left there:
//! the volume under Lume's VM storage (macOS VMs), under the cua home
//! (QEMU instances and the containerDisk cache), and the container engine's
//! data disk (Colima's VM disk, Docker Desktop's `Docker.raw`, OrbStack's
//! data directory, or `/var/lib/docker`).
//!
//! Read-only: nothing is started or installed. Free space goes through
//! [`crate::disk::space`], so `CUA_DISK_FAKE_AVAILABLE` and
//! [`crate::disk::set_probe`] apply here too.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::container::engine::{self, EngineKind};
use crate::host;

/// Space on the volume that holds something.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    /// Bytes available to this user.
    pub available: u64,
    /// Volume size in bytes.
    pub total: u64,
    /// What a person calls it: `Macintosh HD`, `Colima`, `Docker Desktop`.
    pub name: String,
}

/// The volume holding `path` (which may not exist yet), named the way the
/// Finder names it.
pub fn host_volume(path: &Path) -> Option<Volume> {
    let s = crate::disk::space(path).ok()?;
    Some(Volume {
        available: s.available,
        total: s.total,
        name: volume_name(path),
    })
}

/// Lume's default VM storage directory (`~/.config/lume/config.yaml`,
/// else `~/.lume`): where a macOS Space's clone is written.
pub fn lume_dir() -> PathBuf {
    let home = host::home_dir();
    std::fs::read_to_string(home.join(".config/lume/config.yaml"))
        .ok()
        .and_then(|y| lume_default_location(&y))
        .map(|p| expand_home(&p, &home))
        .unwrap_or_else(|| home.join(".lume"))
}

/// The cua home: QEMU instances (`vmm/qemu`) and the containerDisk cache
/// (`images`) live under it.
pub fn qemu_dir() -> PathBuf {
    host::cua_home()
}

fn expand_home(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if p == "~" => home.to_path_buf(),
        None => PathBuf::from(p),
    }
}

/// The path of Lume's `defaultLocationName` in its `config.yaml`.
pub fn lume_default_location(yaml: &str) -> Option<String> {
    let unquote = |v: &str| v.trim().trim_matches('"').trim_matches('\'').to_string();
    let default = yaml
        .lines()
        .find_map(|l| l.trim().strip_prefix("defaultLocationName:"))
        .map(unquote)?;
    let mut name: Option<String> = None;
    for line in yaml.lines() {
        let t = line.trim().trim_start_matches("- ").trim();
        if let Some(n) = t.strip_prefix("name:") {
            name = Some(unquote(n));
        } else if let Some(p) = t.strip_prefix("path:")
            && name.as_deref() == Some(default.as_str())
        {
            return Some(unquote(p));
        }
    }
    None
}

/// The container engine's data disk, when its free space can be read:
/// Colima (`df` in its VM, bounded by the host volume under its sparse
/// disk), Docker Desktop (the disk limit less what `Docker.raw` holds,
/// bounded by the host volume), OrbStack (its data directory's volume) and
/// dockerd on Linux (`/var/lib/docker`). `None` for anything else.
pub async fn container_volume() -> Option<Volume> {
    let ep = engine::discover()?;
    let home = host::home_dir();
    match ep.kind {
        EngineKind::Colima { profile } => {
            let colima = crate::container::gvisor::colima_home();
            let host_vol = host_volume(&colima);
            let bin = host::which("colima")?;
            let out = tokio::time::timeout(
                Duration::from_secs(8),
                host::run(
                    &bin,
                    &[
                        "ssh",
                        "--profile",
                        &profile,
                        "--",
                        "df",
                        "-P",
                        "-B1",
                        "/var/lib/docker",
                    ],
                ),
            )
            .await
            .ok()?
            .ok()?;
            let (available, total) = parse_df(&out)?;
            let vm = Volume {
                available,
                total,
                name: "Colima".into(),
            };
            Some(tighter(vm, host_vol))
        }
        EngineKind::DockerDesktop => {
            let raw = home.join("Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw");
            let host_vol = host_volume(&raw);
            let limit = docker_desktop_disk_limit(&home)?;
            let used = allocated(&raw).unwrap_or(0);
            let vm = Volume {
                available: limit.saturating_sub(used),
                total: limit,
                name: "Docker Desktop".into(),
            };
            Some(tighter(vm, host_vol))
        }
        EngineKind::OrbStack => host_volume(&home.join(".orbstack")),
        EngineKind::NativeLinux => host_volume(Path::new("/var/lib/docker")),
        _ => None,
    }
}

/// The engine's own disk, unless the host volume under it has less room.
fn tighter(engine: Volume, host: Option<Volume>) -> Volume {
    match host {
        Some(h) if h.available < engine.available => h,
        _ => engine,
    }
}

/// `(available, total)` from `df -P -B1 <path>`.
pub fn parse_df(out: &str) -> Option<(u64, u64)> {
    let line = out.lines().filter(|l| !l.trim().is_empty()).nth(1)?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 4 {
        return None;
    }
    Some((cols[3].parse().ok()?, cols[1].parse().ok()?))
}

/// Docker Desktop's disk limit in bytes (`DiskSizeMiB` in its settings).
fn docker_desktop_disk_limit(home: &Path) -> Option<u64> {
    for f in [
        "Library/Group Containers/group.com.docker/settings-store.json",
        "Library/Group Containers/group.com.docker/settings.json",
    ] {
        if let Some(mib) = std::fs::read_to_string(home.join(f))
            .ok()
            .and_then(|t| disk_size_mib(&t))
        {
            return Some(mib << 20);
        }
    }
    None
}

/// `DiskSizeMiB` (or the older `diskSizeMiB`) of a Docker Desktop settings file.
pub fn disk_size_mib(json: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("DiskSizeMiB")
        .or_else(|| v.get("diskSizeMiB"))
        .and_then(serde_json::Value::as_u64)
}

#[cfg(unix)]
fn allocated(p: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| m.blocks() * 512)
}

#[cfg(not(unix))]
fn allocated(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

/// The name of the volume holding `path`: the volume label on macOS (the
/// data volume answers with the startup disk's name, `Macintosh HD`), else
/// the mount point.
pub fn volume_name(path: &Path) -> String {
    let mut p = path.to_path_buf();
    while !p.exists() && p.pop() {}
    if p.as_os_str().is_empty() {
        p = PathBuf::from("/");
    }
    platform::volume_name(&p).unwrap_or_else(|| "this Mac".into())
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn mount_point(path: &Path) -> Option<String> {
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c` is NUL-terminated; `st` is plain data statfs fills.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        // SAFETY: statfs NUL-terminates f_mntonname.
        let m = unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) };
        Some(m.to_string_lossy().into_owned())
    }

    fn label(mount: &str) -> Option<String> {
        #[repr(C)]
        struct Buf {
            length: u32,
            name: libc::attrreference_t,
            data: [u8; 1024],
        }
        let c = CString::new(mount).ok()?;
        let mut list = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: 0,
            volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_NAME,
            dirattr: 0,
            fileattr: 0,
            forkattr: 0,
        };
        // SAFETY: zeroed plain data.
        let mut buf: Buf = unsafe { std::mem::zeroed() };
        // SAFETY: `list` and `buf` are valid for the call; the size passed is
        // the buffer's.
        let r = unsafe {
            libc::getattrlist(
                c.as_ptr(),
                (&mut list as *mut libc::attrlist).cast(),
                (&mut buf as *mut Buf).cast(),
                std::mem::size_of::<Buf>(),
                0,
            )
        };
        if r != 0 {
            return None;
        }
        let base = std::ptr::addr_of!(buf.name) as usize;
        let start = base + buf.name.attr_dataoffset as usize;
        let len = buf.name.attr_length as usize;
        let end = std::ptr::addr_of!(buf) as usize + std::mem::size_of::<Buf>();
        if len == 0 || start + len > end {
            return None;
        }
        // SAFETY: bounds checked against the buffer above.
        let bytes = unsafe { std::slice::from_raw_parts(start as *const u8, len) };
        let s = CStr::from_bytes_until_nul(bytes)
            .ok()?
            .to_string_lossy()
            .into_owned();
        (!s.is_empty()).then_some(s)
    }

    pub fn volume_name(path: &Path) -> Option<String> {
        let mount = mount_point(path)?;
        // The data volume is firmlinked under the startup disk: people
        // know it by the startup disk's name.
        let mount = if mount == "/System/Volumes/Data" {
            "/".to_string()
        } else {
            mount
        };
        label(&mount).or(Some(mount))
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    pub fn volume_name(path: &Path) -> Option<String> {
        // The longest mount point that holds `path` (/proc/self/mounts).
        let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
        let p = std::fs::canonicalize(path).ok()?;
        mounts
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1))
            .filter(|m| p.starts_with(m))
            .max_by_key(|m| m.len())
            .map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lume_default_location_follows_the_config() {
        let yaml = r#"
defaultLocationName: "ext"
cacheDirectory: "~/.lume/cache"
vmLocations:
  - name: "home"
    path: "~/.lume"
  - name: "ext"
    path: "/Volumes/Fast/lume"
"#;
        assert_eq!(
            lume_default_location(yaml).as_deref(),
            Some("/Volumes/Fast/lume")
        );
        let home = "defaultLocationName: home\nvmLocations:\n  - name: home\n    path: ~/.lume\n";
        assert_eq!(lume_default_location(home).as_deref(), Some("~/.lume"));
        assert_eq!(
            expand_home("~/.lume", Path::new("/Users/a")),
            PathBuf::from("/Users/a/.lume")
        );
        assert_eq!(lume_default_location("cachingEnabled: false"), None);
    }

    #[test]
    fn df_and_docker_desktop_settings_parse() {
        let df = "Filesystem       1-blocks        Used   Available Capacity Mounted on\n\
                  /dev/vdb1  105089261568 57034448896 42668208128      58% /mnt/lima-colima\n";
        assert_eq!(parse_df(df), Some((42668208128, 105089261568)));
        assert_eq!(parse_df("Filesystem\n"), None);
        assert_eq!(disk_size_mib(r#"{"DiskSizeMiB": 65536}"#), Some(65536));
        assert_eq!(disk_size_mib(r#"{"diskSizeMiB": 1024}"#), Some(1024));
        assert_eq!(disk_size_mib("{}"), None);
    }

    #[test]
    fn the_tighter_volume_wins() {
        let v = |a: u64, n: &str| Volume {
            available: a,
            total: 100,
            name: n.into(),
        };
        assert_eq!(
            tighter(v(40, "Colima"), Some(v(90, "Macintosh HD"))).name,
            "Colima"
        );
        assert_eq!(
            tighter(v(40, "Colima"), Some(v(10, "Macintosh HD"))).name,
            "Macintosh HD"
        );
        assert_eq!(tighter(v(40, "Colima"), None).name, "Colima");
    }

    #[test]
    fn the_home_volume_has_a_name() {
        let v = host_volume(&host::home_dir()).expect("home volume");
        assert!(v.total >= v.available);
        assert!(!v.name.is_empty());
    }
}
