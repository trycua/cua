//! The `Cua` entry object and its configuration.

use super::{Fleet, Sandboxes, SpacesdClient, run, runtime};
use crate::{CuaError, Result};
use cua_auth::account::AccountApi;
use cua_daemon::{Runtime, RuntimeConfig, client::DaemonAddress, client::DaemonClient};
use std::{path::PathBuf, sync::Arc};

/// Cua account credentials and endpoints (Cua Cloud, closed, used them for
/// sandboxes; the account's billing still does). Unset fields fall back to
/// the environment (`CUA_FLEET_BASE_URL`, `CUA_TOKEN_URL`, `CUA_CLIENT_ID`,
/// `CUA_CLIENT_SECRET`, `FLEETS_TOKEN`) when `CuaConfig.fleet_from_env`.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct FleetSettings {
    /// Fleet API base URL.
    #[uniffi(default = None)]
    pub base_url: Option<String>,
    /// OAuth token URL.
    #[uniffi(default = None)]
    pub token_url: Option<String>,
    /// OAuth client id.
    #[uniffi(default = None)]
    pub client_id: Option<String>,
    /// OAuth client secret.
    #[uniffi(default = None)]
    pub client_secret: Option<String>,
    /// Static Fleet token (wins over client credentials).
    #[uniffi(default = None)]
    pub token: Option<String>,
}

/// Configuration of an embedded SDK runtime.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CuaConfig {
    /// State directory for sandbox state files (default `~/.cua/sandboxes`).
    #[uniffi(default = None)]
    pub state_dir: Option<String>,
    /// Fleet settings (merged over the environment).
    #[uniffi(default = None)]
    pub fleet: Option<FleetSettings>,
    /// Read Fleet settings from the environment.
    #[uniffi(default = true)]
    pub fleet_from_env: bool,
    /// spacesd probe timeout (default 15 s).
    #[uniffi(default = None)]
    pub env_probe_timeout_ms: Option<u32>,
    /// Spaces registry directory (default `$CUA_HOME` or `~/.cua`).
    #[uniffi(default = None)]
    pub spaces_home: Option<String>,
    /// Teleport reads app sessions under this home directory with no host
    /// side effects (tests, CI). Default: the real host.
    #[uniffi(default = None)]
    pub teleport_home: Option<String>,
    /// When the settings and environment carry no Fleet credentials, use
    /// the signed-in session (`Cua.auth()`, `cua auth login`) from the
    /// shared credential store, refreshed as needed.
    #[uniffi(default = false)]
    pub fleet_from_session: bool,
    /// Unused: managed Fleet pools (Cua Cloud, closed) kept their name
    /// cache here.
    #[uniffi(default = None)]
    pub fleet_pool_home: Option<String>,
}

impl Default for CuaConfig {
    fn default() -> Self {
        Self {
            state_dir: None,
            fleet: None,
            fleet_from_env: true,
            env_probe_timeout_ms: None,
            spaces_home: None,
            teleport_home: None,
            fleet_from_session: false,
            fleet_pool_home: None,
        }
    }
}

