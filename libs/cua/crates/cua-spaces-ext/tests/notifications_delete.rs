// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `notifications_delete` over the MCP server, as every client (the apps, a
//! phone through the relay, `cua agent notifications`) reaches it: one entry,
//! several, or all; the feed and its unread count follow, and a new entry
//! posts after everything was deleted. Against the in-process spacesd mock.

use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

async fn call(server: &McpServer, tool: &str, args: Value) -> Value {
    let r = Box::pin(
        server.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": tool, "arguments": args}})),
    )
    .await
    .unwrap();
    assert!(r.get("error").is_none(), "{tool}: {r}");
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or("");
    serde_json::from_str(text).unwrap_or(Value::String(text.into()))
}

fn ids(v: &Value) -> Vec<String> {
    v["notifications"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletes_one_some_or_all_and_the_feed_follows() {
    let _srv = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home.path()),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let server = McpServer::new(spaces);
    let mut posted = vec![];
    for i in 0..4 {
        let r = call(&server, "notify_user", json!({"title": format!("n{i}")})).await;
        posted.push(r["id"].as_str().unwrap().to_string());
    }
    assert_eq!(
        call(&server, "notifications_ack", json!({"ids": [posted[0]]})).await["marked"],
        1
    );

    // One entry: the others keep their ids, newest first; unread follows.
    assert_eq!(
        call(&server, "notifications_delete", json!({"ids": [posted[1]]})).await["deleted"],
        1
    );
    let all = call(&server, "notifications_list", json!({})).await;
    assert_eq!(
        ids(&all),
        vec![posted[3].clone(), posted[2].clone(), posted[0].clone()]
    );
    let unread = call(&server, "notifications_list", json!({"unread_only": true})).await;
    assert_eq!(ids(&unread).len(), 2);

    // Nothing named is nothing deleted (an empty list is not "all").
    assert_eq!(
        call(&server, "notifications_delete", json!({"ids": []})).await["deleted"],
        0
    );
    assert_eq!(
        call(&server, "notifications_delete", json!({})).await["deleted"],
        0
    );
    assert_eq!(
        call(&server, "notifications_delete", json!({"ids": ["nope"]})).await["deleted"],
        0
    );
    assert_eq!(
        ids(&call(&server, "notifications_list", json!({})).await).len(),
        3
    );

    assert_eq!(
        call(&server, "notifications_delete", json!({"all": true})).await["deleted"],
        3
    );
    assert!(ids(&call(&server, "notifications_list", json!({})).await).is_empty());

    call(&server, "notify_user", json!({"title": "again"})).await;
    assert_eq!(
        ids(&call(&server, "notifications_list", json!({})).await).len(),
        1
    );
}
