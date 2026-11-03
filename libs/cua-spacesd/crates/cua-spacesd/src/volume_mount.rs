// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Linux guest mount of Cua Volume: FUSE over the file operations
//! protocol, dialled through the driver's loopback volume listener (whose
//! connections the driver relays to the client over `/volume`).
//!
//! The mount needs root: gVisor refuses an unprivileged `fusermount3`, and
//! does not honour setuid binaries, so `sudo` cannot elevate there either.
//! Two entry points:
//!
//! - `cua-spacesd volume-helper`: a root service started by the image's init
//!   (supervisord in containers, systemd in VMs). It listens on
//!   [`HELPER_SOCKET`], accepts requests only from the allowed user, and
//!   mounts on a directory that user owns (fusermount's rule).
//! - `cua-spacesd volume-mount`: one mount in the foreground, for a driver
//!   that is root or can `sudo -n`.
//!
//! - `cua-spacesd volume-bind`: one bind mount of a folder of the volume at
//!   another path (a named volume mounted into the Space), or the undoing
//!   of one. The helper answers the same as `bind`/`unbind` requests.
//!   Binds are recorded in [`BIND_REGISTRY`] so a restarted driver can
//!   undo what the one before it left.
//!
//! A mount ends on request, on SIGTERM, or when the client goes away.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use cua_volume::remote::RemoteFs;
use cua_volume::vfs::{FsOps, ROOT};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};

/// Where the helper listens.
pub const HELPER_SOCKET: &str = "/run/cua/volume.sock";

/// Where the binds in place are recorded (one target per line).
pub const BIND_REGISTRY: &str = "/run/cua/volume-binds";

/// Arguments of `volume-bind`.
#[derive(Args, Debug)]
pub struct VolumeBindArgs {
    /// The folder to bind (inside the mounted volume).
    #[arg(long)]
    source: Option<PathBuf>,
    /// Where it appears.
    #[arg(long)]
    target: Option<PathBuf>,
    /// Make the bind read-only.
    #[arg(long)]
    read_only: bool,
    /// Undo the bind at `--target`.
    #[arg(long)]
    unbind: bool,
    /// Undo every bind recorded (what a driver that died left).
    #[arg(long)]
    clear: bool,
}

/// Arguments of `volume-mount`.
#[derive(Args, Debug)]
pub struct VolumeMountArgs {
    /// The driver's loopback volume listener.
    #[arg(long)]
    port: u16,
    /// The mount point.
    #[arg(long)]
    path: PathBuf,
    /// The user the files are reported as owned by (default: the user
    /// `sudo` ran this for, else this process).
    #[arg(long)]
    uid: Option<u32>,
    #[arg(long)]
    gid: Option<u32>,
}

/// Arguments of `volume-helper`.
#[derive(Args, Debug)]
pub struct VolumeHelperArgs {
    /// The socket to listen on.
    #[arg(long, default_value = HELPER_SOCKET)]
    socket: PathBuf,
    /// The only uid allowed to ask (the desktop user). Default: any
    /// non-root caller, still limited to mount points it owns.
    #[arg(long)]
    allow_uid: Option<u32>,
}

/// One request to the helper (one JSON line; one JSON line answers).
#[derive(Debug, Serialize, Deserialize)]
pub struct HelperRequest {
    /// `mount`, `unmount`, `bind`, `unbind` or `unbind_all`.
    pub op: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub path: PathBuf,
    /// For `bind`: the folder of a mounted volume to bind at `path`.
    #[serde(default)]
    pub source: PathBuf,
    /// For `bind`: read-only.
    #[serde(default)]
    pub read_only: bool,
}

/// The helper's answer.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HelperReply {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
}

/// Mounts `path` over the listener at `port`, reports on `ready` once
/// mounted (or with the reason it could not), serves until `stop` resolves
/// or the client goes away, then unmounts.
async fn serve_mount(
    port: u16,
    path: &Path,
    owner: Option<(u32, u32)>,
    ready: oneshot::Sender<Result<(), String>>,
    stop: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let setup = async {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|e| format!("the volume listener: {e}"))?;
        let _ = stream.set_nodelay(true);
        let remote = RemoteFs::new(stream);
        // The client answers before anything is mounted.
        remote
            .getattr(ROOT)
            .await
            .map_err(|e| format!("the client did not answer: {e}"))?;
        std::fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // SAFETY: geteuid never fails.
        let root = unsafe { libc::geteuid() } == 0;
        let mount =
            // Small writes are gathered: each one would be a round trip
            // to the host.
            cua_volume::fuse::FuseMount::mount_with(
                cua_volume::remote::WriteBack::new(remote.clone()),
                path,
                root,
                "cua-volume",
                owner,
            )
                .map_err(|e| e.to_string())?;
        Ok::<_, String>((remote, mount))
    };
    let (remote, mount) = match setup.await {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };
    let _ = ready.send(Ok(()));
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if remote.is_closed() {
                    break;
                }
            }
        }
    }
    mount.unmount().await.map_err(|e| e.to_string())
}

