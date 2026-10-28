// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS Local Network access before a macOS Space is created on this Mac:
//! what the app checks, says and does, for every shell.
//!
//! A macOS VM lives on Apple's VM network inside the Mac, and the app's
//! daemon (which macOS counts as the app) can only reach it once the app is
//! allowed under System Settings › Privacy & Security › Local Network.
//! Left alone, macOS asks at a random moment (or the app's first connect),
//! and a create without access waited two minutes before failing. So before
//! the create the shell runs its platform check (a connect from the app's
//! own process, and the routing table) and asks [`step`] what to do:
//!
//! - **Proceed**: create.
//! - **Explain**: access is not allowed yet and the app has not explained
//!   it: show [`LocalNetworkStep`]'s title and message with Continue and
//!   Cancel, then make macOS ask (the same connect), wait for the answer,
//!   and call [`step`] again with `asked` set.
//! - **Fail**: stop at once with the message (the create's error, its
//!   `error_variant` for telemetry) and, for missing access, a button that
//!   opens the Local Network settings.
//!
//! The decision and the words are `cua_create_failure`'s (the daemon says
//! the same when a create fails the same way).

use cua_create_failure::local_network::{self as ln, Access, Gate};
use cua_create_failure::preflight::{self, IssueKind, Pane};
use serde::{Deserialize, Serialize};

use crate::model::{SpaceOs, SpaceProvider};

/// The app's name in the sentences (what System Settings lists).
pub const APP: &str = "Cua Spaces";

/// The explanation's title.
pub const EXPLAIN_TITLE: &str = "Allow Cua Spaces on your local network";
/// The explanation, shown once before macOS asks.
pub const EXPLAIN_MESSAGE: &str = "macOS Spaces run on a small network inside this Mac. To \
     connect to them, Cua Spaces needs Local Network access. If macOS asks, click Allow.";
/// The explanation's buttons.
pub const CONTINUE: &str = "Continue";
pub const CANCEL: &str = "Cancel";
/// The failure's title.
pub const FAIL_TITLE: &str = "Can't connect to macOS Spaces";
/// The failure's buttons: open the settings, or close.
pub const OPEN_SETTINGS: &str = "Open Settings";
pub const OK: &str = "OK";

/// What the shell's platform check found from the app's own process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalNetworkAccess {
    /// A connect to the local network went out.
    Allowed,
    /// macOS blocked it (not allowed yet, or turned off).
    Blocked,
    /// No local network to try, or no clear answer.
    #[default]
    Unknown,
    /// Not probed: the app has not explained yet, and a connect while access
    /// is undecided makes macOS ask before the explanation.
    NotChecked,
}

impl From<LocalNetworkAccess> for Access {
    fn from(a: LocalNetworkAccess) -> Self {
        match a {
            LocalNetworkAccess::Allowed => Access::Allowed,
            LocalNetworkAccess::Blocked => Access::Blocked,
            LocalNetworkAccess::Unknown | LocalNetworkAccess::NotChecked => Access::Unknown,
        }
    }
}

/// What the shell found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalNetworkInput {
    /// The app's own connect.
    pub access: LocalNetworkAccess,
    /// The app already explained and made macOS ask (saved by the shell).
    pub asked: bool,
    /// `netstat -rn -f inet`, or empty when it could not be read.
    pub routes: String,
}

/// What to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalNetworkStepKind {
    #[default]
    Proceed,
    Explain,
    Fail,
}

/// The step, in the words to show.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalNetworkStep {
    pub kind: LocalNetworkStepKind,
    /// The dialog's title (none for Proceed).
    pub title: String,
    /// The dialog's text; for Fail also the create's error.
    pub message: String,
    /// The main button: Continue, Open Settings or OK.
    pub primary: String,
    /// The other button: Cancel or OK; empty when there is one button.
    pub secondary: String,
    /// What the main button opens (Fail for missing access).
    pub settings_url: Option<String>,
    /// For Fail: the create's error variant (`local_network_denied`,
    /// `local_network_vpn`; telemetry's `error_subclass`).
    pub error_variant: String,
}

/// Whether a create needs the check: a macOS Space on this Mac, when this
/// Mac is a Mac.
pub fn applies(provider: SpaceProvider, os: SpaceOs, host_is_mac: bool) -> bool {
    host_is_mac && os == SpaceOs::Macos && provider == SpaceProvider::Local
}

/// The step for what the shell found.
pub fn step(input: &LocalNetworkInput) -> LocalNetworkStep {
    // Not explained yet: explain before anything makes macOS ask.
    if input.access == LocalNetworkAccess::NotChecked && !input.asked {
        return explain();
    }
    let vpn = ln::vmnet_tunnel_route(&input.routes);
    match ln::gate(input.access.into(), input.asked, vpn.as_deref()) {
        Gate::Proceed => LocalNetworkStep::default(),
        Gate::Explain => explain(),
        Gate::Fail(kind) => fail(kind),
    }
}

fn explain() -> LocalNetworkStep {
    LocalNetworkStep {
        kind: LocalNetworkStepKind::Explain,
        title: EXPLAIN_TITLE.into(),
        message: EXPLAIN_MESSAGE.into(),
        primary: CONTINUE.into(),
        secondary: CANCEL.into(),
        ..Default::default()
    }
}

