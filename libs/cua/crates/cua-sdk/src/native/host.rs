//! Unattended access: `Relay` (the account's machines on a cua-relay,
//! connected as spacesd clients through `<relay>/m/<id>`) and `Host`
//! (set this machine up as one; cua-host). Account tokens are passed in by
//! the caller (the app's or CLI's cua.ai sign-in); the SDK never stores them.

use super::{Auth, SpacesdClient, run};
use crate::{CuaError, Result};
use std::sync::Arc;

impl From<cua_host::Error> for CuaError {
    fn from(e: cua_host::Error) -> Self {
        use cua_host::Error as E;
        let m = e.to_string();
        match e {
            E::InvalidArgument(_) | E::Conflict(_) => CuaError::InvalidArgument(m),
            E::Unauthenticated(_) => CuaError::Unauthenticated(m),
            E::PermissionDenied(_) => CuaError::PermissionDenied(m),
            E::NotFound(_) => CuaError::NotFound(m),
            E::Relay(_) | E::Download(_) => CuaError::Http(m),
            E::Service(_) => CuaError::Runtime(m),
            E::Io(_) | E::Internal(_) => CuaError::Internal(m),
        }
    }
}

/// Somebody connected to a machine through the relay.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelayClientInfo {
    /// User id.
    pub id: String,
    /// Email, when shared by the identity provider.
    pub email: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Open streams.
    pub streams: u32,
    /// Unix seconds of the first open stream.
    pub since: u64,
}

impl From<cua_host::ConnectedClient> for RelayClientInfo {
    fn from(c: cua_host::ConnectedClient) -> Self {
        Self {
            id: c.id,
            email: c.email,
            name: c.name,
            streams: c.streams,
            since: c.since,
        }
    }
}

/// A machine of the account's relay directory (`relay:<id>`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelayMachine {
    /// Machine id.
    pub id: String,
    /// `relay:<id>`.
    pub space_id: String,
    /// Display name.
    pub name: String,
    /// Owner account id.
    pub owner_id: String,
    /// Owner email.
    pub owner_email: Option<String>,
    /// `owner` or `shared`.
    pub role: String,
    /// Connected to the relay.
    pub online: bool,
    /// Accepting clients.
    pub sharing: bool,
    /// spacesd version.
    pub version: String,
    /// `<relay>/m/<id>`.
    pub url: String,
    /// Allowlist (owner view only).
    pub allow: Vec<String>,
    /// Connected clients.
    pub clients: Vec<RelayClientInfo>,
    /// Registered with an enrolled device's signature or MFA, or confirmed
    /// since from one; `true` for anything a relay registered before this
    /// check existed. `false` shows the machine as "new" until an enrolled
    /// device confirms it ([`Devices::confirm_machine`]) (S5).
    pub confirmed: bool,
}

impl From<cua_host::Machine> for RelayMachine {
    fn from(m: cua_host::Machine) -> Self {
        Self {
            space_id: format!("relay:{}", m.id),
            id: m.id,
            name: m.name,
            owner_id: m.owner.id,
            owner_email: m.owner.email,
            role: m.role,
            online: m.online,
            sharing: m.sharing,
            version: m.version,
            url: m.url,
            allow: m.allow,
            clients: m.clients.into_iter().map(Into::into).collect(),
            confirmed: m.confirmed,
        }
    }
}

/// The signed-in account's view of a cua-relay.
#[derive(uniffi::Object)]
pub struct Relay {
    client: cua_host::RelayClient,
    account_token: String,
}

#[uniffi::export]
impl Relay {
    /// A relay (`None` = `CUA_RELAY_URL`, else `https://relay.cua.ai`) seen
    /// as the account behind `account_token` (a cua.ai access token).
    #[uniffi::constructor]
    pub fn new(relay_url: Option<String>, account_token: String) -> Result<Arc<Self>> {
        let url = relay_url
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(cua_host::relay_url_from_env);
        Ok(Arc::new(Self {
            client: cua_host::RelayClient::new(&url)?,
            account_token,
        }))
    }

    /// Normalized relay base URL.
    pub fn url(&self) -> String {
        self.client.base().to_string()
    }

