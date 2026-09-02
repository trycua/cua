//! Dedicated Windows Credential Manager storage for provider bootstrap credentials.

use cua_driver_core::credentials::{
    decode_bootstrap_store_record, encode_bootstrap_store_record, BootstrapNamespace,
    BootstrapSecret, BootstrapSecretLease, BootstrapSecretStore, BootstrapStoreError,
    BootstrapStoreHealth, BootstrapVersion,
};
use keyring::{Entry, Error as KeyringError};
use std::sync::Arc;
use zeroize::Zeroizing;

const BOOTSTRAP_ACCOUNT: &str = "provider-bootstrap-token-v1";

#[derive(Default)]
pub struct WindowsBootstrapSecretStore;

impl WindowsBootstrapSecretStore {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self)
    }

    fn service(namespace: &BootstrapNamespace) -> String {
        format!(
            "com.trycua.cua-driver.credential-bootstrap.v1.{}",
            namespace.as_str()
        )
    }

    fn entry(namespace: &BootstrapNamespace) -> Result<Entry, BootstrapStoreError> {
        Entry::new(&Self::service(namespace), BOOTSTRAP_ACCOUNT).map_err(map_keyring_error)
    }

    fn verified_write(
        entry: &Entry,
        record: &Zeroizing<Vec<u8>>,
    ) -> Result<(), BootstrapStoreError> {
        entry.set_secret(record).map_err(map_keyring_error)?;
        let verified = Zeroizing::new(entry.get_secret().map_err(map_keyring_error)?);
        if verified.as_slice() != record.as_slice() {
            return Err(BootstrapStoreError::Corrupt);
        }
        decode_bootstrap_store_record(verified.to_vec())?;
        Ok(())
    }
}

impl BootstrapSecretStore for WindowsBootstrapSecretStore {
    fn enroll(
        &self,
        namespace: BootstrapNamespace,
        value: BootstrapSecret,
    ) -> Result<BootstrapVersion, BootstrapStoreError> {
        let entry = Self::entry(&namespace)?;
        match entry.get_secret() {
            Err(KeyringError::NoEntry) => {}
            Ok(existing) => {
                let _existing = Zeroizing::new(existing);
                return Err(BootstrapStoreError::AlreadyEnrolled);
            }
            Err(error) => return Err(map_keyring_error(error)),
        }
        let version = BootstrapVersion(1);
        let record = encode_bootstrap_store_record(version, value)?;
        Self::verified_write(&entry, &record)?;
        Ok(version)
    }

    fn load(
        &self,
        namespace: &BootstrapNamespace,
    ) -> Result<BootstrapSecretLease, BootstrapStoreError> {
        let record = Self::entry(namespace)?
            .get_secret()
            .map_err(map_keyring_error)?;
        decode_bootstrap_store_record(record).map(|(_, lease)| lease)
    }

    fn rotate(
        &self,
        namespace: &BootstrapNamespace,
        value: BootstrapSecret,
    ) -> Result<BootstrapVersion, BootstrapStoreError> {
        let entry = Self::entry(namespace)?;
        let current = entry.get_secret().map_err(map_keyring_error)?;
        let (current, _lease) = decode_bootstrap_store_record(current)?;
        let version = BootstrapVersion(
            current
                .0
                .checked_add(1)
                .ok_or(BootstrapStoreError::Corrupt)?,
        );
        let record = encode_bootstrap_store_record(version, value)?;
        Self::verified_write(&entry, &record)?;
        Ok(version)
    }

