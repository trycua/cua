//! The daemon: `cua.daemon.v1` + env passthrough + media bridge, served on a
//! Unix socket (no token; the socket is mode 0600) and/or loopback TCP
//! (bearer token). Both listeners serve native gRPC (h2c) and gRPC-Web.

use crate::{
    Discovery, Error, Result, Runtime, VERSION,
    convert::{self, from_dur, info_to_pb, probe_from_pb, provider_from_pb},
    passthrough,
};
use axum::body::Body as AxumBody;
use cua_proto::daemon::v1::{
    self as pb, daemon_service_server::*, fork_batch_service_server::*, pool_service_server::*,
    runtime_service_server::*, sandbox_service_server::*, share_service_server::*,
    space_service_server::*,
};
use cua_proto::env::v1 as envpb;
use http::{Request, Response, StatusCode};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tokio::sync::watch;
use tonic::Status;
use tower::{Service, ServiceExt};

/// Where and how to listen.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Unix socket (unix only). Created with mode 0600; a stale socket file
    /// is replaced.
    pub socket_path: Option<PathBuf>,
    /// Loopback TCP address (for example `127.0.0.1:0`). Needs `token`.
    pub loopback: Option<SocketAddr>,
    /// Bearer token of the loopback listener.
    pub token: String,
    /// Discovery file to write (and remove on shutdown).
    pub discovery_path: Option<PathBuf>,
    /// Lifetime of media bridge tickets.
    pub bridge_ticket_ttl: Duration,
}

impl ServerConfig {
    /// Defaults: `~/.cua/cua.sock` (unix), loopback `127.0.0.1:0`, a random
    /// token, `~/.cua/daemon.json`. The Spaces registry lives where the
    /// runtime's `spaces_home` says (default `~/.cua`).
    pub fn default_paths() -> Self {
        Self {
            socket_path: cfg!(unix).then(crate::default_socket_path),
            loopback: Some(SocketAddr::from(([127, 0, 0, 1], 0))),
            token: crate::random_token(),
            discovery_path: Some(crate::default_discovery_path()),
            bridge_ticket_ttl: Duration::from_secs(60),
        }
    }
}

/// A media bridge registered by `OpenMediaBridge`.
#[derive(Clone, Debug)]
pub(crate) struct Bridge {
    pub upstream_url: String,
    pub headers: Vec<(String, String)>,
    pub expires: SystemTime,
}

pub(crate) struct Shared {
    pub runtime: Runtime,
    pub token: String,
    pub info: Mutex<pb::GetInfoResponse>,
    pub bridges: Mutex<HashMap<String, Bridge>>,
    pub bridge_ttl: Duration,
    pub shutdown: watch::Sender<bool>,
    /// Requests being served, and the last time one started (unix secs):
    /// maintenance runs only when the daemon is idle.
    pub inflight: std::sync::atomic::AtomicUsize,
    pub last_activity: std::sync::atomic::AtomicU64,
    /// Spaces this machine provides to its owner's devices
    /// (`cua.env.v1.HostSpacesService`, reached through the host's
    /// cua-spacesd).
    #[cfg(feature = "spaces")]
    pub host_spaces: Arc<cua_spaces::host_spaces::HostSpacesServer>,
}

/// Counts a request as in flight until dropped.
struct Busy(Arc<Shared>);

impl Busy {
    fn new(s: Arc<Shared>) -> Self {
        use std::sync::atomic::Ordering::SeqCst;
        s.inflight.fetch_add(1, SeqCst);
        s.last_activity.store(unix_now(), SeqCst);
        Self(s)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.0.inflight.fetch_sub(1, SeqCst);
        self.0.last_activity.store(unix_now(), SeqCst);
    }
}

/// How often the daemon ends expired local sandboxes
/// (`CUA_SANDBOX_REAP_INTERVAL_MS`, default 2 s).
fn reap_interval() -> Duration {
    std::env::var("CUA_SANDBOX_REAP_INTERVAL_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(Duration::from_secs(2), Duration::from_millis)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How often the daemon checks whether the built-in Linux runtime VM runs
/// with no Linux Space in it.
const IDLE_RUNTIME_CHECK: Duration = Duration::from_secs(300);
/// First maintenance pass after start.
const MAINTENANCE_FIRST: Duration = Duration::from_secs(60);
/// How often the daemon checks that its home is still there.
const HOME_CHECK: Duration = Duration::from_secs(5);

/// Stops the daemon when its home directory is removed from under it (a
/// torn-down environment, a deleted test home). Its sockets, discovery file
/// and state are gone with it, nothing can reach it, and every periodic
/// writer it runs (the Cua Volume's device heartbeat, the lifetimes
/// reaper, the pool) would otherwise bring the directories back one file at
/// a time. Two misses in a row, so a rename in progress is not mistaken for
/// a removal. Checked only when the home existed at start.
pub async fn home_watchdog(
    home: std::path::PathBuf,
    every: Duration,
    shared_shutdown: watch::Sender<bool>,
    mut shutdown: watch::Receiver<bool>,
) {
    if !home.is_dir() {
        return;
    }
    let mut misses = 0;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(every) => {}
        }
        if home.is_dir() {
            misses = 0;
            continue;
        }
        misses += 1;
        if misses >= 2 {
            tracing::warn!(home = %home.display(), "the daemon's home was removed; stopping");
            let _ = shared_shutdown.send(true);
            return;
        }
    }
}

/// First look for VMs nobody uses after the daemon starts.
pub const GUARD_FIRST: Duration = Duration::from_secs(2);
/// Spacing of looks for VMs nobody uses: a scan of the process table and a
/// few small files, so an orphan lives about this long plus
/// [`crate::guard::Policy::starter_gone`].
pub const GUARD_EVERY: Duration = Duration::from_secs(20);

/// Ends the VMs nobody uses ([`crate::guard`]): `first` after start (the VMs
/// a crashed daemon, an expired lifetime or a deleted home left), then every
/// `every`. It sweeps `home` (which this daemon serves) and, with
/// `host_wide`, any home on the machine that has no daemon or no home at
/// all. `CUA_VM_GUARD=0` turns the daemon's own guard off.
pub async fn guard_loop(
    home: std::path::PathBuf,
    host_wide: bool,
    policy: crate::guard::Policy,
    first: Duration,
    every: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut wait = first;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(wait) => {}
        }
        let ended = if host_wide {
            crate::guard::sweep(&home, true, &policy, false).await
        } else {
            crate::guard::sweep_own(&home, true, &policy, false).await
        };
        if !ended.is_empty() {
            tracing::info!(
                ended = ended.iter().filter(|e| e.error.is_none()).count(),
                "ended VMs nobody uses"
            );
        }
        wait = every;
    }
}

/// Idle time before a maintenance pass may run.
const MAINTENANCE_IDLE: Duration = Duration::from_secs(300);
/// Spacing of maintenance passes.
const MAINTENANCE_EVERY: Duration = Duration::from_secs(3600);

/// Reaps orphaned ephemeral sandboxes, collects the cache and caps logs:
/// once a minute after start, then hourly while no request is in flight
/// and none arrived for five minutes. `CUA_DAEMON_MAINTENANCE=0` turns it
/// off.
/// How often the repo cache is tidied and its mirrors refreshed, and how
/// old a mirror used in the last week may get before it is.
const REPO_CACHE_EVERY: Duration = Duration::from_secs(15 * 60);
const REPO_CACHE_REFRESH_AFTER: Duration = Duration::from_secs(60 * 60);

