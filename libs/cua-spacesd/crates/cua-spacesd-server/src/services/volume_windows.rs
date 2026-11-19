// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Windows guest mount of Cua Volume: the built-in NFS client ("Client
//! for NFS", the `NFS-Client` feature) against the guest's volume listener,
//! whose connections cua-spacesd relays to the client over `/volume`. The
//! client serves NFSv3 from the Space's view, as for macOS guests.
//!
//! - The Windows client has no port option, so a loopback portmapper
//!   ([`super::volume_portmap`]) on `127.0.0.1:111` points it at the
//!   listener.
//! - `mount -o anon,nolock 127.0.0.1:/ V:`: anonymous (the host serves the
//!   view, whatever the uid), no NLM (locks stay local to the guest, as on
//!   macOS). The drive is labelled "Cua Volume".
//! - A folder mount point (for example `C:\Users\<user>\Cua Volume`) is a
//!   directory symbolic link to the drive.
//! - Unmount is `umount V:` (forced when that fails or the client is gone).
//!
//! No third-party driver: the NFS redirector ships with Windows.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::volume_portmap::{Portmap, Ports, UdpBridge, PORTMAP_PORT};

/// The drive letter the volume takes by default, when free.
pub const DEFAULT_DRIVE: char = 'V';
/// The label Explorer shows for the drive.
pub const LABEL: &str = "Cua Volume";
/// `mount.exe` options. Soft with a long RPC timeout: an application sees
/// an error, not a hang, if the client goes away before the unmount.
pub const MOUNT_OPTIONS: &str = "anon,nolock,mtype=soft,timeout=60,retry=3,rsize=32,wsize=32";

const MOUNT_TIMEOUT: Duration = Duration::from_secs(60);
/// The first component of the export each mount names (the host's
/// `cua_volume::nfs::EXPORT_PREFIX`): `\\127.0.0.1\cua-volume-<volume id>`
/// mounts the root of the view. The Windows client caches the export's root
/// handle and its directory listings by server and export name across
/// mounts, so each mount (a Space's view, later an agent's) names its own.
/// (`127.0.0.1:/`, the root with an empty share name, maps a drive that
/// never answers: ERROR_BAD_NETPATH.)
pub const EXPORT_PREFIX: &str = "cua-volume-";

