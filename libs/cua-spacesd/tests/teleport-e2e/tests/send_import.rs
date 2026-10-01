// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua_teleport::Teleporter::send` → cua.env.v1 `TeleportService` of an
//! in-process cua-spacesd-server → `cua_spacesd_teleport` importers.
//!
//! Hermetic: the sender reads a fake profile under a temporary source home
//! through a sender `FakeHost`; the receiver writes into a temporary
//! destination home and "launches" through a receiver `FakeHost` that only
//! records the launch. Nothing touches a real app, profile or Keychain.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use cua_teleport::providers::chrome::ChromeProvider;
use cua_teleport::providers::electron::ElectronProvider;
use cua_teleport::{
    AppRef, ApprovalRequest, AutoApprove, Error, ExportRegistry, Platform, Selection, SendOptions,
    Teleporter, TransferScope,
};

const TOKEN: &str = "teleport-e2e-token";

struct Guest {
    url: String,
    receiver_host: Arc<cua_spacesd_teleport::FakeHost>,
    dest_home: tempfile::TempDir,
    _dirs: Vec<tempfile::TempDir>,
}

async fn guest() -> Guest {
    let data = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let dest_home = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        data_dir: data.path().to_path_buf(),
        downloads_dir: Some(downloads.path().to_path_buf()),
        teleport_home: Some(dest_home.path().to_path_buf()),
        shutdown_grace: Duration::from_secs(1),
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, Some(TOKEN.into()));
    let receiver_host = Arc::new(cua_spacesd_teleport::FakeHost::new());
    let receiver = Arc::new(cua_spacesd_teleport::Receiver::with_host(
        dest_home.path().to_path_buf(),
        receiver_host.clone(),
    ));
    let server = ServerBuilder::new(ctx).teleport_receiver(receiver).build();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    Guest {
        url: format!("http://{addr}"),
        receiver_host,
        dest_home,
        _dirs: vec![data, downloads],
    }
}

async fn client(url: &str, transport: TransportPreference) -> SpacesdClient {
    SpacesdClient::connect(
        ConnectOptions::parse(url)
            .unwrap()
            .token(TOKEN)
            .transport(transport),
    )
    .await
    .unwrap()
}

/// A fake Linux Chrome profile under `home` (the sender's source home).
fn fake_chrome(home: &Path) {
    let profile = home.join(".config/google-chrome/Default");
    std::fs::create_dir_all(profile.join("Sessions")).unwrap();
    // A real (empty) Chrome cookie store: the sender reads it as SQLite.
    rusqlite::Connection::open(profile.join("Cookies"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE cookies (creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL,
             top_frame_site_key TEXT NOT NULL DEFAULT '', name TEXT NOT NULL,
             value TEXT NOT NULL, encrypted_value BLOB NOT NULL DEFAULT '',
             path TEXT NOT NULL, expires_utc INTEGER NOT NULL, is_secure INTEGER NOT NULL,
             is_httponly INTEGER NOT NULL, last_access_utc INTEGER NOT NULL DEFAULT 0,
             has_expires INTEGER NOT NULL DEFAULT 1, is_persistent INTEGER NOT NULL DEFAULT 1,
             priority INTEGER NOT NULL DEFAULT 1, samesite INTEGER NOT NULL DEFAULT -1,
             source_scheme INTEGER NOT NULL DEFAULT 0, source_port INTEGER NOT NULL DEFAULT -1,
             UNIQUE (host_key, top_frame_site_key, name, path));",
        )
        .unwrap();
    std::fs::write(profile.join("Preferences"), b"{\"p\":1}").unwrap();
    std::fs::write(profile.join("Bookmarks"), b"{\"roots\":{}}").unwrap();
    // Big enough to span several 1 KiB upload chunks.
    std::fs::write(profile.join("Sessions/Session_1"), vec![7u8; 5000]).unwrap();
}

fn sender(home: &Path) -> (Arc<cua_teleport::FakeHost>, Teleporter) {
    let host = Arc::new(cua_teleport::FakeHost::new().with_home(home));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(
        ChromeProvider::new()
            .without_devtools()
            .with_host(host.clone()),
    ));
    registry.register(Box::new(
        ElectronProvider::slack()
            .without_keychain()
            .with_host(host.clone()),
    ));
    let teleporter = Teleporter::with_registry(registry).options(SendOptions {
        chunk_bytes: Some(1024),
        ..SendOptions::default()
    });
    (host, teleporter)
}

