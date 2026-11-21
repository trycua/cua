// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Server end of the daemon's local socket or named pipe.
//!
//! [`LocalIpcListener`] binds the endpoint and accepts only clients that run
//! as the daemon's own user, each with its [`PeerIdentity`].
//! [`serve_connections`] runs one task per accepted connection and, once its
//! shutdown future resolves, aborts and joins them all before it returns.
//! The request protocol belongs to the caller's handler.

use std::future::Future;

use super::LocalIpcStream;

/// Who is on the other end of an accepted connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIdentity {
    /// A Unix socket peer, from `SO_PEERCRED` or `LOCAL_PEERCRED`.
    Unix { uid: u32, pid: Option<u32> },
    /// A named-pipe client, from its process token.
    Windows { sid: String, pid: u32 },
}

impl PeerIdentity {
    /// The peer's process id, when the OS reports one.
    pub fn pid(&self) -> Option<u32> {
        match self {
            Self::Unix { pid, .. } => *pid,
            Self::Windows { pid, .. } => Some(*pid),
        }
    }
}

/// Run `handler` on every connection `listener` accepts until `shutdown`
/// resolves or accepting fails for good. Either way every connection task is
/// aborted and joined before this returns, so none outlives the caller's
/// runtime state.
pub async fn serve_connections<H, F>(
    listener: &mut LocalIpcListener,
    mut handler: H,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()>
where
    H: FnMut(LocalIpcStream, PeerIdentity) -> F,
    F: Future<Output = ()> + Send + 'static,
{
    let mut connections = tokio::task::JoinSet::new();
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    reap_finished(&mut connections);
                    connections.spawn(handler(stream, peer));
                }
                Err(error) => break Err(error),
            },
            () = &mut shutdown => break Ok(()),
        }
    };
    connections.shutdown().await;
    result
}

/// Drop the connection tasks that already ended, so a long-lived daemon does
/// not hold one entry per connection it ever served.
fn reap_finished(connections: &mut tokio::task::JoinSet<()>) {
    while let Some(result) = connections.try_join_next() {
        if let Err(error) = result {
            if error.is_panic() {
                tracing::warn!("service connection task panicked: {error}");
            }
        }
    }
}

#[cfg(unix)]
pub use self::unix::LocalIpcListener;
#[cfg(windows)]
pub use self::windows::LocalIpcListener;

#[cfg(unix)]
mod unix {
    use super::PeerIdentity;
    use crate::ipc::LocalIpcStream;

