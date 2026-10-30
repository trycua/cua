//! Linux adapters for encrypted Computer History.

use cua_driver_core::history::{ApplicationIdentity, ApplicationIdentityProvider};
pub use cua_driver_core::history_keyring::KeyringKeyProvider as LinuxSecretServiceKeyProvider;
use std::sync::Arc;

#[derive(Default)]
pub struct LinuxApplicationIdentityProvider;

#[derive(Clone, Copy)]
enum WindowAppNameProvenance {
    StableAppId,
    Ambiguous,
}

fn stable_window_app_name(
    app_name: Option<String>,
    provenance: WindowAppNameProvenance,
) -> Option<String> {
    match provenance {
        WindowAppNameProvenance::StableAppId => app_name.filter(|name| !name.trim().is_empty()),
        WindowAppNameProvenance::Ambiguous => None,
    }
}

fn application_identity_from_stable_sources(
    process_name: String,
    platform_id: Option<String>,
) -> Option<ApplicationIdentity> {
    let process_name = (!process_name.trim().is_empty()).then_some(process_name);
    let bundle_id = platform_id.or_else(|| process_name.clone());
    (bundle_id.is_some() || process_name.is_some()).then_some(ApplicationIdentity {
        bundle_id,
        display_name: process_name,
    })
}

impl ApplicationIdentityProvider for LinuxApplicationIdentityProvider {
    fn resolve(&self, pid: i64, _window_id: Option<u64>) -> Option<ApplicationIdentity> {
        let pid = u32::try_from(pid).ok()?;
        let process = crate::proc_fs::list_processes()
            .into_iter()
            .find(|process| process.pid == pid)?;
        let platform_id = if crate::wayland::is_wayland() {
            // Wayland's shared WindowInfo cannot distinguish a native app ID
            // from the shell-helper fallback that copies the window title.
            // Do not enumerate or persist that ambiguous value in history.
            stable_window_app_name(None, WindowAppNameProvenance::Ambiguous)
        } else {
            stable_window_app_name(
                crate::x11::list_windows(Some(pid))
                    .into_iter()
                    .find_map(|window| {
                        (!window.app_name.trim().is_empty()).then_some(window.app_name)
                    }),
                WindowAppNameProvenance::StableAppId,
            )
        };
        application_identity_from_stable_sources(process.name, platform_id)
    }
}

pub fn application_identity_provider() -> Arc<dyn ApplicationIdentityProvider> {
    Arc::new(LinuxApplicationIdentityProvider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_identity_is_resolvable_without_titles_or_paths() {
        let identity = LinuxApplicationIdentityProvider
            .resolve(i64::from(std::process::id()), None)
            .expect("current process identity");
        assert!(identity.bundle_id.is_some() || identity.display_name.is_some());
    }

    #[test]
    fn wayland_title_fallback_is_not_stored_as_application_identity() {
        let title = "Confidential roadmap - Web Browser".to_owned();
        let platform_id =
            stable_window_app_name(Some(title.clone()), WindowAppNameProvenance::Ambiguous);
        let identity = application_identity_from_stable_sources("browser".to_owned(), platform_id)
            .expect("process-derived identity");

        assert_eq!(identity.bundle_id.as_deref(), Some("browser"));
        assert_eq!(identity.display_name.as_deref(), Some("browser"));
        assert_ne!(identity.bundle_id.as_deref(), Some(title.as_str()));
        assert_ne!(identity.display_name.as_deref(), Some(title.as_str()));
    }
}
