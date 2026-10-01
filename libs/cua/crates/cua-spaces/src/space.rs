//! A connected Space: the services it declares, a spacesd client plus the
//! capabilities it reported (when the image runs cua-spacesd), and the gate
//! every spacesd primitive goes through.

use crate::error::{Error, Result};
use crate::id::{Provider, SpaceId};
use cua_sandbox_core::{McpClient, McpConfig, ServiceEndpoint};
use cua_spacesd_client::{SpacesdClient, pb};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The spacesd pseudo-feature every spacesd primitive needs; a Space
/// without cua-spacesd reports it missing.
pub const SPACESD_FEATURE: &str = "spacesd";

/// How a declared service is reached.
#[derive(Clone)]
pub enum ServiceSource {
    /// A sandbox service (local loopback or any other sandbox route).
    Sandbox(cua_sandbox_core::Service),
    /// A Fleet claim's service, through the gateway.
    Fleet {
        /// Client (mints the gateway bearer).
        fleet: cua_fleet::FleetClient,
        /// The claim.
        bound: cua_fleet::BoundSandbox,
        /// Service name.
        service: String,
    },
    /// A fixed URL and headers (a Space added by its MCP URL, or a `cua
    /// daemon` passthrough).
    Url(ServiceEndpoint),
}

/// A service a Space declares (an MCP server, a web app, ...), reachable
/// through a protocol-transparent pipe: loopback locally, the Fleet gateway
/// on Fleet, or a URL.
#[derive(Clone)]
pub struct SpaceService {
    /// How requests reach it.
    pub source: ServiceSource,
    /// Its MCP endpoint path (default `/mcp`).
    pub mcp_path: String,
}

impl std::fmt::Debug for SpaceService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpaceService")
            .field("mcp_path", &self.mcp_path)
            .finish_non_exhaustive()
    }
}

impl SpaceService {
    /// Where the service is reachable from this process (URL and headers,
    /// freshly minted for Fleet).
    pub async fn endpoint(&self) -> Result<ServiceEndpoint> {
        match &self.source {
            ServiceSource::Sandbox(s) => Ok(s.endpoint().await?),
            ServiceSource::Fleet {
                fleet,
                bound,
                service,
            } => {
                let token = fleet.access_token(false).await?;
                Ok(ServiceEndpoint {
                    url: fleet.service_url(bound, service)?,
                    headers: vec![
                        ("authorization".into(), format!("Bearer {token}")),
                        ("x-cua-fleet-claim".into(), bound.claim.clone()),
                    ],
                })
            }
            ServiceSource::Url(e) => Ok(e.clone()),
        }
    }

    /// Its MCP endpoint (URL and headers), for any MCP client.
    pub async fn mcp_config(&self) -> Result<McpConfig> {
        Ok(McpConfig::of(&self.endpoint().await?, Some(&self.mcp_path)))
    }
}

/// Media / tunnel WebSockets through the Fleet gateway need the gateway's own
/// credentials next to the ticket.
#[derive(Clone)]
pub(crate) enum Gateway {
    Fleet {
        fleet: cua_fleet::FleetClient,
        claim: String,
    },
    /// Fixed headers (a `cua daemon` env passthrough: its loopback bearer).
    Headers(Vec<(String, String)>),
}

pub(crate) struct SpaceInner {
    pub(crate) id: SpaceId,
    pub(crate) name: String,
    /// `None`: the image runs no cua-spacesd (capabilities are empty).
    pub(crate) env: Option<SpacesdClient>,
    pub(crate) caps: pb::GetCapabilitiesResponse,
    pub(crate) token: Option<String>,
    pub(crate) gateway: Option<Gateway>,
    /// Declared services (never `env`).
    pub(crate) services: BTreeMap<String, SpaceService>,
    /// One MCP session per service, opened on first use.
    pub(crate) mcp: tokio::sync::Mutex<BTreeMap<String, Arc<McpClient>>>,
    /// The image the sandbox runs and its digest ([`Space::image`]).
    pub(crate) image: std::sync::OnceLock<(String, String)>,
    /// `(kind, arch)` from the local sandbox's record, when it has one.
    pub(crate) platform: std::sync::OnceLock<(String, String)>,
}

