//! One error type for every Spaces call, with a stable machine tag.
//!
//! The tag ([`Error::tag`]) is what bindings and the MCP server surface to
//! callers that branch on the kind of failure; the message is for humans.
//! Two kinds matter most and are never folded into anything else:
//!
//! - [`Error::SpacesdNotAvailable`]: the machine answered, but it is not a
//!   Space (no cua-spacesd). Only the Spaces primitives need the driver;
//!   sandboxes in general do not (plan §1.1).
//! - [`Error::CapabilityMissing`]: the driver is there but reports the feature
//!   a primitive needs as unsupported, with the driver's own limitation text.

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Spaces errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Bad input from the caller.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// No Space (or run, or file) with this id.
    #[error("{0} not found")]
    NotFound(String),
    /// The target has no cua-spacesd, and nothing else answered either
    /// (a Space without spacesd needs a declared service or an MCP URL).
    #[error(
        "{space} has no cua-spacesd ({reason}); the spacesd primitives need an \
         image that runs it, for example ghcr.io/trycua/linux:24.04"
    )]
    SpacesdNotAvailable {
        /// The Space or URL.
        space: String,
        /// Why the probe failed.
        reason: String,
    },
    /// The Space's spacesd reports a required feature as unsupported.
    #[error("{space} does not support `{feature}`{}", limitation_suffix(.limitation))]
    CapabilityMissing {
        /// The Space.
        space: String,
        /// The spacesd feature name (`GetCapabilities.features[].name`).
        feature: String,
        /// The driver's own limitation text, if any.
        limitation: String,
    },
    /// A host-side prerequisite (Fleet credentials, a local runtime, an
    /// operator display, app-session providers) is not configured.
    #[error("{what} is not available on this host: {why}")]
    HostCapabilityMissing {
        /// Which prerequisite (`cua_spaces_contract::host::*`).
        what: String,
        /// How to get it.
        why: String,
    },
    /// The operation does not apply to this kind of Space.
    #[error("{op} is not supported for {provider} Spaces")]
    WrongProvider {
        /// The operation.
        op: String,
        /// `fleet`, `local` or `direct`.
        provider: String,
    },
    /// A transfer did not land, or landed corrupted.
    #[error("transfer failed: {0}")]
    Transfer(String),
    /// The teleport consent gate refused.
    #[error("teleport refused: {0}")]
    TeleportRefused(String),
    /// The Keyvault refused a site login (declined, no saved login for the
    /// origin, a tab on another origin, a locked or disabled Keyvault).
    #[error("site login refused: {0}")]
    LoginRefused(String),
    /// An agent-run failure that is not a transport error.
    #[error("agent: {0}")]
    Agent(String),
    /// A wait ran out.
    #[error("timed out: {0}")]
    Timeout(String),
    /// Media plane failure.
    #[error("stream: {0}")]
    Stream(String),
    /// spacesd RPC failure.
    #[error(transparent)]
    Env(#[from] cua_spacesd_client::Error),
    /// Fleet control-plane failure.
    #[error(transparent)]
    Fleet(#[from] cua_fleet::Error),
    /// Sandbox lifecycle failure.
    #[error(transparent)]
    Sandbox(cua_sandbox_core::Error),
    /// Local I/O.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// cua-relay directory / account failure.
    #[error(transparent)]
    Relay(#[from] cua_host::Error),
    /// A generic MCP service inside the Space failed (rmcp).
    #[error("mcp: {0}")]
    Mcp(String),
    /// A host is at a capacity limit (two macOS VMs per Mac by Apple's
    /// license, or the host's own Space limit).
    #[error("{0}")]
    LimitExceeded(String),
    /// A machine name (`on="host:<name>"`) matches more than one of your
    /// machines equally well.
    #[error("{message}")]
    AmbiguousHost {
        /// The name as given.
        query: String,
        /// `host:<id>` of every machine that fits.
        choices: Vec<String>,
        /// For people: the choices with their names.
        message: String,
    },
    /// A Spaces extension (the Cua Volume, teleport, persistent agents: see
    /// [`crate::extension`]) refused or failed, with the stable kind its
    /// tools report (`forbidden`, `lease_held`, `volume_backend`, ...).
    #[error("{message}")]
    Extension {
        /// One of [`cua_spaces_contract::ERROR_KINDS`].
        kind: &'static str,
        /// For humans.
        message: String,
    },
    /// The create was cancelled ([`crate::Spaces::cancel_create`], or its
    /// caller went away); what it made is gone. The message says what was
    /// removed and what stays.
    #[error("{0}")]
    Cancelled(String),
}

fn limitation_suffix(limitation: &str) -> String {
    if limitation.is_empty() {
        String::new()
    } else {
        format!(": {limitation}")
    }
}

impl Error {
    /// A stable, lowercase kind for callers that branch on failures.
    pub fn tag(&self) -> &'static str {
        match self {
            Error::InvalidArgument(_) => "invalid_argument",
            Error::NotFound(_) => "not_found",
            Error::SpacesdNotAvailable { .. } => "spacesd_not_available",
            Error::CapabilityMissing { .. } => "capability_missing",
            Error::HostCapabilityMissing { .. } => "host_capability_missing",
            Error::WrongProvider { .. } => "wrong_provider",
            Error::Transfer(_) => "transfer_failed",
            Error::TeleportRefused(_) => "teleport_refused",
            Error::LoginRefused(_) => "login_refused",
            Error::Agent(_) => "agent",
            Error::Timeout(_) => "timeout",
            Error::Stream(_) => "stream",
            Error::Env(e) => match e {
                cua_spacesd_client::Error::SpacesdNotAvailable { .. } => "spacesd_not_available",
                cua_spacesd_client::Error::Unauthenticated(_) => "unauthenticated",
                _ => "env",
            },
            Error::Fleet(cua_fleet::Error::AdmissionDenied { .. }) => "fleet_admission_denied",
            Error::Fleet(cua_fleet::Error::CreditExhausted { .. }) => "cloud_credit_exhausted",
            Error::Fleet(_) => "fleet",
            Error::Sandbox(cua_sandbox_core::Error::AmbiguousSandbox { .. }) => "ambiguous_sandbox",
            Error::Sandbox(cua_sandbox_core::Error::NotFound(_)) => "not_found",
            Error::Sandbox(cua_sandbox_core::Error::InvalidPlacement(_)) => "invalid_placement",
            Error::Sandbox(_) => "sandbox",
            Error::Io(_) => "io",
            Error::Mcp(_) => "mcp",
            Error::Json(_) => "json",
            Error::LimitExceeded(_) => "limit_exceeded",
            Error::AmbiguousHost { .. } => "ambiguous_host",
            Error::Extension { kind, .. } => kind,
            Error::Cancelled(_) => "cancelled",
            Error::Relay(e) => match e {
                cua_host::Error::Unauthenticated(_) => "unauthenticated",
                cua_host::Error::PermissionDenied(_) => "permission_denied",
                cua_host::Error::NotFound(_) => "not_found",
                _ => "relay",
            },
        }
    }

    /// An extension failure of `kind`: the matching
    /// [`cua_spaces_contract::ERROR_KINDS`] entry, else `env`.
    pub fn extension(kind: &str, message: impl Into<String>) -> Self {
        let kind = cua_spaces_contract::ERROR_KINDS
            .iter()
            .map(|k| k.kind)
            .find(|k| *k == kind)
            .unwrap_or("env");
        Error::Extension {
            kind,
            message: message.into(),
        }
    }

    /// A capability that ships with Cua Spaces (teleport, the Cua Volume,
    /// persistent agents) and that no registered extension provides.
    pub fn needs_cua_spaces(what: &str) -> Self {
        Error::HostCapabilityMissing {
            what: what.into(),
            why: format!(
                "{what} ships with Cua Spaces (source-available, FSL-1.1-MIT); \
                 connect to the daemon Cua Spaces runs (`cua daemon` from the Cua Spaces app) \
                 or register the Cua Spaces extensions in this process"
            ),
        }
    }

    /// An [`Error::InvalidArgument`].
    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::InvalidArgument(msg.into())
    }

    /// An [`Error::HostCapabilityMissing`].
    pub fn host(what: &str, why: impl Into<String>) -> Self {
        Error::HostCapabilityMissing {
            what: what.into(),
            why: why.into(),
        }
    }
}