/// `volume-mount`: one mount in the foreground; returns the exit code.
pub async fn run(args: VolumeMountArgs) -> i32 {
    let (tx, rx) = oneshot::channel();
    let stop = async {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => return std::future::pending().await,
            };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    };
    let from_sudo = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok());
    let owner = match (
        args.uid.or_else(|| from_sudo("SUDO_UID")),
        args.gid.or_else(|| from_sudo("SUDO_GID")),
    ) {
        (Some(u), Some(g)) => Some((u, g)),
        _ => None,
    };
    let serve = serve_mount(args.port, &args.path, owner, tx, stop);
    tokio::spawn(async move {
        if let Ok(Err(e)) = rx.await {
            eprintln!("volume-mount: {e}");
        }
    });
    match serve.await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("volume-mount: {e}");
            1
        }
    }
}

/// Directories nobody but root may put a volume over, however they are
/// owned: a bind over them would let a user replace system files.
const PROTECTED: &[&str] = &[
    "/etc",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/usr",
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/boot",
    "/root",
    "/var/lib",
    "/var/run",
    "/var/log",
    "/var/spool",
    "/snap",
];

/// Where any caller may bind without owning the place: the usual spots for
/// mounted data.
const OPEN_ROOTS: &[&str] = &["/mnt", "/media", "/srv", "/data", "/workspace", "/volumes"];

fn lexical_components_ok(p: &Path) -> bool {
    use std::path::Component;
    p.is_absolute()
        && p.components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        // `components` drops a `.` in the middle: look at the text too.
        && p.to_str()
            .is_some_and(|t| t.split('/').all(|seg| seg != "." && seg != ".."))
}

/// Whether `uid` may bind a volume folder at `target`: not over system
/// directories, and either under one of the usual data roots or in a place
/// it owns (the nearest existing directory above it, for a target that is
/// created). `owner` says who owns an existing path.
fn may_bind_target(
    uid: u32,
    target: &Path,
    owner: impl Fn(&Path) -> Option<u32>,
) -> Result<(), String> {
    if !lexical_components_ok(target) || target == Path::new("/") {
        return Err(format!(
            "{}: use an absolute path without `.` or `..`",
            target.display()
        ));
    }
    if let Some(p) = PROTECTED
        .iter()
        .find(|p| target == Path::new(p) || target.starts_with(p))
    {
        return Err(format!("{}: {p} is a system directory", target.display()));
    }
    if uid == 0 || OPEN_ROOTS.iter().any(|r| target.starts_with(r)) {
        return Ok(());
    }
    let mut nearest = target;
    loop {
        if let Some(o) = owner(nearest) {
            return if o == uid {
                Ok(())
            } else {
                Err(format!(
                    "{} is owned by uid {o}, not the caller ({uid}); bind under /mnt or a directory you own",
                    nearest.display()
                ))
            };
        }
        match nearest.parent() {
            Some(p) if p != Path::new("/") => nearest = p,
            _ => {
                return Err(format!(
                    "{}: bind under /mnt or a directory you own",
                    target.display()
                ));
            }
        }
    }
}

fn owner_of(p: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::symlink_metadata(p).ok().map(|m| m.uid())
}

