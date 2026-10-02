// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Session teleport: moving a logged-in host app session into a Space.
//!
//! The sender side uses the SDK's `cua-teleport` providers, so every host
//! effect (reading a profile, the Keychain, the Touch ID prompt for
//! sensitive items) goes through the provider's `HostEffects`: the real host
//! in the daemon, a `FakeHost` over a temp `$HOME` in tests. The receiver is
//! the Space's `TeleportService.ImportSession` (offset-addressed chunks,
//! SHA-256 verified on commit). No rcdp CLI, no `:8700` teleport server, no
//! hard-coded teleport token.
//!
//! Two facts shape the API, and they are facts about real data (see the
//! Claude Code manifest in the tests below):
//!
//! **A manifest is a consent surface, not a file list.** Items carry
//! `is_sensitive` and `is_checked_by_default`; the largest Claude Code item
//! (conversation transcripts, ~900 MB) is sensitive and *unchecked*.
//!
//! **Consent is a type, not a defaulted parameter.** [`Approval`] has no
//! public constructor and no public fields; only
//! [`TeleportManifest::approving`] and
//! [`TeleportManifest::approving_default`] mint one, both refuse a path the
//! manifest does not offer, and both refuse a sensitive path that was not
//! acknowledged. An empty-but-present selection is refused as ambiguous; no
//! selection means the manifest's default-checked set, never "everything".
//!
//! Ported from `cua-spaces-core::teleport` (libs/spaces-sdk), which gated the
//! same decisions for the Python-backed SDKs.
//!
//! **This module is the only place cua-spaces names the sender-side provider
//! crate.** Everything else (the MCP tools, tests, bindings) goes through
//! [`AppSessions`] and the [`providers`] re-exports. The sender is the SDK's
//! `cua-teleport` crate; nothing here depends on the import side.

use cua_spaces::Space;
use cua_spaces::error::{Error, Result};
use cua_spaces::mcp::ToolOutcome;
use cua_spacesd_client::pb;
use cua_teleport::{
    AppRef, ExportProvider, ExportRegistry, HostEffects, Platform, TransferManifest, TransferScope,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;

/// Sender-side provider types, re-exported so no caller names the provider
/// crate (tests build a [`providers::FakeHost`]-backed [`AppSessions`] from
/// these).
pub mod providers {
    pub use cua_teleport::providers::firefox::FirefoxProvider;
    pub use cua_teleport::{ExportProvider, ExportRegistry, FakeHost, HostEffects, RealHost};
}

/// How much of an app's session to consider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeleportScope {
    /// The whole profile: credentials, cookies, preferences, local state.
    Full,
    /// Open tabs / documents only.
    Tabs,
}

impl TeleportScope {
    /// Parses `full` / `tabs` (and the provider spellings).
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("full").trim() {
            "" | "full" | "full_profile" | "full-profile" | "profile" => Ok(TeleportScope::Full),
            "tabs" | "tabs_only" | "tabs-only" | "session" => Ok(TeleportScope::Tabs),
            other => Err(Error::invalid(format!(
                "unknown teleport scope {other:?}; expected \"full\" or \"tabs\""
            ))),
        }
    }

    /// Wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            TeleportScope::Full => "full",
            TeleportScope::Tabs => "tabs",
        }
    }

    fn transfer(self) -> TransferScope {
        match self {
            TeleportScope::Full => TransferScope::FullProfile,
            TeleportScope::Tabs => TransferScope::TabsOnly,
        }
    }

    fn spacesd(self) -> pb::TeleportScope {
        match self {
            TeleportScope::Full => pb::TeleportScope::Profile,
            TeleportScope::Tabs => pb::TeleportScope::Session,
        }
    }
}

/// One transferable item.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TeleportItem {
    /// The bundle-relative path: the exact string an approval names.
    pub relative_path: String,
    /// Label for a consent UI.
    pub label: String,
    /// Estimated bytes (0 when unknown).
    pub estimated_bytes: u64,
    /// Credentials, cookies, tokens, transcripts.
    pub is_sensitive: bool,
    /// In the provider's default selection.
    pub is_checked_by_default: bool,
    /// Count of things it holds, when cheap to know.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Noun for `count`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count_noun: Option<String>,
}

/// Exactly what would leave this machine, before anything does.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TeleportManifest {
    /// Provider id (`firefox`, `chrome`, `claude-code`, ...).
    pub app: String,
    /// Display name.
    pub display_name: String,
    /// Scope.
    pub scope: TeleportScope,
    /// Items.
    pub items: Vec<TeleportItem>,
    /// The provider's own total.
    pub total_estimated_bytes: u64,
    /// The provider's own caveats, verbatim.
    pub notes: Vec<String>,
}

impl TeleportManifest {
    /// Converts a provider manifest.
    pub fn from_transfer(app: &str, scope: TeleportScope, m: TransferManifest) -> Self {
        TeleportManifest {
            app: if m.provider_id.is_empty() {
                app.into()
            } else {
                m.provider_id
            },
            display_name: m.app_display_name,
            scope,
            items: m
                .items
                .into_iter()
                .filter(|i| !i.rel_path.is_empty())
                .map(|i| TeleportItem {
                    relative_path: i.rel_path,
                    label: i.label,
                    estimated_bytes: i.est_bytes,
                    is_sensitive: i.sensitive,
                    is_checked_by_default: i.default_checked,
                    count: i.count,
                    count_noun: i.count_noun,
                })
                .collect(),
            total_estimated_bytes: m.total_est_bytes,
            notes: m.notes,
        }
    }

