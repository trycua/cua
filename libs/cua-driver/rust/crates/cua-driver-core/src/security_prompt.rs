// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Operating-system security and consent prompts are the user's to answer.
//!
//! "HoloWork would like to access data from other apps", a password or
//! Touch ID sheet, an accessibility-control request and a Gatekeeper warning
//! each grant or refuse access on the user's behalf. An agent clicked Allow
//! on the first one. Input whose target window, or whose screen point,
//! belongs to such a prompt is refused with [`SECURITY_PROMPT_PROTECTED`]
//! before any event is sent. Reading the prompt (window state, screenshots)
//! stays allowed.
//!
//! The refusal is the `os_security_prompt` enforcement adapter in
//! [`crate::authorization`]. It holds in every permission mode except
//! `unrestricted`, which a trusted host enables at launch for a disposable
//! machine; there input may answer the prompt (an unattended VM seed, say).
//! The dispatch boundary decides once per call ([`PromptInput::for_mode`])
//! and hands the platform the decision as a reserved argument that callers
//! cannot set.
//!
//! The platform supplies the owner's identity; this module decides.
//!
//! - macOS: the bundle ids and process names in [`MACOS_PROMPT_OWNERS`].
//!   UserNotificationCenter also draws daemon alerts that grant nothing
//!   ("… quit unexpectedly"), so its windows count only when
//!   [`is_consent_alert`] says so.
//! - Windows: UAC (`consent.exe`) runs on the secure desktop, which no input
//!   from the user's desktop reaches. Windows Security credential prompts
//!   (`CredentialUIBroker.exe`) are not checked yet.
//! - Linux: polkit authentication agents are desktop-specific processes and
//!   are not checked yet.

use serde_json::{json, Value};

use crate::authorization::{ModeBehavior, PermissionMode};
use crate::protocol::ToolResult;

/// The enforcement adapter that owns this refusal.
pub const ADAPTER_ID: &str = "os_security_prompt";

/// The refusal code for input aimed at a security or consent prompt.
pub const SECURITY_PROMPT_PROTECTED: &str = "security_prompt_protected";

/// Reserved argument the dispatch boundary sets when the call's permission
/// mode lets input reach a prompt. Reserved (`_`) arguments are stripped from
/// caller input before dispatch, so only the boundary can set it.
pub const ALLOW_ARG: &str = "_security_prompt_input_allowed";

/// Whether this call may send input to a security or consent prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptInput {
    Refuse,
    Allow,
}

impl PromptInput {
    /// The `os_security_prompt` adapter's decision for `mode`.
    pub fn for_mode(mode: PermissionMode) -> Self {
        let adapter = crate::authorization::ENFORCEMENT_ADAPTERS
            .iter()
            .find(|adapter| adapter.id == ADAPTER_ID)
            .expect("the os_security_prompt adapter is declared");
        match adapter.profile_behavior.for_mode(mode) {
            ModeBehavior::Allow => Self::Allow,
            ModeBehavior::Deny | ModeBehavior::RequireGrant | ModeBehavior::AllowWithoutGrant => {
                Self::Refuse
            }
        }
    }

    /// The decision the dispatch boundary left on a call's arguments. A call
    /// that did not pass through the boundary is refused.
    pub fn from_args(args: &Value) -> Self {
        if args.get(ALLOW_ARG).and_then(Value::as_bool) == Some(true) {
            Self::Allow
        } else {
            Self::Refuse
        }
    }

    /// Record the decision on the dispatched arguments.
    pub fn stamp(self, args: &mut Value) {
        if let (Self::Allow, Some(object)) = (self, args.as_object_mut()) {
            object.insert(ALLOW_ARG.to_owned(), Value::Bool(true));
        }
    }
}

/// The macOS processes that draw security and consent prompts: bundle id and
/// executable name. Either one matches.
pub const MACOS_PROMPT_OWNERS: &[(&str, &str)] = &[
    // Privacy (TCC) consent alerts such as "would like to access data from
    // other apps". Its other alerts are filtered by `is_consent_alert`.
    (MACOS_NOTIFICATION_CENTER.0, MACOS_NOTIFICATION_CENTER.1),
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

/// UserNotificationCenter: it draws privacy consent alerts and also daemon
/// alerts that grant nothing.
pub const MACOS_NOTIFICATION_CENTER: (&str, &str) =
    ("com.apple.UserNotificationCenter", "UserNotificationCenter");

/// How a process's windows are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOwner {
    /// Not a prompt owner.
    None,
    /// Every window it draws is a prompt.
    Always,
    /// A window is a prompt only if [`is_consent_alert`] says so.
    ConsentAlertsOnly,
}

/// How a window owned by a process with this bundle id or executable name
/// is treated.
pub fn macos_prompt_owner(bundle_id: Option<&str>, process_name: Option<&str>) -> PromptOwner {
    let matches =
        |(bundle, name): (&str, &str)| bundle_id == Some(bundle) || process_name == Some(name);
    if matches(MACOS_NOTIFICATION_CENTER) {
        PromptOwner::ConsentAlertsOnly
    } else if MACOS_PROMPT_OWNERS.iter().any(|owner| matches(*owner)) {
        PromptOwner::Always
    } else {
        PromptOwner::None
    }
}

