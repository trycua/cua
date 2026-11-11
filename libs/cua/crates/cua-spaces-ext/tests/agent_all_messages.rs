// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Every message an agent says is appended to the thread: in order, none
//! replaced, none glued, none dropped, including words that arrive after
//! its turn ended (a background job finished) and a second `thread_reply`
//! under a task it already closed. Subagents it starts are recorded too. Against the in-process spacesd mock
//! and a temporary cua home: runs are files the test writes as the runner
//! would.

#![allow(dead_code)]

use cua_spaces::Spaces;
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
    // Closed tasks leave `thread_read`'s list: their state is the last update.
    let state = thread(fx)
        .await
        .iter()
        .rev()
        .find(|e| e["kind"] == "task_update" && e["update"]["task"] == seq)
        .map(|e| e["update"]["state"].clone())
        .unwrap_or(Value::Null);
    json!({"seq": seq, "state": state})
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

fn child_tool(turn: u32, id: &str) -> Value {
    json!({"turn": turn, "type": "update", "update": {"_meta": {"claudeCode": {"parentToolUseId": "toolu_sub"}}, "sessionUpdate": "tool_call", "toolCallId": id, "title": "ls", "kind": "execute", "status": "pending"}})
}

fn task_call(turn: u32, status: Option<&str>, background: bool) -> Value {
    let mut meta = json!({"subagent": true, "toolName": "Agent"});
    if background {
        meta["toolResponse"] = json!({"status": "async_launched"});
    }
    let mut u = json!({"_meta": {"claudeCode": meta}, "sessionUpdate": if status.is_some() {"tool_call_update"} else {"tool_call"},
        "toolCallId": "toolu_sub", "title": "Task", "kind": "think",
        "rawInput": {"description": "Map the desktop", "prompt": "List the files"}});
    if let Some(s) = status {
        u["status"] = json!(s);
    }
    json!({"turn": turn, "type": "update", "update": u})
}

/// Posts `text` to ada, delivers it, plays `turn` as the runner journals it
/// and returns the fixture, the member and the thread's entries afterwards.
async fn play(text: &str, turn: &[Value]) -> (Fx, Persistent, u64, Vec<Value>) {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    let seq = post(&fx, json!({"text": text})).await.unwrap();
    Box::pin(p.tick()).await;
    run_state(&fx, 1, json!({"status": "running", "turn": 2}));
    events(&fx, 1, turn);
    run_state(&fx, 1, json!({"status": "idle", "turn": 2}));
    Box::pin(p.tick()).await;
    let t = thread(&fx).await;
    (fx, p, seq, t)
}

