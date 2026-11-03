//! `cua-compat`: run other sandbox providers' SDKs, unchanged, on Cua
//! Spaces. A local gateway speaks the provider's wire protocol and serves
//! it from Cua Spaces through the cua SDK; see `DESIGN.md`.
//!
//! ```no_run
//! # async fn f() -> Result<(), Box<dyn std::error::Error>> {
//! use cua_compat::{Gateway, GatewayOptions, Provider};
//! let gw = Gateway::start(GatewayOptions::new(Provider::E2b), None).await?;
//! for (k, v) in gw.env() {
//!     println!("export {k}={v}");
//! }
//! gw.wait().await;
//! # Ok(()) }
//! ```

// The SDK-backed gateway futures nest deep enough to pass rustc's default (128).
#![recursion_limit = "256"]

pub mod auth;
pub mod backend;
pub mod connect;
pub mod daytona;
pub mod e2b;
pub mod guest;
pub mod guestfs;
pub mod host;
pub mod journal;
pub mod lifetime;
pub mod modal;
pub mod state;
pub mod tls;

#[cfg(test)]
mod proptests;

use crate::auth::Keys;
use crate::backend::{Backend, CuaBackend, SpaceDefaults};
use crate::state::Store;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

/// A wire protocol the gateway speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// The E2B SDK (REST control plane + envd).
    E2b,
    /// The Daytona SDK (REST API + toolbox + Socket.IO events).
    Daytona,
    /// The Modal clients (gRPC control plane + task command router).
    Modal,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::E2b => "e2b",
            Provider::Daytona => "daytona",
            Provider::Modal => "modal",
        }
    }

    pub fn parse(s: &str) -> Option<Provider> {
        match s.trim().to_ascii_lowercase().as_str() {
            "e2b" => Some(Provider::E2b),
            "daytona" => Some(Provider::Daytona),
            "modal" => Some(Provider::Modal),
            _ => None,
        }
    }

    /// Default API port (E2B's envd port minus one) and host-router port.
    fn default_ports(self) -> (u16, u16) {
        match self {
            Provider::E2b => (49982, 49443),
            Provider::Daytona => (49984, 49444),
            Provider::Modal => (49985, 49445),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Tls(#[from] tls::TlsError),
    #[error("{0}")]
    Config(String),
}

/// How to run a gateway.
#[derive(Debug, Clone)]
pub struct GatewayOptions {
    pub provider: Provider,
    /// API (and envd) listener. Default `127.0.0.1:49982`.
    pub listen: SocketAddr,
    /// Host-router listener (TLS and plain) for `<port>-<id>.<domain>`.
    /// Default port 49443 on loopback (IPv4 and IPv6).
    pub host_port: u16,
    /// Bind the host router on all loopback families (`127.0.0.1`, `::1`);
    /// with `listen` on a LAN address it binds that address instead.
    pub domain: String,
    /// Where keys, state and TLS files live (default
    /// `$CUA_HOME/compat/<provider>`).
    pub dir: Option<PathBuf>,
    /// Placement and size of new Spaces.
    pub defaults: SpaceDefaults,
    /// The guest user for the provider's default user.
    pub default_user: String,
    /// Create the default user in image-based sandboxes that lack it.
    pub create_default_user: bool,
}

impl Provider {
    /// The provider's own default guest user.
    pub fn default_user(self) -> &'static str {
        match self {
            Provider::E2b => "user",
            Provider::Daytona => "daytona",
            Provider::Modal => "root",
        }
    }

    /// API key prefix.
    pub fn key_prefix(self) -> &'static str {
        match self {
            Provider::E2b => "e2b_",
            Provider::Daytona => "dtn_",
            Provider::Modal => "modal_",
        }
    }
}

impl GatewayOptions {
    pub fn new(provider: Provider) -> Self {
        let (api, host) = provider.default_ports();
        Self {
            provider,
            listen: SocketAddr::from(([127, 0, 0, 1], api)),
            host_port: host,
            domain: format!("{}.localhost", provider.name()),
            dir: None,
            defaults: SpaceDefaults {
                // E2B's base template is 2 vCPU; give its sandboxes enough
                // memory for the quickstarts (pip installs, a browser).
                // Daytona's default sandbox is 1 vCPU / 1 GiB (3 GiB disk).
                cpus: match provider {
                    Provider::E2b => Some(2),
                    Provider::Daytona => Some(1),
                    _ => None,
                },
                memory_mb: match provider {
                    Provider::E2b => Some(2048),
                    Provider::Daytona => Some(1024),
                    _ => None,
                },
                ..SpaceDefaults::default()
            },
            default_user: provider.default_user().into(),
            create_default_user: true,
        }
    }

