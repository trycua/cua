// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Broker-side usage telemetry: counts and fixed vocabularies only.
//!
//! What leaves the broker: a consent decision (`requested`, `approved`,
//! `denied`, `expired`), and per teleport the provider's public catalog id,
//! the verified caller *kind*, a typed outcome, a duration bucket and an
//! item-count bucket. Never item ids, labels, sites, targets, fingerprints,
//! executable paths, bundle ids of third-party apps, error messages or
//! anything stored in the vault.

use std::time::Instant;

use cua_telemetry::Telemetry;
use cua_telemetry::events::{
    self, CallerKind, CallerKindSource, ConsentDecision, TeleportInfo, TeleportOutcome,
};

use crate::broker::TeleportOutcome as Delivered;
use crate::caller::{CallerIdentity, Signing};
use crate::{Error, Result};

/// The caller kind of a verified identity:
///
/// - `cua_app_signed`: OS-verified, team-signed, and satisfies the Cua code
///   requirement (the Cua app or CLI);
/// - `embedded_sdk`: OS-verified and team-signed by someone else (a
///   third-party app that embeds the cua SDK);
/// - `unsigned`: ad hoc signed or unsigned code;
/// - `unknown`: the platform cannot verify signatures (Linux callers are
///   identified by path only).
pub fn caller_kind(c: &CallerIdentity) -> CallerKind {
    match (&c.signing, c.os_verified) {
        (Signing::Signed { .. }, true) if c.first_party => CallerKind::CuaAppSigned,
        (Signing::Signed { .. }, true) => CallerKind::EmbeddedSdk,
        (Signing::AdHoc { .. } | Signing::Unsigned, _) => CallerKind::Unsigned,
        _ => CallerKind::Unknown,
    }
}

/// The typed teleport outcome of a broker error.
pub fn outcome(e: &Error) -> TeleportOutcome {
    match e {
        Error::Denied(_) | Error::PresenceFailed(_) => TeleportOutcome::ConsentDenied,
        Error::Forbidden(_) | Error::Capability(_) => TeleportOutcome::Forbidden,
        Error::Locked | Error::NoVault(_) => TeleportOutcome::Locked,
        Error::Disabled => TeleportOutcome::Disabled,
        Error::RateLimited(_) => TeleportOutcome::RateLimited,
        Error::NotFound(_) => TeleportOutcome::NotFound,
        Error::Invalid(_) => TeleportOutcome::Invalid,
        _ => TeleportOutcome::Error,
    }
}

pub(crate) fn consent(t: &Telemetry, d: ConsentDecision) {
    t.capture(events::keyvault_consent(d));
}

pub(crate) fn expired(t: &Telemetry, n: usize) {
    for _ in 0..n.min(32) {
        consent(t, ConsentDecision::Expired);
    }
}

/// The teleport info the broker records for `caller` and provider `app`.
pub fn info(caller: &CallerIdentity, app: &str) -> TeleportInfo {
    TeleportInfo::new(
        app,
        "full",
        "session",
        caller_kind(caller),
        CallerKindSource::BrokerVerified,
        "broker",
    )
}