/// Keeps the repo cache within its cap and its mirrors up to date, in the
/// background and one repository at a time (`cua-repo-cache`). Nothing
/// happens while the cache is off.
async fn repo_cache_loop(mut shutdown: watch::Receiver<bool>) {
    let mut next = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep_until(next) => {}
        }
        let cache = cua_sandbox_core::repo_cache::current();
        tokio::select! {
            _ = shutdown.changed() => return,
            () = cache.maintain(REPO_CACHE_REFRESH_AFTER) => {}
        }
        next = tokio::time::Instant::now() + REPO_CACHE_EVERY;
    }
}

async fn maintenance_loop(shared: Arc<Shared>, mut shutdown: watch::Receiver<bool>) {
    use std::sync::atomic::Ordering::SeqCst;
    let Ok(vmm) = shared.runtime.vmm().cloned() else {
        return;
    };
    let state = shared.runtime.sandboxes().state().clone();
    let mut next = tokio::time::Instant::now() + MAINTENANCE_FIRST;
    let mut ran = false;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep_until(next) => {}
        }
        let idle_for = unix_now().saturating_sub(shared.last_activity.load(SeqCst));
        // The first pass (orphans of a crash before this daemon started)
        // only waits for in-flight requests; later ones for five idle
        // minutes.
        if shared.inflight.load(SeqCst) > 0 || (ran && idle_for < MAINTENANCE_IDLE.as_secs()) {
            next = tokio::time::Instant::now() + Duration::from_secs(60);
            continue;
        }
        let config = cua_disk::CacheConfig::load();
        let gc = crate::maintenance::daemon_gc_options(&config);
        let r =
            crate::maintenance::run(vmm.as_ref(), &state, cua_disk::Layout::default(), gc, false)
                .await;
        if !r.reaped.is_empty() || r.gc.as_ref().is_some_and(|g| g.freed > 0) {
            tracing::info!(
                reaped = r.reaped.len(),
                freed = r.gc.as_ref().map(|g| g.freed).unwrap_or(0),
                "daemon maintenance"
            );
        }
        ran = true;
        next = tokio::time::Instant::now() + MAINTENANCE_EVERY;
    }
}

/// Set to `0` to keep this daemon from supervising persistent agents.
pub const ENV_DAEMON_PERSISTENT: &str = "CUA_DAEMON_PERSISTENT";

/// How often the persistent-agent supervisor runs (routine wake latency is
/// at most this).
#[cfg(feature = "spaces")]
pub const PERSISTENT_TICK: Duration = Duration::from_secs(1);

/// A running daemon.
pub struct DaemonHandle {
    /// Loopback URL, when listening on TCP.
    pub loopback_url: Option<String>,
    /// Unix socket, when listening on one.
    pub socket_path: Option<PathBuf>,
    /// Loopback token.
    pub token: String,
    shared: Arc<Shared>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    discovery_path: Option<PathBuf>,
}

impl DaemonHandle {
    /// A handle that requests shutdown when sent `true` (signal handlers).
    pub fn shutdown_trigger(&self) -> watch::Sender<bool> {
        self.shared.shutdown.clone()
    }

    /// Requests shutdown.
    pub fn shutdown(&self) {
        let _ = self.shared.shutdown.send(true);
    }

    /// Waits until shutdown was requested and the listeners stopped.
    pub async fn wait(mut self) {
        let mut rx = self.shared.shutdown.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        // What an extension serves must not outlive the daemon (a mount,
        // writes not yet stored): each ends first, bounded.
        for a in self.shared.runtime.attached_extensions() {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                a.shutdown(&self.shared.runtime),
            )
            .await;
        }
        // The warm pool's members are this daemon's: a stopped daemon takes
        // them with it (they would otherwise run, holding memory, until
        // some later daemon adopts or reaps them). Bounded; whatever is
        // still starting is left to the lease reaper.
        if let Ok(vmm) = self.shared.runtime.vmm()
            && let Some(pool) = vmm.pool()
        {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(90), pool.drain()).await;
        }
        for t in self.tasks.drain(..) {
            t.abort();
            let _ = t.await;
        }
        // The built-in Linux runtime VM must not outlive the daemon that
        // started it (nothing else would stop it): stop it unless Linux
        // Spaces still run in it, and leave the answer for `cua daemon stop`.
        if let Ok(vmm) = self.shared.runtime.vmm().cloned() {
            let stop = tokio::time::timeout(
                std::time::Duration::from_secs(200),
                vmm.stop_linux_runtime_now(),
            )
            .await
            .unwrap_or_else(|_| {
                cua_vmm::managed::RuntimeStop::Failed("stopping took too long".into())
            });
            tracing::info!(?stop, "the Linux runtime at daemon shutdown");
            cua_vmm::managed::record_shutdown(&stop);
        }
        self.cleanup();
    }

    /// [`wait`](Self::wait), with the shutdown bounded: once shutdown is
    /// requested, the daemon's own wind-down (extensions, the warm pool, the
    /// built-in Linux runtime) gets `grace`; past it the socket and discovery
    /// file go and this returns `false`, so a stop signal always ends the
    /// process (a supervisor that sees the daemon gone never waits on one
    /// still winding down for minutes).
    pub async fn wait_bounded(self, grace: Duration) -> bool {
        let mut rx = self.shared.shutdown.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let socket = self.socket_path.clone();
        let discovery = self.discovery_path.clone();
        if tokio::time::timeout(grace, self.wait()).await.is_ok() {
            return true;
        }
        tracing::warn!(
            ?grace,
            "the daemon's shutdown took too long; exiting without it"
        );
        cleanup_paths(socket.as_deref(), discovery.as_deref());
        false
    }

    fn cleanup(&self) {
        cleanup_paths(self.socket_path.as_deref(), self.discovery_path.as_deref());
    }
}

/// Removes the daemon's socket, and its discovery file while it names this
/// process.
fn cleanup_paths(socket: Option<&std::path::Path>, discovery: Option<&std::path::Path>) {
    if let Some(p) = socket {
        let _ = std::fs::remove_file(p);
    }
    if let Some(p) = discovery
        && Discovery::read(p).is_some_and(|d| d.pid == std::process::id())
    {
        let _ = std::fs::remove_file(p);
    }
}

/// How long a stopped daemon may take to wind down before it exits anyway
/// ([`DaemonHandle::wait_bounded`]).
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Resolves on the first stop signal: SIGTERM (`kill`, launchd, systemd, an
/// app quitting) or SIGINT (Ctrl-C) on Unix, Ctrl-C elsewhere.
pub async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(mut term), Ok(mut int)) => {
                tokio::select! {
                    _ = term.recv() => tracing::info!("SIGTERM: stopping the daemon"),
                    _ = int.recv() => tracing::info!("SIGINT: stopping the daemon"),
                }
            }
            _ => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Starts the daemon listeners and returns once they accept connections.
