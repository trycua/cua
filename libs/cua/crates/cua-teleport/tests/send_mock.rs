// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `Teleporter::send` against cua-spacesd-client's in-process mock spacesd (which
//! reassembles and SHA-checks the upload; it imports nothing).
//!
//! Host-safe: the sender reads a fake Chrome profile under a temporary home
//! through a `FakeHost`, with live-tab enumeration disabled.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use cua_spacesd_client::testing::{MockAuth, MockServer};
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_teleport::bundle::BundleReader;
use cua_teleport::layout::chrome::user_data_dir_for;
use cua_teleport::providers::chrome::ChromeProvider;
use cua_teleport::{
    AppRef, ApprovalRequest, AutoApprove, Error, ExportRegistry, FakeHost, Platform, Selection,
    SendOptions, Teleporter, TransferScope,
};

fn teleporter(home: &std::path::Path) -> (Arc<FakeHost>, Teleporter) {
    let host = Arc::new(FakeHost::new().with_home(home));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(
        ChromeProvider::new()
            .without_devtools()
            .with_host(host.clone()),
    ));
    (host, Teleporter::with_registry(registry))
}

fn fake_chrome() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let profile = home
        .path()
        .join(user_data_dir_for(Platform::Linux))
        .join("Default");
    std::fs::create_dir_all(profile.join("Sessions")).unwrap();
    std::fs::write(profile.join("Preferences"), b"{}").unwrap();
    std::fs::write(profile.join("Cookies"), b"cookies").unwrap();
    std::fs::write(profile.join("Sessions/Session_1"), vec![3u8; 4000]).unwrap();
    home
}

fn chrome() -> AppRef {
    AppRef {
        app_id: "chrome".into(),
        display_name: "Chrome".into(),
        platform: Platform::Linux,
    }
}

async fn client(mock: &MockServer, transport: TransportPreference) -> SpacesdClient {
    SpacesdClient::connect(
        ConnectOptions::parse(&mock.url())
            .unwrap()
            .token("t")
            .transport(transport),
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_in_offset_addressed_chunks_over_both_transports() {
    for transport in [TransportPreference::Native, TransportPreference::GrpcWeb] {
        let mock = MockServer::start(MockAuth {
            token: Some("t".into()),
            ..Default::default()
        })
        .await;
        let home = fake_chrome();
        let (host, t) = teleporter(home.path());
        let sent = Arc::new(Mutex::new(Vec::new()));
        let log = sent.clone();
        let t = t.options(SendOptions {
            chunk_bytes: Some(512),
            progress: Some(Arc::new(move |s, total| {
                log.lock().unwrap().push((s, total))
            })),
            ..Default::default()
        });
        let env = client(&mock, transport).await;
        let outcome = t
            .send(
                &env,
                &chrome(),
                TransferScope::FullProfile,
                Selection::Default,
                Arc::new(AutoApprove),
            )
            .await
            .unwrap();
        // Default selection: no credentials, so no OS prompt.
        assert!(host.authorizations().is_empty());
        assert!(outcome.withheld.iter().any(|p| p.ends_with("/Cookies")));

        let imports = mock.state.teleport_imports();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].app, "chrome");
        assert!(imports[0].chunks >= 8, "{} chunks", imports[0].chunks);
        assert_eq!(imports[0].bundle.len() as u64, outcome.bundle_bytes);
        let entries = BundleReader::open(Cursor::new(imports[0].bundle.clone()))
            .unwrap()
            .read_all()
            .unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.rel_path == ".config/google-chrome/Default/Sessions/Session_1")
        );
        assert!(!entries.iter().any(|e| e.rel_path.ends_with("/Cookies")));
        let sent = sent.lock().unwrap().clone();
        assert_eq!(sent.first(), Some(&(0, outcome.bundle_bytes)));
        assert_eq!(
            sent.last(),
            Some(&(outcome.bundle_bytes, outcome.bundle_bytes))
        );
        // The total is reported as the last chunk (the import) goes out,
        // then again once the Space has it.
        let n = outcome.bundle_bytes;
        assert_eq!(sent.iter().filter(|p| **p == (n, n)).count(), 2, "{sent:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_the_guest_cannot_import_is_refused_before_consent() {
    let mock = MockServer::start(MockAuth {
        token: Some("t".into()),
        ..Default::default()
    })
    .await;
    mock.state
        .teleport_unsupported
        .lock()
        .unwrap()
        .push("chrome".into());
    let home = fake_chrome();
    let (host, t) = teleporter(home.path());
    let asked = Arc::new(Mutex::new(0));
    let count = asked.clone();
    let env = client(&mock, TransportPreference::Native).await;
    let err = t
        .send(
            &env,
            &chrome(),
            TransferScope::FullProfile,
            Selection::All,
            Arc::new(move |_: &ApprovalRequest<'_>| {
                *count.lock().unwrap() += 1;
                true
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err}");
    assert_eq!(*asked.lock().unwrap(), 0);
    assert!(host.authorizations().is_empty());
    assert!(mock.state.teleport_imports().is_empty());
}

/// S1's refusal gate is enforced now that end-to-end sealing and the app's
/// consent dialog have shipped (step 2): over a real relay-prefixed
/// connection, a Space that reports no sealed-delivery key is refused by
/// `Teleporter::send` unless the caller explicitly acks the plaintext
/// risk; with the ack, or with a reported key (not this test; see
/// `a_relay_delivery_is_sealed_and_the_relay_sees_no_plaintext`), it is
/// not. The enforced decision itself is also covered by
/// `send::tests::gate_relay_delivery_both_states`, a pure unit test
/// independent of `RELAY_SEALING_ENFORCED`'s live value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::assertions_on_constants)]
async fn a_relay_destination_without_a_key_is_refused_unless_acked() {
    assert!(cua_teleport::RELAY_SEALING_ENFORCED);
    let mock = MockServer::start(MockAuth {
        token: Some("t".into()),
        prefix: Some("/m/fake-machine".into()),
        ..Default::default()
    })
    .await;
    let home = fake_chrome();
    let (_host, t) = teleporter(home.path());
    let env = SpacesdClient::connect(
        ConnectOptions::parse(&format!("{}/m/fake-machine", mock.url()))
            .unwrap()
            .token("t")
            .transport(TransportPreference::Native),
    )
    .await
    .unwrap();
    assert_eq!(
        env.endpoint().kind(),
        &cua_spacesd_client::EndpointKind::Relay {
            machine_id: "fake-machine".into(),
        }
    );

    // No ack, and this mock reports no machine_seal_public_key (it never
    // sets one), so the gate refuses after the manifest check, right
    // before the upload would otherwise start.
    let err = t
        .send(
            &env,
            &chrome(),
            TransferScope::FullProfile,
            Selection::Default,
            Arc::new(AutoApprove),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::RelayUnsealed),
        "the gate must refuse an unsealed relay destination without ack: {err}"
    );
    assert!(mock.state.teleport_imports().is_empty(), "nothing uploaded");

    // With the ack, it proceeds past the gate and actually uploads (in the
    // clear, since there is still no key to seal with -- that is exactly
    // the risk the ack accepts).
    let acked = t.options(SendOptions {
        relay_plaintext_ack: true,
        ..Default::default()
    });
    acked
        .send(
            &env,
            &chrome(),
            TransferScope::FullProfile,
            Selection::Default,
            Arc::new(AutoApprove),
        )
        .await
        .unwrap();
    assert_eq!(mock.state.teleport_imports().len(), 1);
}

