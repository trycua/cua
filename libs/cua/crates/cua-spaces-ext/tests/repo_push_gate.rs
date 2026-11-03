// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `repo_push` belongs to the Ask first experiment. Against the in-process
//! spacesd mock: while it is off the agent is not offered the tool and a call
//! of it is an unknown tool, as today (nothing is measured, asked or sent);
//! while it is on the tool is listed. Nothing runs in a guest.

use cua_spaces::Spaces;
use cua_spaces::experiments::{Gate, hold};
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::{AgentSpec, bridge};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const TOKEN: &str = "repo-push-gate-token-0123456789";

async fn bridge_for_ada() -> (MockServer, tempfile::TempDir, cua_spaces::mcp::McpServer) {
    let srv = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home.path()),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let info = spaces
        .add(
            &format!("http://{}", srv.addr),
            Some(TOKEN.into()),
            Some("gate".into()),
        )
        .await
        .unwrap();
    let p = spaces.persistent();
    let rec = p
        .create(AgentSpec {
            name: "ada".into(),
            harness: "claude-code".into(),
            space: info.id,
            ..Default::default()
        })
        .unwrap();
    let server = bridge::server(&spaces, p.clone(), &rec, "run-00000001");
    (srv, home, server)
}

async fn rpc(server: &cua_spaces::mcp::McpServer, method: &str, params: Value) -> Value {
    server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .await
        .expect("an answer")
}

fn names(listing: &Value) -> Vec<String> {
    listing["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn while_ask_first_is_off_the_agent_is_not_offered_repo_push_and_a_call_is_an_unknown_tool() {
    let _off = hold(Gate::AskFirst, false);
    let (_srv, _home, server) = bridge_for_ada().await;
    let listing = rpc(&server, "tools/list", json!({})).await;
    let tools = names(&listing);
    assert!(!tools.iter().any(|t| t == "repo_push"), "{tools:?}");
    assert!(tools.iter().any(|t| t == "notify_user"), "{tools:?}");
    let reply = rpc(
        &server,
        "tools/call",
        json!({"name": "repo_push", "arguments": {"path": "/home/cua/work", "repo": "acme/app", "branch": "fix"}}),
    )
    .await;
    // Exactly what an agent gets today for a tool that does not exist.
    assert_eq!(reply["error"]["code"], -32601, "{reply}");
    assert!(
        reply.to_string().contains("unknown tool repo_push"),
        "{reply}"
    );
}

#[tokio::test]
async fn while_ask_first_is_on_the_agent_is_offered_repo_push() {
    let _on = hold(Gate::AskFirst, true);
    let (_srv, _home, server) = bridge_for_ada().await;
    let listing = rpc(&server, "tools/list", json!({})).await;
    let tools = names(&listing);
    assert!(tools.iter().any(|t| t == "repo_push"), "{tools:?}");
    // The tool says the user can say no and that the agent holds no access.
    let t = listing["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "repo_push")
        .unwrap();
    let d = t["description"].as_str().unwrap();
    assert!(d.contains("you hold no GitHub access"), "{d}");
}
