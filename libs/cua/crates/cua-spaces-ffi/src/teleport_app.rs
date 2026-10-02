// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Teleport an app…" and drag-and-drop onto a Space (feature `teleport`),
//! on `cua_teleport::ux`: the catalog of host apps with capability levels,
//! plans with consent items, runs with progress, drag payloads, and window
//! drags with a one-window preview. Every client (the Spaces app, the
//! OpenKoalaBots examples) shares these calls.

use std::path::PathBuf;
use std::sync::Arc;

use cua_teleport::ux::{self, UxError};

use crate::Teleport;
use cua_sdk::{CuaError, Result};

pub(crate) fn ux_err(e: UxError) -> CuaError {
    {
        let m = e.to_string();
        match e {
            UxError::Unsupported(_) => CuaError::Unsupported(m),
            UxError::Invalid(_) => CuaError::InvalidArgument(m),
            UxError::NotApproved(_) => CuaError::TeleportRefused(m),
            UxError::PermissionDenied(_) | UxError::HostEffectsRefused(_) => {
                CuaError::PermissionDenied(m)
            }
            UxError::Io(_) => CuaError::Internal(m),
        }
    }
}

/// What teleport can do with an app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TeleportCapability {
    /// Its signed-in state can move (a provider), on top of the app.
    Full,
    /// Installed in the Space; opens empty or with chosen files.
    InstallOnly,
    /// Shown disabled; `reason` says why.
    Unsupported,
}

/// What a teleport moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TeleportMove {
    /// The app only.
    AppOnly,
    /// The app plus chosen files or folders.
    AppWithFiles,
    /// The app plus its signed-in state (consent lists every path and
    /// secret).
    AppWithState,
}

impl From<ux::MoveKind> for TeleportMove {
    fn from(m: ux::MoveKind) -> Self {
        match m {
            ux::MoveKind::AppOnly => Self::AppOnly,
            ux::MoveKind::AppWithFiles => Self::AppWithFiles,
            ux::MoveKind::AppWithState => Self::AppWithState,
        }
    }
}

/// Credential-shaped state the signed-in state move leaves out by default,
/// which a person opts into one group at a time (each item is a secret in
/// the plan's consent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TeleportSensitiveGroup {
    /// The session cookies: what keeps the app signed in.
    SignIns,
    /// Saved passwords.
    Passwords,
    /// Browsing history.
    History,
}

impl From<ux::SensitiveGroup> for TeleportSensitiveGroup {
    fn from(g: ux::SensitiveGroup) -> Self {
        match g {
            ux::SensitiveGroup::SignIns => Self::SignIns,
            ux::SensitiveGroup::Passwords => Self::Passwords,
            ux::SensitiveGroup::History => Self::History,
        }
    }
}

impl From<TeleportSensitiveGroup> for ux::SensitiveGroup {
    fn from(g: TeleportSensitiveGroup) -> Self {
        match g {
            TeleportSensitiveGroup::SignIns => Self::SignIns,
            TeleportSensitiveGroup::Passwords => Self::Passwords,
            TeleportSensitiveGroup::History => Self::History,
        }
    }
}

impl From<TeleportMove> for ux::MoveKind {
    fn from(m: TeleportMove) -> Self {
        match m {
            TeleportMove::AppOnly => Self::AppOnly,
            TeleportMove::AppWithFiles => Self::AppWithFiles,
            TeleportMove::AppWithState => Self::AppWithState,
        }
    }
}

/// One row of "Teleport an app…".
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TeleportCatalogEntry {
    /// Stable id (`vscode`, `firefox`, or the host bundle id).
    pub id: String,
    /// Display name.
    pub name: String,
    /// The host app bundle, `.desktop` entry or shortcut.
    pub host_path: Option<String>,
    /// Host bundle or desktop id.
    pub host_app_id: Option<String>,
    /// Host app version.
    pub version: Option<String>,
    /// Capability level.
    pub capability: TeleportCapability,
    /// Why it is unsupported, or a caveat.
    pub reason: Option<String>,
    /// Offered moves, in UI order.
    pub moves: Vec<TeleportMove>,
    /// Provider id, when state can move.
    pub provider_id: Option<String>,
    /// The opt-in groups the signed-in state move offers
    /// ([`TeleportPlanOptions::sensitive_groups`]).
    pub sensitive_groups: Vec<TeleportSensitiveGroup>,
    /// `manifest` (pinned install), `image` (ships in cua's images),
    /// `space` (must already be there), or none.
    pub install_source: Option<String>,
    /// Install manifest id and pinned version, for `manifest`.
    pub install_id: Option<String>,
    pub install_version: Option<String>,
    /// Binary started in the Space.
    pub launch_bin: Option<String>,
    /// Last teleported (Unix ms).
    pub last_used_ms: Option<u64>,
    /// The entry as JSON (what `plan` reads back).
    pub json: String,
}