pub async fn start(runtime: Runtime, config: ServerConfig) -> Result<DaemonHandle> {
    cua_spacesd_client::transport::ensure_crypto_provider();
    if config.socket_path.is_none() && config.loopback.is_none() {
        return Err(Error::InvalidArgument(
            "the daemon needs a socket path or a loopback address".into(),
        ));
    }
    // A discovery file of another daemon that still accepts connections
    // wins; one left by a daemon that exited is replaced below (like a
    // stale socket).
    if let Some(p) = &config.discovery_path
        && let Some(d) = Discovery::read(p)
        && d.pid != std::process::id()
        && crate::pid_alive(d.pid)
        && d.listening()
    {
        return Err(Error::InvalidArgument(format!(
            "a daemon is already running (pid {}, {})",
            d.pid,
            p.display()
        )));
    }
    let (tx, _) = watch::channel(false);
    // The daemon hosts local public URLs itself.
    runtime.mark_share_host();
    let features = {
        let mut f = vec!["env".into(), "sandboxes".into()];
        if cfg!(feature = "spaces") {
            f.extend(["spaces".into(), "mcp".into()]);
        }
        // Its extensions, as `extension:<name>` (the Cua Spaces build
        // reports `extension:cua-spaces`).
        #[cfg(feature = "spaces")]
        f.extend(
            runtime
                .extension_names()
                .iter()
                .map(|n| format!("{}{n}", crate::extension::FEATURE_PREFIX)),
        );
        // What the extensions serve (`thread.v1`, ...).
        #[cfg(feature = "spaces")]
        f.extend(runtime.extension_features().iter().cloned());
        f
    };
    #[cfg(feature = "spaces")]
    let host_spaces = cua_spaces::host_spaces::HostSpacesServer::new(runtime.spaces().clone());
    #[cfg(feature = "spaces")]
    runtime.set_host_spaces(host_spaces.clone());
    // Prewarms keep this machine's warm pools (`SetHostWarm`, and this
    // machine's own prewarms).
    #[cfg(feature = "spaces")]
    runtime
        .spaces()
        .set_local_pools(Some(Arc::new(crate::prewarm_pools::DaemonPools(
            runtime.clone(),
        ))));
    // Cluster jobs a crash interrupted end now (their Spaces go).
    #[cfg(feature = "spaces")]
    {
        let h = host_spaces.clone();
        tokio::spawn(async move { h.resume_jobs().await });
    }
    // Local sandbox lifetimes (`KeepAlive` on a local sandbox) end here,
    // whether or not the client that set them is still around.
    {
        let rt = runtime.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(reap_interval());
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                rt.reap_expired().await;
                #[cfg(feature = "spaces")]
                rt.spaces().reap_expired().await;
            }
        });
    }
    // The elastic pool's scale-in: idle cloud hosts this home added go at
    // their billing boundary, the least-used one is cordoned when the
    // others have room (`CUA_ELASTIC_RECONCILE_SECS`, default 30; 0: off).
    #[cfg(feature = "spaces")]
    {
        let every = std::env::var("CUA_ELASTIC_RECONCILE_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(30);
        if every > 0 {
            let rt = runtime.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(every));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    match rt.spaces().elastic_provision_pending().await {
                        Ok(h) if !h.is_empty() => tracing::info!(
                            hosts = h.len(),
                            "elastic pool added hosts for waiting jobs"
                        ),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("elastic pool (waiting jobs): {e}"),
                    }
                    match rt.spaces().elastic_reconcile().await {
                        Ok(d) if !d.is_empty() => {
                            tracing::debug!(decisions = d.len(), "elastic pool reconciled")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("elastic pool reconcile: {e}"),
                    }
                }
            });
        }
    }
    // Running Spaces' thumbnails stay fresh while a client asks for them
    // (the Spaces apps' notch and previews, SDK scripts); idle otherwise.
    #[cfg(feature = "spaces")]
    let _thumbnails = runtime.spaces().spawn_thumbnail_refresh();
    // This computer's processor, memory and disk, sampled every five
    // minutes while nobody watches (the menu bar warning), and published
    // into its relay machine record when it is one.
    #[cfg(feature = "spaces")]
    let _resources = runtime.spaces().spawn_resources_publisher();
    // A local Space that stops on its own (out of memory, a crash) is
    // recorded as stopped, and why, for every client.
    let _stop_watch = runtime.sandboxes().spawn_stop_watch();
    let own = std::env::current_exe()
        .ok()
        .and_then(|e| crate::identity::of(&e));
    let shared = Arc::new(Shared {
        #[cfg(feature = "spaces")]
        host_spaces,
        runtime,
        token: config.token.clone(),
        info: Mutex::new(pb::GetInfoResponse {
            version: VERSION.into(),
            pid: std::process::id(),
            socket_path: String::new(),
            loopback_url: String::new(),
            loopback_token: config.token.clone(),
            features,
            started_at: Some(convert::ts(SystemTime::now())),
            // Taken now, at start: a later rebuild of the file must read
            // as a different build.
            executable: own
                .as_ref()
                .map(|i| i.executable.clone())
                .unwrap_or_default(),
            build_id: own.map(|i| i.build_id).unwrap_or_default(),
            // Filled in per request (the pool changes).
            warm_spaces: 0,
            warm_memory_bytes: 0,
            warm_reason: String::new(),
        }),
        bridges: Mutex::new(HashMap::new()),
        bridge_ttl: config.bridge_ticket_ttl,
        shutdown: tx,
        inflight: Default::default(),
        last_activity: std::sync::atomic::AtomicU64::new(unix_now()),
    });

    let mut tasks = Vec::new();
    let mut loopback_url = None;
    if let Some(addr) = config.loopback {
        if !addr.ip().is_loopback() {
            return Err(Error::InvalidArgument(format!(
                "refusing to listen on non-loopback address {addr}"
            )));
        }
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let url = format!("http://{local}");
        shared.info.lock().unwrap().loopback_url = url.clone();
        loopback_url = Some(url);
        let app = app(shared.clone(), true);
        let rx = shared.shutdown.subscribe();
        tasks.push(tokio::spawn(accept_loop(listener, app, rx)));
    }
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut socket_path: Option<PathBuf> = None;
    #[cfg(unix)]
    if let Some(path) = &config.socket_path {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if path.exists() {
            if tokio::net::UnixStream::connect(path).await.is_ok() {
                return Err(Error::InvalidArgument(format!(
                    "a daemon is already listening on {}",
                    path.display()
                )));
            }
            std::fs::remove_file(path)?;
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        crate::restrict_permissions(path)?;
        shared.info.lock().unwrap().socket_path = path.display().to_string();
        socket_path = Some(path.clone());
        let app = app(shared.clone(), false);
        let rx = shared.shutdown.subscribe();
        tasks.push(tokio::spawn(accept_loop(listener, app, rx)));
    }
    #[cfg(not(unix))]
    if config.socket_path.is_some() && config.loopback.is_none() {
        return Err(Error::Unsupported(
            "Unix sockets are not available on this platform; use loopback".into(),
        ));
    }
    // What this machine's VMs use and may still use, for placement.
    #[cfg(feature = "spaces")]
    if let Ok(vmm) = shared.runtime.vmm() {
        vmm.report_vm_capacity();
    }
    // Local warm pools: the daemon keeps Spaces of the shapes this host
    // creates ready ahead of time (`pool.*` settings).
    if let Ok(vmm) = shared.runtime.vmm()
        && let Some(t) = vmm.enable_pool(
            shared.runtime.sandboxes().state().clone(),
            shared.shutdown.subscribe(),
        )
    {
        tasks.push(t);
    }
    tasks.push(tokio::spawn(home_watchdog(
        crate::cua_home(),
        HOME_CHECK,
        shared.shutdown.clone(),
        shared.shutdown.subscribe(),
    )));
    if std::env::var("CUA_VM_GUARD").as_deref() != Ok("0") {
        tasks.push(tokio::spawn(guard_loop(
            crate::cua_home(),
            true,
            crate::guard::Policy::default(),
            GUARD_FIRST,
            GUARD_EVERY,
            shared.shutdown.subscribe(),
        )));
    }
    // The built-in Linux runtime VM holds the memory its Spaces used until
    // it stops: stop it once idle, also when this daemon started after the
    // last Space was gone.
    if let Ok(vmm) = shared.runtime.vmm().cloned() {
        let mut rx = shared.shutdown.subscribe();
        tasks.push(tokio::spawn(async move {
            loop {
                vmm.stop_idle_linux_runtime().await;
                tokio::select! {
                    _ = rx.changed() => return,
                    _ = tokio::time::sleep(IDLE_RUNTIME_CHECK) => {}
                }
            }
        }));
    }
    if std::env::var(crate::maintenance::ENV_DAEMON_MAINTENANCE).as_deref() != Ok("0") {
        tasks.push(tokio::spawn(maintenance_loop(
            shared.clone(),
            shared.shutdown.subscribe(),
        )));
        tasks.push(tokio::spawn(repo_cache_loop(shared.shutdown.subscribe())));
    }
    // The automatic cloud sweep (`CUA_CLOUD_SWEEP_SECS`, 0: off).
    #[cfg(feature = "byoc")]
    tasks.extend(crate::cloud_sweep::spawn(
        &shared.runtime,
        shared.shutdown.subscribe(),
    ));
    // `cua share`: the records and the gateway the host's cua-spacesd
    // forwards /share/... to.
    #[cfg(feature = "spaces")]
    if let Err(e) = shared.runtime.start_shares(&crate::cua_home()).await {
        tracing::warn!(error = %e, "cua share is unavailable in this daemon");
    }
    // A host in direct mode forwards a port to each Space it provides:
    // open them again (nothing for any other machine).
    #[cfg(feature = "spaces")]
    {
        let host_spaces = shared.host_spaces.clone();
        tasks.push(tokio::spawn(async move {
            host_spaces.restore_direct_forwards().await;
        }));
    }
    // Extension tasks (the Cua Spaces build: the persistent-agent
    // supervisor and the Keyvault socket beside `cua.sock`).
    // Where they put their sockets: beside `cua.sock`, or on Windows (which
    // has no Unix socket) in the home the discovery file is in.
    #[cfg(feature = "spaces")]
    {
        let extension_dir = socket_path.as_ref().and_then(|p| p.parent()).or_else(|| {
            config
                .discovery_path
                .as_deref()
                .and_then(|p| p.parent())
                .filter(|_| cfg!(windows))
        });
        for a in shared.runtime.attached_extensions() {
            tasks.extend(a.daemon_tasks(&shared.runtime, extension_dir));
        }
    }
    if let Some(p) = &config.discovery_path {
        Discovery {
            pid: std::process::id(),
            socket_path: socket_path.as_ref().map(|p| p.display().to_string()),
            loopback_url: loopback_url.clone(),
            token: Some(config.token.clone()),
            version: VERSION.into(),
        }
        .write(p)?;
    }
    Ok(DaemonHandle {
        loopback_url,
        socket_path,
        token: config.token,
        shared,
        tasks,
        discovery_path: config.discovery_path,
    })
}

