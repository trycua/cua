// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Sign-in you can see: every state of the account's sign-in, in one place
//! (the sidebar's foot), in both shells.
//!
//! - Waiting: a spinner, "Waiting for your browser…", the code to confirm,
//!   "Open again" and "Cancel".
//! - Failed: "Sign-in didn't finish." with a short reason, "Try again", the
//!   error's own words behind "Details", and a toast, so it never fails
//!   silently.
//! - Signed in: "Signed in as <email>".
//!
//! A failure is typed ([`SignInFailure`]): the shells hand the core what
//! they caught (the SDK error's case and its words, [`failure_of`]) or the
//! reason they know ([`failed`]: the wait ran out), and the core picks the
//! words ([`reason_text`]). Every reason has words; a test fails if one is
//! added without them.

use serde::{Deserialize, Serialize};

use crate::settings::SignInPhase;

/// Why a sign-in did not finish.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignInFailure {
    /// The browser never confirmed: the wait ran out, or the code expired
    /// before it was confirmed.
    NotConfirmed,
    /// The person declined it in the browser.
    Declined,
    /// The sign-in service refused the code (expired or already used).
    CodeRejected,
    /// This computer could not reach the sign-in service.
    Offline,
    /// The sign-in service answered with its own error.
    ServiceUnavailable,
    /// Signed in, but the session could not be stored on this computer (a
    /// locked keychain, a credential store that refused the write).
    CouldNotSave,
    /// Anything else (the words are behind Details).
    #[default]
    Unknown,
}

impl SignInFailure {
    /// Every reason, for the words test and the shells' own tests.
    pub const ALL: [SignInFailure; 7] = [
        SignInFailure::NotConfirmed,
        SignInFailure::Declined,
        SignInFailure::CodeRejected,
        SignInFailure::Offline,
        SignInFailure::ServiceUnavailable,
        SignInFailure::CouldNotSave,
        SignInFailure::Unknown,
    ];
}

/// The line every failure starts with (also the toast's title).
pub const FAILED_TITLE: &str = "Sign-in didn't finish";

/// What the person can do about `reason`, in a few plain words (empty when
/// there is nothing to add to "Try again").
pub fn reason_text(reason: SignInFailure) -> &'static str {
    // Exhaustive on purpose: a new reason does not build without its words.
    match reason {
        SignInFailure::NotConfirmed => "The browser didn't confirm in time.",
        SignInFailure::Declined => "It was declined in the browser.",
        SignInFailure::CodeRejected => "The sign-in code expired or was already used.",
        SignInFailure::Offline => "Can't reach Cua. Check your connection.",
        SignInFailure::ServiceUnavailable => {
            "Cua's sign-in service had a problem. Try again in a minute."
        }
        SignInFailure::CouldNotSave => {
            "This computer couldn't save the sign-in. Unlock your keychain, then try again."
        }
        SignInFailure::Unknown => "",
    }
}

/// The failure's whole line: "Sign-in didn't finish." and the reason.
pub fn failure_line(reason: SignInFailure) -> String {
    match reason_text(reason) {
        "" => format!("{FAILED_TITLE}."),
        why => format!("{FAILED_TITLE}. {why}"),
    }
}

/// The fixed word usage data records for `reason` (`schema::ERROR_KINDS`).
pub fn error_kind(reason: SignInFailure) -> &'static str {
    match reason {
        SignInFailure::NotConfirmed => "timeout",
        SignInFailure::Declined => "permission_denied",
        SignInFailure::CodeRejected => "unauthenticated",
        SignInFailure::Offline => "transport",
        SignInFailure::ServiceUnavailable => "http",
        SignInFailure::CouldNotSave => "internal",
        SignInFailure::Unknown => "other",
    }
}

/// Longest Details text kept (the error's words, never shown by default).
const DETAIL_MAX: usize = 400;

/// The failed phase for `reason`, with the error's own words (`detail`) for
/// Details.
pub fn failed(reason: SignInFailure, detail: Option<&str>) -> SignInPhase {
    let detail = detail.map(str::trim).filter(|d| !d.is_empty()).map(|d| {
        if d.chars().count() > DETAIL_MAX {
            let cut: String = d.chars().take(DETAIL_MAX).collect();
            format!("{cut}\u{2026}")
        } else {
            d.to_string()
        }
    });
    SignInPhase::Failed {
        reason,
        message: failure_line(reason),
        detail,
    }
}

/// The failed phase for an error the shell caught: `variant` is the SDK
/// error's case (`Timeout`, `Unauthenticated`, `Http`, `Internal`, ...; empty
/// for any other error) and `message` its words.
pub fn failed_from_error(variant: &str, message: &str) -> SignInPhase {
    failed(failure_of(variant, message), Some(message))
}

