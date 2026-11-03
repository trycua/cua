//! Live e2e against a real linux container (X11 desktop,
//! PipeWire, Firefox, the real spacesd). Run through
//! `tests/e2e/run-docker-e2e.sh`, which starts the container with
//! `--memory=4g` and exports:
//!
//! - `CUA_SPACES_E2E_URL`   `http://127.0.0.1:<published 3211>`
//! - `CUA_SPACES_E2E_TOKEN` the spacesd token it started the guest with
//!
//! Without them every test here is skipped (prints why and passes).
//! Host safety: the only host-side effects are temp directories, a
//! loopback HTTP server, and a hotspot dialer that refuses everything but
//! that server..

use cua_spaces::files::SendFileOptions;
use cua_spaces::presence::{Cursor, Identity, PresenceEvent};
use cua_spaces::{Space, Spaces};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;

fn target() -> Option<(String, String)> {
    let url = std::env::var("CUA_SPACES_E2E_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    let token = std::env::var("CUA_SPACES_E2E_TOKEN").unwrap_or_default();
    Some((url, token))
}

macro_rules! require_target {
    () => {
        match target() {
            Some(t) => t,
            None => {
                eprintln!("skipped: set CUA_SPACES_E2E_URL (run tests/e2e/run-docker-e2e.sh)");
                return;
            }
        }
    };
}

async fn connect(reg: &std::path::Path) -> (Spaces, Space) {
    let (url, token) = target().unwrap();
    let spaces = Spaces::builder()
        .home(reg)
        .download_dir(reg.join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&url, Some(token), Some("e2e".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    (spaces, space)
}

async fn sh(space: &Space, cmd: &str) -> String {
    let out = space.bash(cmd, Duration::from_secs(120)).await.unwrap();
    assert!(out.success(), "{cmd}: {}", out.render());
    out.stdout
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[tokio::test]
async fn e2e_bash_and_send_file_verified_in_the_guest() {
    require_target!();
    let reg = tempfile::tempdir().unwrap();
    let (_spaces, space) = connect(reg.path()).await;
    assert!(sh(&space, "uname -s").await.starts_with("Linux"));

    // 5 MiB of pseudo-random bytes plus a folder with ignore rules.
    let src = tempfile::tempdir().unwrap();
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let blob: Vec<u8> = (0..5 * 1024 * 1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::fs::write(src.path().join("blob.bin"), &blob).unwrap();
    let r = space
        .send_file(
            &src.path().join("blob.bin"),
            SendFileOptions {
                subdir: "cua-e2e".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(r.verified);
    let guest_path = &r.files[0].path;
    assert!(guest_path.contains("/Downloads/cua-e2e/"), "{guest_path}");
    let guest_sha = sh(&space, &format!("sha256sum '{guest_path}' | cut -d' ' -f1")).await;
    assert_eq!(
        guest_sha.trim(),
        sha(&blob),
        "sha256 verified by the guest's own sha256sum"
    );

    let proj = src.path().join("proj");
    std::fs::create_dir_all(proj.join("node_modules/x")).unwrap();
    std::fs::write(proj.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::write(proj.join("main.py"), "print('hi')\n").unwrap();
    std::fs::write(proj.join("node_modules/x/i.js"), "x").unwrap();
    let r = space
        .send_file(
            &proj,
            SendFileOptions {
                subdir: "cua-e2e".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(r.skipped_by_ignorefiles, vec!["node_modules/"]);
    assert_eq!(
        sh(
            &space,
            "cd ~/Downloads/cua-e2e/proj && python3 main.py && ls -A | sort | tr '\\n' ' '"
        )
        .await,
        "hi\n.gitignore main.py "
    );
}

/// Presence tests run one at a time: a process names the participant it
/// last joined a Space as on the media sessions it opens there.
static PRESENCE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn e2e_presence_with_two_clients() {
    require_target!();
    let _presence = PRESENCE.lock().await;
    let reg_a = tempfile::tempdir().unwrap();
    let reg_b = tempfile::tempdir().unwrap();
    let (_a, space_a) = connect(reg_a.path()).await;
    let (_b, space_b) = connect(reg_b.path()).await;
    let t = Duration::from_secs(20);
    let mut alice = space_a
        .join_presence(
            Identity {
                id: "cua-e2e-alice".into(),
                display_name: "Alice".into(),
                ..Default::default()
            },
            t,
        )
        .await
        .unwrap();
    let bob = space_b
        .join_presence(
            Identity {
                id: "cua-e2e-bob".into(),
                display_name: "Bob".into(),
                agent: true,
                ..Default::default()
            },
            t,
        )
        .await
        .unwrap();
    assert!(
        bob.roster()
            .iter()
            .any(|(p, _)| p.participant_id == alice.me().participant_id),
        "Bob's roster lists Alice"
    );
    let bob_id = bob.me().participant_id.clone();
    let joined = alice
        .wait_for(t, 50, |e| matches!(e, PresenceEvent::Joined { participant } if participant.participant_id == bob_id))
        .await
        .unwrap();
    if let PresenceEvent::Joined { participant } = joined {
        assert_eq!(participant.display_name, "Bob");
        assert_eq!(participant.kind, "agent");
    }
    bob.update_cursor(&Cursor::at(0.25, 0.75)).await.unwrap();
    let moved = alice
        .wait_for(t, 50, |e| matches!(e, PresenceEvent::CursorMoved { participant_id, .. } if *participant_id == bob_id))
        .await
        .unwrap();
    if let PresenceEvent::CursorMoved { cursor, .. } = moved {
        assert!((cursor.x - 0.25).abs() < 1e-6 && (cursor.y - 0.75).abs() < 1e-6);
    }
    bob.leave().await.unwrap();
    alice
        .wait_for(
            t,
            50,
            |e| matches!(e, PresenceEvent::Left { participant_id, .. } if *participant_id == bob_id),
        )
        .await
        .unwrap();
    alice.leave().await.unwrap();
}

#[tokio::test]
async fn e2e_hotspot_egress_through_this_host() {
    use cua_spaces::hotspot::{Dialer, HotspotOptions};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    require_target!();
    let reg = tempfile::tempdir().unwrap();
    let (_spaces, space) = connect(reg.path()).await;

    let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web_port = web.local_addr().unwrap().port();
    tokio::spawn(async move {
        for _ in 0..8 {
            let Ok((mut c, _)) = web.accept().await else {
                return;
            };
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf).await;
            let _ = c
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 14\r\nconnection: close\r\n\r\nfrom-the-host!")
                .await;
        }
    });
    // Every destination is answered by the loopback server above; nothing
    // else on this host or network is dialed.
    let dialer: Dialer = Arc::new(move |_host: String, _port: u16| {
        Box::pin(async move { tokio::net::TcpStream::connect(("127.0.0.1", web_port)).await })
    });
    let hotspot = space
        .start_hotspot(HotspotOptions {
            socks_port: 18_080 + (rand_u32() % 500),
            set_system_proxy: false,
            bypass: vec![],
            dialer,
        })
        .await
        .unwrap();
    let socks = hotspot.socks_address().to_string();
    let body = sh(
        &space,
        &format!("curl -sS --max-time 20 --socks5-hostname {socks} http://hotspot-probe.cua.test/"),
    )
    .await;
    assert_eq!(body, "from-the-host!");
    let status = hotspot.status().await.unwrap();
    assert!(status.bytes_in > 0 || status.bytes_out > 0, "{status:?}");
    hotspot.stop().await.unwrap();
}

/// The Space's agent runner reaches the guest (real harness runs, against
/// the scripted mock provider, are `cua-agents`' e2e_live).
#[tokio::test]
async fn e2e_agents_runner_attaches() {
    require_target!();
    let reg = tempfile::tempdir().unwrap();
    let (_spaces, space) = connect(reg.path()).await;
    let agents = space.agents().await.unwrap();
    assert!(agents.home().starts_with('/'));
    let runs = agents.list().await.unwrap();
    assert!(runs.iter().all(|r| r.run_id.starts_with("run-")));
}

fn rand_u32() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}