impl From<cua_sandbox_core::Error> for Error {
    fn from(e: cua_sandbox_core::Error) -> Self {
        match e {
            cua_sandbox_core::Error::SpacesdNotAvailable { sandbox, reason } => {
                Error::SpacesdNotAvailable {
                    space: sandbox,
                    reason,
                }
            }
            cua_sandbox_core::Error::Env(e) => Error::Env(e),
            cua_sandbox_core::Error::Fleet(e) => Error::Fleet(e),
            cua_sandbox_core::Error::NotFound(n) => Error::NotFound(n),
            cua_sandbox_core::Error::Mcp(e) => Error::Mcp(e),
            cua_sandbox_core::Error::Cloud(m) => Error::extension("cloud", m),
            // A refusal is the caller's to fix, not a runtime failure.
            cua_sandbox_core::Error::InvalidArgument(m) => Error::InvalidArgument(m),
            cua_sandbox_core::Error::Cancelled(m) => Error::Cancelled(m),
            cua_sandbox_core::Error::ProviderNotConfigured(p) => Error::host(
                match p {
                    cua_sandbox_core::ProviderKind::Fleet => cua_spaces_contract::host::FLEET,
                    _ => cua_spaces_contract::host::LOCAL_RUNTIME,
                },
                format!("the {p:?} provider is not configured"),
            ),
            other => Error::Sandbox(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use std::collections::HashMap;

    /// Every tag a tool error can carry is documented in the contract's
    /// `ERROR_KINDS`, which the Spaces reference renders.
    #[test]
    fn every_tag_is_a_documented_error_kind() {
        let details = || cua_spacesd_client::ErrorDetails {
            code: 16,
            message: "no".into(),
            metadata: HashMap::new(),
        };
        let errors = vec![
            Error::InvalidArgument("x".into()),
            Error::NotFound("x".into()),
            Error::SpacesdNotAvailable {
                space: "x".into(),
                reason: "x".into(),
            },
            Error::CapabilityMissing {
                space: "x".into(),
                feature: "x".into(),
                limitation: String::new(),
            },
            Error::host("x", "x"),
            Error::WrongProvider {
                op: "x".into(),
                provider: "x".into(),
            },
            Error::Transfer("x".into()),
            Error::TeleportRefused("x".into()),
            Error::LoginRefused("x".into()),
            Error::Agent("x".into()),
            Error::Timeout("x".into()),
            Error::Cancelled("x".into()),
            Error::Stream("x".into()),
            Error::Env(cua_spacesd_client::Error::Unauthenticated(details())),
            Error::Env(cua_spacesd_client::Error::Transport("x".into())),
            Error::Fleet(cua_fleet::Error::Timeout("x".into())),
            Error::Fleet(cua_fleet::Error::AdmissionDenied {
                operation: "create template".into(),
                status: 403,
                message: "x".into(),
            }),
            Error::Fleet(cua_fleet::Error::CreditExhausted {
                message: "x".into(),
                billing_url: "https://x".into(),
            }),
            Error::Sandbox(cua_sandbox_core::Error::AmbiguousSandbox {
                name: "x".into(),
                candidates: vec![],
                message: "x".into(),
            }),
            Error::Sandbox(cua_sandbox_core::Error::Timeout("x".into())),
            Error::Io(std::io::Error::other("x")),
            Error::Json(serde_json::from_str::<()>("{").unwrap_err()),
            Error::Relay(cua_host::Error::Unauthenticated("x".into())),
            Error::Relay(cua_host::Error::PermissionDenied("x".into())),
            Error::Relay(cua_host::Error::Relay("x".into())),
            Error::Mcp("x".into()),
            Error::extension("forbidden", "x"),
            Error::extension("lease_held", "x"),
            Error::extension("volume_backend", "x"),
            Error::extension("no such kind", "x"),
            Error::needs_cua_spaces("teleport"),
            Error::LimitExceeded("x".into()),
            Error::AmbiguousHost {
                query: "x".into(),
                choices: vec![],
                message: "x".into(),
            },
        ];
        for e in errors {
            let tag = e.tag();
            assert!(
                cua_spaces_contract::ERROR_KINDS
                    .iter()
                    .any(|k| k.kind == tag),
                "error kind {tag} is not in cua_spaces_contract::ERROR_KINDS"
            );
        }
    }
}