impl CuaConfig {
    /// The Cua account API from the settings over the environment (when
    /// `fleet_from_env`), else the signed-in session (when
    /// `fleet_from_session`). `None`: no account.
    pub(crate) fn account(&self) -> Option<AccountApi> {
        const VARS: [&str; 5] = [
            "CUA_FLEET_BASE_URL",
            "CUA_TOKEN_URL",
            "CUA_CLIENT_ID",
            "CUA_CLIENT_SECRET",
            "FLEETS_TOKEN",
        ];
        let mut vars: std::collections::HashMap<&str, String> = if self.fleet_from_env {
            VARS.iter()
                .filter_map(|k| std::env::var(k).ok().map(|v| (*k, v)))
                .collect()
        } else {
            Default::default()
        };
        if let Some(f) = &self.fleet {
            for (k, v) in [
                ("CUA_FLEET_BASE_URL", &f.base_url),
                ("CUA_TOKEN_URL", &f.token_url),
                ("CUA_CLIENT_ID", &f.client_id),
                ("CUA_CLIENT_SECRET", &f.client_secret),
                ("FLEETS_TOKEN", &f.token),
            ] {
                if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
                    vars.insert(k, v.clone());
                }
            }
        }
        let get = |k: &str| vars.get(k).cloned();
        AccountApi::from_lookup(&get).or_else(|| {
            if !self.fleet_from_session {
                return None;
            }
            let session = cua_auth::Session::from_env();
            session.credentials().ok().flatten()?;
            Some(AccountApi::new(
                &AccountApi::base_url_from_lookup(&get),
                Arc::new(session),
            ))
        })
    }
}

/// Where the SDK runtime lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CuaMode {
    /// In this process.
    Embedded,
    /// In a `cua daemon`.
    Daemon,
}

/// Identity of the SDK (and daemon, when connected).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CuaInfo {
    /// SDK library version.
    pub sdk_version: String,
    /// Topology.
    pub mode: CuaMode,
    /// Daemon version (daemon mode).
    pub daemon_version: Option<String>,
    /// Daemon pid (daemon mode).
    pub daemon_pid: Option<u32>,
    /// Daemon socket (daemon mode).
    pub socket_path: Option<String>,
    /// Daemon loopback URL (daemon mode).
    pub loopback_url: Option<String>,
    /// Compiled-in modules.
    pub features: Vec<String>,
}

#[derive(Clone)]
pub(crate) enum Backend {
    Embedded(Runtime),
    Daemon(DaemonClient),
}

pub(crate) fn features() -> Vec<String> {
    let mut f = vec![
        "env".to_string(),
        "fleet".into(),
        "sandboxes".into(),
        "daemon".into(),
    ];
    if cfg!(feature = "local") {
        f.push("local".into());
    }
    if cfg!(feature = "spaces") {
        f.push("spaces".into());
    }
    if cfg!(feature = "host") {
        f.push("host".into());
    }
    if cfg!(feature = "media") {
        f.push("media".into());
    }
    f
}

/// The entry point of the SDK: create one with `Cua::embedded` or
/// `Cua::connect`, then reach sandboxes, Fleet, Spaces and the rest
/// through its accessors.
#[derive(uniffi::Object)]
pub struct Cua {
    pub(crate) backend: Backend,
    pub(crate) config: CuaConfig,
}

impl Cua {
    /// Rust hosts: an embedded SDK over a caller-built runtime (tests,
    /// custom Fleet HTTP clients, local runtimes).
    pub fn from_runtime(runtime: Runtime) -> Arc<Self> {
        Arc::new(Self {
            backend: Backend::Embedded(runtime),
            config: CuaConfig::default(),
        })
    }

    /// Rust hosts: the embedded runtime, if any.
    pub fn embedded_runtime(&self) -> Option<&Runtime> {
        match &self.backend {
            Backend::Embedded(r) => Some(r),
            Backend::Daemon(_) => None,
        }
    }
}

#[uniffi::export]
impl Cua {
    /// Runs the SDK runtime in this process. Performs no I/O beyond
    /// reading the credential store when `fleet_from_session` is set.
    #[uniffi::constructor]
    pub fn embedded(config: CuaConfig) -> Result<Arc<Self>> {
        let rc = RuntimeConfig {
            state_dir: config.state_dir.clone().map(PathBuf::from),
            account: config.account(),
            local: None,
            vmm: Some(Arc::new(cua_daemon::local::VmmLocal::default())),
            env_probe_timeout: config.env_probe_timeout_ms.map(super::millis),
            spaces_home: config.spaces_home.clone().map(PathBuf::from),
            teleport_home: config.teleport_home.clone().map(PathBuf::from),
            // Every contrib provider this build includes (none by default).
            providers: None,
            // Extensions: the host's registered ones (`cua_daemon::extension`),
            // none in an MIT-only process.
            ..Default::default()
        };
        let rt = {
            let _guard = runtime().enter();
            Runtime::new(rc)?
        };
        Ok(Arc::new(Self {
            backend: Backend::Embedded(rt),
            config,
        }))
    }

