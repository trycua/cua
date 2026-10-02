// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The teleport UX core every client shares: "Teleport an app…" and
//! drag-and-drop onto a Space.
//!
//! - [`apps`]: installed host apps (macOS bundles, Linux `.desktop`
//!   entries, Windows Start Menu shortcuts);
//! - [`catalog`]: each app classified against the install manifest and the
//!   provider registry ([`catalog::Capability`]), with search and recents;
//! - [`plan`]: what a teleport will install, send and import, and the
//!   consent items that list every path and secret;
//! - [`drop`]: drag payloads (app bundles, files, URLs);
//! - [`window`]: dragging a real app window onto a Space, and its preview.
//!
//! Running a plan needs a Space: `cua_spaces::Space::run_app_teleport`.
//! Every real-machine read (app roots, windows, thumbnails, the event tap)
//! refuses under `cfg(test)` or `CUA_ENV_TEST_SANDBOX=1`; tests use
//! fixture directories and fixture window lists.

pub mod apps;
pub mod catalog;
pub mod drop;
mod icon;
pub mod plan;
pub mod recents;
pub mod run;
#[cfg(test)]
pub(crate) mod testing;
pub mod window;
#[cfg(target_os = "macos")]
mod window_macos;

use std::collections::BTreeMap;
use std::path::PathBuf;

pub use apps::HostApp;
pub use catalog::{
    Capability, CatalogEntry, InstallSource, Launch, MoveKind, ProviderView, SensitiveGroup,
    TargetHint,
};
pub use drop::{DropKind, DropPayload};
pub use icon::{app_icon_png, app_icon_png_cached, capture_thumbnail_png_cached, host_icon_key};
pub use plan::{
    ApprovedPlan, Consent, ConsentItem, ConsentKind, FileStat, PlanOptions, PlanStep, SpaceFacts,
    TeleportPlan,
};
pub use run::{RunEvent, RunPhase, RunReport};

use crate::{ExportRegistry, Platform};

/// Why a UX call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UxError {
    /// Not possible for this app, Space or platform (the message says why).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A bad argument.
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// Consent was not given, or secrets were not acknowledged.
    #[error("not approved: {0}")]
    NotApproved(String),
    /// An OS permission is missing.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Real-machine effects are refused (tests).
    #[error("host effects refused: {0}")]
    HostEffectsRefused(String),
    /// I/O.
    #[error("io: {0}")]
    Io(String),
}

/// What [`catalog`] reads.
#[derive(Clone, Debug, Default)]
pub struct CatalogRequest {
    /// App roots; `None` is [`apps::default_roots`] (the real machine).
    pub roots: Option<Vec<PathBuf>>,
    /// The Space, when known.
    pub hint: TargetHint,
    /// Recents file; `None` is [`recents::default_path`].
    pub recents_path: Option<PathBuf>,
    /// Check whether provider CLIs are installed on this machine (their
    /// install probes). Off for fixture roots.
    pub probe_providers: bool,
}

/// "Teleport an app…": the classified catalog for this machine.
pub fn catalog(
    registry: &ExportRegistry,
    req: &CatalogRequest,
) -> Result<Vec<CatalogEntry>, UxError> {
    let host = Platform::current();
    let roots = match &req.roots {
        Some(r) => r.clone(),
        None => {
            apps::default_roots(host).map_err(|e| UxError::HostEffectsRefused(e.to_string()))?
        }
    };
    let found = apps::enumerate_cached(&roots, host);
    let providers = provider_views(registry, req.probe_providers);
    let recents = req
        .recents_path
        .clone()
        .or_else(recents::default_path)
        .map(|p| recents::load(&p))
        .unwrap_or_default();
    Ok(catalog::classify_all(
        &found, &providers, host, &req.hint, &recents,
    ))
}

/// Warms what the picker shows first, on the calling thread: the app list
/// ([`apps::enumerate_cached`]) and every app's icon (the SDK's icon cache).
/// Clients run it in the background when they start, so the first picker
/// opens on cached apps and icons. `roots` as in [`CatalogRequest`]; does
/// nothing where the real machine is refused (tests).
pub fn prefetch(roots: Option<Vec<PathBuf>>) {
    let host = Platform::current();
    let roots = match roots {
        Some(r) => r,
        None => match apps::default_roots(host) {
            Ok(r) => r,
            Err(_) => return,
        },
    };
    for app in apps::enumerate_cached(&roots, host) {
        if let Some(path) = app.path.to_str() {
            let _ = app_icon_png_cached(path, cua_icon_cache::SIZE_2X);
        }
    }
}

