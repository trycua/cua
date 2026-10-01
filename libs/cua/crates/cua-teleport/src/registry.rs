// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::sync::Arc;

use serde::Serialize;

use crate::ExportProvider;
use crate::host::HostEffects;
use crate::{AppRef, InstallProbe, Platform, Result, TeleportError};

/// Holds the registered [`ExportProvider`]s and resolves the right one for an
/// application.
#[derive(Default)]
pub struct ExportRegistry {
    providers: Vec<Box<dyn ExportProvider>>,
}

/// A provider's static description, as `cua teleport providers` prints it for
/// consent UIs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderInfo {
    /// Provider id (also the receiver's importer id).
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Bundle ids and app names the provider matches.
    pub app_ids: Vec<String>,
    /// Whether it can export on macOS.
    pub macos: bool,
    /// Whether it can export on Linux.
    pub linux: bool,
    /// Whether it can export on Windows.
    pub windows: bool,
    /// Host-side install check (`{probe, on_path}`), when the app has one.
    pub install_probe: Option<InstallProbe>,
}

impl ExportRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry with every built-in provider registered, acting on
    /// [`crate::host::default_host`] (the real machine outside this crate's
    /// tests).
    pub fn with_builtin() -> Self {
        Self::with_builtin_host(crate::host::default_host())
    }

    /// A registry with every built-in provider registered, all acting on
    /// `host`. Tests pass a [`crate::host::FakeHost`] with a temporary home.
    pub fn with_builtin_host(host: Arc<dyn HostEffects>) -> Self {
        Self::builtin(host, None)
    }

    /// Like [`Self::with_builtin_host`], capturing the named (or path-given)
    /// Chrome profile instead of `Default`.
    pub fn with_builtin_host_and_chrome_profile(
        host: Arc<dyn HostEffects>,
        chrome_profile: Option<String>,
    ) -> Self {
        Self::builtin(host, chrome_profile)
    }

    fn builtin(host: Arc<dyn HostEffects>, chrome_profile: Option<String>) -> Self {
        use crate::providers::{
            chrome::ChromeProvider, claude_code::ClaudeCodeProvider, electron::ElectronProvider,
            firefox::FirefoxProvider, steam::SteamProvider, whatsapp::WhatsAppProvider,
        };
        let mut chrome = ChromeProvider::new().with_host(host.clone());
        if let Some(profile) = chrome_profile {
            chrome = chrome.with_profile(profile);
        }
        let mut registry = Self::new();
        registry.register(Box::new(chrome));
        registry.register(Box::new(FirefoxProvider::new().with_host(host.clone())));
        registry.register(Box::new(ElectronProvider::slack().with_host(host.clone())));
        registry.register(Box::new(
            ElectronProvider::discord().with_host(host.clone()),
        ));
        registry.register(Box::new(
            ElectronProvider::unity_hub().with_host(host.clone()),
        ));
        registry.register(Box::new(SteamProvider::new().with_host(host.clone())));
        registry.register(Box::new(WhatsAppProvider::new().with_host(host.clone())));
        registry.register(Box::new(ClaudeCodeProvider::new().with_host(host)));
        registry
    }

    /// Add a provider. Later registrations lose to earlier ones on `matches`.
    pub fn register(&mut self, provider: Box<dyn ExportProvider>) {
        self.providers.push(provider);
    }

    /// All registered providers.
    pub fn providers(&self) -> &[Box<dyn ExportProvider>] {
        &self.providers
    }

    /// Static descriptions of every registered provider.
    pub fn infos(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|p| ProviderInfo {
                id: p.id().to_string(),
                display_name: p.display_name().to_string(),
                app_ids: p.app_ids().iter().map(|s| s.to_string()).collect(),
                macos: p.platform_supported(Platform::MacOS),
                linux: p.platform_supported(Platform::Linux),
                windows: p.platform_supported(Platform::Windows),
                install_probe: p.install_probe(),
            })
            .collect()
    }

    /// The first provider whose `matches` accepts the app, if any.
    pub fn find_for_app(&self, app: &AppRef) -> Option<&dyn ExportProvider> {
        self.providers
            .iter()
            .find(|provider| provider.matches(app))
            .map(|provider| provider.as_ref())
    }

    /// The first provider whose `matches` accepts the app, or an error.
    pub fn resolve_for_app(&self, app: &AppRef) -> Result<&dyn ExportProvider> {
        self.find_for_app(app)
            .ok_or_else(|| TeleportError::NoProviderForApp {
                app_id: app.app_id.clone(),
            })
    }

    /// The provider with the given id, if any.
    pub fn find_by_id(&self, provider_id: &str) -> Option<&dyn ExportProvider> {
        self.providers
            .iter()
            .find(|provider| provider.id() == provider_id)
            .map(|provider| provider.as_ref())
    }

    /// The provider with the given id, or an error.
    pub fn resolve_by_id(&self, provider_id: &str) -> Result<&dyn ExportProvider> {
        self.find_by_id(provider_id)
            .ok_or_else(|| TeleportError::UnknownProvider {
                provider_id: provider_id.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, platform: Platform) -> AppRef {
        AppRef {
            app_id: id.into(),
            display_name: id.into(),
            platform,
        }
    }

    #[test]
    fn resolves_chrome_and_claude_code_by_app_and_id() {
        let registry = ExportRegistry::with_builtin();
        let provider = registry
            .resolve_for_app(&app("com.google.Chrome", Platform::MacOS))
            .unwrap();
        assert_eq!(provider.id(), "chrome");
        assert!(registry.find_by_id("chrome").is_some());
        let provider = registry
            .resolve_for_app(&app("claude-code", Platform::MacOS))
            .unwrap();
        assert_eq!(provider.id(), "claude-code");
    }

    #[test]
    fn unknown_app_and_id_error() {
        let registry = ExportRegistry::with_builtin();
        assert!(matches!(
            registry
                .resolve_for_app(&app("com.example.Unknown", Platform::Linux))
                .map(|_| ()),
            Err(TeleportError::NoProviderForApp { .. })
        ));
        assert!(matches!(
            registry.resolve_by_id("nope").map(|_| ()),
            Err(TeleportError::UnknownProvider { .. })
        ));
    }

    /// The sender exports exactly the providers the receiver imports: both
    /// sides register the ids of the shared layout table, in its order.
    #[test]
    fn builtin_ids_match_the_shared_layout() {
        let registry = ExportRegistry::with_builtin();
        let ids: Vec<&str> = registry.providers().iter().map(|p| p.id()).collect();
        assert_eq!(ids, crate::layout::PROVIDER_IDS);
        let infos = registry.infos();
        let slack = infos.iter().find(|i| i.id == "slack").unwrap();
        assert!(
            slack
                .app_ids
                .contains(&"com.tinyspeck.slackmacgap".to_string())
        );
        assert_eq!(
            slack.install_probe,
            Some(InstallProbe::path("/Applications/Slack.app"))
        );
    }
}
