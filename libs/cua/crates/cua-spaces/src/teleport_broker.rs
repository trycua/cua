//! The seam between the `teleport_app` MCP tool and the Cua Keyvault broker.
//!
//! The tool must never mint its own consent (an LLM could replay it) and must
//! never perform delivery itself (red-team E2). It only files a request and,
//! on a retry, asks the broker for the decision. When the request is approved
//! (live consent with Touch ID, or a matching unattended rule) the broker,
//! running in the daemon's own hardened process, performs the delivery over
//! the daemon's authenticated spacesd channel.
//!
//! The trait keeps `cua-spaces` free of a `cua-keyvault` dependency: the
//! daemon supplies the implementation (see `cua_daemon::keyvault`). With no
//! implementation wired in, the tool is fail-closed: it returns a consent
//! requirement and delivers nothing.

use std::time::Duration;

/// A Keyvault failure, surfaced to the tool with its code.
#[derive(Clone, Debug)]
pub struct SessionBrokerError {
    /// The Keyvault error code (`requires_cua_app`, `denied`, `disabled`, ...).
    pub code: String,
    /// A human-readable message (never a secret).
    pub message: String,
}

/// The outcome of awaiting and (when granted) performing a delivery.
#[derive(Clone, Debug)]
pub enum SessionDelivery {
    /// The user has not decided yet.
    Pending,
    /// The user declined (with the reason).
    Denied(String),
    /// The broker delivered the approved items to the target.
    Delivered {
        /// App id.
        app: String,
        /// Target Space.
        target: String,
        /// Vault item ids delivered.
        items: Vec<String>,
        /// The paths the receiver imported (relative to the app's profile).
        imported: Vec<String>,
        /// Receiver import ids (for later wipe).
        import_ids: Vec<String>,
        /// When the target wipes the copy (Unix ms).
        expires_ms: u64,
    },
}

/// The Keyvault operations `teleport_app` uses. The implementation runs the
/// call under an unverified third-party identity, so a delivery needs a live
/// consent or an audited unattended rule.
#[async_trait::async_trait]
pub trait SessionBroker: Send + Sync {
    /// Files an access request for the whole app session into `target`.
    /// Returns the request id. Moves nothing.
    async fn request_access(
        &self,
        app: &str,
        target: &str,
        duration_secs: u64,
        reason: &str,
    ) -> Result<String, SessionBrokerError>;

    /// Waits up to `timeout` for the decision on `request_id` and, when it is
    /// granted, performs the delivery through the broker.
    async fn await_and_deliver(
        &self,
        app: &str,
        target: &str,
        request_id: &str,
        timeout: Duration,
    ) -> Result<SessionDelivery, SessionBrokerError>;
}
