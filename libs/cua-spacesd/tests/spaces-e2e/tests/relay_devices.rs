// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! cua-host's client-device protocol against the real cua-relay (account
//! mode, a fake OIDC issuer, grace period over): enrollment with a second
//! factor, approval by code, a same-machine re-key, device sessions on the
//! machine directory and Spaces, audit, revocation, and a host re-running
//! setup with its machine token only.

use std::sync::Arc;

use cua_host::relay::RegisterRequest;
use cua_host::{DeviceAuth, DeviceState, MemoryKeySlot, RelayClient, StaticToken};
use cua_relay::devices::DevicePolicy;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};

const ISSUER: &str = "https://auth.test/realms/cua";

async fn relay(issuer: &FakeIssuer) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_policy: DevicePolicy {
            grace_secs: 0,
            ..DevicePolicy::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    url
}

/// Relay device policy (S4) this suite pins: a fresh sign-in on an account
/// with a verified email enrolls a brand-new additional device (another
/// machine) without an approval (cua-relay 4f62b9c91). When the relay
/// requires an approval or MFA for those again, set this to `false`: the
/// asserts that read it expect `Pending` and approve by code instead. The
/// first device and a same-machine re-key stay approval-free either way.
const FRESH_SIGN_IN_ENROLLS_A_NEW_MACHINE: bool = false;

/// A device with a fresh key on the machine `machine` (the machine id it
/// reports; never this host's, so the suite neither reads the host's
/// hardware id nor ties devices together by it).
fn device(url: &str, token: &str, name: &str, machine: &str) -> Arc<DeviceAuth> {
    Arc::new(
        DeviceAuth::new(
            url,
            Arc::new(StaticToken(token.into())),
            Arc::new(MemoryKeySlot::default()),
            name,
        )
        .unwrap()
        .with_machine_id(Some(format!("e2e-machine-{machine}"))),
    )
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn devices_enroll_and_reach_machines_on_the_real_relay() {
    let issuer = FakeIssuer::new(ISSUER);
    let url = relay(&issuer).await;
    let client = RelayClient::new(&url).unwrap();
    // Signed in an hour ago: a long-lived session.
    issuer.set_auth_time("ada", now() - 3600);
    let stale = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);

    // Hosting needs no client device.
    let reg = client
        .register(
            &stale,
            &RegisterRequest {
                id: "0123abcd4567ef89".into(),
                name: "studio-mac".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    // A host re-running setup proves itself with its machine token.
    let rotated = client
        .register_as_machine(
            &stale,
            Some(&reg.machine_token),
            &RegisterRequest {
                id: "0123abcd4567ef89".into(),
                name: "studio-mac".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    assert_ne!(rotated.machine_token, reg.machine_token);
    // Without it, a session alone cannot rotate the token.
    assert!(
        client
            .register(
                &stale,
                &RegisterRequest {
                    id: "0123abcd4567ef89".into(),
                    name: "x".into(),
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .is_err()
    );
    // The session alone lists nothing.
    assert!(matches!(
        client.machines(&stale).await,
        Err(cua_host::Error::PermissionDenied(_))
    ));

    // The first device needs a fresh sign-in.
    let laptop = device(&url, &stale, "laptop", "laptop");
    let r = laptop.enroll().await.unwrap();
    assert_eq!(r.device.state, DeviceState::Pending);
    assert!(r.code.is_some());
    issuer.set_auth_time("ada", now());
    let fresh = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let laptop = device(&url, &fresh, "laptop", "laptop");
    // (A new key: the relay tells devices apart by key.)
    let r = laptop.enroll().await.unwrap();
    assert_eq!(r.device.state, DeviceState::Enrolled, "{r:?}");
    let session = laptop.session().await.unwrap();
    let machines = client
        .clone()
        .with_device_session(Some(session))
        .machines(&fresh)
        .await
        .unwrap();
    assert_eq!(machines.len(), 1);

    // Spaces lists relay machines with the device.
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces::Spaces::builder()
        .home(home.path())
        .relay(
            cua_spaces::RelayAccount::new(&url, Arc::new(StaticToken(fresh.clone())))
                .with_device(laptop.clone()),
        )
        .build();
    assert_eq!(spaces.relay_machines().await.unwrap().len(), 1);

    // A second device on a long-lived session: code shown there, approved
    // from the laptop. (Any enrollment policy: no fresh sign-in, no bypass.)
    let phone = device(&url, &stale, "phone", "phone");
    let r = phone.enroll().await.unwrap();
    assert_eq!(r.device.state, DeviceState::Pending);
    assert!(phone.session().await.is_err());
    laptop.approve(r.code.as_deref(), None).await.unwrap();
    assert!(phone.session().await.is_ok());
    let old_phone_id = phone.device_id().unwrap().unwrap();

    // POLICY (S4): a new key on an already enrolled machine (another build
    // of cua there) enrolls by a fresh sign-in and replaces the old key.
    let phone = device(&url, &fresh, "phone", "phone");
    let r = phone.enroll().await.unwrap();
    assert_eq!(r.device.state, DeviceState::Enrolled, "{r:?}");
    assert_eq!(r.superseded, vec![old_phone_id.clone()]);
    assert!(r.code.is_none());
    assert!(phone.session().await.is_ok());

    // POLICY (S4): a brand-new machine on a fresh sign-in.
    let tablet = device(&url, &fresh, "tablet", "tablet");
    let r = tablet.enroll().await.unwrap();
    if FRESH_SIGN_IN_ENROLLS_A_NEW_MACHINE {
        assert_eq!(r.device.state, DeviceState::Enrolled, "{r:?}");
        assert!(r.code.is_none());
    } else {
        assert_eq!(r.device.state, DeviceState::Pending, "{r:?}");
        assert!(tablet.session().await.is_err());
        laptop.approve(r.code.as_deref(), None).await.unwrap();
    }
    assert!(r.superseded.is_empty());
    assert!(tablet.session().await.is_ok());

    // One device per machine: laptop, phone (its new key) and tablet.
    let list = laptop.devices().await.unwrap();
    assert_eq!(
        list.iter()
            .filter(|d| d.state == DeviceState::Enrolled)
            .count(),
        3,
        "{list:?}"
    );
    let phone_id = phone.device_id().unwrap().unwrap();
    assert_ne!(phone_id, old_phone_id);
    laptop.rename(&phone_id, "work phone").await.unwrap();
    laptop.revoke(&phone_id).await.unwrap();
    phone.reset_session().await;
    assert!(phone.session().await.is_err());

    let kinds: Vec<_> = laptop
        .audit(200)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    for kind in [
        "machine_registered",
        "machine_reregistered",
        "device_enrolled",
        "device_rekeyed",
        "device_session",
        "device_renamed",
        "device_revoked",
    ] {
        assert!(kinds.iter().any(|k| k == kind), "{kind} missing: {kinds:?}");
    }
}
