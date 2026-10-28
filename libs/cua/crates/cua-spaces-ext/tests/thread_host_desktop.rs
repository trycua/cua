// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The desktop row of this machine ("This Mac") has the thread of any
//! Space: people post and read there, though it is not a Space (nothing
//! runs there, so no agent is a member).

use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use serde_json::{Value, json};

async fn call(mcp: &McpServer, tool: &str, args: Value) -> Result<Value, (String, String)> {
    let out = Box::pin(mcp.call(tool, args)).await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

fn mcp(home: &std::path::Path) -> McpServer {
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home),
        cua_volume::Drive::open_local(home),
        None,
    )
    .build();
    McpServer::new(spaces)
}

#[tokio::test]
async fn this_machines_desktop_has_a_thread_people_can_post_and_read() {
    let home = tempfile::tempdir().unwrap();
    let mcp = mcp(home.path());
    let id = cua_spaces_ext::persistent::members::HOST_DESKTOP;
    assert_eq!(id, "this-mac");

    // Empty at first, and readable.
    let first = call(&mcp, "thread_read", json!({"space": id}))
        .await
        .expect("the desktop row reads like a Space");
    assert_eq!(
        first["entries"].as_array().map(Vec::len),
        Some(0),
        "{first}"
    );

    // A message is posted as the person, kept, and read back.
    let posted = call(
        &mcp,
        "thread_post",
        json!({"space": id, "text": "look at my screen"}),
    )
    .await
    .expect("a message posts");
    assert_eq!(posted["entry"]["text"], "look at my screen");
    let after = call(&mcp, "thread_read", json!({"space": id}))
        .await
        .unwrap();
    let texts: Vec<&str> = after["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert_eq!(texts, ["look at my screen"], "{after}");

    // Its members are none (nothing runs on the desktop), and it is its own
    // thread: a Space of the same name elsewhere would not share it.
    let members = call(&mcp, "agent_members", json!({"space": id}))
        .await
        .expect("members read");
    assert_eq!(
        members["members"].as_array().map(Vec::len),
        Some(0),
        "{members}"
    );

    // An unknown name is still an error, not a thread.
    let e = call(&mcp, "thread_read", json!({"space": "no-such-space"}))
        .await
        .unwrap_err();
    assert!(!e.1.is_empty());
}
