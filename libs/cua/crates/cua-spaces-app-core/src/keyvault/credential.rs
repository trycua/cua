// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Setting up and unlocking the Keyvault: which way the page offers (Touch
//! ID with the OS key store, or a passphrase) and the passphrase field's
//! hint. Pure: the broker's status decides the way, and nothing here keeps a
//! passphrase. The shells hold it only in their secure field and hand it to
//! the broker client (`client::KeyvaultCommands`), which sends it over the
//! verified Keyvault socket and nowhere else.

use serde::{Deserialize, Serialize};

use super::wire::KeyvaultOverview;

/// The shortest passphrase setup accepts (the broker's own minimum,
/// `cua_keyvault::protector::MIN_PASSPHRASE_CHARS`).
pub const MIN_PASSPHRASE_CHARS: u32 = 12;

/// Setting up a new vault or unlocking the existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KvFormMode {
    /// No vault yet.
    Setup,
    /// The vault is locked.
    Unlock,
}

/// How the vault key is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KvMethod {
    /// The OS key store (the login keychain), confirmed with Touch ID.
    TouchId,
    /// A passphrase.
    Passphrase,
}

/// The setup or unlock form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvCredentialForm {
    /// Setup or unlock.
    pub mode: KvFormMode,
    /// Touch ID or a passphrase.
    pub method: KvMethod,
    /// One line under the form.
    pub help: String,
    /// The passphrase field's label (passphrase only).
    pub passphrase_label: Option<String>,
    /// The second field's label (passphrase setup only).
    pub confirm_label: Option<String>,
    /// The button.
    pub submit_label: String,
}

/// The OS key store kinds `cua_keyvault::protector::ProtectorKind` names.
const OS_KINDS: [&str; 2] = ["macos-keychain", "windows-credential"];

/// The vault unlocks with a passphrase and not with the OS key store.
pub fn passphrase_only(s: &super::wire::KvStatus) -> bool {
    s.unlock_protectors.iter().any(|k| k == "passphrase")
        && !s
            .unlock_protectors
            .iter()
            .any(|k| OS_KINDS.contains(&k.as_str()))
}

/// The form the page shows: setup when there is no vault, unlock when it is
/// locked, else none. Touch ID when the daemon can use the OS key store
/// (setup) or the vault has an OS protector this daemon can use (unlock);
/// otherwise a passphrase.
pub fn credential_form(o: &KeyvaultOverview) -> Option<KvCredentialForm> {
    let s = o.status.as_ref();
    let labels = super::view::labels();
    match o.availability.as_str() {
        "no_vault" => {
            let os = s.is_some_and(|s| s.os_protector_available);
            Some(if os {
                KvCredentialForm {
                    mode: KvFormMode::Setup,
                    method: KvMethod::TouchId,
                    help: "Touch ID confirms. The vault key stays in your login keychain.".into(),
                    passphrase_label: None,
                    confirm_label: None,
                    submit_label: labels.set_up,
                }
            } else {
                KvCredentialForm {
                    mode: KvFormMode::Setup,
                    method: KvMethod::Passphrase,
                    help: format!(
                        "{MIN_PASSPHRASE_CHARS} or more characters. The recovery key is the only \
                         other way in."
                    ),
                    passphrase_label: Some("Passphrase".into()),
                    confirm_label: Some("Confirm passphrase".into()),
                    submit_label: labels.set_up,
                }
            })
        }
        "locked" => {
            let os = s.is_some_and(|s| {
                s.unlock_protectors
                    .iter()
                    .any(|k| OS_KINDS.contains(&k.as_str()))
            });
            Some(if os {
                KvCredentialForm {
                    mode: KvFormMode::Unlock,
                    method: KvMethod::TouchId,
                    help: "Unlocks with your login keychain.".into(),
                    passphrase_label: None,
                    confirm_label: None,
                    submit_label: labels.unlock,
                }
            } else {
                KvCredentialForm {
                    mode: KvFormMode::Unlock,
                    method: KvMethod::Passphrase,
                    help: "Enter the Keyvault passphrase.".into(),
                    passphrase_label: Some("Passphrase".into()),
                    confirm_label: None,
                    submit_label: labels.unlock,
                }
            })
        }
        _ => None,
    }
}

/// A rough passphrase strength, for the hint only (the broker enforces the
/// minimum length).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KvStrength {
    /// Too short, or easy to guess.
    Weak,
    /// Acceptable.
    Fair,
    /// Long and varied.
    Strong,
}

/// What the passphrase fields allow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvPassphraseCheck {
    /// The button is enabled.
    pub can_submit: bool,
    /// Setup only, once something is typed.
    pub strength: Option<KvStrength>,
    /// One short line under the fields.
    pub hint: Option<String>,
}