type BoxedApp = tower::util::BoxCloneSyncService<
    Request<hyper::body::Incoming>,
    Response<AxumBody>,
    std::convert::Infallible,
>;

trait Accept: Send + 'static {
    type Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static;
    fn accept_io(&self) -> impl std::future::Future<Output = std::io::Result<Self::Io>> + Send;
}

impl Accept for tokio::net::TcpListener {
    type Io = tokio::net::TcpStream;
    async fn accept_io(&self) -> std::io::Result<Self::Io> {
        let (s, _) = self.accept().await?;
        let _ = s.set_nodelay(true);
        Ok(s)
    }
}

#[cfg(unix)]
impl Accept for tokio::net::UnixListener {
    type Io = tokio::net::UnixStream;
    async fn accept_io(&self) -> std::io::Result<Self::Io> {
        Ok(self.accept().await?.0)
    }
}

async fn accept_loop<L: Accept>(listener: L, app: BoxedApp, mut shutdown: watch::Receiver<bool>) {
    use hyper_util::{
        rt::{TokioExecutor, TokioIo},
        server::conn::auto::Builder,
        service::TowerToHyperService,
    };
    let mut conns = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept_io() => {
                let Ok(stream) = accepted else { continue };
                let svc = TowerToHyperService::new(app.clone());
                conns.spawn(async move {
                    let mut builder = Builder::new(TokioExecutor::new());
                    // The env passthrough carries file transfers and
                    // streamed output at memory speed: the same windows and
                    // frames as cua-spacesd's own listener, not hyper's
                    // 64 KiB / 16 KiB defaults.
                    builder
                        .http2()
                        .initial_stream_window_size(cua_spacesd_client::transport::STREAM_WINDOW)
                        .initial_connection_window_size(
                            cua_spacesd_client::transport::CONNECTION_WINDOW,
                        )
                        .max_frame_size(cua_spacesd_client::transport::MAX_FRAME);
                    let _ = builder
                        .serve_connection_with_upgrades(TokioIo::new(stream), svc)
                        .await;
                });
            }
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
    conns.abort_all();
}

/// Headers that name a caller other than the owner: the relay-verified
/// caller the host's cua-spacesd forwards (`x-cua-host-caller`), and the
/// share gateway's (`x-cua-share-caller`), which no daemon call reads.
pub const FORWARDED_CALLER_HEADERS: &[&str] = &["x-cua-host-caller", "x-cua-share-caller"];

/// Removes [`FORWARDED_CALLER_HEADERS`] from a request that did not come
/// through the daemon socket.
pub fn strip_forwarded_caller(headers: &mut http::HeaderMap) {
    for h in FORWARDED_CALLER_HEADERS {
        headers.remove(*h);
    }
}