/// The share name for a volume session: lowercase hex of its id. A share
/// named with capitals (`cua-volume-vol-TlC_5zz...`) mounted and served
/// reads and writes, but the client refused a rename within one directory
/// on it by itself (ERROR_NOT_SAME_DEVICE, no RENAME call reached the
/// server): it compares share names in a case it chooses.
fn share_id(volume_id: &str) -> String {
    use std::hash::{Hash as _, Hasher as _};
    // SipHash with fixed keys: the same id gives the same name.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    volume_id.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The export in the forms `mount.exe` takes, tried in order until one
/// answers.
fn sources(volume_id: &str) -> [String; 2] {
    let share = format!("{EXPORT_PREFIX}{}", share_id(volume_id));
    [
        format!("\\\\127.0.0.1\\{share}"),
        format!("127.0.0.1:/{share}"),
    ]
}
/// How long a fresh mount may take to answer its first listing.
const FIRST_ANSWER: Duration = Duration::from_secs(10);

fn system32(exe: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Path::new(&root).join("System32").join(exe)
}

/// The switch that turns the Windows mount on. Off by default: renames
/// fail in the Windows NFS client (ERROR_NOT_SAME_DEVICE, see the volume
/// test's rename matrix), so the mount is not offered to Spaces until they
/// work. The Windows CI lane sets it.
pub const PREVIEW_ENV: &str = "CUA_VOLUME_WINDOWS_PREVIEW";

/// Whether this guest can mount (the preview is on and Client for NFS is
/// installed).
pub fn backend() -> Result<(), String> {
    if std::env::var(PREVIEW_ENV).as_deref() != Ok("1") {
        return Err("the Cua Volume mount is coming soon to Windows guests; \
                    use the volume_* tools meanwhile"
            .into());
    }
    if system32("mount.exe").is_file() && system32("umount.exe").is_file() {
        Ok(())
    } else {
        Err("this guest has no Windows NFS client (enable Client for NFS: \
             Install-WindowsFeature NFS-Client on Windows Server, \
             Enable-WindowsOptionalFeature -Online -FeatureName ServicesForNFS-ClientOnly,ClientForNFS-Infrastructure on Windows)"
            .into())
    }
}

/// The drive letter `path` names (`V:`, `V:\`, `V:/`), if it is one.
pub fn drive_of(path: &Path) -> Option<char> {
    let s = path.to_str()?;
    let b = s.as_bytes();
    let letter = *b.first()?;
    let rest = b.get(1..)?;
    (letter.is_ascii_alphabetic() && matches!(rest, b":" | b":\\" | b":/"))
        .then(|| (letter as char).to_ascii_uppercase())
}

/// `V:\` for a drive path, else the path as given.
pub fn normalize(path: PathBuf) -> PathBuf {
    match drive_of(&path) {
        Some(d) => PathBuf::from(format!("{d}:\\")),
        None => path,
    }
}

/// Letters in use (a bit per letter, A = bit 0).
fn used_drives() -> u32 {
    #[cfg(windows)]
    // SAFETY: no arguments; returns a bitmask.
    unsafe {
        windows_sys::Win32::Storage::FileSystem::GetLogicalDrives()
    }
    #[cfg(not(windows))]
    0
}

fn in_use(d: char, used: u32) -> bool {
    used & (1 << (d as u32 - 'A' as u32)) != 0
}

/// The first free letter: V, then W to Z, then U down to D.
fn free_drive(used: u32) -> Option<char> {
    std::iter::once(DEFAULT_DRIVE)
        .chain('W'..='Z')
        .chain(('D'..='U').rev())
        .find(|&d| !in_use(d, used))
}

/// Where the volume mounts by default: `V:\` (or the first free letter).
pub fn default_mount_path() -> PathBuf {
    let d = free_drive(used_drives()).unwrap_or(DEFAULT_DRIVE);
    PathBuf::from(format!("{d}:\\"))
}

/// Whether `d:` is a network drive now.
fn is_remote_drive(d: char) -> bool {
    if !in_use(d, used_drives()) {
        return false;
    }
    #[cfg(windows)]
    {
        let root: Vec<u16> = format!("{d}:\\").encode_utf16().chain([0]).collect();
        // SAFETY: a NUL-terminated wide string.
        let t = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) };
        // DRIVE_REMOTE
        t == 4
    }
    #[cfg(not(windows))]
    false
}

/// [`is_remote_drive`] on a blocking thread (the redirector may ask the
/// mount's server, which can be this same process).
async fn remote_drive(d: char) -> bool {
    tokio::task::spawn_blocking(move || is_remote_drive(d))
        .await
        .unwrap_or(false)
}

/// The drive a folder mount point links to.
fn linked_drive(path: &Path) -> Option<char> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    drive_of(&std::fs::read_link(path).ok()?)
}

/// Whether `path` (a drive, or a folder linked to one) is mounted now.
pub fn is_mounted(path: &Path) -> bool {
    match drive_of(path).or_else(|| linked_drive(path)) {
        Some(d) => is_remote_drive(d),
        None => false,
    }
}

/// A mounted volume.
#[derive(Debug)]
pub struct WinMount {
    drive: char,
    link: Option<PathBuf>,
    _portmap: Portmap,
    _udp: Option<UdpBridge>,
}

async fn run(exe: &str, args: &[&str], limit: Duration) -> Result<String, String> {
    let path = system32(exe);
    let out = tokio::time::timeout(
        limit,
        tokio::process::Command::new(&path)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("{exe} timed out"))?
    .map_err(|e| format!("{}: {e}", path.display()))?;
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    )
    .trim()
    .to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("{exe} {}: {text}", args.join(" ")))
    }
}

/// The remote name Windows gives drive `d:` (`\\127.0.0.1\...`).
fn remote_name(d: char) -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
        let local: Vec<u16> = format!("{d}:").encode_utf16().chain([0]).collect();
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: NUL-terminated input, a buffer of `len` wide chars.
        let rc = unsafe { WNetGetConnectionW(local.as_ptr(), buf.as_mut_ptr(), &mut len) };
        if rc != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
    #[cfg(not(windows))]
    {
        let _ = d;
        None
    }
}