    /// Sensitive items.
    pub fn sensitive_items(&self) -> Vec<TeleportItem> {
        self.items
            .iter()
            .filter(|i| i.is_sensitive)
            .cloned()
            .collect()
    }

    /// The provider's default selection. **Not everything.**
    pub fn default_selection(&self) -> Vec<TeleportItem> {
        self.items
            .iter()
            .filter(|i| i.is_checked_by_default)
            .cloned()
            .collect()
    }

    /// Just the logged-in session: the smallest teleport that still leaves an
    /// in-Space agent authenticated.
    ///
    /// For a browser (Chrome, Firefox), the session is specifically its
    /// `SignIns` opt-in item (its cookies) -- never checked by default since
    /// `OPT_IN_ITEMS` made "Keep me signed in" an explicit choice, so it
    /// cannot come from [`Self::default_selection`] at all. A single-app
    /// provider (Slack, Claude Code, ...) has no separate sign-in opt-in:
    /// its default-checked, sensitive item already *is* the session move,
    /// so that case falls through to the previous rule unchanged.
    pub fn login_only_selection(&self) -> Vec<TeleportItem> {
        let sign_ins: Vec<TeleportItem> = self
            .items
            .iter()
            .filter(|i| {
                cua_teleport::ux::SensitiveGroup::of_item(&self.app, &i.relative_path)
                    == Some(cua_teleport::ux::SensitiveGroup::SignIns)
            })
            .cloned()
            .collect();
        if !sign_ins.is_empty() {
            return sign_ins;
        }
        let checked = self.default_selection();
        let sensitive: Vec<TeleportItem> =
            checked.iter().filter(|i| i.is_sensitive).cloned().collect();
        if sensitive.is_empty() {
            checked
        } else {
            sensitive
        }
    }

    /// Mints an approval for an explicit selection. Refuses a path this
    /// manifest does not offer, an empty selection, and any sensitive path
    /// without `acknowledging_sensitive_items`.
    pub fn approving(
        &self,
        space_id: &str,
        relative_paths: &[String],
        acknowledging_sensitive_items: bool,
    ) -> Result<Approval> {
        if relative_paths.is_empty() {
            return Err(Error::TeleportRefused(
                "an empty selection is ambiguous: pass no selection at all to use the \
                 provider's default set, which is never everything"
                    .into(),
            ));
        }
        self.mint(
            space_id,
            relative_paths,
            acknowledging_sensitive_items,
            false,
        )
    }

    /// Approves the provider's default-checked set.
    pub fn approving_default(
        &self,
        space_id: &str,
        acknowledging_sensitive_items: bool,
    ) -> Result<Approval> {
        let paths: Vec<String> = self
            .default_selection()
            .into_iter()
            .map(|i| i.relative_path)
            .collect();
        if paths.is_empty() {
            return Err(Error::TeleportRefused(format!(
                "the {} manifest marks nothing as default; name the items to send",
                self.app
            )));
        }
        self.mint(space_id, &paths, acknowledging_sensitive_items, true)
    }

    fn mint(
        &self,
        space_id: &str,
        relative_paths: &[String],
        acknowledging_sensitive_items: bool,
        default: bool,
    ) -> Result<Approval> {
        let unknown: Vec<&str> = relative_paths
            .iter()
            .filter(|p| !self.items.iter().any(|i| &i.relative_path == *p))
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            return Err(Error::TeleportRefused(format!(
                "approved entries that are not in this manifest: {}",
                unknown.join(", ")
            )));
        }
        let selected: Vec<&TeleportItem> = relative_paths
            .iter()
            .filter_map(|p| self.items.iter().find(|i| &i.relative_path == p))
            .collect();
        let sensitive: Vec<&str> = selected
            .iter()
            .filter(|i| i.is_sensitive)
            .map(|i| i.relative_path.as_str())
            .collect();
        if !sensitive.is_empty() && !acknowledging_sensitive_items {
            return Err(Error::TeleportRefused(format!(
                "{} approved entries are sensitive ({}); acknowledge them explicitly to send them",
                sensitive.len(),
                sensitive.join(", ")
            )));
        }
        Ok(Approval {
            app: self.app.clone(),
            scope: self.scope,
            space: space_id.to_string(),
            paths: relative_paths.to_vec(),
            uses_default: default,
            approved_bytes: selected.iter().map(|i| i.estimated_bytes).sum(),
        })
    }
}

/// Proof a human agreed, and to what. Only a [`TeleportManifest`] mints one;
/// only [`Space::teleport`] accepts one. Fields are private on purpose: an
/// approval that can be edited after it is minted is not an approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    app: String,
    scope: TeleportScope,
    space: String,
    paths: Vec<String>,
    uses_default: bool,
    approved_bytes: u64,
}

impl Approval {
    /// App.
    pub fn app(&self) -> &str {
        &self.app
    }
    /// Scope.
    pub fn scope(&self) -> TeleportScope {
        self.scope
    }
    /// The Space it was approved for.
    pub fn space(&self) -> &str {
        &self.space
    }
    /// Bytes consented to.
    pub fn approved_bytes(&self) -> u64 {
        self.approved_bytes
    }
    /// The paths that will move (always explicit; the default set is resolved
    /// when the approval is minted).
    pub fn approved_paths(&self) -> &[String] {
        &self.paths
    }
    /// Whether the caller deferred to the provider's default set.
    pub fn uses_default(&self) -> bool {
        self.uses_default
    }
}

