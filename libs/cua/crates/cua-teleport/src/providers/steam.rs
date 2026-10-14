// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Steam session export.
//!
//! Steam is a native client (not Electron): its "stay logged in" state lives in
//! a few Valve KeyValues (`.vdf`) files — `config/config.vdf` (the account's
//! refresh/MachineAuth tokens), `config/loginusers.vdf` (the SteamID + Remember
//! Password flag), `registry.vdf` (AutoLoginUser) — plus any `ssfn*` sentry
//! files that mark a machine as Steam-Guard-trusted ([`layout::steam`]).
//! Copying these into a Space's Steam directory lets Steam auto-log-in.
//!
//! Caveat (surfaced, not hidden): Steam Guard may still re-challenge a genuinely
//! new machine that has no matching `ssfn` sentry, and the macOS client is x86,
//! so a Space needs Rosetta to run it.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bundle::{BundleWriter, DEFAULT_MAX_TOTAL_BYTES};
use crate::host::{HostEffects, default_host};
use crate::layout::steam::{
    APP_IDS as STEAM_APP_IDS, BUNDLE_PREFIX, LOGIN_FILES, SENTRY_PREFIX, root_for,
};
use crate::providers::util::{add_file_best_effort, file_len};
use crate::{
    AppRef, ExportProvider, ManifestItem, Platform, Result, TeleportError, TransferManifest,
    TransferScope,
};

pub struct SteamProvider {
    root_override: Option<PathBuf>,
    max_total_bytes: u64,
    /// Every `$HOME`, Keychain and authorization effect goes through this.
    host: Arc<dyn HostEffects>,
}

impl Default for SteamProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SteamProvider {
    pub fn new() -> Self {
        Self {
            root_override: None,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host: default_host(),
        }
    }

    /// Act on `host` instead of [`default_host`].
    pub fn with_host(mut self, host: Arc<dyn HostEffects>) -> Self {
        self.host = host;
        self
    }

    /// Override the source Steam root directory (tests).
    pub fn with_root(mut self, dir: impl Into<PathBuf>) -> Self {
        self.root_override = Some(dir.into());
        self
    }

    fn source_root(&self, platform: Platform) -> Option<PathBuf> {
        if let Some(dir) = &self.root_override {
            return Some(dir.clone());
        }
        Some(self.host.home_dir()?.join(root_for(platform)))
    }

    /// `ssfn*` sentry files in a Steam root (Steam-Guard machine trust).
    fn ssfn_files(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(SENTRY_PREFIX) && entry.path().is_file() {
                    out.push(name);
                }
            }
        }
        out
    }
}

impl ExportProvider for SteamProvider {
    fn id(&self) -> &str {
        "steam"
    }

    fn host(&self) -> &dyn HostEffects {
        &*self.host
    }

    fn display_name(&self) -> &str {
        "Steam"
    }

    fn platform_supported(&self, _platform: Platform) -> bool {
        true
    }

    fn install_probe(&self) -> Option<crate::InstallProbe> {
        Some(crate::InstallProbe::path(crate::layout::steam::MACOS_APP))
    }

    fn matches(&self, app: &AppRef) -> bool {
        STEAM_APP_IDS
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&app.app_id))
    }

    fn app_ids(&self) -> &[&str] {
        STEAM_APP_IDS
    }

    fn manifest(
        &self,
        app: &AppRef,
        _window: Option<&crate::WindowRef>,
        scope: TransferScope,
    ) -> Result<TransferManifest> {
        let root = self.source_root(app.platform);
        let mut items = Vec::new();
        let mut total = 0u64;
        if let Some(root) = &root {
            for rel in LOGIN_FILES {
                let disk = root.join(rel);
                if disk.is_file() {
                    let bytes = file_len(&disk);
                    total += bytes;
                    items.push(item(pretty(rel), &format!("{BUNDLE_PREFIX}/{rel}"), bytes));
                }
            }
            for name in Self::ssfn_files(root) {
                let bytes = file_len(&root.join(&name));
                total += bytes;
                items.push(item(
                    "Steam Guard machine token",
                    &format!("{BUNDLE_PREFIX}/{name}"),
                    bytes,
                ));
            }
        }
        Ok(TransferManifest {
            provider_id: "steam".to_string(),
            app_display_name: app.display_name.clone(),
            scope,
            items,
            total_est_bytes: total,
            notes: vec![
                "Carries the logged-in Steam account tokens. Steam Guard may still \
                 re-verify a new machine; the macOS client is x86 (needs Rosetta)."
                    .to_string(),
                "Close Steam before transfer for a clean capture.".to_string(),
            ],
        })
    }

    fn capture_selected(
        &self,
        app: &AppRef,
        _scope: TransferScope,
        include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> Result<()> {
        let root = self.source_root(app.platform).ok_or_else(|| {
            TeleportError::Provider("no Steam installation found on this machine".to_string())
        })?;
        let mut writer = BundleWriter::with_limit(
            out,
            "steam",
            app.display_name.clone(),
            _scope,
            self.max_total_bytes,
        );
        let wants = |r: &str| include.is_none_or(|set| set.contains(r));

        for rel in LOGIN_FILES {
            let bundle_rel = format!("{BUNDLE_PREFIX}/{rel}");
            if wants(&bundle_rel) {
                let disk = root.join(rel);
                if disk.is_file() {
                    add_file_best_effort(&mut writer, &disk, &bundle_rel)?;
                }
            }
        }
        for name in Self::ssfn_files(&root) {
            let bundle_rel = format!("{BUNDLE_PREFIX}/{name}");
            if wants(&bundle_rel) {
                add_file_best_effort(&mut writer, &root.join(&name), &bundle_rel)?;
            }
        }

        writer.finish()?;
        Ok(())
    }
}