/// Names the drive "Cua Volume" in Explorer (best effort).
async fn label(d: char) {
    let Some(remote) = remote_name(d) else {
        return;
    };
    let key = format!(
        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\MountPoints2\\{}",
        remote.replace('\\', "#")
    );
    let args = [
        "add",
        key.as_str(),
        "/v",
        "_LabelFromReg",
        "/t",
        "REG_SZ",
        "/d",
        LABEL,
        "/f",
    ];
    if let Err(e) = run("reg.exe", &args, Duration::from_secs(10)).await {
        tracing::debug!(error = %e, "volume drive label");
    }
}

async fn umount(d: char, clean: bool) {
    let drive = format!("{d}:");
    if clean
        && run("umount.exe", &[&drive], Duration::from_secs(30))
            .await
            .is_ok()
        && !remote_drive(d).await
    {
        return;
    }
    if let Err(e) = run("umount.exe", &["-f", &drive], Duration::from_secs(30)).await {
        if remote_drive(d).await {
            tracing::warn!(error = %e, "volume unmount");
        }
    }
}

/// Mounts the listener at `port` on `path` (a drive, or a folder linked to
/// a free drive).
pub async fn mount(port: u16, path: &Path, volume_id: &str) -> Result<WinMount, String> {
    let used = used_drives();
    let (drive, link) = match drive_of(path) {
        Some(d) if in_use(d, used) => return Err(format!("{d}: is already in use")),
        Some(d) => (d, None),
        None => {
            let d = free_drive(used).ok_or("no free drive letter")?;
            (d, Some(path.to_path_buf()))
        }
    };
    if let Some(link) = &link {
        match std::fs::symlink_metadata(link) {
            Ok(m) if m.file_type().is_symlink() => {
                std::fs::remove_dir(link).map_err(|e| format!("{}: {e}", link.display()))?;
            }
            Ok(m) if m.is_dir() => {
                // An empty folder gives way to the link; anything else stays.
                std::fs::remove_dir(link)
                    .map_err(|e| format!("{} is not an empty folder: {e}", link.display()))?;
            }
            Ok(_) => return Err(format!("{} exists and is a file", link.display())),
            Err(_) => {}
        }
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
    }
    // UDP on the listener's port number, carried to it over TCP: the
    // redirector may use UDP although the listener only speaks TCP.
    let udp = match UdpBridge::start(port, port).await {
        Ok(b) => Some(b),
        Err(e) => {
            tracing::warn!(error = %e, port, "volume: no UDP bridge; NFS over TCP only");
            None
        }
    };
    let ports = Ports {
        tcp: port,
        udp: udp.as_ref().map_or(0, |b| b.addr().port()),
    };
    let portmap = Portmap::start(PORTMAP_PORT, ports).await.map_err(|e| {
        format!(
            "the portmapper on 127.0.0.1:{PORTMAP_PORT}: {e} (the Windows NFS client finds \
             the volume through it; is Server for NFS or another portmapper running?)"
        )
    })?;
    let letter = format!("{drive}:");
    let root = PathBuf::from(format!("{drive}:\\"));
    let mut attempts = Vec::new();
    let mut mounted = false;
    let t0 = std::time::Instant::now();
    for source in sources(volume_id) {
        let source = source.as_str();
        match run(
            "mount.exe",
            &["-o", MOUNT_OPTIONS, source, &letter],
            MOUNT_TIMEOUT,
        )
        .await
        {
            Err(e) => {
                attempts.push(e);
                continue;
            }
            Ok(out) => tracing::info!(%source, %out, "volume: mount.exe"),
        }
        match first_answer(&root).await {
            Ok(()) if remote_drive(drive).await => {
                tracing::info!(
                    %source,
                    ms = t0.elapsed().as_millis() as u64,
                    earlier = ?attempts,
                    portmapper = ?portmap.calls(),
                    udp_datagrams = udp.as_ref().map_or(0, |b| b.calls()),
                    "volume: {letter} answers"
                );
                mounted = true;
                break;
            }
            Ok(()) => attempts.push(format!("{source}: {letter} is not a network drive")),
            Err(e) => attempts.push(format!("{source}: mounted, then {e}")),
        }
        umount(drive, false).await;
    }
    if !mounted {
        let listing = run("mount.exe", &[], Duration::from_secs(10))
            .await
            .unwrap_or_else(|e| e);
        return Err(format!(
            "{}; portmapper calls: [{}]; UDP datagrams bridged: {}; mount.exe lists: {}",
            attempts.join("; "),
            portmap.calls().join(", "),
            udp.as_ref().map_or(0, |b| b.calls()),
            listing.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
    label(drive).await;
    if let Some(link) = &link {
        if let Err(e) = link_dir(&root, link) {
            umount(drive, false).await;
            return Err(format!(
                "linking {} to {letter}: {e} (a folder mount point needs the right to create \
                 symbolic links: an elevated driver, or Developer Mode)",
                link.display()
            ));
        }
    }
    Ok(WinMount {
        drive,
        link,
        _portmap: portmap,
        _udp: udp,
    })
}

/// Lists `root` until it answers (the redirector may need a moment after
/// `mount.exe` returns), for at most [`FIRST_ANSWER`].
async fn first_answer(root: &Path) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + FIRST_ANSWER;
    loop {
        let probe = root.to_path_buf();
        let listed = tokio::time::timeout(
            FIRST_ANSWER,
            tokio::task::spawn_blocking(move || std::fs::read_dir(&probe).map(|_| ())),
        )
        .await;
        let e = match listed {
            Ok(Ok(Ok(()))) => return Ok(()),
            Ok(Ok(Err(e))) => e.to_string(),
            Ok(Err(e)) => e.to_string(),
            Err(_) => format!("listing {} timed out", root.display()),
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(e);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Forces off a mount at `path` that no [`WinMount`] records (a mount cut
/// short between `mount.exe` and its bookkeeping).
pub async fn unmount_path(path: &Path) {
    let link = linked_drive(path);
    if let Some(d) = drive_of(path).or(link) {
        // Only the volume's own mount (served from loopback), never a drive
        // someone else mapped on that letter.
        let ours = remote_name(d).is_some_and(|r| r.contains("127.0.0.1"));
        if ours && remote_drive(d).await {
            if link.is_some() {
                let _ = std::fs::remove_dir(path);
            }
            umount(d, false).await;
        }
    }
}

/// Unmounts (cleanly first when the client still serves the volume) and
/// removes a folder mount point.
pub async fn unmount(m: WinMount, client_attached: bool) {
    if let Some(link) = &m.link {
        if linked_drive(link) == Some(m.drive) {
            let _ = std::fs::remove_dir(link);
        }
    }
    umount(m.drive, client_attached).await;
}

/// A directory symbolic link at `link` to `target`.
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link)
    }
    #[cfg(not(windows))]
    {
        let _ = (target, link);
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_paths() {
        assert_eq!(drive_of(Path::new("v:")), Some('V'));
        assert_eq!(drive_of(Path::new("V:\\")), Some('V'));
        assert_eq!(drive_of(Path::new("V:/")), Some('V'));
        assert_eq!(drive_of(Path::new("V:\\x")), None);
        assert_eq!(drive_of(Path::new("C:\\Users\\a\\Cua Volume")), None);
        assert_eq!(drive_of(Path::new("1:")), None);
        assert_eq!(normalize("w:".into()), PathBuf::from("W:\\"));
        let folder = PathBuf::from("C:\\Users\\a\\Cua Volume");
        assert_eq!(normalize(folder.clone()), folder);
    }

    #[test]
    fn each_mount_names_its_own_export() {
        let [unc, nfs] = sources("vol-TlC_5zzgwIQv");
        let id = share_id("vol-TlC_5zzgwIQv");
        assert_eq!(id.len(), 16);
        assert!(id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(unc, format!("\\\\127.0.0.1\\cua-volume-{id}"));
        assert_eq!(nfs, format!("127.0.0.1:/cua-volume-{id}"));
        // Stable per id, different between ids (and ids differing in case).
        assert_eq!(sources("vol-abc"), sources("vol-abc"));
        assert_ne!(sources("vol-abc"), sources("vol-abd"));
        assert_ne!(sources("vol-abc"), sources("vol-ABC"));
    }

    #[test]
    fn free_letters_prefer_v() {
        let bit = |d: char| 1u32 << (d as u32 - 'A' as u32);
        assert_eq!(free_drive(0), Some('V'));
        assert_eq!(free_drive(bit('V')), Some('W'));
        assert_eq!(
            free_drive(bit('V') | bit('W') | bit('X') | bit('Y') | bit('Z')),
            Some('U')
        );
        assert_eq!(free_drive(u32::MAX), None);
    }
}
