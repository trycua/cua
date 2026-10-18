// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Read-only probes of the guest the doctor runs in: file modes, mounts,
//! processes, init system. Nothing here writes outside the doctor's own
//! scratch directory, and every read and command is size- and time-bounded.

use std::path::Path;
use std::time::Duration;

/// Largest file or command output the doctor reads.
pub const MAX_READ_BYTES: usize = 1024 * 1024;

/// Most processes scanned under `/proc`.
pub const MAX_PROCESSES: usize = 4096;

/// The init system of this guest: "systemd", "supervisord", "launchd",
/// "scm" or "unknown".
pub fn detect_init() -> String {
    if cfg!(target_os = "macos") {
        return "launchd".into();
    }
    if cfg!(windows) {
        return "scm".into();
    }
    let comm = read_capped(Path::new("/proc/1/comm")).unwrap_or_default();
    let cmdline = proc_cmdline(1).unwrap_or_default();
    let init = classify_init(comm.trim(), &cmdline);
    if init != "unknown" || !PLATFORM_INITS.contains(&comm.trim()) {
        return init;
    }
    // A platform's own minimal PID 1 (Docker's --init, Modal) reaps and
    // forwards signals to the image's init, its child.
    let children: Vec<(String, String)> = pids()
        .into_iter()
        .filter(|p| *p != 1 && ppid(*p) == Some(1))
        .filter_map(|p| {
            let comm = read_capped(Path::new(&format!("/proc/{p}/comm")))?;
            Some((comm.trim().to_owned(), proc_cmdline(p).unwrap_or_default()))
        })
        .collect();
    init_under_platform(&children).unwrap_or(init)
}

/// PID 1 programs that are a platform's, not the image's: minimal inits
/// that start the image's command as their child (`docker run --init`,
/// tini, catatonit, Modal's `dumb-init` and its VM runtime's
/// `runch-agent`).
pub const PLATFORM_INITS: &[&str] = &[
    "dumb-init",
    "tini",
    "docker-init",
    "catatonit",
    "runch-agent",
];

/// The platform's PID 1 when it is one of [`PLATFORM_INITS`] (`None`
/// otherwise, and off Linux).
pub fn platform_init() -> Option<String> {
    let comm = read_capped(Path::new("/proc/1/comm"))?;
    let comm = comm.trim();
    PLATFORM_INITS.contains(&comm).then(|| comm.to_owned())
}

/// The image's init among PID 1's children (`(comm, cmdline)` each).
pub fn init_under_platform(children: &[(String, String)]) -> Option<String> {
    children
        .iter()
        .map(|(comm, cmdline)| classify_init(comm, cmdline))
        .find(|i| i != "unknown")
}

/// A process's parent pid (`/proc/<pid>/stat`, the field after the
/// parenthesised command).
pub fn ppid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Classifies PID 1 by its `comm` and command line: the program itself
/// (or a script interpreter's script), never a word deeper in its
/// arguments (a platform init's `sh -c` script may name anything).
pub fn classify_init(comm: &str, cmdline: &str) -> String {
    let mut words = cmdline.split_whitespace();
    let exe = words.next().unwrap_or_default();
    let script = words.next().unwrap_or_default();
    let is = |name: &str| {
        [exe, script]
            .iter()
            .any(|w| w.rsplit('/').next() == Some(name))
    };
    if comm == "systemd" || exe.starts_with("/sbin/init") || is("systemd") {
        "systemd"
    } else if comm == "supervisord" || is("supervisord") {
        "supervisord"
    } else {
        "unknown"
    }
    .into()
}

/// Per-run scratch directory in the guest.
pub fn scratch_dir(nonce: &str) -> String {
    let base = std::env::temp_dir();
    base.join(format!("cua-doctor-{nonce}"))
        .to_string_lossy()
        .into_owned()
}

/// Reads at most [`MAX_READ_BYTES`] of a file as lossy UTF-8.
pub fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(MAX_READ_BYTES as u64)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Owner, group and permission bits of a path (following symlinks).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Permission bits (`0o640`).
    pub mode: u32,
    /// Owner uid.
    pub uid: u32,
    /// Group gid.
    pub gid: u32,
}

impl Mode {
    /// "0640 root:cua".
    pub fn describe(&self) -> String {
        format!(
            "{:04o} {}:{}",
            self.mode,
            user_name(self.uid).unwrap_or_else(|| self.uid.to_string()),
            group_name(self.gid).unwrap_or_else(|| self.gid.to_string())
        )
    }
}

