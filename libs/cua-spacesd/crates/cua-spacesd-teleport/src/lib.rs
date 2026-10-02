// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teleport RECEIVE: the sandbox half of moving a desktop app session.
//!
//! cua-spacesd runs inside the sandbox and only ever *receives*: the
//! sender (`cua-teleport` in the cua SDK) captures a session on the user's
//! machine and uploads it through `cua.env.v1.TeleportService`. This crate
//! imports what arrives:
//!
//! - [`ImportProvider`] is the per-application importer: it verifies a
//!   [`bundle::SessionBundle`], materializes it under the destination home
//!   (remapped to this platform's layout), installs carried Keychain items
//!   ([`keychain`]), stops a running instance first where that matters, and
//!   returns the [`LaunchSpec`] that relaunches the app.
//! - [`ImportRegistry`] resolves an importer by the bundle's `provider_id`.
//! - [`Receiver`] is what cua-spacesd-server's `TeleportService` drives: import a
//!   staged bundle and launch the app through the injected host. It also
//!   carries the file-transfer path rules ([`validate_relative_path`],
//!   [`conflict_free_path`], [`IgnoreRules`]).
//!
//! The format and the per-app layouts come from the effect-free
//! [`cua_teleport_bundle`] shared with the sender; this crate never depends
//! on the sender. Every process, Keychain and `$HOME` effect goes through
//! [`HostEffects`] ([`RealHost`] refuses under `cfg(test)` or
//! `CUA_ENV_TEST_SANDBOX=1`; tests inject [`FakeHost`]).

pub mod cookies;
pub mod host;
pub mod importers;
pub mod keychain;
pub mod ledger;
pub mod logins;
mod receiver;

pub use cua_teleport_bundle::{
    bundle, layout, AppRef, LaunchSpec, Platform, Result, TeleportError, TransferScope,
    WindowRestore, BUNDLE_VERSION,
};
pub use host::{
    default_host, EffectKind, FakeHost, HostCommand, HostEffects, HostOutput, RealHost, StdinBytes,
};
pub use ledger::{ImportRecord, KeychainRef, Ledger, LedgerStore, WipeReport};
pub use receiver::{
    conflict_free_path, validate_relative_path, IgnoreRules, ImportError, ImportOutcome, Receiver,
};

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

/// One application's session importer (receiver side).
pub trait ImportProvider: Send + Sync {
    /// Provider id; bundles whose header names it are imported by it.
    fn id(&self) -> &str;

    /// Human-readable name.
    fn display_name(&self) -> &str;

    /// Materialize a bundle into `dest_home` for an explicit destination
    /// `platform` (so every platform's layout is testable on any host) and
    /// describe how to relaunch the app there. Entries are checksum-verified
    /// as they are read; a bad entry aborts before it is written.
    ///
    /// Every file written, directory created and Keychain item installed is
    /// reported in `record` as it happens (also on failure, so a partial
    /// import can be undone). That is what the import ledger and
    /// `WipeImport` act on.
    fn import_recorded(
        &self,
        bundle: &mut dyn Read,
        dest_home: &Path,
        platform: Platform,
        record: &mut ImportRecord,
    ) -> Result<LaunchSpec>;

    /// [`Self::import_recorded`] without keeping the record.
    fn import_to(
        &self,
        bundle: &mut dyn Read,
        dest_home: &Path,
        platform: Platform,
    ) -> Result<LaunchSpec> {
        self.import_recorded(bundle, dest_home, platform, &mut ImportRecord::default())
    }

    /// [`Self::import_to`] for the platform this code runs on.
    fn import(&self, bundle: &mut dyn Read, dest_home: &Path) -> Result<LaunchSpec> {
        self.import_to(bundle, dest_home, Platform::current())
    }
}

/// Holds the registered [`ImportProvider`]s.
#[derive(Default)]
pub struct ImportRegistry {
    importers: Vec<Box<dyn ImportProvider>>,
}

impl ImportRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every built-in importer (the shared layout table's ids), acting on
    /// `host`.
    pub fn with_builtin_host(host: Arc<dyn HostEffects>) -> Self {
        use importers::{
            chrome::ChromeImporter, claude_code::ClaudeCodeImporter, electron::ElectronImporter,
            firefox::FirefoxImporter, steam::SteamImporter, whatsapp::WhatsAppImporter,
        };
        let mut registry = Self::new();
        registry.register(Box::new(ChromeImporter::new().with_host(host.clone())));
        registry.register(Box::new(FirefoxImporter::new()));
        registry.register(Box::new(ElectronImporter::slack().with_host(host.clone())));
        registry.register(Box::new(
            ElectronImporter::discord().with_host(host.clone()),
        ));
        registry.register(Box::new(ElectronImporter::unity_hub().with_host(host)));
        registry.register(Box::new(SteamImporter::new()));
        registry.register(Box::new(WhatsAppImporter::new()));
        registry.register(Box::new(ClaudeCodeImporter::new()));
        registry
    }

    /// Add an importer.
    pub fn register(&mut self, importer: Box<dyn ImportProvider>) {
        self.importers.push(importer);
    }

    /// All registered importers.
    pub fn importers(&self) -> &[Box<dyn ImportProvider>] {
        &self.importers
    }

    /// The importer with the given provider id, if any.
    pub fn find_by_id(&self, provider_id: &str) -> Option<&dyn ImportProvider> {
        self.importers
            .iter()
            .find(|importer| importer.id() == provider_id)
            .map(|importer| importer.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The receiver imports exactly the providers the sender exports: both
    /// register the ids of the shared layout table, in its order.
    #[test]
    fn builtin_ids_match_the_shared_layout() {
        let registry = ImportRegistry::with_builtin_host(default_host());
        let ids: Vec<&str> = registry.importers().iter().map(|i| i.id()).collect();
        assert_eq!(ids, layout::PROVIDER_IDS);
        assert!(registry.find_by_id("unity-hub").is_some());
        assert!(registry.find_by_id("nope").is_none());
    }

    /// cua-spacesd is server-only: nothing in its workspace may depend on
    /// the SDK sender (`cua-teleport`) or on the old combined crates.
    #[test]
    fn driver_workspace_never_links_the_sender() {
        let lock = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
        let lock = std::fs::read_to_string(lock).unwrap();
        for forbidden in [
            "name = \"cua-teleport\"",
            "name = \"cua-env-app-session\"",
            "name = \"cua-env-handoff\"",
            "name = \"cua-env-cli\"",
        ] {
            assert!(
                !lock.contains(forbidden),
                "{forbidden} in the driver lockfile"
            );
        }
        assert!(lock.contains("name = \"cua-teleport-bundle\""));
    }
}
