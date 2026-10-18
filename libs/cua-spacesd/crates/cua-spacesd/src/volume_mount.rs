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
    /// `mount` or `unmount`.
    pub op: String,
    #[serde(default)]
    pub port: u16,
    pub path: PathBuf,
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
            "unmount" => {
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
    // Unmount everything on the way out.
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
