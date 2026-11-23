// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Acting as another guest user (`run_as` on `FilesystemService` requests
//! and signed URLs).
//!
//! The file operations themselves are the ordinary ones in this module's
//! parent. To run them as another user, the service starts a short-lived
//! helper process per user: `cua-spacesd fs-as --user NAME`, the same
//! binary, which drops to that user (supplementary groups, gid, uid, as
//! `ProcessService` does for `Command.user`) and then serves
//! `FilesystemService` itself on the socket it was started with. The agent
//! forwards the request (with absolute paths already resolved against the
//! user's home) and relays the answer, so a helper's files are created by
//! the real user and every permission check is the kernel's. Uploads and
//! watchers stay in their helper; the agent remembers which user owns an
//! upload or watcher id and routes later calls there.
//!
//! Privilege (see [`crate::privilege`]): when the agent is root, or holds
//! `CAP_SETUID` and `CAP_SETGID` (the Spaces images give their `cua` agent
//! exactly those and `CAP_KILL`), it forks the helper and takes the target
//! user's identity between `fork` and `exec`: no `sudo`, so it works under
//! gVisor (`nosuid`, no new privileges), runc and VMs alike, and the helper
//! itself never holds any privilege. Otherwise the helper is started through
//! `sudo -n` (older images, run as `cua` with passwordless sudo). With none
//! of these, the call fails with `PERMISSION_DENIED` and says why.
//!
//! Unix only; elsewhere `run_as` for another user is `FAILED_PRECONDITION`.

use std::collections::HashMap;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_proto::env::v1::filesystem_service_client::FilesystemServiceClient;
use cua_proto::env::v1::ErrorReason;
use tonic::transport::Channel;
use tonic::{Code, Status};

use crate::context::ServerContext;
use crate::error::status;
#[cfg(unix)]
use crate::error::unsupported;
use crate::process::spawn::{self, UserInfo};

/// Helpers idle this long (and owning no upload or watcher) are stopped.
pub const HELPER_IDLE_TTL: Duration = Duration::from_secs(10 * 60);
/// A route unused for this long is forgotten (longer than an upload's
/// longest lifetime, 24 hours).
pub const ROUTE_TTL: Duration = Duration::from_secs(25 * 60 * 60);
/// The `ServerCapabilities` feature advertising `run_as`.
pub const RUN_AS_FEATURE: &str = "fs.run_as";
/// The feature advertising extended-attribute metadata.
pub const XATTR_FEATURE: &str = "fs.xattr";
/// The feature advertising `POST multipart/form-data` on `PUT` signed URLs.
pub const MULTIPART_FEATURE: &str = "files.multipart";

/// Who a request acts as.
pub enum Acting {
    /// The agent's own identity (no `run_as`, or `run_as` names the user the
    /// agent already is). The user, when named, supplies `~` and the default
    /// directory.
    Own(Option<UserInfo>),
    /// Another user, through that user's helper.
    Other(UserInfo, FilesystemServiceClient<Channel>),
}

struct Helper {
    client: FilesystemServiceClient<Channel>,
    child: tokio::sync::Mutex<tokio::process::Child>,
    last_used: Mutex<Instant>,
}

impl Helper {
    async fn alive(&self) -> bool {
        matches!(self.child.lock().await.try_wait(), Ok(None))
    }

    fn touch(&self) {
        *self.last_used.lock().expect("helper clock") = Instant::now();
    }

    fn idle(&self) -> Duration {
        self.last_used.lock().expect("helper clock").elapsed()
    }
}

/// The helpers of one agent and the upload/watcher routes into them.
pub struct RunAs {
    #[cfg_attr(not(unix), allow(dead_code))]
    ctx: ServerContext,
    helpers: tokio::sync::Mutex<HashMap<String, Arc<Helper>>>,
    /// Upload or watcher id to (user, last used).
    routes: Mutex<HashMap<String, (String, Instant)>>,
}

/// How this agent can switch users.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    /// The agent is root (or the target is the agent's own user): the
    /// helper is started directly, as the target user.
    Root,
    /// The agent holds `CAP_SETUID` and `CAP_SETGID`: the helper is started
    /// directly, as the target user.
    Capabilities,
    /// Through `sudo -n`.
    Sudo,
}

/// The privilege available for switching users, or why there is none.
pub fn privilege() -> Result<Privilege, String> {
    #[cfg(unix)]
    {
        match crate::privilege::switch_mode() {
            crate::privilege::SwitchMode::Root => return Ok(Privilege::Root),
            crate::privilege::SwitchMode::Capabilities => return Ok(Privilege::Capabilities),
            crate::privilege::SwitchMode::None => {}
        }
        let has_sudo = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("sudo").is_file())
        });
        if has_sudo {
            return Ok(Privilege::Sudo);
        }
        Err(format!(
            "the agent runs as {:?}, not root, holds no CAP_SETUID/CAP_SETGID, and this guest has no sudo",
            spawn::current_user_name()
        ))
    }
    #[cfg(not(unix))]
    {
        Err("acting as another user is not supported on this platform".into())
    }
}