    /// Machines the account owns or that are shared with it.
    pub async fn machines(&self) -> Result<Vec<RelayMachine>> {
        let client = self.client.clone();
        let token = self.account_token.clone();
        run(async move {
            Ok(client
                .machines(&token)
                .await?
                .into_iter()
                .map(Into::into)
                .collect())
        })
        .await
    }

    /// One machine.
    pub async fn machine(&self, machine_id: String) -> Result<RelayMachine> {
        let client = self.client.clone();
        let token = self.account_token.clone();
        run(async move { Ok(client.machine(&token, &machine_id).await?.into()) }).await
    }

    /// Cuts every client of a machine the account owns and refuses new ones.
    pub async fn stop_sharing(&self, machine_id: String) -> Result<RelayMachine> {
        let client = self.client.clone();
        let token = self.account_token.clone();
        run(async move { Ok(client.stop_sharing(&token, &machine_id).await?.into()) }).await
    }

    /// Accepts clients again.
    pub async fn start_sharing(&self, machine_id: String) -> Result<RelayMachine> {
        let client = self.client.clone();
        let token = self.account_token.clone();
        run(async move { Ok(client.start_sharing(&token, &machine_id).await?.into()) }).await
    }

    /// Connects to a machine's spacesd through `<relay>/m/<id>`, with the
    /// account token as bearer (the relay swaps it for its signed identity).
    pub async fn connect(&self, machine_id: String) -> Result<Arc<SpacesdClient>> {
        let url = self.client.machine_url(&machine_id);
        let token = self.account_token.clone();
        run(async move {
            let o = cua_spacesd_client::ConnectOptions::parse(&url)?
                .fleet_gateway(Arc::new(cua_spacesd_client::StaticBearer(token)), None);
            let client = cua_spacesd_client::SpacesdClient::connect(o).await?;
            Ok(Arc::new(SpacesdClient::new(client, vec![])))
        })
        .await
    }
}

/// `cua host setup` options.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct HostSetupOptions {
    /// `relay` (default) or `direct`.
    pub mode: Option<String>,
    /// Relay URL (relay mode; default `CUA_RELAY_URL` / `https://relay.cua.ai`).
    pub relay_url: Option<String>,
    /// `ip:port` (direct mode; default `0.0.0.0:3211`).
    pub direct: Option<String>,
    /// Display name (default: host name).
    pub name: Option<String>,
    /// Accounts (ids or emails) allowed besides the owner.
    pub allow: Vec<String>,
    /// Driver binary (else `CUA_SPACESD_BIN`, bundled, or download).
    pub driver_bin: Option<String>,
    /// `auto`, `systemd`, `launchd`, `windows-task` or `process`.
    pub runner: Option<String>,
    /// `desktop` (share this desktop; the default) or `spare` (do not
    /// share the desktop; provide Spaces). The two settings below override
    /// it.
    #[uniffi(default = None)]
    pub profile: Option<String>,
    /// Share this machine's own desktop as a Space.
    #[uniffi(default = None)]
    pub share_desktop: Option<bool>,
    /// Create Spaces for your enrolled devices on this machine (relay
    /// mode).
    #[uniffi(default = None)]
    pub provide_spaces: Option<bool>,
}

/// A change to this machine's Spaces settings (`None` keeps a value).
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct HostSettingsChange {
    /// Share this desktop.
    #[uniffi(default = None)]
    pub share_desktop: Option<bool>,
    /// Provide Spaces.
    #[uniffi(default = None)]
    pub provide_spaces: Option<bool>,
    /// Provided Spaces at once (0: no limit).
    #[uniffi(default = None)]
    pub max_spaces: Option<u32>,
    /// macOS VMs at once (0 to 2, Apple's license).
    #[uniffi(default = None)]
    pub max_macos_vms: Option<u32>,
}

/// A Space this machine provides to one of your devices.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostProvidedSpace {
    /// The relay machine it is reached as (`relay:<machine>`).
    pub relay_machine: String,
    /// The Space on this machine (`local:<name>`).
    pub local_space: String,
    /// Display name.
    pub name: String,
    /// The image, as requested.
    pub image: String,
    /// `linux`, `macos` (empty when unknown).
    pub os: String,
    /// `container` or `vm`.
    pub kind: String,
    /// Who created it.
    pub created_by: String,
    /// Epoch ms.
    pub created_at_ms: i64,
}

