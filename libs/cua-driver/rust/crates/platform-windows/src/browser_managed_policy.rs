// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Machine policy that forces extensions on a Windows browser.
//!
//! Chromium reads mandatory policy from `HKLM\SOFTWARE\Policies\<vendor>`,
//! which only administrators can write. `ExtensionInstallForcelist` is a
//! subkey of numbered string values and `ExtensionSettings` a JSON string.
//! The key choice and the policy assembly are pure, so their unit tests run
//! on any host; only the registry read is Windows-only.

use std::path::Path;

use serde_json::Value;

/// The `HKLM` policy key Chromium reads for the browser at `executable`.
fn policy_key(executable: &Path) -> Option<&'static str> {
    let path = executable
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let file = path.rsplit('\\').next().unwrap_or_default();
    match file {
        "msedge.exe" => Some(r"SOFTWARE\Policies\Microsoft\Edge"),
        "brave.exe" => Some(r"SOFTWARE\Policies\BraveSoftware\Brave"),
        "chrome.exe" if path.contains(r"\google\chrome") => {
            Some(r"SOFTWARE\Policies\Google\Chrome")
        }
        "chrome.exe" => Some(r"SOFTWARE\Policies\Chromium"),
        _ => None,
    }
}

/// The policy document for the values read from a policy key.
fn policy_document(forcelist: Vec<String>, settings: Option<String>) -> Value {
    let mut policy = serde_json::Map::new();
    policy.insert("ExtensionInstallForcelist".to_owned(), forcelist.into());
    if let Some(settings) = settings {
        policy.insert("ExtensionSettings".to_owned(), settings.into());
    }
    Value::Object(policy)
}

/// The `HKLM` policy key that forces extensions on the browser at
/// `executable`, if one does.
#[cfg(target_os = "windows")]
pub(crate) fn managed_extension_policy(executable: &str) -> Option<String> {
    use cua_driver_core::browser::managed_policy::policy_forces_extensions;

    let key = policy_key(Path::new(executable))?;
    let policy = policy_document(
        registry::string_values(&format!(r"{key}\ExtensionInstallForcelist")),
        registry::string_value(key, "ExtensionSettings"),
    );
    policy_forces_extensions(&policy).then(|| format!(r"HKLM\{key}"))
}

#[cfg(target_os = "windows")]
mod registry {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_READ, REG_SZ, RRF_RT_REG_SZ,
    };

    fn utf16_string(units: &[u16]) -> String {
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        String::from_utf16_lossy(&units[..end])
    }

    /// The `REG_SZ` value `name` of `HKLM\<key>`.
    pub(super) fn string_value(key: &str, name: &str) -> Option<String> {
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        let mut bytes = 0u32;
        // SAFETY: a size query; no buffer is written.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut bytes),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let mut units = vec![0u16; (bytes as usize).div_ceil(2)];
        // SAFETY: `units` holds `bytes` bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                Some(units.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        (status == ERROR_SUCCESS).then(|| utf16_string(&units))
    }

    /// Every `REG_SZ` value of `HKLM\<key>`, in registry order.
    pub(super) fn string_values(key: &str) -> Vec<String> {
        let mut handle = HKEY::default();
        // SAFETY: `handle` receives the opened key, closed below.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                &HSTRING::from(key),
                None,
                KEY_READ,
                &mut handle,
            )
        };
        if status != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut values = Vec::new();
        // Value names are at most 16,383 characters; data grows on demand.
        let mut name = vec![0u16; 16_384];
        let mut data = vec![0u8; 4096];
        let mut index = 0;
        loop {
            let mut name_len = name.len() as u32;
            let mut kind = 0u32;
            let mut data_len = data.len() as u32;
            // SAFETY: each buffer's length is passed with it.
            let status = unsafe {
                RegEnumValueW(
                    handle,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut name_len,
                    None,
                    Some(&mut kind),
                    Some(data.as_mut_ptr()),
                    Some(&mut data_len),
                )
            };
            if status == ERROR_MORE_DATA {
                data.resize((data_len as usize).max(data.len() * 2), 0);
                continue;
            }
            if status != ERROR_SUCCESS {
                break;
            }
            index += 1;
            if kind == REG_SZ.0 {
                let units = data[..data_len as usize]
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect::<Vec<_>>();
                values.push(utf16_string(&units));
            }
        }
        // SAFETY: `handle` was opened above and is not used again.
        let _ = unsafe { RegCloseKey(handle) };
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::browser::managed_policy::policy_forces_extensions;

    #[test]
    fn policy_key_follows_the_launched_product() {
        for (executable, key) in [
            (
                r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                Some(r"SOFTWARE\Policies\Google\Chrome"),
            ),
            (
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                Some(r"SOFTWARE\Policies\Microsoft\Edge"),
            ),
            (
                r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
                Some(r"SOFTWARE\Policies\BraveSoftware\Brave"),
            ),
            (
                r"C:\Users\me\AppData\Local\Chromium\Application\chrome.exe",
                Some(r"SOFTWARE\Policies\Chromium"),
            ),
            (r"C:\Program Files\Mozilla Firefox\firefox.exe", None),
        ] {
            assert_eq!(policy_key(Path::new(executable)), key, "{executable}");
        }
    }

    #[test]
    fn registry_values_force_extensions_like_policy_files() {
        assert!(policy_forces_extensions(&policy_document(
            vec![
                "ddkjiahejlhfcafbddmgiahcphecmpfh;https://clients2.google.com/service/update2/crx"
                    .to_owned()
            ],
            None,
        )));
        assert!(policy_forces_extensions(&policy_document(
            Vec::new(),
            Some(
                r#"{"ddkjiahejlhfcafbddmgiahcphecmpfh": {"installation_mode": "force_installed"}}"#
                    .to_owned()
            ),
        )));
        assert!(!policy_forces_extensions(&policy_document(
            Vec::new(),
            Some(r#"{"*": {"installation_mode": "blocked"}}"#.to_owned()),
        )));
    }
}
