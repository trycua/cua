// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `VolumeService` and its ticket-authenticated `/volume` socket: the guest
//! mount of Cua Volume, served by the client.
//!
//! `AttachVolume` binds a loopback listener and mints a ticket. The client
//! opens `/volume` and keeps it open; the listener's connections travel over
//! it as tunnel streams to the target `nfs` (macOS) or `fs` (Linux), which
//! the client answers from the Space's view of the volume. Once the client
//! is attached, the volume is mounted:
//!
//! - macOS: the system NFS client, `mount_nfs 127.0.0.1:/` on the listener's
//!   port, as the session user.
//! - Linux: FUSE over the file operations protocol, mounted as root (gVisor
//!   refuses an unprivileged `fusermount3` and ignores setuid) and shared
//!   with the desktop user (`allow_other`). Through the image's root
//!   `cua-spacesd volume-helper` service when spacesd is not root, else a
//!   `cua-spacesd volume-mount` child (root, or `sudo -n`).
//!
//! - Windows (a preview, off unless `CUA_VOLUME_WINDOWS_PREVIEW=1`): the
//!   built-in NFS client (Client for NFS), `mount -o anon` on a drive
//!   letter (`V:` by default), found through a loopback portmapper; see
//!   [`super::volume_windows`].
//!
//! The mount goes away when the socket closes, on `DetachVolume`, on
//! session revocation and at shutdown. A guest without the backend (no
//! `/dev/fuse`, no `mount_nfs`, Windows without the preview) reports
//! `volume.mount` as unsupported and refuses `AttachVolume`; the Space
//! works without it.
//!
//! Adding a backend: return its name from
//! [`backend`], mount and unmount it in [`mount`] and [`unmount`], and
//! connect it to the loopback listener, whose streams name the backend as
//! their target. The client side answers targets in
//! `cua_volume::guest::GuestServers::target`; nothing else in the protocol
//! changes, and `AttachVolumeResponse.backend` tells the client which one
//! the guest uses.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use cua_proto::env::v1::volume_service_server::{VolumeService, VolumeServiceServer};
use cua_proto::env::v1::*;
use cua_spacesd_socks::Hub;
use futures_util::{SinkExt, StreamExt};
use tokio_util::sync::CancellationToken;
use tonic::{Code, Request, Response as GrpcResponse, Status};

use crate::auth::{caller, TicketScope};
use crate::context::ServerContext;
use crate::error::{session_not_found, status};
use crate::services::tunnel::{refuse, ticket_path, with_protocol};
use crate::util::duration;

/// Default ticket lifetime.
pub const VOLUME_DEFAULT_TTL: Duration = Duration::from_secs(60);
/// The capability feature.
pub const FEATURE: &str = "volume.mount";
/// How long a mount may take before it is reported as failed.
const MOUNT_TIMEOUT: Duration = Duration::from_secs(20);

/// The mount backend of this guest, or why there is none.
pub fn backend() -> Result<&'static str, String> {
    if cfg!(target_os = "macos") {
        if Path::new("/sbin/mount_nfs").exists() {
            Ok("nfs")
        } else {
            Err("this guest has no /sbin/mount_nfs".into())
        }
    } else if cfg!(target_os = "linux") {
        if !Path::new("/dev/fuse").exists() {
            return Err(
                "this guest has no /dev/fuse (a container needs gVisor with SYS_ADMIN, or a VM)"
                    .into(),
            );
        }
        // SAFETY: geteuid never fails.
        #[cfg(unix)]
        let root = unsafe { libc::geteuid() } == 0;
        #[cfg(not(unix))]
        let root = false;
        if !root && !Path::new(HELPER_SOCKET).exists() && which("sudo").is_none() {
            return Err(
                "the FUSE mount needs root: this guest has neither the volume helper nor sudo"
                    .into(),
            );
        }
        Ok("fs")
    } else if cfg!(windows) {
        super::volume_windows::backend().map(|()| "nfs")
    } else {
        Err("the Cua Volume mount is not supported on this system".into())
    }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// Where the volume mounts by default.
pub fn default_mount_path() -> PathBuf {
    if cfg!(windows) {
        return super::volume_windows::default_mount_path();
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    if cfg!(target_os = "linux") {
        let v = Path::new("/volume");
        if v.is_dir() && writable(v) {
            return v.to_path_buf();
        }
    }
    home.join("Cua Volume")
}

fn writable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = CString::new(p.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: a valid NUL-terminated path.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        false
    }
}

