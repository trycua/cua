//! Coding agents inside sandboxes.
//!
//! - [`harness`]: the harnesses and how each is installed, authenticated
//!   and pointed at an endpoint; every one is driven over the Agent Client
//!   Protocol (natively or through its maintained adapter).
//! - [`installables`]: one pinned, verified "ensure X is installed" for
//!   harnesses and apps.
//! - [`runs`]: detached runs whose state lives in the sandbox.
//! - [`events`]: the normalized event stream.
//!
//! Works over any [`cua_spacesd_client::SpacesdClient`]: a Space, a local container or
//! VM, or a Fleet sandbox.

pub mod events;
pub mod harness;
pub mod installables;
pub mod runs;
pub mod transcript;

pub use events::{AgentEvent, EventPage};
pub use harness::{Endpoint, Harness, Wire};
pub use runs::{
    Agents, Artifact, Attachment, McpServer, RunInfo, RunOptions, RunResult, RunStatus, Started,
};
pub use transcript::{Transcript, TranscriptItem};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("install failed: {0}")]
    Install(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("guest: {0}")]
    Guest(String),
    #[error(transparent)]
    Client(#[from] cua_spacesd_client::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    AgentSetup(#[from] cua_agent_setup::Error),
}

/// Result.
pub type Result<T> = std::result::Result<T, Error>;

/// POSIX shell quoting.
pub fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@%+=:,./-_".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}
