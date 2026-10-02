// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teleport SEND: move a desktop app session from this machine into a
//! sandbox.
//!
//! This is the client half of teleport. It runs on the user's machine, next
//! to the app whose session moves, and never inside a sandbox:
//!
//! - [`ExportProvider`] is the per-application extension point: it describes
//!   what a transfer would move ([`TransferManifest`], for consent UIs) and
//!   captures the selected items into a [`bundle::SessionBundle`].
//! - [`ExportRegistry`] resolves a provider for an [`AppRef`].
//! - [`Teleporter`] (and the [`send`] shortcut) runs the whole flow against a
//!   `cua.env.v1` endpoint: manifest → selection → consent ([`Approval`]) →
//!   OS authorization for sensitive items ([`biometric`]) → capture →
//!   chunked `TeleportService.ImportSession` upload.
//!
//! The receiving half (verify, materialize, Keychain install, relaunch) is
//! `cua-spacesd-teleport` inside cua-spacesd. The two share only the
//! effect-free contract in [`cua_teleport_bundle`], re-exported here.
//!
//! Every host effect (processes, AppleScript, Keychain reads, DevTools,
//! Touch ID, `$HOME`) goes through [`HostEffects`]; [`RealHost`] refuses them
//! under `cfg(test)` or `CUA_ENV_TEST_SANDBOX=1`, and tests inject a
//! [`FakeHost`] with a temporary home.

pub mod biometric;
pub mod browser_cookies;
pub mod cookies;
pub mod favicons;
pub mod host;
pub mod keychain;
pub mod passwords;
pub mod providers;
mod registry;
pub mod safe_storage;
mod send;
pub mod ux;

pub use biometric::AuthError;
pub use cua_teleport_bundle::{
    AppRef, BUNDLE_VERSION, InstallProbe, LaunchSpec, ManifestItem, Platform, Result,
    TeleportError, TransferManifest, TransferScope, WindowRef, WindowRestore, bundle, layout,
};
pub use host::{
    EffectKind, FakeHost, HostCommand, HostEffects, HostOutput, RealHost, default_host,
    is_installed,
};
pub use registry::{ExportRegistry, ProviderInfo};
pub use send::{
    Approval, ApprovalRequest, AutoApprove, BundleSource, Error, ExportedBundle, Progress,
    RELAY_SEALING_ENFORCED, RELAY_UNSEALED_WARNING, Resolved, Selection, SendOptions, SendOutcome,
    Teleporter, resolve_selection, send, upload_bundle,
};

use std::collections::HashSet;
use std::io::Write;

/// One application's session-capture implementation (sender side).
///
/// Providers are transport-neutral: `export` writes a
/// [`bundle::SessionBundle`] tar stream to any writer. A provider may support
/// a subset of [`TransferScope`] values and must return
/// [`TeleportError::UnsupportedScope`] for the rest.
pub trait ExportProvider: Send + Sync {
    /// Stable provider identifier recorded in the bundle header. The receiver
    /// imports with the provider of the same id.
    fn id(&self) -> &str;

    /// Human-readable provider name for consent UIs.
    fn display_name(&self) -> &str;

    /// The host this provider acts on. Every process, Keychain, AppleScript,
    /// DevTools, authorization and `$HOME` effect goes through it, so tests
    /// can inject a [`FakeHost`] and never touch the real machine.
    fn host(&self) -> &dyn HostEffects;

    /// Whether this provider can export on the given platform.
    fn platform_supported(&self, platform: Platform) -> bool;

    /// A host-side check proving this provider's application is installed on
    /// the source machine, for consent UIs. `None` means "always offer".
    /// Consent UIs read this via `cua teleport providers`, so a newly added
    /// provider appears without any UI-side change.
    fn install_probe(&self) -> Option<InstallProbe> {
        None
    }

    /// Whether this provider handles the given application.
    fn matches(&self, app: &AppRef) -> bool;

    /// The application identifiers this provider matches — bundle ids and app
    /// names, e.g. `["com.unity3d.unityhub", "unityhub", "Unity Hub"]`. Consent
    /// UIs read these (via `cua teleport providers`) to decide whether an app
    /// the user dragged or picked is teleportable.
    fn app_ids(&self) -> &[&str] {
        &[]
    }

    /// Describe what a transfer would move, for consent UIs.
    fn manifest(
        &self,
        app: &AppRef,
        window: Option<&WindowRef>,
        scope: TransferScope,
    ) -> Result<TransferManifest>;

    /// Raw capture: read the selected items and write them into a
    /// [`bundle::SessionBundle`] tar stream. **Internal seam — implement this,
    /// but do not call it directly to export.**
    ///
    /// This is the single choke point where selected items are read and packed.
    /// It is intentionally *ungated*: the biometric authorization for sensitive
    /// items is applied once, up front, by [`Self::export_selected`] (which every
    /// public export path flows through).
    ///
    /// `include == None` captures everything the `scope` implies. When a set is
    /// given, only entries whose `rel_path` (as reported by [`Self::manifest`])
    /// is present are written, so a consent UI can transfer exactly the items the
    /// user checked.
    fn capture_selected(
        &self,
        app: &AppRef,
        scope: TransferScope,
        include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> Result<()>;

    /// Capture the app session as a [`bundle::SessionBundle`] tar stream.
    ///
    /// Gated: equivalent to [`Self::export_selected`] with `include == None`.
    /// Do not override.
    fn export(&self, app: &AppRef, scope: TransferScope, out: &mut dyn Write) -> Result<()> {
        self.export_selected(app, scope, None, out)
    }

    /// Capture the selected items as a [`bundle::SessionBundle`] tar stream,
    /// obtaining interactive OS user authorization first when the selection
    /// includes any `sensitive: true` item.
    ///
    /// This is the enforced export choke point: before *any* sensitive item is
    /// read or packed it asks the host for authorization (Touch ID / device
    /// passcode on macOS; fail-closed elsewhere). If authorization fails the
    /// export is aborted and nothing is written; a selection with no sensitive
    /// item is never prompted. The check runs once per export, not per item.
    ///
    /// Provided and final in spirit — providers implement
    /// [`Self::capture_selected`] instead of overriding this so the gate cannot
    /// be bypassed.
    fn export_selected(
        &self,
        app: &AppRef,
        scope: TransferScope,
        include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> Result<()> {
        // Determine sensitivity from this provider's own manifest, then gate
        // BEFORE reading or packing anything so a denied prompt never touches the
        // credential store or writes a partial bundle.
        let manifest = self.manifest(app, None, scope)?;
        if biometric::selection_is_sensitive(&manifest.items, include) {
            let reason = format!(
                "Authorize teleporting your {} session off this Mac",
                manifest.app_display_name
            );
            self.host()
                .authorize_sensitive_export(&reason)
                .map_err(|error| {
                    TeleportError::Provider(format!(
                        "sensitive session export was not authorized: {error}"
                    ))
                })?;
        }
        self.capture_selected(app, scope, include, out)
    }
}
