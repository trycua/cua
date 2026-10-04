//! The cua SDK: the only `#[uniffi::export]` crate of `libs/cua`.
//!
//! One [`Cua`] object, in one of two topologies (RFC 2549):
//!
//! - [`Cua::embedded`] runs the SDK runtime ([`cua_daemon::Runtime`]) in
//!   this process;
//! - [`Cua::connect`] talks to a running `cua daemon` over its Unix socket
//!   (`~/.cua/cua.sock`) or loopback + token. Sandbox handles, spacesd
//!   connections and credentials then live in the daemon, shared across
//!   processes.
//!
//! Both expose the same objects, and both pass the same test suite:
//!
//! | Object | Module |
//! |---|---|
//! | [`Sandboxes`] / [`Sandbox`] / [`Service`] / [`PortForward`] | daemon-agnostic sandboxes (Fleet, local, direct) |
//! | [`SpacesdClient`] / [`SpacesdProcess`] / [`MediaSession`] | cua-spacesd (`cua.env.v1`), including a JSON escape hatch |
//! | [`Fleet`] | pools, templates, claims, images (cyclops-sdk) |
//! | [`Local`] | local runtimes and images (cua-vmm, cua-image) |
//! | [`Spaces`] / [`Space`] / [`SpaceStreamSession`] / [`SpacePresence`] | Spaces: registry, Fleet/local/direct Spaces, exec, files, services, streams, presence, teleport, hotspot, agents |
//! | `Teleport` (feature `teleport`) | move a desktop app session from this machine into a sandbox (cua-teleport) |
//! | [`Auth`] / [`LoginAttempt`] | sign in to Cua (browser PKCE or device code) and the shared credential store (cua-auth) |
//! | [`AgentSetup`] | AI coding agents on this machine: cua skills and the cua MCP server (cua-agent-setup) |
//!
//! Conventions: one flat error enum
//! ([`CuaError`]); typed ids cross as strings; async exports run on a Tokio
//! runtime owned by this crate (callers never provide one); foreign
//! callbacks ([`FrameSink`], [`AudioSink`]) must not block.

mod json;
mod types;
pub use json::{JSON_STREAM_METHODS, JSON_UNARY_METHODS, TYPED_ONLY_METHODS};
pub use types::*;

// The browser API is `web`: always on wasm32, and on the host with the
// `web` feature so UBRN can read the same export metadata it generates the
// browser bindings from.
#[cfg(not(any(target_arch = "wasm32", feature = "web")))]
mod native;
#[cfg(not(any(target_arch = "wasm32", feature = "web")))]
pub use native::*;

/// For crates that export more over this SDK's runtime and telemetry (the
/// Cua Spaces app export, `cua-spaces-ffi`). Not part of the binding API.
#[doc(hidden)]
#[cfg(not(any(target_arch = "wasm32", feature = "web")))]
pub mod support {
    use std::future::Future;
    use std::time::Instant;

    /// Runs `fut` on the SDK runtime and awaits it from any executor.
    pub async fn run<T, F>(fut: F) -> crate::Result<T>
    where
        T: Send + 'static,
        F: Future<Output = crate::Result<T>> + Send + 'static,
    {
        crate::native::run(fut).await
    }

    /// Spawns `fut` on the SDK runtime.
    pub fn spawn<F>(fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        crate::native::runtime().spawn(fut);
    }

    /// Records a teleport made through this SDK (telemetry).
    pub fn record_teleport<T>(
        app: &str,
        capability: &str,
        move_kind: &str,
        started: Instant,
        r: &crate::Result<T>,
        items: u64,
    ) {
        crate::native::telemetry::teleport(app, capability, move_kind, started, r, items)
    }
}

#[cfg(any(target_arch = "wasm32", feature = "web"))]
mod web;
#[cfg(any(target_arch = "wasm32", feature = "web"))]
pub use web::*;

uniffi::setup_scaffolding!("cua_sdk");

