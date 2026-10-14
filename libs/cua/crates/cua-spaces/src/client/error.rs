//! One flat error type. `#[uniffi(flat_error)]` is applied where it crosses
//! the boundary, in `cua-spaces-sdk`; here it is plain Rust so the core can be
//! unit-tested without FFI machinery.
//!
//! The case names are the Swift package's case names, deliberately, so a
//! caller migrating from `libs/spaces-sdk-swift` can move its `catch` sites
//! mechanically.

/// Everything that can go wrong, on the language's error channel.
///
/// `FRICTION.md` §3: the wire protocol reports a failing tool as a
/// *successful* result carrying `isError: true`. The core's single job at that
/// seam is to make reading that as a value impossible — every tool failure is
/// an `Err`, and no control-plane call returns a value that might be a failure
/// wearing a value's clothes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpacesError {
    #[error("spaces transport unavailable: {0}")]
    TransportUnavailable(String),

    /// The stdio stream produced something that is not a framed JSON-RPC
    /// message. `FRICTION.md` §1 — a desync here is undetectable from the app
    /// side, so any framing anomaly is fatal rather than guessed at.
    #[error("spaces transport framing: {0}")]
    TransportFraming(String),

    /// A tool ran and failed — an `isError: true` result.
    ///
    /// Deliberately **not** the same case as [`SpacesError::RpcFailed`]. The
    /// server reports a failing tool as a *successful* JSON-RPC response
    /// carrying `isError: true`; the only genuine JSON-RPC `error` objects it
    /// sends are `-32601` for an unknown method or tool. Collapsing the two
    /// would make "the sandbox refused this command" indistinguishable from
    /// "the control plane is not running", and those need different handling
    /// by every caller.
    #[error("spaces tool {tool} failed: {message}")]
    ToolFailed { tool: String, message: String },

    /// The peer answered with a JSON-RPC `error` object: the method or tool
    /// does not exist, or the control plane is not serving. Beneath the tool
    /// call, never a tool's own verdict.
    #[error("spaces rpc {method} failed ({code}): {message}")]
    RpcFailed {
        method: String,
        code: i64,
        message: String,
    },

    #[error("unexpected {tool} response: {detail}")]
    MalformedResponse { tool: String, detail: String },

    #[error("{tool} is not available on {provider} Spaces: {detail}")]
    UnsupportedByProvider {
        tool: String,
        provider: String,
        detail: String,
    },

    #[error("Space {0} unavailable: {1}")]
    SpaceUnavailable(String, String),

    #[error("no such run: {0}")]
    RunNotFound(String),

    #[error("{0}")]
    LimitExceeded(String),

    #[error("refusing to provision a Space: {0}")]
    WouldProvision(String),

    #[error("host path not found: {0}")]
    LocalFileUnavailable(String),

    #[error("timed out waiting for {waiting_for} after {after_ms}ms")]
    TimedOut { waiting_for: String, after_ms: u64 },

    #[error("not implemented yet: {0}")]
    NotImplementedYet(String),

    #[error("teleport refused: {0}")]
    TeleportRefused(String),
}

pub type Result<T> = std::result::Result<T, SpacesError>;

impl SpacesError {
    /// The stable discriminator a binding puts on its error object, so a
    /// TypeScript `catch` can branch without string-matching a message.
    pub fn tag(&self) -> &'static str {
        match self {
            SpacesError::TransportUnavailable(_) => "TransportUnavailable",
            SpacesError::TransportFraming(_) => "TransportFraming",
            SpacesError::ToolFailed { .. } => "ToolFailed",
            SpacesError::RpcFailed { .. } => "RpcFailed",
            SpacesError::MalformedResponse { .. } => "MalformedResponse",
            SpacesError::UnsupportedByProvider { .. } => "UnsupportedByProvider",
            SpacesError::SpaceUnavailable(..) => "SpaceUnavailable",
            SpacesError::RunNotFound(_) => "RunNotFound",
            SpacesError::LimitExceeded(_) => "LimitExceeded",
            SpacesError::WouldProvision(_) => "WouldProvision",
            SpacesError::LocalFileUnavailable(_) => "LocalFileUnavailable",
            SpacesError::TimedOut { .. } => "TimedOut",
            SpacesError::NotImplementedYet(_) => "NotImplementedYet",
            SpacesError::TeleportRefused(_) => "TeleportRefused",
        }
    }

    pub(crate) fn malformed(tool: &str, detail: impl Into<String>) -> Self {
        SpacesError::MalformedResponse {
            tool: tool.to_string(),
            detail: detail.into(),
        }
    }
}
