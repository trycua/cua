//! Set this machine up for unattended access ("host" mode).
//!
//! [`Host::setup`] installs cua-spacesd as a per-OS service
//! ([`service`]) that either joins a `cua-relay` with this machine
//! registered to the signed-in cua.ai account (the default: outbound WSS, no
//! port forwarding) or serves on a direct `ip:port` with a generated env
//! token (LAN / port-forwarded, the "Advanced" option). [`Host::status`],
//! [`Host::stop_sharing`], [`Host::start_sharing`] and [`Host::remove`]
//! manage it afterwards. The `cua host …` commands and the Cua Spaces app's
//! "This machine" entry both call this crate.
//!
//! [`relay`] is the machine-directory client (`GET /v1/machines`, …) that
//! the Spaces `relay:<id>` provider also uses; [`device`] enrolls this
//! device as a client of the account (hosting never does).
//!
//! A host has two independent settings ([`HostSpacesSettings`]): share its
//! own desktop, and provide Spaces to the owner's enrolled devices
//! ([`HostProfile::Spare`] turns the first off and the second on).
//! [`provided`] keeps the Spaces a host provides, their hash-chained audit,
//! and [`match_machine`], which resolves "spare mac mini" to a machine.
//! A host in direct mode provides Spaces without the relay ([`direct`]).
//!
//! State lives under `<cua home>/host` (`~/.cua/host`): `config.json`,
//! the driver policy `host.json`, the 0600 `machine-token` and `env-token`,
//! the installed binary `bin/cua-spacesd`, and `driver.log`. The machine
//! id is `<cua home>/spacesd/id`, the file `cua-spacesd join` reads.
//!
//! Nothing here grants OS permissions. On macOS [`HostStatus::permissions`]
//! lists the Screen Recording and Accessibility panes for the user to open.
//! [`preflight`] checks, before `setup` bootstraps the launchd service,
//! that this account has a GUI (Aqua) session to run it in (set up over
//! ssh can hit a machine with nobody logged in at the console).

pub mod access;
pub mod device;
pub mod direct;
pub mod driver;
mod host;
pub mod machine;
pub mod preflight;
pub mod provided;
pub mod relay;
pub mod service;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod tests;

pub use device::{DeviceAuth, DeviceKey, FileKeySlot, KeySlot, MemoryKeySlot};
pub use host::{
    DirectHosting, Host, HostConfig, HostMode, HostPaths, HostPolicy, HostSettingsChange,
    HostStatus, PermissionHint, SetupOptions, SpacesDaemon, permission_hints,
};
pub use machine::machine_id;
pub use provided::{
    HostProfile, HostSpacesSettings, MachineMatch, ProvidedSpace, SpacesAuditRecord, match_machine,
};
pub use relay::{
    AccountTokens, AuditEvent, ConnectedClient, DEFAULT_RELAY_URL, DeviceListing, DeviceState,
    DeviceView, Enrollment, Identity, Machine, MachinePatch, NoAccount, RelayClient, StaticToken,
    relay_url_from_env,
};
pub use service::{RunnerKind, ServiceManager, ServiceSpec, ServiceState};

/// The default display name of this device (its host name).
pub fn device_name() -> String {
    host::hostname()
}

/// The operating system a device reports when it registers (`macos`,
/// `windows`, `linux`, ...).
pub fn device_platform() -> &'static str {
    std::env::consts::OS
}

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Bad input.
    #[error("{0}")]
    InvalidArgument(String),
    /// Not signed in / token refused.
    #[error("{0}")]
    Unauthenticated(String),
    /// Signed in but not allowed.
    #[error("{0}")]
    PermissionDenied(String),
    /// Unknown machine / not configured.
    #[error("{0}")]
    NotFound(String),
    /// Machine id owned by another account.
    #[error("{0}")]
    Conflict(String),
    /// Relay unreachable or failed.
    #[error("relay: {0}")]
    Relay(String),
    /// Driver download failed.
    #[error("download: {0}")]
    Download(String),
    /// Service install / control failed.
    #[error("service: {0}")]
    Service(String),
    /// Filesystem.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Anything else.
    #[error("{0}")]
    Internal(String),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;