    fn revoke(&self, namespace: &BootstrapNamespace) -> Result<(), BootstrapStoreError> {
        let entry = Self::entry(namespace)?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(error) => return Err(map_keyring_error(error)),
        }
        match entry.get_secret() {
            Err(KeyringError::NoEntry) => Ok(()),
            Ok(record) => {
                let _record = Zeroizing::new(record);
                Err(BootstrapStoreError::Unavailable)
            }
            Err(error) => Err(map_keyring_error(error)),
        }
    }

    fn health(&self, namespace: &BootstrapNamespace) -> BootstrapStoreHealth {
        let entry = match Self::entry(namespace) {
            Ok(entry) => entry,
            Err(error) => return health_for_error(error),
        };
        match entry.get_secret() {
            Ok(record) => match decode_bootstrap_store_record(record) {
                Ok(_) => BootstrapStoreHealth::Available,
                Err(_) => BootstrapStoreHealth::Corrupt,
            },
            Err(error) => health_for_error(map_keyring_error(error)),
        }
    }
}

fn map_keyring_error(error: KeyringError) -> BootstrapStoreError {
    match error {
        KeyringError::NoEntry => BootstrapStoreError::Missing,
        KeyringError::NoStorageAccess(_) => BootstrapStoreError::Locked,
        KeyringError::BadDataFormat(_, _)
        | KeyringError::BadStoreFormat(_)
        | KeyringError::BadEncoding(_)
        | KeyringError::Ambiguous(_) => BootstrapStoreError::Corrupt,
        _ => BootstrapStoreError::Unavailable,
    }
}

fn health_for_error(error: BootstrapStoreError) -> BootstrapStoreHealth {
    match error {
        BootstrapStoreError::Missing => BootstrapStoreHealth::Missing,
        BootstrapStoreError::Locked => BootstrapStoreHealth::Locked,
        BootstrapStoreError::Corrupt => BootstrapStoreHealth::Corrupt,
        _ => BootstrapStoreHealth::Unavailable,
    }
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
    fn bootstrap_namespace_is_strict_and_separate_from_computer_history() {
        let namespace = BootstrapNamespace::new("automation-vault").unwrap();
        let service = WindowsBootstrapSecretStore::service(&namespace);
        assert_eq!(
            service,
            "com.trycua.cua-driver.credential-bootstrap.v1.automation-vault"
        );
        assert!(!service.contains("computer-history"));
        assert_eq!(format!("{namespace:?}"), "BootstrapNamespace([private])");
    }

    #[test]
    fn inaccessible_store_maps_to_locked_without_fallback() {
        let error = KeyringError::NoStorageAccess(Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "synthetic locked store",
        )));
        assert_eq!(map_keyring_error(error), BootstrapStoreError::Locked);
    }

    #[test]
    #[ignore = "mutates a uniquely named Credential Manager item; run in native Fleet qualification"]
    fn credential_manager_bootstrap_lifecycle_is_versioned_and_destroyable() {
        let namespace =
            BootstrapNamespace::new(format!("bootstrap-test-{}", uuid::Uuid::new_v4().simple()))
                .unwrap();
        let store = WindowsBootstrapSecretStore;
        let entry = WindowsBootstrapSecretStore::entry(&namespace).unwrap();
        let _cleanup = CredentialCleanup(entry);
        assert_eq!(store.health(&namespace), BootstrapStoreHealth::Missing);
        assert_eq!(
            store
                .enroll(
                    namespace.clone(),
                    BootstrapSecret::from_bytes(b"synthetic-bootstrap-a".to_vec()).unwrap(),
                )
                .unwrap(),
            BootstrapVersion(1)
        );
        assert_eq!(store.health(&namespace), BootstrapStoreHealth::Available);
        assert!(store.load(&namespace).is_ok());
        assert_eq!(
            store
                .rotate(
                    &namespace,
                    BootstrapSecret::from_bytes(b"synthetic-bootstrap-b".to_vec()).unwrap(),
                )
                .unwrap(),
            BootstrapVersion(2)
        );
        store.revoke(&namespace).unwrap();
        assert_eq!(store.health(&namespace), BootstrapStoreHealth::Missing);
        assert_eq!(
            store.load(&namespace).err(),
            Some(BootstrapStoreError::Missing)
        );
    }
}