    pub fn dir(&self) -> PathBuf {
        self.dir
            .clone()
            .unwrap_or_else(|| auth::provider_dir(self.provider.name()))
    }

    /// The domain clients see, with the host-router port unless it is 443.
    pub fn client_domain(&self, host_port: u16) -> String {
        if host_port == 443 {
            self.domain.clone()
        } else {
            format!("{}:{host_port}", self.domain)
        }
    }
}

/// A running gateway.
pub struct Gateway {
    pub provider: Provider,
    pub api_addr: SocketAddr,
    pub host_addrs: Vec<SocketAddr>,
    /// The first API key (what `env()` exports).
    pub api_key: String,
    pub domain: String,
    pub bundle: PathBuf,
    pub dir: PathBuf,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Gateway {
    /// Starts the listeners. `backend`: `None` uses Cua Spaces through the
    /// cua SDK ([`CuaBackend`]).
    pub async fn start(
        opts: GatewayOptions,
        backend: Option<Arc<dyn Backend>>,
    ) -> Result<Gateway, GatewayError> {
        let dir = opts.dir();
        std::fs::create_dir_all(&dir)?;
        tls::restrict_dir(&dir)?;
        let keys = Keys::load_or_create(
            &dir.join("keys.json"),
            &format!("{}_", opts.provider.name()),
        )?;
        let api_key = keys.keys[0].key.clone();
        let keys = Arc::new(RwLock::new(keys));
        let store = Store::open(Some(dir.join("state.json")))?;
        let backend: Arc<dyn Backend> =
            backend.unwrap_or_else(|| Arc::new(CuaBackend::new(opts.defaults.clone())));
        // Journal every Space this gateway makes (see journal.rs): a kill
        // or a daemon outage can no longer leave one nothing owns.
        let journal = journal::Journal::open(Some(dir.join("owned.jsonl")))?;
        let modal_cell: Arc<std::sync::OnceLock<Arc<modal::Modal>>> = Arc::default();
        let recorded: Option<journal::Recorded> = match opts.provider {
            Provider::E2b | Provider::Daytona => {
                let s = store.clone();
                Some(Arc::new(move || {
                    let s = s.clone();
                    Box::pin(async move { s.all().await.into_iter().map(|r| r.cua_name).collect() })
                }))
            }
            // Modal keeps its sandboxes in its own object database; until the
            // gateway is up everything owned is claimed.
            Provider::Modal => {
                let cell = modal_cell.clone();
                let j = journal.clone();
                Some(Arc::new(move || {
                    let cell = cell.clone();
                    let j = j.clone();
                    Box::pin(async move {
                        let owned = j.owned_names();
                        match cell.get() {
                            Some(m) => m.claimed_spaces(&owned).await,
                            None => owned.into_iter().collect(),
                        }
                    })
                }))
            }
        };
        let journaled = journal::Journaled::new(backend, journal, recorded);
        let backend: Arc<dyn Backend> = journaled.clone();

        let api_listener = TcpListener::bind(opts.listen).await?;
        let api_addr = api_listener.local_addr()?;
        let lan = !opts.listen.ip().is_loopback();
        let host_ips: Vec<std::net::IpAddr> = if lan {
            vec![opts.listen.ip()]
        } else {
            vec![[127, 0, 0, 1].into(), std::net::Ipv6Addr::LOCALHOST.into()]
        };
        let mut host_listeners = Vec::new();
        let mut host_port = opts.host_port;
        for ip in host_ips {
            match TcpListener::bind(SocketAddr::new(ip, host_port)).await {
                Ok(l) => {
                    host_port = l.local_addr()?.port();
                    host_listeners.push(l);
                }
                // No IPv6 loopback on this host: IPv4 serves.
                Err(e) if ip.is_ipv6() && !host_listeners.is_empty() => {
                    tracing::debug!("compat: no [::1]:{host_port} listener: {e}");
                }
                Err(e) => return Err(e.into()),
            }
        }
        let host_addrs = host_listeners
            .iter()
            .map(|l| l.local_addr())
            .collect::<Result<Vec<_>, _>>()?;
        let domain = opts.client_domain(host_port);
        let tls_files = tls::TlsFiles::new(dir.join("tls"));
        let tls_config = tls::ensure(&tls_files, &opts.domain)?;

        let (app, resolver) = match opts.provider {
            Provider::E2b => {
                let templates = load_templates(&dir.join("templates.json"))?;
                let cfg = e2b::E2bConfig {
                    sandbox_domain: domain.clone(),
                    default_user: opts.default_user.clone(),
                    create_default_user: opts.create_default_user,
                    templates,
                };
                let e = e2b::E2b::new(
                    cfg,
                    backend.clone(),
                    store.clone(),
                    keys.clone(),
                    Some(dir.clone()),
                );
                let resolver: Arc<dyn host::PortResolver> = Arc::new(e2b::Ports(e.clone()));
                (e2b::router(e), resolver)
            }
            Provider::Daytona => {
                let templates = load_templates(&dir.join("templates.json"))?;
                let cfg = daytona::DaytonaConfig {
                    toolbox_url: format!("http://{api_addr}/api/toolbox"),
                    preview_domain: domain.clone(),
                    default_user: opts.default_user.clone(),
                    create_default_user: opts.create_default_user,
                    snapshots_file: Some(dir.join("snapshots.json")),
                    dir: Some(dir.clone()),
                };
                let d = daytona::Daytona::new(
                    cfg,
                    backend.clone(),
                    store.clone(),
                    keys.clone(),
                    templates,
                );
                let resolver: Arc<dyn host::PortResolver> = Arc::new(daytona::Ports(d.clone()));
                (daytona::router(d), resolver)
            }
            Provider::Modal => {
                if !api_addr.ip().is_loopback() {
                    return Err(GatewayError::Config(
                        "the Modal clients accept a plain-HTTP task command router only on loopback; serve the Modal gateway on 127.0.0.1 (a job Space reaches it through `cua run --compat modal`)".into(),
                    ));
                }
                let m = modal::start(
                    &opts,
                    &dir,
                    &format!("http://{api_addr}"),
                    &domain,
                    &api_key,
                    backend.clone(),
                    keys.clone(),
                )
                .await?;
                let _ = modal_cell.set(m.clone());
                let resolver: Arc<dyn host::PortResolver> = Arc::new(modal::Ports(m.clone()));
                (m.router(), resolver)
            }
        };

        state::reconcile(&store, backend.as_ref()).await;
        let mut tasks = Vec::new();
        tasks.push(tokio::spawn(state::reaper(store.clone(), backend.clone())));
        tasks.push(tokio::spawn(journal::sweeper(journaled)));
        tasks.push(tokio::spawn(lifetime::sync(store.clone(), backend.clone())));
        let router = host::HostRouter {
            domain: opts.domain.clone(),
            envd_port: match opts.provider {
                Provider::E2b => e2b::ENVD_PORT,
                Provider::Daytona | Provider::Modal => 0,
            },
            tls: tls_config,
            app,
            resolver,
        };
        {
            let r = router.clone();
            tasks.push(tokio::spawn(
                async move { serve_api(api_listener, r).await },
            ));
        }
        for l in host_listeners {
            let r = router.clone();
            tasks.push(tokio::spawn(async move { r.serve(l).await }));
        }
        Ok(Gateway {
            provider: opts.provider,
            api_addr,
            host_addrs,
            api_key,
            domain,
            bundle: tls_files.bundle(),
            dir,
            tasks,
        })
    }

    /// What a client exports to use this gateway with zero code changes.
    pub fn env(&self) -> Vec<(String, String)> {
        client_env(
            self.provider,
            self.api_addr,
            &self.api_key,
            &self.domain,
            &self.bundle,
        )
    }

    /// [`Gateway::start`], then [`Gateway::warm`]: returns (so a caller can
    /// announce the gateway) only once the first create has been made. A
    /// warm-up failure is logged and does not stop the gateway.
    pub async fn start_warmed(
        opts: GatewayOptions,
        backend: Option<Arc<dyn Backend>>,
    ) -> Result<Gateway, GatewayError> {
        let gw = Gateway::start(opts, backend).await?;
        if let Err(e) = gw.warm().await {
            eprintln!("compat: warming the {} gateway: {e}", gw.provider.name());
        }
        Ok(gw)
    }

    /// Makes the first sandbox of the default shape and deletes it, so the
    /// image pull and the first boot (and E2B's prepared base) happen now
    /// rather than inside an SDK's create, which gives up after 60 s. `cua
    /// run --compat` calls this before the program starts. Best effort: an
    /// error is returned for the caller to log, and creates still work.
    pub async fn warm(&self) -> Result<(), String> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(900))
            .build()
            .map_err(|e| e.to_string())?;
        let api = format!("http://{}", self.api_addr);
        let (create, auth, body, id_field) = match self.provider {
            Provider::E2b => (
                format!("{api}/sandboxes"),
                ("x-api-key", self.api_key.clone()),
                serde_json::json!({"templateID": "base", "timeout": 300}),
                "sandboxID",
            ),
            Provider::Daytona => (
                format!("{api}/api/sandbox"),
                ("authorization", format!("Bearer {}", self.api_key)),
                serde_json::json!({}),
                "id",
            ),
            Provider::Modal => return Ok(()),
        };
        let r = http
            .post(&create)
            .header(auth.0, &auth.1)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = r.status();
        let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
        let id = v[id_field]
            .as_str()
            .ok_or_else(|| format!("warm-up create: {status}: {v}"))?
            .to_string();
        let del = match self.provider {
            Provider::E2b => format!("{api}/sandboxes/{id}"),
            _ => format!("{api}/api/sandbox/{id}"),
        };
        http.delete(del)
            .header(auth.0, &auth.1)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Runs until the listeners stop (they do not, short of an error).
    pub async fn wait(mut self) {
        for t in std::mem::take(&mut self.tasks) {
            let _ = t.await;
        }
    }