/// The Fail step for `kind`.
fn fail(kind: IssueKind) -> LocalNetworkStep {
    let variant = kind
        .error_subclass()
        .unwrap_or(kind.error_kind())
        .to_string();
    if kind == IssueKind::LocalNetworkVpn {
        return LocalNetworkStep {
            kind: LocalNetworkStepKind::Fail,
            title: FAIL_TITLE.into(),
            message: preflight::LOCAL_NETWORK_VPN.into(),
            primary: OK.into(),
            error_variant: variant,
            ..Default::default()
        };
    }
    let issue = preflight::local_network_issue(APP);
    LocalNetworkStep {
        kind: LocalNetworkStepKind::Fail,
        title: FAIL_TITLE.into(),
        message: issue.message,
        primary: OPEN_SETTINGS.into(),
        secondary: OK.into(),
        settings_url: Pane::LocalNetwork.url().map(Into::into),
        error_variant: variant,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(access: LocalNetworkAccess, asked: bool, routes: &str) -> LocalNetworkInput {
        LocalNetworkInput {
            access,
            asked,
            routes: routes.into(),
        }
    }

    #[test]
    fn only_a_macos_space_on_this_mac_is_checked() {
        assert!(applies(SpaceProvider::Local, SpaceOs::Macos, true));
        assert!(!applies(SpaceProvider::Local, SpaceOs::Macos, false));
        assert!(!applies(SpaceProvider::Local, SpaceOs::Linux, true));
        assert!(!applies(SpaceProvider::Relay, SpaceOs::Macos, true));
    }

    #[test]
    fn the_first_time_explains_before_any_probe() {
        let s = step(&input(LocalNetworkAccess::NotChecked, false, ""));
        assert_eq!(s.kind, LocalNetworkStepKind::Explain);
        assert!(
            s.message.ends_with("If macOS asks, click Allow."),
            "{}",
            s.message
        );
        // Explained before: a shell that did not probe goes ahead.
        assert_eq!(
            step(&input(LocalNetworkAccess::NotChecked, true, "")).kind,
            LocalNetworkStepKind::Proceed
        );
    }

    #[test]
    fn allowed_goes_ahead() {
        assert_eq!(
            step(&input(LocalNetworkAccess::Allowed, false, "")).kind,
            LocalNetworkStepKind::Proceed
        );
        assert_eq!(
            step(&input(LocalNetworkAccess::Unknown, true, "")).kind,
            LocalNetworkStepKind::Proceed
        );
    }

    #[test]
    fn not_allowed_is_explained_once_then_fails_with_the_fix() {
        let first = step(&input(LocalNetworkAccess::Blocked, false, ""));
        assert_eq!(first.kind, LocalNetworkStepKind::Explain);
        assert_eq!(first.title, "Allow Cua Spaces on your local network");
        assert_eq!(
            (first.primary.as_str(), first.secondary.as_str()),
            ("Continue", "Cancel")
        );
        assert!(first.message.contains("click Allow"), "{}", first.message);

        let then = step(&input(LocalNetworkAccess::Blocked, true, ""));
        assert_eq!(then.kind, LocalNetworkStepKind::Fail);
        assert_eq!(
            then.message,
            "Cua Spaces can't connect to macOS Spaces on this Mac. Allow Cua Spaces in System \
             Settings \u{203a} Privacy & Security \u{203a} Local Network, then try again."
        );
        assert_eq!(then.primary, "Open Settings");
        assert_eq!(
            then.settings_url.as_deref(),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")
        );
        assert_eq!(then.error_variant, "local_network_denied");
        // The create's error reads back as the same cause, kind and subclass.
        assert_eq!(
            crate::spaces::creating::failure_variant(
                &then.message,
                Some("preparing"),
                false,
                &then.error_variant,
            ),
            "local_network_denied"
        );
        assert_eq!(
            crate::spaces::creating::failure_text(&then.message, Some("preparing"), None, false),
            then.message
        );
    }

    #[test]
    fn a_vpn_route_over_the_vm_network_fails_with_its_own_words() {
        let routes = "Destination        Gateway            Flags               Netif Expire\n\
                      192.168.64/24      10.8.0.1           UGSc                utun4\n";
        let s = step(&input(LocalNetworkAccess::Allowed, false, routes));
        assert_eq!(s.kind, LocalNetworkStepKind::Fail);
        assert!(
            s.message
                .starts_with("A VPN or firewall is blocking this Mac's own network")
        );
        assert_eq!(s.primary, "OK");
        assert!(s.settings_url.is_none());
        assert_eq!(s.error_variant, "local_network_vpn");
        assert_eq!(
            crate::spaces::creating::failure_text(&s.message, Some("preparing"), None, false),
            s.message
        );
        // A full-tunnel VPN does not block it.
        let wide = "default            10.8.0.1           UGScg               utun4\n";
        assert_eq!(
            step(&input(LocalNetworkAccess::Allowed, false, wide)).kind,
            LocalNetworkStepKind::Proceed
        );
    }
}