/// Mode of `path` (following symlinks), `None` when missing.
#[cfg(unix)]
pub fn mode(path: &Path) -> Option<Mode> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(path).ok()?;
    Some(Mode {
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
    })
}

/// Mode of `path` (not available on this platform).
#[cfg(not(unix))]
pub fn mode(_path: &Path) -> Option<Mode> {
    None
}

fn lookup(file: &str, id: u32) -> Option<String> {
    let text = read_capped(Path::new(file))?;
    text.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next()?;
        let value: u32 = fields.next()?.parse().ok()?;
        (value == id).then(|| name.to_owned())
    })
}

fn lookup_id(file: &str, name: &str) -> Option<u32> {
    let text = read_capped(Path::new(file))?;
    text.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == name).then_some(())?;
        let _password = fields.next()?;
        fields.next()?.parse().ok()
    })
}

/// User name of `uid` from `/etc/passwd`.
pub fn user_name(uid: u32) -> Option<String> {
    lookup("/etc/passwd", uid)
}

/// Group name of `gid` from `/etc/group`.
pub fn group_name(gid: u32) -> Option<String> {
    lookup("/etc/group", gid)
}

/// Uid of user `name`.
pub fn user_id(name: &str) -> Option<u32> {
    lookup_id("/etc/passwd", name)
}

/// Gid of group `name`.
pub fn group_id(name: &str) -> Option<u32> {
    lookup_id("/etc/group", name)
}

/// One mount from `/proc/self/mountinfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// Mount point.
    pub point: String,
    /// Filesystem type.
    pub fstype: String,
    /// Source.
    pub source: String,
}

/// Parses mountinfo text (the last mount at a point is the visible one).
pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let point = left.split_whitespace().nth(4)?.to_owned();
            let mut right = right.split_whitespace();
            let fstype = right.next()?.to_owned();
            let source = right.next().unwrap_or_default().to_owned();
            Some(Mount {
                point: unescape_mount(&point),
                fstype,
                source,
            })
        })
        .collect()
}

fn unescape_mount(point: &str) -> String {
    point.replace("\\040", " ")
}

/// The visible mount at exactly `point`, if any.
pub fn mount_at(point: &str) -> Option<Mount> {
    let text = read_capped(Path::new("/proc/self/mountinfo"))?;
    parse_mountinfo(&text)
        .into_iter()
        .rev()
        .find(|m| m.point == point)
}

/// Whether a virtio-fs device (virtio id 0x001a) is attached: KubeVirt's
/// claim-secrets share, or QEMU's `--claim-secrets`.
pub fn virtio_fs_present() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/bus/virtio/devices") else {
        return false;
    };
    entries.take(256).flatten().any(|entry| {
        read_capped(&entry.path().join("device")).is_some_and(|d| d.trim() == "0x001a")
    })
}

/// Pids under `/proc` (bounded).
pub fn pids() -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .take(MAX_PROCESSES)
        .collect()
}

/// A process's command line with NULs as spaces.
pub fn proc_cmdline(pid: u32) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_READ_BYTES)])
            .replace('\0', " ")
            .trim()
            .to_owned(),
    )
}

/// A process's environment block (`None` when unreadable).
pub fn proc_environ(pid: u32) -> Option<Vec<u8>> {
    let bytes = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    Some(bytes[..bytes.len().min(MAX_READ_BYTES)].to_vec())
}

/// Owner uid of a process.
#[cfg(unix)]
pub fn proc_uid(pid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(format!("/proc/{pid}"))
        .ok()
        .map(|m| m.uid())
}

/// Owner uid of a process (not available on this platform).
#[cfg(not(unix))]
pub fn proc_uid(_pid: u32) -> Option<u32> {
    None
}

/// This process's effective uid.
#[cfg(unix)]
pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// This process's effective uid (not available on this platform).
#[cfg(not(unix))]
pub fn euid() -> u32 {
    u32::MAX
}

/// Filesystem capacity of `path`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FsStat {
    /// Total bytes.
    pub total: u64,
    /// Bytes available to unprivileged users.
    pub available: u64,
    /// Total inodes.
    pub files: u64,
    /// Free inodes.
    pub files_free: u64,
}

/// `statvfs(path)`.
#[cfg(unix)]
pub fn statvfs(path: &Path) -> Option<FsStat> {
    use std::os::unix::ffi::OsStrExt as _;
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: zeroed out-struct, valid C string.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let frag = stat.f_frsize as u64;
    Some(FsStat {
        total: stat.f_blocks as u64 * frag,
        available: stat.f_bavail as u64 * frag,
        files: stat.f_files as u64,
        files_free: stat.f_ffree as u64,
    })
}