    /// Stops the listeners (sandboxes and records stay).
    pub fn stop(&self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The exports for a client of a gateway at `api_addr` (also what
/// `cua compat env` prints without starting one).
pub fn client_env(
    provider: Provider,
    api_addr: SocketAddr,
    api_key: &str,
    domain: &str,
    bundle: &std::path::Path,
) -> Vec<(String, String)> {
    let bundle = bundle.to_string_lossy().into_owned();
    match provider {
        Provider::E2b => {
            let url = format!("http://{api_addr}");
            vec![
                ("E2B_API_URL".into(), url.clone()),
                ("E2B_SANDBOX_URL".into(), url),
                ("E2B_API_KEY".into(), api_key.to_string()),
                ("E2B_DOMAIN".into(), domain.to_string()),
                ("SSL_CERT_FILE".into(), bundle),
            ]
        }
        Provider::Daytona => vec![
            ("DAYTONA_API_URL".into(), format!("http://{api_addr}/api")),
            ("DAYTONA_API_KEY".into(), api_key.to_string()),
            ("SSL_CERT_FILE".into(), bundle.clone()),
            // Node ignores SSL_CERT_FILE: the TypeScript SDK's uploads of
            // Image build contexts (HTTPS to the gateway) need this.
            ("NODE_EXTRA_CA_CERTS".into(), bundle),
        ],
        Provider::Modal => vec![
            ("MODAL_SERVER_URL".into(), format!("http://{api_addr}")),
            ("MODAL_TOKEN_ID".into(), modal::token_id(api_key)),
            ("MODAL_TOKEN_SECRET".into(), api_key.to_string()),
            ("SSL_CERT_FILE".into(), bundle),
        ],
    }
}

/// Serves the API on plain TCP (HTTP/1.1 and h2c); requests for a sandbox's
/// guest port are piped there ([`host::HostRouter::api_connection`]).
async fn serve_api(listener: TcpListener, router: host::HostRouter) {
    loop {
        let (sock, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("compat: accept: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };
        let r = router.clone();
        tokio::spawn(async move {
            if let Err(e) = r.api_connection(sock).await {
                tracing::debug!("compat: api connection: {e}");
            }
        });
    }
}

/// `templates.json`: `{"<e2b template or snapshot id>": {"image": "..."} |
/// {"checkpoint": "..."}}`. Missing: none.
pub fn load_templates(
    path: &std::path::Path,
) -> Result<HashMap<String, e2b::TemplateTarget>, GatewayError> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| GatewayError::Config(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod default_size_tests {
    use super::*;

    /// Each provider's plain form (`cua run --compat P -- app`) creates the
    /// size that provider's own default sandbox has, and no more.
    #[test]
    fn the_default_sandbox_is_the_providers_own_size() {
        let d = GatewayOptions::new(Provider::Daytona).defaults;
        assert_eq!((d.cpus, d.memory_mb), (Some(1), Some(1024)));
        let e = GatewayOptions::new(Provider::E2b).defaults;
        assert_eq!((e.cpus, e.memory_mb), (Some(2), Some(2048)));
        assert!(d.on.is_empty(), "no --on: the core default placement");
    }
}