/// The reason for an error from a sign-in (`cua_auth` through the SDK).
///
/// The SDK maps the sign-in's errors to its cases (`cua_auth::Error` ->
/// `CuaError`): `Timeout` (the browser did not confirm, the code expired),
/// `Unauthenticated` (the provider refused, or the person declined:
/// `cua_auth::LOGIN_DECLINED`), `Http` (unreachable:
/// `cua_auth::UNREACHABLE`, or the provider's own error), `Internal` (the
/// credential store: "credential store: ..."). The words decide only where
/// one case covers two reasons. `cua-auth`'s `sign_in_failures` test runs
/// every real failure through this and fails when they drift apart.
pub fn failure_of(variant: &str, message: &str) -> SignInFailure {
    let v: String = variant
        .chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let m = message.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| m.contains(w));
    if has(&["declined", "access_denied"]) || v == "cancelled" {
        return SignInFailure::Declined;
    }
    if has(&["credential store", "keychain", "credential vault"]) {
        return SignInFailure::CouldNotSave;
    }
    if v == "transport"
        || has(&[
            "could not reach",
            "error sending request",
            "connection refused",
            "network",
            "offline",
            "dns",
            "unreachable",
        ])
    {
        return SignInFailure::Offline;
    }
    if v == "timeout" || has(&["expired before", "not completed in time", "timed out"]) {
        return SignInFailure::NotConfirmed;
    }
    if has(&["could not start"]) {
        return SignInFailure::ServiceUnavailable;
    }
    if v == "unauthenticated" || has(&["invalid_grant", "expired_token"]) {
        return SignInFailure::CodeRejected;
    }
    if matches!(v.as_str(), "http" | "unsupported") || has(&["http 5", "discovery"]) {
        return SignInFailure::ServiceUnavailable;
    }
    SignInFailure::Unknown
}

/// What the sidebar's foot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignInFooterState {
    /// Not signed in: "Sign in".
    SignedOut,
    /// Asking for a code / opening the browser.
    Starting,
    /// The browser is open.
    Waiting,
    /// It failed: the reason and Try again.
    Failed,
    /// Signed in.
    SignedIn,
}

/// What the foot depends on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SignInFooterInput {
    /// The signed-in account.
    pub identity: Option<String>,
    /// The Cua account works through client credentials (an API key).
    pub cloud_configured: bool,
    /// Signing in is possible here.
    pub can_sign_in: bool,
    /// The sign-in in progress.
    pub sign_in: SignInPhase,
    /// The page the sign-in finishes on is known (Open again).
    pub has_url: bool,
}

/// A toast for a failure, so it is seen wherever the person is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInToast {
    /// "Sign-in didn't finish".
    pub title: String,
    /// The reason (none for an unknown one).
    pub body: Option<String>,
}

/// The sidebar's foot: one place for every sign-in state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInFooter {
    /// Which state.
    pub state: SignInFooterState,
    /// The line: "Signed in as …", "Waiting for your browser…", "Sign-in
    /// didn't finish. …", "Not signed in".
    pub text: String,
    /// The signed-in account (state `SignedIn`).
    pub identity: Option<String>,
    /// Waiting: the code to confirm in the browser.
    pub code: Option<String>,
    /// "Code".
    pub code_label: String,
    /// Waiting with a page: "Open again".
    pub reopen_label: Option<String>,
    /// Starting or waiting: "Cancel".
    pub cancel_label: Option<String>,
    /// "Sign in" or "Try again" (none while signed in or waiting).
    pub action_label: Option<String>,
    /// Failed: why.
    pub reason: Option<SignInFailure>,
    /// Failed: the error's own words, behind "Details".
    pub detail: Option<String>,
    /// "Details".
    pub details_label: String,
    /// Failed: the toast the shell shows once, when it fails.
    pub toast: Option<SignInToast>,
}