/// Estimated bits: the length times the bits of the character classes used,
/// halved when the characters repeat a lot. A hint, not a guarantee.
fn estimate_bits(p: &str) -> f64 {
    let (mut lower, mut upper, mut digit, mut other) = (false, false, false, false);
    let mut seen: Vec<char> = Vec::new();
    let mut len = 0usize;
    for c in p.chars() {
        len += 1;
        if c.is_ascii_lowercase() {
            lower = true;
        } else if c.is_ascii_uppercase() {
            upper = true;
        } else if c.is_ascii_digit() {
            digit = true;
        } else {
            other = true;
        }
        if !seen.contains(&c) {
            seen.push(c);
        }
    }
    let pool = [(lower, 26.0), (upper, 26.0), (digit, 10.0), (other, 33.0)]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| n)
        .sum::<f64>()
        .max(1.0);
    let bits = len as f64 * pool.log2();
    if seen.len() * 3 < len {
        bits / 2.0
    } else {
        bits
    }
}

/// The hint and whether the form can be sent. Setup needs the minimum
/// length and both fields equal; unlock needs anything typed.
pub fn passphrase_check(mode: KvFormMode, passphrase: &str, confirm: &str) -> KvPassphraseCheck {
    if mode == KvFormMode::Unlock {
        return KvPassphraseCheck {
            can_submit: !passphrase.is_empty(),
            strength: None,
            hint: None,
        };
    }
    if passphrase.is_empty() {
        return KvPassphraseCheck {
            can_submit: false,
            strength: None,
            hint: None,
        };
    }
    let long_enough = passphrase.chars().count() >= MIN_PASSPHRASE_CHARS as usize;
    let bits = estimate_bits(passphrase);
    let strength = if !long_enough || bits < 50.0 {
        KvStrength::Weak
    } else if bits < 75.0 {
        KvStrength::Fair
    } else {
        KvStrength::Strong
    };
    let mismatch = !confirm.is_empty() && confirm != passphrase;
    let hint = if !long_enough {
        format!("At least {MIN_PASSPHRASE_CHARS} characters")
    } else if mismatch {
        "The passphrases don't match".into()
    } else {
        match strength {
            KvStrength::Weak => "Weak: a few unrelated words are stronger",
            KvStrength::Fair => "Fair",
            KvStrength::Strong => "Strong",
        }
        .into()
    };
    KvPassphraseCheck {
        can_submit: long_enough && confirm == passphrase,
        strength: Some(strength),
        hint: Some(hint),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyvault::wire::KvStatus;

    fn overview(availability: &str, os: bool, unlock: &[&str]) -> KeyvaultOverview {
        KeyvaultOverview {
            availability: availability.into(),
            status: Some(KvStatus {
                version: "0".into(),
                initialized: availability != "no_vault",
                unlocked: availability == "ready",
                disabled: false,
                caller_first_party: true,
                caller_display: "Cua".into(),
                items: 0,
                pending: 0,
                unlock_policy: None,
                auto_wipe: None,
                os_protector_available: os,
                passphrase_available: true,
                unlock_protectors: unlock.iter().map(|s| s.to_string()).collect(),
                browse_until_ms: None,
                skip_unlock_prompt: None,
                reset_notice: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn setup_offers_touch_id_only_when_the_daemon_can_use_the_key_store() {
        let f = credential_form(&overview("no_vault", true, &[])).unwrap();
        assert_eq!((f.mode, f.method), (KvFormMode::Setup, KvMethod::TouchId));
        assert!(f.passphrase_label.is_none());
        let f = credential_form(&overview("no_vault", false, &[])).unwrap();
        assert_eq!(f.method, KvMethod::Passphrase);
        assert!(f.confirm_label.is_some());
        assert!(credential_form(&overview("ready", true, &["passphrase"])).is_none());
    }

    #[test]
    fn unlock_follows_the_enrolled_protectors() {
        let f =
            credential_form(&overview("locked", true, &["macos-keychain", "recovery"])).unwrap();
        assert_eq!(f.method, KvMethod::TouchId);
        let f = credential_form(&overview("locked", false, &["passphrase", "recovery"])).unwrap();
        assert_eq!(f.method, KvMethod::Passphrase);
        assert!(f.confirm_label.is_none());
    }

    #[test]
    fn the_check_needs_the_minimum_and_a_match() {
        let c = passphrase_check(KvFormMode::Setup, "short", "");
        assert!(!c.can_submit);
        assert_eq!(c.strength, Some(KvStrength::Weak));
        let p = "correct horse battery staple";
        let c = passphrase_check(KvFormMode::Setup, p, "correct horse");
        assert_eq!(c.hint.as_deref(), Some("The passphrases don't match"));
        assert!(!c.can_submit);
        let c = passphrase_check(KvFormMode::Setup, p, p);
        assert!(c.can_submit);
        assert_eq!(c.strength, Some(KvStrength::Strong));
        let c = passphrase_check(KvFormMode::Setup, "aaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaa");
        assert_eq!(c.strength, Some(KvStrength::Weak));
        assert!(c.can_submit, "the minimum is the only rule; weak is a hint");
        assert!(passphrase_check(KvFormMode::Unlock, "x", "").can_submit);
        assert!(!passphrase_check(KvFormMode::Unlock, "", "").can_submit);
    }
}
