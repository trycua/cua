//! Windows adapters for encrypted Computer History.

use cua_driver_core::history::{ApplicationIdentity, ApplicationIdentityProvider};
pub use cua_driver_core::history_keyring::KeyringKeyProvider as WindowsCredentialKeyProvider;
use std::{path::PathBuf, sync::Arc};

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

    #[test]
    fn current_process_identity_is_resolvable() {
        let path = process_executable_path(std::process::id()).expect("current process path");
        assert!(path.is_absolute());
    }
}
