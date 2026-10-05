// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `Teleport` (feature `teleport`): move a desktop app session from this
//! machine into a sandbox, on `cua-teleport`.
//!
//! [`Teleport::manifest`] describes what would move (for a consent UI);
//! [`Teleport::send`] captures the selected items (after the optional
//! [`TeleportApproval`] callback and, for sensitive items, the OS
//! authorization prompt) and uploads them to the sandbox's cua-spacesd,
//! which imports them and relaunches the app.

use std::sync::Arc;

use cua_sdk::support::run;
use cua_sdk::{Cua, Sandbox, SpacesdClient, TeleportItem, TeleportManifest};
use cua_sdk::{CuaError, Result};

/// How much of an app session to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TeleportScope {
    /// Only the open tabs / documents and session state.
    Tabs,
    /// The full profile (may include cookies, logins, history).
    Full,
}

impl From<TeleportScope> for cua_teleport::TransferScope {
    fn from(s: TeleportScope) -> Self {
        match s {
            TeleportScope::Tabs => Self::TabsOnly,
            TeleportScope::Full => Self::FullProfile,
        }
    }
}

// `TeleportItem` / `TeleportManifest` are shared with the Spaces API
// (`teleport_types`), so one consent UI renders both.

pub(crate) fn item_from(i: cua_teleport::ManifestItem) -> TeleportItem {
    {
        TeleportItem {
            relative_path: i.rel_path,
            label: i.label,
            estimated_bytes: i.est_bytes,
            is_sensitive: i.sensitive,
            is_checked_by_default: i.default_checked,
            count: i.count,
            count_noun: i.count_noun,
        }
    }
}

pub(crate) fn manifest_from(m: cua_teleport::TransferManifest) -> TeleportManifest {
    {
        TeleportManifest {
            app: m.provider_id,
            display_name: m.app_display_name,
            scope: match m.scope {
                cua_teleport::TransferScope::TabsOnly => "tabs".into(),
                cua_teleport::TransferScope::FullProfile => "full".into(),
            },
            items: m.items.into_iter().map(item_from).collect(),
            total_estimated_bytes: m.total_est_bytes,
            notes: m.notes,
        }
    }
}

/// An app this machine can teleport.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TeleportProvider {
    /// Provider id.
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Bundle ids and app names it matches.
    pub app_ids: Vec<String>,
    /// Exports on macOS.
    pub macos: bool,
    /// Exports on Linux.
    pub linux: bool,
    /// Exports on Windows.
    pub windows: bool,
    /// Install check: a path, or a binary name when `install_probe_on_path`.
    pub install_probe: Option<String>,
    /// Whether `install_probe` is a `PATH` binary name.
    pub install_probe_on_path: bool,
}

/// What the approval callback is asked.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TeleportApprovalRequest {
    /// Everything that could move.
    pub manifest: TeleportManifest,
    /// What will move.
    pub selected: Vec<TeleportItem>,
    /// Whether any selected item is sensitive (the OS prompt follows).
    pub sensitive: bool,
    /// The destination spacesd endpoint.
    pub destination: String,
}

/// Consent callback: shown the manifest and selection before anything is
/// read; return `false` to abort. Runs on a worker thread and may block (for
/// example on a dialog). It never replaces the OS authorization prompt for
/// sensitive items.
#[uniffi::export(with_foreign)]
pub trait TeleportApproval: Send + Sync {
    /// Approve or decline.
    fn approve(&self, request: TeleportApprovalRequest) -> bool;
}

/// Options for [`Teleport::manifest`] and [`Teleport::send`].
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct TeleportOptions {
    /// Chrome profile to capture: a name ("Profile 1") or a path. Default
    /// `Default`.
    #[uniffi(default = None)]
    pub chrome_profile: Option<String>,
    /// Display name for the app (UI only). Default: the app id.
    #[uniffi(default = None)]
    pub display_name: Option<String>,
    /// Launch the app in the sandbox after importing. Default true.
    #[uniffi(default = None)]
    pub launch_after: Option<bool>,
    /// Ask the sandbox to close a running instance first.
    #[uniffi(default = false)]
    pub close_running_app: bool,
    /// Explicit, per-delivery opt-in to send over a `relay:` Space whose
    /// image predates end-to-end sealing (S1). Without it, a `relay:`
    /// destination fails with a `PermissionDenied` naming the risk
    /// (`cua_teleport::send::RELAY_UNSEALED_WARNING`) before anything is
    /// read or uploaded. Show that warning and get the user's explicit,
    /// per-delivery consent before setting this; never a standing setting.
    /// Local and direct Spaces are unaffected.
    #[uniffi(default = false)]
    pub relay_plaintext_ack: bool,
}