impl From<ux::CatalogEntry> for TeleportCatalogEntry {
    fn from(e: ux::CatalogEntry) -> Self {
        let json = serde_json::to_string(&e).unwrap_or_default();
        let (install_source, install_id, install_version) = match &e.install {
            Some(ux::InstallSource::Manifest { id, version, .. }) => (
                Some("manifest".into()),
                Some(id.clone()),
                Some(version.clone()),
            ),
            Some(ux::InstallSource::Image) => (Some("image".into()), None, None),
            Some(ux::InstallSource::Space) => (Some("space".into()), None, None),
            None => (None, None, None),
        };
        Self {
            id: e.id,
            name: e.name,
            host_path: e.host_path,
            host_app_id: e.host_app_id,
            version: e.version,
            capability: match e.capability {
                ux::Capability::Full => TeleportCapability::Full,
                ux::Capability::InstallOnly => TeleportCapability::InstallOnly,
                ux::Capability::Unsupported => TeleportCapability::Unsupported,
            },
            reason: e.reason,
            moves: e.moves.into_iter().map(Into::into).collect(),
            provider_id: e.provider_id,
            sensitive_groups: e.sensitive_groups.into_iter().map(Into::into).collect(),
            install_source,
            install_id,
            install_version,
            launch_bin: e.launch.map(|l| l.bin),
            last_used_ms: e.last_used_ms,
            json,
        }
    }
}

impl TeleportCatalogEntry {
    fn core(&self) -> Result<ux::CatalogEntry> {
        serde_json::from_str(&self.json).map_err(|e| {
            CuaError::InvalidArgument(format!("not a catalog entry from this SDK: {e}"))
        })
    }
}

/// Options for [`Teleport::catalog`].
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct TeleportCatalogOptions {
    /// App roots to scan. Default: this machine's (`/Applications`,
    /// `~/Applications`; `.desktop` dirs; Start Menu). Tests pass fixture
    /// directories.
    #[uniffi(default = None)]
    pub roots: Option<Vec<String>>,
    /// The Space's OS (`linux`, `macos`, `windows`), to narrow the catalog.
    #[uniffi(default = None)]
    pub space_os: Option<String>,
    /// The Space's CPU (`aarch64`, `x86_64`).
    #[uniffi(default = None)]
    pub space_arch: Option<String>,
    /// Recents file. Default: `~/.cua/teleport-recents.json`.
    #[uniffi(default = None)]
    pub recents_path: Option<String>,
}

fn hint(o: &TeleportCatalogOptions) -> Result<ux::TargetHint> {
    Ok(ux::TargetHint {
        os: match o.space_os.as_deref() {
            None | Some("") => None,
            Some("linux") => Some(cua_teleport::Platform::Linux),
            Some("macos") => Some(cua_teleport::Platform::MacOS),
            Some("windows") => Some(cua_teleport::Platform::Windows),
            Some(other) => {
                return Err(CuaError::InvalidArgument(format!(
                    "space_os must be linux, macos or windows (got {other:?})"
                )));
            }
        },
        arch: o.space_arch.clone().filter(|a| !a.is_empty()),
    })
}

/// A parsed drop.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TeleportDrop {
    /// `app`, `files`, `url` or `empty`.
    pub kind: String,
    /// App bundles, `.desktop` entries, shortcuts.
    pub apps: Vec<String>,
    /// Files and folders.
    pub files: Vec<String>,
    /// URLs.
    pub urls: Vec<String>,
    /// Unusable entries.
    pub ignored: Vec<String>,
}

