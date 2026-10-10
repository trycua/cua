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
#[cfg(test)]
mod setup_tests;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod tests;

pub use device::{DeviceAuth, DeviceKey, FileKeySlot, KeySlot, MemoryKeySlot, PendingCode};
pub use host::SetupStage;
pub use host::{
    DirectHosting, Host, HostConfig, HostMode, HostPaths, HostPolicy, HostSettingsChange,
    HostStatus, META_ARCH, META_OS, META_PROVIDES_SPACES, PermissionHint, SetupOptions,
    SpacesDaemon, local_network_hint, permission_hints,
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

/// The default display name of this device: the macOS Computer Name, else
/// the host name without a `.local` / `.localdomain` suffix.
pub fn device_name() -> String {
    host::friendly_name()
}

/// The relay this machine talks to: `flag` (e.g. `--relay`), else
/// `CUA_RELAY_URL`, else the relay this machine is set up with
/// (`<home>/host/config.json`), else the default relay. The CLI and the
/// app resolve it the same way, so both enroll and approve devices on one
/// relay.
pub fn relay_url_for(flag: Option<&str>, home: &std::path::Path) -> String {
    flag.map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("CUA_RELAY_URL")
                .ok()
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
        })
        .or_else(|| {
            Host::new(home)
                .config()
                .ok()
                .flatten()?
                .relay_url
                .filter(|u| !u.trim().is_empty())
        })
        .unwrap_or_else(relay_url_from_env)
}

/// Where this device remembers the one-time code it last showed (see
/// [`device::PendingCode`]).
pub fn device_pending_file(home: &std::path::Path) -> std::path::PathBuf {
    home.join("device-pending.json")
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

impl Error {
    /// The HTTP status the relay or the download server answered with,
    /// when one did. None: the server was not reached (offline, DNS, TLS,
    /// timeout) or the error is not about HTTP at all.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            // `relay::http_error` maps these statuses to these variants.
            Error::Unauthenticated(m) if m.starts_with("relay: ") => Some(401),
            Error::PermissionDenied(m) if m.starts_with("relay: ") => Some(403),
            Error::NotFound(m) if m.starts_with("relay: ") => Some(404),
            Error::Conflict(m) if m.starts_with("relay: ") => Some(409),
            Error::Relay(m) | Error::Download(m) => status_in(m),
            _ => None,
        }
    }
}

/// The status in an "HTTP 503: ..." / "<url>: HTTP 404 Not Found" message.
fn status_in(message: &str) -> Option<u16> {
    message.match_indices("HTTP ").find_map(|(i, _)| {
        let digits = message.get(i + 5..i + 8)?;
        let code: u16 = digits.parse().ok()?;
        (100..600).contains(&code).then_some(code)
    })
}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;
