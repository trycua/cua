// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Operating-system security and consent prompts are the user's to answer.
//!
//! "HoloWork would like to access data from other apps", a password or
//! Touch ID sheet, an accessibility-control request and a Gatekeeper warning
//! each grant or refuse access on the user's behalf. An agent clicked Allow
//! on the first one (HoloWork HW-110). Every input action whose target
//! window, or whose screen point, belongs to the process that draws such a
//! prompt is refused with [`SECURITY_PROMPT_PROTECTED`] before any event is
//! sent. Reading the prompt (window state, screenshots) stays allowed.
//!
//! The platform supplies the owner's identity; this module decides.
//!
//! - macOS: the bundle ids and process names in [`MACOS_PROMPT_OWNERS`].
//! - Windows: UAC (`consent.exe`) runs on the secure desktop, which no input
//!   from the user's desktop reaches. Windows Security credential prompts
//!   (`CredentialUIBroker.exe`) are not checked yet.
//! - Linux: polkit authentication agents are desktop-specific processes and
//!   are not checked yet.

use serde_json::json;

use crate::protocol::ToolResult;

/// The refusal code for input aimed at a security or consent prompt.
pub const SECURITY_PROMPT_PROTECTED: &str = "security_prompt_protected";

/// The macOS processes that draw security and consent prompts: bundle id and
/// executable name. Either one matches.
pub const MACOS_PROMPT_OWNERS: &[(&str, &str)] = &[
    // Privacy (TCC) consent alerts such as "would like to access data from
    // other apps", and other system alerts posted by daemons.
    ("com.apple.UserNotificationCenter", "UserNotificationCenter"),
    // Administrator password, keychain and authorization sheets.
    ("com.apple.SecurityAgent", "SecurityAgent"),
    // "… would like to control this computer using accessibility features".
    (
        "com.apple.accessibility.universalAccessAuthWarn",
        "universalAccessAuthWarn",
    ),
    // Touch ID and password prompts of LocalAuthentication.
    ("com.apple.LocalAuthentication.UIAgent", "coreautha"),
    // Gatekeeper and quarantine warnings ("… downloaded from the Internet").
    ("com.apple.coreservices.uiagent", "CoreServicesUIAgent"),
    // Location access requests.
    ("com.apple.CoreLocationAgent", "CoreLocationAgent"),
];

/// Whether a window owned by a process with this bundle id or executable
/// name is a macOS security or consent prompt.
pub fn is_macos_prompt_owner(bundle_id: Option<&str>, process_name: Option<&str>) -> bool {
    MACOS_PROMPT_OWNERS.iter().any(|(bundle, name)| {
        bundle_id == Some(*bundle) || process_name.is_some_and(|process| process == *name)
    })
}

/// The refusal for input aimed at a prompt of `owner` (its process name).
pub fn refusal(owner: &str) -> ToolResult {
    let message = format!(
        "That window is a system security or consent prompt ({owner}), so nothing was sent. \
         Answering it grants or refuses access on the user's behalf: leave it to the user."
    );
    ToolResult::error(&message).with_structured(json!({
        "code": SECURITY_PROMPT_PROTECTED,
        "effect": "refused",
        "owner": owner,
        "message": message,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_prompt_owners_match_by_bundle_id_or_process_name() {
        assert!(is_macos_prompt_owner(
            Some("com.apple.UserNotificationCenter"),
            None
        ));
        assert!(is_macos_prompt_owner(None, Some("SecurityAgent")));
        assert!(is_macos_prompt_owner(Some(""), Some("coreautha")));
        assert!(is_macos_prompt_owner(
            Some("com.apple.coreservices.uiagent"),
            Some("CoreServicesUIAgent")
        ));
        for (bundle, name) in [
            (Some("com.apple.finder"), Some("Finder")),
            (Some("com.apple.systempreferences"), Some("System Settings")),
            (
                Some("com.apple.usernotificationcenter"),
                Some("securityagent"),
            ),
            (None, None),
        ] {
            assert!(!is_macos_prompt_owner(bundle, name), "{bundle:?} {name:?}");
        }
    }

    #[test]
    fn the_refusal_has_its_code_and_names_the_owner() {
        let result = refusal("UserNotificationCenter");
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], SECURITY_PROMPT_PROTECTED);
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["owner"], "UserNotificationCenter");
    }
}