/// Every error the SDK raises. Flat across the FFI: each language sees one
/// exception/error type with a case per variant and the message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CuaError {
    /// Bad input.
    ///
    /// Fix: Check the value against the reference; the message names the
    /// argument and what it accepts.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// A location, kind or runtime that does not exist, or a combination of
    /// them (or with the image) that does not. The message ends with the
    /// valid values for that context.
    ///
    /// Fix: Use one of the values the message lists, or leave `kind` and
    /// `runtime` unset (`auto`). `cua config list` shows the defaults in
    /// effect.
    #[error("invalid placement: {0}")]
    InvalidPlacement(String),
    /// No such sandbox, process, path, forward or session.
    ///
    /// Fix: Check the ref, path or id. `Sandboxes.list` (`cua sb ls`) lists the
    /// sandboxes this client can see.
    #[error("not found: {0}")]
    NotFound(String),
    /// The provider is not configured (for example no Fleet credentials).
    ///
    /// Fix: Set `FLEETS_TOKEN`, or `CUA_CLIENT_ID` and `CUA_CLIENT_SECRET`, or
    /// run `cua auth login` and create the client with `fleet_from_session`.
    #[error("provider not configured: {0}")]
    ProviderNotConfigured(String),
    /// Not supported by this provider, guest or build.
    ///
    /// Fix: Use a provider, image or runtime that supports the call; the
    /// message names what is missing.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// No cua-spacesd answered in the sandbox or at the URL.
    ///
    /// Fix: Use an image that runs cua-spacesd (the canonical Linux image does),
    /// or reach the sandbox through the services it declares.
    #[error("cua-spacesd is not available: {0}")]
    SpacesdNotAvailable(String),
    /// A deadline elapsed.
    ///
    /// Fix: Raise the timeout (for example `ready_timeout_ms`), and check that
    /// the sandbox or service actually starts.
    #[error("timed out: {0}")]
    Timeout(String),
    /// Fleet API failure.
    ///
    /// Fix: Read the status in the message. Retry a transient failure; check
    /// the credentials and the account's capacity otherwise.
    #[error("fleet: {0}")]
    Fleet(String),
    /// Fleet's admission refused the request: a sandbox or pool size over
    /// this account's limits, or a template its policy does not admit. The
    /// message is Fleet's own and names the limit.
    ///
    /// Fix: Pick a size within the limit the message names (most accounts
    /// run 1-8 vCPUs and 1-32 GiB per sandbox), or contact Cua support to
    /// raise the account's limits.
    #[error("fleet admission denied: {0}")]
    FleetAdmissionDenied(String),
    /// The account is out of Cua Cloud credit: no credit left, and no card
    /// or plan. New cloud sandboxes and Spaces are refused; running ones
    /// keep running, and local ones are never affected. The message ends
    /// with the website billing page's URL.
    ///
    /// Fix: Add credit on that page (a plan, or a card for pay as you go),
    /// or run the sandbox locally.
    #[error("{0}")]
    CloudCreditExhausted(String),
    /// Local runtime failure.
    ///
    /// Fix: Run `cua runtime doctor` (`Local.doctor`) and follow the steps it
    /// prints.
    #[error("local runtime: {0}")]
    Runtime(String),
    /// spacesd call failure.
    ///
    /// Fix: Read the status in the message; `SpacesdClient.capabilities` tells
    /// whether this spacesd supports the call.
    #[error("env: {0}")]
    Env(String),
    /// HTTP failure talking to a sandbox service.
    ///
    /// Fix: Check that the service listens on its declared port and answers
    /// HTTP.
    #[error("http: {0}")]
    Http(String),
    /// Missing or wrong token or ticket.
    ///
    /// Fix: Pass the sandbox's env token, refresh the Fleet credentials, or run
    /// `cua auth login` again.
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),
    /// The guest OS denied permission.
    ///
    /// Fix: Run the call as a guest user allowed to do it, or change the
    /// permissions of the path.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Could not reach the daemon or the endpoint.
    ///
    /// Fix: Check the address. For a daemon client, start the daemon with `cua
    /// daemon start`.
    #[error("transport: {0}")]
    Transport(String),
    /// No `cua daemon` is running: nothing listens at its socket or
    /// port, or its discovery file (`~/.cua/daemon.json`) was left by a
    /// daemon that exited. The message says so in plain words.
    ///
    /// Fix: Start Cua (the Spaces app starts the daemon), or run `cua daemon
    /// start`. `Cua.auto` uses the runtime in this process when no daemon
    /// runs.
    #[error("{0}")]
    DaemonNotRunning(String),
    /// The object was closed.
    ///
    /// Fix: Open a new handle or session; a closed one stays closed.
    #[error("closed: {0}")]
    Closed(String),
    /// The Space's spacesd reports a feature this call needs as
    /// unsupported (the message names it).
    ///
    /// Fix: Use a Space whose spacesd supports the feature (`Space.supports`),
    /// or update cua-spacesd on it.
    #[error("capability missing: {0}")]
    CapabilityMissing(String),
    /// A host-side prerequisite (app-session providers, an operator
    /// display, a local runtime) is not available.
    ///
    /// Fix: Install or start the prerequisite the message names on this
    /// machine.
    #[error("host capability missing: {0}")]
    HostCapabilityMissing(String),
    /// The teleport consent gate refused (or the approver declined).
    ///
    /// Fix: Approve the manifest in the consent callback, or teleport a smaller
    /// scope.
    #[error("teleport refused: {0}")]
    TeleportRefused(String),
    /// A named cloud pool's template differs from the requested sandbox
    /// fields; the message holds the diff (pass `apply` to update it).
    ///
    /// Fix: Pass the pool's own fields, or pass `apply` to update its template.
    #[error("pool spec mismatch: {0}")]
    PoolSpecMismatch(String),
    /// A claim bound but its per-claim secrets (the env token) never
    /// reached the sandbox within the bounded wait; the claim was released.
    ///
    /// Fix: Retry. If it keeps failing, check the pool's claim-secrets setup
    /// and the Fleet status.
    #[error("claim secrets not delivered: {0}")]
    ClaimSecretsNotDelivered(String),
    /// Bug or I/O failure.
    ///
    /// Fix: Report it with the message at https://github.com/trycua/cua/issues.
    #[error("internal: {0}")]
    Internal(String),
    /// A bare sandbox name matches sandboxes in more than one location. The
    /// message ends with the qualified candidates (`use one of: local:box,
    /// cloud:box`); `ambiguous_sandbox_candidates` extracts them.
    ///
    /// Fix: Use one of the qualified refs the message lists (`local:<name>`,
    /// `cloud:<name>`).
    #[error("ambiguous sandbox name: {0}")]
    AmbiguousSandbox(String),
    /// A pull, build or VM create would leave less than the configured
    /// minimum free disk space (`CUA_DISK_MIN_FREE`, default 5 GiB);
    /// nothing was written.
    ///
    /// Fix: Free space with `cua cache prune`, or lower `CUA_DISK_MIN_FREE`.
    #[error("insufficient disk: {0}")]
    InsufficientDisk(String),
    /// A catalog image (an `Image.*` constructor, a tier or `cua sb create
    /// <word>`) that CI has not published yet. The message names the
    /// reference.
    ///
    /// Fix: Pass the reference explicitly (`Image.from_registry("<ref>")`,
    /// `cua sb create <ref>`) to use it anyway, or pick a published image
    /// (`cua images ls`).
    #[error("image not published: {0}")]
    ImageNotPublished(String),
    /// The create was cancelled (`Spaces.cancel_create`, Ctrl-C in `cua`,
    /// or its caller went away). What it made is gone; the message says
    /// what was removed and what stays (finished image downloads stay
    /// cached, so the next create resumes).
    ///
    /// Fix: Nothing to clean up. Create it again when you want it.
    #[error("cancelled: {0}")]
    Cancelled(String),
    /// Your own cloud account (AWS, Google Cloud, Modal) refused or failed
    /// a call Cua made for a Space there: a missing permission, a quota, a
    /// region without the machine type. The message names the cloud's own
    /// error.
    ///
    /// Fix: Fix what the message names in that account (`cua cloud test
    /// <provider>` checks it without creating anything), or connect another
    /// region or project.
    #[error("your cloud: {0}")]
    Cloud(String),
}