async fn thread(fx: &Fx) -> Vec<Value> {
    call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

fn says(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e["kind"] == "message" && e["author"]["kind"] == "agent")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_message_of_a_turn_is_appended_in_order() {
    let (fx, _p, seq, t) = play(
        "@ada hi",
        &[
            chunk(2, "I'll look at the thread first."),
            tool(2),
            chunk(2, "Found it. "),
            chunk(2, "Setting things up now."),
            tool(2),
            chunk(2, "All set: the report is ready."),
            ended(2),
        ],
    )
    .await;
    assert_eq!(
        says(&t),
        [
            "I'll look at the thread first.",
            "Found it. Setting things up now.",
            "All set: the report is ready."
        ]
    );
    // All three answer the task, which is done.
    let replies = t
        .iter()
        .filter(|e| e["kind"] == "message" && e["reply_to"] == seq)
        .count();
    assert_eq!(replies, 3);
    assert_eq!(task(&fx, seq).await["state"], "done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_tick_does_not_post_anything_twice() {
    let (fx, p, _seq, t) = play(
        "@ada give me two facts about otters",
        &[
            chunk(2, "Otters hold hands."),
            tool(2),
            chunk(2, "Sea otters use tools."),
            ended(2),
        ],
    )
    .await;
    assert_eq!(says(&t).len(), 2);
    Box::pin(p.tick()).await;
    Box::pin(p.tick()).await;
    assert_eq!(says(&thread(&fx).await), says(&t));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn words_after_the_turn_ended_are_appended_not_dropped() {
    // The owner's thread: "Creating the space..." ended the turn, then a background
    // job failed and the agent woke up and explained, in the same turn number.
    let (fx, p, _seq, t) = play(
        "@ada start the long job",
        &[
            tool(2),
            chunk(2, "Starting the job, this may take a while."),
            ended(2),
        ],
    )
    .await;
    assert_eq!(says(&t), ["Starting the job, this may take a while."]);
    events(
        &fx,
        1,
        &[
            tool(2),
            chunk(2, "The job failed: "),
            chunk(2, "the gateway is missing."),
        ],
    );
    Box::pin(p.tick()).await;
    // Still being written: not posted yet, and not lost.
    tokio::time::sleep(std::time::Duration::from_millis(1700)).await;
    Box::pin(p.tick()).await;
    Box::pin(p.tick()).await;
    assert_eq!(
        says(&thread(&fx).await),
        [
            "Starting the job, this may take a while.",
            "The job failed: the gateway is missing."
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_can_say_more_under_a_task_it_closed() {
    let (fx, _p, seq, t) = play("@ada hello there", &[chunk(2, "Hello!"), ended(2)]).await;
    assert_eq!(task(&fx, seq).await["state"], "done");
    let p = fx.spaces.persistent();
    let rec = p.get("ada").unwrap();
    let late = bridge_call(
        &fx,
        &p,
        1,
        "thread_reply",
        json!({"task": seq, "text": "One more thing."}),
    )
    .await;
    assert!(late["result"]["isError"] != json!(true), "{late}");
    let all = says(&thread(&fx).await);
    assert_eq!(all, ["Hello!", "One more thing."], "{rec:?} {t:?}");
    assert_eq!(task(&fx, seq).await["state"], "done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_reply_run_again_by_a_restarted_bridge_is_not_added_twice() {
    let (fx, p, seq, _t) = play("@ada hello there", &[chunk(2, "Hello!"), ended(2)]).await;
    for _ in 0..2 {
        let r = bridge_call(
            &fx,
            &p,
            1,
            "thread_reply",
            json!({"task": seq, "text": "All set."}),
        )
        .await;
        assert!(r["result"]["isError"] != json!(true), "{r}");
    }
    assert_eq!(says(&thread(&fx).await), ["Hello!", "All set."]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_the_agent_posted_itself_is_not_posted_twice() {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    let seq = post(&fx, json!({"text": "@ada status?"})).await.unwrap();
    Box::pin(p.tick()).await;
    run_state(&fx, 1, json!({"status": "running", "turn": 2}));
    let said = bridge_call(
        &fx,
        &p,
        1,
        "thread_reply",
        json!({"task": seq, "text": "Working on it.", "state": "working"}),
    )
    .await;
    assert!(said["result"]["isError"] != json!(true), "{said}");
    events(
        &fx,
        1,
        &[
            chunk(2, "Working on it."),
            tool(2),
            chunk(2, "Done, all green."),
            ended(2),
        ],
    );
    run_state(&fx, 1, json!({"status": "idle", "turn": 2}));
    Box::pin(p.tick()).await;
    assert_eq!(
        says(&thread(&fx).await),
        ["Working on it.", "Done, all green."]
    );
}

/// A reply is told apart from an earlier one by the task it answers, not by its words: "Done." for the
/// second task is its own reply though the first task was answered with the same word.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_reply_with_the_same_words_is_not_taken_for_an_earlier_one() {
    let (fx, p, _seq, _t) = play("@ada first", &[chunk(2, "Done."), ended(2)]).await;
    post(&fx, json!({"text": "@ada second"})).await.unwrap();
    Box::pin(p.tick()).await;
    run_state(&fx, 1, json!({"status": "running", "turn": 3}));
    events(&fx, 1, &[chunk(3, "Done."), ended(3)]);
    run_state(&fx, 1, json!({"status": "idle", "turn": 3}));
    Box::pin(p.tick()).await;
    assert_eq!(says(&thread(&fx).await), ["Done.", "Done."]);
}

/// The same words after other words under one task are a new reply, not the retry of the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_words_after_other_words_under_one_task_are_a_new_reply() {
    let (fx, p, seq, _t) = play("@ada hello there", &[chunk(2, "Hello!"), ended(2)]).await;
    for (text, state) in [
        ("Done.", "working"),
        ("Next.", "working"),
        ("Done.", "done"),
    ] {
        let r = bridge_call(
            &fx,
            &p,
            1,
            "thread_reply",
            json!({"task": seq, "text": text, "state": state}),
        )
        .await;
        assert!(r["result"]["isError"] != json!(true), "{r}");
    }
    assert_eq!(
        says(&thread(&fx).await),
        ["Hello!", "Done.", "Next.", "Done."]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_silent_turn_still_ends_with_finished() {
    let (_fx, _p, _seq, t) = play("@ada tidy up", &[tool(2), ended(2)]).await;
    assert_eq!(says(&t), ["Finished."]);
}

fn subagent_notes(entries: &[Value]) -> Vec<(String, String)> {
    entries
        .iter()
        .filter(|e| e["event"]["kind"] == "subagent")
        .map(|e| {
            (
                e["event"]["state"].as_str().unwrap().to_string(),
                e["event"]["title"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subagent_is_recorded_when_it_starts_and_when_it_ends() {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    let seq = post(&fx, json!({"text": "@ada have a subagent map my files"}))
        .await
        .unwrap();
    Box::pin(p.tick()).await;
    run_state(&fx, 1, json!({"status": "running", "turn": 2}));
    events(
        &fx,
        1,
        &[
            task_call(2, None, false),
            child_tool(2, "c1"),
            child_tool(2, "c2"),
        ],
    );
    Box::pin(p.tick()).await;
    let t = thread(&fx).await;
    assert_eq!(
        subagent_notes(&t),
        [("running".to_string(), "Map the desktop".to_string())]
    );
    // The live row carries it while it runs.
    let live = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let rows = live["live"]["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(rows[0]["children"][0]["title"], "Map the desktop", "{live}");
    assert_eq!(rows[0]["children"][0]["steps"], 2);
    events(
        &fx,
        1,
        &[
            task_call(2, Some("completed"), false),
            chunk(2, "The tree is on your desktop."),
            ended(2),
        ],
    );
    run_state(&fx, 1, json!({"status": "idle", "turn": 2}));
    Box::pin(p.tick()).await;
    Box::pin(p.tick()).await;
    let t = thread(&fx).await;
    assert_eq!(
        subagent_notes(&t),
        [
            ("running".to_string(), "Map the desktop".to_string()),
            ("done".to_string(), "Map the desktop".to_string())
        ]
    );
    assert_eq!(says(&t), ["The tree is on your desktop."]);
    assert!(
        t.iter()
            .any(|e| e["event"]["kind"] == "subagent" && e["task_seq"] == seq)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subagent_of_a_new_run_is_recorded_though_its_events_number_from_one_again() {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    post(&fx, json!({"text": "@ada first job"})).await.unwrap();
    Box::pin(p.tick()).await;
    run_state(&fx, 1, json!({"status": "running", "turn": 2}));
    let mut many = vec![task_call(2, None, false)];
    many.extend((0..30).map(|i| child_tool(2, &format!("c{i}"))));
    events(&fx, 1, &many);
    events(
        &fx,
        1,
        &[
            task_call(2, Some("completed"), false),
            chunk(2, "Done."),
            ended(2),
        ],
    );
    run_state(&fx, 1, json!({"status": "idle", "turn": 2}));
    Box::pin(p.tick()).await;
    Box::pin(p.tick()).await;
    // The agent's run is replaced: the new one starts numbering its events at 1.
    let s = fx.spaces.space(&fx.space).await.unwrap();
    let proc = s
        .spacesd()
        .unwrap()
        .spawn(
            Command::new("sleep")
                .arg("600000")
                .tag(format!("cua-agent/{}", run(2))),
        )
        .await
        .unwrap();
    fx._procs.push(proc);
    edit_record(&fx, "ada", |r| {
        r["run_id"] = json!(run(2));
        r["cursor"] = json!(0);
        r["saved_turn"] = json!(0);
    });
    post(&fx, json!({"text": "@ada second job"})).await.unwrap();
    run_state(&fx, 2, json!({"status": "running", "turn": 1}));
    let mut second = vec![task_call(1, None, false)];
    second[0]["update"]["toolCallId"] = json!("toolu_second");
    second[0]["update"]["rawInput"]["description"] = json!("Second helper");
    events(&fx, 2, &second);
    Box::pin(p.tick()).await;
    let notes = subagent_notes(&thread(&fx).await);
    assert!(notes.iter().any(|(_, t)| t == "Second helper"), "{notes:?}");
}
