//! Windows adapters for encrypted Computer History.

use cua_driver_core::history::{
    ApplicationIdentity, ApplicationIdentityProvider, HistoryError, HistoryHealthCategory,
    HistoryKey, KeyProvider,
};
use keyring::{Entry, Error as KeyringError};
use std::{path::PathBuf, sync::Arc};
use zeroize::Zeroizing;

const KEY_ACCOUNT: &str = "namespace-root-key-v1";

#[derive(Default)]
pub struct WindowsCredentialKeyProvider;

impl WindowsCredentialKeyProvider {
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

impl KeyProvider for WindowsCredentialKeyProvider {
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
pub struct WindowsApplicationIdentityProvider;

impl ApplicationIdentityProvider for WindowsApplicationIdentityProvider {
    fn resolve(&self, pid: i64, window_id: Option<u64>) -> Option<ApplicationIdentity> {
        let mut pid = u32::try_from(pid).ok()?;
        let owner_name = process_name(pid)?;
        if owner_name.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
            pid = crate::win32::resolve_uwp_app_pid(pid, window_id?)?;
        }
        let name = process_name(pid)?;
        Some(ApplicationIdentity {
            bundle_id: Some(name.clone()),
            display_name: Some(name),
        })
    }
}

fn process_name(pid: u32) -> Option<String> {
    let name = crate::win32::list_processes()
        .into_iter()
        .find(|process| process.pid == pid)?
        .name;
    (!name.trim().is_empty()).then_some(name)
}

pub fn application_identity_provider() -> Arc<dyn ApplicationIdentityProvider> {
    Arc::new(WindowsApplicationIdentityProvider)
}

pub fn process_executable_path(pid: u32) -> Option<PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0_u16; 32_768];
        let mut length = u32::try_from(buffer.len()).ok()?;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        buffer.truncate(length as usize);
        Some(PathBuf::from(String::from_utf16_lossy(&buffer)))
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
    fn credential_namespace_is_strict_and_separated() {
        assert_ne!(
            WindowsCredentialKeyProvider::service("cua-driver").unwrap(),
            WindowsCredentialKeyProvider::service("cua-driver-local").unwrap()
        );
        assert!(WindowsCredentialKeyProvider::service("../escape").is_err());
    }

    #[test]
    fn current_process_identity_is_resolvable() {
        let path = process_executable_path(std::process::id()).expect("current process path");
        assert!(path.is_absolute());
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
    #[ignore = "mutates uniquely named Windows Credential Manager items; run in native release qualification"]
    fn credential_manager_key_lifecycle_is_isolated_corruptible_and_destroyable() {
        let namespace = format!("cua-driver-local-test-{}", uuid::Uuid::new_v4().simple());
        let other_namespace = format!("{namespace}-other");
        let provider = WindowsCredentialKeyProvider;
        assert!(provider.references(&namespace).unwrap().is_empty());
        assert!(provider.references(&other_namespace).unwrap().is_empty());

        let created = provider.load_or_create(&namespace).unwrap();
        let entry = WindowsCredentialKeyProvider::entry(&created.reference).unwrap();
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

        WindowsCredentialKeyProvider::entry(&created.reference)
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
