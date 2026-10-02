// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! OS biometric-authorization gate for **sensitive** session exports.
//!
//! The export runs on the end user's own machine, so any code path that reads a
//! `sensitive: true` [`crate::ManifestItem`] and packs it into a
//! [`cua_teleport_bundle::bundle::SessionBundle`] must first obtain interactive OS user
//! authorization. This prevents a hijacked or misused SDK from silently
//! exfiltrating a user's credentials (for example a Claude Code OAuth token)
//! off their computer.
//!
//! The gate is enforced inside this library — at the export choke point in the
//! [`crate::ExportProvider`] trait — so the CLI and any embedder are both
//! covered, not just a consent UI.
//!
//! ## Platform behavior
//!
//! - **macOS**: prompt via the LocalAuthentication framework
//!   (`LAContext` / `evaluatePolicy:localizedReason:reply:`) using
//!   `LAPolicyDeviceOwnerAuthentication` — Touch ID with a device-passcode
//!   fallback, the robust choice for an unsigned CLI.
//! - **Other platforms**: fail **closed** — [`AuthError::Unavailable`] — unless
//!   the operator sets `CUA_ENV_ALLOW_UNVERIFIED_SENSITIVE_EXPORT=1`, an explicit
//!   override that logs to stderr and proceeds. This keeps Linux/Windows from
//!   silently exfiltrating while not hard-breaking deliberate use.
//!
//! ## Escape hatch (test/CI only)
//!
//! Setting `CUA_ENV_SKIP_SENSITIVE_AUTH=1` bypasses the gate entirely (returns
//! `Ok`). It exists so unit tests never raise an interactive prompt; it must not
//! be set in production.

use std::collections::HashSet;

use cua_teleport_bundle::ManifestItem;

/// Env var that bypasses the gate entirely (returns `Ok`). **Test/CI only** —
/// documented so unit tests never raise an interactive prompt.
pub const SKIP_ENV: &str = "CUA_ENV_SKIP_SENSITIVE_AUTH";

/// Env var that, on non-macOS platforms, lets a sensitive export proceed
/// without OS authorization. An explicit operator override; when set, the gate
/// logs to stderr that it proceeded unverified.
pub const ALLOW_UNVERIFIED_ENV: &str = "CUA_ENV_ALLOW_UNVERIFIED_SENSITIVE_EXPORT";

/// Why an interactive authorization did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// The user (or the OS) refused: wrong biometric/passcode, or the policy
    /// evaluation failed for a non-cancel reason.
    Denied,
    /// No authorization mechanism is available on this platform/build (for
    /// example a non-macOS host without the operator override, or macOS with no
    /// Touch ID and no passcode set).
    Unavailable,
    /// The user actively dismissed the prompt, or the OS cancelled it.
    Cancelled,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => write!(f, "biometric authorization was denied"),
            Self::Unavailable => write!(
                f,
                "biometric authorization is unavailable on this platform (set {ALLOW_UNVERIFIED_ENV}=1 to override on non-macOS)"
            ),
            Self::Cancelled => write!(f, "biometric authorization was cancelled"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Whether the current selection would pack at least one `sensitive: true`
/// manifest item into the bundle.
///
/// `include == None` means "everything the scope implies" (every manifest
/// item); a set restricts the selection to the checked `rel_path`s, matching
/// the export choke point's semantics. Pure, so the gate wiring is unit-testable
/// without touching the OS or any env var.
pub fn selection_is_sensitive(items: &[ManifestItem], include: Option<&HashSet<String>>) -> bool {
    items
        .iter()
        .filter(|item| include.is_none_or(|set| set.contains(&item.rel_path)))
        .any(|item| item.sensitive)
}

/// Obtain interactive OS user authorization before a sensitive session export
/// leaves this machine.
///
/// `reason` is shown to the user (on macOS it fills the LocalAuthentication
/// prompt's "…is trying to <reason>" line). Returns `Ok(())` when authorized and
/// an [`AuthError`] otherwise; callers MUST abort the export on `Err`.
///
/// Honors `CUA_ENV_SKIP_SENSITIVE_AUTH=1` (test/CI escape hatch) first, before any
/// platform-specific work, so tests never prompt.
///
/// The bypass is honored only in debug/test builds. A released (optimized)
/// binary compiles with `debug_assertions` off, so [`skip_is_honored`] returns
/// false there regardless of the environment: a shipped Cua has no bypass
/// (red-team E5). Under `cargo test` the crate is built with `debug_assertions`
/// on, so unit tests still never raise an interactive prompt.
pub fn authorize_sensitive_export(reason: &str) -> Result<(), AuthError> {
    authorize_unless_skipped(
        skip_is_honored(cfg!(debug_assertions), env_flag(SKIP_ENV)),
        reason,
    )
}

/// Whether the `CUA_ENV_SKIP_SENSITIVE_AUTH` escape hatch takes effect.
///
/// Pure, so a test can assert that in a non-debug (released) build path the
/// environment variable has no effect at all. `debug_build` is normally
/// `cfg!(debug_assertions)`.
pub fn skip_is_honored(debug_build: bool, env_set: bool) -> bool {
    debug_build && env_set
}

/// The gate with the escape hatch passed in, so it is testable without
/// mutating process-global environment variables.
fn authorize_unless_skipped(skip: bool, reason: &str) -> Result<(), AuthError> {
    if skip {
        return Ok(());
    }
    authorize_platform(reason)
}

/// Read a boolean env var: true only when set to exactly `"1"`.
fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| value == "1")
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn authorize_platform(reason: &str) -> Result<(), AuthError> {
    macos::authorize(reason)
}

