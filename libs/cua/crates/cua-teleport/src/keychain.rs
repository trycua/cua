// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS Keychain *reads* for session teleport (sender side).
//!
//! Chromium/Electron apps (Chrome, Slack, Discord, …) encrypt their local
//! cookies and auth tokens with an app-specific **"Safe Storage"** key held in
//! the login Keychain (`<App> Safe Storage`). Copying the profile files alone
//! lands an *encrypted* session the destination cannot read — so a teleport
//! that preserves the login must also carry that Keychain key.
//!
//! [`probe_generic`] extracts a generic-password item on the source (macOS
//! only, through the injected [`HostEffects`]); providers pack the items into
//! the reserved [`KEYCHAIN_ENTRY`] with [`serialize`]; the receiver
//! (`cua-spacesd-teleport`) installs them. Non-macOS hosts read nothing.

pub use cua_teleport_bundle::keychain::{KEYCHAIN_ENTRY, KeychainItem, serialize};

use crate::host::HostEffects;
#[cfg(target_os = "macos")]
use crate::host::{EffectKind, HostCommand};

/// Why a declared Keychain service produced no item, so a caller can tell
/// "this host simply has no such item" from "the secret is here but macOS
/// would not hand it over".
///
/// The distinction matters: an app whose auth token is silently left behind
/// arrives on the destination looking signed in (its profile and account
/// database transferred) while being unable to authenticate — Unity Hub lands
/// with its account row present but no license, which reads as a broken image
/// rather than a skipped credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeychainRead {
    /// The secret was read and can be carried.
    Read(KeychainItem),
    /// No item with this service exists on this host.
    Absent,
    /// The item exists, but its value could not be read — the authorization
    /// dialog was denied, ignored, or timed out. Approving it once (choosing
    /// "Always Allow") adds this tool to the item's ACL so later reads are
    /// silent.
    Unreadable,
}

/// Read a generic-password secret from the login Keychain. Returns `None` off
/// macOS, when the item is absent, or when a Keychain prompt is denied — use
/// [`probe_generic`] when you need to tell those apart. When
/// `account` is `None` the first item matching `service` is returned and its
/// real account is captured from the item's attributes (Chromium Safe Storage
/// items use varied accounts — e.g. "Slack Key", "Chrome" — that the
/// destination must reinstall under).
pub fn read_generic(
    host: &dyn HostEffects,
    service: &str,
    account: Option<&str>,
) -> Option<KeychainItem> {
    match probe_generic(host, service, account) {
        KeychainRead::Read(item) => Some(item),
        _ => None,
    }
}

/// Like [`read_generic`], but reports *why* nothing came back so callers can
/// warn about a credential that exists yet could not be carried.
pub fn probe_generic(host: &dyn HostEffects, service: &str, account: Option<&str>) -> KeychainRead {
    #[cfg(target_os = "macos")]
    {
        // Resolve the account first when not given, by parsing the item's
        // attribute dump (`"acct"<blob>="..."`).
        let resolved_account = match account {
            Some(acct) => acct.to_string(),
            None => account_for_service(host, service).unwrap_or_default(),
        };

        let mut args = vec!["find-generic-password", "-s", service, "-w"];
        if !resolved_account.is_empty() {
            args.push("-a");
            args.push(&resolved_account);
        }
        // Reading another app's Safe Storage secret triggers a Keychain
        // authorization dialog (rcdp isn't in the item's ACL until the user
        // clicks "Always Allow" once). Bound the wait so an unattended run or an
        // ignored prompt doesn't hang the whole export forever — the session
        // still transfers, just without the key (the app opens to its sign-in).
        // Whether the item exists at all is answerable without authorization
        // (attributes are readable; only the secret is gated), so probe that
        // first to tell Absent from Unreadable.
        let exists = account_for_service(host, service).is_some();
        let Ok(output) = host.run(
            &HostCommand::new(EffectKind::KeychainRead, "security")
                .args(args)
                .timeout(KEYCHAIN_PROMPT_BUDGET),
        ) else {
            return if exists {
                KeychainRead::Unreadable
            } else {
                KeychainRead::Absent
            };
        };
        if !output.success {
            return if exists {
                KeychainRead::Unreadable
            } else {
                KeychainRead::Absent
            };
        }
        // `-w` prints only the secret as text with a trailing newline. Chromium
        // Safe Storage keys are printable base64, so text is faithful here.
        let mut secret = output.stdout;
        if secret.last() == Some(&b'\n') {
            secret.pop();
        }
        if secret.is_empty() {
            return if exists {
                KeychainRead::Unreadable
            } else {
                KeychainRead::Absent
            };
        }
        KeychainRead::Read(KeychainItem {
            service: service.to_string(),
            account: resolved_account,
            secret,
            trust_app: None,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (host, service, account);
        KeychainRead::Absent
    }
}

/// How long to wait for a Keychain authorization dialog before giving up and
/// transferring the session without the key.
#[cfg(target_os = "macos")]
const KEYCHAIN_PROMPT_BUDGET: std::time::Duration = std::time::Duration::from_secs(45);

/// Parse the account (`acct`) of the first generic-password item matching
/// `service` from its attribute dump. macOS only.
#[cfg(target_os = "macos")]
fn account_for_service(host: &dyn HostEffects, service: &str) -> Option<String> {
    let output = host
        .run(
            &HostCommand::new(EffectKind::KeychainRead, "security").args([
                "find-generic-password",
                "-s",
                service,
            ]),
        )
        .ok()?;
    if !output.success {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Lines look like:  "acct"<blob>="Slack Key"
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"acct\"<blob>=") {
            return Some(rest.trim().trim_matches('"').to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{EffectKind, FakeHost};

    /// A service no host has must read as `Absent`, never `Unreadable` — the
    /// two drive different messages, and crying "approve the prompt" for an app
    /// that was never signed in would send the user chasing a dialog that will
    /// not appear.
    #[test]
    fn a_service_that_does_not_exist_reads_as_absent() {
        let host = FakeHost::new();
        let read = probe_generic(&host, "cua nonexistent service for tests", None);
        assert_eq!(read, KeychainRead::Absent, "got {read:?}");
        assert!(read_generic(&host, "cua nonexistent service for tests", None).is_none());
        // Only reads were attempted, and only against the fake.
        assert!(
            host.calls()
                .iter()
                .all(|call| call.kind == EffectKind::KeychainRead)
        );
    }

    /// On macOS a present, readable item comes back with its account resolved
    /// from the attribute dump — all through the fake host.
    #[cfg(target_os = "macos")]
    #[test]
    fn reads_secret_and_account_through_the_injected_host() {
        use crate::host::HostOutput;
        let host = FakeHost::new().with_responder(|command| {
            Ok(if command.args.iter().any(|a| a == "-w") {
                HostOutput::ok("s3cret\n")
            } else {
                HostOutput::ok("    \"acct\"<blob>=\"Slack Key\"\n")
            })
        });
        let item = read_generic(&host, "Slack Safe Storage", None).expect("read");
        assert_eq!(item.account, "Slack Key");
        assert_eq!(item.secret, b"s3cret");
        assert!(host.calls().iter().all(|c| c.program == "security"));
    }
}
