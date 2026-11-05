// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One clean reply: a turn that narrates, runs a tool, then answers posts
//! only the answer to the thread (narration and answer are never glued),
//! a text-only turn posts its text, and a turn that ends right after a tool
//! call posts the last thing it said. Against the in-process spacesd mock
//! and a temporary cua home: runs are files the test writes as the runner
//! would.

#![allow(dead_code)]

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::{Persistent, bridge};
use cua_spacesd_client::Command;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const RUNS: &str = "/home/cua/.cua/agents";

fn run(k: u32) -> String {
    format!("run-0000000{k}")
}

struct Fx {
    srv: MockServer,
    home: tempfile::TempDir,
    spaces: Spaces,
    space: String,
    mcp: McpServer,
    _procs: Vec<cua_spacesd_client::ProcessHandle>,
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
        srv,
        home,
        spaces,
        space: info.id,
        mcp,
        _procs: vec![],
    }
}

async fn call(mcp: &McpServer, tool: &str, args: Value) -> Result<Value, (String, String)> {
    let out = Box::pin(mcp.call(tool, args)).await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

fn agents_json(fx: &Fx) -> std::path::PathBuf {
    fx.home.path().join("persistent").join("agents.json")
}

fn edit_record(fx: &Fx, name: &str, f: impl FnOnce(&mut Value)) {
    let path = agents_json(fx);
    let mut all: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let rec = all.iter_mut().find(|r| r["name"] == name).unwrap();
    f(rec);
    std::fs::write(&path, serde_json::to_vec_pretty(&all).unwrap()).unwrap();
}

/// Session `k`'s run state, as the runner writes it.
fn run_state(fx: &Fx, k: u32, state: Value) {
    let r = run(k);
    let meta = json!({"run_id": r, "harness": "claude-code", "prompt": "hi",
                      "cwd": "/home/cua/work", "created_at": 0.0});
    fx.srv.state.put_file(
        &format!("{RUNS}/{r}/meta.json"),
        meta.to_string().into_bytes(),
    );
    fx.srv.state.put_file(
        &format!("{RUNS}/{r}/state.json"),
        state.to_string().into_bytes(),
    );
}

/// Appends raw runner event lines to session `k`'s run.
fn events(fx: &Fx, k: u32, evs: &[Value]) {
    let path = format!("{RUNS}/{}/events.jsonl", run(k));
    let mut log = fx
        .srv
        .state
        .file(&path)
        .map(|b| String::from_utf8(b).unwrap())
        .unwrap_or_default();
    let seq = log.lines().count();
    for (i, e) in evs.iter().enumerate() {
        let mut e = e.clone();
        e["seq"] = json!(seq + i + 1);
        e["ts"] = json!(0);
        log.push_str(&e.to_string());
        log.push('\n');
    }
    fx.srv.state.put_file(&path, log.into_bytes());
}

/// Prompts session `k`'s runner was handed, in order.
fn inbox(fx: &Fx, k: u32) -> Vec<String> {
    let prefix = format!("{RUNS}/{}/inbox/", run(k));
    let mut files: Vec<(String, Vec<u8>)> = fx
        .srv
        .state
        .files()
        .into_iter()
        .filter(|(p, _)| p.starts_with(&prefix))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|(_, b)| {
            serde_json::from_slice::<Value>(&b).unwrap()["text"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// A member `ada` with `n` sessions, each with an idle run at turn 1 the
/// supervisor has seen.
async fn member(fx: &mut Fx, n: u32) -> Persistent {
    let p = fx.spaces.persistent();
    let added = call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code", "sessions": n}),
    )
    .await
    .unwrap();
    assert_eq!(added["sessions"], n);
    edit_record(fx, "ada", |r| {
        r["run_id"] = json!(run(1));
        r["saved_turn"] = json!(1);
        r["cursor"] = json!(1);
        r["session_runs"] = json!(
            (2..=n)
                .map(|k| json!({"k": k, "run_id": run(k), "saved_turn": 1, "cursor": 1}))
                .collect::<Vec<_>>()
        );
    });
    let s = fx.spaces.space(&fx.space).await.unwrap();
    for k in 1..=n {
        run_state(fx, k, json!({"status": "idle", "turn": 1}));
        events(
            fx,
            k,
            &[json!({"turn": 1, "type": "turn_ended", "stopReason": "end_turn"})],
        );
        let proc = s
            .spacesd()
            .unwrap()
            .spawn(
                Command::new("sleep")
                    .arg("600000")
                    .tag(format!("cua-agent/{}", run(k))),
            )
            .await
            .unwrap();
        fx._procs.push(proc);
    }
    p
}

async fn post(fx: &Fx, args: Value) -> Result<u64, (String, String)> {
    let mut a = json!({"space": fx.space});
    a.as_object_mut()
        .unwrap()
        .extend(args.as_object().unwrap().clone());
    Ok(call(&fx.mcp, "thread_post", a).await?["entry"]["seq"]
        .as_u64()
        .unwrap())
}

async fn tasks(fx: &Fx) -> Vec<Value> {
    call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap()["tasks"]
        .as_array()
        .unwrap()
        .clone()
}

async fn task(fx: &Fx, seq: u64) -> Value {
    tasks(fx)
        .await
        .into_iter()
        .find(|t| t["seq"] == seq)
        .unwrap_or(Value::Null)
}

async fn sessions(fx: &Fx) -> Vec<Value> {
    let m = call(&fx.mcp, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    m["members"][0]["session_states"]
        .as_array()
        .unwrap()
        .clone()
}

/// A bridge tool call from session `k`'s run.
async fn bridge_call(fx: &Fx, p: &Persistent, k: u32, tool: &str, args: Value) -> Value {
    let rec = p.get("ada").unwrap().session(k);
    let server = bridge::server(&fx.spaces, p.clone(), &rec, &run(k));
    server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": tool, "arguments": args}}))
        .await
        .unwrap()
}

fn chunk(turn: u32, text: &str) -> Value {
    json!({"turn": turn, "type": "update", "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}})
}

fn tool(turn: u32) -> Value {
    json!({"turn": turn, "type": "update", "update": {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "ToolSearch", "kind": "other", "status": "pending"}})
}

fn ended(turn: u32) -> Value {
    json!({"turn": turn, "type": "turn_ended", "stopReason": "end_turn"})
}

/// Posts "@ada hi", delivers it, plays `turn` as the runner journals it, and
/// returns the text the thread got as ada's reply.
async fn reply_to_hi(turn: &[Value]) -> String {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    let seq = post(&fx, json!({"text": "@ada hi"})).await.unwrap();
    Box::pin(p.tick()).await;
    run_state(&fx, 1, json!({"status": "running", "turn": 2}));
    events(&fx, 1, turn);
    run_state(&fx, 1, json!({"status": "idle", "turn": 2}));
    Box::pin(p.tick()).await;
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let reply = t["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["reply_to"] == seq && e["kind"] == "message")
        .cloned()
        .unwrap_or_else(|| panic!("no reply in {t}"));
    let text = reply["text"].as_str().unwrap().to_string();
    println!("POSTED REPLY: {text:?}");
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn narration_before_a_tool_call_is_not_posted() {
    let text = reply_to_hi(&[
        chunk(
            2,
            "I'll check the thread and respond. Let me get the tool schemas first.",
        ),
        tool(2),
        chunk(2, "Hi! I'm ready to help. "),
        chunk(2, "What would you like me to do?"),
        ended(2),
    ])
    .await;
    assert_eq!(text, "Hi! I'm ready to help. What would you like me to do?");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_only_turn_posts_its_text() {
    let text = reply_to_hi(&[chunk(2, "Hi! "), chunk(2, "How can I help?"), ended(2)]).await;
    assert_eq!(text, "Hi! How can I help?");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_ending_on_a_tool_call_posts_the_last_text() {
    let text = reply_to_hi(&[chunk(2, "Looking into it."), tool(2), ended(2)]).await;
    assert_eq!(text, "Looking into it.");
}