    /// The default topology: the `cua daemon` this machine runs when one
    /// accepts connections (so the CLI, MCP clients and apps share
    /// sandboxes and Spaces), else the runtime in this process with
    /// `config`. A discovery file (`~/.cua/daemon.json`) or socket left by a
    /// daemon that exited is not a running daemon: the files are removed
    /// when their pid is provably dead, and this falls back to embedded.
    /// A daemon that is starting (`~/.cua/daemon.starting`) is waited for,
    /// up to 30 s. Probes with a local connect only (no RPC), except in a
    /// process inside
    /// an app bundle that ships its own `cua`: it uses only that build's
    /// daemon, and fails with [`CuaError::DaemonNotRunning`] naming the
    /// running one (another app's, or its own before a rebuild or update)
    /// rather than silently use it; `<bundle>/Contents/MacOS/cua daemon
    /// start` replaces it.
    #[uniffi::constructor]
    pub fn auto(config: CuaConfig) -> Result<Arc<Self>> {
        // A daemon that is starting is waited for (bounded), not raced with
        // an embedded runtime.
        match cua_daemon::client::wait_for_starting(std::time::Duration::from_secs(30))? {
            Some(addr) => {
                let client = {
                    let _guard = runtime().enter();
                    DaemonClient::new(addr)?
                };
                if cua_daemon::identity::bundled_cua().is_some() {
                    check_identity(&client)?;
                }
                Ok(Arc::new(Self {
                    backend: Backend::Daemon(client),
                    config,
                }))
            }
            None => Self::embedded(config),
        }
    }

    /// Connects to a running `cua daemon`. `address` is a socket path
    /// (`unix:` prefix optional) or a loopback URL (then `token` is
    /// required unless the discovery file has it); `None` uses
    /// `~/.cua/daemon.json`, then `~/.cua/cua.sock`. Connects lazily: the
    /// first call fails with `DaemonNotRunning` when no daemon runs; use
    /// [`Cua::info`] to check, or [`Cua::auto`] to fall back to the runtime
    /// in this process.
    #[uniffi::constructor]
    pub fn connect(address: Option<String>, token: Option<String>) -> Result<Arc<Self>> {
        let addr = DaemonAddress::resolve(address.as_deref(), token)?;
        let client = {
            let _guard = runtime().enter();
            DaemonClient::new(addr)?
        };
        Ok(Arc::new(Self {
            backend: Backend::Daemon(client),
            config: CuaConfig::default(),
        }))
    }

    /// Topology.
    pub fn mode(&self) -> CuaMode {
        match self.backend {
            Backend::Embedded(_) => CuaMode::Embedded,
            Backend::Daemon(_) => CuaMode::Daemon,
        }
    }

    /// SDK (and daemon) identity. In daemon mode this is the connectivity
    /// check.
    pub async fn info(&self) -> Result<CuaInfo> {
        let backend = self.backend.clone();
        run(async move {
            let mut info = CuaInfo {
                sdk_version: crate::cua_sdk_version(),
                mode: CuaMode::Embedded,
                daemon_version: None,
                daemon_pid: None,
                socket_path: None,
                loopback_url: None,
                features: features(),
            };
            if let Backend::Daemon(d) = backend {
                let i = d.info().await?;
                let opt = |s: String| (!s.is_empty()).then_some(s);
                info.mode = CuaMode::Daemon;
                info.daemon_version = Some(i.version);
                info.daemon_pid = Some(i.pid);
                info.socket_path = opt(i.socket_path);
                info.loopback_url = opt(i.loopback_url);
            }
            Ok(info)
        })
        .await
    }