#[cfg(not(target_os = "macos"))]
fn authorize_platform(reason: &str) -> Result<(), AuthError> {
    non_macos_decision(env_flag(ALLOW_UNVERIFIED_ENV), reason)
}

/// Fail-closed decision for platforms without a supported biometric backend.
///
/// Split out (and pure) so it can be unit-tested without mutating process env.
/// With the operator override it logs to stderr and proceeds; otherwise it
/// refuses so the sensitive data never leaves the machine unverified.
#[cfg(not(target_os = "macos"))]
fn non_macos_decision(allow_unverified: bool, reason: &str) -> Result<(), AuthError> {
    if allow_unverified {
        eprintln!(
            "cua-spacesd-client: WARNING: sensitive session export proceeding WITHOUT OS biometric \
             authorization because {ALLOW_UNVERIFIED_ENV}=1 is set ({reason})"
        );
        Ok(())
    } else {
        Err(AuthError::Unavailable)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    //! macOS LocalAuthentication backend.
    //!
    //! `evaluatePolicy:localizedReason:reply:` is asynchronous: it returns
    //! immediately and invokes the completion block on a framework-private
    //! queue. We bridge that back to a synchronous call by handing the block an
    //! `mpsc::Sender` and blocking the caller on the receiver until the reply
    //! arrives. The `LAContext` is held on the stack across that wait so the
    //! evaluation is not cancelled by the context being deallocated.

    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    use super::AuthError;

    /// Upper bound on how long we wait for the user to answer the prompt before
    /// treating it as cancelled, so a forgotten dialog cannot wedge the export
    /// forever.
    const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

    pub(super) fn authorize(reason: &str) -> Result<(), AuthError> {
        // Touch ID OR device passcode fallback (LAPolicyDeviceOwnerAuthentication,
        // value 2): the robust choice for an unsigned CLI that may run where no
        // biometric is enrolled but a passcode is set.
        let policy = LAPolicy::DeviceOwnerAuthentication;

        // SAFETY: `LAContext::new` returns a fresh, owned context.
        let context = unsafe { LAContext::new() };

        // Preflight: if the policy can't be evaluated at all (no passcode, no
        // biometric enrolled, unsupported hardware) treat it as unavailable
        // rather than raising a prompt that is guaranteed to fail.
        // SAFETY: `context` is a valid LAContext; the call only reads state.
        if unsafe { context.canEvaluatePolicy_error(policy) }.is_err() {
            return Err(AuthError::Unavailable);
        }

        let ns_reason = NSString::from_str(reason);
        let (tx, rx) = mpsc::channel::<Result<(), AuthError>>();

        // The reply block runs on a private framework queue in an unspecified
        // thread; `Sender` is `Send`, so handing the result back over the channel
        // is sound. `error` is only borrowed for the duration of the block.
        let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let result = if success.as_bool() {
                Ok(())
            } else {
                // SAFETY: on failure LocalAuthentication passes a valid NSError;
                // guard against null defensively.
                let code = if error.is_null() {
                    0
                } else {
                    unsafe { (*error).code() }
                };
                Err(classify(code))
            };
            let _ = tx.send(result);
        });

        // SAFETY: all arguments are valid; the reply block is kept alive by
        // `reply` until after the evaluation completes (we block on `rx` below,
        // and `reply`/`context` outlive that wait).
        unsafe {
            context.evaluatePolicy_localizedReason_reply(policy, &ns_reason, &reply);
        }

        match rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(result) => result,
            // No reply within the budget: treat as cancelled rather than a
            // silent success.
            Err(_) => Err(AuthError::Cancelled),
        }
    }

    /// Map an `LAError` code to an [`AuthError`]. User/OS cancellations are
    /// distinguished from an outright denial so callers can message accordingly.
    fn classify(code: isize) -> AuthError {
        // LAErrorUserCancel = -2, LAErrorSystemCancel = -4, LAErrorAppCancel = -9.
        match code {
            -2 | -4 | -9 => AuthError::Cancelled,
            _ => AuthError::Denied,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(rel: &str, sensitive: bool) -> ManifestItem {
        ManifestItem {
            label: rel.to_string(),
            rel_path: rel.to_string(),
            est_bytes: 0,
            count: None,
            count_noun: None,
            sensitive,
            default_checked: true,
        }
    }

    #[test]
    fn selection_none_is_sensitive_when_any_item_is() {
        let items = vec![item("a", false), item("secret", true)];
        // `None` selects everything, so the sensitive item counts.
        assert!(selection_is_sensitive(&items, None));
    }

    #[test]
    fn selection_only_nonsensitive_items_is_not_sensitive() {
        let items = vec![item("a", false), item("secret", true)];
        // Include only the non-sensitive item: no auth required.
        let include: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert!(!selection_is_sensitive(&items, Some(&include)));
    }

    #[test]
    fn selection_including_a_sensitive_item_is_sensitive() {
        let items = vec![item("a", false), item("secret", true)];
        let include: HashSet<String> = ["secret".to_string()].into_iter().collect();
        assert!(selection_is_sensitive(&items, Some(&include)));
    }

    #[test]
    fn empty_items_never_sensitive() {
        assert!(!selection_is_sensitive(&[], None));
    }

    #[test]
    fn skip_bypasses_the_gate() {
        // The test/CI escape hatch must short-circuit before any platform work,
        // so this never raises a real prompt (including on macOS dev machines).
        assert!(authorize_unless_skipped(true, "test reason").is_ok());
    }

    #[test]
    fn skip_env_has_no_effect_in_a_released_build() {
        // Red-team E5: a released (non-debug) binary must have NO bypass. Even
        // with the env var set, the skip is not honored when debug_assertions
        // is off, so a shipped Cua always runs the real OS authorization gate.
        assert!(
            !skip_is_honored(false, true),
            "released build must ignore the bypass env"
        );
        assert!(!skip_is_honored(false, false));
        // In a debug/test build the escape hatch still works so tests never prompt.
        assert!(skip_is_honored(true, true));
        assert!(!skip_is_honored(true, false));
    }

    #[test]
    fn auth_error_displays_are_distinct() {
        assert_ne!(
            AuthError::Denied.to_string(),
            AuthError::Unavailable.to_string()
        );
        assert_ne!(
            AuthError::Denied.to_string(),
            AuthError::Cancelled.to_string()
        );
    }

    // Fail-closed behavior for non-macOS hosts. Tested via the pure decision
    // function with explicit inputs so there is no dependency on (or race over)
    // process-global env vars.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_fails_closed_without_override() {
        assert_eq!(
            non_macos_decision(false, "reason"),
            Err(AuthError::Unavailable)
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_override_allows_export() {
        assert_eq!(non_macos_decision(true, "reason"), Ok(()));
    }
}
