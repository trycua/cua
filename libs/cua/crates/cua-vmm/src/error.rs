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
