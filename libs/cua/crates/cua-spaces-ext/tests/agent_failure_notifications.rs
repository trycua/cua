// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A failed agent run or turn posts exactly one notification
//! ("<agent> stopped: <short reason>"), for `agent_start` runs (followed by
//! the supervisor once `agent_start` hands them over) and for persistent
//! agents. Against the in-process spacesd mock: the runs' files are written
//! by the test, as the runner would; nothing runs in a guest.

use cua_spaces::Spaces;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::{AgentSpec, Persistent};
use cua_spacesd_client::Command;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const RUNS: &str = "/home/cua/.cua/agents";
const AUTH_401: &str = r#"API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
/// What the runner's key check records (`acp-runner.mjs`) the moment the
/// provider rejects the run's key, instead of the harness's minutes of retries.
const KEY_CHECK_401: &str = "Failed to authenticate. Anthropic API Error: 401 API key is invalid.";

async fn setup() -> (MockServer, tempfile::TempDir, Spaces, String) {
    let srv = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home.path()),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let info = spaces
        .add(&format!("http://{}", srv.addr), None, Some("runs".into()))
        .await
        .unwrap();
    (srv, home, spaces, info.id)
}

/// Writes a run's meta and state as the runner does.
fn run(srv: &MockServer, run_id: &str, state: Value) {
    let meta = json!({"run_id": run_id, "harness": "claude-code", "prompt": "hi",
                      "cwd": "/home/cua/work", "created_at": 0.0});
    srv.state.put_file(
        &format!("{RUNS}/{run_id}/meta.json"),
        meta.to_string().into_bytes(),
    );
    set_state(srv, run_id, state);
}

fn set_state(srv: &MockServer, run_id: &str, state: Value) {
    srv.state.put_file(
        &format!("{RUNS}/{run_id}/state.json"),
        state.to_string().into_bytes(),
    );
}

/// Appends runner events (`(type, turn, message)`).
fn events(srv: &MockServer, run_id: &str, evs: &[(&str, u32, Option<&str>)]) {
    let path = format!("{RUNS}/{run_id}/events.jsonl");
    let mut log = srv
        .state
        .file(&path)
        .map(|b| String::from_utf8(b).unwrap())
        .unwrap_or_default();
    let seq = log.lines().count();
    for (i, (kind, turn, msg)) in evs.iter().enumerate() {
        let mut e = json!({"seq": seq + i + 1, "ts": 0, "turn": turn, "type": kind});
        if let Some(m) = msg {
            e["message"] = json!(m);
        }
        if *kind == "turn_ended" {
            e["stopReason"] = json!(if msg.is_some() { "error" } else { "end_turn" });
            e.as_object_mut().unwrap().remove("message");
        }
        log.push_str(&e.to_string());
        log.push('\n');
    }
    srv.state.put_file(&path, log.into_bytes());
}

/// The run's process, alive until killed.
async fn alive(spaces: &Spaces, space: &str, run_id: &str) -> cua_spacesd_client::ProcessHandle {
    let s = spaces.space(space).await.unwrap();
    s.spacesd()
        .unwrap()
        .spawn(
            Command::new("sleep")
                .arg("600000")
                .tag(format!("cua-agent/{run_id}")),
        )
        .await
        .unwrap()
}

