// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Deleting a Space ends the persistent agents that lived in it (no record
//! left "running" for a Space that is gone, nothing supervises it, its
//! schedules are dropped), and an agent whose local Space is not registered
//! (deleted behind the daemon's back) is marked gone when the supervisor
//! reconciles, with its home in the Cua Volume untouched. Against the
//! in-process spacesd mock and a temporary cua home.

use std::time::Duration;

use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::SpaceState;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

struct Fx {
    _srv: MockServer,
    home: tempfile::TempDir,
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
    Fx {
        _srv: srv,
        home,
        spaces,
        space: info.id,
        mcp,
    }
}

async fn call(mcp: &McpServer, tool: &str, args: Value) -> Value {
    let out = Box::pin(mcp.call(tool, args)).await;
    cua_spaces::extension::outcome_json(out).unwrap_or_else(|e| panic!("{tool}: {e}"))
}

async fn add_agent(fx: &Fx, name: &str) {
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": name, "harness": "claude-code"}),
    )
    .await;
    call(
        &fx.mcp,
        "agent_schedule_add",
        json!({"agent": name, "title": "Digest", "prompt": "summarize", "every_minutes": 60}),
    )
    .await;
}

#[tokio::test]
async fn deleting_a_space_removes_its_agents_and_schedules() {
    let fx = setup().await;
    add_agent(&fx, "ada").await;
    add_agent(&fx, "bob").await;
    let p = fx.spaces.persistent();
    assert_eq!(p.list().unwrap().len(), 2);
    assert_eq!(p.routines(None).await.unwrap().len(), 2);

    fx.spaces.delete(&fx.space).await.unwrap();

    let p = fx.spaces.persistent();
    assert!(p.list().unwrap().is_empty(), "no agent left for the Space");
    assert!(p.routines(None).await.unwrap().is_empty());
    // Nothing for the supervisor to tick, and no error to repeat.
    let report = p.tick().await;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

#[tokio::test]
async fn deleting_one_space_keeps_the_agents_of_another() {
    let fx = setup().await;
    add_agent(&fx, "ada").await;
    // A second record that names a different Space.
    let path = fx.home.path().join("persistent").join("agents.json");
    let mut all: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut other = all[0].clone();
    other["name"] = json!("elsewhere");
    other["space"] = json!("local:elsewhere");
    all.push(other);
    std::fs::write(&path, serde_json::to_vec(&all).unwrap()).unwrap();

    fx.spaces.delete(&fx.space).await.unwrap();

    let names: Vec<String> = fx
        .spaces
        .persistent()
        .list()
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, vec!["elsewhere".to_string()]);
}

#[tokio::test]
async fn an_agent_of_a_space_missing_from_the_registry_is_marked_gone_and_left_alone() {
    let fx = setup().await;
    add_agent(&fx, "ada").await;
    // The Space vanished without the delete path (a record that names a
    // local Space the registry never had).
    let path = fx.home.path().join("persistent").join("agents.json");
    let mut all: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    all[0]["space"] = json!("local:home-deleted");
    all[0]["run_id"] = json!("run-00000001");
    std::fs::write(&path, serde_json::to_vec(&all).unwrap()).unwrap();
    // The agent's home in the Cua Volume.
    let home_marker = fx.home.path().join("keep-me");
    std::fs::write(&home_marker, b"home").unwrap();

    let p = fx.spaces.persistent();
    // Not at once: a Space deleted and created again under its name is not
    // missed.
    assert!(p.reconcile_gone(Duration::from_secs(3600)).await.is_empty());
    assert_eq!(p.list().unwrap()[0].space_state, SpaceState::Running);

    let marked = p.reconcile_gone(Duration::ZERO).await;
    assert_eq!(marked, vec!["ada".to_string()]);
    let rec = p.get("ada").unwrap();
    assert_eq!(rec.space_state, SpaceState::Gone);
    assert!(rec.paused);
    assert!(rec.run_id.is_none());
    assert!(rec.last_error.unwrap().contains("was deleted"));
    assert!(p.routines(Some("ada")).await.unwrap().is_empty());
    assert!(home_marker.exists());

    // Supervised no more: a pass is quiet, a second reconcile marks nothing,
    // and resuming says to remove it.
    assert!(p.tick().await.errors.is_empty());
    assert!(p.reconcile_gone(Duration::ZERO).await.is_empty());
    assert!(p.resume("ada", None).await.is_err());
    // `agent rm` keeps working on it.
    p.remove("ada").await.unwrap();
    assert!(p.list().unwrap().is_empty());
}
