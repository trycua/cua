// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pure classification of a dialog snapshot: what is it, and which answer
//! (if any) is safe. No platform calls, so it is unit-tested everywhere
//! against the real strings macOS 26 produces.

use serde::Serialize;

/// What a dialog is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// "<app> wants to use the 'login' keychain. Please enter the keychain
    /// password": a locked keychain; the answer is its password.
    KeychainUnlock,
    /// "<app> wants to use your confidential information stored in
    /// '<item>' in your keychain" / "wants to access key '<item>'": an item
    /// ACL prompt (Allow / Always Allow / Deny), usually with a password field.
    KeychainAccess,
    /// "<app> wants to make changes. Enter your password": an admin
    /// authorization panel (user name + password).
    Authorization,
    /// "Keychain Not Found" / "unable to unlock your login keychain": every
    /// button destroys or replaces the keychain. Reported, never answered.
    KeychainBroken,
    /// Anything else. Reported, never answered.
    Unknown,
}

impl Kind {
    /// Stable name used in tool arguments and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::KeychainUnlock => "keychain_unlock",
            Kind::KeychainAccess => "keychain_access",
            Kind::Authorization => "authorization",
            Kind::KeychainBroken => "keychain_broken",
            Kind::Unknown => "unknown",
        }
    }

    /// The class a caller enables in `kinds`: `keychain` or `authorization`.
    pub fn class(self) -> Option<&'static str> {
        match self {
            Kind::KeychainUnlock | Kind::KeychainAccess => Some("keychain"),
            Kind::Authorization => Some("authorization"),
            _ => None,
        }
    }
}

/// One text field of a dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSnap {
    /// A password field (`AXSecureTextField`).
    pub secure: bool,
    /// Nothing typed yet.
    pub empty: bool,
}

/// What a dialog shows, flattened.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Static text, in tree order.
    pub texts: Vec<String>,
    /// Button titles (or descriptions when untitled).
    pub buttons: Vec<String>,
    /// Text fields.
    pub fields: Vec<FieldSnap>,
}

/// The decision for a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The classification.
    pub kind: Kind,
    /// Type the password into the password field first.
    pub fill_password: bool,
    /// Type the account name into an empty name field first.
    pub fill_user: bool,
    /// The button to press; `None` for kinds that are never answered.
    pub press: Option<String>,
}

fn has(buttons: &[String], wanted: &str) -> Option<String> {
    buttons
        .iter()
        .find(|b| b.trim().eq_ignore_ascii_case(wanted))
        .cloned()
}

