// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The pool agent in `cua daemon` ([`cua_spaces_cluster::pool`]): every
//! signed-in machine runs one, so `cua sb run --on auto` uses all of the
//! account's machines with nothing to set up. This module is the glue to
//! the daemon: the signed-in account and its relay, the account's machines
//! that provide Spaces and how to reach them, this machine's own job
//! runner, and Modal when it is connected here.

use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cua_cluster::link::{GrpcLink, LinkError, LocalLink, NodeLink};
use cua_daemon::extension::{AttachedExtension, DaemonExtension, ExtensionContext};
use cua_spaces_cluster::coordinator::LinkFactory;
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::pool::keys::DeviceSigner;
use cua_spaces_cluster::pool::relay::{PoolAuth, RelayPool};
use cua_spaces_cluster::pool::seal::PoolSecret;
use cua_spaces_cluster::pool::{AgentConfig, Host, PoolAgent, PoolMachine};
use serde_json::{Value, json};

/// Turns the pool agent off for a daemon (`0`): tests and fixtures.
pub const ENV_POOL: &str = "CUA_POOL";

/// The daemon extension.
pub struct PoolExtension;

/// Registers it for every runtime this process builds.
pub fn register() {
    cua_daemon::extension::register(Arc::new(PoolExtension));
}

impl Attached {
    /// Why a request placed on a cloud (`on: cloud:aws`) cannot run: that
    /// cloud is not linked here.
    async fn unlinked_cloud(&self, params: &Value) -> Option<String> {
        let on = params["on"].as_str().or(params["spec"]["on"].as_str())?;
        let p = cua_cluster::placement::cloud_pin(on)?;
        let linked = match self.spaces.sandboxes().clouds() {
            Ok(c) => c.status(Some(p)).await.is_ok_and(|s| {
                s.providers
                    .iter()
                    .any(|x| x.name.eq_ignore_ascii_case(p) && x.connected)
            }),
            Err(_) => false,
        };
        (!linked).then(|| format!("--on {p}: {p} is not linked here; run `cua cloud connect {p}`"))
    }
}

impl DaemonExtension for PoolExtension {
    fn name(&self) -> &str {
        "cua-pool"
    }

    fn attach(
        &self,
        spaces: &cua_spaces::Spaces,
        cx: &ExtensionContext<'_>,
    ) -> Option<Arc<dyn AttachedExtension>> {
        let agent = Arc::new(tokio::sync::OnceCell::new());
        // Durable Spaces are kept by whichever machine coordinates.
        spaces.set_pool(Some(Arc::new(DurableRequests(agent.clone()))));
        Some(Arc::new(Attached {
            spaces: spaces.clone(),
            home: cx.home.to_path_buf(),
            agent,
        }))
    }
}

struct Attached {
    spaces: cua_spaces::Spaces,
    home: PathBuf,
    agent: Arc<tokio::sync::OnceCell<Arc<PoolAgent>>>,
}

impl AttachedExtension for Attached {
    fn daemon_tasks(
        &self,
        runtime: &cua_daemon::Runtime,
        _socket_dir: Option<&Path>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        if std::env::var(ENV_POOL).is_ok_and(|v| v.trim() == "0") {
            return vec![];
        }
        let host = Arc::new(DaemonHost {
            spaces: self.spaces.clone(),
            home: self.home.clone(),
            runtime_host: runtime.host_spaces(),
            relays: Mutex::default(),
            os: Arc::default(),
            provides: Mutex::default(),
            signer: Mutex::default(),
            secret: std::sync::OnceLock::new(),
        });
        let agent = self.agent.clone();
        let dir = self.home.join("pool");
        vec![tokio::spawn(async move {
            match PoolAgent::start(dir, host, AgentConfig::default()).await {
                Ok(a) => {
                    let _ = agent.set(a);
                }
                Err(e) => tracing::error!("the pool agent did not start: {e}"),
            }
        })]
    }

