// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live run of the app's command layer against a real Space: a
//! `linux` container (gVisor, `--memory=4g`) and a real
//! `cua daemon`, both started by `tests/e2e/run-live-docker.sh`, which
//! exports:
//!
//! - `CUA_SPACES_APP_E2E_URL`   `http://127.0.0.1:<published 3211>`
//! - `CUA_SPACES_APP_E2E_TOKEN` the spacesd token of that container
//! - `CUA_SPACES_APP_E2E_HOME`  a temp `CUA_HOME` the daemon runs under
//! - `CUA_BIN`                  the `cua` binary
//!
//! Without them the test prints why and passes. No GUI is launched: this
//! drives `AppCore` (what every Tauri command calls) and attaches to the
//! returned media tickets with a plain WebSocket, as the webview does.
//! Host safety: temp `CUA_HOME`, temp files, a generated Firefox profile read
//! through `FakeHost`; nothing reads the real user profile or keychain.

use cua_spaces_ext::teleport::providers::{ExportRegistry, FakeHost, FirefoxProvider};
use cua_spaces_ext::teleport::AppSessions;
use cua_spaces_lib::core::{AppCore, CoreConfig, DaemonMode, StreamOpts, StreamTargetArg};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

struct Target {
    url: String,
    token: String,
    home: PathBuf,
    cua_bin: PathBuf,
}

fn target() -> Option<Target> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    Some(Target {
        url: get("CUA_SPACES_APP_E2E_URL")?,
        token: get("CUA_SPACES_APP_E2E_TOKEN")?,
        home: PathBuf::from(get("CUA_SPACES_APP_E2E_HOME")?),
        cua_bin: PathBuf::from(get("CUA_BIN")?),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What attaching to a media ticket URL produced (bounded read).
#[derive(Debug, Default)]
struct Attach {
    control: Vec<String>,
    first_video_keyframe: Option<bool>,
    video_packets: usize,
}

/// Attaches to `ws_url` with no headers (the ticket is in the URL) and
/// reads at most 400 messages or 30 s, stopping once 3 video packets came.
async fn attach(ws_url: &str) -> Attach {
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(ws_url),
    )
    .await
    .expect("connect timed out")
    .expect("ticket URL attaches without headers");
    let mut out = Attach::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    for _ in 0..400 {
        let next = tokio::time::timeout_at(deadline, ws.next()).await;
        let Ok(Some(Ok(msg))) = next else { break };
        match msg {
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                out.control
                    .push(v["type"].as_str().unwrap_or_default().to_string());
            }
            Message::Binary(b) if b.first() == Some(&0) && b.len() >= 8 => {
                let hlen = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
                if out.first_video_keyframe.is_none() && b.len() >= 8 + hlen {
                    let h: serde_json::Value =
                        serde_json::from_slice(&b[8..8 + hlen]).unwrap_or_default();
                    // `{"direction":"video","message":<VideoFrameDescriptor>}`
                    let key = h["message"]["keyframe"].as_bool().unwrap_or(false);
                    out.first_video_keyframe = Some(key);
                }
                out.video_packets += 1;
                if out.video_packets >= 3 {
                    break;
                }
            }
            _ => {}
        }
    }
    let _ = ws.close(None).await;
    out
}