fn chrome() -> AppRef {
    AppRef {
        app_id: "google-chrome".into(),
        display_name: "Google Chrome".into(),
        platform: Platform::Linux,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chrome_session_lands_in_the_guest_home_over_both_transports() {
    for transport in [TransportPreference::Native, TransportPreference::GrpcWeb] {
        let g = guest().await;
        let src = tempfile::tempdir().unwrap();
        fake_chrome(src.path());
        let (sender_host, teleporter) = sender(src.path());
        let progress = Arc::new(Mutex::new(Vec::new()));
        let seen = progress.clone();
        let teleporter = teleporter.options(SendOptions {
            chunk_bytes: Some(1024),
            progress: Some(Arc::new(move |sent, total| {
                seen.lock().unwrap().push((sent, total))
            })),
            ..SendOptions::default()
        });
        let approvals = Arc::new(Mutex::new(Vec::new()));
        let log = approvals.clone();
        let env = client(&g.url, transport).await;
        let outcome = teleporter
            .send(
                &env,
                &chrome(),
                TransferScope::FullProfile,
                Selection::All,
                Arc::new(move |r: &ApprovalRequest<'_>| {
                    log.lock().unwrap().push((
                        r.sensitive,
                        r.selected.len(),
                        r.destination.clone(),
                    ));
                    true
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("{transport:?}: {e}"));

        // The consent callback saw the sensitive selection, then the OS gate
        // (the fake) was asked exactly once.
        let approvals = approvals.lock().unwrap().clone();
        assert_eq!(approvals.len(), 1);
        assert!(approvals[0].0, "cookies are sensitive");
        assert_eq!(sender_host.authorizations().len(), 1);

        assert_eq!(outcome.provider_id, "chrome");
        assert!(outcome.launched, "{outcome:?}");
        assert!(outcome.bundle_bytes > 5000);
        // Upload progress covered the bundle in several chunks.
        let progress = progress.lock().unwrap().clone();
        assert!(progress.len() >= 5, "{progress:?}");
        assert_eq!(
            progress.last().copied(),
            Some((outcome.bundle_bytes, outcome.bundle_bytes))
        );

        // Files landed in the guest's (temp) home, remapped for this platform.
        let user_data = if cfg!(target_os = "macos") {
            "Library/Application Support/Google/Chrome"
        } else {
            ".config/google-chrome"
        };
        let profile = g.dest_home.path().join(user_data).join("Default");
        // Cookies travel as decrypted rows and are re-encrypted for the guest
        // (unit-tested in cua-spacesd-teleport); this store is empty.
        assert_eq!(
            std::fs::read(profile.join("Sessions/Session_1")).unwrap(),
            vec![7u8; 5000]
        );
        // The launch was recorded by the receiver's fake host, not executed.
        let launches = g
            .receiver_host
            .calls_of(cua_spacesd_teleport::EffectKind::AppLaunch);
        assert_eq!(launches.len(), 1, "{:?}", g.receiver_host.calls());
        assert!(launches[0]
            .args
            .iter()
            .any(|a| a.starts_with("--user-data-dir=")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_selection_withholds_credentials() {
    let g = guest().await;
    let src = tempfile::tempdir().unwrap();
    fake_chrome(src.path());
    let (sender_host, teleporter) = sender(src.path());
    let env = client(&g.url, TransportPreference::Native).await;
    let outcome = teleporter
        .send(
            &env,
            &chrome(),
            TransferScope::FullProfile,
            Selection::Default,
            Arc::new(AutoApprove),
        )
        .await
        .unwrap();
    assert!(outcome.withheld.iter().any(|p| p.ends_with("/Cookies")));
    // No sensitive item was selected, so the OS gate never prompted.
    assert!(sender_host.authorizations().is_empty());
    let user_data = if cfg!(target_os = "macos") {
        "Library/Application Support/Google/Chrome"
    } else {
        ".config/google-chrome"
    };
    let profile = g.dest_home.path().join(user_data).join("Default");
    assert!(profile.join("Preferences").is_file());
    assert!(!profile.join("Cookies").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declined_approval_sends_nothing() {
    let g = guest().await;
    let src = tempfile::tempdir().unwrap();
    fake_chrome(src.path());
    let (sender_host, teleporter) = sender(src.path());
    let env = client(&g.url, TransportPreference::Native).await;
    let err = teleporter
        .send(
            &env,
            &chrome(),
            TransferScope::FullProfile,
            Selection::All,
            Arc::new(|_: &ApprovalRequest<'_>| false),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotApproved), "{err}");
    assert!(sender_host.authorizations().is_empty());
    assert!(g.receiver_host.calls().is_empty());
    assert_eq!(std::fs::read_dir(g.dest_home.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn electron_import_stops_nothing_real_and_launches_through_the_fake() {
    let g = guest().await;
    let src = tempfile::tempdir().unwrap();
    let prof = src.path().join(".config/Slack");
    std::fs::create_dir_all(prof.join("Local Storage/leveldb")).unwrap();
    std::fs::write(prof.join("Cookies"), b"slack-cookies").unwrap();
    std::fs::write(prof.join("Local Storage/leveldb/000003.log"), b"token").unwrap();
    let (_, teleporter) = sender(src.path());
    let env = client(&g.url, TransportPreference::GrpcWeb).await;
    let outcome = teleporter
        .send(
            &env,
            &AppRef {
                app_id: "Slack".into(),
                display_name: "Slack".into(),
                platform: Platform::Linux,
            },
            TransferScope::FullProfile,
            Selection::Default,
            Arc::new(AutoApprove),
        )
        .await
        .unwrap();
    assert_eq!(outcome.provider_id, "slack");
    let support = if cfg!(target_os = "macos") {
        "Library/Application Support/Slack"
    } else {
        ".config/Slack"
    };
    assert_eq!(
        std::fs::read(g.dest_home.path().join(support).join("Cookies")).unwrap(),
        b"slack-cookies"
    );
    // The receiver looked for a running Slack through its fake (none), and
    // recorded the launch instead of running it.
    let calls = g.receiver_host.calls();
    assert!(calls
        .iter()
        .any(|c| c.kind == cua_spacesd_teleport::EffectKind::ProcessLookup));
    assert!(!calls
        .iter()
        .any(|c| c.kind == cua_spacesd_teleport::EffectKind::ProcessTerminate));
    assert_eq!(
        g.receiver_host
            .calls_of(cua_spacesd_teleport::EffectKind::AppLaunch)
            .len(),
        1
    );
}
