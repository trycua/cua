//! Pure decision logic for choosing the Windows isolated-launch browser.
//!
//! The Windows adapter gathers facts about each trusted Chrome and Edge
//! installation (present, write access for the current token, vendor
//! signature) and this module turns them into either a launch choice or a
//! refusal message. It has no Win32 dependencies so its unit tests run on any
//! host.
//!
//! The security rule is unchanged by the diagnostic: a candidate launches only
//! when it is installed, protected from the current token, and vendor-signed.
//! The distinct message exists because an elevated or built-in Administrator
//! token can write `Program Files`, so correctly signed browsers are refused
//! for that token. Reporting that case as "no vendor-signed executable"
//! wrongly implies the browser is missing or unsigned.

/// Whether the current token can modify a candidate's installation tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallationWriteAccess {
    /// The executable and every ancestor up to the trusted root deny write
    /// access to the current token.
    Protected,
    /// The current token was granted a write-class right on the executable or
    /// an ancestor (typical for elevated and built-in Administrator tokens).
    WritableByCurrentToken,
    /// The path is outside the trusted root or a write probe failed closed.
    Untrusted,
}

/// Facts about one isolated-launch candidate, in product-preference order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IsolatedCandidateFacts {
    /// Human-readable product name used in the refusal message.
    pub product: &'static str,
    /// The executable exists at its trusted path without redirection.
    pub installed: bool,
    pub write_access: InstallationWriteAccess,
    /// The executable carries the expected vendor Authenticode identity. The
    /// adapter evaluates this only for installed candidates whose write access
    /// is `Protected` or `WritableByCurrentToken`; otherwise it is `false`.
    pub vendor_signed: bool,
}

impl IsolatedCandidateFacts {
    pub(crate) fn missing(product: &'static str) -> Self {
        Self {
            product,
            installed: false,
            write_access: InstallationWriteAccess::Untrusted,
            vendor_signed: false,
        }
    }