#[derive(Clone, Debug)]
struct Phase {
    state: VolumeState,
    detail: String,
}

struct Session {
    id: String,
    hub: Hub,
    port: u16,
    mount_path: PathBuf,
    backend: &'static str,
    stop: CancellationToken,
    phase: Mutex<Phase>,
    helper: Mutex<Option<tokio::process::Child>>,
    /// Mounted through the root volume helper (Linux).
    via_helper: std::sync::atomic::AtomicBool,
    /// The Windows mount (its drive, folder link and portmapper).
    windows: Mutex<Option<super::volume_windows::WinMount>>,
}

impl Session {
    fn set(&self, state: VolumeState, detail: impl Into<String>) {
        *self.phase.lock().expect("phase") = Phase {
            state,
            detail: detail.into(),
        };
    }
}

/// State shared by the gRPC service and the `/volume` route.
#[derive(Clone)]
pub struct VolumeShared {
    ctx: ServerContext,
    current: Arc<Mutex<Option<Arc<Session>>>>,
    /// Held while a detach unmounts, so shutdown can wait for it.
    unmounting: Arc<tokio::sync::Mutex<()>>,
}

impl VolumeShared {
    /// Creates empty state; revoked sessions and shutdown unmount.
    pub fn new(ctx: ServerContext) -> Self {
        let state = Self {
            ctx,
            current: Arc::default(),
            unmounting: Arc::default(),
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let mut revoked = state.ctx.session_revocations();
            let shutdown = state.ctx.shutdown_token();
            let s = state.clone();
            handle.spawn(async move {
                loop {
                    tokio::select! {
                        changed = revoked.changed() => {
                            if changed.is_err() { return }
                            s.detach(None).await;
                        }
                        _ = shutdown.cancelled() => {
                            s.detach(None).await;
                            return;
                        }
                    }
                }
            });
        }
        state
    }

    /// Unmounts and forgets the session (`id` must match when given).
    /// Returns whether one was attached.
    async fn detach(&self, id: Option<&str>) -> bool {
        let _unmounting = self.unmounting.lock().await;
        let session = {
            let mut slot = self.current.lock().expect("volume");
            match slot.as_ref() {
                Some(s) if id.is_none_or(|i| i == s.id) => slot.take(),
                _ => None,
            }
        };
        let Some(session) = session else {
            return false;
        };
        // Unmount while the client still serves the mount (a clean flush),
        // then close its socket. A client that is already gone gets a forced
        // unmount at once instead of waiting out the NFS timeouts.
        unmount(&session, session.hub.is_connected()).await;
        session.stop.cancel();
        tracing::info!(volume = %session.id, "volume detached");
        true
    }

    /// Shutdown: unmounts whatever is mounted and waits for an unmount
    /// already under way (bounded, so a stuck umount cannot hold the
    /// process).
    pub async fn finish(&self) {
        let _ = tokio::time::timeout(SHUTDOWN_UNMOUNT_TIMEOUT, self.detach(None)).await;
    }

    /// Start-up: unmounts a mount a previous daemon left at the volume path
    /// (it exited without unmounting; nothing serves it, so every access
    /// hangs until the NFS client gives up). Never blocks start-up for long.
    pub async fn clear_stale_mount(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(30), clear_stale_mounts()).await;
    }
}

/// How long shutdown waits for the volume to unmount.
const SHUTDOWN_UNMOUNT_TIMEOUT: Duration = Duration::from_secs(25);

/// The paths a volume may have been mounted at.
fn stale_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        return vec![];
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let mut v = vec![home.join("Cua Volume")];
    if cfg!(target_os = "linux") {
        v.push(PathBuf::from("/volume"));
    }
    v
}

/// Whether `table` (macOS `mount` output or Linux `/proc/self/mountinfo`)
/// lists `path` as a mount point. Compares text only: stat-ing a dead mount
/// is what hangs.
fn listed_in_mount_table(table: &str, path: &Path, linux: bool) -> bool {
    let raw = path.to_string_lossy().to_string();
    // macOS lists /var, /tmp and /etc as /private/...
    let private = format!("/private{raw}");
    table.lines().any(|l| {
        if linux {
            l.split(' ')
                .nth(4)
                .is_some_and(|m| m.replace("\\040", " ") == raw)
        } else {
            [&raw, &private]
                .iter()
                .any(|p| l.contains(&format!(" on {p} (")))
        }
    })
}