/// One on-screen window of this machine.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TeleportWindow {
    /// Platform window id (for the thumbnail).
    pub window_id: u32,
    /// Owning process.
    pub pid: i64,
    /// Owning app.
    pub app_name: String,
    /// Title (empty without Screen Recording on macOS).
    pub title: String,
    /// The owning app's bundle, when known.
    pub bundle_path: Option<String>,
}

/// A window-drag event (global top-left display points).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TeleportWindowDragEvent {
    /// `start`, `move` or `end`.
    pub phase: String,
    pub x: f64,
    pub y: f64,
    /// The dragged window (`start`, `end`).
    pub window: Option<TeleportWindow>,
    /// Its app, classified (`start`, `end`).
    pub app: Option<TeleportCatalogEntry>,
    /// The window's frame at mouse down (`start`).
    #[uniffi(default = None)]
    pub start_frame: Option<crate::app_core_types::AppLogicalRect>,
    /// The window's frame now (`start`, `end`, and some `move`s). Compare
    /// with `start_frame` to tell a move from a resize
    /// (`app_drag_trigger_apply` does).
    #[uniffi(default = None)]
    pub frame: Option<crate::app_core_types::AppLogicalRect>,
}

/// Receives window-drag events on the monitor thread (keep it quick).
#[uniffi::export(with_foreign)]
pub trait TeleportWindowDragListener: Send + Sync {
    fn on_event(&self, event: TeleportWindowDragEvent);
}

/// A running window-drag monitor. `stop` (or dropping it) ends it.
#[derive(uniffi::Object)]
pub struct TeleportWindowDragMonitor {
    inner: ux::window::WindowDragMonitor,
}

#[uniffi::export]
impl TeleportWindowDragMonitor {
    /// Stops watching.
    pub fn stop(&self) {
        self.inner.stop();
    }
}

impl Teleport {
    fn registry(&self) -> cua_teleport::ExportRegistry {
        cua_teleport::ExportRegistry::with_builtin_host(self.host.clone())
    }
}

#[uniffi::export]
impl Teleport {
    /// "Teleport an app…": every installed app on this machine, classified
    /// (full, install only, unsupported with a reason), recents first.
    pub async fn catalog(
        &self,
        options: Option<TeleportCatalogOptions>,
    ) -> Result<Vec<TeleportCatalogEntry>> {
        let o = options.unwrap_or_default();
        let registry = self.registry();
        let req = ux::CatalogRequest {
            probe_providers: o.roots.is_none(),
            roots: o
                .roots
                .clone()
                .map(|r| r.into_iter().map(PathBuf::from).collect()),
            hint: hint(&o)?,
            recents_path: o.recents_path.clone().map(PathBuf::from),
        };
        cua_sdk::support::run(async move {
            tokio::task::spawn_blocking(move || ux::catalog(&registry, &req))
                .await
                .map_err(|e| CuaError::Internal(e.to_string()))?
                .map(|v| v.into_iter().map(Into::into).collect())
                .map_err(ux_err)
        })
        .await
    }

