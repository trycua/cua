// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! An agent member with several sessions: tasks go to free sessions (or
//! the one `to_session` names), a task claim is a lease one session holds,
//! an expired claim requeues its task, a durable re-drive takes the claim
//! over at a higher epoch, and each session's state shows in
//! `agent_members`. Against the in-process spacesd mock and a temporary
//! cua home: runs are files the test writes as the runner would.

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

fn bridge_error(out: &Value) -> String {
    assert_eq!(out["result"]["isError"], true, "{out}");
    out["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tasks_go_to_the_lowest_free_session_or_the_one_named() {
    let mut fx = setup().await;
    let p = member(&mut fx, 3).await;
    // Named: session 2 takes it although session 1 is free.
    let a = post(
        &fx,
        json!({"text": "@ada run the e2e suite", "to_session": 2}),
    )
    .await
    .unwrap();
    // Any free session: the lowest one.
    let b = post(&fx, json!({"text": "@ada lint"})).await.unwrap();
    Box::pin(p.tick()).await;
    assert_eq!(inbox(&fx, 2).len(), 1);
    assert!(inbox(&fx, 2)[0].starts_with(&format!("[thread #{a} from Owner]")));
    assert_eq!(inbox(&fx, 1).len(), 1);
    assert!(inbox(&fx, 1)[0].starts_with(&format!("[thread #{b} from Owner]")));
    assert!(inbox(&fx, 3).is_empty());
    assert_eq!(task(&fx, a).await["session"], 2);
    assert_eq!(task(&fx, a).await["holder"], "ada/2@1");
    assert_eq!(task(&fx, b).await["holder"], "ada/1@1");
    let rec = p.get("ada").unwrap();
    assert_eq!(rec.task.as_ref().unwrap().seq, b);
    assert_eq!(rec.session(2).task.unwrap().seq, a);
    // Session 1 and 2 busy: the next one goes to session 3, and one named
    // for a busy session waits for it.
    run_state(&fx, 1, json!({"status": "running", "turn": 2}));
    run_state(&fx, 2, json!({"status": "running", "turn": 2}));
    let c = post(&fx, json!({"text": "@ada wait for two", "to_session": 2}))
        .await
        .unwrap();
    let d = post(&fx, json!({"text": "@ada docs"})).await.unwrap();
    Box::pin(p.tick()).await;
    assert_eq!(inbox(&fx, 3).len(), 1);
    assert!(inbox(&fx, 3)[0].contains("docs"));
    assert_eq!(task(&fx, c).await["state"], "queued");
    assert_eq!(task(&fx, d).await["session"], 3);
    // Session 2's turn ends: its reply closes task a, and c (named for it)
    // is delivered there.
    events(
        &fx,
        2,
        &[
            json!({"turn": 2, "type": "update", "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "suite green"}}}),
            json!({"turn": 2, "type": "turn_ended", "stopReason": "end_turn"}),
        ],
    );
    run_state(&fx, 2, json!({"status": "idle", "turn": 2}));
    Box::pin(p.tick()).await;
    assert_eq!(task(&fx, a).await, Value::Null, "task a is closed");
    assert_eq!(inbox(&fx, 2).len(), 2);
    assert!(inbox(&fx, 2)[1].contains("wait for two"));
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let reply = t["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["reply_to"] == a && e["kind"] == "message")
        .cloned()
        .unwrap();
    assert_eq!(reply["text"], "suite green");
    // A session outside the member's range is refused.
    let e = post(&fx, json!({"text": "@ada x", "to_session": 4}))
        .await
        .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
    assert!(e.1.contains("ada has 3 sessions"), "{}", e.1);
    let e = post(&fx, json!({"text": "no mention", "to_session": 1}))
        .await
        .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_session_gets_task_claimed_with_the_holder() {
    let mut fx = setup().await;
    let p = member(&mut fx, 3).await;
    // Session 1 is mid turn, so the dispatcher leaves the task to
    // whoever claims it first.
    for k in 1..=3 {
        run_state(&fx, k, json!({"status": "running", "turn": 2}));
    }
    let seq = post(&fx, json!({"text": "@ada triage the bug"}))
        .await
        .unwrap();
    Box::pin(p.tick()).await;
    assert_eq!(task(&fx, seq).await["state"], "queued");
    let won = bridge_call(&fx, &p, 2, "thread_claim", json!({"task": seq})).await;
    assert_eq!(won["result"]["isError"], false, "{won}");
    let body: Value =
        serde_json::from_str(won["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["claim"]["holder"], "ada/2@1");
    assert_eq!(body["claim"]["session"], 2);
    assert_eq!(body["claim"]["sessions"], 3);
    for k in [1, 3] {
        let lost = bridge_call(&fx, &p, k, "thread_claim", json!({"task": seq})).await;
        let msg = bridge_error(&lost);
        assert!(
            msg.contains("task_claimed") || msg.contains("claimed by ada/2@1"),
            "{msg}"
        );
        assert!(msg.contains("ada/2@1"), "{msg}");
        // ... and may not answer it either.
        let r = bridge_call(
            &fx,
            &p,
            k,
            "thread_reply",
            json!({"task": seq, "text": "mine"}),
        )
        .await;
        assert!(bridge_error(&r).contains("ada/2@1"));
    }
    // A person claiming for the agent gets the exact error code.
    let e = call(
        &fx.mcp,
        "thread_claim",
        json!({"space": fx.space, "task": seq, "session": 1}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "task_claimed");
    assert!(e.1.contains("ada/2@1"));
    let e = call(
        &fx.mcp,
        "thread_claim",
        json!({"space": fx.space, "task": seq, "session": 4}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
    // The holder answers it; the claim ends with the task.
    let r = bridge_call(
        &fx,
        &p,
        2,
        "thread_reply",
        json!({"task": seq, "text": "fixed"}),
    )
    .await;
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(task(&fx, seq).await, Value::Null);
    let claims =
        std::fs::read_to_string(p.thread_store(&fx.space).dir().join("claims.json")).unwrap();
    assert!(!claims.contains(&format!("\"{seq}\"")), "{claims}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_claim_requeues_its_task_with_a_note() {
    let mut fx = setup().await;
    let p = member(&mut fx, 2).await;
    for k in 1..=2 {
        run_state(&fx, k, json!({"status": "running", "turn": 2}));
    }
    let seq = post(&fx, json!({"text": "@ada research"})).await.unwrap();
    let won = bridge_call(&fx, &p, 2, "thread_claim", json!({"task": seq})).await;
    assert_eq!(won["result"]["isError"], false, "{won}");
    // Session 2's turn ended without answering it, so nobody renews its
    // claim: it runs out.
    run_state(&fx, 2, json!({"status": "idle", "turn": 2}));
    let path = p.thread_store(&fx.space).dir().join("claims.json");
    let mut claims: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    claims[seq.to_string()]["expires_ms"] = json!(1);
    std::fs::write(&path, claims.to_string()).unwrap();
    Box::pin(p.tick()).await;
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let entries = t["entries"].as_array().unwrap();
    let requeued = entries
        .iter()
        .find(|e| e["kind"] == "task_update" && e["update"]["state"] == "queued")
        .cloned()
        .expect("a queued update");
    assert_eq!(requeued["update"]["task"], seq);
    let note = entries
        .iter()
        .find(|e| e["kind"] == "system" && e["task_seq"] == seq)
        .cloned()
        .expect("a note about the task");
    assert!(
        note["text"].as_str().unwrap().contains("Session 2's claim"),
        "{note}"
    );
    // The same pass gave it to the lowest free session (1 is mid turn).
    let now = task(&fx, seq).await;
    assert_eq!(now["state"], "working", "{now}");
    assert_eq!(now["holder"], "ada/2@1");
    assert_eq!(inbox(&fx, 2).len(), 1);
    assert!(inbox(&fx, 1).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_durable_redrive_takes_the_claim_over_at_a_higher_epoch() {
    let mut fx = setup().await;
    let p = member(&mut fx, 2).await;
    let seq = post(&fx, json!({"text": "@ada long job", "to_session": 2}))
        .await
        .unwrap();
    Box::pin(p.tick()).await;
    assert_eq!(task(&fx, seq).await["holder"], "ada/2@1");
    // The session's runner crashed and was re-driven at epoch 2: the same
    // claim, now held at epoch 2 (before it ran out).
    p.session_redriven("ada", 2, 2).unwrap();
    let t = task(&fx, seq).await;
    assert_eq!(t["holder"], "ada/2@2");
    assert_eq!(t["state"], "working");
    assert_eq!(p.get("ada").unwrap().session(2).drive_epoch(), 2);
    assert_eq!(p.get("ada").unwrap().drive_epoch(), 1);
    // The supervisor renews it at epoch 2 and never requeues it.
    run_state(&fx, 2, json!({"status": "running", "turn": 2}));
    Box::pin(p.tick()).await;
    assert_eq!(task(&fx, seq).await["holder"], "ada/2@2");
    // An older epoch of the same session can no longer claim it.
    let store = p.thread_store(&fx.space);
    let e = store
        .claim(
            seq,
            &cua_spaces_ext::thread::claims::Holder::new("ada", 2, 1),
            60_000,
            Default::default(),
        )
        .unwrap_err();
    assert_eq!(e.tag(), "task_claimed");
    // The re-driven session answers it as the holder.
    let r = bridge_call(
        &fx,
        &p,
        2,
        "thread_reply",
        json!({"task": seq, "text": "done"}),
    )
    .await;
    assert_eq!(r["result"]["isError"], false, "{r}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_session_shows_its_state_and_activity() {
    let mut fx = setup().await;
    let p = member(&mut fx, 3).await;
    let s = sessions(&fx).await;
    assert_eq!(s.len(), 3);
    assert!(s.iter().all(|x| x["state"] == "idle"), "{s:?}");
    let m = call(&fx.mcp, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(m["members"][0]["status"], json!({"state": "idle"}));
    assert_eq!(m["members"][0]["sessions"], 3);
    // Session 1 works on a task and runs a tool; session 2 asks for an
    // approval; session 3's run failed.
    let a = post(&fx, json!({"text": "@ada fix the flaky test"}))
        .await
        .unwrap();
    let b = post(&fx, json!({"text": "@ada buy the domain", "to_session": 2}))
        .await
        .unwrap();
    Box::pin(p.tick()).await;
    run_state(&fx, 1, json!({"status": "running", "turn": 2}));
    events(
        &fx,
        1,
        &[
            json!({"turn": 2, "type": "turn_started"}),
            json!({"turn": 2, "type": "update", "update": {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "cargo test -p flaky", "kind": "execute", "status": "pending"}}),
        ],
    );
    let asked = bridge_call(
        &fx,
        &p,
        2,
        "thread_post",
        json!({"text": "May I buy example.dev for $12?", "approval": {"action": "buy"}}),
    )
    .await;
    assert_eq!(asked["result"]["isError"], false, "{asked}");
    run_state(
        &fx,
        3,
        json!({"status": "failed", "turn": 1, "error": "the agent exited (code 1)"}),
    );
    edit_record(&fx, "ada", |r| {
        r["session_runs"][1]["last_error"] = json!("the agent exited (code 1)")
    });
    Box::pin(p.tick()).await;
    let s = sessions(&fx).await;
    assert_eq!(s[0]["state"], "working", "{s:?}");
    assert_eq!(s[0]["task_seq"], a);
    assert_eq!(s[0]["activity"], "cargo test -p flaky");
    assert_eq!(s[1]["state"], "waiting", "{s:?}");
    assert_eq!(s[1]["task_seq"], b);
    assert!(
        s[1]["activity"]
            .as_str()
            .unwrap()
            .starts_with("waiting for an answer to #")
    );
    assert_eq!(s[2]["state"], "failed", "{s:?}");
    assert!(s[2]["activity"].as_str().unwrap().contains("exited"));
    let m = call(&fx.mcp, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(m["members"][0]["status"]["state"], "working");
    // The approval is answered: session 2 is no longer waiting.
    let approval = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "approval")
        .unwrap()["seq"]
        .as_u64()
        .unwrap();
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "yes", "answer": {"approval": approval, "approve": true}}),
    )
    .await
    .unwrap();
    assert_eq!(sessions(&fx).await[1]["state"], "working");
    // A secret in a tool title never becomes the activity line.
    events(
        &fx,
        1,
        &[
            json!({"turn": 2, "type": "update", "update": {"sessionUpdate": "tool_call", "toolCallId": "t2", "title": "curl -H 'Authorization: token ghp_0123456789abcdefghijklmnopqrstuvwxyzAB'", "kind": "execute", "status": "pending"}}),
        ],
    );
    Box::pin(p.tick()).await;
    let line = sessions(&fx).await[0]["activity"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!line.contains("ghp_"), "{line}");
    // Paused: every session says so.
    edit_record(&fx, "ada", |r| r["paused"] = json!(true));
    assert!(sessions(&fx).await.iter().all(|x| x["state"] == "paused"));
    // No run: asleep.
    edit_record(&fx, "ada", |r| {
        r["paused"] = json!(false);
        r["run_id"] = Value::Null;
        r["task"] = Value::Null;
    });
    assert_eq!(sessions(&fx).await[0]["state"], "asleep");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_sessions_a_member_is_one_session_and_old_shapes_hold() {
    let mut fx = setup().await;
    let p = member(&mut fx, 1).await;
    // A record from before sessions (no `sessions`, no `session_runs`,
    // and a field from a newer daemon inside a session) loads as one.
    edit_record(&fx, "ada", |r| {
        r.as_object_mut().unwrap().remove("sessions");
        r["session_runs"] = json!([{"k": 2, "run_id": null, "from_newer": 1}]);
    });
    let rec = p.get("ada").unwrap();
    assert_eq!(rec.sessions, 1);
    assert_eq!(rec.session_views().len(), 1);
    // An old client's post (no to_session) and claim (no session) work as
    // before; the claim is session 1's.
    run_state(&fx, 1, json!({"status": "running", "turn": 2}));
    let seq = post(&fx, json!({"text": "@ada hello"})).await.unwrap();
    let c = call(
        &fx.mcp,
        "thread_claim",
        json!({"space": fx.space, "task": seq}),
    )
    .await
    .unwrap();
    assert_eq!(c["update"]["state"], "claimed");
    assert_eq!(c["update"]["session"], 1);
    assert_eq!(c["claim"]["holder"], "ada/1@1");
    let s = sessions(&fx).await;
    assert_eq!(s.len(), 1);
    // Saving the file (another agent added) keeps the newer daemon's
    // field.
    p.create(cua_spaces_ext::persistent::AgentSpec {
        name: "other".into(),
        harness: "claude-code".into(),
        space: fx.space.clone(),
        ..Default::default()
    })
    .unwrap();
    let saved: Vec<Value> =
        serde_json::from_slice(&std::fs::read(agents_json(&fx)).unwrap()).unwrap();
    assert_eq!(saved[0]["session_runs"][0]["from_newer"], 1);
    // A relay guest that is not in the thread is still refused.
    let e = Caller::Relay {
        account: "acct_x".into(),
        name: None,
        role: "viewer".into(),
    }
    .scope(call(
        &fx.mcp,
        "thread_claim",
        json!({"space": fx.space, "task": seq}),
    ))
    .await
    .unwrap_err();
    assert_eq!(e.0, "permission_denied");
}