    /// Sandboxes (local, direct, providers).
    pub fn sandboxes(&self) -> Arc<Sandboxes> {
        Arc::new(Sandboxes {
            backend: self.backend.clone(),
        })
    }

    /// Fleet (Cua Cloud, closed: its calls fail with that message) and the
    /// account's billing, with this SDK's account credentials (in daemon
    /// mode, from this process's settings and environment).
    pub fn fleet(&self) -> Result<Arc<Fleet>> {
        let account = match &self.backend {
            Backend::Embedded(rt) => rt.account().cloned(),
            Backend::Daemon(_) => self.config.account(),
        }
        .ok_or_else(|| {
            CuaError::ProviderNotConfigured(
                "no Cua account: run `cua auth login` or set CUA_CLIENT_ID/CUA_CLIENT_SECRET"
                    .into(),
            )
        })?;
        Ok(Arc::new(Fleet { account }))
    }

    /// Connects to cua-spacesd at `url` (`host:port`, `http(s)://…` or a
    /// relay URL) without a sandbox.
    pub async fn spacesd(&self, url: String, token: Option<String>) -> Result<Arc<SpacesdClient>> {
        let backend = self.backend.clone();
        run(async move {
            let client = match backend {
                Backend::Embedded(rt) => rt.env_url(&url, token).await?,
                Backend::Daemon(_) => {
                    let mut o = cua_spacesd_client::ConnectOptions::parse(&url)?;
                    o.token = token;
                    cua_spacesd_client::SpacesdClient::connect(o).await?
                }
            };
            Ok(Arc::new(SpacesdClient::new(client, vec![])))
        })
        .await
    }

    /// Asks a connected daemon to stop (daemon mode only).
    pub async fn shutdown_daemon(&self) -> Result<()> {
        let backend = self.backend.clone();
        run(async move {
            match backend {
                Backend::Daemon(d) => Ok(d.shutdown().await?),
                Backend::Embedded(_) => Err(CuaError::Unsupported(
                    "shutdown_daemon needs a daemon connection".into(),
                )),
            }
        })
        .await
    }
}

#[cfg(feature = "local")]
#[uniffi::export]
impl Cua {
    /// Local runtimes (doctor and setup) and local images.
    pub fn local(&self) -> Arc<super::Local> {
        Arc::new(super::Local {
            backend: self.backend.clone(),
        })
    }
}

#[cfg(feature = "spaces")]
#[uniffi::export]
impl Cua {
    /// Spaces: the registry and every Space primitive.
    pub fn spaces(&self) -> Arc<super::Spaces> {
        Arc::new(super::Spaces::new(&self.backend))
    }
}

/// [`cua_daemon::identity::check`] on the daemon `client` reaches, from a
/// thread of its own (so a caller already inside an async runtime can call
/// [`Cua::auto`]), bounded. A daemon that does not answer in time is left
/// to fail on first use, as without the check.
fn check_identity(client: &DaemonClient) -> Result<()> {
    let client = client.clone();
    let info = std::thread::spawn(move || {
        runtime().block_on(async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), client.info()).await
        })
    })
    .join()
    .map_err(|_| CuaError::Internal("the daemon identity check panicked".into()))?;
    match info {
        Ok(Ok(info)) => Ok(cua_daemon::identity::check(&info)?),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_fleet_settings_override_environment() {
        let cfg = CuaConfig {
            fleet_from_env: false,
            fleet: Some(FleetSettings {
                base_url: Some("https://fleet.example/".into()),
                token: Some("t".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let a = cfg.account().expect("a token is an account");
        assert_eq!(a.base_url(), "https://fleet.example");
        let none = CuaConfig {
            fleet_from_env: false,
            ..Default::default()
        };
        assert!(none.account().is_none());
    }
}
