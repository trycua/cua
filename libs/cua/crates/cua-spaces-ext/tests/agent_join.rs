// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! How an agent joins a Space quickly: a posted task wakes the supervisor at
//! once (no waiting for the poll), a member with no run says it is joining
//! right away (and "woke up and joined" only once it has a session), and each
//! member is served on its own. Against the in-process spacesd mock and a
//! temporary cua home (the mock runs no shell, so a start cannot finish here).

use std::time::Duration;

use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

async fn call(mcp: &McpServer, tool: &str, args: Value) -> Result<Value, (String, String)> {
    let out = Box::pin(mcp.call(tool, args)).await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

struct Fx {
    _srv: MockServer,
    _home: tempfile::TempDir,
    spaces: Spaces,
    space: String,
    mcp: McpServer,
}

async fn setup() -> Fx {
    let srv = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home.path()),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let info = spaces
        .add(&format!("http://{}", srv.addr), None, Some("room".into()))
        .await
        .unwrap();
    let mcp = McpServer::new(spaces.clone());
    call(
        &mcp,
        "agent_add",
        json!({"space": info.id, "name": "ada", "harness": "claude-code"}),
    )
    .await
    .unwrap();
    Fx {
        _srv: srv,
        _home: home,
        spaces,
        space: info.id,
        mcp,
    }
}

/// Whether the agent has answered the task (the mock guest cannot run a
/// start, so the answer is its plain "could not start" reply).
fn answered(thread: &Value) -> bool {
    thread["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "message" && e["author"]["kind"] == "agent")
}

fn notes(thread: &Value) -> Vec<String> {
    thread["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "system")
        .filter_map(|e| e["text"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_posted_task_wakes_the_supervisor_without_waiting_for_the_poll() {
    let fx = setup().await;
    // The poll is a minute: only a wake can serve the task sooner.
    let supervisor = fx.spaces.spawn_supervisor(Duration::from_secs(60));
    // The first pass (it sets the member up) ends, and the supervisor sleeps.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let posted = std::time::Instant::now();
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada hello"}),
    )
    .await
    .unwrap();
    // The supervisor serves it well inside the minute.
    let mut waited = None;
    for _ in 0..40 {
        let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
            .await
            .unwrap();
        if answered(&t) {
            waited = Some(posted.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    supervisor.abort();
    let waited = waited.expect("a posted task wakes the supervisor");
    assert!(waited < Duration::from_secs(4), "served after {waited:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_with_no_run_is_not_said_to_have_joined_before_it_has() {
    let fx = setup().await;
    let p = fx.spaces.persistent();
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada look at the flaky test"}),
    )
    .await
    .unwrap();
    // (The mock guest runs no shell, so the start itself cannot finish.)
    Box::pin(p.tick()).await;
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert!(answered(&t), "the failed start is answered");
    let said = notes(&t);
    assert!(
        !said.iter().any(|n| n.contains("woke up and joined")),
        "joined waits for the agent being up: {said:?}"
    );
}