impl RunAs {
    /// Creates the (empty) pool.
    pub fn new(ctx: ServerContext) -> Self {
        Self {
            ctx,
            helpers: tokio::sync::Mutex::new(HashMap::new()),
            routes: Mutex::new(HashMap::new()),
        }
    }

    /// Looks `name` up, or fails `INVALID_ARGUMENT`.
    pub fn lookup(name: &str) -> Result<UserInfo, Status> {
        spawn::lookup_user(name).map_err(|e| {
            status(
                Code::InvalidArgument,
                ErrorReason::Unspecified,
                format!("run_as user {name:?}: {e}"),
            )
        })
    }

    fn is_self(&self, user: &UserInfo) -> bool {
        #[cfg(unix)]
        {
            // SAFETY: geteuid has no preconditions.
            user.uid == unsafe { libc::geteuid() } && !self.ctx.config().fs_helper_for_self
        }
        #[cfg(not(unix))]
        {
            let _ = user;
            true
        }
    }

    /// Resolves a request's `run_as`: empty is the agent itself.
    pub async fn acting(&self, run_as: &str) -> Result<Acting, Status> {
        if run_as.is_empty() {
            return Ok(Acting::Own(None));
        }
        let user = Self::lookup(run_as)?;
        if self.is_self(&user) {
            return Ok(Acting::Own(Some(user)));
        }
        let client = self.client_for(&user).await?;
        Ok(Acting::Other(user, client))
    }

    /// Validates that the agent could act as `run_as` (signed URLs check
    /// when minted, not first when used).
    pub fn check(&self, run_as: &str) -> Result<UserInfo, Status> {
        let user = Self::lookup(run_as)?;
        if !self.is_self(&user) {
            if let Err(why) = privilege() {
                return Err(denied(run_as, &why));
            }
        }
        Ok(user)
    }

    /// The helper client of `user` (started on first use).
    pub async fn client_for(
        &self,
        user: &UserInfo,
    ) -> Result<FilesystemServiceClient<Channel>, Status> {
        let mut helpers = self.helpers.lock().await;
        if let Some(helper) = helpers.get(&user.name).cloned() {
            if helper.alive().await {
                helper.touch();
                return Ok(helper.client.clone());
            }
            helpers.remove(&user.name);
            self.routes
                .lock()
                .expect("routes lock")
                .retain(|_, (owner, _)| owner != &user.name);
        }
        let helper = Arc::new(self.start(user).await?);
        let client = helper.client.clone();
        helpers.insert(user.name.clone(), helper);
        Ok(client)
    }