/// The sidebar's foot for `input`.
pub fn footer(input: &SignInFooterInput) -> SignInFooter {
    let mut f = SignInFooter {
        state: SignInFooterState::SignedOut,
        text: String::new(),
        identity: None,
        code: None,
        code_label: "Code".into(),
        reopen_label: None,
        cancel_label: None,
        action_label: None,
        reason: None,
        detail: None,
        details_label: "Details".into(),
        toast: None,
    };
    if let Some(id) = input.identity.clone().filter(|i| !i.is_empty()) {
        f.state = SignInFooterState::SignedIn;
        f.text = format!("Signed in as {id}");
        f.identity = Some(id);
        return f;
    }
    match &input.sign_in {
        SignInPhase::Starting => {
            f.state = SignInFooterState::Starting;
            f.text = "Opening your browser\u{2026}".into();
            f.cancel_label = Some("Cancel".into());
        }
        SignInPhase::Waiting { user_code, opened } => {
            f.state = SignInFooterState::Waiting;
            f.text = if *opened {
                "Waiting for your browser\u{2026}".into()
            } else {
                // No browser opened here: the page can be opened anywhere.
                "Open the sign-in page to continue.".into()
            };
            f.code = user_code.clone().filter(|c| !c.is_empty());
            f.reopen_label = input.has_url.then(|| "Open again".into());
            f.cancel_label = Some("Cancel".into());
        }
        SignInPhase::Failed {
            reason,
            message,
            detail,
        } => {
            f.state = SignInFooterState::Failed;
            f.text = if message.is_empty() {
                failure_line(*reason)
            } else {
                message.clone()
            };
            f.reason = Some(*reason);
            f.detail = detail.clone();
            f.action_label = Some("Try again".into());
            let why = reason_text(*reason);
            f.toast = Some(SignInToast {
                title: FAILED_TITLE.into(),
                body: (!why.is_empty()).then(|| why.into()),
            });
        }
        SignInPhase::Idle => {
            f.text = if input.cloud_configured {
                "Signed in with an API key".into()
            } else {
                "Not signed in".into()
            };
            f.action_label = input.can_sign_in.then(|| "Sign in".into());
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_has_plain_short_words_and_a_telemetry_kind() {
        for r in SignInFailure::ALL {
            let line = failure_line(r);
            assert!(line.starts_with("Sign-in didn't finish."), "{r:?}: {line}");
            assert!(line.len() <= 120, "{r:?} is too long: {line}");
            // Never a raw error: no protocol words, codes or quoting.
            for raw in ["HTTP", "error", "Error", "`", "{", "_", "OIDC", "token"] {
                assert!(!line.contains(raw), "{r:?} shows {raw:?}: {line}");
            }
            if r != SignInFailure::Unknown {
                assert!(!reason_text(r).is_empty(), "{r:?} has no reason");
            }
            let kind = error_kind(r);
            assert!(
                [
                    "timeout",
                    "permission_denied",
                    "unauthenticated",
                    "transport",
                    "http",
                    "internal",
                    "other"
                ]
                .contains(&kind),
                "{r:?}: {kind}"
            );
        }
        // Every reason is in ALL (the words test covers them all): the
        // match below does not build without a new one.
        for r in SignInFailure::ALL {
            let i = match r {
                SignInFailure::NotConfirmed => 0,
                SignInFailure::Declined => 1,
                SignInFailure::CodeRejected => 2,
                SignInFailure::Offline => 3,
                SignInFailure::ServiceUnavailable => 4,
                SignInFailure::CouldNotSave => 5,
                SignInFailure::Unknown => 6,
            };
            assert_eq!(SignInFailure::ALL[i], r);
        }
    }

    #[test]
    fn errors_map_to_reasons() {
        let cases = [
            (
                "Timeout",
                "device authorization expired before it was completed",
                SignInFailure::NotConfirmed,
            ),
            (
                "Timeout",
                "browser sign-in was not completed in time; try `cua auth login --remote`",
                SignInFailure::NotConfirmed,
            ),
            (
                "Timeout",
                "the sign-in code expired before it was confirmed",
                SignInFailure::NotConfirmed,
            ),
            (
                "Unauthenticated",
                "sign-in was declined in the browser: User denied",
                SignInFailure::Declined,
            ),
            (
                "Unauthenticated",
                "Code not valid",
                SignInFailure::CodeRejected,
            ),
            (
                "Unauthenticated",
                "invalid_grant",
                SignInFailure::CodeRejected,
            ),
            (
                "Http",
                "could not reach the sign-in service: error sending request for url (http://x)",
                SignInFailure::Offline,
            ),
            (
                "Http",
                "the sign-in service answered HTTP 503: unavailable",
                SignInFailure::ServiceUnavailable,
            ),
            (
                "Http",
                "OIDC discovery failed (HTTP 502)",
                SignInFailure::ServiceUnavailable,
            ),
            (
                "Unsupported",
                "browser sign-in is unavailable: x",
                SignInFailure::ServiceUnavailable,
            ),
            (
                "Internal",
                "credential store: User interaction is not allowed.",
                SignInFailure::CouldNotSave,
            ),
            ("Transport", "connection reset", SignInFailure::Offline),
            (
                "Closed",
                "this sign-in already finished",
                SignInFailure::Unknown,
            ),
            (
                "",
                "The sign-in timed out. Try again.",
                SignInFailure::NotConfirmed,
            ),
            ("", "something odd", SignInFailure::Unknown),
        ];
        for (variant, message, want) in cases {
            assert_eq!(failure_of(variant, message), want, "{variant}: {message}");
        }
    }

    #[test]
    fn the_failed_phase_keeps_the_words_for_details_only() {
        let p = failed_from_error("Http", "could not reach the sign-in service: dns error");
        let SignInPhase::Failed {
            reason,
            message,
            detail,
        } = p
        else {
            panic!()
        };
        assert_eq!(reason, SignInFailure::Offline);
        assert_eq!(
            message,
            "Sign-in didn't finish. Can't reach Cua. Check your connection."
        );
        assert_eq!(
            detail.as_deref(),
            Some("could not reach the sign-in service: dns error")
        );
        let long = "x".repeat(1000);
        let SignInPhase::Failed { detail, .. } = failed(SignInFailure::Unknown, Some(&long)) else {
            panic!()
        };
        assert_eq!(detail.unwrap().chars().count(), DETAIL_MAX + 1);
        let SignInPhase::Failed { detail, .. } = failed(SignInFailure::Unknown, Some("  ")) else {
            panic!()
        };
        assert_eq!(detail, None);
    }

    #[test]
    fn the_footer_shows_every_state_in_one_place() {
        let signed_out = footer(&SignInFooterInput {
            can_sign_in: true,
            ..Default::default()
        });
        assert_eq!(signed_out.state, SignInFooterState::SignedOut);
        assert_eq!(signed_out.text, "Not signed in");
        assert_eq!(signed_out.action_label.as_deref(), Some("Sign in"));
        assert!(signed_out.toast.is_none());

        let waiting = footer(&SignInFooterInput {
            sign_in: SignInPhase::Waiting {
                user_code: Some("4F2-91K".into()),
                opened: true,
            },
            has_url: true,
            can_sign_in: true,
            ..Default::default()
        });
        assert_eq!(waiting.state, SignInFooterState::Waiting);
        assert_eq!(waiting.text, "Waiting for your browser\u{2026}");
        assert_eq!(waiting.code.as_deref(), Some("4F2-91K"));
        assert_eq!(waiting.reopen_label.as_deref(), Some("Open again"));
        assert_eq!(waiting.cancel_label.as_deref(), Some("Cancel"));
        assert_eq!(waiting.action_label, None);

        let not_opened = footer(&SignInFooterInput {
            sign_in: SignInPhase::Waiting {
                user_code: None,
                opened: false,
            },
            has_url: true,
            ..Default::default()
        });
        assert_eq!(not_opened.text, "Open the sign-in page to continue.");
        assert_eq!(not_opened.code, None);

        let failed = footer(&SignInFooterInput {
            sign_in: failed(
                SignInFailure::NotConfirmed,
                Some("device authorization expired"),
            ),
            can_sign_in: true,
            ..Default::default()
        });
        assert_eq!(failed.state, SignInFooterState::Failed);
        assert_eq!(
            failed.text,
            "Sign-in didn't finish. The browser didn't confirm in time."
        );
        assert_eq!(failed.action_label.as_deref(), Some("Try again"));
        assert_eq!(
            failed.detail.as_deref(),
            Some("device authorization expired")
        );
        assert_eq!(
            failed.toast,
            Some(SignInToast {
                title: "Sign-in didn't finish".into(),
                body: Some("The browser didn't confirm in time.".into()),
            })
        );

        let signed_in = footer(&SignInFooterInput {
            identity: Some("ellie@example.com".into()),
            sign_in: failed.reason.map(|r| self::failed(r, None)).unwrap(),
            ..Default::default()
        });
        assert_eq!(signed_in.state, SignInFooterState::SignedIn);
        assert_eq!(signed_in.text, "Signed in as ellie@example.com");
        assert!(signed_in.toast.is_none());
    }

    #[test]
    fn a_failed_phase_from_an_older_shell_still_reads() {
        // A shell that only has words (no reason, no detail).
        let p: SignInPhase = serde_json::from_str(
            r#"{"kind":"failed","message":"The sign-in timed out. Try again."}"#,
        )
        .unwrap();
        assert_eq!(
            p,
            SignInPhase::Failed {
                reason: SignInFailure::Unknown,
                message: "The sign-in timed out. Try again.".into(),
                detail: None
            }
        );
        let w: SignInPhase = serde_json::from_str(r#"{"kind":"waiting","userCode":"AB"}"#).unwrap();
        assert_eq!(
            w,
            SignInPhase::Waiting {
                user_code: Some("AB".into()),
                opened: true
            }
        );
        let f = serde_json::to_value(failed(
            SignInFailure::CouldNotSave,
            Some("credential store: x"),
        ))
        .unwrap();
        assert_eq!(f["kind"], "failed");
        assert_eq!(f["reason"], "couldNotSave");
        assert_eq!(f["detail"], "credential store: x");
    }
}