fn app(shared: Arc<Shared>, require_token: bool) -> BoxedApp {
    let svc = Svc(shared.clone());
    // The clients' limit, not gRPC's 4 MiB default: a tool call can carry
    // a full-size screenshot or file either way.
    let limit = cua_spacesd_client::MAX_MESSAGE_BYTES;
    let grpc = tonic::service::Routes::new(
        ForkBatchServiceServer::new(svc.clone())
            .max_decoding_message_size(limit)
            .max_encoding_message_size(limit),
    )
    .add_service(
        SandboxServiceServer::new(svc.clone())
            .max_decoding_message_size(limit)
            .max_encoding_message_size(limit),
    )
    .add_service(
        SpaceServiceServer::new(svc.clone())
            .max_decoding_message_size(limit)
            .max_encoding_message_size(limit),
    )
    .add_service(
        RuntimeServiceServer::new(svc.clone())
            .max_decoding_message_size(limit)
            .max_encoding_message_size(limit),
    )
    .add_service(
        PoolServiceServer::new(svc.clone())
            .max_decoding_message_size(limit)
            .max_encoding_message_size(limit),
    )
    .add_service(
        DaemonServiceServer::new(svc.clone())
            .max_decoding_message_size(limit)
            .max_encoding_message_size(limit),
    )
    .add_service(ShareServiceServer::new(svc.clone()));
    #[cfg(feature = "spaces")]
    let grpc = grpc
        .add_service(
            cua_proto::env::v1::host_spaces_service_server::HostSpacesServiceServer::new(
                crate::host_svc::HostSvc(svc.0.clone()),
            ),
        )
        .add_service(
            cua_proto::env::v1::host_jobs_service_server::HostJobsServiceServer::new(
                crate::host_svc::HostJobsSvc(svc.0.clone()),
            )
            .max_decoding_message_size(64 * 1024 * 1024)
            .max_encoding_message_size(64 * 1024 * 1024),
        )
        .add_service(
            cua_proto::env::v1::host_thread_service_server::HostThreadServiceServer::new(
                crate::host_svc::HostThreadSvc(svc.0.clone()),
            )
            .max_decoding_message_size(limit)
            .max_encoding_message_size(limit),
        )
        .add_service(
            cua_proto::env::v1::host_snapshots_service_server::HostSnapshotsServiceServer::new(
                crate::host_svc::HostSnapshotsSvc(svc.0.clone()),
            )
            .max_decoding_message_size(64 * 1024 * 1024)
            .max_encoding_message_size(64 * 1024 * 1024),
        );
    let grpc = grpc.into_axum_router();
    let router = axum::Router::new()
        .route(
            "/v1/sandboxes/{name}/env/{*rest}",
            axum::routing::any(passthrough::env_passthrough),
        )
        .route(
            "/v1/sandboxes/{name}/svc/{service}/{*rest}",
            axum::routing::any(passthrough::service_passthrough),
        )
        .route(
            "/v1/spaces/{key}/env/{*rest}",
            axum::routing::any(passthrough::space_env_passthrough),
        )
        .route(
            "/v1/spaces/{key}/svc/{service}/{*rest}",
            axum::routing::any(passthrough::space_service_passthrough),
        )
        .route(
            "/v1/bridge/media",
            axum::routing::get(passthrough::bridge_media),
        )
        .with_state(shared.clone())
        .merge(grpc);
    // The Spaces MCP server over streamable HTTP. The listener's own auth
    // (loopback bearer, or the 0600 socket) already guards it.
    #[cfg(feature = "spaces")]
    let router = {
        let mut mcp = cua_spaces::mcp::McpServer::new(shared.runtime.spaces().clone());
        if let Some(broker) = shared.runtime.session_broker() {
            mcp = mcp.with_session_broker(broker);
        }
        router.merge(cua_spaces::mcp::http::router(mcp, None))
    };
    // tonic-web answers every non-gRPC-Web HTTP/1.1 request with 400, so
    // only gRPC-Web goes through it; native gRPC (h2c), WebSocket upgrades
    // and plain HTTP reach the router directly.
    let web = tower::ServiceBuilder::new()
        .layer(tonic_web::GrpcWebLayer::new())
        .service(router.clone());
    let token = shared.token.clone();
    let authed = tower::service_fn(move |req: Request<hyper::body::Incoming>| {
        let mut web = web.clone();
        let mut router = router.clone();
        let token = token.clone();
        let busy = Busy::new(shared.clone());
        async move {
            let _busy = busy;
            let mut req = req.map(AxumBody::new);
            if require_token && !authorized(&req, &token) {
                return Ok::<_, std::convert::Infallible>(unauthorized(&req));
            }
            if require_token {
                // Only the 0600 socket's peer (the host's cua-spacesd)
                // forwards a relay-verified caller. On the loopback listener
                // (the apps, web views, anything holding the token) a
                // caller header is the client's own claim: dropped, so the
                // call is the owner's.
                strip_forwarded_caller(req.headers_mut());
            }
            let grpc_web = req
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|c| c.starts_with("application/grpc-web"));
            if grpc_web {
                let resp = match ServiceExt::<Request<AxumBody>>::ready(&mut web).await {
                    Ok(s) => s.call(req).await,
                    Err(e) => match e {},
                };
                return Ok(match resp {
                    Ok(r) => r.map(AxumBody::new),
                    Err(e) => match e {},
                });
            }
            let resp = match ServiceExt::<Request<AxumBody>>::ready(&mut router).await {
                Ok(s) => s.call(req).await,
                Err(e) => match e {},
            };
            Ok(match resp {
                Ok(r) => r,
                Err(e) => match e {},
            })
        }
    });
    let cors = tower_http::cors::CorsLayer::very_permissive().expose_headers([
        http::HeaderName::from_static("grpc-status"),
        http::HeaderName::from_static("grpc-message"),
        http::HeaderName::from_static("grpc-status-details-bin"),
    ]);
    let svc = tower::ServiceBuilder::new()
        .layer(cors)
        .service(authed)
        .map_response(|r: Response<_>| r.map(AxumBody::new));
    tower::util::BoxCloneSyncService::new(svc)
}

