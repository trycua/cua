// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The agent tool names (`agent_add`, `agent_send`, `agent_remove`,
//! `agent_schedule_*`) and their former names (`persistent_agent_*`,
//! `routine_*`) are one implementation: through the MCP server, a call by
//! either name reaches the same records and returns the same answer. Against
//! the in-process spacesd mock; nothing runs in a guest.

use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

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
        .add(&format!("http://{}", srv.addr), None, Some("box".into()))
        .await
        .unwrap();
    (srv, home, spaces, info.id)
}

/// Calls `tool` over JSON-RPC `tools/call`, as an MCP client does.
async fn call(server: &McpServer, tool: &str, args: Value) -> (bool, Value) {
    // Boxed: the dispatch future is large in debug builds.
    let r = Box::pin(
        server.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": tool, "arguments": args}})),
    )
    .await
    .unwrap();
    assert!(r.get("error").is_none(), "{tool}: {r}");
    let result = &r["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or("");
    let v = serde_json::from_str(text).unwrap_or(Value::String(text.into()));
    (result["isError"] == true, v)
}

/// Drops fields that differ between two otherwise identical records.
fn strip(mut v: Value) -> Value {
    for k in [
        "id",
        "createdAt",
        "created_ms",
        // When it became a member (the same moment as created_ms).
        "member_since_ms",
        "next_fire",
        "name",
        "botID",
    ] {
        if let Some(o) = v.as_object_mut() {
            o.remove(k);
        }
    }
    // The identity names the agent and when it joined: per agent, like `name`.
    if let Some(id) = v.get_mut("identity").and_then(Value::as_object_mut) {
        for k in ["handle", "display_name"] {
            id.remove(k);
        }
        if let Some(j) = id.get_mut("joined").and_then(Value::as_object_mut) {
            j.remove("at_ms");
        }
    }
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn former_and_current_agent_tool_names_are_one_implementation() {
    let (_srv, _home, spaces, space) = setup().await;
    let server = McpServer::new(spaces);

    // tools/list advertises only the current names.
    let list = Box::pin(server.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})))
        .await
        .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for n in [
        "agent_add",
        "agent_send",
        "agent_remove",
        "agent_schedule_add",
        "agent_schedule_list",
        "agent_schedule_remove",
        "agent_schedule_set_enabled",
    ] {
        assert!(names.contains(&n), "{n} is not listed");
    }
    for n in [
        "persistent_agent_create",
        "persistent_agent_send",
        "routine_add",
    ] {
        assert!(!names.contains(&n), "{n} is listed");
    }

    // Create one agent by each name: the same record.
    let mk = |name: &str| json!({"name": name, "agent": "hermes", "space": space});
    let (e1, a) = call(&server, "persistent_agent_create", mk("old-ada")).await;
    let (e2, b) = call(&server, "agent_add", mk("new-ada")).await;
    assert!(!e1 && !e2, "{a} {b}");
    assert_eq!(strip(a), strip(b));

    // Both appear in the one list.
    let (_, listed) = call(&server, "persistent_agent_list", json!({})).await;
    let listed: Vec<&str> = listed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert!(listed.contains(&"old-ada") && listed.contains(&"new-ada"));

    // A schedule added by either name is listed by either name.
    let sched = |agent: &str| json!({"agent": agent, "title": "Morning", "prompt": "plan", "daily_at": "08:00"});
    let (e1, r1) = call(&server, "routine_add", sched("old-ada")).await;
    let (e2, r2) = call(&server, "agent_schedule_add", sched("new-ada")).await;
    assert!(!e1 && !e2, "{r1} {r2}");
    assert_eq!(strip(r1.clone()), strip(r2.clone()));
    let (_, l1) = call(&server, "routine_list", json!({})).await;
    let (_, l2) = call(&server, "agent_schedule_list", json!({})).await;
    assert_eq!(l1, l2);
    assert_eq!(l1["routines"].as_array().unwrap().len(), 2);

    // Turning off and removing work by either name.
    let id1 = r1["id"].as_str().unwrap();
    let id2 = r2["id"].as_str().unwrap();
    let (_, o1) = call(
        &server,
        "routine_set_enabled",
        json!({"id": id1, "enabled": false}),
    )
    .await;
    let (_, o2) = call(
        &server,
        "agent_schedule_set_enabled",
        json!({"id": id2, "enabled": false}),
    )
    .await;
    assert_eq!(strip(o1), strip(o2));
    let (e1, _) = call(&server, "routine_remove", json!({"id": id1})).await;
    let (e2, _) = call(&server, "agent_schedule_remove", json!({"id": id2})).await;
    assert!(!e1 && !e2);

    // The same refusal by either name (no such agent).
    let (e1, m1) = call(
        &server,
        "persistent_agent_send",
        json!({"name": "nobody", "text": "hi"}),
    )
    .await;
    let (e2, m2) = call(
        &server,
        "agent_send",
        json!({"name": "nobody", "text": "hi"}),
    )
    .await;
    assert!(e1 && e2);
    assert_eq!(m1, m2);

    // Remove by either name.
    let (e1, _) = call(
        &server,
        "persistent_agent_remove",
        json!({"name": "old-ada"}),
    )
    .await;
    let (e2, _) = call(&server, "agent_remove", json!({"name": "new-ada"})).await;
    assert!(!e1 && !e2);
    let (_, left) = call(&server, "persistent_agent_list", json!({})).await;
    assert_eq!(left["agents"], json!([]));
}