    /// Whether this candidate satisfies every isolated-launch requirement.
    pub(crate) fn launchable(&self) -> bool {
        self.installed
            && self.write_access == InstallationWriteAccess::Protected
            && self.vendor_signed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IsolatedBrowserDecision {
    /// Launch the candidate at this index.
    Launch(usize),
    /// Refuse with `browser_route_unavailable` and this message.
    Refuse(String),
}

pub(crate) const NO_PROTECTED_BROWSER_MESSAGE: &str =
    "no vendor-signed protected Chromium executable is available for isolated launch";

/// Choose the first launchable candidate, or explain why none qualifies.
pub(crate) fn decide_isolated_browser(
    candidates: &[IsolatedCandidateFacts],
) -> IsolatedBrowserDecision {
    if let Some(index) = candidates
        .iter()
        .position(IsolatedCandidateFacts::launchable)
    {
        return IsolatedBrowserDecision::Launch(index);
    }
    let mut writable_products: Vec<&str> = Vec::new();
    for candidate in candidates {
        if candidate.installed
            && candidate.vendor_signed
            && candidate.write_access == InstallationWriteAccess::WritableByCurrentToken
            && !writable_products.contains(&candidate.product)
        {
            writable_products.push(candidate.product);
        }
    }
    if writable_products.is_empty() {
        return IsolatedBrowserDecision::Refuse(NO_PROTECTED_BROWSER_MESSAGE.to_owned());
    }
    IsolatedBrowserDecision::Refuse(format!(
        "installed vendor-signed {} is writable by the current elevated or administrator token, \
         so Cua Driver refuses to launch it for an isolated browser; run Cua Driver from a \
         non-administrator, non-elevated session (the built-in Administrator account is always \
         elevated)",
        writable_products.join("/"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        product: &'static str,
        write_access: InstallationWriteAccess,
        vendor_signed: bool,
    ) -> IsolatedCandidateFacts {
        IsolatedCandidateFacts {
            product,
            installed: true,
            write_access,
            vendor_signed,
        }
    }

    fn refusal_message(decision: IsolatedBrowserDecision) -> String {
        match decision {
            IsolatedBrowserDecision::Refuse(message) => message,
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn no_installed_candidate_keeps_generic_message() {
        let facts = [
            IsolatedCandidateFacts::missing("Chrome"),
            IsolatedCandidateFacts::missing("Edge"),
        ];
        assert_eq!(
            refusal_message(decide_isolated_browser(&facts)),
            NO_PROTECTED_BROWSER_MESSAGE
        );
        assert_eq!(
            refusal_message(decide_isolated_browser(&[])),
            NO_PROTECTED_BROWSER_MESSAGE
        );
    }

    #[test]
    fn installed_but_unsigned_candidate_keeps_generic_message() {
        let facts = [
            candidate("Chrome", InstallationWriteAccess::Protected, false),
            candidate(
                "Edge",
                InstallationWriteAccess::WritableByCurrentToken,
                false,
            ),
        ];
        assert_eq!(
            refusal_message(decide_isolated_browser(&facts)),
            NO_PROTECTED_BROWSER_MESSAGE
        );
    }

    #[test]
    fn untrusted_location_keeps_generic_message() {
        let facts = [candidate(
            "Chrome",
            InstallationWriteAccess::Untrusted,
            false,
        )];
        assert_eq!(
            refusal_message(decide_isolated_browser(&facts)),
            NO_PROTECTED_BROWSER_MESSAGE
        );
    }

    #[test]
    fn signed_but_writable_candidates_get_elevated_token_message() {
        let facts = [
            candidate(
                "Chrome",
                InstallationWriteAccess::WritableByCurrentToken,
                true,
            ),
            IsolatedCandidateFacts::missing("Chrome"),
            candidate(
                "Edge",
                InstallationWriteAccess::WritableByCurrentToken,
                true,
            ),
            candidate(
                "Edge",
                InstallationWriteAccess::WritableByCurrentToken,
                true,
            ),
        ];
        let message = refusal_message(decide_isolated_browser(&facts));
        assert_ne!(message, NO_PROTECTED_BROWSER_MESSAGE);
        assert!(message.starts_with("installed vendor-signed Chrome/Edge is writable"));
        assert!(message.contains("elevated or administrator token"));
        assert!(message.contains("non-administrator"));
    }

    #[test]
    fn writable_message_names_only_signed_writable_products() {
        let facts = [
            candidate("Chrome", InstallationWriteAccess::Protected, false),
            candidate(
                "Edge",
                InstallationWriteAccess::WritableByCurrentToken,
                true,
            ),
        ];
        let message = refusal_message(decide_isolated_browser(&facts));
        assert!(message.starts_with("installed vendor-signed Edge is writable"));
    }

    #[test]
    fn one_protected_signed_candidate_wins_over_writable_ones() {
        let facts = [
            candidate(
                "Chrome",
                InstallationWriteAccess::WritableByCurrentToken,
                true,
            ),
            IsolatedCandidateFacts::missing("Chrome"),
            candidate("Edge", InstallationWriteAccess::Protected, true),
            candidate("Edge", InstallationWriteAccess::Protected, true),
        ];
        assert_eq!(
            decide_isolated_browser(&facts),
            IsolatedBrowserDecision::Launch(2)
        );
    }

    #[test]
    fn launchable_requires_every_condition() {
        assert!(candidate("Chrome", InstallationWriteAccess::Protected, true).launchable());
        assert!(!candidate("Chrome", InstallationWriteAccess::Protected, false).launchable());
        assert!(!candidate(
            "Chrome",
            InstallationWriteAccess::WritableByCurrentToken,
            true
        )
        .launchable());
        assert!(!candidate("Chrome", InstallationWriteAccess::Untrusted, true).launchable());
        let mut missing = candidate("Chrome", InstallationWriteAccess::Protected, true);
        missing.installed = false;
        assert!(!missing.launchable());
    }
}
