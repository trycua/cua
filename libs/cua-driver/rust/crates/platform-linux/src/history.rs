//! Linux adapters for encrypted Computer History.

use cua_driver_core::history::{
    ApplicationIdentity, ApplicationIdentityProvider, HistoryError, HistoryHealthCategory,
    HistoryKey, KeyProvider,
};
use keyring::{Entry, Error as KeyringError};
use std::sync::Arc;
use zeroize::Zeroizing;

const KEY_ACCOUNT: &str = "namespace-root-key-v1";

#[derive(Default)]
pub struct LinuxSecretServiceKeyProvider;

impl LinuxSecretServiceKeyProvider {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self)
    }

    fn service(namespace: &str) -> Result<String, HistoryError> {
        validate_namespace(namespace)?;
        Ok(format!("com.trycua.{namespace}.computer-history.v1"))
    }

    fn entry(service: &str) -> Result<Entry, HistoryError> {
        Entry::new(service, KEY_ACCOUNT).map_err(map_keyring_error)
    }
}

impl KeyProvider for LinuxSecretServiceKeyProvider {
    fn load_or_create(&self, namespace: &str) -> Result<HistoryKey, HistoryError> {
        let service = Self::service(namespace)?;
        let entry = Self::entry(&service)?;
        match entry.get_secret() {
            Ok(bytes) => key(service, bytes),
            Err(KeyringError::NoEntry) => {
                let mut bytes = Zeroizing::new(vec![0_u8; 32]);
                getrandom::fill(&mut bytes)
                    .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyUnavailable))?;
                entry.set_secret(&bytes).map_err(map_keyring_error)?;
                let verified = Zeroizing::new(entry.get_secret().map_err(map_keyring_error)?);
                if verified.as_slice() != bytes.as_slice() {
                    return Err(HistoryError::new(HistoryHealthCategory::KeyCorrupt));
                }
                zeroizing_key(service, verified)
            }
            Err(error) => Err(map_keyring_error(error)),
        }
    }

    fn load(&self, namespace: &str, reference: &str) -> Result<HistoryKey, HistoryError> {
        let service = Self::service(namespace)?;
        if reference != service {
            return Err(HistoryError::new(HistoryHealthCategory::KeyUnavailable));
        }
        key(
            service.clone(),
            Self::entry(&service)?
                .get_secret()
                .map_err(map_keyring_error)?,
        )
    }

    fn references(&self, namespace: &str) -> Result<Vec<String>, HistoryError> {
        let service = Self::service(namespace)?;
        match Self::entry(&service)?.get_secret() {
            Ok(bytes) => {
                let _bytes = Zeroizing::new(bytes);
                Ok(vec![service])
            }
            Err(KeyringError::NoEntry) => Ok(Vec::new()),
            Err(error) => Err(map_keyring_error(error)),
        }
    }

    fn destroy(&self, namespace: &str, reference: &str) -> Result<(), HistoryError> {
        let service = Self::service(namespace)?;
        if reference != service {
            return Err(HistoryError::new(HistoryHealthCategory::KeyUnavailable));
        }
        let entry = Self::entry(&service)?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(error) => return Err(map_keyring_error(error)),
        }
        match entry.get_secret() {
            Err(KeyringError::NoEntry) => Ok(()),
            Ok(bytes) => {
                let _bytes = Zeroizing::new(bytes);
                Err(HistoryError::new(HistoryHealthCategory::KeyDestroyFailed))
            }
            Err(error) => Err(map_keyring_error(error)),
        }
    }
}

fn validate_namespace(namespace: &str) -> Result<(), HistoryError> {
    let valid = !namespace.is_empty()
        && namespace.len() <= 64
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    valid
        .then_some(())
        .ok_or_else(|| HistoryError::new(HistoryHealthCategory::KeyUnavailable))
}

fn key(reference: String, bytes: Vec<u8>) -> Result<HistoryKey, HistoryError> {
    zeroizing_key(reference, Zeroizing::new(bytes))
}

fn zeroizing_key(reference: String, bytes: Zeroizing<Vec<u8>>) -> Result<HistoryKey, HistoryError> {
    if bytes.len() != 32 {
        return Err(HistoryError::new(HistoryHealthCategory::KeyCorrupt));
    }
    Ok(HistoryKey {
        reference,
        epoch: 1,
        bytes,
    })
}

fn map_keyring_error(error: KeyringError) -> HistoryError {
    let category = match error {
        KeyringError::NoStorageAccess(_) => HistoryHealthCategory::KeyLocked,
        KeyringError::BadDataFormat(_, _)
        | KeyringError::BadStoreFormat(_)
        | KeyringError::BadEncoding(_)
        | KeyringError::Ambiguous(_) => HistoryHealthCategory::KeyCorrupt,
        _ => HistoryHealthCategory::KeyUnavailable,
    };
    HistoryError::new(category)
}

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

    struct CredentialCleanup(Entry);

    impl Drop for CredentialCleanup {
        fn drop(&mut self) {
            let _ = self.0.delete_credential();
        }
    }

    #[test]
    fn secret_service_namespace_is_strict_and_separated() {
        assert_ne!(
            LinuxSecretServiceKeyProvider::service("cua-driver").unwrap(),
            LinuxSecretServiceKeyProvider::service("cua-driver-local").unwrap()
        );
        assert!(LinuxSecretServiceKeyProvider::service("../escape").is_err());
    }

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

    #[test]
    fn inaccessible_store_maps_to_locked_without_fallback() {
        let error = KeyringError::NoStorageAccess(Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "synthetic locked vault",
        )));
        assert_eq!(
            map_keyring_error(error).category,
            HistoryHealthCategory::KeyLocked
        );
    }

    #[test]
    #[ignore = "mutates uniquely named Secret Service items; run in native release qualification"]
    fn secret_service_key_lifecycle_is_isolated_corruptible_and_destroyable() {
        let namespace = format!("cua-driver-local-test-{}", uuid::Uuid::new_v4().simple());
        let other_namespace = format!("{namespace}-other");
        let provider = LinuxSecretServiceKeyProvider;
        assert!(provider.references(&namespace).unwrap().is_empty());
        assert!(provider.references(&other_namespace).unwrap().is_empty());

        let created = provider.load_or_create(&namespace).unwrap();
        let entry = LinuxSecretServiceKeyProvider::entry(&created.reference).unwrap();
        let _cleanup = CredentialCleanup(entry);
        assert_eq!(created.bytes.len(), 32);
        assert_eq!(
            provider
                .load(&namespace, &created.reference)
                .unwrap()
                .bytes
                .as_slice(),
            created.bytes.as_slice()
        );
        assert!(provider.references(&other_namespace).unwrap().is_empty());

        LinuxSecretServiceKeyProvider::entry(&created.reference)
            .unwrap()
            .set_secret(b"corrupt")
            .unwrap();
        match provider.load(&namespace, &created.reference) {
            Err(error) => assert_eq!(error.category, HistoryHealthCategory::KeyCorrupt),
            Ok(_) => panic!("corrupt credential unexpectedly loaded"),
        }
        provider.destroy(&namespace, &created.reference).unwrap();
        assert!(provider.references(&namespace).unwrap().is_empty());
    }
}