fn registry_read() -> Vec<PathBuf> {
    std::fs::read_to_string(BIND_REGISTRY)
        .map(|t| {
            t.lines()
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn registry_write(targets: &[PathBuf]) {
    if let Some(dir) = Path::new(BIND_REGISTRY).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body: String = targets
        .iter()
        .map(|t| format!("{}\n", t.display()))
        .collect();
    let _ = std::fs::write(BIND_REGISTRY, body);
}

fn c_path(p: &Path) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt as _;
    std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| "a NUL in the path".to_string())
}

fn errno() -> String {
    std::io::Error::last_os_error().to_string()
}

/// Binds `source` at `target` (read-only when asked), creating `target`
/// (owned by `owner` when given) and recording the bind. Needs root.
pub fn bind(
    source: &Path,
    target: &Path,
    read_only: bool,
    owner: Option<(u32, u32)>,
) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!("{}: not a folder in the volume", source.display()));
    }
    if !target.exists() {
        std::fs::create_dir_all(target).map_err(|e| format!("{}: {e}", target.display()))?;
        if let Some((u, g)) = owner {
            let c = c_path(target)?;
            // SAFETY: a valid NUL-terminated path.
            unsafe { libc::chown(c.as_ptr(), u, g) };
        }
    } else if !target.is_dir() {
        return Err(format!("{} is not a directory", target.display()));
    }
    // A second bind at the same place replaces the first.
    let _ = unbind(target);
    let (s, t) = (c_path(source)?, c_path(target)?);
    // SAFETY: valid NUL-terminated paths; null for the unused arguments.
    let rc = unsafe {
        libc::mount(
            s.as_ptr(),
            t.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        )
    };
    if rc != 0 {
        return Err(format!(
            "bind {} at {}: {}",
            source.display(),
            target.display(),
            errno()
        ));
    }
    if read_only {
        // SAFETY: as above.
        let rc = unsafe {
            libc::mount(
                std::ptr::null(),
                t.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY,
                std::ptr::null(),
            )
        };
        if rc != 0 {
            let e = errno();
            // SAFETY: a valid path.
            unsafe { libc::umount2(t.as_ptr(), libc::MNT_DETACH) };
            return Err(format!("make {} read-only: {e}", target.display()));
        }
    }
    let mut all = registry_read();
    if !all.iter().any(|p| p == target) {
        all.push(target.to_path_buf());
        registry_write(&all);
    }
    Ok(())
}

/// Undoes the bind at `target` (lazily, so a busy folder lets go at once);
/// fine when there is none.
pub fn unbind(target: &Path) -> Result<(), String> {
    let t = c_path(target)?;
    // SAFETY: a valid NUL-terminated path.
    let rc = unsafe { libc::umount2(t.as_ptr(), libc::MNT_DETACH) };
    let gone = rc == 0
        || matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EINVAL | libc::ENOENT)
        );
    let rest: Vec<PathBuf> = registry_read()
        .into_iter()
        .filter(|p| p != target)
        .collect();
    registry_write(&rest);
    if gone {
        Ok(())
    } else {
        Err(format!("unbind {}: {}", target.display(), errno()))
    }
}

/// Undoes every recorded bind.
pub fn unbind_all() {
    for t in registry_read() {
        let _ = unbind(&t);
    }
    registry_write(&[]);
}

/// `volume-bind`: one bind (or its undoing); returns the exit code.
pub fn run_bind(args: VolumeBindArgs) -> i32 {
    // SAFETY: geteuid never fails.
    let root = unsafe { libc::geteuid() } == 0;
    let from_sudo = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok());
    let owner = match (from_sudo("SUDO_UID"), from_sudo("SUDO_GID")) {
        (Some(u), Some(g)) => Some((u, g)),
        _ => None,
    };
    let result = if args.clear {
        unbind_all();
        Ok(())
    } else {
        match (&args.target, &args.source) {
            (Some(t), _) if args.unbind => unbind(t),
            (Some(t), Some(s)) if root => bind(s, t, args.read_only, owner),
            (Some(_), Some(_)) => Err("volume-bind must run as root".to_string()),
            _ => Err("--target (and --source) are required".to_string()),
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("volume-bind: {e}");
            1
        }
    }
}