/// A connected Space. Cheap to clone; every clone shares one spacesd
/// connection.
///
/// A Space is any sandbox: lifecycle, declared services and generic MCP
/// ([`Space::call_tool`] with `service=<declared service>`) work on any
/// image. The spacesd primitives check the feature they need with
/// [`Space::require`] first, so an unsupported call (or any spacesd call
/// on an image without cua-spacesd, whose capability set is empty) fails
/// at once with [`Error::CapabilityMissing`] naming the feature, instead of
/// failing late with a transport error.
#[derive(Clone)]
pub struct Space {
    pub(crate) inner: Arc<SpaceInner>,
}

impl std::fmt::Debug for Space {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Space")
            .field("id", &self.inner.id.to_string())
            .field("name", &self.inner.name)
            .field("spacesd", &self.inner.caps.version)
            .field("services", &self.inner.services.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Space {
    pub(crate) fn new(
        id: SpaceId,
        name: String,
        env: Option<SpacesdClient>,
        caps: pb::GetCapabilitiesResponse,
        token: Option<String>,
        gateway: Option<Gateway>,
        services: BTreeMap<String, SpaceService>,
    ) -> Self {
        Self {
            inner: Arc::new(SpaceInner {
                id,
                name,
                env,
                caps,
                token,
                gateway,
                services,
                mcp: Default::default(),
                image: Default::default(),
                platform: Default::default(),
            }),
        }
    }

    /// A Space over declared services only, without cua-spacesd (for
    /// example a `cua daemon` passthrough of a registered generic Space):
    /// capabilities are empty and every spacesd primitive fails with
    /// [`Error::CapabilityMissing`].
    pub fn generic(
        id: SpaceId,
        name: impl Into<String>,
        services: BTreeMap<String, SpaceService>,
    ) -> Self {
        Self::new(
            id,
            name.into(),
            None,
            Default::default(),
            None,
            None,
            services,
        )
    }

    /// A Space over a spacesd connection someone else established: the
    /// `cua daemon` env passthrough for a registered Space (`cua-sdk` in
    /// daemon mode builds one from `SpaceService.ConnectSpace`), or any
    /// already-connected [`SpacesdClient`] (for example a sandbox the `cua` CLI
    /// resolved). `ws_headers` are sent on media / tunnel WebSockets next to
    /// the ticket. Performs no I/O; `caps` is the handshake the caller did.
    pub fn attach(
        id: SpaceId,
        name: impl Into<String>,
        env: SpacesdClient,
        caps: pb::GetCapabilitiesResponse,
        ws_headers: Vec<(String, String)>,
    ) -> Self {
        let gateway = (!ws_headers.is_empty()).then_some(Gateway::Headers(ws_headers));
        Self::new(
            id,
            name.into(),
            Some(env),
            caps,
            None,
            gateway,
            BTreeMap::new(),
        )
    }

    /// Adds declared services to a handle built with [`Space::attach`].
    pub fn with_services(self, services: BTreeMap<String, SpaceService>) -> Self {
        match Arc::try_unwrap(self.inner) {
            Ok(mut inner) => {
                inner.services.extend(services);
                Self {
                    inner: Arc::new(inner),
                }
            }
            Err(shared) => {
                let mut all = shared.services.clone();
                all.extend(services);
                Self::new(
                    shared.id.clone(),
                    shared.name.clone(),
                    shared.env.clone(),
                    shared.caps.clone(),
                    shared.token.clone(),
                    shared.gateway.clone(),
                    all,
                )
            }
        }
    }

    /// The image the sandbox runs (as requested) and the digest of the
    /// variant that runs, when known: local Spaces from their sandbox
    /// record; `None` for a Space added by address.
    pub fn image(&self) -> Option<(&str, &str)> {
        self.inner
            .image
            .get()
            .map(|(r, d)| (r.as_str(), d.as_str()))
    }

    pub(crate) fn set_image(&self, reference: String, digest: String) {
        let _ = self.inner.image.set((reference, digest));
    }

    /// Container or VM (`container`, `vm`) and CPU architecture (`arm64`,
    /// `amd64`): what the guest reported at the handshake (its runtime and
    /// arch), else the local sandbox's record. Empty when unknown.
    pub fn platform(&self) -> (String, String) {
        use pb::{Architecture, Runtime};
        let caps = self.capabilities();
        let recorded = self.inner.platform.get().cloned().unwrap_or_default();
        let kind = match caps.runtime() {
            Runtime::Container | Runtime::Gvisor => "container",
            Runtime::Qemu | Runtime::Kubevirt | Runtime::Lume | Runtime::Hyperv => "vm",
            _ => "",
        };
        let arch = match caps.arch() {
            Architecture::Arm64 => "arm64",
            Architecture::X8664 => "amd64",
            _ => "",
        };
        let or = |live: &str, rec: String| {
            if live.is_empty() {
                rec
            } else {
                live.to_string()
            }
        };
        (or(kind, recorded.0), or(arch, recorded.1))
    }

    pub(crate) fn set_platform(&self, kind: String, arch: String) {
        let _ = self.inner.platform.set((kind, arch));
    }

    /// The canonical id.
    pub fn id(&self) -> &SpaceId {
        &self.inner.id
    }

    /// Display name.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The provider.
    pub fn provider(&self) -> Provider {
        self.inner.id.provider()
    }

    /// The spacesd client, for anything the Spaces API does not wrap.
    /// Fails with [`Error::CapabilityMissing`] (`spacesd`) when the image
    /// runs no cua-spacesd.
    pub fn spacesd(&self) -> Result<&SpacesdClient> {
        self.inner.env.as_ref().ok_or_else(|| self.no_spacesd())
    }

    /// Whether the image runs cua-spacesd.
    pub fn has_spacesd(&self) -> bool {
        self.inner.env.is_some()
    }

    fn no_spacesd(&self) -> Error {
        Error::CapabilityMissing {
            space: self.inner.id.to_string(),
            feature: SPACESD_FEATURE.into(),
            limitation: "this Space's image runs no cua-spacesd; only its declared \
                         services and generic MCP (list_tools/call_tool with service=...) \
                         are available"
                .into(),
        }
    }

    /// `GetCapabilities` as of connecting (empty without cua-spacesd).
    pub fn capabilities(&self) -> &pb::GetCapabilitiesResponse {
        &self.inner.caps
    }

    /// The declared services (never `env`), by name.
    pub fn declared_services(&self) -> &BTreeMap<String, SpaceService> {
        &self.inner.services
    }

    /// The spacesd token this connection uses (needed by in-guest agents'
    /// MCP config for the driver's `/mcp`).
    pub fn env_token(&self) -> Option<&str> {
        self.inner.token.as_deref()
    }

    /// A feature entry by canonical name.
    pub fn feature(&self, name: &str) -> Option<&pb::Feature> {
        self.inner.caps.features.iter().find(|f| f.name == name)
    }

    /// Whether the driver reports `name` as supported.
    pub fn supports(&self, name: &str) -> bool {
        self.feature(name).is_some_and(|f| f.supported)
    }

    /// Fails with [`Error::CapabilityMissing`] unless `name` is supported.
    pub fn require(&self, name: &str) -> Result<()> {
        if self.inner.env.is_none() {
            let mut e = self.no_spacesd();
            if let Error::CapabilityMissing { feature, .. } = &mut e {
                *feature = name.into();
            }
            return Err(e);
        }
        match self.feature(name) {
            Some(f) if f.supported => Ok(()),
            Some(f) => Err(Error::CapabilityMissing {
                space: self.inner.id.to_string(),
                feature: name.into(),
                limitation: f.limitation.clone(),
            }),
            None => Err(Error::CapabilityMissing {
                space: self.inner.id.to_string(),
                feature: name.into(),
                limitation: format!(
                    "cua-spacesd {} does not report this feature",
                    self.inner.caps.version
                ),
            }),
        }
    }

    /// Fails unless at least one of `names` is supported; the error names the
    /// first.
    pub fn require_any(&self, names: &[&str]) -> Result<()> {
        if names.iter().any(|n| self.supports(n)) {
            return Ok(());
        }
        self.require(names.first().copied().unwrap_or_default())
    }

    /// The guest OS family.
    pub fn os_family(&self) -> pb::OsFamily {
        self.inner
            .caps
            .os
            .as_ref()
            .and_then(|o| pb::OsFamily::try_from(o.family).ok())
            .unwrap_or(pb::OsFamily::Unspecified)
    }

    /// Whether the guest is Windows (shell commands go through `cmd /C`).
    pub fn is_windows(&self) -> bool {
        self.os_family() == pb::OsFamily::Windows
    }

    /// Headers a WebSocket to this Space's spacesd needs besides its
    /// ticket (the Fleet gateway's bearer and claim).
    pub async fn websocket_headers(&self) -> Result<Vec<(String, String)>> {
        match &self.inner.gateway {
            None => Ok(vec![]),
            Some(Gateway::Fleet { fleet, claim }) => {
                let bearer = fleet.access_token(false).await?;
                let mut h = vec![
                    (
                        cua_proto::metadata::AUTHORIZATION.into(),
                        format!("Bearer {bearer}"),
                    ),
                    ("x-cua-fleet-claim".into(), claim.clone()),
                ];
                // The gateway consumes `authorization`; the env token rides
                // in its own header (MEDIA.md: tickets still authenticate
                // the socket, this lets a gateway-side check pass too).
                if let Some(t) = self.inner.token.as_deref().filter(|t| !t.is_empty()) {
                    h.push((
                        cua_proto::metadata::ENV_AUTHORIZATION.into(),
                        format!("Bearer {t}"),
                    ));
                }
                Ok(h)
            }
            Some(Gateway::Headers(h)) => Ok(h.clone()),
        }
    }

    /// Absolute `ws(s)://` URL for a driver-relative path such as the
    /// `ws_path` of a media, tunnel or hotspot ticket.
    pub fn websocket_url(&self, ws_path: &str) -> Result<String> {
        Ok(self.spacesd()?.endpoint().ws_url(ws_path))
    }
}

/// A registered Space as listed, whether or not it is connected.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SpaceInfo {
    /// Canonical id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `fleet`, `local` or `direct`.
    pub provider: Provider,
    /// spacesd version at the last handshake.
    pub spacesd_version: String,
    /// Supported features at the last handshake.
    pub features: Vec<String>,
    /// Guest OS family at the last handshake: `linux`, `macos`, `windows`,
    /// or empty when not reported.
    pub os: String,
    /// Guest OS product or distribution at the last handshake ("Ubuntu",
    /// "macOS"), or empty when not reported.
    #[serde(default)]
    pub os_name: String,
    /// The guest's full OS string at the last handshake ("Ubuntu 24.04.3
    /// LTS"), or empty when the driver does not report one.
    #[serde(default)]
    pub os_pretty_name: String,
    /// The image the sandbox runs, or empty when unknown (added by address).
    #[serde(default)]
    pub image: String,
    /// The digest of the variant that runs, or empty.
    #[serde(default)]
    pub image_digest: String,
    /// `container` or `vm`, or empty when unknown.
    #[serde(default)]
    pub kind: String,
    /// The guest's CPU architecture (`arm64`, `amd64`), or empty.
    #[serde(default)]
    pub arch: String,
    /// Declared services (never `env`), for example `mcp`.
    #[serde(default)]
    pub services: Vec<String>,
    /// When it was added (RFC 3339), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
    /// For a Space one of your machines provides (`on="host:<machine>"`):
    /// that host's relay machine id, or for a host added by its direct
    /// address, that host's Space id (`direct:<addr>:<port>`). Empty
    /// otherwise. Lists group these Spaces under their host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// The host's display name, when known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_name: String,
    /// How it turns off and on again ([`crate::Spaces::stop`],
    /// [`crate::Spaces::start`]): `suspend` (its memory is kept), `stop`
    /// (its disk is kept), or empty when it cannot.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub power: String,
    /// `running`, `suspended` or `stopped` as cua last recorded it; empty
    /// when unknown (a Space one of your machines provides is reachable
    /// while it runs).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub power_state: String,
    /// A Space in your own cloud: the provider (`aws`, `gcp`, `modal`);
    /// empty otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cloud: String,
    /// Where it runs ("AWS · us-west-2").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cloud_place: String,
    /// How this device would delete it permanently: `here` (this home
    /// created and records it), `host:<machine>` (ask the device that
    /// created it, through the relay) or `elsewhere` (only there). Empty
    /// for other Spaces.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cloud_delete: String,
}

impl SpaceInfo {
    pub(crate) fn from_record(record: &cua_proto::daemon::v1::Space) -> Result<Self> {
        let id = SpaceId::parse(&record.id)?;
        Ok(Self {
            id: record.id.clone(),
            name: record.name.clone(),
            provider: id.provider(),
            spacesd_version: record.spacesd_version.clone(),
            features: record.features.clone(),
            os: record.os.clone(),
            os_name: record.os_name.clone(),
            os_pretty_name: record.os_pretty_name.clone(),
            image: record.image.clone(),
            image_digest: record.image_digest.clone(),
            kind: record.kind.clone(),
            arch: record.arch.clone(),
            services: record.services.clone(),
            added_at: record.added_at.as_ref().map(|t| {
                let secs = t.seconds.max(0) as u64;
                let at =
                    std::time::UNIX_EPOCH + std::time::Duration::new(secs, t.nanos.max(0) as u32);
                humantime_rfc3339(at)
            }),
            host: record.host.clone(),
            host_name: record.host_name.clone(),
            power: record.power.clone(),
            power_state: record.power_state.clone(),
            cloud: record.cloud.clone(),
            cloud_place: record.cloud_place.clone(),
            cloud_delete: record.cloud_delete.clone(),
        })
    }
}

/// What [`crate::Spaces::stop`] or [`crate::Spaces::start`] did.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpacePower {
    /// The Space.
    pub space: String,
    /// `running`, `suspended` or `stopped`: the state it is in now.
    pub state: String,
    /// How it turns off: `suspend` or `stop`.
    pub power: String,
    /// What happened, for people.
    pub message: String,
}

impl SpacePower {
    /// The report for `space`, now in `state`, turned off with `control`.
    pub fn new(
        space: &str,
        control: cua_sandbox_core::PowerControl,
        state: cua_sandbox_core::PowerState,
    ) -> Self {
        use cua_sandbox_core::{PowerControl, PowerState};
        let message = match (state, control) {
            (PowerState::Running, PowerControl::Suspend) => format!("Resumed {space}."),
            (PowerState::Running, PowerControl::Stop) => format!("Started {space}."),
            (PowerState::Suspended, _) => format!("Suspended {space} (its memory is kept)."),
            (PowerState::Stopped, _) => format!("Stopped {space} (its disk is kept)."),
        };
        Self {
            space: space.into(),
            state: state.as_str().into(),
            power: control.as_str().into(),
            message,
        }
    }
}

fn humantime_rfc3339(at: std::time::SystemTime) -> String {
    humantime::format_rfc3339_seconds(at).to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_formats_known_instants() {
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_758_300_000);
        assert_eq!(super::humantime_rfc3339(at), "2025-09-19T16:40:00Z");
        assert_eq!(
            super::humantime_rfc3339(std::time::UNIX_EPOCH),
            "1970-01-01T00:00:00Z"
        );
    }
}
