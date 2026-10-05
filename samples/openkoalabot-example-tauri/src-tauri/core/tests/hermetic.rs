// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The core against the in-process mock cua-spacesd on loopback
//! (`cua_spacesd_client::testing`, MIT): processes are simulated and files
//! live in memory, so nothing runs on or writes to the host. Nothing touches
//! the real home directory, an app, the keychain or `~/.cua`.
//!
//! The mock has no file transfers (`TeleportService.ReceiveFiles`), so a
//! drop is checked to fail loudly here; the bytes-and-SHA-256 path is covered
//! by the docker lane of the shared scenario (`samples/openkoalabot-example-scenario`).

use cua_spacesd_client::testing::{MockAuth, MockServer};
use openkoalabot_example_core::presence::Presence;
use openkoalabot_example_core::scenario::{Lane, run};
use openkoalabot_example_core::teleport::{ships_with_cua_spaces, teleport};
use openkoalabot_example_core::thread::{BotThread, roster};
use openkoalabot_example_core::{Core, CoreConfig, Space, files};
use std::path::Path;
use std::time::Duration;

const TOKEN: &str = "openkoalabots-test-token";

async fn mock(features: &[&str]) -> MockServer {
    let srv = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    srv.state.advertise(features);
    srv
}

async fn setup(dir: &Path, features: &[&str]) -> (MockServer, Core, Space) {
    let srv = mock(features).await;
    let core = Core::new(CoreConfig::in_dir(dir.join("state"))).unwrap();
    let info = core
        .add_space(&srv.url(), Some(TOKEN.into()), Some("hermetic".into()))
        .await
        .unwrap();
    let space = core.space(&info.id).await.unwrap();
    (srv, core, space)
}

#[tokio::test]
async fn add_list_and_delete_a_direct_space() {
    let dir = tempfile::tempdir().unwrap();
    let (_d, core, space) = setup(dir.path(), &[]).await;
    let listed = core.list_spaces().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, space.id().to_string());
    assert_eq!(listed[0].name, "hermetic");
    let msg = core.delete_space(&listed[0].id).await.unwrap();
    // Added by address: only forgotten, the machine is untouched.
    assert!(msg.contains("only forgotten"), "{msg}");
    assert!(core.list_spaces().unwrap().is_empty());
}

/// Agent runs read the guest user's `$HOME` first. The mock simulates only
/// a few programs and cannot answer that probe, so attaching is refused with
/// a typed reason instead of a thread that fails later; the roster says the
/// same. Attaching to a real guest, the roster and NotFound for an unknown
/// run are covered by the docker lane of the shared scenario, and real
/// harness runs by `cargo test -p cua-agents --test e2e_live`
/// (libs/cua/crates/cua-agents/tests/e2e/run-agents-e2e.sh).
#[tokio::test]
async fn an_agent_thread_needs_a_guest_with_a_home() {
    let dir = tempfile::tempdir().unwrap();
    let (_d, _core, space) = setup(dir.path(), &[]).await;
    let Err(e) = BotThread::new(&space, "claude-code").await else {
        panic!("attached without a guest home");
    };
    assert!(e.to_string().contains("home directory"), "{e}");
    let e = roster(&space).await.unwrap_err();
    assert!(e.to_string().contains("home directory"), "{e}");
}

/// A drop the Space cannot receive is an error, never a "delivered".
#[tokio::test]
async fn a_dropped_file_the_space_cannot_receive_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let (_d, _core, space) = setup(dir.path(), &[]).await;
    let p = dir.path().join("drop.bin");
    std::fs::write(&p, files::xorshift_bytes(200_000, 7)).unwrap();
    let e = files::send_verified(&space, &p, "openkoalabots")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("file transfers"), "{e}");
}

/// Session teleport ships with Cua Spaces: this in-process MIT runtime has
/// no teleport extension, so the call is refused with `HostCapabilityMissing`
/// before the approver sees anything, and nothing moves.
#[tokio::test]
async fn teleport_says_it_ships_with_cua_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let (_d, core, space) = setup(dir.path(), &[]).await;
    let mut asked = false;
    let e = teleport(&core, &space, "firefox", Some("full"), |_| {
        asked = true;
        None
    })
    .await
    .unwrap_err();
    assert!(!asked, "no manifest reached the approver");
    assert!(ships_with_cua_spaces(&e), "{e:?}");
    assert!(
        e.to_string()
            .contains("ships with Cua Spaces (source-available"),
        "{e}"
    );
}