    /// Which file the listener bound, so cleanup leaves a replacement alone.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) struct SocketIdentity {
        device: u64,
        inode: u64,
    }

    pub(super) fn socket_identity(path: &str) -> std::io::Result<SocketIdentity> {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(SocketIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    /// Restrict the socket to its owner.
    pub(super) fn secure_socket(path: &str) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| anyhow::anyhow!("secure local daemon socket {path}: {e}"))
    }

    pub(super) fn remove_owned_socket(path: &str, identity: SocketIdentity) {
        if socket_identity(path).is_ok_and(|current| current == identity) {
            let _ = std::fs::remove_file(path);
        }
    }

    pub(super) fn authorize_uid(expected: u32, actual: u32) -> anyhow::Result<()> {
        if actual != expected {
            anyhow::bail!("reject Unix peer uid {actual} for runtime owned by uid {expected}");
        }
        Ok(())
    }

    pub(super) fn peer_identity(stream: &tokio::net::UnixStream) -> std::io::Result<PeerIdentity> {
        let credentials = stream.peer_cred()?;
        Ok(PeerIdentity::Unix {
            uid: credentials.uid(),
            pid: credentials
                .pid()
                .and_then(|pid| u32::try_from(pid).ok())
                .filter(|pid| *pid != 0),
        })
    }

    /// A bound Unix socket that accepts same-user peers.
    pub struct LocalIpcListener {
        listener: tokio::net::UnixListener,
        path: String,
        bound: SocketIdentity,
    }

    impl LocalIpcListener {
        /// Bind `path`, creating its directory, and restrict it to the
        /// current user. A file already at `path` fails the bind; clearing a
        /// stale socket is the caller's call. `exclusive_name` has no effect
        /// on Unix, where a bound path is always exclusive.
        pub fn bind(path: &str, exclusive_name: bool) -> anyhow::Result<Self> {
            let _ = exclusive_name;
            if let Some(dir) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(dir)?;
            }
            let listener = tokio::net::UnixListener::bind(path)
                .map_err(|e| anyhow::anyhow!("bind {path}: {e}"))?;
            secure_socket(path)?;
            let bound = socket_identity(path)?;
            Ok(Self {
                listener,
                path: path.to_owned(),
                bound,
            })
        }

        /// The next same-user connection. A peer whose credentials are
        /// unreadable or name another user is closed before anything is read
        /// from it. Cancel-safe.
        pub async fn accept(&mut self) -> anyhow::Result<(LocalIpcStream, PeerIdentity)> {
            // SAFETY: geteuid has no preconditions.
            let expected_uid = unsafe { libc::geteuid() };
            loop {
                let (stream, _) = self.listener.accept().await?;
                let peer = match peer_identity(&stream) {
                    Ok(peer) => peer,
                    Err(error) => {
                        tracing::warn!(
                            "service connection rejected before request parsing: read Unix peer credentials: {error}"
                        );
                        continue;
                    }
                };
                let PeerIdentity::Unix { uid, .. } = peer else {
                    unreachable!("a Unix socket peer has Unix credentials");
                };
                if let Err(error) = authorize_uid(expected_uid, uid) {
                    tracing::warn!("service connection rejected before request parsing: {error}");
                    continue;
                }
                return Ok((LocalIpcStream::from_unix(stream), peer));
            }
        }

        /// Stop listening and remove the socket, unless another daemon has
        /// replaced it since.
        pub fn close(self) {
            drop(self.listener);
            remove_owned_socket(&self.path, self.bound);
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::os::windows::io::AsRawHandle as _;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use windows::Win32::Foundation::HANDLE;

    use super::PeerIdentity;
    use crate::ipc::windows::{pipe_client_identity, sid_is_authorized, PipeSecurity};
    use crate::ipc::LocalIpcStream;

    /// Pause between attempts to replace a pipe instance that failed.
    const INSTANCE_RETRY_DELAY: Duration = Duration::from_millis(500);

    /// Consecutive instance failures (about one minute of retries) after
    /// which the listener gives up, so a supervisor can restart the daemon
    /// instead of it keeping a live PID behind an unreachable pipe.
    const INSTANCE_FAILURE_LIMIT: u32 = 120;

    /// A named pipe that accepts clients running as the current user.
    ///
    /// Each connection takes the waiting instance, and the next accept
    /// creates another. A failure to create or connect a later instance does
    /// not end the listener: connections being served keep their instances,
    /// and the next one usually succeeds.
    pub struct LocalIpcListener {
        path: String,
        security: PipeSecurity,
        waiting: Option<NamedPipeServer>,
        consecutive_failures: u32,
    }

    impl LocalIpcListener {
        /// Create the first instance of `path` under a current-user DACL.
        /// With `exclusive_name`, the name must not exist yet. Failing to
        /// build the DACL fails closed: an open ACL would expose the
        /// endpoint to every local principal.
        pub fn bind(path: &str, exclusive_name: bool) -> anyhow::Result<Self> {
            let security = PipeSecurity::current_user().map_err(|error| {
                anyhow::anyhow!("build current-user security descriptor for named pipe: {error}")
            })?;
            let first = create_instance(path, &security, exclusive_name)
                .map_err(|error| anyhow::anyhow!("create named pipe {path}: {error}"))?;
            Ok(Self {
                path: path.to_owned(),
                security,
                waiting: Some(first),
                consecutive_failures: 0,
            })
        }

        /// The next same-user connection. A client whose identity is
        /// unreadable or names another user is disconnected before anything
        /// is read from it. Cancel-safe: the waiting instance and the failure
        /// count stay on the listener.
        pub async fn accept(&mut self) -> anyhow::Result<(LocalIpcStream, PeerIdentity)> {
            loop {
                if self.consecutive_failures >= INSTANCE_FAILURE_LIMIT {
                    anyhow::bail!(
                        "named pipe {}: {} consecutive pipe instance failures",
                        self.path,
                        self.consecutive_failures
                    );
                }
                if self.consecutive_failures > 0 {
                    tokio::time::sleep(INSTANCE_RETRY_DELAY).await;
                }
                let instance = match self.waiting.as_mut() {
                    Some(instance) => instance,
                    None => match create_instance(&self.path, &self.security, false) {
                        Ok(instance) => self.waiting.insert(instance),
                        Err(error) => {
                            self.consecutive_failures += 1;
                            eprintln!(
                                "Cua Driver: create named pipe {} failed ({} consecutive): {error}; retrying",
                                self.path, self.consecutive_failures
                            );
                            continue;
                        }
                    },
                };
                if let Err(error) = instance.connect().await {
                    self.waiting = None;
                    self.consecutive_failures += 1;
                    eprintln!(
                        "Cua Driver: named pipe connect on {} failed ({} consecutive): {error}; recreating the instance",
                        self.path, self.consecutive_failures
                    );
                    continue;
                }
                self.consecutive_failures = 0;
                let instance = self
                    .waiting
                    .take()
                    .expect("a connected instance is waiting");
                match pipe_client_identity(HANDLE(instance.as_raw_handle())) {
                    Ok((pid, sid)) if sid_is_authorized(self.security.user_sid(), &sid) => {
                        return Ok((
                            LocalIpcStream::from_pipe_server(instance),
                            PeerIdentity::Windows { sid, pid },
                        ));
                    }
                    _ => {
                        tracing::warn!(
                            "service named-pipe connection rejected before request parsing"
                        );
                        let _ = instance.disconnect();
                    }
                }
            }
        }

        /// Close the waiting instance. Connections already accepted keep
        /// their own.
        pub fn close(self) {
            drop(self.waiting);
        }
    }

    fn create_instance(
        path: &str,
        security: &PipeSecurity,
        first_pipe_instance: bool,
    ) -> std::io::Result<NamedPipeServer> {
        // SAFETY: `security` outlives the call, which copies the descriptor
        // into the new instance.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first_pipe_instance)
                .create_with_security_attributes_raw(path, security.attributes())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::reap_finished;

    #[tokio::test]
    async fn finished_connection_tasks_are_released() {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..8 {
            tasks.spawn(async {});
        }
        let live = std::sync::Arc::new(tokio::sync::Notify::new());
        let waiter = live.clone();
        tasks.spawn(async move { waiter.notified().await });
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }

        reap_finished(&mut tasks);

        assert_eq!(tasks.len(), 1, "only the still-running connection remains");
        live.notify_one();
    }

    #[tokio::test]
    async fn a_panicked_connection_task_is_reaped_without_panicking_the_daemon() {
        let mut tasks = tokio::task::JoinSet::new();
        tasks.spawn(async { panic!("connection task panic") });
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        reap_finished(&mut tasks);
        assert!(tasks.is_empty());
    }

    mod endpoint {
        use super::super::{serve_connections, LocalIpcListener, PeerIdentity};
        use crate::ipc::{ConnectPolicy, LocalIpcStream};
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

        #[cfg(unix)]
        fn endpoint() -> (String, tempfile::TempDir) {
            let directory = tempfile::Builder::new()
                .prefix("cua-ipc-")
                .tempdir_in("/tmp")
                .unwrap();
            let path = directory.path().join("server.sock");
            (path.to_str().unwrap().to_owned(), directory)
        }

        #[cfg(windows)]
        fn endpoint() -> (String, ()) {
            let name = format!(
                r"\\.\pipe\cua-driver-ipc-server-{}",
                uuid::Uuid::new_v4().simple()
            );
            (name, ())
        }

        #[cfg(unix)]
        fn expected_peer() -> PeerIdentity {
            PeerIdentity::Unix {
                // SAFETY: geteuid has no preconditions.
                uid: unsafe { libc::geteuid() },
                pid: Some(std::process::id()),
            }
        }

        #[cfg(windows)]
        fn expected_peer() -> PeerIdentity {
            PeerIdentity::Windows {
                sid: crate::ipc::windows::PipeSecurity::current_user()
                    .unwrap()
                    .user_sid()
                    .to_owned(),
                pid: std::process::id(),
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_accepted_peer_is_this_user_and_process() {
            let (path, _directory) = endpoint();
            let mut listener = LocalIpcListener::bind(&path, true).unwrap();
            let client = tokio::spawn({
                let path = path.clone();
                async move { LocalIpcStream::connect(&path, ConnectPolicy::STARTUP).await }
            });
            let (_stream, peer) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .expect("the client connects")
                .unwrap();
            assert_eq!(peer, expected_peer());
            assert_eq!(peer.pid(), Some(std::process::id()));
            client.await.unwrap().unwrap();
            listener.close();
        }

        #[cfg(unix)]
        #[test]
        fn close_removes_the_socket_it_bound_and_only_that_one() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let (path, _directory) = endpoint();
            let listener = runtime.block_on(async { LocalIpcListener::bind(&path, false) });
            let listener = listener.unwrap();
            assert_eq!(
                std::os::unix::fs::PermissionsExt::mode(
                    &std::fs::metadata(&path).unwrap().permissions()
                ) & 0o777,
                0o600
            );
            std::fs::remove_file(&path).unwrap();
            let _replacement = std::os::unix::net::UnixListener::bind(&path).unwrap();
            listener.close();
            assert!(std::path::Path::new(&path).exists(), "a replacement stays");

            std::fs::remove_file(&path).unwrap();
            let listener = runtime
                .block_on(async { LocalIpcListener::bind(&path, false) })
                .unwrap();
            listener.close();
            assert!(!std::path::Path::new(&path).exists());
        }

        #[cfg(unix)]
        #[test]
        fn a_foreign_uid_is_rejected() {
            let error = super::super::unix::authorize_uid(501, 502).unwrap_err();
            assert_eq!(
                error.to_string(),
                "reject Unix peer uid 502 for runtime owned by uid 501"
            );
        }

        /// Shutdown aborts a connection that never ends on its own and joins
        /// it before `serve_connections` returns.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn shutdown_aborts_and_joins_open_connections() {
            struct Dropped(std::sync::Arc<std::sync::atomic::AtomicBool>);
            impl Drop for Dropped {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }

            let (path, _directory) = endpoint();
            let mut listener = LocalIpcListener::bind(&path, true).unwrap();
            let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel::<()>();
            let mut accepted_tx = Some(accepted_tx);
            let handler_dropped = dropped.clone();
            let handler = move |stream: LocalIpcStream, _peer: PeerIdentity| {
                let guard = Dropped(handler_dropped.clone());
                let accepted = accepted_tx.take();
                async move {
                    let _guard = guard;
                    let mut lines = BufReader::new(stream).lines();
                    let _ = lines.next_line().await;
                    if let Some(accepted) = accepted {
                        let _ = accepted.send(());
                    }
                    std::future::pending::<()>().await;
                }
            };
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let client = async {
                let mut client = LocalIpcStream::connect(&path, ConnectPolicy::STARTUP)
                    .await
                    .unwrap();
                client.write_all(b"hello\n").await.unwrap();
                accepted_rx.await.unwrap();
                assert!(
                    !dropped.load(std::sync::atomic::Ordering::SeqCst),
                    "the handler is still running"
                );
                shutdown_tx.send(()).unwrap();
                client
            };
            let shutdown = async {
                let _ = shutdown_rx.await;
            };
            let (served, _client) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(serve_connections(&mut listener, handler, shutdown), client)
            })
            .await
            .expect("serve_connections returns after shutdown");
            served.unwrap();
            assert!(
                dropped.load(std::sync::atomic::Ordering::SeqCst),
                "shutdown aborted and joined the open connection"
            );
            listener.close();
        }
    }
}
