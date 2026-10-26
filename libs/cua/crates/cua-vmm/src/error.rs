//! Error type shared by every backend.
//!
//! Messages are written for the person who will read them in a terminal: when a
//! prerequisite is missing, the error says what is missing *and* how to get it
//! (or which option lets the SDK provision it automatically).

use std::path::PathBuf;

/// Result alias used throughout the crate.
pub type Result<T, E = VmmError> = std::result::Result<T, E>;

/// Every failure a runtime can report.
#[derive(Debug, thiserror::Error)]
pub enum VmmError {
    /// A host prerequisite (binary, daemon, firmware) is missing. `hint` is an
    /// actionable next step for the user.
    #[error("{what} is not available: {hint}")]
    Missing { what: String, hint: String },

    /// A host prerequisite is installed but not running (a container
    /// engine whose socket accepts nothing). `hint` says how to start it.
    #[error("{what} is not running: {hint}")]
    NotRunning { what: String, hint: String },

    /// This user may not use a host prerequisite (the engine socket, KVM).
    /// The message names it and how to get access.
    #[error("{0}")]
    PermissionDenied(String),

    /// No instance with that name is known to the backend.
    #[error("sandbox '{0}' not found")]
    NotFound(String),

    /// An instance with that name already exists where a fresh one was needed.
    #[error("sandbox '{0}' already exists")]
    AlreadyExists(String),

    /// The operation is not meaningful for this backend.
    #[error("{backend} does not support {op}")]
    Unsupported {
        backend: &'static str,
        op: &'static str,
    },

    /// The image exists but cannot run on any local backend (for example a
    /// macOS image in no Lume format). `reason` says what to use instead.
    #[error("{reason}")]
    UnsupportedImage { image: String, reason: String },

    /// This host has no room for another VM now: its VMs are at their
    /// memory or CPU limit ([`crate::density`]), or the machine is low on
    /// memory. The message says which, by how much, and what to do.
    #[error("{0}")]
    HostCapacity(String),

    /// The request was malformed (bad name, missing disk, conflicting options).
    #[error("invalid request: {0}")]
    Invalid(String),

    /// The instance did not become ready (running + probes) in time.
    #[error("sandbox '{name}' was not ready after {secs}s: {detail}")]
    Timeout {
        name: String,
        secs: u64,
        detail: String,
    },

    /// An image download ended before it finished: Lume's pull failed, was
    /// cancelled outside the SDK, or `lume serve` stopped under it. Lume
    /// keeps the layers it finished, so another try picks up from there.
    #[error(
        "The image download stopped before it finished ({reason}). Try again; it picks up where it stopped."
    )]
    PullStopped { name: String, reason: String },

    /// A spawned host command failed.
    #[error("`{cmd}` failed (exit {code:?}): {stderr}")]
    Command {
        cmd: String,
        code: Option<i32>,
        stderr: String,
    },

    /// The Lume HTTP API returned an error.
    #[error("lume API {status}: {message}")]
    Lume { status: u16, message: String },

    /// The container engine returned an error.
    #[error("container engine: {0}")]
    Engine(String),

    /// QMP protocol error or a QMP command returned `error`.
    #[error("QMP: {0}")]
    Qmp(String),

    /// A state file could not be parsed.
    #[error("corrupt state file {path}: {detail}")]
    State { path: PathBuf, detail: String },

    /// The operation would leave less than the configured minimum free
    /// disk space (`cua cache prune` frees space).
    #[error(transparent)]
    InsufficientDisk(#[from] crate::disk::InsufficientDisk),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Anything else, with context.
    #[error("{0}")]
    Other(String),
}

/// Why a local create failed, as a bounded word for telemetry (never the
/// message): `runtime_missing`, `runtime_not_running`,
/// `runtime_permission`, `image_pull`, else `None`.
pub fn failure_kind(message: &str) -> Option<&'static str> {
    let m = message.to_ascii_lowercase();
    if m.contains("container engine: pull ")
        || m.contains("manifest unknown")
        || m.contains("pull access denied")
        || m.contains("toomanyrequests")
        || m.contains("the image download stopped before it finished")
    {
        Some("image_pull")
    } else if m.contains("permission denied") {
        Some("runtime_permission")
    } else if m.contains(" is not running: ") {
        Some("runtime_not_running")
    } else if m.contains(" is not available: ")
        && !m.contains("local network access")
        && !m.contains("network route")
    {
        Some("runtime_missing")
    } else {
        None
    }
}

impl VmmError {
    pub fn missing(what: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::Missing {
            what: what.into(),
            hint: hint.into(),
        }
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each cause classifies from the message the create shows, also when
    /// a caller wrapped it (`local runtime: [local] ...`).
    #[test]
    fn failure_kind_splits_runtime_causes() {
        let wrap = |e: VmmError| format!("local runtime: [local] {e}");
        let missing = VmmError::missing("a container engine", "no Docker-API socket found");
        assert_eq!(failure_kind(&wrap(missing)), Some("runtime_missing"));
        let down = VmmError::NotRunning {
            what: "container engine at unix:///var/run/docker.sock".into(),
            hint: "start it with `sudo systemctl start docker`".into(),
        };
        assert_eq!(failure_kind(&wrap(down)), Some("runtime_not_running"));
        let denied = VmmError::PermissionDenied(
            "permission denied on unix:///var/run/docker.sock: add yourself".into(),
        );
        assert_eq!(failure_kind(&wrap(denied)), Some("runtime_permission"));
        let pull = VmmError::Engine("pull ghcr.io/x/y:1: manifest unknown".into());
        assert_eq!(failure_kind(&wrap(pull)), Some("image_pull"));
        let denied_pull = VmmError::Engine("pull x: pull access denied for x".into());
        assert_eq!(failure_kind(&denied_pull.to_string()), Some("image_pull"));
        let stopped = VmmError::PullStopped {
            name: "cua-base-x".into(),
            reason: "Lume stopped running".into(),
        };
        assert_eq!(failure_kind(&wrap(stopped)), Some("image_pull"));
        assert_eq!(failure_kind(&VmmError::other("boom").to_string()), None);
        let lan = VmmError::missing("Local Network access", "click Allow");
        assert_eq!(failure_kind(&lan.to_string()), None);
    }
}
