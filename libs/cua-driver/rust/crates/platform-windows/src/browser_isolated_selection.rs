//! Pure decision logic for choosing the Windows isolated-launch browser.
//!
//! The Windows adapter gathers facts about each trusted Chrome and Edge
//! installation (present, write access for the token that will run the
//! browser, vendor signature) and this module turns them into either a launch
//! choice or a refusal message. It has no Win32 dependencies so its unit
//! tests run on any host.
//!
//! The security rule: a candidate launches only when it is installed,
//! vendor-signed, and protected from the token that runs the browser. For a
//! non-elevated Driver that is the Driver's own token. An elevated Driver
//! runs the browser with a derived standard-user token instead (see
//! `browser_launch_token`), so protection is proven for that token. A signed
//! candidate that the launch token can still write gets a distinct message,
//! because "no vendor-signed executable" would wrongly imply the browser is
//! missing or unsigned.

use crate::browser_launch_token::BrowserLaunchToken;

/// Whether the browser launch token can modify a candidate's installation
/// tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallationWriteAccess {
    /// The executable and every ancestor up to the trusted root deny write
    /// access to the launch token.
    Protected,
    /// The launch token was granted a write-class right on the executable or
    /// an ancestor.
    WritableByLaunchToken,
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
    /// is `Protected` or `WritableByLaunchToken`; otherwise it is `false`.
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
    launch_token: BrowserLaunchToken,
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
            && candidate.write_access == InstallationWriteAccess::WritableByLaunchToken
            && !writable_products.contains(&candidate.product)
        {
            writable_products.push(candidate.product);
        }
    }
    if writable_products.is_empty() {
        return IsolatedBrowserDecision::Refuse(NO_PROTECTED_BROWSER_MESSAGE.to_owned());
    }
    let products = writable_products.join("/");
    IsolatedBrowserDecision::Refuse(match launch_token {
        BrowserLaunchToken::Driver => format!(
            "installed vendor-signed {products} is writable by the current non-elevated token, \
             so Cua Driver refuses to launch it for an isolated browser; restore the browser \
             installation's default permissions so that standard users cannot modify it"
        ),
        BrowserLaunchToken::StandardUser => format!(
            "installed vendor-signed {products} is writable even by the standard-user token that \
             the elevated Cua Driver uses for isolated browsers, so Cua Driver refuses to launch \
             it; restore the browser installation's default permissions so that standard users \
             cannot modify it"
        ),
    })
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

    fn decide(candidates: &[IsolatedCandidateFacts]) -> IsolatedBrowserDecision {
        decide_isolated_browser(candidates, BrowserLaunchToken::Driver)
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
            refusal_message(decide(&facts)),
            NO_PROTECTED_BROWSER_MESSAGE
        );
        assert_eq!(refusal_message(decide(&[])), NO_PROTECTED_BROWSER_MESSAGE);
    }

    #[test]
    fn installed_but_unsigned_candidate_keeps_generic_message() {
        let facts = [
            candidate("Chrome", InstallationWriteAccess::Protected, false),
            candidate(
                "Edge",
                InstallationWriteAccess::WritableByLaunchToken,
                false,
            ),
        ];
        assert_eq!(
            refusal_message(decide(&facts)),
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
            refusal_message(decide(&facts)),
            NO_PROTECTED_BROWSER_MESSAGE
        );
    }

    #[test]
    fn signed_but_writable_candidates_get_elevated_token_message() {
        let facts = [
            candidate(
                "Chrome",
                InstallationWriteAccess::WritableByLaunchToken,
                true,
            ),
            IsolatedCandidateFacts::missing("Chrome"),
            candidate("Edge", InstallationWriteAccess::WritableByLaunchToken, true),
            candidate("Edge", InstallationWriteAccess::WritableByLaunchToken, true),
        ];
        let message = refusal_message(decide(&facts));
        assert_ne!(message, NO_PROTECTED_BROWSER_MESSAGE);
        assert!(message.starts_with("installed vendor-signed Chrome/Edge is writable"));
        assert!(message.contains("current non-elevated token"));
        assert!(message.contains("default permissions"));

        let message = refusal_message(decide_isolated_browser(
            &facts,
            BrowserLaunchToken::StandardUser,
        ));
        assert!(message.starts_with("installed vendor-signed Chrome/Edge is writable"));
        assert!(message.contains("even by the standard-user token"));
        assert!(message.contains("default permissions"));
    }

    #[test]
    fn launch_token_does_not_change_which_candidate_is_launchable() {
        let facts = [
            IsolatedCandidateFacts::missing("Chrome"),
            candidate("Edge", InstallationWriteAccess::Protected, true),
        ];
        for token in [BrowserLaunchToken::Driver, BrowserLaunchToken::StandardUser] {
            assert_eq!(
                decide_isolated_browser(&facts, token),
                IsolatedBrowserDecision::Launch(1)
            );
        }
    }

    #[test]
    fn writable_message_names_only_signed_writable_products() {
        let facts = [
            candidate("Chrome", InstallationWriteAccess::Protected, false),
            candidate("Edge", InstallationWriteAccess::WritableByLaunchToken, true),
        ];
        let message = refusal_message(decide(&facts));
        assert!(message.starts_with("installed vendor-signed Edge is writable"));
    }

    #[test]
    fn one_protected_signed_candidate_wins_over_writable_ones() {
        let facts = [
            candidate(
                "Chrome",
                InstallationWriteAccess::WritableByLaunchToken,
                true,
            ),
            IsolatedCandidateFacts::missing("Chrome"),
            candidate("Edge", InstallationWriteAccess::Protected, true),
            candidate("Edge", InstallationWriteAccess::Protected, true),
        ];
        assert_eq!(decide(&facts), IsolatedBrowserDecision::Launch(2));
    }

    #[test]
    fn launchable_requires_every_condition() {
        assert!(candidate("Chrome", InstallationWriteAccess::Protected, true).launchable());
        assert!(!candidate("Chrome", InstallationWriteAccess::Protected, false).launchable());
        assert!(!candidate(
            "Chrome",
            InstallationWriteAccess::WritableByLaunchToken,
            true
        )
        .launchable());
        assert!(!candidate("Chrome", InstallationWriteAccess::Untrusted, true).launchable());
        let mut missing = candidate("Chrome", InstallationWriteAccess::Protected, true);
        missing.installed = false;
        assert!(!missing.launchable());
    }
}