#[tokio::test]
async fn live_add_send_stream_teleport_delete() {
    let Some(t) = target() else {
        eprintln!("skipped: run tests/e2e/run-live-docker.sh (needs CUA_SPACES_APP_E2E_*)");
        return;
    };

    // A generated Firefox profile under a temp HOME (FakeHost sender).
    let host_home = tempfile::tempdir().unwrap();
    let profile = host_home.path().join("fixture-profile");
    std::fs::create_dir_all(&profile).unwrap();
    let marker = format!("cua-e2e-app-{:08x}", std::process::id());
    std::fs::write(
        profile.join("prefs.js"),
        format!("user_pref(\"cua.e2e.marker\", \"{marker}\");\n"),
    )
    .unwrap();
    std::fs::write(profile.join("places.sqlite"), b"synthetic history").unwrap();
    std::fs::write(profile.join("cookies.sqlite"), b"synthetic cookies").unwrap();
    let fake = Arc::new(FakeHost::new().with_home(host_home.path()));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(
        FirefoxProvider::new()
            .with_host(fake)
            .with_profile_dir(&profile),
    ));

    let mut cfg = CoreConfig::hermetic(&t.home);
    cfg.state_dir = Some(t.home.join("sandboxes"));
    cfg.app_sessions = Arc::new(AppSessions::from_registry(registry));
    cfg.probe_timeout = Duration::from_secs(20);
    cfg.daemon = DaemonMode::Auto {
        cua_bin: Some(t.cua_bin.clone()),
    };
    let app = AppCore::new(cfg);

    // The daemon the harness started is found through <home>/daemon.json.
    let d = app.daemon_status(false).await;
    assert!(d.connected, "cua daemon: {d:?}");

    // 1. Add Space by address.
    let row = app
        .add_space(&t.url, Some(t.token.clone()), Some("cua-e2e-app".into()))
        .await
        .unwrap();
    eprintln!("added {} features={:?}", row.id, row.features);
    assert!(row.reachable);
    assert_eq!(row.os.as_deref(), Some("linux"));
    assert!(row.features.iter().any(|f| f == "desktop_stream"));
    let listed = app.list_spaces().await.unwrap();
    assert!(listed.iter().any(|r| r.id == row.id && r.reachable));

    // 2. Screenshot + windows.
    let shot = app.space_screenshot(&row.id, Some(640)).await.unwrap();
    assert!(shot.starts_with("data:image/") && shot.len() > 200);
    let windows = app.list_remote_windows(&row.id).await.unwrap();
    eprintln!("{} windows", windows.len());

    // 3. Send a file; the guest's own sha256sum must agree.
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("cua-e2e-app.bin");
    let bytes: Vec<u8> = (0..(3 * 1024 * 1024u32))
        .map(|i| (i * 31 % 251) as u8)
        .collect();
    std::fs::write(&file, &bytes).unwrap();
    let want = hex(&Sha256::digest(&bytes));
    let sent = app
        .send_files(
            &row.id,
            &[file.to_string_lossy().into_owned()],
            Some("cua-e2e".into()),
        )
        .await
        .unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].sha256, want);
    let space = app.spaces().space(&row.id).await.unwrap();
    let out = space
        .bash(
            &format!("sha256sum '{}'", sent[0].dest),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(out.stdout.starts_with(&want), "guest: {}", out.render());

    // 4. Stream ticket (desktop), attached like the webview: no headers,
    //    server speaks first, first video packet is a keyframe.
    let ticket = app
        .open_stream(
            &row.id,
            StreamTargetArg::Display { display_id: None },
            StreamOpts {
                max_fps: Some(10),
                max_dimension: Some(960),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(ticket.via, "direct");
    assert_eq!(ticket.wire_version, 2);
    let a = attach(&ticket.ws_url).await;
    eprintln!("direct attach: {a:?}");
    assert_eq!(a.control.first().map(String::as_str), Some("hello"));
    assert!(a.control.iter().any(|c| c == "session_opened"));
    assert!(a.video_packets > 0, "frames arrive");
    assert_eq!(a.first_video_keyframe, Some(true), "keyframe on attach");
    app.close_stream(&row.id, &ticket.media_session_id)
        .await
        .unwrap();

    // 4b. The daemon media bridge (what Fleet Spaces use): register this
    //     Space with the daemon and attach through its loopback ticket URL.
    let daemon = app.daemon(false).await.unwrap();
    let name = "cua-e2e-app-direct";
    daemon
        .create(cua_daemon::CreateRequest {
            provider: Some(cua_sandbox_core::ProviderKind::Direct),
            name: Some(name.into()),
            url: Some(t.url.clone()),
            token: Some(t.token.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let bridged = daemon
        .open_media_bridge(
            name,
            Some(r#"{"target":{"displayId":"primary"},"maxFps":10,"maxDimension":960}"#.into()),
        )
        .await
        .unwrap();
    assert!(
        bridged.ws_url.starts_with("ws://127.0.0.1:"),
        "{}",
        bridged.ws_url
    );
    let b = attach(&bridged.ws_url).await;
    eprintln!("bridge attach: {b:?}");
    assert_eq!(b.control.first().map(String::as_str), Some("hello"));
    assert!(b.video_packets > 0, "frames arrive through the bridge");
    let _ = daemon.delete(name).await;

    // 5. Teleport the generated profile through the consent path.
    let manifest = app.teleport_manifest("firefox", "full").await.unwrap();
    let include: Vec<String> = manifest
        .items
        .iter()
        .filter(|i| i.default_checked)
        .map(|i| i.rel_path.clone())
        .collect();
    let has_sensitive = manifest
        .items
        .iter()
        .any(|i| i.sensitive && include.contains(&i.rel_path));
    let result = app
        .teleport_push("firefox", "full", &row.id, &include, has_sensitive)
        .await
        .unwrap();
    assert!(result.ok && !result.imported.is_empty(), "{result:?}");
    let found = space
        .bash(
            &format!("grep -rl '{marker}' ~/.mozilla ~/snap 2>/dev/null | head -1"),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(found.stdout.contains("prefs.js"), "{}", found.render());

    // 6. Delete (a Space added by address is only forgotten).
    let msg = app.delete_space(&row.id).await.unwrap();
    assert!(msg.contains("forgotten"), "{msg}");
    assert!(app.list_spaces().await.unwrap().is_empty());
}