/// One line of this machine's Spaces audit (remote creates, deletes,
/// refusals and settings changes).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostSpacesAuditRecord {
    /// Epoch ms.
    pub at_ms: i64,
    /// `create`, `delete`, `refused`, `failed` or `config`.
    pub action: String,
    /// Who (an account, or `local`).
    pub who: String,
    /// The relay machine or Space.
    pub space: String,
    /// Detail.
    pub detail: String,
}

/// A macOS privacy pane the user must grant.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostPermission {
    /// `screen-recording` or `accessibility`.
    pub id: String,
    /// Title.
    pub title: String,
    /// `x-apple.systempreferences:` URL.
    pub settings_url: String,
    /// Instructions.
    pub instructions: String,
}

/// Host status.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostStatus {
    /// Set up.
    pub configured: bool,
    /// `relay` or `direct`.
    pub mode: Option<String>,
    /// Relay URL.
    pub relay_url: Option<String>,
    /// Direct URL.
    pub direct_url: Option<String>,
    /// Env token file (direct mode).
    pub env_token_path: Option<String>,
    /// Machine id.
    pub machine_id: Option<String>,
    /// Name.
    pub name: Option<String>,
    /// Accepting clients.
    pub sharing: bool,
    /// Service installed.
    pub service_installed: bool,
    /// Service running.
    pub service_running: bool,
    /// Runner kind.
    pub service_kind: String,
    /// Online at the relay.
    pub online: Option<bool>,
    /// Connected clients.
    pub clients: Vec<RelayClientInfo>,
    /// Allowlist.
    pub allow: Vec<String>,
    /// Permission panes (macOS).
    pub permissions: Vec<HostPermission>,
    /// Relay error.
    pub error: Option<String>,
    /// Who reached this machine recently, newest first (the driver's
    /// hash-chained access log).
    #[uniffi(default = [])]
    pub recent_access: Vec<HostAccessRecord>,
    /// Set when the access log does not verify (edited or truncated).
    #[uniffi(default = None)]
    pub access_log_error: Option<String>,
    /// This machine's desktop is a Space.
    #[uniffi(default = true)]
    pub share_desktop: bool,
    /// This machine provides Spaces to your other devices.
    #[uniffi(default = false)]
    pub provide_spaces: bool,
    /// Provided Spaces at once (0: no limit).
    #[uniffi(default = 0)]
    pub max_spaces: u32,
    /// macOS VMs at once (at most two).
    #[uniffi(default = 0)]
    pub max_macos_vms: u32,
    /// The Spaces this machine provides now.
    #[uniffi(default = [])]
    pub provided_spaces: Vec<HostProvidedSpace>,
    /// The Spaces audit, newest first.
    #[uniffi(default = [])]
    pub spaces_audit: Vec<HostSpacesAuditRecord>,
    /// Set when the Spaces audit does not verify.
    #[uniffi(default = None)]
    pub spaces_audit_error: Option<String>,
}

/// One access to this machine.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostAccessRecord {
    /// Epoch ms.
    pub at_ms: i64,
    /// `relay`, `viewer` or `token`.
    pub via: String,
    /// Who.
    pub who: String,
    /// What they used (a service name, or `MCP`).
    pub what: String,
}