pub(crate) fn teleport(
    t: &Telemetry,
    caller: &CallerIdentity,
    app: &str,
    started: Instant,
    r: &Result<Delivered>,
    items: u64,
) {
    let info = info(caller, app);
    t.capture(events::teleport_attempted(&info));
    let o = match r {
        Ok(_) => TeleportOutcome::Ok,
        Err(e) => outcome(e),
    };
    t.capture(events::teleport_completed(
        &info,
        o,
        started.elapsed(),
        items,
    ));
    // A site's sign-in used in a Space (Keyvault adoption).
    t.capture(events::keyvault_action(
        events::KeyvaultAction::SiteLogin,
        events::KeyvaultMethod::None,
        match o {
            TeleportOutcome::Ok => cua_telemetry::Outcome::Ok,
            TeleportOutcome::ConsentDenied => cua_telemetry::Outcome::Cancelled,
            _ => cua_telemetry::Outcome::Error,
        },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(signing: Signing, first_party: bool, os_verified: bool) -> CallerIdentity {
        CallerIdentity {
            pid: 42,
            uid: 501,
            path: Some("/Users/alice/Applications/Acme Secret.app/Contents/MacOS/acme".into()),
            signing,
            first_party,
            os_verified,
            launched_by: Some("/Users/alice/bin/zsh".into()),
            verified_name: Some("Developer ID Application: Alice Smith (ABCDE12345)".into()),
        }
    }

    fn signed(team: &str, ident: &str) -> Signing {
        Signing::Signed {
            team_id: team.into(),
            identifier: ident.into(),
            cdhash: "ab".repeat(20),
        }
    }

    #[test]
    fn caller_kind_per_caller_type() {
        // The signed Cua app / CLI.
        assert_eq!(
            caller_kind(&id(
                signed(crate::caller::CUA_TEAM_ID, "com.trycua.spaces.app"),
                true,
                true
            )),
            CallerKind::CuaAppSigned
        );
        // A third-party app embedding the SDK, verified.
        assert_eq!(
            caller_kind(&id(signed("ABCDE12345", "com.acme.secret"), false, true)),
            CallerKind::EmbeddedSdk
        );
        // Ad hoc and unsigned code.
        assert_eq!(
            caller_kind(&id(
                Signing::AdHoc {
                    identifier: "com.trycua.cua".into(),
                    cdhash: "00".into()
                },
                false,
                true
            )),
            CallerKind::Unsigned
        );
        assert_eq!(
            caller_kind(&id(Signing::Unsigned, false, false)),
            CallerKind::Unsigned
        );
        // Not verifiable (Linux), even when marked first party by path.
        assert_eq!(
            caller_kind(&id(Signing::Unknown, true, false)),
            CallerKind::Unknown
        );
        // A signature the OS did not verify never counts as the Cua app.
        assert_eq!(
            caller_kind(&id(
                signed(crate::caller::CUA_TEAM_ID, "com.trycua.cua"),
                true,
                false
            )),
            CallerKind::Unknown
        );
    }

    #[test]
    fn broker_teleport_payload_holds_no_identity_or_vault_content() {
        let h = tempfile::tempdir().unwrap();
        let sink = std::sync::Arc::new(cua_telemetry::sink::MemorySink::new());
        let t = Telemetry::builder()
            .env(|_| None)
            .home(h.path())
            .sink(sink.clone())
            .product("daemon", "1.0.0")
            .foreground()
            .build();
        t.acknowledge_notice();
        let caller = id(signed("ABCDE12345", "com.acme.secret"), false, true);
        teleport(
            &t,
            &caller,
            "chrome",
            Instant::now(),
            &Err(Error::Forbidden(
                "request access first for item it-7f3 to space-alice".into(),
            )),
            2,
        );
        teleport(
            &t,
            &caller,
            "com.acme.secret",
            Instant::now(),
            &Err(Error::Locked),
            1,
        );
        t.flush(std::time::Duration::from_secs(1));
        let events = sink.events();
        let text = serde_json::to_string(&events).unwrap().to_lowercase();
        for leak in [
            "alice",
            "acme",
            "abcde12345",
            "it-7f3",
            "space-",
            "/users",
            "zsh",
            "fp1-",
            "developer id",
            "request access",
        ] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
        let done: Vec<_> = events
            .iter()
            .filter(|e| e["event"] == "cua_teleport_completed")
            .collect();
        assert_eq!(done.len(), 2);
        assert_eq!(done[0]["properties"]["caller_kind"], "embedded_sdk");
        assert_eq!(
            done[0]["properties"]["caller_kind_source"],
            "broker_verified"
        );
        assert_eq!(done[0]["properties"]["path"], "broker");
        assert_eq!(done[0]["properties"]["outcome"], "forbidden");
        assert_eq!(done[0]["properties"]["app"], "chrome");
        assert_eq!(done[1]["properties"]["app"], "other");
        assert_eq!(done[1]["properties"]["outcome"], "locked");
    }
}