fn authorized(req: &Request<AxumBody>, token: &str) -> bool {
    let path = req.uri().path();
    // Ticket-authenticated route (browsers cannot set headers).
    if path == "/v1/bridge/media" {
        return true;
    }
    let expected = format!("Bearer {token}");
    let presented = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    presented.is_some_and(|p| constant_time_eq(p.as_bytes(), expected.as_bytes()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unauthorized(req: &Request<AxumBody>) -> Response<AxumBody> {
    let grpc = req
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("application/grpc"));
    if grpc {
        let status = Error::Unauthenticated("missing or wrong daemon token".into()).to_status();
        let mut resp: Response<AxumBody> = status.into_http();
        if let Some(ct) = req.headers().get(http::header::CONTENT_TYPE) {
            resp.headers_mut()
                .insert(http::header::CONTENT_TYPE, ct.clone());
        }
        resp
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(AxumBody::from("missing or wrong daemon token"))
            .unwrap()
    }
}

// ------------------------------------------------------------------ services

#[derive(Clone)]
pub(crate) struct Svc(pub(crate) Arc<Shared>);

impl Svc {
    pub(crate) fn rt(&self) -> &Runtime {
        &self.0.runtime
    }

    /// The env connection and WebSocket headers a media bridge for `name`
    /// relays to: a registered Space (a `relay:` or legacy `space://…` id, or
    /// a name the Spaces registry resolves when no sandbox has it), with its
    /// gateway headers (relay or env token), else a sandbox.
    async fn media_upstream(
        &self,
        name: &str,
    ) -> Result<(cua_spacesd_client::SpacesdClient, Vec<(String, String)>)> {
        #[cfg(feature = "spaces")]
        {
            let spaces = self.rt().spaces();
            let is_space = name.starts_with("space://")
                || name.starts_with("relay:")
                || (name.starts_with("direct:")
                    && matches!(spaces.registry().get(name), Ok(Some(_))))
                || (self.rt().record(name).await.is_err() && spaces.resolve(name).is_ok());
            if is_space {
                let space = spaces.space(name).await?;
                let headers = space.websocket_headers().await?;
                return Ok((space.spacesd()?.clone(), headers));
            }
        }
        let a = self.rt().env(name, None).await?;
        Ok((a.client, a.ws_headers))
    }
}

/// A `CreateSandboxRequest` as the runtime's create. The deprecated request
/// fields still count when `location` / `runtime` are empty (older
/// clients).
#[allow(deprecated)]
/// The guest network a create applies once it succeeds, as its response
/// says it (`CreateSandboxResponse.network`): a client checks it, since a
/// daemon that predates `network` never reads it.
fn applied_network(create: &crate::CreateRequest) -> String {
    let mode = cua_sandbox_core::NetworkMode::parse(create.network.as_deref().unwrap_or_default())
        .unwrap_or_default();
    crate::spaces_svc::network_word(mode).to_string()
}

fn create_request(
    r: pb::CreateSandboxRequest,
) -> std::result::Result<crate::CreateRequest, Status> {
    crate::convert::create_request_from_pb(r).map_err(st)
}

pub(crate) fn st(e: Error) -> Status {
    e.to_status()
}

pub(crate) type R<T> = std::result::Result<tonic::Response<T>, Status>;

pub(crate) fn ok<T>(t: T) -> R<T> {
    Ok(tonic::Response::new(t))
}

#[tonic::async_trait]
impl ForkBatchService for Svc {
    type ForkSandboxBatchStream = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<
                    Item = std::result::Result<pb::ForkSandboxBatchResponse, Status>,
                > + Send,
        >,
    >;

    async fn fork_sandbox_batch(
        &self,
        req: tonic::Request<pb::ForkSandboxBatchRequest>,
    ) -> R<Self::ForkSandboxBatchStream> {
        #[cfg(feature = "spaces")]
        {
            use tokio_stream::StreamExt as _;
            let rx = self.rt().fork_batch(req.into_inner()).await.map_err(st)?;
            ok(Box::pin(
                tokio_stream::wrappers::ReceiverStream::new(rx).map(|r| r.map_err(st)),
            ))
        }
        #[cfg(not(feature = "spaces"))]
        {
            let _ = req;
            Err(Status::unimplemented(
                "batched forks need the cua daemon of Cua Spaces",
            ))
        }
    }
}

#[tonic::async_trait]
impl SandboxService for Svc {
    async fn list_sandboxes(
        &self,
        req: tonic::Request<pb::ListSandboxesRequest>,
    ) -> R<pb::ListSandboxesResponse> {
        let req = req.into_inner();
        #[allow(deprecated)]
        let provider = match req.location.trim() {
            "" => provider_from_pb(req.provider),
            l => match cua_sandbox_core::Location::parse(l) {
                Some(cua_sandbox_core::Location::Local) => {
                    Some(cua_sandbox_core::ProviderKind::Local)
                }
                // Cua Cloud has ended: it has no sandboxes to list.
                Some(cua_sandbox_core::Location::Cloud) => {
                    return ok(pb::ListSandboxesResponse::default());
                }
                Some(cua_sandbox_core::Location::Direct) => {
                    Some(cua_sandbox_core::ProviderKind::Direct)
                }
                _ => {
                    return Err(st(Error::InvalidArgument(format!(
                        "location {l:?}: expected local or direct"
                    ))));
                }
            },
        };
        let list = self.rt().list_filtered(provider).await.map_err(st)?;
        ok(pb::ListSandboxesResponse {
            sandboxes: list.sandboxes.iter().map(info_to_pb).collect(),
            warnings: list.warnings,
        })
    }

    async fn create_sandbox(
        &self,
        req: tonic::Request<pb::CreateSandboxRequest>,
    ) -> R<pb::CreateSandboxResponse> {
        let create = create_request(req.into_inner())?;
        let network = applied_network(&create);
        let rec = self.rt().create(create).await.map_err(st)?;
        ok(pb::CreateSandboxResponse {
            sandbox: Some(info_to_pb(&rec)),
            network,
        })
    }

    type CreateSandboxStreamStream = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<
                    Item = std::result::Result<pb::CreateSandboxStreamResponse, Status>,
                > + Send,
        >,
    >;

    async fn create_sandbox_stream(
        &self,
        req: tonic::Request<pb::CreateSandboxStreamRequest>,
    ) -> R<Self::CreateSandboxStreamStream> {
        use pb::create_sandbox_stream_response::Event;
        let create = create_request(req.into_inner().create.unwrap_or_default())?;
        let network = applied_network(&create);
        // Bounded: a slow reader drops progress, never the result (which
        // goes out on the same channel after every report).
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let progress = tx.clone();
        let sink: cua_sandbox_core::progress::Sink = std::sync::Arc::new(move |p| {
            let _ = progress.try_send(Ok(pb::CreateSandboxStreamResponse {
                event: Some(Event::Progress(crate::spaces_svc::progress_pb(p))),
            }));
        });
        let rt = self.rt().clone();
        tokio::spawn(async move {
            let create = cua_sandbox_core::progress::scope(sink, rt.create(create));
            // A closed stream drops the create: cleaned up as a
            // `CreateSandbox` whose caller went away.
            let result = tokio::select! {
                r = create => r,
                () = tx.closed() => return,
            };
            let _ = tx
                .send(
                    result
                        .map(|rec| pb::CreateSandboxStreamResponse {
                            event: Some(Event::Result(pb::CreateSandboxResponse {
                                sandbox: Some(info_to_pb(&rec)),
                                network,
                            })),
                        })
                        .map_err(st),
                )
                .await;
        });
        ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn get_sandbox(
        &self,
        req: tonic::Request<pb::GetSandboxRequest>,
    ) -> R<pb::GetSandboxResponse> {
        let rec = self.rt().record(&req.into_inner().name).await.map_err(st)?;
        ok(pb::GetSandboxResponse {
            sandbox: Some(info_to_pb(&rec)),
        })
    }

    async fn cancel_create_sandbox(
        &self,
        req: tonic::Request<pb::CancelCreateSandboxRequest>,
    ) -> R<pb::CancelCreateSandboxResponse> {
        let what = self
            .rt()
            .cancel_create(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::CancelCreateSandboxResponse {
            cancelled: what.is_some(),
            message: what.unwrap_or_else(|| "no create of that name is running".into()),
        })
    }

    async fn delete_sandbox(
        &self,
        req: tonic::Request<pb::DeleteSandboxRequest>,
    ) -> R<pb::DeleteSandboxResponse> {
        self.rt().delete(&req.into_inner().name).await.map_err(st)?;
        ok(pb::DeleteSandboxResponse {})
    }

    async fn connect_sandbox(
        &self,
        req: tonic::Request<pb::ConnectSandboxRequest>,
    ) -> R<pb::ConnectSandboxResponse> {
        let rec = self
            .rt()
            .connect(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::ConnectSandboxResponse {
            sandbox: Some(info_to_pb(&rec)),
        })
    }

    async fn suspend_sandbox(
        &self,
        req: tonic::Request<pb::SuspendSandboxRequest>,
    ) -> R<pb::SuspendSandboxResponse> {
        self.rt()
            .suspend(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::SuspendSandboxResponse {})
    }

    async fn resume_sandbox(
        &self,
        req: tonic::Request<pb::ResumeSandboxRequest>,
    ) -> R<pb::ResumeSandboxResponse> {
        self.rt().resume(&req.into_inner().name).await.map_err(st)?;
        ok(pb::ResumeSandboxResponse {})
    }

    async fn restart_sandbox(
        &self,
        req: tonic::Request<pb::RestartSandboxRequest>,
    ) -> R<pb::RestartSandboxResponse> {
        self.rt()
            .restart(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::RestartSandboxResponse {})
    }

    async fn resize_sandbox(
        &self,
        req: tonic::Request<pb::ResizeSandboxRequest>,
    ) -> R<pb::ResizeSandboxResponse> {
        let r = req.into_inner();
        let shape = cua_vmm::Resize {
            cpus: r.cpus,
            memory_mb: r.memory_mb,
            disk_gb: r.disk_gb,
            probes: Vec::new(),
        };
        self.rt().resize(&r.name, &shape).await.map_err(st)?;
        ok(pb::ResizeSandboxResponse {})
    }

    async fn keep_alive(
        &self,
        req: tonic::Request<pb::KeepAliveRequest>,
    ) -> R<pb::KeepAliveResponse> {
        let r = req.into_inner();
        let d = r
            .duration
            .as_ref()
            .map(from_dur)
            .unwrap_or(Duration::from_secs(600));
        if r.on_expire.is_empty() {
            self.rt().keep_alive(&r.name, d).await.map_err(st)?;
        } else {
            let action = cua_sandbox_core::LocalExpiry::parse(&r.on_expire).ok_or_else(|| {
                tonic::Status::invalid_argument(format!(
                    "on_expire {:?}: expected \"delete\" or \"suspend\"",
                    r.on_expire
                ))
            })?;
            self.rt()
                .set_lifetime(&r.name, d, action)
                .await
                .map_err(st)?;
        }
        ok(pb::KeepAliveResponse {})
    }

    async fn wait_ready(
        &self,
        req: tonic::Request<pb::WaitReadyRequest>,
    ) -> R<pb::WaitReadyResponse> {
        let r = req.into_inner();
        let probes = r
            .probes
            .iter()
            .map(probe_from_pb)
            .collect::<Result<Vec<_>>>()
            .map_err(st)?;
        let t = r
            .timeout
            .as_ref()
            .map(from_dur)
            .unwrap_or(Duration::from_secs(300));
        self.rt()
            .wait_ready(&r.name, &probes, t)
            .await
            .map_err(st)?;
        ok(pb::WaitReadyResponse {})
    }

    async fn service_request(
        &self,
        req: tonic::Request<pb::ServiceRequestRequest>,
    ) -> R<pb::ServiceRequestResponse> {
        let r = req.into_inner();
        let t = r
            .timeout
            .as_ref()
            .map(from_dur)
            .unwrap_or(Duration::from_secs(30));
        let body = (!r.body.is_empty()).then(|| r.body.to_vec());
        let headers: Vec<(String, String)> =
            r.headers.into_iter().map(|h| (h.name, h.value)).collect();
        let resp = self
            .rt()
            .service_request(&r.name, &r.service, &r.method, &r.path, &headers, body, t)
            .await
            .map_err(st)?;
        ok(pb::ServiceRequestResponse {
            status: resp.status as u32,
            headers: resp
                .headers
                .into_iter()
                .map(|(name, value)| pb::HttpHeader { name, value })
                .collect(),
            body: resp.body,
        })
    }

    async fn forward_port(
        &self,
        req: tonic::Request<pb::ForwardPortRequest>,
    ) -> R<pb::ForwardPortResponse> {
        let r = req.into_inner();
        let port = u16::try_from(r.port)
            .map_err(|_| st(Error::InvalidArgument(format!("port {}", r.port))))?;
        let f = self.rt().forward(&r.name, port).await.map_err(st)?;
        ok(pb::ForwardPortResponse {
            forward_id: f.id,
            guest_port: f.guest_port as u32,
            local_addr: f.local_addr.unwrap_or_default(),
            url: f.url.unwrap_or_default(),
        })
    }

    async fn get_service_url(
        &self,
        req: tonic::Request<pb::GetServiceUrlRequest>,
    ) -> R<pb::GetServiceUrlResponse> {
        let r = req.into_inner();
        let url = self
            .rt()
            .service_url(&r.name, &r.service)
            .await
            .map_err(st)?;
        ok(pb::GetServiceUrlResponse { url })
    }

    async fn create_public_url(
        &self,
        req: tonic::Request<pb::CreatePublicUrlRequest>,
    ) -> R<pb::CreatePublicUrlResponse> {
        let r = req.into_inner();
        let ttl = r.ttl.as_ref().map(from_dur);
        let u = if r.upstream_url.is_empty() {
            let label = (!r.label.is_empty()).then_some(r.label);
            self.rt()
                .public_url(&r.name, &r.service, ttl, label)
                .await
                .map_err(st)?
        } else {
            self.rt()
                .host_share(
                    &r.upstream_url,
                    ttl.unwrap_or(crate::shares::DEFAULT_TTL),
                    &r.name,
                    &r.service,
                )
                .await
                .map_err(st)?
        };
        ok(pb::CreatePublicUrlResponse {
            public_url: Some(convert::public_url_to_pb(&u)),
        })
    }

    async fn revoke_public_url(
        &self,
        req: tonic::Request<pb::RevokePublicUrlRequest>,
    ) -> R<pb::RevokePublicUrlResponse> {
        let r = req.into_inner();
        self.rt()
            .revoke_public_url(&r.name, &r.id)
            .await
            .map_err(st)?;
        ok(pb::RevokePublicUrlResponse {})
    }

    async fn checkpoint_sandbox(
        &self,
        req: tonic::Request<pb::CheckpointSandboxRequest>,
    ) -> R<pb::CheckpointSandboxResponse> {
        let r = req.into_inner();
        // A client that disconnects or dies mid-checkpoint must not cancel
        // it: a microVM is paused while its memory is saved, and work dropped
        // there would leave it paused. The checkpoint runs to completion in
        // its own task whatever happens to this call.
        let rt = self.rt().clone();
        let checkpoint =
            tokio::spawn(
                async move { rt.checkpoint_anywhere(&r.name, &r.checkpoint, r.pin).await },
            )
            .await
            .map_err(|e| tonic::Status::internal(format!("checkpoint task: {e}")))?
            .map_err(st)?;
        ok(pb::CheckpointSandboxResponse {
            checkpoint: Some(checkpoint),
        })
    }

    async fn fork_sandbox(
        &self,
        req: tonic::Request<pb::ForkSandboxRequest>,
    ) -> R<pb::ForkSandboxResponse> {
        let r = req.into_inner();
        // Like a checkpoint, a fork runs to completion even if its caller
        // goes away, so it never leaves a half-started sandbox behind.
        let rt = self.rt().clone();
        let rec = tokio::spawn(async move {
            rt.fork(&r.source, &r.name, r.labels.into_iter().collect())
                .await
        })
        .await
        .map_err(|e| tonic::Status::internal(format!("fork task: {e}")))?
        .map_err(st)?;
        ok(pb::ForkSandboxResponse {
            sandbox: Some(info_to_pb(&rec)),
        })
    }

    async fn list_checkpoints(
        &self,
        _req: tonic::Request<pb::ListCheckpointsRequest>,
    ) -> R<pb::ListCheckpointsResponse> {
        ok(pb::ListCheckpointsResponse {
            checkpoints: self.rt().checkpoints_everywhere().await,
        })
    }

    async fn delete_checkpoint(
        &self,
        req: tonic::Request<pb::DeleteCheckpointRequest>,
    ) -> R<pb::DeleteCheckpointResponse> {
        self.rt()
            .delete_checkpoint_everywhere(&req.into_inner().name)
            .await
            .map_err(st)?;
        ok(pb::DeleteCheckpointResponse {})
    }

    async fn pin_checkpoint(
        &self,
        req: tonic::Request<pb::PinCheckpointRequest>,
    ) -> R<pb::PinCheckpointResponse> {
        let r = req.into_inner();
        let rec = self
            .rt()
            .pin_checkpoint(&r.name, r.pinned)
            .await
            .map_err(st)?;
        ok(pb::PinCheckpointResponse {
            checkpoint: Some(convert::checkpoint_to_pb(&rec)),
        })
    }

    async fn prune_checkpoints(
        &self,
        _req: tonic::Request<pb::PruneCheckpointsRequest>,
    ) -> R<pb::PruneCheckpointsResponse> {
        // Like a checkpoint, a prune runs to completion even if its caller
        // goes away, so no checkpoint is left half deleted.
        let rt = self.rt().clone();
        let deleted = tokio::spawn(async move { rt.prune_checkpoints().await })
            .await
            .map_err(|e| tonic::Status::internal(format!("prune task: {e}")))?
            .map_err(st)?;
        ok(pb::PruneCheckpointsResponse { deleted })
    }

    async fn close_forward(
        &self,
        req: tonic::Request<pb::CloseForwardRequest>,
    ) -> R<pb::CloseForwardResponse> {
        self.rt()
            .close_forward(&req.into_inner().forward_id)
            .map_err(st)?;
        ok(pb::CloseForwardResponse {})
    }

    async fn get_service_endpoint(
        &self,
        req: tonic::Request<pb::GetServiceEndpointRequest>,
    ) -> R<pb::GetServiceEndpointResponse> {
        let r = req.into_inner();
        // Fails with NotFound for an unknown sandbox or service.
        self.rt()
            .service_endpoint(&r.name, &r.service)
            .await
            .map_err(st)?;
        let (base, token) = {
            let info = self.0.info.lock().unwrap();
            (info.loopback_url.clone(), info.loopback_token.clone())
        };
        if base.is_empty() {
            return Err(st(Error::Unsupported(
                "the service passthrough needs the daemon's loopback listener".into(),
            )));
        }
        ok(pb::GetServiceEndpointResponse {
            url: format!(
                "{base}/v1/sandboxes/{}/svc/{}",
                passthrough::encode_segment(&r.name),
                passthrough::encode_segment(&r.service)
            ),
            headers: vec![pb::HttpHeader {
                name: "authorization".into(),
                value: format!("Bearer {token}"),
            }],
        })
    }

    async fn get_env_endpoint(
        &self,
        req: tonic::Request<pb::GetEnvEndpointRequest>,
    ) -> R<pb::GetEnvEndpointResponse> {
        let r = req.into_inner();
        let a = self
            .rt()
            .env(&r.name, r.probe_timeout.as_ref().map(from_dur))
            .await
            .map_err(st)?;
        let caps = a.client.capabilities().await.map_err(|e| st(e.into()))?;
        let (base, token) = {
            let info = self.0.info.lock().unwrap();
            (info.loopback_url.clone(), info.loopback_token.clone())
        };
        if base.is_empty() {
            return Err(st(Error::Unsupported(
                "the env passthrough needs the daemon's loopback listener".into(),
            )));
        }
        ok(pb::GetEnvEndpointResponse {
            url: format!(
                "{base}/v1/sandboxes/{}/env",
                passthrough::encode_segment(&r.name)
            ),
            token,
            spacesd_version: caps.version,
        })
    }
}

#[tonic::async_trait]
impl PoolService for Svc {
    async fn create_pool(
        &self,
        req: tonic::Request<pb::CreatePoolRequest>,
    ) -> R<pb::CreatePoolResponse> {
        let r = req.into_inner();
        let spec =
            crate::convert::create_request_from_pb(r.spec.unwrap_or_default()).map_err(st)?;
        let p = self
            .rt()
            .create_pool(
                Some(r.name.as_str()),
                spec,
                r.min,
                r.max,
                r.idle.as_ref().map(from_dur),
            )
            .await
            .map_err(st)?;
        ok(pb::CreatePoolResponse {
            pool: Some(crate::convert::pool_to_pb(&p)),
        })
    }

    async fn get_pool(&self, req: tonic::Request<pb::GetPoolRequest>) -> R<pb::GetPoolResponse> {
        let p = self.rt().get_pool(&req.into_inner().name).map_err(st)?;
        ok(pb::GetPoolResponse {
            pool: Some(crate::convert::pool_to_pb(&p)),
        })
    }

    async fn list_pools(
        &self,
        _: tonic::Request<pb::ListPoolsRequest>,
    ) -> R<pb::ListPoolsResponse> {
        let pools = self.rt().list_pools().map_err(st)?;
        ok(pb::ListPoolsResponse {
            pools: pools.iter().map(crate::convert::pool_to_pb).collect(),
        })
    }

    async fn delete_pool(
        &self,
        req: tonic::Request<pb::DeletePoolRequest>,
    ) -> R<pb::DeletePoolResponse> {
        let deleted = self.rt().delete_pool(&req.into_inner().name).map_err(st)?;
        ok(pb::DeletePoolResponse { deleted })
    }
}

#[tonic::async_trait]
impl RuntimeService for Svc {
    async fn doctor(&self, _: tonic::Request<pb::DoctorRequest>) -> R<pb::DoctorResponse> {
        ok(crate::local::doctor_response().await)
    }

    async fn setup(&self, req: tonic::Request<pb::SetupRequest>) -> R<pb::SetupResponse> {
        let r = req.into_inner();
        ok(pb::SetupResponse {
            steps: crate::local::setup(&r.components, r.dry_run)
                .await
                .map_err(st)?,
        })
    }

    async fn pull_image(
        &self,
        req: tonic::Request<pb::PullImageRequest>,
    ) -> R<pb::PullImageResponse> {
        let vmm = self.rt().vmm().map_err(st)?.clone();
        let i = crate::local::pull_image(&vmm, &req.into_inner().reference)
            .await
            .map_err(st)?;
        ok(pb::PullImageResponse {
            image: Some(pb::LocalImage {
                reference: i.reference,
                kind: i.kind,
                location: i.location,
                size_bytes: i.size_bytes,
            }),
        })
    }

    async fn build_image(
        &self,
        req: tonic::Request<pb::BuildImageRequest>,
    ) -> R<pb::BuildImageResponse> {
        let r = req.into_inner();
        let vmm = self.rt().vmm().map_err(st)?.clone();
        let out_dir = crate::cua_home()
            .join("build")
            .join(format!("out-{:x}", rand::random::<u32>()));
        let push = (!r.push.is_empty()).then_some(r.push);
        let result = crate::local::build_image(&vmm, &r.spec_json, &r.base, &out_dir, push)
            .await
            .map_err(st)?;
        ok(pb::BuildImageResponse { result })
    }

    async fn push_image(
        &self,
        req: tonic::Request<pb::PushImageRequest>,
    ) -> R<pb::PushImageResponse> {
        let r = req.into_inner();
        ok(pb::PushImageResponse {
            digest: crate::local::push_image(&r.reference, &r.destination)
                .await
                .map_err(st)?,
        })
    }
}

#[tonic::async_trait]
impl DaemonService for Svc {
    async fn get_info(&self, _: tonic::Request<pb::GetInfoRequest>) -> R<pb::GetInfoResponse> {
        let mut info = self.0.info.lock().unwrap().clone();
        if let Some(pool) = self.0.runtime.vmm().ok().and_then(|v| v.pool()) {
            let st = pool.status();
            info.warm_spaces = st.members;
            info.warm_memory_bytes = st.memory_mb * 1024 * 1024;
            info.warm_reason = st.reason;
        }
        ok(info)
    }

    async fn open_media_bridge(
        &self,
        req: tonic::Request<pb::OpenMediaBridgeRequest>,
    ) -> R<pb::OpenMediaBridgeResponse> {
        let r = req.into_inner();
        let open: envpb::OpenMediaRequest = if r.open_media_json.trim().is_empty() {
            envpb::OpenMediaRequest {
                target: Some(envpb::MediaTarget {
                    target: Some(envpb::media_target::Target::DisplayId("primary".into())),
                }),
                ..Default::default()
            }
        } else {
            serde_json::from_str(&r.open_media_json)
                .map_err(|e| st(Error::InvalidArgument(format!("open_media_json: {e}"))))?
        };
        let (client, ws_headers) = self.media_upstream(&r.name).await.map_err(st)?;
        let mut resp = client
            .stream()
            .open_media(open)
            .await
            .map_err(|s| st(cua_spacesd_client::Error::from(s).into()))?
            .into_inner();
        let ws_path = if resp.ws_path.is_empty() {
            format!(
                "{}?ticket={}",
                cua_proto::metadata::MEDIA_WS_PATH,
                resp.ticket
            )
        } else {
            resp.ws_path.clone()
        };
        let upstream_url = client.endpoint().ws_url(&ws_path);
        let ticket = crate::random_token();
        let expires = SystemTime::now() + self.0.bridge_ttl;
        {
            let mut bridges = self.0.bridges.lock().unwrap();
            let now = SystemTime::now();
            bridges.retain(|_, b| b.expires > now);
            bridges.insert(
                ticket.clone(),
                Bridge {
                    upstream_url,
                    headers: ws_headers,
                    expires,
                },
            );
        }
        resp.ticket = String::new();
        resp.ws_path = String::new();
        let base = self.0.info.lock().unwrap().loopback_url.clone();
        if base.is_empty() {
            return Err(st(Error::Unsupported(
                "the media bridge needs the daemon's loopback listener".into(),
            )));
        }
        let ws_base = base.replacen("http://", "ws://", 1);
        ok(pb::OpenMediaBridgeResponse {
            ws_url: format!("{ws_base}/v1/bridge/media?ticket={ticket}"),
            ticket,
            expires_at: Some(convert::ts(expires)),
            open_media_response_json: serde_json::to_string(&resp)
                .map_err(|e| st(Error::Internal(e.to_string())))?,
        })
    }

    async fn shutdown(&self, _: tonic::Request<pb::ShutdownRequest>) -> R<pb::ShutdownResponse> {
        let tx = self.0.shutdown.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = tx.send(true);
        });
        ok(pb::ShutdownResponse {})
    }
}

impl Shared {
    /// Registers a bridge directly (tests and in-process hosts).
    pub(crate) fn take_bridge(&self, ticket: &str) -> Option<Bridge> {
        let bridges = self.bridges.lock().unwrap();
        bridges
            .get(ticket)
            .filter(|b| b.expires > SystemTime::now())
            .cloned()
    }
}