/// What a UserNotificationCenter alert shows, as far as the decision needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlertContent {
    /// Whether the alert has a secure (password) text field.
    pub has_secure_field: bool,
    /// Each button's label (its title, else its description); empty for a
    /// button whose label could not be read.
    pub button_titles: Vec<String>,
}

/// Whether an alert asks for consent: it takes a password, or one of its
/// buttons carries a label the system uses for its consent prompts (in the
/// user's language, from the system's own strings). Only an alert read in
/// full, whose buttons all have labels and none of them a consent label, is
/// not a consent alert: unreadable content, no buttons, a button without a
/// label, or no known labels at all count as consent.
pub fn is_consent_alert(content: Option<&AlertContent>, consent_labels: &[String]) -> bool {
    let Some(content) = content else {
        return true;
    };
    let titles: Vec<&str> = content
        .button_titles
        .iter()
        .map(|title| title.trim())
        .collect();
    content.has_secure_field
        || titles.is_empty()
        || titles.iter().any(|title| title.is_empty())
        || consent_labels.is_empty()
        || titles
            .iter()
            .any(|title| consent_labels.iter().any(|label| label == title))
}

/// The refusal for input aimed at a prompt of `owner` (its process name), in
/// the authorization refusal shape.
pub fn refusal(owner: &str) -> ToolResult {
    let message = format!(
        "That window is a system security or consent prompt ({owner}), so nothing was sent. \
         Answering it grants or refuses access on the user's behalf: leave it to the user."
    );
    ToolResult::error(&message).with_structured(json!({
        "status": "refused",
        "refusal": {
            "code": SECURITY_PROMPT_PROTECTED,
            "message": message,
            "owner": owner,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_owners_match_by_bundle_id_or_process_name() {
        assert_eq!(
            macos_prompt_owner(None, Some("SecurityAgent")),
            PromptOwner::Always
        );
        assert_eq!(
            macos_prompt_owner(Some(""), Some("coreautha")),
            PromptOwner::Always
        );
        assert_eq!(
            macos_prompt_owner(Some("com.apple.coreservices.uiagent"), None),
            PromptOwner::Always
        );
        assert_eq!(
            macos_prompt_owner(Some("com.apple.UserNotificationCenter"), None),
            PromptOwner::ConsentAlertsOnly
        );
        for (bundle, name) in [
            (Some("com.apple.finder"), Some("Finder")),
            (Some("com.apple.systempreferences"), Some("System Settings")),
            (
                Some("com.apple.usernotificationcenter"),
                Some("securityagent"),
            ),
            (None, None),
        ] {
            assert_eq!(
                macos_prompt_owner(bundle, name),
                PromptOwner::None,
                "{bundle:?} {name:?}"
            );
        }
    }

    #[test]
    fn only_consent_alerts_of_the_notification_center_are_prompts() {
        let labels = vec!["Allow".to_owned(), "Don’t Allow".to_owned()];
        let alert = |buttons: &[&str], secure: bool| AlertContent {
            has_secure_field: secure,
            button_titles: buttons.iter().map(|title| (*title).to_owned()).collect(),
        };
        // "Allow “Lume” to find devices on local networks?"
        assert!(is_consent_alert(
            Some(&alert(&["Don’t Allow", "Allow"], false)),
            &labels
        ));
        // "CuaSpacesMac quit unexpectedly.": its icon button reads as "Help".
        assert!(!is_consent_alert(
            Some(&alert(&["Help", "Ignore", "Report…"], false)),
            &labels
        ));
        assert!(
            is_consent_alert(Some(&alert(&["", "Ignore"], false)), &labels),
            "a button without a label could be the consent button"
        );
        assert!(is_consent_alert(Some(&alert(&["OK"], true)), &labels));
        assert!(
            is_consent_alert(None, &labels),
            "unreadable content fails closed"
        );
        assert!(
            is_consent_alert(Some(&alert(&[], false)), &labels),
            "an alert with no buttons read fails closed"
        );
        assert!(
            is_consent_alert(Some(&alert(&["Ignore"], false)), &[]),
            "without the system's labels nothing can be ruled out"
        );
    }

    #[test]
    fn only_unrestricted_mode_lets_input_reach_a_prompt() {
        assert_eq!(
            PromptInput::for_mode(PermissionMode::Standard),
            PromptInput::Refuse
        );
        assert_eq!(
            PromptInput::for_mode(PermissionMode::Bounded),
            PromptInput::Refuse
        );
        assert_eq!(
            PromptInput::for_mode(PermissionMode::Unrestricted),
            PromptInput::Allow
        );
    }

    #[test]
    fn the_decision_round_trips_through_the_reserved_argument() {
        let mut args = json!({"x": 1});
        PromptInput::Refuse.stamp(&mut args);
        assert_eq!(PromptInput::from_args(&args), PromptInput::Refuse);
        PromptInput::Allow.stamp(&mut args);
        assert_eq!(PromptInput::from_args(&args), PromptInput::Allow);
        assert_eq!(PromptInput::from_args(&json!({})), PromptInput::Refuse);
        assert!(
            ALLOW_ARG.starts_with('_'),
            "callers must not be able to set it"
        );
    }

    #[test]
    fn the_refusal_uses_the_authorization_shape() {
        let result = refusal("SecurityAgent");
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["status"], "refused");
        assert_eq!(structured["refusal"]["code"], SECURITY_PROMPT_PROTECTED);
        assert_eq!(structured["refusal"]["owner"], "SecurityAgent");
    }
}