    /// Filters catalog rows by a search query (every word must match the
    /// name or id); order is kept.
    pub fn search_catalog(
        &self,
        entries: Vec<TeleportCatalogEntry>,
        query: String,
    ) -> Vec<TeleportCatalogEntry> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        entries
            .into_iter()
            .filter(|e| {
                let hay = format!(
                    "{} {} {}",
                    e.name.to_lowercase(),
                    e.id.to_lowercase(),
                    e.host_app_id.as_deref().unwrap_or("").to_lowercase()
                );
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect()
    }

    /// The catalog row for a dropped app (a bundle, `.desktop` entry or
    /// shortcut path).
    pub fn catalog_entry_for_path(
        &self,
        path: String,
        options: Option<TeleportCatalogOptions>,
    ) -> Result<TeleportCatalogEntry> {
        let o = options.unwrap_or_default();
        Ok(ux::entry_for_path(&self.registry(), &path, &hint(&o)?)
            .map_err(crate::teleport_app::ux_err)?
            .into())
    }

    /// The catalog row for an app known by name or id (a dragged window
    /// whose bundle is unknown).
    pub fn catalog_entry_for_name(
        &self,
        name: String,
        options: Option<TeleportCatalogOptions>,
    ) -> Result<TeleportCatalogEntry> {
        let o = options.unwrap_or_default();
        Ok(ux::entry_for_name(&self.registry(), &name, &hint(&o)?).into())
    }

    /// Parses a drag payload: paths, `file://` URIs, URLs or a
    /// `text/uri-list` blob.
    pub fn parse_drop(&self, items: Vec<String>) -> TeleportDrop {
        let p = ux::drop::parse(&items);
        TeleportDrop {
            kind: match p.kind() {
                ux::DropKind::App => "app",
                ux::DropKind::Files => "files",
                ux::DropKind::Url => "url",
                ux::DropKind::Empty => "empty",
            }
            .into(),
            apps: p.apps,
            files: p.files,
            urls: p.urls,
            ignored: p.ignored,
        }
    }

    /// An app's icon as PNG bytes (`size` points), or `None`.
    pub fn app_icon_png(&self, path: String, size: u32) -> Option<Vec<u8>> {
        ux::app_icon_png_cached(&path, size)
    }

    /// Warms the catalog's app list and every app's icon in the background
    /// (returns at once). Apps call it when they start so the first
    /// "Teleport an app…" opens on cached apps and icons.
    pub fn prefetch(&self) {
        let _ = std::thread::Builder::new()
            .name("cua-teleport-prefetch".into())
            .spawn(|| ux::prefetch(None));
    }

    /// Records a teleported app so the catalog lists it first.
    pub fn record_recent(&self, id: String, recents_path: Option<String>) -> Result<()> {
        ux::record_recent(recents_path.map(PathBuf::from), &id)
            .map_err(crate::teleport_app::ux_err)?;
        Ok(())
    }

    /// Whether window-drag detection exists on this OS (macOS today).
    pub fn window_drag_supported(&self) -> bool {
        ux::window::supported()
    }

    /// Whether this process may watch window drags (macOS Accessibility).
    pub fn window_drag_permitted(&self) -> bool {
        ux::window::permission_granted()
    }

    /// Asks for the window-drag permission (opens System Settings on
    /// macOS); returns the state after asking.
    pub fn request_window_drag_permission(&self) -> Result<bool> {
        ux::window::request_permission().map_err(crate::teleport_app::ux_err)
    }

    /// This machine's user windows (for a window picker).
    pub fn list_windows(&self) -> Result<Vec<TeleportWindow>> {
        Ok(ux::window::list_user_windows()
            .map_err(crate::teleport_app::ux_err)?
            .into_iter()
            .map(|w| TeleportWindow {
                window_id: w.window_id,
                pid: w.pid,
                app_name: w.owner,
                title: w.title,
                bundle_path: w.bundle_path,
            })
            .collect())
    }

    /// A PNG preview of that one window (never the screen or another
    /// window), at most `max_width` pixels wide, kept in memory for a few
    /// seconds (the SDK's preview cache). `None` when it cannot be
    /// captured (no Screen Recording permission).
    pub fn capture_window_thumbnail(
        &self,
        window_id: u32,
        max_width: Option<u32>,
    ) -> Result<Option<Vec<u8>>> {
        ux::capture_thumbnail_png_cached(
            window_id,
            max_width.unwrap_or(ux::window::THUMBNAIL_WIDTH as u32) as usize,
        )
        .map_err(crate::teleport_app::ux_err)
    }

    /// Watches for a real app window being dragged (macOS; needs the
    /// Accessibility permission). `start` and `end` carry the window and
    /// its classified app.
    pub fn start_window_drag(
        &self,
        listener: Arc<dyn TeleportWindowDragListener>,
    ) -> Result<Arc<TeleportWindowDragMonitor>> {
        let registry = Arc::new(self.registry());
        let inner = ux::window::WindowDragMonitor::start(Box::new(move |e| {
            listener.on_event(window_event(&registry, e));
        }))
        .map_err(crate::teleport_app::ux_err)?;
        Ok(Arc::new(TeleportWindowDragMonitor { inner }))
    }
}

/// Maps a core event, classifying the dragged window's app.
pub(crate) fn window_event(
    registry: &cua_teleport::ExportRegistry,
    e: ux::window::WindowDragEvent,
) -> TeleportWindowDragEvent {
    let app = e.window.as_ref().map(|w| {
        w.bundle_path
            .as_deref()
            .and_then(|p| ux::entry_for_path(registry, p, &Default::default()).ok())
            .unwrap_or_else(|| ux::entry_for_name(registry, &w.app_name, &Default::default()))
            .into()
    });
    TeleportWindowDragEvent {
        phase: match e.phase {
            ux::window::DragPhase::Start => "start",
            ux::window::DragPhase::Move => "move",
            ux::window::DragPhase::End => "end",
        }
        .into(),
        x: e.x,
        y: e.y,
        window: e.window.map(|w| TeleportWindow {
            window_id: w.window_id,
            pid: w.pid,
            app_name: w.app_name,
            title: w.title,
            bundle_path: w.bundle_path,
        }),
        app,
        start_frame: e.start_frame.map(rect),
        frame: e.frame.map(rect),
    }
}

fn rect(r: ux::window::Rect) -> crate::app_core_types::AppLogicalRect {
    crate::app_core_types::AppLogicalRect::new(r.x, r.y, r.width, r.height)
}

// ---- plan and run (need a Space)

pub use space_side::*;

mod space_side {
    use super::*;
    use cua_sdk::Space;
    use cua_spaces_ext::teleport_app::{SpaceAppTeleport as _, SpaceAppTeleportFacts as _};