/// Result alias.
pub type Result<T, E = CuaError> = std::result::Result<T, E>;

/// The errors reference. Each variant has an entry there, anchored by its
/// name in lower case (`#invalidargument`).
pub const ERRORS_DOC_URL: &str = "https://cua.ai/docs/cua-sdk/reference/errors";

impl CuaError {
    /// The variant's name, as every binding spells it (`InvalidArgument`).
    pub fn variant(&self) -> &'static str {
        match self {
            CuaError::InvalidArgument(_) => "InvalidArgument",
            CuaError::InvalidPlacement(_) => "InvalidPlacement",
            CuaError::NotFound(_) => "NotFound",
            CuaError::ProviderNotConfigured(_) => "ProviderNotConfigured",
            CuaError::Unsupported(_) => "Unsupported",
            CuaError::SpacesdNotAvailable(_) => "SpacesdNotAvailable",
            CuaError::Timeout(_) => "Timeout",
            CuaError::Fleet(_) => "Fleet",
            CuaError::FleetAdmissionDenied(_) => "FleetAdmissionDenied",
            CuaError::CloudCreditExhausted(_) => "CloudCreditExhausted",
            CuaError::Runtime(_) => "Runtime",
            CuaError::Env(_) => "Env",
            CuaError::Http(_) => "Http",
            CuaError::Unauthenticated(_) => "Unauthenticated",
            CuaError::PermissionDenied(_) => "PermissionDenied",
            CuaError::Transport(_) => "Transport",
            CuaError::DaemonNotRunning(_) => "DaemonNotRunning",
            CuaError::Closed(_) => "Closed",
            CuaError::CapabilityMissing(_) => "CapabilityMissing",
            CuaError::HostCapabilityMissing(_) => "HostCapabilityMissing",
            CuaError::TeleportRefused(_) => "TeleportRefused",
            CuaError::PoolSpecMismatch(_) => "PoolSpecMismatch",
            CuaError::ClaimSecretsNotDelivered(_) => "ClaimSecretsNotDelivered",
            CuaError::Internal(_) => "Internal",
            CuaError::AmbiguousSandbox(_) => "AmbiguousSandbox",
            CuaError::InsufficientDisk(_) => "InsufficientDisk",
            CuaError::ImageNotPublished(_) => "ImageNotPublished",
            CuaError::Cancelled(_) => "Cancelled",
            CuaError::Cloud(_) => "Cloud",
        }
    }

    /// A link to this error's entry on the errors reference: its cause and
    /// fix. Bindings expose it as `doc_url` (Python), `docUrl` (TypeScript,
    /// Swift, Kotlin).
    pub fn doc_url(&self) -> String {
        error_doc_url(self.variant().to_string())
    }
}