/// The result of [`Teleport::send`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TeleportResult {
    /// Provider id.
    pub provider_id: String,
    /// Upload id.
    pub import_id: String,
    /// Bundle size.
    pub bundle_bytes: u64,
    /// Bundle SHA-256 (hex).
    pub sha256: String,
    /// Selected item keys.
    pub sent: Vec<String>,
    /// Item keys the default selection left out.
    pub withheld: Vec<String>,
    /// What the sandbox imported.
    pub imported: Vec<String>,
    /// What it skipped, as "item: reason".
    pub skipped: Vec<String>,
    /// Whether it launched the app.
    pub launched: bool,
}

impl From<cua_teleport::SendOutcome> for TeleportResult {
    fn from(o: cua_teleport::SendOutcome) -> Self {
        Self {
            provider_id: o.provider_id,
            import_id: o.import_id,
            bundle_bytes: o.bundle_bytes,
            sha256: o.sha256,
            sent: o.sent,
            withheld: o.withheld,
            imported: o.imported,
            skipped: o.skipped,
            launched: o.launched,
        }
    }
}

pub(crate) fn teleport_err(e: cua_teleport::Error) -> CuaError {
    {
        use cua_teleport::{Error as E, TeleportError as T};
        let m = e.to_string();
        match e {
            E::Env(e) => e.into(),
            E::InvalidSelection(_) => CuaError::InvalidArgument(m),
            E::NotApproved => CuaError::PermissionDenied(m),
            E::Unsupported { .. } => CuaError::Unsupported(m),
            E::Teleport(T::NoProviderForApp { .. } | T::UnknownProvider { .. }) => {
                CuaError::Unsupported(m)
            }
            E::Teleport(T::UnsupportedScope { .. }) => CuaError::Unsupported(m),
            E::Teleport(T::Provider(p)) if p.contains("not authorized") => {
                CuaError::PermissionDenied(m)
            }
            // S1: the destination is a relay: Space and nothing has sealed
            // this delivery yet; the caller must set
            // TeleportOptions::relay_plaintext_ack after showing the user
            // the warning in `m`.
            E::RelayUnsealed => CuaError::PermissionDenied(m),
            E::Teleport(_) | E::Task(_) => CuaError::Internal(m),
        }
    }
}

struct ForeignApproval(Arc<dyn TeleportApproval>);

impl cua_teleport::Approval for ForeignApproval {
    fn approve(&self, r: &cua_teleport::ApprovalRequest<'_>) -> bool {
        self.0.approve(TeleportApprovalRequest {
            manifest: manifest_from(r.manifest.clone()),
            selected: r.selected.iter().cloned().map(item_from).collect(),
            sensitive: r.sensitive,
            destination: r.destination.clone(),
        })
    }
}

/// Teleport send. Exports from this machine's real apps; tests of hosts
/// embedding the SDK must set `CUA_ENV_TEST_SANDBOX=1` (every host effect is
/// then refused) or use `cua-teleport` directly with a fake host.
#[derive(uniffi::Object)]
pub struct Teleport {
    pub(crate) host: Arc<dyn cua_teleport::HostEffects>,
}

impl Teleport {
    async fn send_through_keyvault_outer(
        &self,
        sandbox: &Sandbox,
        app: String,
        scope: TeleportScope,
        selected_items: Option<Vec<String>>,
        approval: Option<Arc<dyn TeleportApproval>>,
        options: Option<TeleportOptions>,
    ) -> Result<TeleportResult> {
        let started = std::time::Instant::now();
        let app_id = app.clone();
        let target = sandbox.id();
        let r = self
            .send_through_keyvault_inner(target, app, scope, selected_items, approval, options)
            .await;
        cua_sdk::support::record_teleport(
            &app_id,
            "full",
            "app_with_state",
            started,
            &r,
            r.as_ref().map(|r| r.sent.len() as u64).unwrap_or(0),
        );
        r
    }