/// Classifies `snap`.
pub fn classify(snap: &Snapshot) -> Plan {
    let text = snap.texts.join(" \n ").to_lowercase();
    let secure = snap.fields.iter().any(|f| f.secure);
    let plain_empty = snap.fields.iter().any(|f| !f.secure && f.empty);
    let none = |kind| Plan {
        kind,
        fill_password: false,
        fill_user: false,
        press: None,
    };

    // Destructive recovery alerts first: their buttons ("Reset To Defaults",
    // "Create New Keychain") must never be pressed on a guess.
    if has(&snap.buttons, "Reset To Defaults").is_some()
        || has(&snap.buttons, "Create New Keychain").is_some()
        || text.contains("unable to unlock your login keychain")
        || text.contains("keychain cannot be found")
        || text.contains("keychain could not be found")
    {
        return none(Kind::KeychainBroken);
    }

    let keychainy = text.contains("keychain")
        || text.contains("confidential information")
        || text.contains("access key");

    // Item ACL prompt: Allow / Always Allow / Deny. "Always Allow" so the
    // same app is not asked again for the item.
    let allow = has(&snap.buttons, "Always Allow").or_else(|| has(&snap.buttons, "Allow"));
    if keychainy && has(&snap.buttons, "Deny").is_some() {
        if let Some(button) = allow {
            return Plan {
                kind: Kind::KeychainAccess,
                fill_password: secure,
                fill_user: false,
                press: Some(button),
            };
        }
    }

    // Locked keychain: the password field plus "keychain" wording.
    if secure && keychainy {
        if let Some(button) = has(&snap.buttons, "OK") {
            return Plan {
                kind: Kind::KeychainUnlock,
                fill_password: true,
                fill_user: false,
                press: Some(button),
            };
        }
    }

    // Admin authorization panel.
    if secure
        && (text.contains("wants to make changes")
            || text.contains("enter your password")
            || text.contains("type your password")
            || text.contains("enter an administrator"))
    {
        const OK_BUTTONS: &[&str] = &[
            "OK",
            "Modify Settings",
            "Install Helper",
            "Install Software",
            "Allow",
            "Unlock",
            "Continue",
        ];
        let press = OK_BUTTONS.iter().find_map(|b| has(&snap.buttons, b));
        return Plan {
            kind: Kind::Authorization,
            fill_password: true,
            fill_user: plain_empty,
            press,
        };
    }

    none(Kind::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(texts: &[&str], buttons: &[&str], fields: &[(bool, bool)]) -> Snapshot {
        Snapshot {
            texts: texts.iter().map(|s| s.to_string()).collect(),
            buttons: buttons.iter().map(|s| s.to_string()).collect(),
            fields: fields
                .iter()
                .map(|&(secure, empty)| FieldSnap { secure, empty })
                .collect(),
        }
    }

    #[test]
    fn locked_keychain_dialog_measured_on_macos_26() {
        let s = snap(
            &[
                "security wants to use the \u{201c}probe\u{201d} keychain.",
                "Please enter the keychain password.",
                "Password:",
            ],
            &["Help", "OK", "Cancel"],
            &[(true, true)],
        );
        let plan = classify(&s);
        assert_eq!(plan.kind, Kind::KeychainUnlock);
        assert!(plan.fill_password);
        assert_eq!(plan.press.as_deref(), Some("OK"));
    }

    #[test]
    fn chrome_safe_storage_access_prompt() {
        let s = snap(
            &[
                "security wants to use your confidential information stored in \u{201c}Chrome Safe Storage\u{201d} in your keychain.",
                "To allow this, enter the \u{201c}login\u{201d} keychain password.",
            ],
            &["Always Allow", "Deny", "Allow"],
            &[(true, true)],
        );
        let plan = classify(&s);
        assert_eq!(plan.kind, Kind::KeychainAccess);
        assert!(plan.fill_password);
        assert_eq!(plan.press.as_deref(), Some("Always Allow"));
    }

    #[test]
    fn access_prompt_without_password_field() {
        let s = snap(
            &["Google Chrome wants to access key \u{201c}Chrome Safe Storage\u{201d} in your keychain."],
            &["Deny", "Allow"],
            &[],
        );
        let plan = classify(&s);
        assert_eq!(plan.kind, Kind::KeychainAccess);
        assert!(!plan.fill_password);
        assert_eq!(plan.press.as_deref(), Some("Allow"));
    }

    #[test]
    fn broken_login_keychain_is_never_answered() {
        let s = snap(
            &["The system was unable to unlock your login keychain."],
            &["Cancel", "Create New Keychain"],
            &[],
        );
        let plan = classify(&s);
        assert_eq!(plan.kind, Kind::KeychainBroken);
        assert_eq!(plan.press, None);
        let s = snap(
            &["x"],
            &["Reset To Defaults", "Cancel", "OK"],
            &[(true, true)],
        );
        assert_eq!(classify(&s).press, None);
    }

    #[test]
    fn authorization_panel() {
        let s = snap(
            &[
                "Cua wants to make changes.",
                "Enter your password to allow this.",
            ],
            &["Modify Settings", "Cancel"],
            &[(false, true), (true, true)],
        );
        let plan = classify(&s);
        assert_eq!(plan.kind, Kind::Authorization);
        assert!(plan.fill_user && plan.fill_password);
        assert_eq!(plan.press.as_deref(), Some("Modify Settings"));
    }

    #[test]
    fn unknown_is_left_alone() {
        let s = snap(&["Something else"], &["OK"], &[(true, true)]);
        let plan = classify(&s);
        assert_eq!(plan.kind, Kind::Unknown);
        assert_eq!(plan.press, None);
    }
}
