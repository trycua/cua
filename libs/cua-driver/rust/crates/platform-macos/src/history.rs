//! macOS adapters for encrypted Computer History.

use cua_driver_core::history::{
    ApplicationIdentity, ApplicationIdentityProvider, HistoryError, HistoryHealthCategory,
    HistoryKey, KeyProvider,
};
use security_framework::{
    access_control::{ProtectionMode, SecAccessControl},
    passwords::{
        delete_generic_password_options, generic_password, set_generic_password_options,
        PasswordOptions,
    },
};
use std::sync::Arc;
use zeroize::Zeroizing;

const KEY_ACCOUNT: &str = "namespace-root-key-v1";

#[derive(Default)]
pub struct MacosKeychainKeyProvider;

impl MacosKeychainKeyProvider {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self)
    }

    fn service(namespace: &str) -> Result<String, HistoryError> {
        let valid = !namespace.is_empty()
            && namespace.len() <= 64
            && namespace
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        if !valid {
            return Err(HistoryError::new(HistoryHealthCategory::KeyUnavailable));
        }
        Ok(format!("com.trycua.{namespace}.computer-history.v1"))
    }

    fn uses_data_protection(namespace: &str) -> bool {
        namespace == "cua-driver"
    }

    fn lookup_options(service: &str, data_protection: bool) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(service, KEY_ACCOUNT);
        options.set_access_synchronized(Some(false));
        if data_protection {
            options.use_protected_keychain();
        }
        options
    }

    fn creation_options(
        service: &str,
        data_protection: bool,
    ) -> Result<PasswordOptions, HistoryError> {
        let mut options = Self::lookup_options(service, data_protection);
        if data_protection {
            let access = SecAccessControl::create_with_protection(
                Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
                0,
            )
            .map_err(map_keychain_error)?;
            options.set_access_control(access);
        }
        options.set_label("Cua Driver Computer History encryption key");
        options.set_description("Device-local key for encrypted Cua Driver Computer History");
        Ok(options)
    }
}

impl KeyProvider for MacosKeychainKeyProvider {
    fn load_or_create(&self, namespace: &str) -> Result<HistoryKey, HistoryError> {
        let service = Self::service(namespace)?;
        let data_protection = Self::uses_data_protection(namespace);
        match generic_password(Self::lookup_options(&service, data_protection)) {
            Ok(bytes) => key(service, bytes),
            Err(error) if error.code() == -25300 => {
                let mut bytes = Zeroizing::new(vec![0_u8; 32]);
                getrandom::fill(&mut bytes)
                    .map_err(|_| HistoryError::new(HistoryHealthCategory::KeyUnavailable))?;
                set_generic_password_options(
                    &bytes,
                    Self::creation_options(&service, data_protection)?,
                )
                .map_err(map_keychain_error)?;
                // Read back through the same device-only item before any
                // encrypted file is created. This detects locked/inaccessible
                // Keychain state and prevents a plaintext or replacement-key
                // fallback.
                let verified = Zeroizing::new(
                    generic_password(Self::lookup_options(&service, data_protection))
                        .map_err(map_keychain_error)?,
                );
                if verified.as_slice() != bytes.as_slice() {
                    return Err(HistoryError::new(HistoryHealthCategory::KeyCorrupt));
                }
                zeroizing_key(service, verified)
            }
            Err(error) => Err(map_keychain_error(error)),
        }
    }

    fn load(&self, namespace: &str, reference: &str) -> Result<HistoryKey, HistoryError> {
        let service = Self::service(namespace)?;
        if reference != service {
            return Err(HistoryError::new(HistoryHealthCategory::KeyUnavailable));
        }
        let bytes = generic_password(Self::lookup_options(
            &service,
            Self::uses_data_protection(namespace),
        ))
        .map_err(map_keychain_error)?;
        key(service, bytes)
    }

    fn references(&self, namespace: &str) -> Result<Vec<String>, HistoryError> {
        let service = Self::service(namespace)?;
        match generic_password(Self::lookup_options(
            &service,
            Self::uses_data_protection(namespace),
        )) {
            Ok(bytes) => {
                let _bytes = Zeroizing::new(bytes);
                Ok(vec![service])
            }
            Err(error) if error.code() == -25300 => Ok(Vec::new()),
            Err(error) => Err(map_keychain_error(error)),
        }
    }

    fn destroy(&self, namespace: &str, reference: &str) -> Result<(), HistoryError> {
        let service = Self::service(namespace)?;
        if reference != service {
            return Err(HistoryError::new(HistoryHealthCategory::KeyUnavailable));
        }
        let data_protection = Self::uses_data_protection(namespace);
        match delete_generic_password_options(Self::lookup_options(&service, data_protection)) {
            Ok(()) => {}
            Err(error) if error.code() == -25300 => {}
            Err(error) => return Err(map_keychain_error(error)),
        }
        match generic_password(Self::lookup_options(&service, data_protection)) {
            Err(error) if error.code() == -25300 => Ok(()),
            Ok(bytes) => {
                let _bytes = Zeroizing::new(bytes);
                Err(HistoryError::new(HistoryHealthCategory::KeyDestroyFailed))
            }
            Err(error) => Err(map_keychain_error(error)),
        }
    }
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

fn map_keychain_error(error: security_framework::base::Error) -> HistoryError {
    let category = match error.code() {
        -25308 | -25293 => HistoryHealthCategory::KeyLocked,
        -25300 => HistoryHealthCategory::KeyUnavailable,
        _ => HistoryHealthCategory::KeyUnavailable,
    };
    HistoryError::new(category)
}

#[derive(Default)]
pub struct MacosApplicationIdentityProvider;

impl ApplicationIdentityProvider for MacosApplicationIdentityProvider {
    fn resolve(&self, pid: i64, _window_id: Option<u64>) -> Option<ApplicationIdentity> {
        let pid = i32::try_from(pid).ok()?;
        crate::apps::list_running_apps()
            .into_iter()
            .find(|app| app.pid == pid)
            .map(|app| ApplicationIdentity {
                bundle_id: app.bundle_id,
                display_name: Some(app.name),
            })
    }
}

pub fn application_identity_provider() -> Arc<dyn ApplicationIdentityProvider> {
    Arc::new(MacosApplicationIdentityProvider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_namespace_is_strict_and_separated() {
        assert_ne!(
            MacosKeychainKeyProvider::service("cua-driver").unwrap(),
            MacosKeychainKeyProvider::service("cua-driver-local").unwrap()
        );
        assert!(MacosKeychainKeyProvider::service("../escape").is_err());
        assert!(MacosKeychainKeyProvider::uses_data_protection("cua-driver"));
        assert!(!MacosKeychainKeyProvider::uses_data_protection(
            "cua-driver-local"
        ));
    }

    #[test]
    #[ignore = "mutates one uniquely named login-Keychain item; run in macOS release qualification"]
    fn local_keychain_key_lifecycle_is_destroyable() {
        let namespace = format!("cua-driver-local-test-{}", uuid::Uuid::new_v4().simple());
        let provider = MacosKeychainKeyProvider;
        let created = provider.load_or_create(&namespace).unwrap();
        assert_eq!(created.bytes.len(), 32);
        let loaded = provider.load(&namespace, &created.reference).unwrap();
        assert_eq!(created.bytes.as_slice(), loaded.bytes.as_slice());
        provider.destroy(&namespace, &created.reference).unwrap();
        match provider.load(&namespace, &created.reference) {
            Err(error) => assert_eq!(error.category, HistoryHealthCategory::KeyUnavailable),
            Ok(_) => panic!("destroyed Keychain reference unexpectedly remained loadable"),
        }
    }
}