/// Full S1 round trip: once the guest reports a sealed-delivery key over a
/// real relay-prefixed connection (`MockAuth::prefix`, the same path
/// stripping a real `cua-relay` does), the bytes that reach the "far side"
/// -- exactly what a relay in the middle would see -- are a sealed
/// envelope, never the plaintext bundle, and the guest's own key opens it
/// back to the real bundle.
///
/// The only test in this file that touches `CUA_HOME` (the sender's
/// machine-seal pin store): isolated to a temp dir for the duration: keep
/// it that way if you add another such test here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_delivery_is_sealed_and_the_relay_sees_no_plaintext() {
    let cua_home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("CUA_HOME", cua_home.path()) };

    let mock = MockServer::start(MockAuth {
        token: Some("t".into()),
        prefix: Some("/m/fake-machine".into()),
        ..Default::default()
    })
    .await;
    let guest = cua_machine_seal::MachineKeypair::generate();
    mock.state.set_machine_seal_public_key(guest.public().0);

    let home = fake_chrome();
    let (_host, t) = teleporter(home.path());
    let env = SpacesdClient::connect(
        ConnectOptions::parse(&format!("{}/m/fake-machine", mock.url()))
            .unwrap()
            .token("t")
            .transport(TransportPreference::Native),
    )
    .await
    .unwrap();
    assert_eq!(
        env.endpoint().kind(),
        &cua_spacesd_client::EndpointKind::Relay {
            machine_id: "fake-machine".into(),
        }
    );

    let outcome = t
        .send(
            &env,
            &chrome(),
            TransferScope::FullProfile,
            Selection::Default,
            Arc::new(AutoApprove),
        )
        .await
        .unwrap();

    let imports = mock.state.teleport_imports();
    assert_eq!(imports.len(), 1);
    let wire = imports[0].bundle.clone();
    // What a relay in the middle would see is a sealed envelope: no
    // substring of the real bundle bytes appears in it (a sealed envelope
    // is not a valid session bundle either, so the mock storing it
    // unparsed -- "it imports nothing" -- is exactly the point here).
    assert!(
        cua_machine_seal::looks_sealed(&wire),
        "relay-visible bytes must be sealed, not a plaintext bundle"
    );
    assert_ne!(wire.len(), outcome.bundle_bytes as usize);

    // The guest's own key opens it back to the exact original bundle.
    let envelope = cua_machine_seal::SealedEnvelope::from_bytes(&wire).unwrap();
    let opened = cua_machine_seal::open(
        &guest,
        envelope.space_id(),
        cua_machine_seal::purpose::TELEPORT_BUNDLE,
        &cua_machine_seal::ReplayGuard::new(),
        &envelope,
    )
    .unwrap();
    let entries = cua_teleport::bundle::BundleReader::open(std::io::Cursor::new(opened.to_vec()))
        .unwrap()
        .read_all()
        .unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.rel_path.ends_with("Preferences") || e.rel_path.contains("Sessions")),
        "the opened plaintext is the real bundle: {entries:?}"
    );

    // The wrong key (a different, unpinned guest) cannot open it.
    let other = cua_machine_seal::MachineKeypair::generate();
    assert!(
        cua_machine_seal::open(
            &other,
            envelope.space_id(),
            cua_machine_seal::purpose::TELEPORT_BUNDLE,
            &cua_machine_seal::ReplayGuard::new(),
            &envelope,
        )
        .is_err()
    );

    unsafe { std::env::remove_var("CUA_HOME") };
}