    /// Options for [`Teleport::plan`].
    #[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
    pub struct TeleportPlanOptions {
        /// What moves.
        pub moves: TeleportMove,
        /// Host files or folders (`AppWithFiles`).
        #[uniffi(default = [])]
        pub files: Vec<String>,
        /// Provider manifest items (`AppWithState`); `None` is the
        /// provider's default selection.
        #[uniffi(default = None)]
        pub state_items: Option<Vec<String>>,
        /// With `state_items` `None`: also these opt-in groups of the
        /// entry's `sensitive_groups` (`SignIns` keeps the app signed in).
        /// Their items are secrets in the plan's consent: `run` needs
        /// `acknowledge_sensitive`.
        #[uniffi(default = [])]
        pub sensitive_groups: Vec<TeleportSensitiveGroup>,
        /// `tabs` or `full` (default) state.
        #[uniffi(default = None)]
        pub scope: Option<String>,
        /// Launch the app when done (default true).
        #[uniffi(default = None)]
        pub launch: Option<bool>,
    }

    /// What a consent item is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
    pub enum TeleportConsentKind {
        /// An install into the Space (nothing leaves this machine).
        Install,
        /// A host file that leaves this machine.
        File,
        /// A host folder that leaves this machine.
        Folder,
        /// App state that leaves this machine.
        State,
        /// A secret (cookies, tokens, logins) that leaves this machine.
        Secret,
    }

    /// One line of the consent screen.
    #[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
    pub struct TeleportConsentItem {
        pub kind: TeleportConsentKind,
        /// Installable id, host path, or manifest path.
        pub key: String,
        pub label: String,
        /// Version and checksum, destination, or count.
        pub detail: String,
        /// Bytes that leave this machine.
        pub bytes: u64,
        pub sensitive: bool,
    }

    /// One step of a plan.
    #[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
    pub struct TeleportPlanStep {
        /// `install`, `files`, `state` or `launch`.
        pub kind: String,
        /// Human summary.
        pub summary: String,
    }

    /// Exactly what a teleport will do and move.
    #[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
    pub struct TeleportPlan {
        pub app: TeleportCatalogEntry,
        pub space_id: String,
        pub moves: TeleportMove,
        pub steps: Vec<TeleportPlanStep>,
        /// Every install, path and secret.
        pub consent: Vec<TeleportConsentItem>,
        /// A consent item is a secret (needs `acknowledge_sensitive`).
        pub sensitive: bool,
        /// Bytes that leave this machine.
        pub total_bytes: u64,
        pub warnings: Vec<String>,
        /// This Space is reached through a relay connection that predates
        /// end-to-end sealing: a secret this plan sends would cross it in
        /// the clear (S1). Needs `acknowledge_relay_plaintext` the same way
        /// a secret needs `acknowledge_sensitive`.
        pub relay_unsealed: bool,
        /// The plan as JSON (what `run` reads back).
        pub json: String,
    }

