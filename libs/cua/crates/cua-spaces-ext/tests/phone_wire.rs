// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the paired phone app asks of a machine's daemon, end to end through
//! the Spaces tools the relay's `HostThreadService` calls: approval scopes
//! and replay-safe answers, the tool rows of a running agent, steering and
//! queueing with a retry key, starting a task, and the push events. Against
//! the in-process spacesd mock and a temporary cua home: nothing runs in a
//! guest, nothing touches the host.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_push::PushEvent;
use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::{Persistent, bridge};
use cua_spacesd_client::Command;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const RUNS: &str = "/home/cua/.cua/agents";
const RUN: &str = "run-00000001";

struct Fx {
    srv: MockServer,
    home: tempfile::TempDir,
    spaces: Spaces,
    space: String,
    mcp: McpServer,
}

async fn setup() -> Fx {
    // The phone's daemon wire is the experiment `mobile`; its refusal while
    // off is `phone_gate.rs`.
    cua_spaces::experiments::set_enabled(cua_spaces::experiments::Gate::Mobile, Some(true));
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
    }
}

type Out = Result<Value, (String, String)>;

async fn call_as(fx: &Fx, caller: &Caller, tool: &str, args: Value) -> Out {
    let out = caller
        .clone()
        .scope(Box::pin(fx.mcp.call(tool, args)))
        .await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

/// A call made from the account's phone `device` (the relay vouched for it).
async fn call_as_phone(fx: &Fx, caller: &Caller, device: &str, tool: &str, args: Value) -> Out {
    let device = cua_spaces::extension::CallerDevice {
        id: Some(device.into()),
        phone: true,
    };
    let out = caller
        .clone()
        .scope_with_device(device, Box::pin(fx.mcp.call(tool, args)))
        .await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

fn relay(account: &str, name: &str, role: &str) -> Caller {
    Caller::Relay {
        account: account.into(),
        name: Some(name.into()),
        role: role.into(),
    }
}
fn owner() -> Caller {
    Caller::Owner
}
fn phone() -> Caller {
    // The owner's own phone: the relay vouches for the account as `owner`.
    relay("acct_dillon", "Dillon", "owner")
}
fn admin() -> Caller {
    relay("acct_ad", "Ana", "admin")
}
fn member() -> Caller {
    relay("acct_mia", "Mia", "shared")
}
fn guest() -> Caller {
    relay("acct_gus", "Gus", "viewer")
}

fn edit_record(fx: &Fx, name: &str, f: impl FnOnce(&mut Value)) {
    let path = fx.home.path().join("persistent").join("agents.json");
    let mut all: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let rec = all.iter_mut().find(|r| r["name"] == name).unwrap();
    f(rec);
    std::fs::write(&path, serde_json::to_vec_pretty(&all).unwrap()).unwrap();
}

fn run_state(fx: &Fx, state: Value) {
    let meta = json!({"run_id": RUN, "harness": "claude-code", "prompt": "hi",
                      "cwd": "/home/cua/work", "created_at": 0.0});
    fx.srv.state.put_file(
        &format!("{RUNS}/{RUN}/meta.json"),
        meta.to_string().into_bytes(),
    );
    fx.srv.state.put_file(
        &format!("{RUNS}/{RUN}/state.json"),
        state.to_string().into_bytes(),
    );
}

fn write_events(fx: &Fx, lines: &[Value]) {
    let mut log = String::new();
    for l in lines {
        log.push_str(&l.to_string());
        log.push('\n');
    }
    fx.srv
        .state
        .put_file(&format!("{RUNS}/{RUN}/events.jsonl"), log.into_bytes());
}

fn inbox(fx: &Fx) -> Vec<String> {
    let prefix = format!("{RUNS}/{RUN}/inbox/");
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
        .filter_map(|(_, b)| serde_json::from_slice::<Value>(&b).ok())
        .filter_map(|v| v["text"].as_str().map(str::to_string))
        .collect()
}

/// `ada` with an idle run at turn 1, and a process the supervisor can see.
async fn ada(fx: &Fx) -> (Persistent, cua_spacesd_client::ProcessHandle) {
    call_as(
        fx,
        &owner(),
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code",
               "env_from_host": ["ANTHROPIC_API_KEY"]}),
    )
    .await
    .unwrap();
    call_as(
        fx,
        &owner(),
        "agent_add",
        json!({"space": fx.space, "name": "bo", "harness": "claude-code",
               "base_url": "http://127.0.0.1:9/v1"}),
    )
    .await
    .unwrap();
    edit_record(fx, "ada", |r| r["run_id"] = json!(RUN));
    run_state(fx, json!({"status": "idle", "turn": 1}));
    write_events(
        fx,
        &[json!({"seq": 1, "ts": 0, "turn": 1, "type": "turn_started"})],
    );
    let s = fx.spaces.space(&fx.space).await.unwrap();
    let proc = s
        .spacesd()
        .unwrap()
        .spawn(
            Command::new("sleep")
                .arg("600000")
                .tag(format!("cua-agent/{RUN}")),
        )
        .await
        .unwrap();
    edit_record(fx, "ada", |r| {
        r["saved_turn"] = json!(1);
        r["cursor"] = json!(1);
    });
    (fx.spaces.persistent(), proc)
}