impl From<cua_host::HostStatus> for HostStatus {
    fn from(s: cua_host::HostStatus) -> Self {
        Self {
            configured: s.configured,
            mode: s.mode,
            relay_url: s.relay_url,
            direct_url: s.direct_url,
            env_token_path: s.env_token_path,
            machine_id: s.machine_id,
            name: s.name,
            sharing: s.sharing,
            service_installed: s.service.installed,
            service_running: s.service.running,
            service_kind: s.service.kind,
            online: s.online,
            clients: s.clients.into_iter().map(Into::into).collect(),
            allow: s.allow,
            permissions: s
                .permissions
                .into_iter()
                .map(|p| HostPermission {
                    id: p.id,
                    title: p.title,
                    settings_url: p.settings_url,
                    instructions: p.instructions,
                })
                .collect(),
            error: s.error,
            recent_access: s
                .recent_access
                .into_iter()
                .map(|a| HostAccessRecord {
                    at_ms: i64::try_from(a.at_ms).unwrap_or(i64::MAX),
                    via: a.via,
                    who: a.who,
                    what: a.what,
                })
                .collect(),
            access_log_error: s.access_log_error,
            share_desktop: s.share_desktop,
            provide_spaces: s.provide_spaces,
            max_spaces: s.max_spaces,
            max_macos_vms: s.max_macos_vms,
            provided_spaces: s
                .provided_spaces
                .into_iter()
                .map(|p| HostProvidedSpace {
                    relay_machine: p.relay_machine,
                    local_space: p.local_space,
                    name: p.name,
                    image: p.image,
                    os: p.os,
                    kind: p.kind,
                    created_by: p.created_by,
                    created_at_ms: i64::try_from(p.created_at_ms).unwrap_or(i64::MAX),
                })
                .collect(),
            spaces_audit: s
                .spaces_audit
                .into_iter()
                .map(|a| HostSpacesAuditRecord {
                    at_ms: i64::try_from(a.at_ms).unwrap_or(i64::MAX),
                    action: a.action,
                    who: a.who,
                    space: a.space,
                    detail: a.detail,
                })
                .collect(),
            spaces_audit_error: s.spaces_audit_error,
        }
    }
}

/// Converts binding options to cua-host options.
pub(crate) fn setup_options(o: HostSetupOptions) -> Result<cua_host::SetupOptions> {
    let mut opts = match o.mode.as_deref().unwrap_or("relay") {
        "relay" => cua_host::SetupOptions::relay(
            o.relay_url
                .filter(|u| !u.trim().is_empty())
                .unwrap_or_else(cua_host::relay_url_from_env),
        ),
        "direct" => {
            let listen = o.direct.as_deref().unwrap_or("0.0.0.0:3211");
            cua_host::SetupOptions::direct(listen.parse().map_err(|_| {
                CuaError::InvalidArgument(format!("direct address {listen:?} is not ip:port"))
            })?)
        }
        other => {
            return Err(CuaError::InvalidArgument(format!(
                "mode {other:?} (use relay or direct)"
            )));
        }
    };
    opts.name = o.name;
    opts.allow = o.allow;
    opts.driver_bin = o.driver_bin.map(Into::into);
    opts.runner = cua_host::RunnerKind::parse(o.runner.as_deref().unwrap_or("auto"))?;
    if let Some(p) = o.profile.as_deref().filter(|p| !p.trim().is_empty()) {
        opts = opts.profile(cua_host::HostProfile::parse(p)?);
    }
    if let Some(d) = o.share_desktop {
        opts.share_desktop = d;
    }
    if let Some(p) = o.provide_spaces {
        opts.provide_spaces = p;
    }
    Ok(opts)
}

/// This machine as an unattended-access host.
#[derive(uniffi::Object)]
pub struct Host {
    home: std::path::PathBuf,
}