async fn mount_table() -> String {
    if cfg!(target_os = "linux") {
        tokio::fs::read_to_string("/proc/self/mountinfo")
            .await
            .unwrap_or_default()
    } else {
        // `mount` only reads the kernel's table; it does not stat mounts.
        match tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new("/sbin/mount").output(),
        )
        .await
        {
            Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).into_owned(),
            _ => String::new(),
        }
    }
}

async fn clear_stale_mounts() {
    let table = mount_table().await;
    for path in stale_candidates() {
        if !listed_in_mount_table(&table, &path, cfg!(target_os = "linux")) {
            continue;
        }
        // Only a mount that does not answer is stale: a host's own live
        // mount at the same path (a machine that is both a host and a Space)
        // is left alone. A dead mount's stat hangs until the NFS timeouts
        // give up, so it runs on a thread with a deadline.
        let probe = path.clone();
        let alive = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || std::fs::metadata(&probe).is_ok()),
        )
        .await;
        if matches!(alive, Ok(Ok(true))) {
            continue;
        }
        let p = path.to_string_lossy().to_string();
        tracing::warn!(path = %p, "clearing a stale volume mount left by a previous daemon");
        let r = if cfg!(target_os = "linux") {
            // Lazy: a dead FUSE/NFS mount may refuse a plain unmount.
            // SAFETY: geteuid never fails.
            #[cfg(unix)]
            let root = unsafe { libc::geteuid() } == 0;
            #[cfg(not(unix))]
            let root = false;
            if root {
                run("umount", &["-l", &p]).await
            } else {
                run("sudo", &["-n", "umount", "-l", &p]).await
            }
        } else {
            run("/sbin/umount", &["-f", &p]).await
        };
        if let Err(e) = r {
            tracing::warn!(path = %p, error = %e, "stale volume mount did not unmount");
        }
    }
}

async fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = tokio::time::timeout(
        MOUNT_TIMEOUT,
        tokio::process::Command::new(cmd).args(args).output(),
    )
    .await
    .map_err(|_| format!("{cmd} timed out"))?
    .map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Whether `path` is a mount point now (compared canonically: macOS lists
/// `/var/...` as `/private/var/...`).
pub fn is_mounted(path: &Path) -> bool {
    if cfg!(windows) {
        return super::volume_windows::is_mounted(path);
    }
    let want = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string();
    if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/self/mountinfo")
            .map(|info| {
                info.lines().any(|l| {
                    l.split(' ')
                        .nth(4)
                        .is_some_and(|m| m.replace("\\040", " ") == want)
                })
            })
            .unwrap_or(false)
    } else {
        std::process::Command::new("/sbin/mount")
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.contains(&format!(" on {want} (")))
            })
            .unwrap_or(false)
    }
}

/// [`is_mounted`] on a blocking thread. It stats the mount point and runs
/// `/sbin/mount`, which wait on the mount's server; when the client serves
/// the mount from this same process (the doctor, the tests), blocking a
/// runtime worker can hold the very task that would answer, until the NFS
/// client's timeout (seconds per call) gives up.
pub async fn is_mounted_async(path: &Path) -> bool {
    let p = path.to_path_buf();
    tokio::task::spawn_blocking(move || is_mounted(&p))
        .await
        .unwrap_or(false)
}

