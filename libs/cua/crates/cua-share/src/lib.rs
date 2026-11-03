//! Share a link to anything you run (`cua share`).
//!
//! This crate is the core: what a share is ([`model`]), who may open it
//! ([`access`]), the daemon's records ([`store`]), the read-only file server
//! ([`files`]), public-link rate limits ([`limiter`]) and the [`gateway`] that
//! checks all of that for each request and serves or forwards it. The cua
//! daemon owns the records and runs the gateway on a Unix socket; the host's
//! relay-joined cua-spacesd forwards `/share/...` requests to it with the
//! account the relay verified, so the relay's routing and sign-in are the
//! only way in. Nothing here listens on a network address.

pub mod access;
pub mod files;
pub mod gateway;
pub mod limiter;
pub mod model;
pub mod notices;
pub mod probe;
pub mod store;

mod error;

pub use access::{CALLER_HEADER, Caller, Verdict, decide};
pub use error::{Error, Result};
pub use gateway::{AuditSink, Gateway, GatewayConfig, GatewayHandle, UpstreamResolver};
pub use model::{
    Access, DEFAULT_PUBLIC_TTL, ID_LEN, MAX_TTL, MIN_TTL, Owner, Share, TOKEN_LEN, Target,
    check_ttl, now_ms, random_hex,
};
pub use probe::Probe;
#[cfg(unix)]
pub use probe::probe_unix;
pub use store::ShareStore;

/// Error type of response bodies.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
/// Body type of gateway responses.
pub type GwBody = http_body_util::combinators::BoxBody<bytes::Bytes, BoxError>;