#[uniffi::export]
impl Host {
    /// Host state under `cua_home` (default `$CUA_HOME` or `~/.cua`).
    #[uniffi::constructor]
    pub fn new(cua_home: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            home: cua_home
                .map(Into::into)
                .unwrap_or_else(cua_daemon::cua_home),
        })
    }

    /// Current state.
    pub async fn status(&self) -> Result<HostStatus> {
        let home = self.home.clone();
        run(async move { Ok(cua_host::Host::new(home).status().await?.into()) }).await
    }

    /// Installs and starts the host service (relay mode needs
    /// `account_token`).
    pub async fn setup(
        &self,
        options: HostSetupOptions,
        account_token: Option<String>,
    ) -> Result<HostStatus> {
        let home = self.home.clone();
        let mode = options.mode.clone().unwrap_or_else(|| "relay".into());
        let opts = setup_options(options);
        let (desktop, provide) = opts
            .as_ref()
            .map(|o| (o.share_desktop, o.provide_spaces))
            .unwrap_or((true, false));
        let r = match opts {
            Ok(opts) => {
                run(async move {
                    let host = cua_host::Host::new(home);
                    let status = match account_token.filter(|t| !t.is_empty()) {
                        Some(t) => host.setup(opts, &cua_host::StaticToken(t)).await?,
                        None => host.setup(opts, &cua_host::NoAccount).await?,
                    };
                    Ok(status.into())
                })
                .await
            }
            Err(e) => Err(e),
        };
        // Host Spaces adoption: the mode and what the machine is for,
        // never its name, address or who it is shared with.
        cua_telemetry::capture(cua_telemetry::events::host_setup(
            &mode,
            desktop,
            provide,
            if r.is_ok() {
                cua_telemetry::Outcome::Ok
            } else {
                cua_telemetry::Outcome::Error
            },
            super::telemetry::variant(&r),
        ));
        r
    }

    /// Changes what this machine shares: its own desktop, and whether it
    /// creates Spaces for your other devices (and their limits). Turning
    /// the desktop on or off restarts the host service.
    pub async fn configure(&self, change: HostSettingsChange) -> Result<HostStatus> {
        let home = self.home.clone();
        run(async move {
            Ok(cua_host::Host::new(home)
                .configure(cua_host::HostSettingsChange {
                    share_desktop: change.share_desktop,
                    provide_spaces: change.provide_spaces,
                    max_spaces: change.max_spaces,
                    max_macos_vms: change.max_macos_vms,
                    cua_bin: None,
                })
                .await?
                .into())
        })
        .await
    }

    /// Stop sharing (cut clients, refuse new ones).
    pub async fn stop_sharing(&self) -> Result<HostStatus> {
        let home = self.home.clone();
        run(async move { Ok(cua_host::Host::new(home).stop_sharing().await?.into()) }).await
    }

    /// Start sharing again.
    pub async fn start_sharing(&self) -> Result<HostStatus> {
        let home = self.home.clone();
        run(async move { Ok(cua_host::Host::new(home).start_sharing().await?.into()) }).await
    }

    /// Unregister, uninstall the service and delete the host state.
    pub async fn remove(&self) -> Result<()> {
        let home = self.home.clone();
        run(async move { Ok(cua_host::Host::new(home).remove().await?) }).await
    }
}

// ---- This device as a client of the account on the relay -----------------

/// A client device of the account, as the relay lists it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelayDevice {
    /// `dev_…`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `pending`, `enrolled`, `expired` or `revoked`.
    pub state: String,
    /// Operating system it reported (`macos`, `windows`, `linux`); empty
    /// when unknown.
    pub platform: String,
    /// Registered at (Unix seconds).
    pub created_at: u64,
    /// Re-verification due at (Unix seconds).
    pub enrolled_until: Option<u64>,
    /// Last session (Unix seconds).
    pub last_seen: Option<u64>,
    /// This device.
    pub current: bool,
}

impl From<cua_host::DeviceView> for RelayDevice {
    fn from(d: cua_host::DeviceView) -> Self {
        Self {
            state: match d.state {
                cua_host::DeviceState::Pending => "pending",
                cua_host::DeviceState::Enrolled => "enrolled",
                cua_host::DeviceState::Expired => "expired",
                cua_host::DeviceState::Revoked => "revoked",
            }
            .into(),
            id: d.id,
            name: d.name,
            platform: d.platform,
            created_at: d.created_at,
            enrolled_until: d.enrolled_until,
            last_seen: d.last_seen,
            current: d.current,
        }
    }
}

/// One event of the account's audit log.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelayAuditEvent {
    /// Unix seconds.
    pub ts: u64,
    /// Kind (`machine_access`, `device_enrolled`, `share_added`, ...).
    pub kind: String,
    /// Acting device.
    pub device: Option<String>,
    /// Machine id.
    pub machine: Option<String>,
    /// Other party.
    pub subject: Option<String>,
    /// Detail.
    pub detail: Option<String>,
}

impl From<cua_host::AuditEvent> for RelayAuditEvent {
    fn from(e: cua_host::AuditEvent) -> Self {
        Self {
            ts: e.ts,
            kind: e.kind,
            device: e.device,
            machine: e.machine,
            subject: e.subject,
            detail: e.detail,
        }
    }
}