    async fn send_through_keyvault_inner(
        &self,
        target: String,
        app: String,
        scope: TeleportScope,
        selected_items: Option<Vec<String>>,
        approval: Option<Arc<dyn TeleportApproval>>,
        options: Option<TeleportOptions>,
    ) -> Result<TeleportResult> {
        let options = options.unwrap_or_default();
        let teleporter = self.teleporter(&options);
        let app_ref = app_ref(app, &options);
        let scope: cua_teleport::TransferScope = scope.into();
        let selection = match selected_items {
            Some(items) => cua_teleport::Selection::Items(items),
            None => cua_teleport::Selection::Default,
        };
        let manifest = {
            let app_ref = app_ref.clone();
            tokio::task::spawn_blocking(move || teleporter.manifest(&app_ref, scope))
                .await
                .map_err(|e| CuaError::Internal(e.to_string()))?
                .map_err(teleport_err)?
        };
        let (_, selected, withheld) =
            cua_teleport::resolve_selection(&manifest, &selection).map_err(teleport_err)?;
        let sensitive = selected.iter().any(|i| i.sensitive);
        if let Some(a) = &approval {
            let request = TeleportApprovalRequest {
                manifest: manifest_from(manifest.clone()),
                selected: selected.iter().cloned().map(item_from).collect(),
                sensitive,
                destination: target.clone(),
            };
            let a = a.clone();
            let approved = tokio::task::spawn_blocking(move || a.approve(request))
                .await
                .map_err(|e| CuaError::Internal(e.to_string()))?;
            if !approved {
                return Err(teleport_err(cua_teleport::Error::NotApproved));
            }
        }
        let paths: Vec<String> = selected.iter().map(|i| i.rel_path.clone()).collect();
        let spec = cua_keyvault::broker::ImportSpec {
            app: manifest.provider_id.clone(),
            profile: None,
            sites: vec![],
            whole_app: true,
            cookies: cua_keyvault::broker::CookieFilter::default(),
            confirm_passwords: true,
            paths: Some(paths.clone()),
            domains: None,
            passwords: false,
        };
        run(async move {
            let mut client = cua_keyvault::client::KeyvaultClient::connect_default()
                .await
                .map_err(|e| {
                    CuaError::Unsupported(format!(
                        "teleport goes through the Cua Keyvault, and it is not reachable \
                         ({e}); install or open Cua, or run this as the signed `cua` app/CLI"
                    ))
                })?;
            let outcome = client
                .import_and_teleport(spec, target, false)
                .await
                .map_err(|e| {
                    CuaError::PermissionDenied(format!("{}: {e}", manifest.provider_id))
                })?;
            let delivery = outcome.deliveries.first().cloned().unwrap_or_default();
            Ok(TeleportResult {
                provider_id: manifest.provider_id.clone(),
                import_id: delivery.import_id,
                bundle_bytes: 0,
                sha256: String::new(),
                sent: paths,
                withheld: withheld.iter().map(|i| i.rel_path.clone()).collect(),
                imported: delivery.imported,
                skipped: delivery.skipped,
                launched: delivery.launched,
            })
        })
        .await
    }

    /// Rust hosts: teleport on a custom host (tests pass
    /// `cua_teleport::FakeHost` with a temporary home).
    pub fn with_host(host: Arc<dyn cua_teleport::HostEffects>) -> Arc<Self> {
        Arc::new(Self { host })
    }

    fn teleporter(&self, options: &TeleportOptions) -> cua_teleport::Teleporter {
        cua_teleport::Teleporter::with_registry(
            cua_teleport::ExportRegistry::with_builtin_host_and_chrome_profile(
                self.host.clone(),
                options.chrome_profile.clone(),
            ),
        )
        .options(cua_teleport::SendOptions {
            launch_after: options.launch_after.unwrap_or(true),
            close_running_app: options.close_running_app,
            relay_plaintext_ack: options.relay_plaintext_ack,
            ..Default::default()
        })
    }

    async fn send_to(
        &self,
        env: cua_spacesd_client::SpacesdClient,
        app: String,
        scope: TeleportScope,
        selected_items: Option<Vec<String>>,
        approval: Option<Arc<dyn TeleportApproval>>,
        options: TeleportOptions,
    ) -> Result<TeleportResult> {
        let started = std::time::Instant::now();
        let app_id = app.clone();
        let r = self
            .send_to_inner(env, app, scope, selected_items, approval, options)
            .await;
        cua_sdk::support::record_teleport(
            &app_id,
            "full",
            "app_with_state",
            started,
            &r,
            r.as_ref().map(|r| r.sent.len() as u64).unwrap_or(0),
        );
        r
    }

    async fn send_to_inner(
        &self,
        env: cua_spacesd_client::SpacesdClient,
        app: String,
        scope: TeleportScope,
        selected_items: Option<Vec<String>>,
        approval: Option<Arc<dyn TeleportApproval>>,
        options: TeleportOptions,
    ) -> Result<TeleportResult> {
        let teleporter = self.teleporter(&options);
        let app = app_ref(app, &options);
        let selection = match selected_items {
            Some(items) => cua_teleport::Selection::Items(items),
            None => cua_teleport::Selection::Default,
        };
        let approval: Arc<dyn cua_teleport::Approval> = match approval {
            Some(a) => Arc::new(ForeignApproval(a)),
            None => Arc::new(cua_teleport::AutoApprove),
        };
        run(async move {
            Ok(teleporter
                .send(&env, &app, scope.into(), selection, approval)
                .await
                .map_err(crate::teleport::teleport_err)?
                .into())
        })
        .await
    }
}

fn app_ref(app: String, options: &TeleportOptions) -> cua_teleport::AppRef {
    cua_teleport::AppRef {
        display_name: options.display_name.clone().unwrap_or_else(|| app.clone()),
        app_id: app,
        platform: cua_teleport::Platform::current(),
    }
}

