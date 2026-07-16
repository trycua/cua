//! Platform-agnostic browser-tool v1 core.
//!
//! Five typed tools — `get_browser_state` (strictly read-only),
//! `browser_prepare`, `browser_navigate`, `browser_click`,
//! `browser_type` — over an exact-or-refused binding model:
//!
//! - The native entrypoint is `pid + window_id`. Browser target ids,
//!   tab ids, and page refs (`p<snapshot>:<index>`) are opaque,
//!   session-scoped capabilities minted by core.
//! - Mutation is permitted only for **exact** bindings (unique
//!   bounds-correlated, optionally title-tie-broken). Heuristic
//!   bindings are read-only; everything else is a structured refusal
//!   with a stable code (see [`refusal::BrowserRefusalCode`]).
//! - Core owns schemas, semantics, CDP target handling, the target/ref
//!   store, lifecycle cleanup and correlation. Platform adapters
//!   implement [`platform::BrowserPlatform`] for process identity,
//!   endpoint ownership, native window metadata, classification and
//!   explicit setup — without depending on core internals.
//!
//! Wiring (per platform crate):
//! ```ignore
//! let engine = BrowserEngine::new(Arc::new(MyPlatformAdapter::new()));
//! register_browser_tools(&engine, &mut registry);
//! ```
//!
//! The legacy `crate::page` contract remains separate because its first-page
//! and URL-hint semantics predate exact browser capabilities. Its CDP paths
//! nevertheless reuse the pooled, loopback-validated WebSocket transport in
//! [`cdp_ws`] so the repository has one event-capable demultiplexer.
//!
//! v2 DOM-ref slice: snapshots compose shadow DOM (piercing, minus
//! user-agent roots), same-process iframes (via `contentDocument`),
//! and — only when the capability is proven live — OOPIF child targets
//! auto-attached beneath the bound tab's own session. Every ref
//! carries its frame's document identity (frame id + loader id), which
//! is re-proven before any mutation; frames whose identity or
//! capability cannot be proven are omitted or refused, never guessed.

pub mod approval;
pub mod binding;
pub mod cdp_ws;
pub mod engine;
mod grant;
#[cfg(test)]
pub(crate) mod mock_cdp;
mod mutation;
pub mod platform;
mod prepare;
mod reconnect;
pub mod refusal;
pub mod store;
pub mod tools;
pub mod types;
#[cfg(test)]
mod v2_tests;

pub use engine::BrowserEngine;
pub use platform::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserPlatform, PrepareAction,
    PrepareAttachment, PrepareAttachmentKind, PrepareAuthorization, PrepareOutcome, PrepareProfile,
    PrepareProfileMode, PrepareRequest, PrepareSideEffects, PrepareStrategy,
};
pub use refusal::{BrowserRefusal, BrowserRefusalCode};
pub use tools::register_browser_tools;
pub use types::{
    BindingQuality, BrowserClassification, BrowserEngineFamily, EndpointOwnershipMethod,
    EndpointOwnershipProof, NativeOwnershipMethod, NativeOwnershipProof, NativeWindowInfo,
    OwnedEndpoint, ProcessFingerprint, Rect,
};
