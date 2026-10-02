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
    // A guest that has never held a Chrome Safe Storage item: reading the
    // secret fails (so the receiver creates one), every other `security`
    // call and the app launch (`open` on macOS) succeed. Only a macOS receiver ever asks.
    let receiver_host = Arc::new(cua_spacesd_teleport::FakeHost::new().with_responder(|c| {
        let reads_secret =
            c.args.iter().any(|a| a == "find-generic-password") && c.args.iter().any(|a| a == "-w");
        let launches = c.kind == cua_spacesd_teleport::EffectKind::AppLaunch;
        if (c.program != "security" && !launches) || reads_secret {
            Ok(cua_spacesd_teleport::HostOutput::failed())
        } else {
            Ok(cua_spacesd_teleport::HostOutput::ok(""))
        }
    }));
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
    std::fs::create_dir_all(profile.join("Network")).unwrap();
    // A real (empty) Chrome cookie store in modern Chrome's layout
    // (`Network/Cookies`, no root `Cookies`): the sender reads it as SQLite.
    rusqlite::Connection::open(profile.join("Network/Cookies"))
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

/// The receiving Space's Chrome was never launched: its profile has no
/// `Cookies` database at all. The teleport must still land the signed-in
/// cookies, in a database Chrome adopts on its first real launch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookies_land_in_a_chrome_that_was_never_launched() {
    use cua_teleport_bundle::chromium_crypto as crypto;

    let g = guest().await;
    // Nothing exists under the guest home: not the profile, not `Cookies`.
    assert_eq!(std::fs::read_dir(g.dest_home.path()).unwrap().count(), 0);
    let src = tempfile::tempdir().unwrap();
    fake_chrome(src.path());
    {
        let key = crypto::derive_key(crypto::LINUX_V10_PASSWORD, crypto::LINUX_V10_PBKDF2_ROUNDS);
        let blob = crypto::encrypt_v10(&key, b"fresh-session-value");
        let conn = rusqlite::Connection::open(
            src.path()
                .join(".config/google-chrome/Default/Network/Cookies"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies (creation_utc, host_key, name, value, encrypted_value, path,
             expires_utc, is_secure, is_httponly, samesite)
             VALUES (1, '.github.com', 'user_session', '', ?1, '/', 13400000000000000, 1, 1, 1)",
            [blob],
        )
        .unwrap();
    }
    let (_, teleporter) = sender(src.path());
    let env = client(&g.url, TransportPreference::Native).await;
    teleporter
        .send(
            &env,
            &chrome(),
            TransferScope::FullProfile,
            Selection::All,
            Arc::new(AutoApprove),
        )
        .await
        .unwrap();

    let user_data = if cfg!(target_os = "macos") {
        "Library/Application Support/Google/Chrome"
    } else {
        ".config/google-chrome"
    };
    let db = g
        .dest_home
        .path()
        .join(user_data)
        .join("Default/Network/Cookies");
    assert!(db.is_file(), "no cookies database was created");
    // Current Chrome (154) reads `Default/Cookies`, older builds
    // `Default/Network/Cookies`: both carry the session, so neither a new nor
    // an old guest Chrome comes up signed out.
    assert!(
        db.with_file_name("..").join("Cookies").is_file(),
        "no root Cookies database was created"
    );
    let conn = rusqlite::Connection::open(&db).unwrap();
    let version: String = conn
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(version.parse::<i64>().unwrap() >= 24, "{version}");
    let encrypted: Vec<u8> = conn
        .query_row(
            "SELECT encrypted_value FROM cookies WHERE host_key = '.github.com'
             AND name = 'user_session'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(encrypted.starts_with(b"v10"));
    // On a Linux guest the key is Chrome's fixed one, so the value can be
    // read back exactly as Chrome would (digest of host_key, then value). A
    // macOS guest's key lives in a Keychain the fake host does not hold.
    if cfg!(not(target_os = "macos")) {
        let key = crypto::derive_key(crypto::LINUX_V10_PASSWORD, crypto::LINUX_V10_PBKDF2_ROUNDS);
        let plain = crypto::decrypt_prefixed(&key, &encrypted).unwrap().1;
        assert!(plain.ends_with(b"fresh-session-value"));
        assert_eq!(plain.len(), 32 + b"fresh-session-value".len());
    }
}

/// The modern layout end to end: a source Chrome with `Network/Cookies` in its
/// current schema (every attribute, a partitioned cookie, a Windows
/// app-bound value) and a `Local Storage` LevelDB goes through the real sender,
/// an in-process spacesd and the receiver. Every cookie attribute lands in the
/// guest's own database, the partitioned cookie stays partitioned, the
/// app-bound one is not sent, and the localStorage values land in the guest's
/// LevelDB.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn modern_layout_cookie_attributes_and_local_storage_arrive_whole() {
    use cua_chromium_storage as ls;
    use cua_teleport::browser_cookies::{
        write_modern_cookies_db_for_tests, CookieExtras, ModernTestRow, TestCookieRow,
    };
    use cua_teleport_bundle::chromium_crypto as crypto;

    let g = guest().await;
    let src = tempfile::tempdir().unwrap();
    fake_chrome(src.path());
    let profile = src.path().join(".config/google-chrome/Default");
    std::fs::remove_file(profile.join("Network/Cookies")).unwrap();
    let key = crypto::derive_key(crypto::LINUX_V10_PASSWORD, crypto::LINUX_V10_PBKDF2_ROUNDS);
    let enc = |v: &[u8]| crypto::encrypt_v10(&key, v);
    let core = |host: &'static str, name: &'static str, value: Vec<u8>| TestCookieRow {
        host_key: host,
        name,
        encrypted_value: value,
        path: "/",
        expires_utc: 13_400_000_000_000_000,
        is_secure: true,
        is_httponly: true,
        samesite: 1,
    };
    let full = CookieExtras {
        creation_utc: Some(13_300_000_000_000_001),
        last_access_utc: Some(13_300_000_000_000_002),
        last_update_utc: Some(13_300_000_000_000_003),
        priority: Some(2),
        source_scheme: Some(2),
        source_port: Some(443),
        source_type: Some(1),
        has_cross_site_ancestor: Some(1),
        top_frame_site_key: Some("https://top.example".into()),
    };
    let mut app_bound = b"v20".to_vec();
    app_bound.extend_from_slice(&[9u8; 40]);
    write_modern_cookies_db_for_tests(
        &profile,
        &[
            ModernTestRow {
                core: core(".example.com", "sid", enc(b"partitioned")),
                extra: full,
            },
            ModernTestRow {
                core: core(".example.com", "sid", enc(b"plain")),
                extra: CookieExtras::default(),
            },
            ModernTestRow {
                core: core(".bank.test", "app_bound", app_bound),
                extra: CookieExtras::default(),
            },
        ],
    )
    .unwrap();
    let item = |origin: &str, key: &str, value: &str| ls::LocalStorageItem {
        origin: origin.into(),
        key: key.into(),
        value: value.into(),
        key_raw: None,
        value_raw: None,
    };
    let source_items = vec![
        item("https://github.com", "color_mode", "dark"),
        item("https://a.example", "日本", "こんにちは"),
    ];
    ls::write(
        &ls::store_dir(&profile),
        &source_items,
        13_300_000_000_000_000,
    )
    .unwrap();

    let (_, teleporter) = sender(src.path());
    let env = client(&g.url, TransportPreference::Native).await;
    teleporter
        .send(
            &env,
            &chrome(),
            TransferScope::FullProfile,
            Selection::All,
            Arc::new(AutoApprove),
        )
        .await
        .unwrap();

    let user_data = if cfg!(target_os = "macos") {
        "Library/Application Support/Google/Chrome"
    } else {
        ".config/google-chrome"
    };
    let dest = g.dest_home.path().join(user_data).join("Default");
    let conn = rusqlite::Connection::open(dest.join("Network/Cookies")).unwrap();
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM cookies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2, "the app-bound cookie is not sent");
    #[allow(clippy::type_complexity)]
    let got: (i64, i64, i64, i64, i64, i64, i64, i64, String) = conn
        .query_row(
            "SELECT creation_utc, last_access_utc, last_update_utc, priority, source_scheme,
             source_port, source_type, has_cross_site_ancestor, top_frame_site_key
             FROM cookies WHERE top_frame_site_key != ''",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        got,
        (
            13_300_000_000_000_001,
            13_300_000_000_000_002,
            13_300_000_000_000_003,
            2,
            2,
            443,
            1,
            1,
            "https://top.example".to_string()
        )
    );
    let unpartitioned: i64 = conn
        .query_row(
            "SELECT count(*) FROM cookies WHERE top_frame_site_key = '' AND name = 'sid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unpartitioned, 1);
    // The guest's own LevelDB holds the same values.
    let mut got = ls::read(&ls::store_dir(&dest)).unwrap();
    got.sort_by(|a, b| a.origin.cmp(&b.origin));
    let mut want = source_items.clone();
    want.sort_by(|a, b| a.origin.cmp(&b.origin));
    assert_eq!(got, want);
    // And not as raw LevelDB files of the sender's.
    assert!(!g.dest_home.path().join("localstorage.json").exists());
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