/// Teleport send: move app sessions from this machine into sandboxes (the
/// Swift app calls it as `cua.teleport()`).
#[uniffi::export]
pub fn teleport(cua: Arc<Cua>) -> Arc<Teleport> {
    let _ = cua;
    Teleport::with_host(cua_teleport::default_host())
}

#[uniffi::export]
impl Teleport {
    /// Every app this SDK can teleport, with install probes for consent UIs.
    pub fn providers(&self) -> Vec<TeleportProvider> {
        cua_teleport::ExportRegistry::with_builtin_host(self.host.clone())
            .infos()
            .into_iter()
            .map(|i| TeleportProvider {
                id: i.id,
                display_name: i.display_name,
                app_ids: i.app_ids,
                macos: i.macos,
                linux: i.linux,
                windows: i.windows,
                install_probe_on_path: i.install_probe.as_ref().is_some_and(|p| p.on_path),
                install_probe: i.install_probe.map(|p| p.probe),
            })
            .collect()
    }

    /// Describes what teleporting `app` (a bundle id or app name, e.g.
    /// `"com.google.Chrome"` or `"Slack"`) would move. Reads the local
    /// profile; never prompts.
    pub async fn manifest(
        &self,
        app: String,
        scope: TeleportScope,
        options: Option<TeleportOptions>,
    ) -> Result<TeleportManifest> {
        let options = options.unwrap_or_default();
        let teleporter = self.teleporter(&options);
        let app = app_ref(app, &options);
        run(async move {
            tokio::task::spawn_blocking(move || teleporter.manifest(&app, scope.into()))
                .await
                .map_err(|e| CuaError::Internal(e.to_string()))?
                .map(manifest_from)
                .map_err(teleport_err)
        })
        .await
    }

    /// Teleports `app` into `sandbox` (through its cua-spacesd), a direct
    /// upload: the bundle is captured on this machine and sent straight to
    /// `sandbox`, never through the Keyvault. `selected_items` are manifest
    /// `rel_path`s; `None` sends the default selection. `approval` is asked
    /// first (`None` approves); sensitive items then need OS authorization.
    ///
    /// For an app whose captured items may include a signed-in browser
    /// session (cookies, saved passwords), prefer
    /// [`Self::send_through_keyvault`], which never lets this process or
    /// the network see the raw secret.
    pub async fn send(
        &self,
        sandbox: Arc<Sandbox>,
        app: String,
        scope: TeleportScope,
        selected_items: Option<Vec<String>>,
        approval: Option<Arc<dyn TeleportApproval>>,
        options: Option<TeleportOptions>,
    ) -> Result<TeleportResult> {
        let env = sandbox.spacesd(None).await?;
        self.send_to(
            env.inner().clone(),
            app,
            scope,
            selected_items,
            approval,
            options.unwrap_or_default(),
        )
        .await
    }

    /// Teleports `app` into `sandbox`, through the caller's own Cua
    /// Keyvault: the session is captured into the Keyvault (sealed, once
    /// Touch ID / presence gated) and delivered from there, so this
    /// process, the Swift app and the network never see a raw password or
    /// cookie. `selected_items` are manifest `rel_path`s; `None` sends the
    /// default selection. `approval` is asked first (`None` approves);
    /// the Keyvault's own presence prompt follows for a sensitive
    /// selection, in place of [`Teleporter`]'s OS authorization.
    ///
    /// Fails with [`CuaError::Unsupported`] when no Keyvault is reachable
    /// (the Cua daemon is not running, or this process is not signed as
    /// first party): this never falls back to [`Self::send`]'s direct
    /// upload, which would leave the session outside the Keyvault.
    pub async fn send_through_keyvault(
        &self,
        sandbox: Arc<Sandbox>,
        app: String,
        scope: TeleportScope,
        selected_items: Option<Vec<String>>,
        approval: Option<Arc<dyn TeleportApproval>>,
        options: Option<TeleportOptions>,
    ) -> Result<TeleportResult> {
        self.send_through_keyvault_outer(&sandbox, app, scope, selected_items, approval, options)
            .await
    }

    /// Like [`Self::send`], to a spacesd connection (for example
    /// `Cua.spacesd(url, token)`).
    pub async fn send_env(
        &self,
        env: Arc<SpacesdClient>,
        app: String,
        scope: TeleportScope,
        selected_items: Option<Vec<String>>,
        approval: Option<Arc<dyn TeleportApproval>>,
        options: Option<TeleportOptions>,
    ) -> Result<TeleportResult> {
        self.send_to(
            env.inner().clone(),
            app,
            scope,
            selected_items,
            approval,
            options.unwrap_or_default(),
        )
        .await
    }
}