/// The registry's providers as the catalog sees them.
pub fn provider_views(registry: &ExportRegistry, probe: bool) -> Vec<ProviderView> {
    registry
        .infos()
        .into_iter()
        .map(|info| {
            let installed_here = probe
                && !crate::host::host_effects_forbidden()
                && info.install_probe.as_ref().is_some_and(crate::is_installed);
            ProviderView {
                info,
                installed_here,
            }
        })
        .collect()
}

/// The catalog entry for one dropped or dragged app (a bundle, `.desktop`
/// entry or shortcut path).
pub fn entry_for_path(
    registry: &ExportRegistry,
    path: &str,
    hint: &TargetHint,
) -> Result<CatalogEntry, UxError> {
    let app = drop::read_dropped_app(path).ok_or_else(|| {
        UxError::Invalid(format!(
            "{path} is not an app bundle, .desktop entry or shortcut"
        ))
    })?;
    Ok(catalog::classify(
        &app,
        &provider_views(registry, false),
        Platform::current(),
        hint,
    ))
}

/// The catalog entry for an app known only by name or id (a dragged window
/// whose bundle is unknown, a provider id).
pub fn entry_for_name(registry: &ExportRegistry, name: &str, hint: &TargetHint) -> CatalogEntry {
    let app = HostApp {
        name: name.to_string(),
        app_id: Some(name.to_string()),
        path: Default::default(),
        version: None,
        icon: None,
        platform: Platform::current(),
    };
    catalog::classify(
        &app,
        &provider_views(registry, false),
        Platform::current(),
        hint,
    )
}

/// Records a teleported app in the recents file.
pub fn record_recent(path: Option<PathBuf>, id: &str) -> Result<BTreeMap<String, u64>, UxError> {
    let p = path
        .or_else(recents::default_path)
        .ok_or_else(|| UxError::Invalid("no recents path (HOME is unset)".into()))?;
    recents::record(&p, id, recents::now_ms()).map_err(|e| UxError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn catalog_over_fixture_roots_with_recents() {
        let root = tempfile::tempdir().unwrap();
        // The host's own app format: the catalog enumerates what this OS lists.
        testing::fixture_host_app(
            root.path(),
            "Visual Studio Code",
            "com.microsoft.VSCode",
            "code",
        );
        testing::fixture_host_app(
            root.path(),
            "Fixture Paint",
            "com.example.paint",
            "com.example.paint",
        );
        let recents = root.path().join("recents.json");
        let reg = ExportRegistry::with_builtin_host(Arc::new(crate::FakeHost::new()));
        let req = CatalogRequest {
            roots: Some(vec![root.path().to_path_buf()]),
            recents_path: Some(recents.clone()),
            ..Default::default()
        };
        let before = catalog(&reg, &req).unwrap();
        assert_eq!(before.len(), 2, "{before:?}");
        let paint = before
            .iter()
            .find(|e| e.capability == Capability::Unsupported)
            .expect("the unknown app is listed")
            .id
            .clone();
        record_recent(Some(recents), &paint).unwrap();
        let all = catalog(&reg, &req).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, paint, "the recent app sorts first");
        assert!(all[0].last_used_ms.is_some());
        assert_eq!(all[0].capability, Capability::Unsupported);
        assert_eq!(all[1].capability, Capability::InstallOnly);
        // The real machine's roots are refused in tests.
        assert!(matches!(
            catalog(&reg, &CatalogRequest::default()),
            Err(UxError::HostEffectsRefused(_))
        ));
    }

    #[test]
    fn entries_for_dropped_paths_and_window_names() {
        let root = tempfile::tempdir().unwrap();
        let b = testing::fixture_app(
            root.path(),
            "Firefox",
            "org.mozilla.firefox",
            "Firefox",
            false,
        );
        let reg = ExportRegistry::with_builtin_host(Arc::new(crate::FakeHost::new()));
        let e = entry_for_path(&reg, b.to_str().unwrap(), &TargetHint::default()).unwrap();
        assert_eq!((e.id.as_str(), e.capability), ("firefox", Capability::Full));
        assert!(entry_for_path(&reg, "/tmp/x.txt", &TargetHint::default()).is_err());
        let e = entry_for_name(&reg, "Visual Studio Code", &TargetHint::default());
        assert_eq!(e.capability, Capability::InstallOnly);
    }
}