    /// The user's answer to the consent screen.
    #[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
    pub struct TeleportConsent {
        /// Confirmed.
        pub approved: bool,
        /// Saw and accepted the secrets.
        #[uniffi(default = false)]
        pub acknowledge_sensitive: bool,
        /// "Save to Keyvault": keep the captured session sealed in the
        /// user's own Cua Keyvault after this delivery.
        #[uniffi(default = false)]
        pub save_to_keyvault: bool,
        /// Saw and accepted `TeleportPlan.relay_unsealed`'s warning (S1).
        #[uniffi(default = false)]
        pub acknowledge_relay_plaintext: bool,
        /// The review's per-site choice: the sites whose cookies to send
        /// (`None`: every cookie in the selection).
        #[uniffi(default = None)]
        pub cookie_domains: Option<Vec<String>>,
        /// Consent items (their keys) the user turned off.
        #[uniffi(default = [])]
        pub exclude: Vec<String>,
        /// Send these saved Keyvault items (ids) instead of reading the
        /// live app: nothing is captured from the host.
        #[uniffi(default = None)]
        pub from_vault: Option<Vec<String>>,
        /// Also send the saved passwords: only when ticked in the review.
        #[uniffi(default = false)]
        pub include_passwords: bool,
    }

    /// One progress event of [`Teleport::run`].
    #[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
    pub struct TeleportRunEvent {
        pub step: u32,
        pub steps: u32,
        /// `install`, `files`, `state`, `launch`, `done`.
        pub kind: String,
        /// `started`, `progress`, `finished`, `failed`, `done`.
        pub phase: String,
        pub detail: String,
        pub done_bytes: u64,
        pub total_bytes: u64,
    }

    /// Receives run progress (on an SDK worker thread).
    #[uniffi::export(with_foreign)]
    pub trait TeleportRunListener: Send + Sync {
        fn on_event(&self, event: TeleportRunEvent);
    }

    /// What a run did.
    #[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
    pub struct TeleportRunReport {
        pub app_id: String,
        pub installed: Vec<String>,
        /// Guest paths of sent files.
        pub sent: Vec<String>,
        pub imported: Vec<String>,
        pub skipped: Vec<String>,
        pub launched: bool,
    }

    fn summary(s: &ux::PlanStep) -> String {
        match s {
            ux::PlanStep::Install { ids } => {
                format!("Install {} (pinned, verified)", ids.join(", "))
            }
            ux::PlanStep::SendFiles { paths, subdir } => {
                format!("Send {} item(s) to ~/Downloads/{subdir}", paths.len())
            }
            ux::PlanStep::ImportState {
                provider_id, items, ..
            } => format!("Import {} {provider_id} item(s)", items.len()),
            ux::PlanStep::Launch { bin, files, .. } if files.is_empty() => format!("Open {bin}"),
            ux::PlanStep::Launch { bin, files, .. } => {
                format!("Open {bin} with {} item(s)", files.len())
            }
        }
    }

    impl From<ux::TeleportPlan> for TeleportPlan {
        fn from(p: ux::TeleportPlan) -> Self {
            let json = serde_json::to_string(&p).unwrap_or_default();
            Self {
                app: p.app.into(),
                space_id: p.space_id,
                moves: p.moves.into(),
                steps: p
                    .steps
                    .iter()
                    .map(|s| TeleportPlanStep {
                        kind: s.name().into(),
                        summary: summary(s),
                    })
                    .collect(),
                consent: p
                    .consent
                    .into_iter()
                    .map(|c| TeleportConsentItem {
                        kind: match c.kind {
                            ux::ConsentKind::Install => TeleportConsentKind::Install,
                            ux::ConsentKind::File => TeleportConsentKind::File,
                            ux::ConsentKind::Folder => TeleportConsentKind::Folder,
                            ux::ConsentKind::State => TeleportConsentKind::State,
                            ux::ConsentKind::Secret => TeleportConsentKind::Secret,
                        },
                        key: c.key,
                        label: c.label,
                        detail: c.detail,
                        bytes: c.bytes,
                        sensitive: c.sensitive,
                    })
                    .collect(),
                sensitive: p.sensitive,
                total_bytes: p.total_bytes,
                warnings: p.warnings,
                relay_unsealed: p.relay_unsealed,
                json,
            }
        }
    }