/// Everything the Devices page reads, in one call.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DevicesSnapshot {
    /// This device's id, when it has a key.
    pub local_device_id: Option<String>,
    /// The account's devices (this one marked `current` once enrolled).
    pub devices: Vec<RelayDevice>,
    /// End of the relay's grace period for unenrolled devices.
    pub enforce_after: Option<u64>,
    /// The account's audit log, newest last (empty when the relay refuses).
    pub audit: Vec<RelayAuditEvent>,
    /// Machine ids to names (empty when this device cannot list them).
    pub machine_names: std::collections::HashMap<String, String>,
    /// The account's relay machines, [`RelayMachine::confirmed`] included
    /// (S5; empty when this device cannot list them, same as
    /// `machine_names`).
    pub machines: Vec<RelayMachine>,
}

/// `Devices.enroll`'s result.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceEnrollment {
    /// This device.
    pub device: RelayDevice,
    /// Enrolled now (right after a fresh sign-in).
    pub enrolled: bool,
    /// The one-time code to approve from an enrolled device otherwise.
    pub code: Option<String>,
}

/// The session as `cua-host` account tokens.
pub(super) struct SessionTokens(pub(super) Arc<cua_auth::Session>);

#[async_trait::async_trait]
impl cua_host::AccountTokens for SessionTokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        self.0
            .access_token(false)
            .await
            .map_err(|e| cua_host::Error::Unauthenticated(e.to_string()))
    }
}

/// This device as a client of the signed-in account on a relay: its
/// enrollment, the account's other devices and its audit log. The device
/// key lives in the session's vault (shared with the `cua` CLI and daemon).
/// Nothing here prompts: approving is the caller's to gate behind presence.
#[derive(uniffi::Object)]
pub struct Devices {
    auth: Arc<cua_host::DeviceAuth>,
    tokens: Arc<SessionTokens>,
}

#[uniffi::export]
impl Auth {
    /// This device on `relay_url` (`None` = `CUA_RELAY_URL`, else
    /// `https://relay.cua.ai`) as the signed-in account, shown as `name`
    /// (default: the host name).
    pub fn devices(&self, relay_url: Option<String>, name: Option<String>) -> Result<Arc<Devices>> {
        let url = relay_url
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(cua_host::relay_url_from_env);
        let session = self.session();
        let tokens = Arc::new(SessionTokens(session.clone()));
        let auth = cua_host::DeviceAuth::new(
            &url,
            tokens.clone(),
            Arc::new(session.store().clone()),
            name.filter(|n| !n.trim().is_empty())
                .unwrap_or_else(cua_host::device_name),
        )?;
        Ok(Arc::new(Devices {
            auth: Arc::new(auth),
            tokens,
        }))
    }
}

#[uniffi::export]
impl Devices {
    /// The relay base URL.
    pub fn relay_url(&self) -> String {
        self.auth.relay_url().to_string()
    }

    /// This device's id, when it has a key (no network).
    pub fn device_id(&self) -> Result<Option<String>> {
        Ok(self.auth.device_id()?)
    }

    /// The account's devices, the grace period, the newest `audit_limit`
    /// audit events and the machines' names. Only the device list must
    /// succeed; the rest is empty when the relay refuses it.
    pub async fn snapshot(&self, audit_limit: u32) -> Result<DevicesSnapshot> {
        let auth = self.auth.clone();
        let tokens = self.tokens.clone();
        run(async move {
            let listing = auth.listing().await?;
            let audit = auth.audit(audit_limit as usize).await.unwrap_or_default();
            let machines: Vec<RelayMachine> = match auth.try_session().await {
                Some(session) => {
                    let token = cua_host::AccountTokens::access_token(tokens.as_ref()).await?;
                    cua_host::RelayClient::new(auth.relay_url())?
                        .with_device_session(Some(session))
                        .machines(&token)
                        .await
                        .map(|l| l.into_iter().map(Into::into).collect())
                        .unwrap_or_default()
                }
                None => Default::default(),
            };
            let machine_names = machines
                .iter()
                .map(|m| (m.id.clone(), m.name.clone()))
                .collect();
            Ok(DevicesSnapshot {
                local_device_id: auth.device_id()?,
                devices: listing.devices.into_iter().map(Into::into).collect(),
                enforce_after: (listing.enforce_after > 0).then_some(listing.enforce_after),
                audit: audit.into_iter().map(Into::into).collect(),
                machine_names,
                machines,
            })
        })
        .await
    }