async fn mount(session: &Arc<Session>) -> Result<(), String> {
    let path = &session.mount_path;
    if cfg!(windows) {
        let m = super::volume_windows::mount(session.port, path, &session.id)
            .await
            .map_err(|e| {
                let st = session.hub.stats();
                format!(
                    "{e}; relay to the client: {} open streams, {} bytes out, {} bytes in",
                    st.active_connections.load(Ordering::SeqCst),
                    st.bytes_out.load(Ordering::Relaxed),
                    st.bytes_in.load(Ordering::Relaxed)
                )
            })?;
        *session.windows.lock().expect("windows") = Some(m);
        return Ok(());
    }
    std::fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let p = path.to_string_lossy().to_string();
    match session.backend {
        "nfs" => {
            if is_mounted_async(path).await {
                let _ = run("/sbin/umount", &["-f", &p]).await;
            }
            let opts = format!(
                "port={port},mountport={port},vers=3,tcp,nolocks,locallocks,soft,timeo=50,retrans=2,deadtimeout=30,intr,noowners,rsize=1048576,wsize=1048576,actimeo=1",
                port = session.port
            );
            run("/sbin/mount_nfs", &["-o", &opts, "127.0.0.1:/", &p]).await
        }
        _ => {
            // SAFETY: geteuid never fails.
            #[cfg(unix)]
            let root = unsafe { libc::geteuid() } == 0;
            #[cfg(not(unix))]
            let root = false;
            if !root && Path::new(HELPER_SOCKET).exists() {
                helper_request("mount", session.port, path).await?;
                session.via_helper.store(true, Ordering::SeqCst);
                return Ok(());
            }
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let exe = exe.to_string_lossy().to_string();
            let port = session.port.to_string();
            let mut cmd = if root {
                tokio::process::Command::new(&exe)
            } else {
                let mut c = tokio::process::Command::new("sudo");
                c.args(["-n", &exe]);
                c
            };
            cmd.args(["volume-mount", "--port", &port, "--path", &p])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            let mut child = cmd.spawn().map_err(|e| format!("volume-mount: {e}"))?;
            let deadline = tokio::time::Instant::now() + MOUNT_TIMEOUT;
            loop {
                if is_mounted_async(path).await {
                    break;
                }
                if let Ok(Some(status)) = child.try_wait() {
                    let mut err = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        use tokio::io::AsyncReadExt as _;
                        let _ = e.read_to_string(&mut err).await;
                    }
                    return Err(format!("volume-mount exited ({status}): {}", err.trim()));
                }
                if tokio::time::Instant::now() > deadline {
                    let _ = child.start_kill();
                    return Err("volume-mount did not mount in time".into());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            *session.helper.lock().expect("helper") = Some(child);
            Ok(())
        }
    }
}

async fn unmount(session: &Arc<Session>, client_attached: bool) {
    let path = &session.mount_path;
    if cfg!(windows) {
        let m = session.windows.lock().expect("windows").take();
        match m {
            Some(m) => super::volume_windows::unmount(m, client_attached).await,
            None => super::volume_windows::unmount_path(path).await,
        }
        return;
    }
    let p = path.to_string_lossy().to_string();
    match session.backend {
        "nfs" => {
            if is_mounted_async(path).await
                && (!client_attached || run("/sbin/umount", &[&p]).await.is_err())
            {
                let _ = run("/sbin/umount", &["-f", &p]).await;
            }
        }
        _ if session.via_helper.load(Ordering::SeqCst) => {
            if let Err(e) = helper_request("unmount", 0, path).await {
                tracing::warn!(error = %e, "volume helper unmount");
            }
        }
        _ => {
            let child = session.helper.lock().expect("helper").take();
            if let Some(mut child) = child {
                // The helper unmounts on SIGTERM (sudo passes it on).
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    // SAFETY: signalling our own child.
                    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                }
                if tokio::time::timeout(Duration::from_secs(10), child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.start_kill();
                }
            }
            if is_mounted_async(path).await {
                let lazy = ["-n", "umount", "-l", &p];
                let _ = run("sudo", &lazy).await;
            }
        }
    }
    // A home-folder mount point goes away with the mount; /volume stays.
    if path != Path::new("/volume") {
        let _ = std::fs::remove_dir(path);
    }
}

/// Where the root volume helper listens (Linux images run it from their
/// init; see `cua-spacesd volume-helper`).
pub const HELPER_SOCKET: &str = "/run/cua/volume.sock";

/// One request to the root volume helper.
async fn helper_request(op: &str, port: u16, path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
        let stream = tokio::net::UnixStream::connect(HELPER_SOCKET)
            .await
            .map_err(|e| format!("the volume helper ({HELPER_SOCKET}): {e}"))?;
        let (read, mut write) = stream.into_split();
        let mut req = serde_json::to_vec(&serde_json::json!({
            "op": op, "port": port, "path": path,
        }))
        .map_err(|e| e.to_string())?;
        req.push(b'\n');
        write.write_all(&req).await.map_err(|e| e.to_string())?;
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(30),
            tokio::io::BufReader::new(read).read_line(&mut line),
        )
        .await
        .map_err(|_| "the volume helper did not answer".to_string())?
        .map_err(|e| e.to_string())?;
        let reply: serde_json::Value =
            serde_json::from_str(&line).map_err(|e| format!("the volume helper: {e}"))?;
        if reply["ok"].as_bool() == Some(true) {
            Ok(())
        } else {
            Err(format!(
                "the volume helper: {}",
                reply["error"].as_str().unwrap_or("refused")
            ))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (op, port, path);
        Err("no volume helper on this platform".into())
    }
}