    impl From<ux::RunEvent> for TeleportRunEvent {
        fn from(e: ux::RunEvent) -> Self {
            Self {
                step: e.step,
                steps: e.steps,
                kind: e.kind,
                phase: serde_json::to_value(e.phase)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                detail: e.detail,
                done_bytes: e.done_bytes,
                total_bytes: e.total_bytes,
            }
        }
    }

    fn sessions(t: &Teleport) -> Arc<cua_spaces_ext::teleport::AppSessions> {
        Arc::new(cua_spaces_ext::teleport::AppSessions::with_host(
            t.host.clone(),
        ))
    }

    #[uniffi::export]
    impl Teleport {
        /// The Space's OS and CPU, to pass as `space_os` / `space_arch` in
        /// [`TeleportCatalogOptions`].
        pub async fn space_hint(&self, space: Arc<Space>) -> Result<TeleportCatalogOptions> {
            let s = space.handle().clone();
            cua_sdk::support::run(async move {
                // OS and CPU only: the home folder a plan needs is a guest
                // round trip the catalog does not.
                let f = s.app_teleport_hint().await?;
                Ok(TeleportCatalogOptions {
                    space_os: f.os.map(|os| {
                        match os {
                            cua_teleport::Platform::Linux => "linux",
                            cua_teleport::Platform::MacOS => "macos",
                            cua_teleport::Platform::Windows => "windows",
                        }
                        .into()
                    }),
                    space_arch: f.arch,
                    ..Default::default()
                })
            })
            .await
        }

        /// What teleporting `app` into `space` will install, send and import,
        /// with the consent items. Reads only file sizes and the provider's
        /// manifest.
        pub async fn plan(
            &self,
            app: TeleportCatalogEntry,
            space: Arc<Space>,
            options: TeleportPlanOptions,
        ) -> Result<TeleportPlan> {
            let entry = app.core()?;
            let sessions = sessions(self);
            let s = space.handle().clone();
            let o = ux::PlanOptions {
                moves: options.moves.into(),
                files: options.files,
                state_items: options.state_items,
                sensitive_groups: options
                    .sensitive_groups
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                scope: match options.scope.as_deref() {
                    None | Some("") | Some("full") => cua_teleport::TransferScope::FullProfile,
                    Some("tabs") => cua_teleport::TransferScope::TabsOnly,
                    Some(other) => {
                        return Err(CuaError::InvalidArgument(format!(
                            "scope must be tabs or full (got {other:?})"
                        )));
                    }
                },
                launch: options.launch.unwrap_or(true),
            };
            cua_sdk::support::run(async move {
                Ok(s.plan_app_teleport(sessions, &entry, &o).await?.into())
            })
            .await
        }

        /// Runs `plan` after the user's `consent` (secrets need
        /// `acknowledge_sensitive`), reporting progress to `listener`, and
        /// records the app in the recents.
        pub async fn run(
            &self,
            plan: TeleportPlan,
            space: Arc<Space>,
            consent: TeleportConsent,
            listener: Option<Arc<dyn TeleportRunListener>>,
        ) -> Result<TeleportRunReport> {
            let core: ux::TeleportPlan = serde_json::from_str(&plan.json)
                .map_err(|e| CuaError::InvalidArgument(format!("not a plan from this SDK: {e}")))?;
            let started = std::time::Instant::now();
            let (app_id, capability, move_kind) = (
                core.app.id.clone(),
                core.app.capability.as_str(),
                core.moves.as_str(),
            );
            let r = self.run_approved(core, space, consent, listener).await;
            cua_sdk::support::record_teleport(
                &app_id,
                capability,
                move_kind,
                started,
                &r,
                r.as_ref().map(|r| r.sent.len() as u64).unwrap_or(0),
            );
            r
        }
    }