/// Whether `uid` may mount on `path`: an existing directory (not a
/// symlink) the caller owns, as fusermount requires.
fn may_mount(uid: u32, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    if !path.is_absolute() {
        return Err("the mount point must be absolute".into());
    }
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("{}: {e} (create it first)", path.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    if uid != 0 && meta.uid() != uid {
        return Err(format!(
            "{} is owned by uid {}, not the caller ({uid})",
            path.display(),
            meta.uid()
        ));
    }
    Ok(())
}

type Mounts = Arc<Mutex<HashMap<PathBuf, (oneshot::Sender<()>, tokio::task::JoinHandle<()>)>>>;

async fn handle(
    stream: tokio::net::UnixStream,
    allow_uid: Option<u32>,
    mounts: Mounts,
) -> std::io::Result<()> {
    let cred = stream.peer_cred()?;
    let (uid, gid) = (cred.uid(), cred.gid());
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let reply = match serde_json::from_str::<HelperRequest>(&line) {
        Err(e) => HelperReply {
            ok: false,
            error: format!("bad request: {e}"),
        },
        Ok(_) if uid != 0 && allow_uid.is_some_and(|a| a != uid) => HelperReply {
            ok: false,
            error: format!("uid {uid} may not use the volume helper"),
        },
        Ok(req) => match req.op.as_str() {
            "mount" => match may_mount(uid, &req.path) {
                Err(e) => HelperReply {
                    ok: false,
                    error: e,
                },
                Ok(()) => {
                    let mut m = mounts.lock().await;
                    if m.contains_key(&req.path) {
                        HelperReply {
                            ok: false,
                            error: format!("{} is already mounted", req.path.display()),
                        }
                    } else {
                        let (ready_tx, ready_rx) = oneshot::channel();
                        let (stop_tx, stop_rx) = oneshot::channel::<()>();
                        let (path, port, ms) = (req.path.clone(), req.port, mounts.clone());
                        let task = tokio::spawn(async move {
                            let stop = async {
                                let _ = stop_rx.await;
                            };
                            // The files belong to the caller, whose rights
                            // the kernel then checks.
                            let owner = Some((uid, gid));
                            if let Err(e) = serve_mount(port, &path, owner, ready_tx, stop).await {
                                eprintln!("volume-helper: {}: {e}", path.display());
                            }
                            // Ended on its own (the client left): forget it.
                            ms.lock().await.remove(&path);
                            unbind_all();
                        });
                        m.insert(req.path.clone(), (stop_tx, task));
                        drop(m);
                        match tokio::time::timeout(Duration::from_secs(20), ready_rx).await {
                            Ok(Ok(Ok(()))) => HelperReply {
                                ok: true,
                                ..Default::default()
                            },
                            Ok(Ok(Err(e))) => HelperReply {
                                ok: false,
                                error: e,
                            },
                            _ => HelperReply {
                                ok: false,
                                error: "the mount did not come up in 20 s".into(),
                            },
                        }
                    }
                }
            },
            "bind" => {
                let sources_ok = {
                    let m = mounts.lock().await;
                    lexical_components_ok(&req.source)
                        && m.keys().any(|root| req.source.starts_with(root))
                };
                if !sources_ok {
                    HelperReply {
                        ok: false,
                        error: format!(
                            "{}: only folders of a mounted volume can be bound",
                            req.source.display()
                        ),
                    }
                } else if let Err(e) = may_bind_target(uid, &req.path, owner_of) {
                    HelperReply {
                        ok: false,
                        error: e,
                    }
                } else {
                    let (source, path, ro) = (req.source.clone(), req.path.clone(), req.read_only);
                    match tokio::task::spawn_blocking(move || {
                        bind(&source, &path, ro, Some((uid, gid)))
                    })
                    .await
                    {
                        Ok(Ok(())) => HelperReply {
                            ok: true,
                            ..Default::default()
                        },
                        Ok(Err(e)) => HelperReply {
                            ok: false,
                            error: e,
                        },
                        Err(e) => HelperReply {
                            ok: false,
                            error: e.to_string(),
                        },
                    }
                }
            }
            "unbind" => {
                // Only what a bind of this helper recorded.
                if registry_read().contains(&req.path) {
                    let path = req.path.clone();
                    match tokio::task::spawn_blocking(move || unbind(&path)).await {
                        Ok(Ok(())) => HelperReply {
                            ok: true,
                            ..Default::default()
                        },
                        Ok(Err(e)) => HelperReply {
                            ok: false,
                            error: e,
                        },
                        Err(e) => HelperReply {
                            ok: false,
                            error: e.to_string(),
                        },
                    }
                } else {
                    HelperReply {
                        ok: true,
                        ..Default::default()
                    }
                }
            }
            "unbind_all" => {
                let _ = tokio::task::spawn_blocking(unbind_all).await;
                HelperReply {
                    ok: true,
                    ..Default::default()
                }
            }
            "unmount" => {
                // The binds of the volume go first: they would hold it busy.
                let _ = tokio::task::spawn_blocking(unbind_all).await;
                let entry = mounts.lock().await.remove(&req.path);
                match entry {
                    Some((stop, task)) => {
                        let _ = stop.send(());
                        let _ = tokio::time::timeout(Duration::from_secs(15), task).await;
                        HelperReply {
                            ok: true,
                            ..Default::default()
                        }
                    }
                    None => HelperReply {
                        ok: true,
                        ..Default::default()
                    },
                }
            }
            other => HelperReply {
                ok: false,
                error: format!("unknown op {other:?}"),
            },
        },
    };
    let mut out = serde_json::to_vec(&reply).unwrap_or_default();
    out.push(b'\n');
    write.write_all(&out).await
}

/// `volume-helper`: the root mount service; returns the exit code.
pub async fn run_helper(args: VolumeHelperArgs) -> i32 {
    // SAFETY: geteuid never fails.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("volume-helper: must run as root");
        return 1;
    }
    if let Some(dir) = args.socket.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::remove_file(&args.socket);
    let listener = match tokio::net::UnixListener::bind(&args.socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("volume-helper: {}: {e}", args.socket.display());
            return 1;
        }
    };
    {
        use std::os::unix::fs::PermissionsExt as _;
        // Anyone may connect; the peer's uid decides what it may do.
        let _ = std::fs::set_permissions(&args.socket, std::fs::Permissions::from_mode(0o666));
    }
    let mounts: Mounts = Arc::default();
    let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("volume-helper: signals: {e}");
            return 1;
        }
    };
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    let m = mounts.clone();
                    let allow = args.allow_uid;
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, allow, m).await {
                            eprintln!("volume-helper: {e}");
                        }
                    });
                }
            }
            _ = term.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    // Unmount everything on the way out, binds first.
    unbind_all();
    let all: Vec<_> = mounts.lock().await.drain().collect();
    for (_, (stop, task)) in all {
        let _ = stop.send(());
        let _ = tokio::time::timeout(Duration::from_secs(15), task).await;
    }
    let _ = std::fs::remove_file(&args.socket);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_targets_are_limited_to_safe_places() {
        let owned_by = |dirs: &'static [(&'static str, u32)]| {
            move |p: &Path| {
                dirs.iter()
                    .find(|(d, _)| Path::new(d) == p)
                    .map(|(_, u)| *u)
            }
        };
        let fs = &[("/", 0), ("/home", 0), ("/home/u", 1000), ("/opt", 0)];
        let own = owned_by(fs);
        // The usual data roots, whoever owns them.
        assert!(may_bind_target(1000, Path::new("/mnt/shared"), own).is_ok());
        assert!(may_bind_target(1000, Path::new("/data/in"), own).is_ok());
        // A place the caller owns, existing or to be created.
        assert!(may_bind_target(1000, Path::new("/home/u/work/shared"), own).is_ok());
        assert!(may_bind_target(1000, Path::new("/home/u"), own).is_ok());
        // Places it does not own, and system directories, are refused.
        assert!(may_bind_target(1000, Path::new("/opt/tools"), own)
            .unwrap_err()
            .contains("owned by"));
        assert!(may_bind_target(1000, Path::new("/home/other"), own).is_err());
        for bad in [
            "/etc",
            "/etc/sudoers.d",
            "/usr/bin",
            "/proc/1",
            "/",
            "/run/cua",
            "/lib/x",
        ] {
            assert!(may_bind_target(0, Path::new(bad), own).is_err(), "{bad}");
        }
        for bad in ["relative", "/mnt/../etc", "/mnt/./x"] {
            assert!(may_bind_target(1000, Path::new(bad), own).is_err(), "{bad}");
        }
        // Root may bind anywhere that is not a system directory.
        assert!(may_bind_target(0, Path::new("/opt/tools"), own).is_ok());
    }

    #[test]
    fn only_the_owner_may_mount_on_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        use std::os::unix::fs::MetadataExt as _;
        let me = std::fs::metadata(dir.path()).unwrap().uid();
        assert!(may_mount(me, dir.path()).is_ok());
        assert!(may_mount(me + 1, dir.path())
            .unwrap_err()
            .contains("owned by"));
        assert!(may_mount(me, Path::new("relative")).is_err());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();
        assert!(may_mount(me, &link)
            .unwrap_err()
            .contains("not a directory"));
        assert!(may_mount(me, &dir.path().join("missing")).is_err());
    }
}