    #[cfg(unix)]
    async fn start(&self, user: &UserInfo) -> Result<Helper, Status> {
        use std::os::fd::OwnedFd;
        use std::process::Stdio;

        // SAFETY: geteuid has no preconditions.
        let privilege = if user.uid == unsafe { libc::geteuid() } {
            // The agent's own user (a helper for it needs no privilege).
            Privilege::Root
        } else {
            privilege().map_err(|why| denied(&user.name, &why))?
        };
        let exe = helper_executable(self.ctx.config().fs_helper_exe.clone()).ok_or_else(|| {
            unsupported(
                RUN_AS_FEATURE,
                "this build is not the cua-spacesd binary and cannot start its file helper",
            )
        })?;
        let (mine, theirs) = std::os::unix::net::UnixStream::pair()
            .map_err(|e| crate::error::internal(format!("socketpair: {e}")))?;
        mine.set_nonblocking(true)
            .map_err(|e| crate::error::internal(format!("socketpair: {e}")))?;
        let mut command = match privilege {
            Privilege::Root | Privilege::Capabilities => {
                // The helper is a child of the agent that takes the target
                // identity before exec: it starts as the user, holds no
                // privilege, and gets none of the agent's environment (the
                // token is in it).
                let mut c = tokio::process::Command::new(&exe);
                c.env_clear()
                    .env("HOME", &user.home)
                    .env("USER", &user.name)
                    .env("LOGNAME", &user.name);
                if let Some(path) = std::env::var_os("PATH") {
                    c.env("PATH", path);
                }
                // SAFETY: geteuid has no preconditions.
                if user.uid != unsafe { libc::geteuid() } {
                    spawn::install_user_switch(&mut c, Some(user.clone()), None, false);
                }
                c
            }
            Privilege::Sudo => {
                let mut c = tokio::process::Command::new("sudo");
                c.arg("-n").arg("--").arg(&exe);
                c
            }
        };
        command
            .arg("fs-as")
            .arg("--user")
            .arg(&user.name)
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|e| denied(&user.name, &format!("cannot start {}: {e}", exe.display())))?;
        // The child has its end of the pair; ours must not hold it open, or a
        // helper that dies at once would never show as a closed socket.
        drop(command);
        let slot = Arc::new(Mutex::new(Some(
            tokio::net::UnixStream::from_std(mine)
                .map_err(|e| crate::error::internal(format!("socketpair: {e}")))?,
        )));
        let connected = tonic::transport::Endpoint::from_static("http://fs-as.invalid")
            .connect_timeout(Duration::from_secs(15))
            .connect_with_connector(tower::service_fn(move |_: http::Uri| {
                let stream = slot.lock().expect("connector slot").take();
                async move {
                    match stream {
                        Some(s) => Ok(hyper_util::rt::TokioIo::new(s)),
                        None => Err(std::io::Error::other("the helper connection is gone")),
                    }
                }
            }))
            .await;
        // An HTTP/2 client connects before the peer has said anything, so a
        // helper that died at once (sudo wants a password, the user has no
        // account) shows only on the first call: probe with a call that
        // answers NOT_FOUND from a live helper.
        let ready = match connected {
            Ok(channel) => {
                let mut client = FilesystemServiceClient::new(channel)
                    .max_decoding_message_size(crate::config::MAX_MESSAGE_BYTES as usize)
                    .max_encoding_message_size(crate::config::MAX_MESSAGE_BYTES as usize);
                let probe = tokio::time::timeout(
                    Duration::from_secs(20),
                    client.get_watcher_events(cua_proto::env::v1::GetWatcherEventsRequest {
                        watcher_id: "ready".into(),
                        max_events: 1,
                    }),
                )
                .await;
                match probe {
                    Ok(Err(status)) if status.code() == Code::NotFound => Ok(client),
                    Ok(Ok(_)) => Ok(client),
                    Ok(Err(status)) => Err(status.message().to_owned()),
                    Err(_) => Err("it did not answer within 20 seconds".to_owned()),
                }
            }
            Err(error) => Err(error.to_string()),
        };
        match ready {
            Ok(client) => Ok(Helper {
                client,
                child: tokio::sync::Mutex::new(child),
                last_used: Mutex::new(Instant::now()),
            }),
            Err(error) => {
                // The helper exited before answering: its stderr says why
                // (sudo wants a password, the user has no shell account...).
                let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                let mut reason = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    use tokio::io::AsyncReadExt as _;
                    let mut buf = vec![0u8; 2048];
                    if let Ok(Ok(n)) =
                        tokio::time::timeout(Duration::from_secs(1), stderr.read(&mut buf)).await
                    {
                        reason = String::from_utf8_lossy(&buf[..n]).trim().to_owned();
                    }
                }
                let why = if reason.is_empty() {
                    format!("the file helper did not start ({error})")
                } else {
                    format!("the file helper did not start: {reason}")
                };
                Err(denied(&user.name, &why))
            }
        }
    }

    #[cfg(not(unix))]
    async fn start(&self, user: &UserInfo) -> Result<Helper, Status> {
        Err(denied(
            &user.name,
            "acting as another user is not supported on this platform",
        ))
    }

    /// Remembers that upload or watcher `id` lives in `user`'s helper.
    pub fn add_route(&self, id: &str, user: &UserInfo) {
        self.routes
            .lock()
            .expect("routes lock")
            .insert(id.to_owned(), (user.name.clone(), Instant::now()));
    }

    /// Forgets `id`.
    pub fn remove_route(&self, id: &str) {
        self.routes.lock().expect("routes lock").remove(id);
    }

    /// The helper an upload or watcher id was created in, if it is routed.
    pub async fn routed(
        &self,
        id: &str,
    ) -> Result<Option<(UserInfo, FilesystemServiceClient<Channel>)>, Status> {
        let name = {
            let mut routes = self.routes.lock().expect("routes lock");
            let Some((name, used)) = routes.get_mut(id) else {
                return Ok(None);
            };
            *used = Instant::now();
            name.clone()
        };
        let user = Self::lookup(&name)?;
        let helpers = self.helpers.lock().await;
        match helpers.get(&name) {
            Some(helper) if helper.alive().await => {
                helper.touch();
                Ok(Some((user, helper.client.clone())))
            }
            _ => {
                drop(helpers);
                self.remove_route(id);
                Err(status(
                    Code::NotFound,
                    ErrorReason::SessionNotFound,
                    format!("{id}: its file helper for {name:?} has stopped"),
                ))
            }
        }
    }

    /// Stops helpers idle past [`HELPER_IDLE_TTL`] that own no upload or
    /// watcher.
    pub async fn sweep(&self) {
        // A route nobody has touched for longer than an upload may live (a
        // watcher abandoned in its helper, an upload that expired there) is
        // forgotten, so it does not keep its helper running.
        let busy: std::collections::HashSet<String> = {
            let mut routes = self.routes.lock().expect("routes lock");
            routes.retain(|_, (_, used)| used.elapsed() < ROUTE_TTL);
            routes.values().map(|(owner, _)| owner.clone()).collect()
        };
        let mut helpers = self.helpers.lock().await;
        let mut stale = Vec::new();
        for (name, helper) in helpers.iter() {
            if !helper.alive().await || (helper.idle() > HELPER_IDLE_TTL && !busy.contains(name)) {
                stale.push(name.clone());
            }
        }
        for name in stale {
            helpers.remove(&name);
            self.routes
                .lock()
                .expect("routes lock")
                .retain(|_, (owner, _)| owner != &name);
        }
    }
}