    impl Teleport {
        async fn run_approved(
            &self,
            core: ux::TeleportPlan,
            space: Arc<Space>,
            consent: TeleportConsent,
            listener: Option<Arc<dyn TeleportRunListener>>,
        ) -> Result<TeleportRunReport> {
            let approved = core
                .approve(ux::Consent {
                    approved: consent.approved,
                    acknowledge_sensitive: consent.acknowledge_sensitive,
                    save_to_keyvault: consent.save_to_keyvault,
                    acknowledge_relay_plaintext: consent.acknowledge_relay_plaintext,
                    cookie_domains: consent.cookie_domains.clone(),
                    exclude: consent.exclude.clone(),
                    from_vault: consent.from_vault.clone(),
                    include_passwords: consent.include_passwords,
                })
                .map_err(crate::teleport_app::ux_err)?;
            let sessions = sessions(self);
            let s = space.handle().clone();
            let r = cua_sdk::support::run(async move {
                Ok(s.run_app_teleport(sessions, &approved, |e| {
                    if let Some(l) = &listener {
                        l.on_event(e.into());
                    }
                })
                .await?)
            })
            .await?;
            // Best effort: a read-only home must not fail a finished run.
            let _ = ux::record_recent(None, &r.app_id);
            Ok(TeleportRunReport {
                app_id: r.app_id,
                installed: r.installed,
                sent: r.sent,
                imported: r.imported,
                skipped: r.skipped,
                launched: r.launched,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> cua_teleport::ExportRegistry {
        cua_teleport::ExportRegistry::with_builtin_host(Arc::new(cua_teleport::FakeHost::new()))
    }

    /// A fixture drag (no real windows): the tracker's events become FFI
    /// events whose app is classified by name when the bundle is unknown.
    #[test]
    fn window_drag_events_carry_the_classified_app() {
        use ux::window::{MouseEvent, Rect, WindowDragTracker, WindowInfo};
        let w = |x: f64| WindowInfo {
            window_id: 7,
            pid: 70,
            owner: "Visual Studio Code".into(),
            title: "main.rs".into(),
            layer: 0,
            alpha: 1.0,
            bounds: Some(Rect {
                x,
                y: 0.0,
                width: 500.0,
                height: 400.0,
            }),
            visible: true,
            bundle_path: None,
        };
        let mut t = WindowDragTracker::new(vec![w(0.0)], 1);
        assert!(t.handle(MouseEvent::Down, 10.0, 10.0).is_none());
        *t.source_mut() = vec![w(50.0)];
        let start = t.handle(MouseEvent::Dragged, 60.0, 10.0).unwrap();
        let e = window_event(&registry(), start);
        assert_eq!(e.phase, "start");
        assert_eq!(e.window.as_ref().unwrap().window_id, 7);
        let app = e.app.unwrap();
        assert_eq!(
            (app.id.as_str(), app.capability),
            ("vscode", TeleportCapability::InstallOnly)
        );
        let mv = window_event(
            &registry(),
            t.handle(MouseEvent::Dragged, 70.0, 5.0).unwrap(),
        );
        assert_eq!((mv.phase.as_str(), mv.app.is_none()), ("move", true));
        let end = window_event(&registry(), t.handle(MouseEvent::Up, 70.0, 5.0).unwrap());
        assert_eq!(end.phase, "end");
    }

    #[test]
    fn plans_round_trip_through_json_and_consent_gates_secrets() {
        let entry = ux::entry_for_name(&registry(), "Visual Studio Code", &Default::default());
        let facts = ux::SpaceFacts {
            space_id: "space://direct/127.0.0.1:1".into(),
            os: cua_teleport::Platform::Linux,
            arch: "aarch64".into(),
            home: "/home/cua".into(),
            importers: vec![],
        };
        let core = ux::plan::build(
            &entry,
            &facts,
            &ux::PlanOptions::new(ux::MoveKind::AppOnly),
            None,
            &[],
        )
        .unwrap();
        let ffi: TeleportPlan = core.clone().into();
        assert_eq!(
            ffi.steps
                .iter()
                .map(|s| s.kind.as_str())
                .collect::<Vec<_>>(),
            ["install", "launch"]
        );
        assert_eq!(ffi.steps[0].summary, "Install vscode (pinned, verified)");
        assert_eq!(ffi.consent[0].kind, TeleportConsentKind::Install);
        let back: ux::TeleportPlan = serde_json::from_str(&ffi.json).unwrap();
        assert_eq!(back, core);
        assert!(back.approve(ux::Consent::default()).is_err());
        let entry_back = ffi.app.core().unwrap();
        assert_eq!(entry_back, entry);
    }
}