/// What actually moved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TeleportReceipt {
    /// App.
    pub app: String,
    /// Space.
    pub space: String,
    /// `import_session`.
    pub method: &'static str,
    /// Paths sent.
    pub transferred_paths: Vec<String>,
    /// Bundle bytes sent.
    pub bundle_bytes: u64,
    /// Bundle SHA-256, verified by the Space.
    pub bundle_sha256: String,
    /// Item groups the Space imported.
    pub imported: Vec<String>,
    /// Items the Space skipped, with reasons.
    pub skipped: Vec<String>,
    /// The app was launched after importing.
    pub launched: bool,
}

/// One teleport provider as offered in a UI ([`AppSessions::catalog`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ProviderInfo {
    /// Provider id (`firefox`, `claude-code`, ...).
    pub id: String,
    /// Human name.
    pub display_name: String,
    /// Bundle ids and app names the provider handles.
    pub app_ids: Vec<String>,
    /// Whether it can export on this host's platform.
    pub supported_here: bool,
    /// Whether the app looks installed on this host.
    pub installed: bool,
}

/// The host-side providers (teleport sender).
pub struct AppSessions {
    registry: ExportRegistry,
}

impl std::fmt::Debug for AppSessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppSessions")
            .field("providers", &self.providers())
            .finish()
    }
}

impl AppSessions {
    /// Every built-in provider on the real host (the daemon's choice).
    pub fn builtin() -> Self {
        Self::from_registry(ExportRegistry::with_builtin())
    }

    /// Every built-in provider acting on `host` (tests pass a `FakeHost`).
    pub fn with_host(host: Arc<dyn HostEffects>) -> Self {
        Self::from_registry(ExportRegistry::with_builtin_host(host))
    }

    /// An explicit registry (for example one provider with a fixed profile
    /// directory).
    pub fn from_registry(registry: ExportRegistry) -> Self {
        Self { registry }
    }

    /// The export registry (for the UX catalog, `cua_teleport::ux`).
    pub fn registry(&self) -> &ExportRegistry {
        &self.registry
    }

    /// Provider ids.
    pub fn providers(&self) -> Vec<String> {
        self.registry
            .providers()
            .iter()
            .map(|p| p.id().to_string())
            .collect()
    }

    /// Every provider with what an app UI needs to offer it: names, the app
    /// identifiers it claims (for resolving a dragged window or a bundle id),
    /// whether it exports on this host's platform, and whether the app looks
    /// installed (a filesystem / `PATH` probe; `true` when the provider has
    /// no probe). Reads nothing from the app's profile.
    pub fn catalog(&self) -> Vec<ProviderInfo> {
        self.catalog_with(cua_teleport::is_installed)
    }

    /// [`catalog`](Self::catalog) with the install probe given: `installed`
    /// answers whether a provider's [`cua_teleport::InstallProbe`] resolves.
    pub fn catalog_with(
        &self,
        installed: impl Fn(&cua_teleport::InstallProbe) -> bool,
    ) -> Vec<ProviderInfo> {
        let here = Platform::current();
        self.registry
            .providers()
            .iter()
            .map(|p| ProviderInfo {
                id: p.id().to_string(),
                display_name: p.display_name().to_string(),
                app_ids: p.app_ids().iter().map(|s| s.to_string()).collect(),
                supported_here: p.platform_supported(here),
                installed: p.install_probe().is_none_or(|probe| installed(&probe)),
            })
            .collect()
    }

    fn resolve(&self, app: &str) -> Result<(&dyn ExportProvider, AppRef)> {
        let provider = self
            .registry
            .find_by_id(app)
            .or_else(|| {
                self.registry.find_for_app(&AppRef {
                    app_id: app.into(),
                    display_name: app.into(),
                    platform: Platform::current(),
                })
            })
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "a teleport provider for {app:?} on this host (have: {})",
                    self.providers().join(", ")
                ))
            })?;
        let app_id = provider
            .app_ids()
            .first()
            .map(|s| s.to_string())
            .unwrap_or_else(|| app.to_string());
        let app_ref = AppRef {
            app_id,
            display_name: provider.display_name().into(),
            platform: Platform::current(),
        };
        Ok((provider, app_ref))
    }

    /// [`Self::resolve`] without the provider handle: a cheap local check
    /// that `app` has a teleport provider on this host at all, for callers
    /// (like [`Space::teleport`]) that do the actual capture elsewhere and
    /// only want a clear, immediate error for an unknown app.
    pub fn resolve_checked(&self, app: &str) -> Result<()> {
        self.resolve(app).map(|_| ())
    }

    /// The manifest for `app` on this host.
    pub fn manifest(&self, app: &str, scope: TeleportScope) -> Result<TeleportManifest> {
        let (provider, app_ref) = self.resolve(app)?;
        let m = provider
            .manifest(&app_ref, None, scope.transfer())
            .map_err(|e| Error::TeleportRefused(format!("{app}: {e}")))?;
        Ok(TeleportManifest::from_transfer(provider.id(), scope, m))
    }

    /// Writes the approved items as a session bundle (through the provider's
    /// gated `export_selected`, so its sensitive-item authorization runs).
    pub fn export(&self, approval: &Approval, out: &mut dyn Write) -> Result<()> {
        let (provider, app_ref) = self.resolve(&approval.app)?;
        let include: HashSet<String> = approval.paths.iter().cloned().collect();
        provider
            .export_selected(&app_ref, approval.scope.transfer(), Some(&include), out)
            .map_err(|e| Error::TeleportRefused(format!("{}: export failed: {e}", approval.app)))
    }
}