/// The approval an agent member posts through its bridge.
async fn asks(fx: &Fx, p: &Persistent, name: &str, action: &str) -> u64 {
    let rec = p.get(name).unwrap();
    let server = bridge::server(&fx.spaces, p.clone(), &rec, RUN);
    let reply = server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "thread_post", "arguments": {"text": "may I?",
                "approval": {"action": action, "details": "the details"}}}}))
        .await
        .unwrap();
    let posted: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(posted["entry"]["kind"], "approval");
    posted["entry"]["seq"].as_u64().unwrap()
}

async fn answer(fx: &Fx, who: &Caller, seq: u64, extra: Value) -> Out {
    let mut a = json!({"approval": seq, "approve": true});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        a[k] = v;
    }
    call_as(
        fx,
        who,
        "thread_post",
        json!({"space": fx.space, "text": "", "answer": a}),
    )
    .await
}

async fn entries(fx: &Fx) -> Vec<Value> {
    call_as(
        fx,
        &owner(),
        "thread_read",
        json!({"space": fx.space, "limit": 500}),
    )
    .await
    .unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

fn err(r: Out) -> String {
    r.unwrap_err().1
}

// ------------------------------------------------------------- approvals

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allow_for_this_space_answers_the_same_request_next_time() {
    let fx = setup().await;
    let (p, _proc) = ada(&fx).await;

    // The request says what an answer may choose, and what kind it is.
    let a1 = asks(&fx, &p, "ada", "Install a package").await;
    let read = call_as(&fx, &phone(), "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let e = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["seq"] == a1)
        .unwrap();
    assert_eq!(e["approval"]["scope_choices"], json!(["once", "space"]));
    assert_eq!(e["approval"]["kind"], "request");
    assert_eq!(read["open_approvals"], json!([a1]));

    // A second one of the same kind is waiting too ("1 of 2").
    let a1b = asks(&fx, &p, "ada", "install  a PACKAGE").await;
    let read = call_as(&fx, &phone(), "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(read["open_approvals"], json!([a1, a1b]));

    // The phone allows the first for this Space: the echo says so, the grant
    // is kept and the sibling is answered at once.
    let out = answer(
        &fx,
        &phone(),
        a1,
        json!({"scope": "space", "client_id": "answer-0001"}),
    )
    .await
    .unwrap();
    assert_eq!(out["entry"]["answer"]["scope"], "space");
    assert_eq!(out["entry"]["answer"]["client_id"], "answer-0001");
    let all = entries(&fx).await;
    let sibling = all
        .iter()
        .find(|e| e["kind"] == "answer" && e["answer"]["approval"] == a1b)
        .expect("the sibling was answered");
    assert_eq!(sibling["author"]["id"], "system:grant");
    assert_eq!(sibling["answer"]["approve"], true);
    let grants = call_as(&fx, &phone(), "thread_grants", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(grants["grants"][0]["agent"], "ada");
    assert_eq!(grants["grants"][0]["action"], "Install a package");

    // The next one is answered the moment it is posted, in the same write: it
    // is never open.
    let a2 = asks(&fx, &p, "ada", "Install a package").await;
    let read = call_as(&fx, &phone(), "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(read["open_approvals"], json!([]));
    let auto = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "answer" && e["answer"]["approval"] == a2)
        .expect("answered by the grant");
    assert_eq!(auto["answer"]["scope"], "space");
    assert_eq!(auto["seq"].as_u64().unwrap(), a2 + 1);

    // Another kind of request, and another agent's, still ask.
    let other = asks(&fx, &p, "ada", "Delete the build folder").await;
    let bo = asks(&fx, &p, "bo", "Install a package").await;
    let read = call_as(&fx, &phone(), "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(read["open_approvals"], json!([other, bo]));

    // Taking the grant back: only the owner and admins; then it asks again.
    let revoke = json!({"space": fx.space, "agent": "ada", "action": "install a package"});
    assert!(
        err(call_as(&fx, &member(), "thread_grant_revoke", revoke.clone()).await)
            .contains("only the owner and admins")
    );
    assert!(
        call_as(&fx, &guest(), "thread_grant_revoke", revoke.clone())
            .await
            .is_err()
    );
    assert_eq!(
        call_as(&fx, &admin(), "thread_grant_revoke", revoke)
            .await
            .unwrap()["revoked"],
        1
    );
    let a3 = asks(&fx, &p, "ada", "Install a package").await;
    let read = call_as(&fx, &phone(), "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert!(
        read["open_approvals"]
            .as_array()
            .unwrap()
            .contains(&json!(a3))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scope_the_request_does_not_offer_is_refused() {
    let fx = setup().await;
    let (p, _proc) = ada(&fx).await;
    // A decline is for one request only.
    let a = asks(&fx, &p, "ada", "buy").await;
    let msg = err(answer(
        &fx,
        &phone(),
        a,
        json!({"approve": false, "scope": "space"}),
    )
    .await);
    assert!(msg.contains("decline is for this request only"), "{msg}");
    // An unknown scope.
    let msg = err(answer(&fx, &phone(), a, json!({"scope": "forever"})).await);
    assert!(msg.contains("use \"once\" or \"space\""), "{msg}");
    // Still open: nothing was written by the refusals.
    let t = p.thread_store(&fx.space).fold().unwrap();
    assert_eq!(t.approvals.get(&a), Some(&false));
    // An ability request is the app's to remember: once only.
    let ab = asks(&fx, &p, "ada", "ability:spaces.list").await;
    let read = call_as(&fx, &phone(), "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let e = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["seq"] == ab)
        .unwrap();
    assert_eq!(e["approval"]["scope_choices"], json!(["once"]));
    assert_eq!(e["approval"]["kind"], "ability");
    assert!(
        err(answer(&fx, &phone(), ab, json!({"scope": "space"})).await).contains("one at a time")
    );

    // The host's push card is bound to one commit: once only, and the
    // host's own wait still hears a plain yes.
    let store = p.thread_store(&fx.space);
    let ask = {
        let store = store.clone();
        tokio::spawn(async move {
            cua_spaces_ext::persistent::host_tools::ask_owner_card(
                &store,
                cua_spaces_ext::persistent::host_tools::REPO_PUSH,
                "Ada wants to push a branch.",
                "details",
                Some(json!({"agent": "ada", "head": "0".repeat(40), "baseSha": "1".repeat(40)})),
                Duration::from_secs(20),
            )
            .await
        })
    };
    let mut seq = 0;
    for _ in 0..100 {
        if let Some(e) = entries(&fx)
            .await
            .iter()
            .find(|e| e["approval"]["action"] == "export:repo.push")
        {
            seq = e["seq"].as_u64().unwrap();
            assert_eq!(e["approval"]["kind"], "push");
            assert_eq!(e["approval"]["scope_choices"], json!(["once"]));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(seq > 0, "the card was asked");
    assert!(
        err(answer(&fx, &phone(), seq, json!({"scope": "space"})).await).contains("one at a time")
    );
    answer(&fx, &phone(), seq, json!({"scope": "once"}))
        .await
        .unwrap();
    let (asked, decision) = ask.await.unwrap().unwrap();
    assert_eq!(asked, seq);
    assert_eq!(
        decision,
        cua_spaces_ext::persistent::host_tools::Decision::Approved
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_sent_twice_is_one_answer_and_a_second_opinion_is_refused() {
    let fx = setup().await;
    let (p, _proc) = ada(&fx).await;
    let a = asks(&fx, &p, "ada", "buy").await;
    let first = answer(&fx, &phone(), a, json!({"client_id": "try-0001"}))
        .await
        .unwrap();
    assert!(first["entry"].get("replayed").is_none());
    let before = entries(&fx).await.len();
    // The retry after a lost connection: the first answer, written once.
    let again = answer(&fx, &phone(), a, json!({"client_id": "try-0001"}))
        .await
        .unwrap();
    assert_eq!(again["entry"]["seq"], first["entry"]["seq"]);
    assert_eq!(again["entry"]["replayed"], true);
    assert_eq!(entries(&fx).await.len(), before);
    // Without a key, the same decision by the same person is the same answer.
    assert_eq!(
        answer(&fx, &phone(), a, json!({})).await.unwrap()["entry"]["replayed"],
        true
    );
    // The other decision, or another person's, is refused and says what was decided.
    let msg = err(answer(&fx, &phone(), a, json!({"approve": false})).await);
    assert!(
        msg.ends_with(&format!("approval #{a} was already answered (allowed)")),
        "{msg}"
    );
    let msg = err(answer(&fx, &admin(), a, json!({})).await);
    assert!(msg.contains("already answered"), "{msg}");
    assert_eq!(entries(&fx).await.len(), before);
    // An answer to something that is not an approval, or is not there.
    assert!(answer(&fx, &phone(), 9999, json!({})).await.is_err());
    // Concurrent retries write once.
    let b = asks(&fx, &p, "ada", "buy again").await;
    let ph = phone();
    let sends = (0..6).map(|_| answer(&fx, &ph, b, json!({"client_id": "race-0001"})));
    let all = futures_util::future::join_all(sends).await;
    assert!(all.iter().all(Result::is_ok));
    let seqs: std::collections::BTreeSet<u64> = all
        .iter()
        .map(|r| r.as_ref().unwrap()["entry"]["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs.len(), 1, "{all:?}");
    let answers = entries(&fx)
        .await
        .iter()
        .filter(|e| e["kind"] == "answer" && e["answer"]["approval"] == b)
        .count();
    assert_eq!(answers, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_posted_twice_with_one_key_is_one_entry() {
    let fx = setup().await;
    let post = |text: &str, key: Value, who: Caller| {
        let fx = &fx;
        let text = text.to_string();
        async move {
            call_as(
                fx,
                &who,
                "thread_post",
                json!({"space": fx.space, "text": text, "client_id": key}),
            )
            .await
        }
    };
    let first = post("hello there", json!("post-0001"), phone())
        .await
        .unwrap();
    assert_eq!(first["entry"]["client_id"], "post-0001");
    assert!(first["entry"].get("replayed").is_none());
    let before = entries(&fx).await.len();
    let again = post("hello there", json!("post-0001"), phone())
        .await
        .unwrap();
    assert_eq!(again["entry"]["seq"], first["entry"]["seq"]);
    assert_eq!(again["entry"]["replayed"], true);
    assert_eq!(entries(&fx).await.len(), before);
    // The key is the author's: another person's post with it is a new post, and no key is no replay.
    let other = post("hello there", json!("post-0001"), admin())
        .await
        .unwrap();
    assert_ne!(other["entry"]["seq"], first["entry"]["seq"]);
    let nokey = call_as(
        &fx,
        &phone(),
        "thread_post",
        json!({"space": fx.space, "text": "hello there"}),
    )
    .await
    .unwrap();
    assert_ne!(nokey["entry"]["seq"], first["entry"]["seq"]);
    assert!(nokey["entry"].get("client_id").is_none());
    // A key too long is refused, and nothing is written.
    let n = entries(&fx).await.len();
    assert!(post("x", json!("k".repeat(200)), phone()).await.is_err());
    assert_eq!(entries(&fx).await.len(), n);
    // Concurrent retries write once.
    let ph = phone();
    let sends = (0..6).map(|_| {
        call_as(
            &fx,
            &ph,
            "thread_post",
            json!({"space": fx.space, "text": "racing", "client_id": "post-race-1"}),
        )
    });
    let all = futures_util::future::join_all(sends).await;
    let seqs: std::collections::BTreeSet<u64> = all
        .iter()
        .map(|r| r.as_ref().unwrap()["entry"]["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs.len(), 1, "{all:?}");
}

// ------------------------------------------------------------- tool rows

fn tool_call(seq: u64, id: &str, title: &str, status: &str, update: bool, ts: u64) -> Value {
    json!({"seq": seq, "ts": ts, "turn": 1, "type": "update", "update": {
        "toolCallId": id, "title": title, "kind": "execute", "status": status,
        "rawInput": {"command": title},
        "sessionUpdate": if update { "tool_call_update" } else { "tool_call" }}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_agents_calls_are_short_rows_under_the_task_they_follow() {
    let fx = setup().await;
    let (_p, _proc) = ada(&fx).await;
    let task = call_as(
        &fx,
        &owner(),
        "thread_post",
        json!({"space": fx.space, "text": "@ada run the tests"}),
    )
    .await
    .unwrap()["entry"]["seq"]
        .as_u64()
        .unwrap();
    let now = cua_volume::now_ms() + 10_000;
    write_events(
        &fx,
        &[
            json!({"seq": 1, "ts": now, "turn": 1, "type": "turn_started", "prompt": "SECRET MODEL TEXT"}),
            tool_call(
                2,
                "tc1",
                "pytest tests/test_cart.py -x",
                "in_progress",
                false,
                now + 1,
            ),
            tool_call(
                3,
                "tc1",
                "pytest tests/test_cart.py -x",
                "completed",
                true,
                now + 2,
            ),
            tool_call(4, "tc2", "ls src", "in_progress", false, now + 3),
        ],
    );
    let rows = call_as(
        &fx,
        &phone(),
        "thread_tool_rows",
        json!({"space": fx.space}),
    )
    .await
    .unwrap();
    let text = rows.to_string();
    assert!(!text.contains("SECRET MODEL TEXT"), "{text}");
    let groups = rows["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{rows}");
    assert_eq!(groups[0]["agent"], "ada");
    assert_eq!(groups[0]["after_seq"], task);
    let calls = groups[0]["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2, "{rows}");
    assert_eq!(calls[0]["id"], "tc1");
    assert_eq!(calls[0]["cat"], "ran");
    assert!(calls[0]["line"].as_str().unwrap().starts_with("Ran "));
    assert!(calls[0]["line"].as_str().unwrap().chars().count() <= 64);
    assert_eq!(calls[0]["state"], "done");
    assert_eq!(calls[1]["state"], "running");
    assert_eq!(rows["caught_up"], true);
    let c1 = rows["cursor"].as_u64().unwrap();

    // More events change the cursor; the same events do not.
    let again = call_as(
        &fx,
        &phone(),
        "thread_tool_rows",
        json!({"space": fx.space}),
    )
    .await
    .unwrap();
    assert_eq!(again["cursor"].as_u64().unwrap(), c1);
    // The log only grows: the same lines, and more after them.
    let mut log = vec![
        json!({"seq": 1, "ts": now, "turn": 1, "type": "turn_started", "prompt": "SECRET MODEL TEXT"}),
        tool_call(
            2,
            "tc1",
            "pytest tests/test_cart.py -x",
            "in_progress",
            false,
            now + 1,
        ),
        tool_call(
            3,
            "tc1",
            "pytest tests/test_cart.py -x",
            "completed",
            true,
            now + 2,
        ),
        tool_call(4, "tc2", "ls src", "in_progress", false, now + 3),
    ];
    log.push(tool_call(5, "tc2", "ls src", "failed", true, now + 4));
    // A made-up key id, spelled in two pieces so no scanner mistakes this file for a leak.
    let fake_key = format!("{}{}", "AKIA", "ABCDEFGHIJKLMNOP");
    log.push(tool_call(
        6,
        "tc3",
        &format!("aws s3 ls --secret {fake_key}"),
        "in_progress",
        false,
        now + 5,
    ));
    write_events(&fx, &log);
    let more = call_as(
        &fx,
        &phone(),
        "thread_tool_rows",
        json!({"space": fx.space, "cursor": c1}),
    )
    .await
    .unwrap();
    assert_ne!(more["cursor"].as_u64().unwrap(), c1);
    let calls = more["groups"][0]["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1]["state"], "failed");
    let secret = more.to_string();
    assert!(
        !secret.contains(&fake_key),
        "a credential never reaches the phone: {secret}"
    );

    // Waiting for a change returns at the wait's end when there is none.
    let c2 = more["cursor"].as_u64().unwrap();
    let t = Instant::now();
    let quiet = call_as(
        &fx,
        &phone(),
        "thread_tool_rows",
        json!({"space": fx.space, "cursor": c2, "follow_ms": 700}),
    )
    .await
    .unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(600),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(quiet["cursor"].as_u64().unwrap(), c2);
    // A group the phone has already is left out.
    let later = call_as(
        &fx,
        &phone(),
        "thread_tool_rows",
        json!({"space": fx.space, "after_seq": task + 1}),
    )
    .await
    .unwrap();
    assert_eq!(later["groups"], json!([]));
    // A guest may read what the thread's live row shows; a call that is not a Space tool is not offered to agents.
    call_as(
        &fx,
        &guest(),
        "thread_tool_rows",
        json!({"space": fx.space}),
    )
    .await
    .unwrap();
}

// -------------------------------------------------------- steer and queue

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_to_a_busy_agent_queues_or_steers_and_a_retry_changes_nothing() {
    let fx = setup().await;
    let (_p, _proc) = ada(&fx).await;
    run_state(&fx, json!({"status": "running", "turn": 2}));
    let ph = phone();
    let send = |text: &str, key: &str, force: bool| {
        call_as(
            &fx,
            &ph,
            "agent_message",
            json!({"space": fx.space, "name": "ada", "text": text,
                                                         "client_id": key, "force": force}),
        )
    };
    let q = send("and also the docs", "msg-0001", false).await.unwrap();
    assert_eq!(q["mode"], "queued");
    assert_eq!(q["delivered"], true);
    assert_eq!(inbox(&fx), ["and also the docs"]);
    // The thread shows what was sent, by the sender, without a second task.
    let seq = q["entry_seq"].as_u64().expect("recorded in the thread");
    let all = entries(&fx).await;
    let said = all.iter().find(|e| e["seq"] == seq).unwrap();
    assert_eq!(
        (said["text"].as_str(), said["author"]["id"].as_str()),
        (Some("and also the docs"), Some("acct_dillon"))
    );
    assert!(said["task"].is_null());
    // The retry is the first answer, and nothing is sent again.
    let r = send("and also the docs", "msg-0001", false).await.unwrap();
    assert_eq!(r["mode"], "queued");
    assert_eq!(r["replayed"], true);
    assert_eq!(r["entry_seq"], q["entry_seq"]);
    assert_eq!(inbox(&fx), ["and also the docs"]);
    assert_eq!(
        entries(&fx)
            .await
            .iter()
            .filter(|e| e["text"] == "and also the docs")
            .count(),
        1
    );

    // Steer: the running turn is interrupted, then the words go as a turn of their own.
    let state = fx.srv.state.clone();
    let idle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        state.put_file(
            &format!("{RUNS}/{RUN}/state.json"),
            json!({"status": "idle", "turn": 2})
                .to_string()
                .into_bytes(),
        );
    });
    let s = send("stop that, do this", "msg-0002", true).await.unwrap();
    idle.await.unwrap();
    assert_eq!(s["mode"], "steered", "{s}");
    assert!(
        inbox(&fx).contains(&"stop that, do this".to_string()),
        "{:?}",
        inbox(&fx)
    );

    // Idle: a plain send starts a turn.
    let started = send("one more thing", "msg-0003", false).await.unwrap();
    assert_eq!(started["mode"], "started");
    // A guest cannot send; a key too long is refused.
    assert!(
        call_as(
            &fx,
            &guest(),
            "agent_message",
            json!({"space": fx.space, "name": "ada", "text": "hi"})
        )
        .await
        .is_err()
    );
    assert!(send("x", &"k".repeat(200), false).await.is_err());
    // The members list says how many are waiting behind a running turn.
    let m = call_as(&fx, &phone(), "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    let ada = m["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "ada")
        .unwrap();
    assert!(ada["queued"].is_u64());
}

// --------------------------------------------------------------- tasks

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_started_in_an_existing_space_adds_the_agent_and_posts_the_job_once() {
    let fx = setup().await;
    let args = |key: &str| {
        json!({"space": fx.space, "client_id": key, "agent": "claude-code", "prompt": "fix the retry backoff",
               "where": {"kind": "space"}, "repo": {"url": "https://github.com/acme/app", "branch": "main"}})
    };
    let first = call_as(&fx, &phone(), "task_start", args("task-key-0001"))
        .await
        .unwrap();
    // Quick when the Space exists: ready, with the entry the job is.
    assert_eq!(first["state"], "ready", "{first}");
    assert_eq!(first["created"], false);
    assert_eq!(first["agent"], "claude");
    let seq = first["entry_seq"].as_u64().unwrap();
    let all = entries(&fx).await;
    let job = all.iter().find(|e| e["seq"] == seq).unwrap();
    assert_eq!(job["author"]["id"], "acct_dillon");
    assert!(
        job["text"]
            .as_str()
            .unwrap()
            .starts_with("@claude fix the retry backoff")
    );
    assert!(
        job["text"]
            .as_str()
            .unwrap()
            .contains("Repo: https://github.com/acme/app (branch main)")
    );
    assert_eq!(job["task"]["to"], "claude");
    // The agent is a member on standby.
    let members = call_as(&fx, &phone(), "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(members["members"][0]["name"], "claude");
    assert_eq!(members["members"][0]["harness"], "claude-code");

    // The same key again: the first answer, no second job, no second member.
    let again = call_as(&fx, &phone(), "task_start", args("task-key-0001"))
        .await
        .unwrap();
    assert_eq!(again["entry_seq"], first["entry_seq"]);
    assert_eq!(entries(&fx).await.len(), all.len());
    let status = call_as(
        &fx,
        &phone(),
        "task_status",
        json!({"client_id": "task-key-0001"}),
    )
    .await
    .unwrap();
    assert_eq!(status["state"], "ready");
    // One account's key is not another's.
    let other = call_as(
        &fx,
        &relay("acct_other", "Other", "owner"),
        "task_status",
        json!({"client_id": "task-key-0001"}),
    )
    .await;
    assert!(other.is_err());

    // What the phone must get right is refused, with nothing written.
    let before = entries(&fx).await.len();
    for bad in [
        json!({"space": fx.space, "client_id": "x", "prompt": "hi", "where": {"kind": "space"}}),
        json!({"space": fx.space, "client_id": "task-key-0002", "prompt": "", "where": {"kind": "space"}}),
        json!({"space": fx.space, "client_id": "task-key-0003", "prompt": "hi", "agent": "nonesuch", "where": {"kind": "space"}}),
        json!({"space": fx.space, "client_id": "task-key-0004", "prompt": "hi", "where": {"kind": "space"}, "repo": {"url": "http://example.com/x"}}),
        json!({"space": fx.space, "client_id": "task-key-0005", "prompt": "hi", "where": {"kind": "elsewhere"}}),
    ] {
        assert!(
            call_as(&fx, &phone(), "task_start", bad.clone())
                .await
                .is_err(),
            "{bad}"
        );
    }
    assert_eq!(entries(&fx).await.len(), before);

    // A new Space is the owner's to make: an editor and a guest are refused
    // before anything is made; an editor may post to an agent already there.
    let create = |key: &str| json!({"space": fx.space, "client_id": key, "prompt": "hi", "where": {"kind": "auto"}});
    assert!(
        err(call_as(&fx, &member(), "task_start", create("task-key-0006")).await)
            .contains("only the owner")
    );
    assert!(
        call_as(&fx, &guest(), "task_start", create("task-key-0007"))
            .await
            .is_err()
    );
    let editor = call_as(
        &fx,
        &member(),
        "task_start",
        json!({"space": fx.space, "client_id": "task-key-0008", "prompt": "check the docs", "where": {"kind": "space"}}),
    )
    .await
    .unwrap();
    assert_eq!(
        editor["state"], "ready",
        "the agent is in the Space already: {editor}"
    );
    // Nothing but the owner makes an OS other than Linux.
    let mac = call_as(
        &fx,
        &phone(),
        "task_start",
        json!({"space": fx.space, "client_id": "task-key-0009", "prompt": "hi", "os": "macos"}),
    )
    .await;
    assert!(err(mac).contains("Linux"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_cannot_start_more_tasks_than_the_limits_allow() {
    let fx = setup().await;
    let start = |key: String| json!({"space": fx.space, "client_id": key, "prompt": "check the docs", "where": {"kind": "space"}});
    // 20 an hour pass, the 21st is refused with the limit's own words and writes nothing.
    for i in 0..20 {
        let r = call_as(
            &fx,
            &phone(),
            "task_start",
            start(format!("task-limit-{i:04}")),
        )
        .await
        .unwrap();
        assert_eq!(r["state"], "ready", "{r}");
    }
    let before = entries(&fx).await.len();
    let refused = call_as(&fx, &phone(), "task_start", start("task-limit-0020".into())).await;
    assert_eq!(
        err(refused),
        "at most 20 tasks an hour can start from a phone: try again later"
    );
    assert_eq!(entries(&fx).await.len(), before);
    // The same key again is still the first answer, not a new start.
    let again = call_as(&fx, &phone(), "task_start", start("task-limit-0000".into()))
        .await
        .unwrap();
    assert_eq!(again["state"], "ready");
    // Another account has its own allowance.
    let other = call_as(
        &fx,
        &relay("acct_other", "Other", "owner"),
        "task_start",
        start("task-limit-9999".into()),
    )
    .await;
    assert!(other.is_ok(), "{other:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_options_say_what_a_machine_offers() {
    let fx = setup().await;
    let o = call_as(&fx, &phone(), "task_options", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(o["machine"]["can_create"], true);
    assert!(o["machine"].get("name").is_some());
    let agents = o["agents"].as_array().unwrap();
    assert_eq!(
        agents
            .iter()
            .map(|a| a["harness"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["claude-code", "openai-codex", "hermes", "openclaw"]
    );
    assert!(agents.iter().all(|a| a["ready"].is_boolean()));
    let m = call_as(&fx, &member(), "task_options", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(m["machine"]["can_create"], false);
    assert!(m["machine"]["why"].as_str().unwrap().contains("owner"));
}

// ------------------------------------------------------------------ push

struct Collect(Mutex<Vec<PushEvent>>);

impl cua_spaces_ext::push_client::Transport for Collect {
    fn send(
        &self,
        event: PushEvent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        self.0.lock().unwrap().push(event);
        Box::pin(async {})
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approval_that_waits_and_a_task_that_ends_become_push_events_without_their_words() {
    let sink = Arc::new(Collect(Mutex::default()));
    assert!(
        cua_spaces_ext::push_client::install(sink.clone()),
        "no other transport in a test process"
    );
    let fx = setup().await;
    let (p, _proc) = ada(&fx).await;
    let a = asks(&fx, &p, "ada", "Install a package").await;
    let mine = |sink: &Collect| -> Vec<PushEvent> {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.space.as_deref() == Some(fx.space.as_str()))
            .cloned()
            .collect()
    };
    let mut got = vec![];
    for _ in 0..100 {
        got = mine(&sink);
        if !got.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].kind, cua_push::EventKind::ApprovalNeeded);
    assert_eq!(got[0].entry, Some(a));
    assert_eq!(got[0].agent.as_deref(), Some("ada"));
    assert_eq!(got[0].scope_choices, ["once", "space"]);
    let json = serde_json::to_string(&got[0]).unwrap();
    assert!(
        !json.contains("the details") && !json.contains("may I?"),
        "{json}"
    );

    // Allowed for this Space: the next one asks nobody, so nothing is pushed.
    answer(&fx, &phone(), a, json!({"scope": "space"}))
        .await
        .unwrap();
    let before = mine(&sink).len();
    asks(&fx, &p, "ada", "Install a package").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(mine(&sink).len(), before);

    // A finished turn is the feed's notification.
    p.feed()
        .post_anchored(
            Some("ada"),
            "turn_ended",
            "ada",
            "MODEL WORDS THAT MUST STAY HOME",
            Some(RUN),
            Some(&fx.space),
            Some(7),
        )
        .unwrap();
    let mut done = vec![];
    for _ in 0..100 {
        done = mine(&sink)
            .into_iter()
            .filter(|e| e.kind == cua_push::EventKind::TaskFinished)
            .collect();
        if !done.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].entry, Some(7));
    assert!(
        !serde_json::to_string(&done[0])
            .unwrap()
            .contains("MODEL WORDS")
    );
}

/// What a phone answers is attributed to it, and what widens a Space's reach is
/// not the phone's to answer: a lost phone could otherwise widen a sandbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_standing_answer_names_the_phone_and_a_phone_does_not_widen_a_spaces_reach() {
    let fx = setup().await;
    let (p, _proc) = ada(&fx).await;
    let post = |seq: u64, extra: Value| {
        let mut a = json!({"approval": seq, "approve": true});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            a[k] = v;
        }
        json!({"space": fx.space, "text": "", "answer": a})
    };
    // A plain request, allowed for the Space from a phone: the grant says which.
    let a1 = asks(&fx, &p, "ada", "Install a package").await;
    call_as_phone(
        &fx,
        &phone(),
        "dev_phone_1",
        "thread_post",
        post(a1, json!({"scope": "space"})),
    )
    .await
    .unwrap();
    let grants = call_as(&fx, &phone(), "thread_grants", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(grants["grants"][0]["device"], "dev_phone_1");
    assert_eq!(grants["grants"][0]["from_phone"], true);
    // The same kind answered at the computer names no phone.
    let a2 = asks(&fx, &p, "ada", "Delete the cache").await;
    call_as(
        &fx,
        &owner(),
        "thread_post",
        post(a2, json!({"scope": "space"})),
    )
    .await
    .unwrap();
    let grants = call_as(&fx, &phone(), "thread_grants", json!({"space": fx.space}))
        .await
        .unwrap();
    let mine = grants["grants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["action"] == "Delete the cache")
        .unwrap();
    assert!(
        mine["device"].is_null() && mine["from_phone"] == false,
        "{mine}"
    );
    // An ability widens a Space's reach: refused from a phone, with the exact words;
    // the approval stays open, and the computer answers it.
    let ab = asks(&fx, &p, "ada", "ability:spaces.list").await;
    let (_, msg) = call_as_phone(
        &fx,
        &phone(),
        "dev_phone_1",
        "thread_post",
        post(ab, json!({})),
    )
    .await
    .unwrap_err();
    assert!(
        msg.contains("this widens what a Space can reach, so it is answered on the computer, not from a phone"),
        "{msg}"
    );
    assert_eq!(
        p.thread_store(&fx.space).fold().unwrap().approvals.get(&ab),
        Some(&false)
    );
    call_as(&fx, &owner(), "thread_post", post(ab, json!({})))
        .await
        .unwrap();
    assert_eq!(
        p.thread_store(&fx.space).fold().unwrap().approvals.get(&ab),
        Some(&true)
    );
}
