//! The platform-adapter contract for the browser-tool surface.
//!
//! Core owns schemas, semantics, CDP handling, the target/ref store,
//! lifecycle cleanup, and correlation. Platform crates own everything
//! that requires OS identity: process fingerprints, native window
//! metadata, browser classification, loopback-endpoint ownership, and
//! explicit endpoint setup. This trait is the entire boundary — a
//! platform crate implements it without depending on core internals,
//! and core never reaches into a platform crate.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::refusal::BrowserRefusal;
use super::types::{
    BrowserClassification, BrowserProduct, NativeWindowInfo, OwnedEndpoint, ProcessFingerprint,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrepareProfileMode {
    IsolatedNew,
    IsolatedNamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareProfile {
    pub mode: PrepareProfileMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Acting strategy for browser preparation that is not a driver-owned profile
/// lifecycle operation. Existing profiles remain owned by the user/browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrepareStrategy {
    ExistingProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareAuthorization {
    McpHost,
    ApprovalArtifact(String),
}

/// Caller context for an explicit `browser_prepare` call. Prepare is never
/// implicit: `get_browser_state` must not trigger it.
#[derive(Debug, Clone)]
pub struct PrepareRequest {
    pub pid: i64,
    /// Exact native window used as the visible approval and ownership anchor
    /// for existing-profile attachment.
    pub window_id: Option<u64>,
    pub session: String,
    /// Private transport lifecycle owner. A daemon-backed MCP proxy supplies
    /// this independently from the public capability session so either proxy
    /// disconnect or explicit `end_session` can reap a spawned browser.
    pub transport_session: Option<String>,
    pub authorization: Option<PrepareAuthorization>,
    /// Omitted for the legacy isolated-profile compatibility form.
    pub strategy: Option<PrepareStrategy>,
    pub profile: Option<PrepareProfile>,
    /// Allows launching a separate driver-owned isolated browser process.
    /// It never authorizes terminating or modifying the requested process.
    pub allow_launch: bool,
}

/// What a platform adapter actually did (or found) during prepare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrepareAction {
    /// An owned endpoint already existed; nothing was changed.
    AlreadyPrepared,
    /// The adapter enabled an endpoint on the running process.
    EnabledEndpoint,
    /// The adapter (re)launched the browser with an endpoint.
    RelaunchedBrowser,
    /// The driver launched a separate isolated browser process.
    LaunchedIsolatedBrowser,
    /// Attached to a user-owned, already-running profile under a live grant.
    AttachedExistingProfile,
    /// Nothing was done — see `message` on the outcome.
    NoOp,
}

/// Result of an explicit prepare. `endpoint` is present iff an owned
/// endpoint is now available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareOutcome {
    pub action: PrepareAction,
    pub endpoint: Option<OwnedEndpoint>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepared_pid: Option<i64>,
    #[serde(default)]
    pub side_effects: PrepareSideEffects,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<PrepareAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareAttachment {
    pub kind: PrepareAttachmentKind,
    pub browser: String,
    pub capabilities_invalidated: bool,
    pub next_action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrepareAttachmentKind {
    ExistingProfile,
}

#[derive(Debug, Clone)]
pub struct BrowserConsentRequest {
    pub pid: i64,
    pub window_id: u64,
    pub attempt: u8,
}

/// Exact, already-approved browser/window scope for enabling an existing
/// Chromium profile's own DevTools endpoint. Core consumes the approval and
/// validates this native window before invoking the platform adapter.
#[derive(Debug, Clone)]
pub struct ExistingProfileSetupRequest {
    pub pid: i64,
    pub window_id: u64,
    /// Product identity attested immediately before approval-bound setup.
    pub browser: BrowserProduct,
}

/// Declared user-visible effects of one bounded existing-profile setup.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExistingProfileSetupOutcome {
    pub opened_setup_page: bool,
    pub closed_setup_page: bool,
    pub enabled_remote_debugging: bool,
    pub focused_setup_address_field: bool,
    pub foregrounded_window: bool,
    pub injected_global_input: bool,
    /// Endpoint proven as part of an exact setup transition. This is needed
    /// for Chrome's in-browser toggle, whose listener does not expose the
    /// classic `/json/version` discovery surface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<OwnedEndpoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserConsentOutcome {
    /// One exact browser-owned consent action was semantically pressed.
    Accepted,
    /// The adapter proved no consent prompt was present in the approved scope.
    NotPresent,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrepareSideEffects {
    pub launched_browser: bool,
    pub restarted_browser: bool,
    pub created_profile: bool,
    pub reused_driver_profile: bool,
    pub copied_profile_data: bool,
    pub changed_preferences: bool,
    pub displayed_consent_prompt: bool,
    pub opened_setup_page: bool,
    pub closed_setup_page: bool,
    pub enabled_remote_debugging: bool,
    pub focused_setup_address_field: bool,
    pub foregrounded_window: bool,
    pub injected_global_input: bool,
}

/// OS-identity services a platform crate provides to the browser-tool
/// core. All methods are point-in-time queries; core re-invokes them
/// for revalidation before every mutation rather than caching.
///
/// Error convention: methods return `Err(BrowserRefusal)` for
/// conditions the calling agent should see as a structured refusal
/// (including infrastructure failures, which map naturally onto
/// `browser_route_unavailable`).
#[async_trait]
pub trait BrowserPlatform: Send + Sync {
    /// Explain why a trusted CDP Input route cannot preserve background
    /// posture for a standalone browser on this platform. Embedded Chromium
    /// routes are independently proven and do not consult this capability.
    fn standalone_trusted_input_background_limitation(&self) -> Option<&'static str> {
        None
    }

    /// Classify `pid`: is it a browser, which engine family, can it do
    /// CDP at all. Must not have side effects.
    async fn classify_browser(&self, pid: i64) -> Result<BrowserClassification, BrowserRefusal>;

    /// Resolve native metadata (title, normalized bounds, ownership
    /// proof) for one window of `pid`. Must fail — not guess — when the
    /// window does not exist or is not attributable to `pid`.
    async fn native_window(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<NativeWindowInfo, BrowserRefusal>;

    /// Prove whether `window_id` is the only native top-level window owned by
    /// `pid`. `Some(true)` is an exact platform-attested cardinality proof;
    /// `Some(false)` means another window exists; `None` means this window
    /// system cannot prove cardinality. Used for embedded Chromium endpoints
    /// that omit Browser.getWindowForTarget and standalone browsers whose
    /// bounds were overridden by a tiling compositor.
    async fn is_only_exact_native_window(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<Option<bool>, BrowserRefusal>;

    /// Discover a loopback DevTools endpoint owned by `pid`, with an
    /// explicit ownership proof. `Ok(None)` means "no endpoint right
    /// now" (core maps that to `browser_requires_setup`); it must NOT
    /// silently start one — setup belongs to [`Self::prepare_endpoint`].
    async fn discover_owned_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal>;

    /// Discover an endpoint while handling an explicitly approved
    /// existing-profile request. The default is the ordinary side-effect-free
    /// discovery path. Platforms may additionally return a uniquely proven
    /// browser-level route that does not expose HTTP discovery, but must not
    /// open its WebSocket or interact with consent UI here.
    async fn discover_existing_profile_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        self.discover_owned_endpoint(pid).await
    }

    /// Re-prove an endpoint that was already claimed under an existing-profile
    /// grant. Unlike discovery, this receives the exact WebSocket URL captured
    /// during the approved setup and may therefore prove that specific route
    /// without treating an arbitrary PID-owned listener as DevTools.
    async fn reprove_existing_profile_endpoint(
        &self,
        pid: i64,
        _expected_ws_url: &str,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        self.discover_existing_profile_endpoint(pid).await
    }

    /// Enable an existing Chromium profile's browser-owned DevTools endpoint
    /// after core has consumed exact interactive approval. Implementations may
    /// act only within `pid` and `window_id`, must fail on ambiguous UI, and
    /// must not restart the browser or modify profile files.
    async fn setup_existing_profile_endpoint(
        &self,
        _request: ExistingProfileSetupRequest,
    ) -> Result<ExistingProfileSetupOutcome, BrowserRefusal> {
        Err(BrowserRefusal::new(
            super::refusal::BrowserRefusalCode::BrowserRequiresSetup,
            "automatic existing-profile endpoint setup is not proven on this platform",
        ))
    }

    /// Commit a setup transition after core has protocol-proven and claimed
    /// the correlated browser endpoint. Implementations use this boundary to
    /// close only the exact temporary setup tab they opened.
    async fn commit_existing_profile_setup(
        &self,
        _request: ExistingProfileSetupRequest,
    ) -> Result<bool, BrowserRefusal> {
        Ok(false)
    }

    /// Roll back a setup transition that core could not safely claim. The
    /// adapter must preserve `error`, adding exact cleanup evidence where
    /// useful, and must never act on an unproven current tab or control.
    async fn abort_existing_profile_setup(
        &self,
        _request: ExistingProfileSetupRequest,
        error: BrowserRefusal,
    ) -> BrowserRefusal {
        error
    }

    /// Handle one pending browser-owned connection prompt. Implementations
    /// must use exact native semantics and may perform at most one declared
    /// consent action for this request. Generic dialog automation is forbidden.
    async fn handle_existing_profile_consent(
        &self,
        _request: BrowserConsentRequest,
    ) -> Result<BrowserConsentOutcome, BrowserRefusal> {
        Err(BrowserRefusal::new(
            super::refusal::BrowserRefusalCode::BrowserRouteUnavailable,
            "prompt-assisted existing-profile attachment is not proven on this platform",
        ))
    }

    /// Current identity fingerprint for `pid`. Used to detect pid reuse
    /// between binding and mutation.
    async fn process_fingerprint(&self, pid: i64) -> Result<ProcessFingerprint, BrowserRefusal>;

    /// Explicitly prepare an owned endpoint for `pid`. Only ever called
    /// from the `browser_prepare` tool. Adapters gate disruptive or
    /// consent-requiring paths on the request's explicit fields and
    /// refuse with `browser_consent_required` otherwise.
    async fn prepare_endpoint(
        &self,
        request: PrepareRequest,
    ) -> Result<PrepareOutcome, BrowserRefusal>;
}
