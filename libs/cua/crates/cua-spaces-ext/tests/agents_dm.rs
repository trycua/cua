// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Agents tab's DM (CUA-2266) through the tools: `thread_read {with}`
//! is the caller's DM with one agent member (its own paging and read
//! cursor), and `agent_members {overview}` adds each agent's DM summary
//! and what waits for the caller. Against the in-process spacesd mock and
//! a temporary cua home: nothing runs, nothing touches the host.

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::bridge;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

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
    Fx {
        _srv: srv,
        _home: home,
        spaces,
        space: info.id,
        mcp,
    }
}

async fn call(fx: &Fx, caller: &Caller, tool: &str, args: Value) -> Value {
    let out = caller
        .clone()
        .scope(Box::pin(fx.mcp.call(tool, args)))
        .await;
    cua_spaces::extension::outcome_json(out).unwrap_or_else(|e| panic!("{tool}: {e}"))
}

fn mia() -> Caller {
    Caller::Relay {
        account: "acct_mia".into(),
        name: Some("Mia".into()),
        role: "shared".into(),
    }
}

fn seqs(page: &Value) -> Vec<String> {
    page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dm_is_my_part_of_the_thread_with_one_agent() {
    let fx = setup().await;
    let owner = Caller::Owner;
    for name in ["ada", "bo"] {
        call(
            &fx,
            &owner,
            "agent_add",
            json!({"space": fx.space, "name": name, "harness": "claude-code"}),
        )
        .await;
    }
    call(
        &fx,
        &owner,
        "thread_post",
        json!({"space": fx.space, "text": "@ada run the suite"}),
    )
    .await;
    call(
        &fx,
        &mia(),
        "thread_post",
        json!({"space": fx.space, "text": "@bo say hi"}),
    )
    .await;
    call(
        &fx,
        &owner,
        "thread_post",
        json!({"space": fx.space, "text": "lunch at noon"}),
    )
    .await;
    // ada asks for approval through its bridge, as a running agent does.
    let p = fx.spaces.persistent();
    let rec = p.get("ada").unwrap();
    let server = bridge::server(&fx.spaces, p.clone(), &rec, "run-1");
    server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "thread_post", "arguments": {"text": "may I push?",
                "approval": {"action": "push to main"}}}}))
        .await
        .unwrap();

    // The DM with ada: my task to it and its approval; not Mia's @bo, not my lunch note.
    let dm = call(
        &fx,
        &owner,
        "thread_read",
        json!({"space": fx.space, "with": "ada"}),
    )
    .await;
    let texts = seqs(&dm);
    assert!(texts.contains(&"@ada run the suite".into()), "{texts:?}");
    assert!(texts.contains(&"may I push?".into()), "{texts:?}");
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("@bo") || t.contains("lunch")),
        "{texts:?}"
    );
    assert_eq!(dm["with"], "ada");
    assert!(dm["unread"].as_u64().unwrap() >= 1);

    // The overview: ada waits for the owner; bo doesn't.
    let rows = call(
        &fx,
        &owner,
        "agent_members",
        json!({"space": fx.space, "overview": true}),
    )
    .await;
    let row = |n: &str| {
        rows["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == n)
            .unwrap()
            .clone()
    };
    let ada = row("ada");
    assert_eq!(ada["needs_you"][0]["kind"], "approval");
    assert_eq!(ada["needs_you"][0]["text"], "push to main");
    assert!(ada["dm"]["unread"].as_u64().unwrap() >= 1);
    assert_eq!(ada["dm"]["last"]["text"], "may I push?");
    assert_eq!(row("bo")["needs_you"], json!([]));
    // Without `overview`, the rows are as before.
    let plain = call(&fx, &owner, "agent_members", json!({"space": fx.space})).await;
    assert!(plain["members"][0].get("dm").is_none());

    // Reading the DM marks the DM read, not the whole thread.
    let thread_unread = call(
        &fx,
        &owner,
        "thread_read",
        json!({"space": fx.space, "limit": 1}),
    )
    .await["unread"]
        .clone();
    call(
        &fx,
        &owner,
        "thread_read",
        json!({"space": fx.space, "with": "ada", "mark_read": true}),
    )
    .await;
    let rows = call(
        &fx,
        &owner,
        "agent_members",
        json!({"space": fx.space, "overview": true}),
    )
    .await;
    let ada = rows["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "ada")
        .unwrap()
        .clone();
    assert_eq!(ada["dm"]["unread"], 0);
    let after = call(
        &fx,
        &owner,
        "thread_read",
        json!({"space": fx.space, "limit": 1}),
    )
    .await["unread"]
        .clone();
    assert_eq!(after, thread_unread);

    // Mia (a member) holds no approve right on ada: nothing waits for her, and her DM with ada is ada's entries only.
    let rows = call(
        &fx,
        &mia(),
        "agent_members",
        json!({"space": fx.space, "overview": true}),
    )
    .await;
    let ada = rows["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "ada")
        .unwrap()
        .clone();
    assert_eq!(ada["needs_you"], json!([]));
    let dm = call(
        &fx,
        &mia(),
        "thread_read",
        json!({"space": fx.space, "with": "ada"}),
    )
    .await;
    let said: Vec<&Value> = dm["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] != "system" && e["kind"] != "task_update")
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(said[0]["text"], "may I push?");
}