    fn shutdown<'a>(
        &'a self,
        _runtime: &'a cua_daemon::Runtime,
    ) -> cua_spaces::extension::BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(a) = self.agent.get() {
                a.shutdown().await;
            }
        })
    }

    fn pool_request<'a>(
        &'a self,
        target: &'a str,
        method: &'a str,
        params: Value,
    ) -> Option<cua_spaces::extension::BoxFuture<'a, (u16, Value)>> {
        Some(Box::pin(async move {
            // The agent starts with the daemon: wait for it, bounded.
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
            let agent = loop {
                if let Some(a) = self.agent.get() {
                    break a.clone();
                }
                if tokio::time::Instant::now() >= deadline {
                    return (
                        503,
                        json!({"error": "this machine's pool agent is not running (see the daemon log)"}),
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            };
            if let Some(why) = self.unlinked_cloud(&params).await {
                return (400, json!({ "error": why }));
            }
            if method == "pool" {
                return match agent.pool_status().await {
                    Some(v) => (200, v),
                    None => (200, json!({"relay": false})),
                };
            }
            agent.request(target, method, params).await
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Durable Spaces' requests to the coordinator of the account's machines.
struct DurableRequests(Arc<tokio::sync::OnceCell<Arc<PoolAgent>>>);

impl cua_spaces::durable::PoolRequests for DurableRequests {
    fn request<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u16, Value)> + Send + 'a>> {
        Box::pin(async move {
            let Some(agent) = wait_agent(&self.0).await else {
                return (
                    503,
                    json!({"error": "this machine's pool agent is not running (see the daemon log)"}),
                );
            };
            agent.request("pool", method, params).await
        })
    }
}

/// The agent, once it started (bounded wait).
async fn wait_agent(cell: &tokio::sync::OnceCell<Arc<PoolAgent>>) -> Option<Arc<PoolAgent>> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(a) = cell.get() {
            return Some(a.clone());
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// The account token and this device's session, per call.
struct AccountAuth(cua_spaces::relay::RelayAccount);

#[async_trait]
impl PoolAuth for AccountAuth {
    async fn headers(&self) -> Result<Vec<(String, String)>, String> {
        let token = self
            .0
            .tokens
            .access_token()
            .await
            .map_err(|e| format!("signing in to the relay: {e}"))?;
        let mut h = vec![("authorization".to_string(), format!("Bearer {token}"))];
        if let Some(s) = self.0.device_session().await {
            h.push((cua_host::relay::DEVICE_SESSION_HEADER.to_string(), s));
        }
        Ok(h)
    }

    async fn reset(&self) {
        if let Some(d) = &self.0.device {
            d.reset_session().await;
        }
    }
}

struct DaemonHost {
    spaces: cua_spaces::Spaces,
    home: PathBuf,
    runtime_host: Option<Arc<cua_spaces::host_spaces::HostSpacesServer>>,
    /// The pool per relay URL, once its account is known.
    relays: Mutex<HashMap<String, (Option<String>, RelayPool)>>,
    /// Each machine's OS, once it answered.
    os: Arc<Mutex<HashMap<String, String>>>,
    /// Whether a machine whose setup said otherwise provides Spaces now,
    /// and when it was asked.
    provides: Mutex<HashMap<String, (std::time::Instant, bool)>>,
    /// This device's key on a relay (by its URL), and when it was read.
    signer: Mutex<Option<CachedSigner>>,
    /// This device's pool key.
    secret: std::sync::OnceLock<PoolSecret>,
}

/// A relay URL, when its device key was read, and the key.
type CachedSigner = (
    String,
    std::time::Instant,
    Option<String>,
    Arc<dyn DeviceSigner>,
);

/// The name of this device's pool key in the credential store (the OS
/// vault for Cua-signed builds, else a 0600 file), next to its device key.
const POOL_KEY_SECRET: &str = "pool-key";

/// The enrolled device key certifies the pool key.
struct DeviceKeySigner(cua_host::DeviceKey);

impl DeviceSigner for DeviceKeySigner {
    fn device(&self) -> String {
        self.0.id()
    }

    fn public_key(&self) -> String {
        self.0.public_key()
    }

    fn sign(&self, message: &str) -> Result<String, String> {
        self.0.sign(message).map_err(|e| e.to_string())
    }
}

impl DaemonHost {
    fn host_config(&self) -> Option<cua_host::HostConfig> {
        cua_host::Host::new(&self.home).config().ok().flatten()
    }

    /// The account this daemon acts as on the relay (as machines see it).
    async fn account(&self) -> Option<String> {
        let pool = self.relay().await?;
        Some(pool.key().rsplit_once('#')?.1.to_string())
    }

    fn connector(&self, space: String) -> cua_cluster::link::Connector {
        let spaces = self.spaces.clone();
        Arc::new(move || {
            let (spaces, space) = (spaces.clone(), space.clone());
            Box::pin(async move {
                let s = spaces
                    .space(&space)
                    .await
                    .map_err(|e| LinkError::Unreachable(e.to_string()))?;
                s.spacesd()
                    .cloned()
                    .map_err(|e| LinkError::Unreachable(e.to_string()))
            })
        })
    }

    async fn provides_now(&self, m: &cua_spaces::relay::RelayMachine) -> bool {
        if !m.online {
            return false;
        }
        if let Some((at, yes)) = self.provides.lock().expect("provides").get(&m.id)
            && at.elapsed() < std::time::Duration::from_secs(60)
        {
            return *yes;
        }
        let link = GrpcLink::with_connector(self.connector(format!("relay:{}", m.id)), "probe");
        let yes = matches!(
            tokio::time::timeout(std::time::Duration::from_secs(8), link.host()).await,
            Ok(Ok(h)) if h.settings.is_some_and(|s| s.provide_spaces)
        );
        self.provides
            .lock()
            .expect("provides")
            .insert(m.id.clone(), (std::time::Instant::now(), yes));
        yes
    }

    async fn os_of(&self, id: &str, link: &Arc<dyn NodeLink>) -> String {
        if let Some(os) = self.os.lock().expect("os").get(id) {
            return os.clone();
        }
        match tokio::time::timeout(std::time::Duration::from_secs(8), link.host()).await {
            Ok(Ok(h)) if !h.os.is_empty() => {
                self.os
                    .lock()
                    .expect("os")
                    .insert(id.to_string(), h.os.clone());
                h.os
            }
            _ => String::new(),
        }
    }
}

#[async_trait]
impl Host for DaemonHost {
    async fn relay(&self) -> Option<RelayPool> {
        let account = self.spaces.relay_account()?;
        // Which account this is: the pool is the account's, so another
        // sign-in needs its own (the relay says its id below).
        let who = account.account_subject().await;
        if let Some((started, p)) = self.relays.lock().expect("relays").get(&account.url)
            && !cua_spaces::relay::account_changed(started, &who)
        {
            return Some(p.clone());
        }
        let auth: Arc<dyn PoolAuth> = Arc::new(AccountAuth(account.clone()));
        let probe = RelayPool::new(&account.url, "", auth.clone());
        // The relay says which account this is (and whether it has pools).
        let id = match probe.status().await {
            Ok(v) => v["account"].as_str().unwrap_or_default().to_string(),
            Err(cua_spaces_cluster::pool::relay::PoolError::Unsupported(_)) => String::new(),
            Err(_) => return None,
        };
        let pool = RelayPool::new(&account.url, &id, auth);
        self.relays
            .lock()
            .expect("relays")
            .insert(account.url.clone(), (who, pool.clone()));
        Some(pool)
    }

    fn name(&self) -> String {
        cua_host::device_name()
    }

    fn machine_id(&self) -> String {
        self.spaces.this_relay_machine_id().unwrap_or_default()
    }

    fn provides_spaces(&self) -> bool {
        self.host_config()
            .is_some_and(|c| c.provide_spaces && c.sharing && !c.paused_signed_out)
    }

    async fn account_machines(&self) -> Result<Vec<PoolMachine>, String> {
        if self.spaces.relay_account().is_none() {
            return Ok(vec![]);
        }
        let machines = self
            .spaces
            .relay_machines()
            .await
            .map_err(|e| e.to_string())?;
        let mine = self.machine_id();
        let account = self.account().await.unwrap_or_default();
        // The clouds that supplied machines (`cua cloud hosts`), by machine.
        let clouds = self.cloud_machines().await;
        // Every machine at once: one that is gone (its probe times out)
        // must not hold up the others, nor a machine taking over the pool.
        let all = machines
            .into_iter()
            .map(|m| self.pool_machine(m, &mine, &account));
        let mut out: Vec<PoolMachine> = futures_util::future::join_all(all)
            .await
            .into_iter()
            .flatten()
            .collect();
        for m in &mut out {
            if let Some(p) = clouds
                .get(&m.row.relay_machine)
                .or_else(|| clouds.get(&m.row.name))
            {
                m.row.provider = p.clone();
            }
        }
        Ok(out)
    }

    async fn direct_machines(&self) -> Vec<PoolMachine> {
        let Ok(hosts) = self.spaces.registry().direct_hosts() else {
            return vec![];
        };
        let mut out = Vec::new();
        for h in hosts {
            let link: Arc<dyn NodeLink> = Arc::new(GrpcLink::with_connector(
                self.connector(h.space.clone()),
                format!("direct {} ({})", h.name, h.space),
            ));
            let os = self.os_of(&h.space, &link).await;
            out.push(PoolMachine {
                row: NodeRow {
                    name: h.name.clone(),
                    direct: h.space.trim_start_matches("direct:").to_string(),
                    via: "direct".into(),
                    state: "ready".into(),
                    os,
                    ..Default::default()
                },
                link,
            });
        }
        out
    }

    async fn local_runner(&self) -> Option<Arc<dyn NodeLink>> {
        self.runner("local").await
    }

    fn elastic(&self) -> Option<(NodeRow, LinkFactory)> {
        elastic(&self.home)
    }

    async fn device_signer(&self, relay: &RelayPool) -> Option<Arc<dyn DeviceSigner>> {
        let account = self.spaces.relay_account()?;
        let url = account.url.trim_end_matches('/').to_string();
        if url != relay.base() {
            return None;
        }
        let who = account.account_subject().await;
        if let Some((u, at, started, s)) = self.signer.lock().expect("signer").as_ref()
            && *u == url
            && at.elapsed() < std::time::Duration::from_secs(60)
            && !cua_spaces::relay::account_changed(started, &who)
        {
            return Some(s.clone());
        }
        let key = account.device.as_ref()?.key().ok()??;
        let s: Arc<dyn DeviceSigner> = Arc::new(DeviceKeySigner(key));
        *self.signer.lock().expect("signer") =
            Some((url, std::time::Instant::now(), who, s.clone()));
        Some(s)
    }

    fn pool_secret(&self) -> Result<PoolSecret, String> {
        if let Some(s) = self.secret.get() {
            return Ok(s.clone());
        }
        let store = cua_auth::Store::from_env();
        let s = match store
            .load_secret(POOL_KEY_SECRET)
            .map_err(|e| e.to_string())?
        {
            Some(b) => PoolSecret::from_bytes(&b).map_err(|e| e.to_string())?,
            None => {
                let s = PoolSecret::generate();
                store
                    .save_secret(POOL_KEY_SECRET, &s.to_bytes())
                    .map_err(|e| e.to_string())?;
                s
            }
        };
        Ok(self.secret.get_or_init(|| s).clone())
    }
}

/// A machine of the account as the pool reaches it: its jobs through
/// `jobs` (this machine's own runner in process, or the relay), its host
/// service through the relay, and the Spaces the pool keeps there created
/// the way `--on host:<machine>` creates them (registered on the relay as
/// the account's, so any of its devices reaches them).
struct SpacesLink {
    jobs: Arc<dyn NodeLink>,
    host: Arc<dyn NodeLink>,
    spaces: cua_spaces::Spaces,
    machine: String,
}

#[async_trait]
impl NodeLink for SpacesLink {
    async fn run(
        &self,
        req: cua_proto::env::v1::RunHostJobsRequest,
    ) -> Result<cua_proto::env::v1::RunHostJobsResponse, LinkError> {
        self.jobs.run(req).await
    }

    async fn get(&self, job_id: &str) -> Result<cua_proto::env::v1::GetHostJobResponse, LinkError> {
        self.jobs.get(job_id).await
    }

    async fn get_output(
        &self,
        job_id: &str,
        from: u64,
    ) -> Result<cua_proto::env::v1::GetHostJobResponse, LinkError> {
        self.jobs.get_output(job_id, from).await
    }

    async fn list(
        &self,
        req: cua_proto::env::v1::ListHostJobsRequest,
    ) -> Result<cua_proto::env::v1::ListHostJobsResponse, LinkError> {
        self.jobs.list(req).await
    }

    async fn cancel(
        &self,
        req: cua_proto::env::v1::CancelHostJobRequest,
    ) -> Result<cua_proto::env::v1::CancelHostJobResponse, LinkError> {
        self.jobs.cancel(req).await
    }

    async fn host(&self) -> Result<cua_proto::env::v1::GetHostSpacesResponse, LinkError> {
        self.host.host().await
    }

    async fn create_space(
        &self,
        req: cua_proto::env::v1::CreateHostSpaceRequest,
    ) -> Result<cua_proto::env::v1::HostSpace, LinkError> {
        use cua_sandbox_core::placement::{Kind, On, Runtime};
        if req.direct {
            return self.host.create_space(req).await;
        }
        let refused = |e: String| LinkError::Refused(e);
        let network = cua_sandbox_core::NetworkMode::parse(&req.network)
            .map_err(|e| refused(e.to_string()))?;
        let created = self
            .spaces
            .create(cua_spaces::SpaceCreate {
                image: (!req.image.is_empty()).then(|| req.image.clone()),
                on: Some(On::Host(self.machine.clone())),
                kind: Kind::parse(&req.kind).map_err(|e| refused(e.message))?,
                runtime: Runtime::parse(&req.runtime).map_err(|e| refused(e.message))?,
                name: (!req.name.is_empty()).then(|| req.name.clone()),
                cpus: (req.cpus > 0).then_some(req.cpus),
                memory_mb: (req.memory_mb > 0).then_some(req.memory_mb),
                wait: Some(true),
                // Enforced by the host or refused there, never dropped.
                network,
                headless: req.headless,
                ..Default::default()
            })
            .await
            .map_err(|e| refused(e.to_string()))?
            .ready()
            .map_err(|p| refused(format!("{} is still starting", p.id)))?;
        let relay_machine = created
            .id
            .strip_prefix("relay:")
            .unwrap_or(&created.id)
            .to_string();
        // Its name on the host, to tell later whether it is still there.
        let local_space = match self.host.host().await {
            Ok(h) => h
                .spaces
                .into_iter()
                .find(|s| s.relay_machine == relay_machine)
                .map(|s| s.local_space),
            Err(_) => None,
        }
        .unwrap_or_else(|| format!("local:{}", req.name));
        Ok(cua_proto::env::v1::HostSpace {
            relay_machine,
            local_space,
            name: created.name,
            image: req.image,
            kind: created.kind,
            network: if network == cua_sandbox_core::NetworkMode::None {
                "none".into()
            } else {
                String::new()
            },
            ..Default::default()
        })
    }

    async fn delete_space(&self, space: &str) -> Result<String, LinkError> {
        if space.starts_with("relay:") {
            // The Space and its relay registration.
            return self.spaces.delete_copy(space).await.map_err(|e| match e {
                cua_spaces::Error::NotFound(m) => LinkError::NotFound(m),
                other => LinkError::Unreachable(other.to_string()),
            });
        }
        self.host.delete_space(space).await
    }

    fn describe(&self) -> String {
        self.jobs.describe()
    }
}

impl DaemonHost {
    /// The cloud of each machine the pool added there, by relay machine id
    /// and by pool name (empty without linked clouds).
    async fn cloud_machines(&self) -> HashMap<String, String> {
        let Ok(clouds) = self.spaces.sandboxes().clouds().cloned() else {
            return HashMap::new();
        };
        let hosts = clouds.hosts().await.unwrap_or_default();
        hosts
            .into_iter()
            .filter(|h| h.state != "gone")
            .flat_map(|h| {
                let by_id =
                    (!h.machine.is_empty()).then(|| (h.machine.clone(), h.provider.clone()));
                by_id.into_iter().chain([(h.name, h.provider)])
            })
            .collect()
    }

    /// One of the account's machines as the pool reaches it (`None`: not one
    /// the pool places on).
    async fn pool_machine(
        &self,
        m: cua_spaces::relay::RelayMachine,
        mine: &str,
        account: &str,
    ) -> Option<PoolMachine> {
        // Hosts only: not a Space a host provides, not one in your own
        // cloud, not one shared to watch only, and one that says it
        // provides Spaces (or predates saying).
        if m.host.is_some()
            || m.role == "viewer"
            || cua_spaces::cloud::is_cloud_machine(&m)
            || m.id.starts_with("space-")
        {
            return None;
        }
        // Its setup said it does not provide Spaces: ask it (now and
        // then) whether it does since (`cua host config`).
        if m.meta
            .get(cua_host::META_PROVIDES_SPACES)
            .is_some_and(|v| v == "off")
            && !self.provides_now(&m).await
        {
            return None;
        }
        let remote: Arc<dyn NodeLink> = Arc::new(GrpcLink::with_connector(
            self.connector(format!("relay:{}", m.id)),
            format!("relay {} ({})", m.name, m.id),
        ));
        let jobs: Arc<dyn NodeLink> = if !mine.is_empty() && m.id == mine {
            // This machine: its own runner, as the account it acts as.
            self.runner(account).await?
        } else {
            remote.clone()
        };
        // Spaces the pool keeps (durable ones) are created there as
        // this account's, like `--on host:<machine>` does.
        let link: Arc<dyn NodeLink> = Arc::new(SpacesLink {
            jobs,
            host: remote,
            spaces: self.spaces.clone(),
            machine: m.id.clone(),
        });
        let os = if m.online {
            self.os_of(&m.id, &link).await
        } else {
            String::new()
        };
        Some(PoolMachine {
            row: NodeRow {
                name: if m.name.is_empty() {
                    m.id.clone()
                } else {
                    m.name.clone()
                },
                relay_machine: m.id.clone(),
                via: "relay".into(),
                state: if m.sharing { "ready" } else { "draining" }.into(),
                os,
                // A machine the pool added in a cloud says which (its setup
                // meta), so any machine that coordinates places `--on
                // <cloud>` work on it, not only the one that added it.
                provider: cloud_of(&m.meta),
                ..Default::default()
            },
            link,
        })
    }

    /// This machine's job runner, acting as `owner`.
    async fn runner(&self, owner: &str) -> Option<Arc<dyn NodeLink>> {
        let host = self.runtime_host.as_ref()?;
        let jobs = match host.jobs().await {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("this machine's job runner: {e}");
                return None;
            }
        };
        Some(Arc::new(
            LocalLink::new(jobs, cua_host::device_name(), std::env::consts::OS).owned_by(owner),
        ))
    }
}

/// Modal as overflow capacity: only when the user connected Modal here
/// (`cua cloud connect modal`) and this build has the Modal provider, with
/// its token and environment (`MODAL_TOKEN_ID`, `MODAL_TOKEN_SECRET`,
/// `MODAL_ENVIRONMENT`); at most `CUA_POOL_MODAL_MAX` sandboxes (default 20).
fn elastic(home: &Path) -> Option<(NodeRow, LinkFactory)> {
    let state = std::fs::read(home.join("cloud").join("state.json")).ok()?;
    let v: Value = serde_json::from_slice(&state).ok()?;
    let connected = v["connections"]
        .as_array()
        .is_some_and(|a| a.iter().any(|c| c["provider"] == "modal"));
    if !connected {
        return None;
    }
    let max: u32 = std::env::var("CUA_POOL_MODAL_MAX")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(20);
    match crate::pool_modal::factory(&home.join("pool"), max) {
        Ok(f) => Some((crate::pool_modal::row(max), f)),
        Err(e) => {
            tracing::info!("Modal is connected but cannot take overflow here: {e}");
            None
        }
    }
}

/// The cloud a relay machine says it runs in (a machine the pool added
/// there), or "" for your own machines.
fn cloud_of(meta: &std::collections::BTreeMap<String, String>) -> String {
    meta.get(cua_host::META_CLOUD).cloned().unwrap_or_default()
}

#[cfg(test)]
mod cloud_tests {
    /// A pool host is placed by the cloud its relay machine names, so the
    /// machine that coordinates need not be the one that added it.
    #[test]
    fn a_pool_host_carries_its_cloud() {
        let meta = [(cua_host::META_CLOUD.to_string(), "hetzner".to_string())].into();
        assert_eq!(super::cloud_of(&meta), "hetzner");
        assert_eq!(super::cloud_of(&Default::default()), "");
    }
}
