// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Chromium managed policy that installs extensions into every profile.
//!
//! An isolated launch normally turns extensions and background networking
//! off. When the machine's administrator forces extensions through managed
//! policy, those switches would stop Chromium from installing them, and a
//! relaunch would remove one already installed. Platform adapters read the
//! admin-only policy store for the browser being launched and pass the
//! document here; core then keeps the launch from blocking the install.

use serde_json::Value;

/// Whether `policy`, one managed Chromium policy document, makes the browser
/// install an extension without the user's choice: a non-empty
/// `ExtensionInstallForcelist`, or an `ExtensionSettings` entry for an
/// extension (not the `*` defaults) whose `installation_mode` is
/// `force_installed` or `normal_installed`. `ExtensionSettings` may be an
/// object or, as the Windows registry stores it, a JSON string.
pub fn policy_forces_extensions(policy: &Value) -> bool {
    let forcelist = policy
        .get("ExtensionInstallForcelist")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.as_str().is_some_and(|entry| !entry.trim().is_empty()))
        });
    let settings = match policy.get("ExtensionSettings") {
        Some(Value::String(json)) => serde_json::from_str(json).ok(),
        settings => settings.cloned(),
    };
    let settings = settings
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|settings| {
            settings.iter().any(|(extension, setting)| {
                extension != "*"
                    && matches!(
                        setting.get("installation_mode").and_then(Value::as_str),
                        Some("force_installed" | "normal_installed")
                    )
            })
        });
    forcelist || settings
}

#[cfg(test)]
mod tests {
    use super::policy_forces_extensions;
    use serde_json::json;

    #[test]
    fn forcelist_or_forced_extension_settings_force_extensions() {
        for policy in [
            json!({"ExtensionInstallForcelist": [
                "ddkjiahejlhfcafbddmgiahcphecmpfh;https://clients2.google.com/service/update2/crx"
            ]}),
            json!({"ExtensionSettings": {
                "ddkjiahejlhfcafbddmgiahcphecmpfh": {"installation_mode": "force_installed",
                    "update_url": "https://clients2.google.com/service/update2/crx"}
            }}),
            json!({"ExtensionSettings": {
                "ddkjiahejlhfcafbddmgiahcphecmpfh": {"installation_mode": "normal_installed",
                    "update_url": "https://clients2.google.com/service/update2/crx"}
            }}),
            json!({"ExtensionSettings":
                r#"{"ddkjiahejlhfcafbddmgiahcphecmpfh": {"installation_mode": "force_installed"}}"#
            }),
        ] {
            assert!(policy_forces_extensions(&policy), "{policy}");
        }
    }

    #[test]
    fn other_managed_policy_forces_no_extension() {
        for policy in [
            json!({}),
            json!({"ForcedLanguages": ["en-US"]}),
            json!({"ExtensionInstallForcelist": []}),
            json!({"ExtensionInstallForcelist": [""]}),
            json!({"ExtensionInstallForcelist": "not a list"}),
            json!({"ExtensionInstallBlocklist": ["*"]}),
            json!({"ExtensionSettings": "{not json"}),
            json!({"ExtensionSettings": {
                "*": {"installation_mode": "force_installed"},
                "ddkjiahejlhfcafbddmgiahcphecmpfh": {"installation_mode": "allowed"},
                "aapocclcgogkmnckokdopfmhonfmgoek": {"installation_mode": "blocked"}
            }}),
        ] {
            assert!(!policy_forces_extensions(&policy), "{policy}");
        }
    }
}