/// `VolumeService` implementation.
pub struct VolumeServiceImpl {
    state: VolumeShared,
}

impl VolumeServiceImpl {
    /// Wraps shared state.
    pub fn new(state: VolumeShared) -> Self {
        Self { state }
    }

    /// The tonic server.
    pub fn into_server(self) -> VolumeServiceServer<Self> {
        VolumeServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl VolumeService for VolumeServiceImpl {
    async fn attach_volume(
        &self,
        request: Request<AttachVolumeRequest>,
    ) -> Result<GrpcResponse<AttachVolumeResponse>, Status> {
        let principal = caller(&request).principal;
        let body = request.into_inner();
        let backend = backend()
            .map_err(|reason| status(Code::FailedPrecondition, ErrorReason::Unspecified, reason))?;
        let mut mount_path = if body.mount_path.is_empty() {
            default_mount_path()
        } else {
            PathBuf::from(&body.mount_path)
        };
        if cfg!(windows) {
            // `V:` is drive-relative; the volume is the drive's root.
            mount_path = super::volume_windows::normalize(mount_path);
        }
        if !mount_path.is_absolute() {
            return Err(crate::error::invalid("mount_path must be absolute"));
        }
        self.state.detach(None).await;
        // Windows: the NFS port where a client that skips the portmapper
        // looks, when it is free.
        let preferred = if cfg!(windows) {
            tokio::net::TcpListener::bind(("127.0.0.1", 2049))
                .await
                .ok()
        } else {
            None
        };
        let listener = match preferred {
            Some(l) => l,
            None => tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| crate::error::internal(format!("volume listener: {e}")))?,
        };
        let port = listener
            .local_addr()
            .map_err(|e| crate::error::internal(e.to_string()))?
            .port();
        let id = format!("vol-{}", crate::util::random_id(9));
        let hub = Hub::new();
        let stop = CancellationToken::new();
        {
            let (hub, stop) = (hub.clone(), stop.clone());
            tokio::spawn(async move {
                tokio::select! {
                    r = cua_spacesd_socks::serve_forward(listener, hub, backend.to_string(), 0) => {
                        if let Err(error) = r {
                            tracing::warn!(%error, "volume listener failed");
                        }
                    }
                    _ = stop.cancelled() => {}
                }
            });
        }
        let ttl = duration(body.ticket_ttl.as_ref())
            .filter(|d| !d.is_zero())
            .unwrap_or(VOLUME_DEFAULT_TTL);
        let (ticket, _) =
            self.state
                .ctx
                .mint_ticket(TicketScope::Volume, &id, principal.as_ref(), ttl);
        *self.state.current.lock().expect("volume") = Some(Arc::new(Session {
            id: id.clone(),
            hub,
            port,
            mount_path: mount_path.clone(),
            backend,
            stop,
            phase: Mutex::new(Phase {
                state: VolumeState::WaitingForClient,
                detail: String::new(),
            }),
            helper: Mutex::new(None),
            via_helper: std::sync::atomic::AtomicBool::new(false),
            windows: Mutex::new(None),
        }));
        Ok(GrpcResponse::new(AttachVolumeResponse {
            volume_id: id,
            ws_path: ticket_path(cua_proto::metadata::VOLUME_WS_PATH, &ticket),
            ticket,
            mount_path: mount_path.to_string_lossy().into_owned(),
            backend: backend.into(),
        }))
    }

    async fn detach_volume(
        &self,
        request: Request<DetachVolumeRequest>,
    ) -> Result<GrpcResponse<DetachVolumeResponse>, Status> {
        let id = request.into_inner().volume_id;
        let id = (!id.is_empty()).then_some(id);
        if !self.state.detach(id.as_deref()).await && id.is_some() {
            return Err(session_not_found("volume", id.as_deref().unwrap_or("")));
        }
        Ok(GrpcResponse::new(DetachVolumeResponse {}))
    }

    async fn get_volume_status(
        &self,
        _request: Request<GetVolumeStatusRequest>,
    ) -> Result<GrpcResponse<GetVolumeStatusResponse>, Status> {
        let session = self.state.current.lock().expect("volume").clone();
        let Some(s) = session else {
            return Ok(GrpcResponse::new(GetVolumeStatusResponse {
                state: VolumeState::Detached as i32,
                backend: backend().unwrap_or("").into(),
                ..Default::default()
            }));
        };
        let phase = s.phase.lock().expect("phase").clone();
        let stats = s.hub.stats();
        Ok(GrpcResponse::new(GetVolumeStatusResponse {
            state: phase.state as i32,
            volume_id: s.id.clone(),
            mount_path: s.mount_path.to_string_lossy().into_owned(),
            backend: s.backend.into(),
            detail: phase.detail,
            active_streams: stats.active_connections.load(Ordering::SeqCst),
            bytes_out: stats.bytes_out.load(Ordering::Relaxed),
            bytes_in: stats.bytes_in.load(Ordering::Relaxed),
        }))
    }
}

/// `GET /volume?ticket=...`: the client attaches; the guest mounts.
pub async fn volume_ws(
    State(state): State<VolumeShared>,
    uri: Uri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let (claims, subprotocol) =
        match state
            .ctx
            .validate_request_ticket(&uri, &headers, TicketScope::Volume)
        {
            Ok(v) => v,
            Err(e) => return refuse(e),
        };
    let session = state.current.lock().expect("volume").clone();
    let Some(session) = session.filter(|s| s.id == claims.resource) else {
        return (StatusCode::GONE, "volume detached").into_response();
    };
    if session.hub.is_connected() {
        return (StatusCode::CONFLICT, "a client is already attached").into_response();
    }
    with_protocol(upgrade, subprotocol)
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let (ws_tx, ws_rx) = socket.split();
            let source = ws_rx
                .take_while(|m| {
                    futures_util::future::ready(!matches!(m, Err(_) | Ok(Message::Close(_))))
                })
                .filter_map(|m| async move {
                    match m {
                        Ok(Message::Binary(data)) => Some(data.to_vec()),
                        _ => None,
                    }
                });
            let sink = ws_tx.with(|data: Vec<u8>| async move {
                Ok::<_, axum::Error>(Message::Binary(data.into()))
            });
            let (source, sink) = (Box::pin(source), Box::pin(sink));
            // Mount once the client is attached (the mount dials through it).
            let mounter = {
                let s = session.clone();
                tokio::spawn(async move {
                    for _ in 0..100 {
                        if s.hub.is_connected() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    s.set(VolumeState::Mounting, "");
                    match mount(&s).await {
                        Ok(()) => {
                            s.set(VolumeState::Mounted, "");
                            tracing::info!(volume = %s.id, path = %s.mount_path.display(), backend = s.backend, "volume mounted");
                        }
                        Err(e) => {
                            tracing::warn!(volume = %s.id, error = %e, "volume mount failed");
                            s.set(VolumeState::Error, e);
                        }
                    }
                })
            };
            tokio::select! {
                _ = session.hub.run_peer(source, sink) => {}
                _ = session.stop.cancelled() => {}
            }
            mounter.abort();
            // The client left: nothing serves the mount any more.
            state.detach(Some(&session.id)).await;
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_mount_is_found_without_stat() {
        let mac = "127.0.0.1:/ on /Users/a/Cua Volume (nfs, nodev, nosuid, mounted by a)\n\
                   /dev/disk3s1 on / (apfs, local)";
        let p = Path::new("/Users/a/Cua Volume");
        assert!(listed_in_mount_table(mac, p, false));
        assert!(!listed_in_mount_table(
            mac,
            Path::new("/Users/b/Cua Volume"),
            false
        ));
        let linux =
            "36 25 0:32 / /volume rw - fuse none rw\n40 25 0:33 / /mnt/my\\040vol rw - tmpfs";
        assert!(listed_in_mount_table(linux, Path::new("/volume"), true));
        assert!(listed_in_mount_table(linux, Path::new("/mnt/my vol"), true));
        assert!(!listed_in_mount_table(linux, Path::new("/vol"), true));
    }

    #[test]
    fn the_backend_and_path_follow_the_guest() {
        let b = backend();
        if cfg!(target_os = "macos") {
            assert_eq!(b, Ok("nfs"));
            assert!(default_mount_path().ends_with("Cua Volume"));
        } else if cfg!(windows) {
            // Off unless the preview is on; then Client for NFS may or may
            // not be installed on this machine.
            if let Err(e) = b {
                assert!(
                    e.contains("coming soon") || e.contains("Client for NFS"),
                    "{e}"
                );
            }
            let d = default_mount_path();
            assert!(
                super::super::volume_windows::drive_of(&d).is_some(),
                "{d:?}"
            );
        }
    }
}
