// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The host side of a persistent agent's bridge, against the in-process
//! spacesd mock: the pipe's files are read by offset, each request is
//! answered as the agent, and the agent's tools keep to the agent's rights
//! (drive rules, computer grants). Nothing runs in a guest; no host effects.

use std::time::{Duration, Instant};

use cua_spaces::Spaces;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::{AgentSpec, bridge};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const TOKEN: &str = "persistent-bridge-token-0123456789";
const RUN_DIR: &str = "/home/cua/.cua/agents/run-00000001";

async fn setup() -> (MockServer, tempfile::TempDir, Spaces, String) {
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
            Some("bridge-space".into()),
        )
        .await
        .unwrap();
    (srv, home, spaces, info.id)
}

fn rpc(id: u64, method: &str, params: Value) -> String {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    )
}

fn call(id: u64, tool: &str, args: Value) -> String {
    rpc(id, "tools/call", json!({"name": tool, "arguments": args}))
}

/// Waits until `path` holds `n` complete lines (bounded) and parses them.
async fn answers(srv: &MockServer, path: &str, n: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = srv
            .state
            .file(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let lines: Vec<Value> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        if lines.len() >= n || Instant::now() > deadline {
            assert_eq!(lines.len(), n, "answers in {path}: {text}");
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn by_id(lines: &[Value], id: u64) -> Value {
    lines
        .iter()
        .find(|l| l["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no answer {id} in {lines:?}"))
}

#[tokio::test]
async fn the_bridge_answers_as_the_agent_and_only_with_its_rights() {
    let (srv, _home, spaces, space_id) = setup().await;
    let p = spaces.persistent();
    let rec = p
        .create(AgentSpec {
            name: "ada".into(),
            harness: "claude-code".into(),
            space: space_id.clone(),
            ..Default::default()
        })
        .unwrap();
    // Another agent's home the bridge must not reach.
    spaces
        .extension::<cua_spaces_ext::DriveExtension>()
        .unwrap()
        .drive()
        .session(cua_volume::Context::user())
        .write(
            "agents/bob/secret-plan.md",
            b"bob's".to_vec(),
            cua_volume::Condition::None,
        )
        .await
        .unwrap();

    let space = spaces.space(&space_id).await.unwrap();
    let guest = space.spacesd().unwrap().clone();
    let server = bridge::server(&spaces, p.clone(), &rec, "run-00000001");
    let mut state = bridge::BridgeState::default();
    let pipe_in = format!("{RUN_DIR}/bridge/1-7.in");
    let pipe_out = format!("{RUN_DIR}/bridge/1-7.out");

    let mut requests = String::new();
    requests.push_str(&rpc(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18"}),
    ));
    requests.push_str(&rpc(2, "tools/list", json!({})));
    requests.push_str(&call(
        3,
        "notify_user",
        json!({"title": "Your research is ready", "body": "3 papers"}),
    ));
    requests.push_str(&call(
        4,
        "volume_write",
        json!({"path": "agents/ada/outputs/report.md", "content": "# Report"}),
    ));
    requests.push_str(&call(
        5,
        "volume_read",
        json!({"path": "agents/bob/secret-plan.md"}),
    ));
    requests.push_str(&call(
        6,
        "computer_call",
        json!({"machine": space_id, "tool": "screenshot"}),
    ));
    requests.push_str(&call(
        7,
        "space_bash",
        json!({"space": space_id, "command": "id"}),
    ));
    // Half a line: not answered until its newline arrives.
    requests.push_str("{\"jsonrpc\":\"2.0\",\"id\":8,");
    srv.state.put_file(&pipe_in, requests.into_bytes());
    srv.state.put_file(&pipe_out, vec![]);

    let n = bridge::pump(&guest, RUN_DIR, &mut state, &server)
        .await
        .unwrap();
    assert_eq!(n, 7, "seven complete requests");
    let lines = answers(&srv, &pipe_out, 7).await;

    let init = by_id(&lines, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "cua-agent");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("notify_user")
    );
    let names: Vec<String> = by_id(&lines, 2)["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"notify_user".to_string()), "{names:?}");
    assert!(names.contains(&"volume_write".to_string()), "{names:?}");
    assert!(
        !names
            .iter()
            .any(|n| n == "space_bash" || n == "create_space"),
        "the host's own Spaces tools never reach an agent: {names:?}"
    );

    assert_eq!(by_id(&lines, 3)["result"]["isError"], false);
    let feed = p.feed().list(false, None).unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].agent.as_deref(), Some("ada"));
    assert_eq!(feed[0].title, "Your research is ready");
    assert_eq!(feed[0].run_id.as_deref(), Some("run-00000001"));

    assert_eq!(by_id(&lines, 4)["result"]["isError"], false, "{lines:?}");
    let (bytes, _) = spaces
        .extension::<cua_spaces_ext::DriveExtension>()
        .unwrap()
        .drive()
        .session(cua_volume::Context::user())
        .read("agents/ada/outputs/report.md", None)
        .await
        .unwrap();
    assert_eq!(bytes, b"# Report");

    let denied = by_id(&lines, 5);
    assert_eq!(denied["result"]["isError"], true);
    assert_eq!(
        denied["result"]["structuredContent"]["error"]["kind"],
        "forbidden"
    );
    let no_grant = by_id(&lines, 6);
    assert_eq!(
        no_grant["result"]["structuredContent"]["error"]["kind"],
        "forbidden"
    );
    assert_eq!(
        by_id(&lines, 7)["error"]["code"],
        -32601,
        "unknown to the agent"
    );

    // Nothing new: nothing dispatched. The rest of the half line arrives.
    assert_eq!(
        bridge::pump(&guest, RUN_DIR, &mut state, &server)
            .await
            .unwrap(),
        0
    );
    let mut more = srv.state.file(&pipe_in).unwrap();
    more.extend_from_slice(b"\"method\":\"ping\"}\n");
    srv.state.put_file(&pipe_in, more);
    assert_eq!(
        bridge::pump(&guest, RUN_DIR, &mut state, &server)
            .await
            .unwrap(),
        1
    );
    let lines = answers(&srv, &pipe_out, 8).await;
    assert_eq!(by_id(&lines, 8)["result"], json!({}));

    // Refusals are in the computer-access audit, attributed to the agent.
    let (events, verdict) = p.access().audit().tail(10).unwrap();
    assert!(verdict.is_ok());
    assert!(
        events
            .iter()
            .any(|e| e.action == "denied" && e.principal == "agent:ada")
    );
}