    /// Registers this device (creating its key on first use). Right after
    /// a fresh sign-in it is enrolled at once (replacing this machine's
    /// older device key, if any); otherwise the result carries a one-time
    /// code to approve from an enrolled device.
    pub async fn enroll(&self) -> Result<DeviceEnrollment> {
        let auth = self.auth.clone();
        run(async move {
            let e = auth.enroll().await?;
            Ok(DeviceEnrollment {
                enrolled: e.device.state == cua_host::DeviceState::Enrolled,
                device: e.device.into(),
                code: e.code,
            })
        })
        .await
    }

    /// Whether the relay lets this device open a session now (enrolled),
    /// asked afresh. Polled while waiting for an approval.
    pub async fn check_enrolled(&self) -> bool {
        let auth = self.auth.clone();
        run(async move {
            auth.reset_session().await;
            Ok(auth.session().await.is_ok())
        })
        .await
        .unwrap_or(false)
    }

    /// Approves the device showing `code`, or re-verifies `device_id`,
    /// from this enrolled device.
    pub async fn approve(
        &self,
        code: Option<String>,
        device_id: Option<String>,
    ) -> Result<RelayDevice> {
        let auth = self.auth.clone();
        run(async move {
            Ok(auth
                .approve(code.as_deref(), device_id.as_deref())
                .await?
                .into())
        })
        .await
    }

    /// Renames a device (from this enrolled device).
    pub async fn rename(&self, id: String, name: String) -> Result<RelayDevice> {
        let auth = self.auth.clone();
        run(async move { Ok(auth.rename(&id, &name).await?.into()) }).await
    }

    /// Revokes a device (from this enrolled device). Revoking this device
    /// also deletes its key.
    pub async fn revoke(&self, id: String) -> Result<RelayDevice> {
        let auth = self.auth.clone();
        run(async move { Ok(auth.revoke(&id).await?.into()) }).await
    }