/// Options applied when the Space imports.
#[derive(Clone, Copy, Debug, Default)]
pub struct ImportOptions {
    /// Replace existing state instead of merging.
    pub replace_existing: bool,
    /// Close the app in the Space first.
    pub close_running_app: bool,
    /// Launch the app after importing.
    pub launch_after: bool,
    /// Keep the captured item sealed in the Cua Keyvault after delivery
    /// (`false` forgets it right after this one delivery). Only
    /// [`Space::teleport`]'s Keyvault-routed delivery honors this; it is
    /// meaningless for a direct, non-Keyvault upload.
    pub save_to_keyvault: bool,
    /// The caller already resolved and acknowledged
    /// [`cua_teleport::ux::TeleportPlan::relay_unsealed`] (an
    /// [`cua_teleport::ux::ApprovedPlan`] can only exist when that was
    /// either false or acknowledged); threaded straight through as
    /// [`cua_teleport::SendOptions::relay_plaintext_ack`] (S1).
    pub relay_plaintext_ack: bool,
}

/// What the review chose beyond the approved paths: which sites' cookies to
/// send, and whether to send saved Keyvault items instead of reading the
/// live app.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TeleportSelection {
    /// The registrable domains whose cookies to send (`None`: every cookie in
    /// the approved selection).
    pub cookie_domains: Option<Vec<String>>,
    /// Send these saved Keyvault items (ids) instead of capturing the live
    /// app. Nothing is read from the host, so its Keychain is never asked;
    /// the broker authorizes it with the user's presence as it does any
    /// delivery.
    pub from_vault: Option<Vec<String>>,
    /// Also send the saved passwords (of `cookie_domains`, or every site):
    /// only when the user ticked them in the review. They are re-encrypted
    /// for the destination browser's own key.
    pub include_passwords: bool,
}

/// Session teleport on a [`Space`]: what it can import, and the export and
/// import of an approved app session.
#[allow(async_fn_in_trait)]
pub trait SpaceTeleport {
    /// What the Space can import for `app` (`TeleportService.GetManifest`).
    async fn teleport_receiver(
        &self,
        app: &str,
        scope: TeleportScope,
    ) -> Result<pb::GetManifestResponse>;

    /// Exports the approved items on this host and imports them into the
    /// Space, verified by SHA-256. The approval must be for this Space.
    async fn teleport(
        &self,
        sessions: Arc<AppSessions>,
        approval: &Approval,
        options: ImportOptions,
    ) -> Result<TeleportReceipt>;

    /// [`Self::teleport`], telling `stage` where the Keyvault is: reading
    /// (when macOS asks for the Keychain), saving, packing, uploading and
    /// importing.
    async fn teleport_with_progress(
        &self,
        sessions: Arc<AppSessions>,
        approval: &Approval,
        options: ImportOptions,
        stage: &mut (dyn FnMut(cua_keyvault::broker::TeleportStage) + Send),
    ) -> Result<TeleportReceipt>;

    /// [`Self::teleport_with_progress`] with the review's choices: cookies of
    /// only some sites, or saved Keyvault items sent without a fresh capture.
    async fn teleport_selected(
        &self,
        sessions: Arc<AppSessions>,
        approval: &Approval,
        options: ImportOptions,
        selection: &TeleportSelection,
        stage: &mut (dyn FnMut(cua_keyvault::broker::TeleportStage) + Send),
    ) -> Result<TeleportReceipt>;
}