fn pretty(rel: &str) -> &str {
    match rel {
        "config/config.vdf" => "Account tokens",
        "config/loginusers.vdf" => "Remembered login",
        "registry.vdf" => "Auto-login setting",
        other => other,
    }
}

fn item(label: &str, rel_path: &str, est_bytes: u64) -> ManifestItem {
    ManifestItem {
        label: label.to_string(),
        rel_path: rel_path.to_string(),
        est_bytes,
        count: None,
        count_noun: None,
        sensitive: true,
        default_checked: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleReader;
    use std::io::Cursor;

    fn app() -> AppRef {
        AppRef {
            app_id: "com.valvesoftware.steam".into(),
            display_name: "Steam".into(),
            platform: Platform::MacOS,
        }
    }

    fn fake_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(
            dir.path().join("config/config.vdf"),
            b"\"InstallConfigStore\"{}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("config/loginusers.vdf"),
            b"\"users\"{\"7656119\"{}}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("registry.vdf"),
            b"\"Registry\"{\"AutoLoginUser\"\"me\"}",
        )
        .unwrap();
        std::fs::write(dir.path().join("ssfn1234567890"), b"sentry").unwrap();
        dir
    }

    #[test]
    fn matches_steam_ids() {
        let p = SteamProvider::new();
        assert!(p.matches(&app()));
        assert!(p.matches(&AppRef {
            app_id: "steam".into(),
            display_name: "s".into(),
            platform: Platform::MacOS
        }));
        assert!(!p.matches(&AppRef {
            app_id: "com.hnc.Discord".into(),
            display_name: "d".into(),
            platform: Platform::MacOS
        }));
    }

    #[test]
    fn manifest_lists_tokens_and_ssfn() {
        let root = fake_root();
        let p = SteamProvider::new().with_root(root.path());
        let m = p
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();
        let labels: Vec<&str> = m.items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Account tokens"));
        assert!(labels.contains(&"Remembered login"));
        assert!(labels.contains(&"Steam Guard machine token"));
        assert!(m.items.iter().all(|i| i.sensitive));
    }

    #[test]
    fn full_export_packs_login_files_and_sentries() {
        let root = fake_root();
        let p = SteamProvider::new().with_root(root.path());
        let mut bundle = Vec::new();
        p.export(&app(), TransferScope::FullProfile, &mut bundle)
            .unwrap();
        let reader = BundleReader::open(Cursor::new(bundle)).unwrap();
        assert_eq!(reader.header().provider_id, "steam");
        let mut paths: Vec<String> = reader
            .read_all()
            .unwrap()
            .into_iter()
            .map(|e| e.rel_path)
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "steam/config/config.vdf",
                "steam/config/loginusers.vdf",
                "steam/registry.vdf",
                "steam/ssfn1234567890"
            ]
        );
    }

    #[test]
    fn selected_export_limits_to_checked() {
        let root = fake_root();
        let p = SteamProvider::new().with_root(root.path());
        let include: HashSet<String> = ["steam/registry.vdf".to_string()].into_iter().collect();
        let mut bundle = Vec::new();
        p.export_selected(
            &app(),
            TransferScope::FullProfile,
            Some(&include),
            &mut bundle,
        )
        .unwrap();
        let paths: Vec<String> = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(paths, vec!["steam/registry.vdf".to_string()]);
    }
}