    /// Vouches for `machine_id`, a machine that registered without an
    /// enrolled device's signature or MFA (from this enrolled device) (S5).
    /// Clears [`RelayMachine::confirmed`]'s "new" flag for every account
    /// member, not just this device.
    pub async fn confirm_machine(&self, machine_id: String) -> Result<RelayMachine> {
        let auth = self.auth.clone();
        let tokens = self.tokens.clone();
        run(async move {
            let session = auth.session().await?;
            let token = cua_host::AccountTokens::access_token(tokens.as_ref()).await?;
            Ok(cua_host::RelayClient::new(auth.relay_url())?
                .with_device_session(Some(session))
                .confirm(&token, &machine_id)
                .await?
                .into())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn devices_enroll_approve_rename_and_revoke_through_the_relay() {
        let relay = cua_host::testing::FakeRelay::start().await;
        relay.add_account("a.eyJlbWFpbCI6ImFAYi5jIn0.s", "user-1", None);
        relay.set_enforce_after(1_900_000_000);
        let signed_in = |dir: &std::path::Path| {
            let store = cua_auth::Store::File(dir.join("credentials.json"));
            store
                .save(
                    &cua_auth::Credentials::from_token_response(&serde_json::json!({
                        "access_token": "a.eyJlbWFpbCI6ImFAYi5jIn0.s", "expires_in": 3600,
                    }))
                    .unwrap(),
                )
                .unwrap();
            super::super::Auth::with_session(cua_auth::Session::new(
                cua_auth::Oidc::new("http://127.0.0.1:9", "cua-cli"),
                store,
            ))
        };
        // Both devices run on this machine: play a relay that ignores
        // machine ids (re-keying is cua-host's to test).
        relay.legacy_enrollment(true);
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mac = signed_in(d1.path())
            .devices(Some(relay.url.clone()), Some("MacBook".into()))
            .unwrap();
        let phone = signed_in(d2.path())
            .devices(Some(relay.url.clone()), Some("Phone".into()))
            .unwrap();
        assert_eq!(mac.device_id().unwrap(), None);
        // The first device enrolls at once; the second shows a code.
        let first = mac.enroll().await.unwrap();
        assert!(first.enrolled && first.code.is_none());
        assert_eq!(first.device.platform, std::env::consts::OS);
        assert!(mac.check_enrolled().await);
        let second = phone.enroll().await.unwrap();
        assert!(!second.enrolled);
        assert!(!phone.check_enrolled().await);
        let snap = mac.snapshot(50).await.unwrap();
        assert_eq!(snap.enforce_after, Some(1_900_000_000));
        assert_eq!(snap.devices.len(), 2);
        assert!(
            snap.devices
                .iter()
                .any(|d| d.current && d.name == "MacBook")
        );
        assert!(snap.audit.iter().any(|e| e.kind == "device_registered"));
        // A pending device cannot approve; the enrolled one can, by code.
        assert!(matches!(
            phone.approve(second.code.clone(), None).await.unwrap_err(),
            CuaError::PermissionDenied(_)
        ));
        let approved = mac.approve(second.code.clone(), None).await.unwrap();
        assert_eq!(approved.state, "enrolled");
        assert!(phone.check_enrolled().await);
        let phone_id = phone.device_id().unwrap().unwrap();
        assert_eq!(
            mac.rename(phone_id.clone(), "Work phone".into())
                .await
                .unwrap()
                .name,
            "Work phone"
        );
        assert_eq!(mac.revoke(phone_id.clone()).await.unwrap().state, "revoked");
        assert!(!phone.check_enrolled().await);
        // A revoked device enrolls again with a new key.
        let again = phone.enroll().await.unwrap();
        assert_ne!(again.device.id, phone_id);
        assert!(!again.enrolled && again.code.is_some());
    }

    #[test]
    fn setup_options_map_modes() {
        let o = setup_options(HostSetupOptions {
            mode: Some("direct".into()),
            direct: Some("10.0.0.2:4000".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            o.mode,
            cua_host::HostMode::Direct {
                listen: "10.0.0.2:4000".parse().unwrap()
            }
        );
        let o = setup_options(HostSetupOptions {
            relay_url: Some("https://relay.example".into()),
            allow: vec!["a@b".into()],
            runner: Some("process".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            o.mode,
            cua_host::HostMode::Relay {
                url: "https://relay.example".into()
            }
        );
        assert_eq!(o.runner, cua_host::RunnerKind::Process);
        assert!(o.share_desktop && !o.provide_spaces, "the desktop profile");
        // A spare machine, and each setting on its own.
        let o = setup_options(HostSetupOptions {
            profile: Some("spare".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(!o.share_desktop && o.provide_spaces);
        let o = setup_options(HostSetupOptions {
            profile: Some("spare".into()),
            share_desktop: Some(true),
            ..Default::default()
        })
        .unwrap();
        assert!(o.share_desktop && o.provide_spaces);
        assert!(
            setup_options(HostSetupOptions {
                mode: Some("carrier-pigeon".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            setup_options(HostSetupOptions {
                mode: Some("direct".into()),
                direct: Some("nope".into()),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[tokio::test]
    async fn relay_lists_machines_from_the_directory() {
        let relay = cua_host::testing::FakeRelay::start().await;
        relay.add_account("tok", "user-1", None);
        cua_host::RelayClient::new(&relay.url)
            .unwrap()
            .register(
                "tok",
                &cua_host::relay::RegisterRequest {
                    id: "0123abcd4567ef89".into(),
                    name: "mini".into(),
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .unwrap();
        let r = Relay::new(Some(relay.url.clone()), "tok".into()).unwrap();
        let machines = r.machines().await.unwrap();
        assert_eq!(machines.len(), 1);
        assert_eq!(machines[0].space_id, "relay:0123abcd4567ef89");
        assert_eq!(machines[0].role, "owner");
        let bad = Relay::new(Some(relay.url.clone()), "nope".into()).unwrap();
        assert!(matches!(
            bad.machines().await.unwrap_err(),
            CuaError::Unauthenticated(_)
        ));
    }
}