/// A link to the errors-reference entry of a [`CuaError`] variant, by name
/// (`InvalidArgument`, case-insensitive). An unknown name links to the
/// page itself. The bindings' `doc_url` / `docUrl` on `CuaError` use it.
#[uniffi::export]
pub fn error_doc_url(variant: String) -> String {
    let anchor: String = variant
        .trim()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if anchor.is_empty() {
        ERRORS_DOC_URL.to_string()
    } else {
        format!("{ERRORS_DOC_URL}#{anchor}")
    }
}

impl From<serde_json::Error> for CuaError {
    fn from(e: serde_json::Error) -> Self {
        CuaError::InvalidArgument(format!("json: {e}"))
    }
}

/// The released cua SDK version (libs/cua/VERSION, or the version
/// cd-cua-sdk.yml publishes), shared by the `cua` CLI.
pub const VERSION: &str = env!("CUA_SDK_VERSION");

/// Version of the SDK library.
#[uniffi::export]
pub fn cua_sdk_version() -> String {
    VERSION.to_string()
}

#[cfg(test)]
mod doc_url_tests {
    use super::*;

    #[test]
    fn every_variant_links_to_its_errors_entry() {
        let e = CuaError::InvalidArgument("x".into());
        assert_eq!(e.variant(), "InvalidArgument");
        assert_eq!(
            e.doc_url(),
            "https://cua.ai/docs/cua-sdk/reference/errors#invalidargument"
        );
        assert_eq!(
            CuaError::SpacesdNotAvailable(String::new()).doc_url(),
            format!("{ERRORS_DOC_URL}#spacesdnotavailable")
        );
        assert_eq!(error_doc_url("  ".into()), ERRORS_DOC_URL);
        assert_eq!(
            error_doc_url("InsufficientDisk".into()),
            format!("{ERRORS_DOC_URL}#insufficientdisk")
        );
    }
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn sdk_version_is_the_released_version() {
        let expected = option_env!("CUA_SDK_RELEASE_VERSION")
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(include_str!("../../../VERSION").trim());
        assert_eq!(super::cua_sdk_version(), expected);
        assert_ne!(super::cua_sdk_version(), "");
    }
}
