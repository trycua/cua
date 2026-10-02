// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! WhatsApp (macOS native, sandboxed) session export.
//!
//! WhatsApp Desktop is a multi-device client: after linking from the phone it
//! keeps the linked-device identity (Signal-protocol keys) and message store in
//! its **App Group containers**, not the Keychain. The bulk lives in
//! `group.net.whatsapp.WhatsApp.shared`, with a little companion state in the
//! `.private` group and the app's sandbox container. Copying those into a
//! Space's containers carries the linked session so WhatsApp opens to the chats
//! signed in.
//!
//! Caveat (surfaced, not hidden): the linked device is cryptographically bound
//! to the phone's registration; WhatsApp may still re-validate on a new machine
//! and require re-linking via QR. macOS-only — there is no Linux client.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use crate::bundle::{BundleWriter, DEFAULT_MAX_TOTAL_BYTES};
use crate::host::{HostEffects, default_host};
use crate::layout::whatsapp::{APP_IDS as WHATSAPP_APP_IDS, ROOTS};
use crate::providers::util::{add_dir_recursive, dir_len};
use crate::{
    AppRef, ExportProvider, ManifestItem, Platform, Result, TeleportError, TransferManifest,
    TransferScope,
};

pub struct WhatsAppProvider {
    home_override: Option<PathBuf>,
    max_total_bytes: u64,
    /// Every `$HOME`, Keychain and authorization effect goes through this.
    host: Arc<dyn HostEffects>,
}

impl Default for WhatsAppProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WhatsAppProvider {
    pub fn new() -> Self {
        Self {
            home_override: None,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host: default_host(),
        }
    }

    /// Act on `host` instead of [`default_host`].
    pub fn with_host(mut self, host: Arc<dyn HostEffects>) -> Self {
        self.host = host;
        self
    }

    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home_override = Some(home.into());
        self
    }

    fn home(&self) -> Option<PathBuf> {
        self.home_override.clone().or_else(|| self.host.home_dir())
    }
}

impl ExportProvider for WhatsAppProvider {
    fn id(&self) -> &str {
        "whatsapp"
    }

    fn host(&self) -> &dyn HostEffects {
        &*self.host
    }

    fn display_name(&self) -> &str {
        "WhatsApp"
    }

    fn platform_supported(&self, platform: Platform) -> bool {
        platform == Platform::MacOS
    }

    fn install_probe(&self) -> Option<crate::InstallProbe> {
        Some(crate::InstallProbe::path(
            crate::layout::whatsapp::MACOS_APP,
        ))
    }

    fn matches(&self, app: &AppRef) -> bool {
        WHATSAPP_APP_IDS
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&app.app_id))
    }

    fn app_ids(&self) -> &[&str] {
        WHATSAPP_APP_IDS
    }

    fn manifest(
        &self,
        app: &AppRef,
        _window: Option<&crate::WindowRef>,
        scope: TransferScope,
    ) -> Result<TransferManifest> {
        let home = self.home();
        let mut items = Vec::new();
        let mut total = 0u64;
        if let Some(home) = &home {
            for (prefix, rel) in ROOTS {
                let disk = home.join(rel);
                if disk.is_dir() {
                    let bytes = dir_len(&disk);
                    total += bytes;
                    items.push(ManifestItem {
                        label: label_for(prefix).to_string(),
                        rel_path: format!("{prefix}/"),
                        est_bytes: bytes,
                        count: None,
                        count_noun: None,
                        sensitive: true,
                        default_checked: true,
                    });
                }
            }
        }
        Ok(TransferManifest {
            provider_id: "whatsapp".to_string(),
            app_display_name: app.display_name.clone(),
            scope,
            items,
            total_est_bytes: total,
            notes: vec![
                "Carries the linked-device session and chat store. WhatsApp may \
                 re-validate on a new machine and ask to re-link via QR."
                    .to_string(),
                "Close WhatsApp before transfer for a clean capture.".to_string(),
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
        if !self.platform_supported(app.platform) {
            return Err(TeleportError::Provider(
                "WhatsApp session transfer is macOS-only".to_string(),
            ));
        }
        let home = self.home().ok_or_else(|| {
            TeleportError::Provider("no home directory to read WhatsApp from".to_string())
        })?;
        let mut writer = BundleWriter::with_limit(
            out,
            "whatsapp",
            app.display_name.clone(),
            _scope,
            self.max_total_bytes,
        );
        let wants = |r: &str| include.is_none_or(|set| set.contains(r));
        for (prefix, rel) in ROOTS {
            if wants(&format!("{prefix}/")) {
                let disk = home.join(rel);
                if disk.is_dir() {
                    add_dir_recursive(&mut writer, &disk, prefix)?;
                }
            }
        }
        writer.finish()?;
        Ok(())
    }
}

fn label_for(prefix: &str) -> &str {
    match prefix {
        "group-shared" => "Linked session and chats",
        "group-private" => "Companion state",
        "container" => "App state",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleReader;
    use std::io::Cursor;

    fn app() -> AppRef {
        AppRef {
            app_id: "net.whatsapp.WhatsApp".into(),
            display_name: "WhatsApp".into(),
            platform: Platform::MacOS,
        }
    }

    fn fake_home() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir
            .path()
            .join("Library/Group Containers/group.net.whatsapp.WhatsApp.shared");
        std::fs::create_dir_all(shared.join("ChatStorage")).unwrap();
        std::fs::write(shared.join("ChatStorage/db.sqlite"), b"session+chats").unwrap();
        let private = dir
            .path()
            .join("Library/Group Containers/group.net.whatsapp.WhatsApp.private");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::write(private.join("state.plist"), b"companion").unwrap();
        let container = dir
            .path()
            .join("Library/Containers/net.whatsapp.WhatsApp/Data/Library/Application Support/net.whatsapp.WhatsApp");
        std::fs::create_dir_all(&container).unwrap();
        std::fs::write(container.join("app.state"), b"appstate").unwrap();
        dir
    }

    #[test]
    fn matches_ids_and_macos_only() {
        let p = WhatsAppProvider::new();
        assert!(p.matches(&app()));
        assert!(p.platform_supported(Platform::MacOS));
        assert!(!p.platform_supported(Platform::Linux));
    }

    #[test]
    fn full_export_packs_every_root_under_its_prefix() {
        let home = fake_home();
        let p = WhatsAppProvider::new().with_home(home.path());
        let mut bundle = Vec::new();
        p.export(&app(), TransferScope::FullProfile, &mut bundle)
            .unwrap();
        let reader = BundleReader::open(Cursor::new(bundle)).unwrap();
        assert_eq!(reader.header().provider_id, "whatsapp");
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
                "container/app.state",
                "group-private/state.plist",
                "group-shared/ChatStorage/db.sqlite"
            ]
        );
    }

    #[test]
    fn linux_export_is_refused() {
        let home = fake_home();
        let p = WhatsAppProvider::new().with_home(home.path());
        let mut linux = app();
        linux.platform = Platform::Linux;
        let mut bundle = Vec::new();
        assert!(
            p.capture_selected(&linux, TransferScope::FullProfile, None, &mut bundle)
                .is_err()
        );
        assert!(bundle.is_empty());
    }

    #[test]
    fn manifest_lists_session_root() {
        let home = fake_home();
        let p = WhatsAppProvider::new().with_home(home.path());
        let m = p
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();
        assert!(
            m.items
                .iter()
                .any(|i| i.label == "Linked session and chats")
        );
        assert!(m.items.iter().all(|i| i.sensitive));
    }
}
