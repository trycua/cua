// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the person's answer and the host's work do to the thread: an answer to an agent's own
//! request wakes the agent with the outcome (allowed or declined), and a Space the host makes for
//! an agent shows as one row's notes (creating with its step, then ready, or failed with the plain
//! reason). Against the in-process spacesd mock and a temporary cua home.

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
#[allow(dead_code)]
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

async fn entries(fx: &Fx) -> Vec<Value> {
    call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

/// ada asks (an approval of its own) and the owner answers it.
async fn ask_and_answer(fx: &Fx, p: &Persistent, approve: bool, note: &str) -> Vec<Value> {
    let out = bridge_call(
        fx,
        p,
        1,
        "thread_post",
        json!({"text": "May I make a macOS Space?", "approval": {"action": "ability:spaces.create", "details": "Create a macOS child Space named macos-child"}}),
    )
    .await;
    let posted: Value =
        serde_json::from_str(out["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let seq = posted["entry"]["seq"].as_u64().unwrap();
    let mut a = json!({"approval": seq, "approve": approve});
    if !note.is_empty() {
        a["note"] = json!(note);
    }
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "", "answer": a}),
    )
    .await
    .unwrap();
    entries(fx).await
}

fn woke(entries: &[Value]) -> Vec<&Value> {
    entries
        .iter()
        .filter(|e| e["author"]["id"] == "system:approval" && e["task"]["to"] == "ada")
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_allowed_request_wakes_the_agent_to_go_on() {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    let all = ask_and_answer(&fx, &p, true, "").await;
    let w = woke(&all);
    assert_eq!(w.len(), 1, "{all:?}");
    let t = w[0]["text"].as_str().unwrap();
    assert!(
        t.contains("allowed your request: Create a macOS child Space named macos-child"),
        "{t}"
    );
    assert!(t.contains("Go ahead now"), "{t}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declined_request_tells_the_agent_no_and_not_to_retry() {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    let all = ask_and_answer(&fx, &p, false, "not now").await;
    let w = woke(&all);
    assert_eq!(w.len(), 1, "{all:?}");
    let t = w[0]["text"].as_str().unwrap();
    assert!(
        t.contains("said no to your request") && t.contains("They said: not now"),
        "{t}"
    );
    assert!(
        t.contains("Do not try it again") && t.contains("do not look for a way around it"),
        "{t}"
    );
}

fn child_notes(all: &[Value]) -> Vec<&Value> {
    all.iter()
        .filter(|e| e["event"]["kind"] == "child_space")
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_space_the_host_makes_shows_its_steps_then_ready_or_the_plain_failure_once_each() {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    post(&fx, json!({"text": "@ada make two computers"}))
        .await
        .unwrap();
    Box::pin(p.tick()).await;
    let home = fx.home.path().to_path_buf();
    let parent = fx.space.clone();
    use cua_spaces::child_creates as cc;
    // One create is cloning, another is making a Linux Space.
    let mac = cc::start(&home, &parent, "macos-child", "macos-child", "macOS", 100);
    cc::update_id(&home, &mac, |e| {
        e.phase = cc::phase_words("creating", "macOS")
    });
    let build = cc::start(&home, &parent, "build", "build", "Linux", 101);
    cc::update_id(&home, &build, |e| {
        e.phase = cc::phase_words("booting", "Linux")
    });
    Box::pin(p.tick()).await;
    Box::pin(p.tick()).await;
    let notes = entries(&fx).await;
    let n = child_notes(&notes);
    assert_eq!(
        n.len(),
        2,
        "one note per create's step, not repeated: {n:?}"
    );
    assert!(n.iter().all(|e| e["event"]["state"] == "creating"));
    assert!(
        n.iter()
            .any(|e| e["event"]["phase"] == "Cloning the disk for the macOS Space")
    );
    // The first fails, the second is ready.
    cc::update_id(&home, &mac, |e| {
        e.state = cc::State::Failed;
        e.error =
            cc::person_reason("invalid request: probe port 3211 is not published by this sandbox");
        e.ended_ms = Some(110);
    });
    cc::update_id(&home, &build, |e| {
        e.state = cc::State::Ready;
        e.space = "local:build".into();
        e.phase = "Ready".into();
        e.ended_ms = Some(111);
    });
    Box::pin(p.tick()).await;
    Box::pin(p.tick()).await;
    let notes = entries(&fx).await;
    let n = child_notes(&notes);
    assert_eq!(n.len(), 4, "{n:?}");
    let failed = n.iter().find(|e| e["event"]["state"] == "failed").unwrap();
    let text = failed["text"].as_str().unwrap();
    assert_eq!(
        text,
        "Couldn't create the macOS Space macos-child: its network setup did not come up. ada was told."
    );
    assert!(
        !text.chars().any(|c| c.is_ascii_digit()) && !text.contains('#'),
        "{text}"
    );
    let ready = n.iter().find(|e| e["event"]["state"] == "ready").unwrap();
    assert_eq!(ready["event"]["space"], "local:build");
    // Each create's notes share one key, so a client folds them into one row.
    let mac_keys: std::collections::BTreeSet<_> = n
        .iter()
        .filter(|e| e["event"]["name"] == "macos-child")
        .map(|e| e["event"]["key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(mac_keys.len(), 1);
}

/// A step's percentage keeps moving in the thread, and a create is told once, by the member it was
/// attributed to, on the task that member was on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_steps_progress_keeps_moving_and_a_create_belongs_to_one_member_and_task() {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    let task = post(&fx, json!({"text": "@ada make a computer"}))
        .await
        .unwrap();
    Box::pin(p.tick()).await;
    let home = fx.home.path().to_path_buf();
    use cua_spaces::child_creates as cc;
    let id = cc::start(&home, &fx.space, "big", "big", "Linux", 100);
    for pct in [10u8, 40, 70] {
        cc::update_id(&home, &id, |e| {
            e.phase = "Downloading the image".into();
            e.percent = Some(pct);
        });
        Box::pin(p.tick()).await;
    }
    let all = entries(&fx).await;
    let n = child_notes(&all);
    let pcts: Vec<_> = n.iter().map(|e| e["event"]["percent"].clone()).collect();
    assert_eq!(pcts, [json!(10), json!(40), json!(70)], "{n:?}");
    let mine = cc::of(&home, &fx.space);
    assert_eq!(mine[0].agent, "ada");
    assert_eq!(mine[0].task, task, "attributed to the task it was asked on");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_nobody_answers_in_time_is_closed_and_the_agent_is_told() {
    // SAFETY: set before any tick of this test runs; the other tests here have no open request.
    unsafe { std::env::set_var("CUA_APPROVAL_TTL_SECS", "0") };
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    let out = bridge_call(
        &fx,
        &p,
        1,
        "thread_post",
        json!({"text": "May I?", "approval": {"action": "ability:spaces.create", "details": "Create a macOS child Space named macos-child"}}),
    )
    .await;
    let posted: Value =
        serde_json::from_str(out["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let seq = posted["entry"]["seq"].as_u64().unwrap();
    Box::pin(p.tick()).await;
    Box::pin(p.tick()).await;
    let all = entries(&fx).await;
    let closed: Vec<_> = all
        .iter()
        .filter(|e| e["kind"] == "answer" && e["answer"]["approval"] == seq)
        .collect();
    assert_eq!(closed.len(), 1, "closed once, not per tick: {all:?}");
    assert_eq!(closed[0]["answer"]["approve"], false);
    let t = woke(&all);
    assert_eq!(t.len(), 1, "{all:?}");
    let text = t[0]["text"].as_str().unwrap();
    assert!(
        text.starts_with(
            "Nobody answered your request in time: Create a macOS child Space named macos-child."
        ),
        "{text}"
    );
    assert!(text.contains("Nothing was done"));
    unsafe { std::env::remove_var("CUA_APPROVAL_TTL_SECS") };
}