#[tokio::test]
async fn presence_sees_a_second_client_join_move_and_leave() {
    let dir = tempfile::tempdir().unwrap();
    let (d, _core, space) = setup(dir.path(), &["presence"]).await;
    assert!(space.supports("presence"));
    let core_b = Core::new(CoreConfig::in_dir(dir.path().join("b"))).unwrap();
    let info = core_b
        .add_space(&d.url(), Some(TOKEN.into()), None)
        .await
        .unwrap();
    let space_b = core_b.space(&info.id).await.unwrap();
    let t = Duration::from_secs(10);
    let mut a = Presence::join(&space, "op", "Operator", false, t)
        .await
        .unwrap();
    let b = Presence::join(&space_b, "koala", "Koala", true, t)
        .await
        .unwrap();
    let bid = b.me().participant_id.clone();
    a.wait_for(t, 50, |e| matches!(e, cua_spaces::presence::PresenceEvent::Joined { participant } if participant.participant_id == bid))
        .await
        .unwrap();
    assert!(
        a.avatars()
            .iter()
            .any(|x| x.participant_id == bid && x.agent)
    );
    b.move_cursor(0.5, 0.25).await.unwrap();
    a.wait_for(t, 50, |e| matches!(e, cua_spaces::presence::PresenceEvent::CursorMoved { participant_id, .. } if *participant_id == bid))
        .await
        .unwrap();
    assert_eq!(
        a.avatars()
            .iter()
            .find(|x| x.participant_id == bid)
            .unwrap()
            .cursor,
        Some((0.5, 0.25))
    );
    b.leave().await.unwrap();
    a.wait_for(t, 50, |e| matches!(e, cua_spaces::presence::PresenceEvent::Left { participant_id, .. } if *participant_id == bid))
        .await
        .unwrap();
    assert_eq!(a.avatars().len(), 1, "only me");
    a.leave().await.unwrap();
}

#[tokio::test]
async fn the_desktop_stream_is_capability_gated() {
    let dir = tempfile::tempdir().unwrap();
    let (_d, _core, space) = setup(dir.path(), &[]).await;
    assert!(!space.supports("desktop_stream"));
    let e = openkoalabot_example_core::stream::open_ticket(&space, 5, 800)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("desktop_stream"), "{e}");
}

#[tokio::test]
async fn window_pip_streams_are_capability_gated() {
    let dir = tempfile::tempdir().unwrap();
    let (_d, _core, space) = setup(dir.path(), &[]).await;
    use openkoalabot_example_core::stream;
    let e = stream::open_window_ticket(&space, "", 5, 800)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("window id"), "{e}");
    assert!(!space.supports("window_stream"));
    assert!(stream::windows(&space).await.unwrap().is_empty());
    let e = stream::open_window_ticket(&space, "target-1", 5, 800)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("window_stream"), "{e}");
}

/// The whole shared scenario through this runner, on the fixture lane
/// against the mock. Steps the mock cannot serve end the way the app would
/// see them: the file drop fails (no file transfers), teleport skips (it
/// ships with Cua Spaces), and `delete` still runs.
#[tokio::test]
async fn the_shared_scenario_runs_against_the_mock() {
    let srv = mock(&[]).await;
    let spec_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../openkoalabot-example-scenario");
    let spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(spec_dir.join("scenario.json")).unwrap())
            .unwrap();
    let r = run(
        &spec,
        &spec_dir,
        Lane {
            name: "fixture".into(),
            url: Some(srv.url()),
            token: Some(TOKEN.into()),
            import_root: "/home/mock".into(),
            cloud_image: None,
        },
    )
    .await
    .unwrap();
    let summary: Vec<_> = r
        .steps
        .iter()
        .map(|s| format!("{}={} ({})", s.id, s.status, s.detail))
        .collect();
    let step = |id: &str| r.steps.iter().find(|s| s.id == id).unwrap();
    for (id, status) in [
        ("space", "pass"),
        ("stream", "skip"),
        // The shared step's fake CLI cannot drive an ACP run: see
        // `an_agent_thread_attaches_without_a_model`.
        ("agent", "skip"),
        ("file", "fail"),
        ("teleport", "skip"),
        ("presence", "skip"),
        ("delete", "pass"),
    ] {
        assert_eq!(step(id).status, status, "{id}: {summary:#?}");
    }
    assert!(
        step("file").detail.contains("file transfers"),
        "{summary:#?}"
    );
    assert!(
        step("teleport").detail.contains("ships with Cua Spaces"),
        "{summary:#?}"
    );
    assert!(!r.ok, "a failed step fails the run: {summary:#?}");
}