impl SpaceTeleport for Space {
    /// What the Space can import for `app` (`TeleportService.GetManifest`).
    async fn teleport_receiver(
        &self,
        app: &str,
        scope: TeleportScope,
    ) -> Result<pb::GetManifestResponse> {
        Ok(self
            .spacesd()?
            .teleport()
            .get_manifest(pb::GetManifestRequest {
                app: app.into(),
                scope: scope.spacesd() as i32,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner())
    }

    /// Imports and delivers the approved items into the Space in one call
    /// to the Cua Keyvault (`keyvault.sock`, as the signed `cua` app/CLI),
    /// verified by SHA-256 on the Space's side same as before. The approval
    /// must be for this Space. `sessions` is used only to fail fast, with a
    /// clear local error, when `approval.app` has no provider on this host
    /// at all -- the actual capture runs inside the daemon that hosts the
    /// Keyvault, which is also this host, so this is a redundant local
    /// check, not a fallback path: nothing here reads a profile or a secret.
    ///
    /// `options.save_to_keyvault` (the review sheet's "Save to Keyvault"
    /// checkbox) decides whether the captured item stays sealed in the
    /// vault afterward for later agent reuse (see [`crate::daemon::keyvault`]
    /// for that other, grant-gated route in) or is forgotten right after
    /// this one delivery, which is the default: the point of a plain
    /// teleport is the delivery, not a kept copy a person did not ask to
    /// keep. The capture, the authorization and the delivery are all still
    /// in the Keyvault's hash-chained audit log either way, and the kill
    /// switch (**Disable Keyvault**) refuses this exactly like every other
    /// route.
    ///
    /// Fails closed with [`Error::HostCapabilityMissing`] when no Keyvault
    /// is reachable (the Cua daemon is not running, or this process is not
    /// signed as first party): a plain teleport never falls back to
    /// exporting and uploading a bundle on its own.
    async fn teleport(
        &self,
        sessions: Arc<AppSessions>,
        approval: &Approval,
        options: ImportOptions,
    ) -> Result<TeleportReceipt> {
        self.teleport_with_progress(sessions, approval, options, &mut |_| {})
            .await
    }

    async fn teleport_with_progress(
        &self,
        sessions: Arc<AppSessions>,
        approval: &Approval,
        // `replace_existing` and `close_running_app` are already inert on
        // the direct upload path too (`replace_existing` has no
        // receiver-side effect at all; `close_running_app` is accepted and
        // reported back as "not supported by this driver" --
        // `cua-spacesd-server`'s `services/teleport.rs`), so routing
        // through the Keyvault changes nothing about them. `launch_after`
        // is threaded through the Keyvault's `import_and_teleport` (the
        // receiver launches the app in the Space's GUI session and the
        // receipt reports whether it did). `relay_plaintext_ack`
        // (S1) is the direct upload path's own concern -- it answers
        // whether *this* connection crosses an unsealed relay
        // (`self.spacesd()`); the Keyvault's own delivery is a separate
        // connection the broker makes on its own, which does not yet have
        // (or need, pending its own transport) an equivalent check. All
        // three are tracked as follow-up work, not specific to this path.
        // `save_to_keyvault` is honored below.
        options: ImportOptions,
        stage: &mut (dyn FnMut(cua_keyvault::broker::TeleportStage) + Send),
    ) -> Result<TeleportReceipt> {
        self.teleport_selected(
            sessions,
            approval,
            options,
            &TeleportSelection::default(),
            stage,
        )
        .await
    }

    async fn teleport_selected(
        &self,
        sessions: Arc<AppSessions>,
        approval: &Approval,
        options: ImportOptions,
        selection: &TeleportSelection,
        stage: &mut (dyn FnMut(cua_keyvault::broker::TeleportStage) + Send),
    ) -> Result<TeleportReceipt> {
        if approval.space != self.id().to_string() {
            return Err(Error::TeleportRefused(format!(
                "this approval is for {}, not {}",
                approval.space,
                self.id()
            )));
        }
        self.require(&format!("teleport.{}", approval.app))?;
        // Local, read-only pre-check: a clear "no such provider" error
        // before the network round trip, not a second capture path.
        sessions.resolve_checked(&approval.app)?;

        // Teleport sets up the environment: a coding agent's session lands
        // on a pinned, verified install of that agent with the cua skills
        // (the same installer and skills as `cua agent run`). Space-side
        // setup, unrelated to the Keyvault; done regardless of how the
        // session itself moves.
        if let Some(h) = cua_agents::harness::harness(&approval.app) {
            let agents = self.agents().await?;
            agents.ensure(h.installs, |_| {}).await?;
            agents.install_skills(h).await?;
        }

        let spec = cua_keyvault::broker::ImportSpec {
            app: approval.app.clone(),
            profile: None,
            sites: vec![],
            whole_app: true,
            cookies: cua_keyvault::broker::CookieFilter::default(),
            confirm_passwords: true,
            paths: Some(approval.paths.clone()),
            domains: selection.cookie_domains.clone(),
            passwords: selection.include_passwords,
        };
        let mut client = cua_keyvault::client::KeyvaultClient::connect_default()
            .await
            .map_err(|e| Error::HostCapabilityMissing {
                what: "teleport".into(),
                why: format!(
                    "the Cua Keyvault is not reachable ({e}); install or open Cua, or run this \
                     as the signed `cua` app/CLI"
                ),
            })?;
        let outcome = match &selection.from_vault {
            // Saved items: no capture, so nothing is read from the host and
            // the Keychain is never asked. One delivery, one presence check.
            Some(items) => {
                stage(cua_keyvault::broker::TeleportStage::Packing);
                let out = client
                    .teleport(cua_keyvault::broker::TeleportRequest {
                        token: None,
                        items: items.clone(),
                        target: self.id().to_string(),
                        include_passwords: selection.include_passwords,
                        launch: options.launch_after,
                    })
                    .await
                    .map_err(|e| Error::TeleportRefused(format!("{}: {e}", approval.app)))?;
                stage(cua_keyvault::broker::TeleportStage::Importing);
                out
            }
            None => client
                .import_and_teleport_launching(
                    spec,
                    self.id().to_string(),
                    options.save_to_keyvault,
                    options.launch_after,
                    stage,
                )
                .await
                .map_err(|e| Error::TeleportRefused(format!("{}: {e}", approval.app)))?,
        };
        let delivery = outcome.deliveries.first();
        Ok(TeleportReceipt {
            app: approval.app.clone(),
            space: self.id().to_string(),
            method: "import_session",
            transferred_paths: approval.paths.clone(),
            bundle_bytes: 0,
            bundle_sha256: String::new(),
            imported: delivery.map(|d| d.imported.clone()).unwrap_or_default(),
            skipped: delivery.map(|d| d.skipped.clone()).unwrap_or_default(),
            launched: delivery.is_some_and(|d| d.launched),
        })
    }
}

// ------------------------------------------------------------------ tools

/// The `teleport_manifest` and `teleport_app` contract tools and the
/// in-process `teleport.send` operation, over `sessions`.
pub(crate) async fn tool(
    sessions: Arc<AppSessions>,
    call: cua_spaces::extension::ToolCall<'_>,
) -> Result<ToolOutcome> {
    use cua_spaces_contract::inputs as i;
    let cua_spaces::extension::ToolCall {
        spaces,
        tool,
        args,
        broker,
        ..
    } = call;
    match tool {
        "teleport_manifest" => {
            let a: i::TeleportManifest = crate::args(tool, args)?;
            let scope = TeleportScope::parse(a.scope.as_deref())?;
            let manifest = sessions.manifest(&a.app, scope)?;
            let mut out = serde_json::to_value(&manifest)?;
            if let Some(space) = a.space {
                let s = spaces.space(&space).await?;
                let r = s.teleport_receiver(&manifest.app, scope).await?;
                out["receiver"] = json!({
                    "space": s.id().to_string(),
                    "supported": r.supported,
                    "limitation": r.limitation,
                    "app_installed": r.app_installed,
                    "supported_apps": r.supported_apps,
                });
            }
            Ok(ToolOutcome::json(&out))
        }
        "teleport_app" => {
            // Red-team E2 / T4: this tool is the automation (MCP / cua.sock)
            // surface. It must NOT deliver the user's logged-in session on a
            // caller-supplied `acknowledge_sensitive`: that is a headless,
            // consent-free credential export an LLM or a malicious caller can
            // drive (data exfiltration). Delivery only runs behind a live user
            // consent and a granted, per-target capability from the Cua
            // Keyvault, never from this tool self-approving.
            //
            // So this tool is fail-closed like `teleport_browser_session`: it
            // returns what *would* move plus a consent requirement, and the
            // user approves in Cua (Touch ID) or the Keyvault matches an
            // unattended rule. The Keyvault broker performs the delivery over
            // the daemon's authenticated channel; it is never done here.
            let a: i::TeleportApp = crate::args(tool, args)?;
            let scope = TeleportScope::parse(a.scope.as_deref())?;
            let s = spaces.space(&a.space).await?;
            let target = s.id().to_string();
            let manifest = sessions.manifest(&a.app, scope)?;
            let selected = match &a.include {
                Some(paths) => manifest.approving(&target, paths, false),
                None => manifest.approving_default(&target, false),
            };
            // Report the selection (or why it is ambiguous) without moving it.
            let would_send = serde_json::to_value(&manifest)?;
            teleport_app_broker(
                broker,
                &manifest.app,
                &target,
                a.request_id.as_deref(),
                selected.is_ok(),
                would_send,
            )
            .await
        }
        // In process only (never an MCP tool): the SDK's embedded
        // `Space.teleport`. The caller runs in the user's own process and
        // decided with the manifest in hand; the approval is minted here
        // from that decision, exactly as the SDK did before.
        "teleport.send" => {
            #[derive(serde::Deserialize)]
            struct Send {
                space: String,
                app: String,
                scope: Option<String>,
                include: Option<Vec<String>>,
                #[serde(default)]
                acknowledge_sensitive: bool,
            }
            let a: Send = crate::args(tool, args)?;
            let scope = TeleportScope::parse(a.scope.as_deref())?;
            let s = spaces.space(&a.space).await?;
            let target = s.id().to_string();
            let current = sessions.manifest(&a.app, scope)?;
            let approval = match &a.include {
                Some(paths) => current.approving(&target, paths, a.acknowledge_sensitive)?,
                None => current.approving_default(&target, a.acknowledge_sensitive)?,
            };
            let receipt = s
                .teleport(
                    sessions,
                    &approval,
                    ImportOptions {
                        launch_after: true,
                        ..ImportOptions::default()
                    },
                )
                .await?;
            Ok(ToolOutcome::json(&receipt))
        }
        other => Err(Error::NotFound(format!("tool {other}"))),
    }
}

/// The `teleport_app` consent and delivery flow. The tool never mints its own
/// consent and never delivers itself: it files a Keyvault request and, on a
/// retry, has the broker perform the delivery when the user approved it. With
/// no broker wired in (a daemon-less MCP host) it is fail-closed: it returns a
/// consent requirement and moves nothing.
async fn teleport_app_broker(
    broker: Option<&std::sync::Arc<dyn cua_spaces::teleport_broker::SessionBroker>>,
    app: &str,
    target: &str,
    request_id: Option<&str>,
    selection_ok: bool,
    would_send: Value,
) -> Result<ToolOutcome> {
    use cua_spaces::teleport_broker::{SessionBrokerError, SessionDelivery};
    const DURATION_SECS: u64 = 15 * 60;
    const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

    let broker_error = |e: SessionBrokerError| -> ToolOutcome {
        let out = json!({
            "moved": false,
            "app": app,
            "space": target,
            "error": { "code": e.code, "message": e.message },
        });
        ToolOutcome {
            content: vec![json!({"type": "text", "text": out.to_string()})],
            structured: Some(out),
            is_error: true,
            meta: None,
        }
    };

    let Some(broker) = broker else {
        let out = json!({
            "consent_required": true,
            "moved": false,
            "app": app,
            "space": target,
            "selection_ok": selection_ok,
            "would_send": would_send,
            "message": format!(
                "Teleporting {app}'s signed-in session moves a credential into Space {target}. \
                 Cua never does this from an automated call. Approve it in Cua (Keyvault page, \
                 with Touch ID or your login password), or set an unattended rule for this app \
                 and Space. The Keyvault delivers it; this tool cannot."
            ),
            "instructions": "Show the user what would move and ask them to approve it in Cua. \
                             Do not retry expecting delivery: this tool is consent-gated by design.",
        });
        return Ok(ToolOutcome::json(&out));
    };

    match request_id {
        None => {
            let reason = format!("teleport_app: move {app}'s signed-in session into {target}");
            match broker
                .request_access(app, target, DURATION_SECS, &reason)
                .await
            {
                Ok(id) => {
                    let out = json!({
                        "consent_required": true,
                        "moved": false,
                        "request_id": id,
                        "app": app,
                        "space": target,
                        "selection_ok": selection_ok,
                        "would_send": would_send,
                        "message": format!(
                            "Moving {app}'s signed-in session into Space {target} needs the user's approval."
                        ),
                        "approve": format!(
                            "The user approves request {id} in Cua (Keyvault page), which asks for Touch ID or the login password."
                        ),
                        "instructions": "Show the user what would move and ask them to approve it in Cua. Then call teleport_app again with the same app and space and this request_id. Do not retry without the user.",
                    });
                    Ok(ToolOutcome::json(&out))
                }
                Err(e) => Ok(broker_error(e)),
            }
        }
        Some(id) => match broker.await_and_deliver(app, target, id, WAIT).await {
            Ok(SessionDelivery::Pending) => Ok(ToolOutcome::json(&json!({
                "consent_required": true,
                "moved": false,
                "request_id": id,
                "status": "pending",
                "instructions": "The user has not decided yet. Remind them to approve it in Cua, then call again with this request_id.",
            }))),
            Ok(SessionDelivery::Denied(why)) => Ok(ToolOutcome::json(&json!({
                "moved": false,
                "denied": true,
                "message": format!("the user declined the teleport ({why}); do not ask again unless they bring it up"),
            }))),
            Ok(SessionDelivery::Delivered {
                items,
                imported,
                import_ids,
                expires_ms,
                ..
            }) => Ok(ToolOutcome::json(&json!({
                "consent_required": false,
                "moved": true,
                "app": app,
                "space": target,
                "items": items,
                "transferred_paths": imported,
                "import_ids": import_ids,
                "expires_ms": expires_ms,
                "next": if expires_ms == 0 {
                    "The Space now has this app's session. It stays until you wipe it in Cua's Keyvault or delete the Space."
                } else {
                    "The Space now has this app's session. Delete the Space when done; the delivered session is also wiped when it expires."
                },
            }))),
            Err(e) => Ok(broker_error(e)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_every_provider_with_names_and_aliases() {
        let sessions = AppSessions::with_host(Arc::new(providers::FakeHost::new()));
        let probed = std::sync::atomic::AtomicUsize::new(0);
        let catalog = sessions.catalog_with(|_| {
            probed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            false
        });
        assert!(probed.into_inner() > 0, "the injected probe answers");
        // A provider without a probe counts as installed; every probed one
        // takes the injected answer.
        for p in &catalog {
            let probe = sessions
                .registry()
                .find_by_id(&p.id)
                .and_then(|x| x.install_probe());
            assert_eq!(p.installed, probe.is_none(), "{}", p.id);
        }
        assert_eq!(
            catalog.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            sessions.providers()
        );
        let firefox = catalog.iter().find(|p| p.id == "firefox").expect("firefox");
        assert!(!firefox.display_name.is_empty());
        assert!(catalog.iter().all(|p| !p.display_name.is_empty()));
    }

    /// The real Claude Code manifest shape, to the byte counts it publishes.
    fn claude_code_manifest() -> TeleportManifest {
        TeleportManifest {
            app: "claude-code".into(),
            display_name: "Claude Code".into(),
            scope: TeleportScope::Full,
            total_estimated_bytes: 935_980_082,
            notes: vec!["The logged-in session carries an OAuth token.".into()],
            items: vec![
                TeleportItem {
                    relative_path: "claude/.credentials.json".into(),
                    label: "Logged-in session".into(),
                    estimated_bytes: 1_024,
                    is_sensitive: true,
                    is_checked_by_default: true,
                    count: None,
                    count_noun: None,
                },
                TeleportItem {
                    relative_path: "claude/settings.json".into(),
                    label: "Settings".into(),
                    estimated_bytes: 4_096,
                    is_sensitive: false,
                    is_checked_by_default: true,
                    count: None,
                    count_noun: None,
                },
                TeleportItem {
                    relative_path: "claude/projects/".into(),
                    label: "Conversation transcripts".into(),
                    estimated_bytes: 935_974_962,
                    is_sensitive: true,
                    is_checked_by_default: false,
                    count: Some(14),
                    count_noun: Some("projects".into()),
                },
            ],
        }
    }

    #[test]
    fn the_default_selection_leaves_the_expensive_item_out() {
        let m = claude_code_manifest();
        let default: Vec<String> = m
            .default_selection()
            .into_iter()
            .map(|i| i.relative_path)
            .collect();
        assert_eq!(
            default,
            vec!["claude/.credentials.json", "claude/settings.json"]
        );
        let login: Vec<String> = m
            .login_only_selection()
            .into_iter()
            .map(|i| i.relative_path)
            .collect();
        assert_eq!(login, vec!["claude/.credentials.json"]);
    }

    /// The real Firefox manifest shape after the "Keep me signed in" opt-in
    /// (cookies.sqlite is sensitive but never checked by default; `prefs.js`
    /// is checked by default but not sensitive).
    fn firefox_manifest() -> TeleportManifest {
        TeleportManifest {
            app: "firefox".into(),
            display_name: "Firefox".into(),
            scope: TeleportScope::Full,
            total_estimated_bytes: 10_000,
            notes: vec![],
            items: vec![
                TeleportItem {
                    relative_path: "firefox/tabs.json".into(),
                    label: "Open tabs".into(),
                    estimated_bytes: 0,
                    is_sensitive: false,
                    is_checked_by_default: true,
                    count: None,
                    count_noun: None,
                },
                TeleportItem {
                    relative_path: "firefox/prefs.js".into(),
                    label: "Preferences".into(),
                    estimated_bytes: 512,
                    is_sensitive: false,
                    is_checked_by_default: true,
                    count: None,
                    count_noun: None,
                },
                TeleportItem {
                    relative_path: "firefox/cookies.sqlite".into(),
                    label: "Cookies and sessions".into(),
                    estimated_bytes: 4_096,
                    is_sensitive: true,
                    is_checked_by_default: false,
                    count: None,
                    count_noun: None,
                },
                TeleportItem {
                    relative_path: "firefox/places.sqlite".into(),
                    label: "Bookmarks and history".into(),
                    estimated_bytes: 4_096,
                    is_sensitive: true,
                    is_checked_by_default: false,
                    count: None,
                    count_noun: None,
                },
            ],
        }
    }

    /// Regression for the live teleport check: Firefox's cookies became an
    /// opt-in ("Keep me signed in", never checked by default) alongside
    /// saved passwords and history, so `login_only_selection`'s old rule
    /// (filter the *default* selection for anything sensitive) always came
    /// back empty of anything sensitive for Firefox and fell back to the
    /// non-sensitive defaults (`tabs.json`, `prefs.js`) -- a "moved" import
    /// that dropped the signed-in cookie itself. It must find the session
    /// (the `SignIns` opt-in) directly, and never pick up the history opt-in
    /// (a real opt-in, but not "the session").
    #[test]
    fn login_only_selection_finds_a_browsers_opted_out_cookie() {
        let m = firefox_manifest();
        let default: Vec<String> = m
            .default_selection()
            .into_iter()
            .map(|i| i.relative_path)
            .collect();
        assert_eq!(default, vec!["firefox/tabs.json", "firefox/prefs.js"]);
        let login: Vec<String> = m
            .login_only_selection()
            .into_iter()
            .map(|i| i.relative_path)
            .collect();
        assert_eq!(login, vec!["firefox/cookies.sqlite"]);
    }

    #[test]
    fn an_approval_cannot_name_a_path_the_manifest_did_not_offer() {
        let e = claude_code_manifest()
            .approving("local:s", &["claude/../../etc/passwd".into()], true)
            .unwrap_err();
        assert_eq!(e.tag(), "teleport_refused");
        assert!(e.to_string().contains("not in this manifest"));
    }

    #[test]
    fn a_sensitive_entry_requires_an_explicit_acknowledgement() {
        let m = claude_code_manifest();
        assert!(
            m.approving("s", &["claude/.credentials.json".into()], false)
                .is_err()
        );
        assert!(
            m.approving("s", &["claude/.credentials.json".into()], true)
                .is_ok()
        );
        assert!(
            m.approving("s", &["claude/settings.json".into()], false)
                .is_ok()
        );
        assert!(m.approving_default("s", false).is_err());
    }

    #[test]
    fn no_selection_means_the_default_and_an_empty_one_is_refused() {
        let m = claude_code_manifest();
        let d = m.approving_default("s", true).unwrap();
        assert!(d.uses_default());
        assert_eq!(d.approved_bytes(), 1_024 + 4_096);
        assert_eq!(
            d.approved_paths(),
            ["claude/.credentials.json", "claude/settings.json"]
        );
        assert_eq!(
            m.approving("s", &[], true).unwrap_err().tag(),
            "teleport_refused"
        );
    }

    #[test]
    fn approving_everything_is_spelled_out_and_costs_what_it_says() {
        let m = claude_code_manifest();
        let all: Vec<String> = m.items.iter().map(|i| i.relative_path.clone()).collect();
        assert_eq!(
            m.approving("s", &all, true).unwrap().approved_bytes(),
            935_980_082
        );
    }

    #[test]
    fn scopes_parse_and_refuse() {
        assert_eq!(TeleportScope::parse(None).unwrap(), TeleportScope::Full);
        assert_eq!(
            TeleportScope::parse(Some("tabs")).unwrap(),
            TeleportScope::Tabs
        );
        assert!(TeleportScope::parse(Some("everything")).is_err());
    }
}