/// `statvfs(path)` (not available on this platform).
#[cfg(not(unix))]
pub fn statvfs(_path: &Path) -> Option<FsStat> {
    None
}

/// Result of a local command.
#[derive(Clone, Debug, Default)]
pub struct Local {
    /// Exit code (`None` when killed or not started).
    pub code: Option<i32>,
    /// stdout (capped).
    pub stdout: String,
    /// stderr (capped).
    pub stderr: String,
}

impl Local {
    /// Exit code 0.
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs a local command with a timeout and capped output. `env` adds
/// variables. Errors when the program cannot start.
pub async fn run_local(
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
) -> std::io::Result<Local> {
    use tokio::io::AsyncReadExt as _;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let work = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take(MAX_READ_BYTES as u64);
        let mut stderr = stderr.take(MAX_READ_BYTES as u64);
        let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        a?;
        b?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>(Local {
            code: status.code(),
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        })
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("{program} did not finish within {timeout:?}"),
        )),
    }
}

/// Whether `program` is on PATH.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join(program);
            candidate.is_file()
        })
    })
}

/// Resolves a symlink chain to its final path (bounded).
pub fn resolve(path: &Path) -> Option<std::path::PathBuf> {
    std::fs::canonicalize(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_pid1() {
        assert_eq!(classify_init("systemd", "/sbin/init splash"), "systemd");
        assert_eq!(
            classify_init(
                "supervisord",
                "/usr/bin/python3 /usr/bin/supervisord -n -c x"
            ),
            "supervisord"
        );
        assert_eq!(classify_init("bash", "bash"), "unknown");
        assert_eq!(
            classify_init("systemd", "/lib/systemd/systemd --system"),
            "systemd"
        );
        // A platform init whose script mentions /run/systemd is not systemd.
        assert_eq!(
            classify_init(
                "dumb-init",
                "/bin/dumb-init -- /bin/sh -c mkdir -p /run/systemd/seats; exec supervisord"
            ),
            "unknown"
        );
    }

    #[test]
    fn the_images_init_under_a_platform_pid1_is_the_init() {
        let kids = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        assert_eq!(
            init_under_platform(&kids(&[
                ("sh", "/bin/sh -c x"),
                (
                    "supervisord",
                    "/usr/bin/python3 /usr/bin/supervisord -n -c x"
                )
            ]))
            .as_deref(),
            Some("supervisord")
        );
        assert_eq!(init_under_platform(&kids(&[("bash", "bash")])), None);
        assert!(PLATFORM_INITS.contains(&"dumb-init") && PLATFORM_INITS.contains(&"runch-agent"));
    }

    #[test]
    fn parses_mountinfo() {
        let text = "\
22 1 0:21 / /run rw,nosuid shared:5 - tmpfs tmpfs rw,size=1024k
30 22 0:40 / /run/cua ro,relatime shared:9 - virtiofs cua-claim-secrets rw
31 22 0:41 / /run/cua rw,nosuid - tmpfs cua-claim-secrets rw,mode=755
40 1 0:50 / /mnt/with\\040space rw - ext4 /dev/vda1 rw
";
        let mounts = parse_mountinfo(text);
        assert_eq!(mounts.len(), 4);
        assert_eq!(mounts[1].fstype, "virtiofs");
        assert_eq!(mounts[3].point, "/mnt/with space");
        // The last mount at a point is the visible one.
        let visible = mounts.iter().rev().find(|m| m.point == "/run/cua").unwrap();
        assert_eq!(visible.fstype, "tmpfs");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_commands_are_bounded() {
        let ok = run_local(
            "sh",
            &["-c", "echo $X"],
            &[("X", "hi")],
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(ok.ok());
        assert_eq!(ok.stdout.trim(), "hi");
        let slow = run_local("sleep", &["5"], &[], Duration::from_millis(100)).await;
        assert_eq!(slow.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert!(
            run_local("/nonexistent/x", &[], &[], Duration::from_secs(1))
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn modes_and_names() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "x").unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let m = mode(&file).unwrap();
        assert_eq!(m.mode, 0o640);
        assert_eq!(m.uid, euid());
        assert!(mode(&dir.path().join("missing")).is_none());
        assert_eq!(user_name(0).as_deref(), Some("root"));
    }
}