/// `PERMISSION_DENIED` naming the user and the missing privilege.
fn denied(user: &str, why: &str) -> Status {
    status(
        Code::PermissionDenied,
        ErrorReason::PermissionDenied,
        format!("cannot act as {user:?}: {why}"),
    )
}

/// The cua-spacesd binary to run as the helper: the configured override, or
/// this executable when it is cua-spacesd (a test harness or an embedding
/// binary never re-runs itself).
#[cfg(unix)]
fn helper_executable(configured: Option<PathBuf>) -> Option<PathBuf> {
    if configured.is_some() {
        return configured;
    }
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_name()?.to_str()?;
    matches!(name, "cua-spacesd" | "cua-env-driver" | "cua-guestd").then_some(exe)
}

/// Drops this process to `user`: supplementary groups, gid, then uid. As
/// root it can switch to anyone; otherwise only to itself.
#[cfg(unix)]
pub fn drop_to(user: &UserInfo) -> std::io::Result<()> {
    use std::ffi::CString;
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        if euid == user.uid {
            return Ok(());
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "not root: cannot switch users",
        ));
    }
    let name = CString::new(user.name.clone())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: plain libc calls on valid arguments.
    unsafe {
        if libc::initgroups(name.as_ptr(), user.gid as _) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::setgid(user.gid) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::setuid(user.uid) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // A privileged process must not be able to climb back.
        if user.uid != 0 && libc::setuid(0) == 0 {
            return Err(std::io::Error::other("could still become root"));
        }
    }
    Ok(())
}

/// The `cua-spacesd fs-as --user NAME` helper: drops to `user`, then serves
/// `FilesystemService` (native gRPC over HTTP/2) on the socket it inherited
/// as stdin until the agent closes it.
#[cfg(unix)]
pub fn run_helper_main(user: &str) -> Result<(), String> {
    use std::os::fd::FromRawFd as _;

    let info = spawn::lookup_user(user).map_err(|e| format!("user {user:?}: {e}"))?;
    // SAFETY: fd 0 is the socket the agent started this process with; this
    // is the only owner of it.
    let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(0) };
    stream
        .set_nonblocking(true)
        .map_err(|e| format!("stdin is not a socket: {e}"))?;
    drop_to(&info).map_err(|e| format!("cannot switch to {user:?}: {e}"))?;
    std::env::set_var("HOME", &info.home);
    std::env::set_var("USER", &info.name);
    std::env::set_var("LOGNAME", &info.name);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    runtime.block_on(async move {
        use hyper_util::rt::{TokioExecutor, TokioIo};
        use hyper_util::server::conn::auto;
        use hyper_util::service::TowerToHyperService;
        use tower::ServiceExt as _;

        let stream =
            tokio::net::UnixStream::from_std(stream).map_err(|e| format!("socket: {e}"))?;
        let ctx = ServerContext::new(crate::config::ServerConfig::default(), None);
        let state = super::FsState::new(ctx);
        let app =
            tonic::service::Routes::new(super::FilesystemServiceImpl::new(state).into_server())
                .prepare()
                .into_axum_router();
        let service = TowerToHyperService::new(app.map_request(
            |req: http::Request<hyper::body::Incoming>| req.map(axum::body::Body::new),
        ));
        let mut builder = auto::Builder::new(TokioExecutor::new());
        builder
            .http2()
            .max_frame_size(1024 * 1024)
            .initial_stream_window_size(4 * 1024 * 1024)
            .initial_connection_window_size(16 * 1024 * 1024);
        builder
            .http2_only()
            .serve_connection(TokioIo::new(stream), service)
            .await
            .map_err(|e| e.to_string())
    })
}