fn errors(p: &Persistent) -> Vec<(String, String)> {
    p.feed()
        .list(false, None)
        .unwrap()
        .into_iter()
        .rev()
        .filter(|n| n.kind == "error")
        .map(|n| (n.title, n.body))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_start_run_posts_one_notification_per_failed_turn() {
    let (srv, _home, spaces, space) = setup().await;
    let p = spaces.persistent();
    let id = "run-0000000a";
    run(&srv, id, json!({"status": "running", "turn": 1}));
    let proc = alive(&spaces, &space, id).await;
    // agent_start hands the run over through the extension operation.
    spaces
        .call_extension(
            "agent runs",
            cua_spaces::agents::WATCH_OP,
            json!({"space": space, "run_id": id, "agent": "claude-code"}),
        )
        .await
        .unwrap();
    assert_eq!(p.watched_runs().unwrap().len(), 1);

    // The provider refuses the key: an error, then the turn ends.
    events(
        &srv,
        id,
        &[
            ("turn_started", 1, None),
            ("error", 1, Some(AUTH_401)),
            ("turn_ended", 1, Some("error")),
        ],
    );
    set_state(
        &srv,
        id,
        json!({"status": "idle", "turn": 1, "stopReason": "error"}),
    );
    p.tick().await;
    let n = errors(&p);
    assert_eq!(
        n,
        vec![(
            "Claude Code stopped: API Error: 401 invalid x-api-key".to_string(),
            AUTH_401.to_string()
        )]
    );
    // Later passes, and a second error of the same turn, add nothing.
    p.tick().await;
    events(
        &srv,
        id,
        &[(
            "error",
            1,
            Some("the agent exited unexpectedly (code 1, signal null)"),
        )],
    );
    p.tick().await;
    assert_eq!(errors(&p).len(), 1, "{:?}", errors(&p));
    assert!(
        p.feed()
            .list(false, None)
            .unwrap()
            .iter()
            .all(|n| n.kind != "turn_ended"),
        "no \"Finished.\" for a failed turn"
    );

    // A follow-up turn that fails again: its own notification.
    events(
        &srv,
        id,
        &[
            ("turn_started", 2, None),
            ("error", 2, Some("Authentication required")),
            ("turn_ended", 2, Some("error")),
        ],
    );
    p.tick().await;
    assert_eq!(errors(&p).len(), 2);
    assert_eq!(
        errors(&p)[1].0,
        "Claude Code stopped: Authentication required"
    );

    // The runner gives up (state failed, same turn): nothing more, and the
    // run is no longer followed once its process is gone.
    set_state(
        &srv,
        id,
        json!({"status": "failed", "turn": 2, "error": "agent exited"}),
    );
    proc.kill().await.unwrap();
    p.tick().await;
    assert_eq!(errors(&p).len(), 2);
    assert!(p.watched_runs().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_the_provider_rejects_at_the_start_posts_one_notification() {
    let (srv, _home, spaces, space) = setup().await;
    let p = spaces.persistent();
    let id = "run-0000000b";
    run(&srv, id, json!({"status": "running", "turn": 0}));
    let proc = alive(&spaces, &space, id).await;
    spaces
        .call_extension(
            "agent runs",
            cua_spaces::agents::WATCH_OP,
            json!({"space": space, "run_id": id, "agent": "claude-code"}),
        )
        .await
        .unwrap();
    // Nothing yet: the run is starting.
    p.tick().await;
    assert!(errors(&p).is_empty());

    // The first turn ends within seconds with the provider's message; the
    // runner then exits (`--exit-when-idle`).
    events(
        &srv,
        id,
        &[
            ("turn_started", 1, None),
            ("error", 1, Some(KEY_CHECK_401)),
            ("turn_ended", 1, Some("error")),
            ("run_exited", 1, None),
        ],
    );
    set_state(
        &srv,
        id,
        json!({"status": "exited", "turn": 1, "stopReason": "error"}),
    );
    proc.kill().await.unwrap();
    p.tick().await;
    assert_eq!(
        errors(&p),
        vec![(
            "Claude Code stopped: Failed to authenticate. Anthropic API Error: 401 API key is invalid"
                .to_string(),
            KEY_CHECK_401.to_string()
        )]
    );
    // Once, however many passes follow, and the run is let go.
    p.tick().await;
    p.tick().await;
    assert_eq!(errors(&p).len(), 1);
    assert!(p.watched_runs().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_that_fails_without_an_error_event_and_a_good_run() {
    let (srv, _home, spaces, space) = setup().await;
    let p = spaces.persistent();
    // Failed from its state only (the runner recorded why).
    run(
        &srv,
        "run-0000000b",
        json!({"status": "failed", "turn": 0, "error": "could not install claude-code"}),
    );
    // A run whose turn went well and whose runner then exited.
    run(&srv, "run-0000000c", json!({"status": "exited", "turn": 1}));
    events(
        &srv,
        "run-0000000c",
        &[
            ("turn_started", 1, None),
            ("message", 1, Some("OK")),
            ("turn_ended", 1, None),
        ],
    );
    for id in ["run-0000000b", "run-0000000c"] {
        p.watch_run(&space, id, "claude-code").unwrap();
    }
    p.tick().await;
    p.tick().await;
    assert_eq!(
        errors(&p),
        vec![(
            "Claude Code stopped: could not install claude-code".to_string(),
            "could not install claude-code".to_string()
        )]
    );
    assert_eq!(p.feed().list(false, None).unwrap().len(), 1);
    assert!(
        p.watched_runs().unwrap().is_empty(),
        "both processes are gone"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persistent_agent_failed_turn_is_one_stopped_notification() {
    let (srv, _home, spaces, space) = setup().await;
    let p = spaces.persistent();
    p.create(AgentSpec {
        name: "ada".into(),
        harness: "claude-code".into(),
        space: space.clone(),
        ..Default::default()
    })
    .unwrap();
    // Its current run (as a start would record it).
    let id = "run-0000000d";
    let path = p.dir().join("agents.json");
    let mut agents: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    agents[0]["run_id"] = json!(id);
    std::fs::write(&path, agents.to_string()).unwrap();
    run(&srv, id, json!({"status": "idle", "turn": 1}));
    let _proc = alive(&spaces, &space, id).await;
    events(
        &srv,
        id,
        &[
            ("turn_started", 1, None),
            ("error", 1, Some(AUTH_401)),
            ("turn_ended", 1, Some("error")),
        ],
    );
    // Whatever else a pass hits (saving the home in a mock guest), the
    // failure is posted once, however many passes read the turn.
    for _ in 0..3 {
        p.tick().await;
    }
    let all = p.feed().list(false, None).unwrap();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].kind, "error");
    assert_eq!(
        all[0].title,
        "ada stopped: API Error: 401 invalid x-api-key"
    );
    assert_eq!(all[0].agent.as_deref(), Some("ada"));
    // The pass recorded a problem (here: the mock cannot save the home).
    assert!(p.get("ada").unwrap().last_error.is_some());
}
