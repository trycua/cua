// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Agents in a Space: the thread, agent members, wake-on-message and the
//! compatibility of everything that came before. Against the in-process
//! spacesd mock and a temporary cua home: runs are files the test writes as
//! the runner would, nothing runs in a guest, nothing touches the host.

use std::time::Duration;

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::{AgentSpec, Persistent, bridge};
use cua_spacesd_client::Command;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const RUNS: &str = "/home/cua/.cua/agents";
const RUN: &str = "run-00000001";
const SECRET: &str = "ghp_0123456789abcdefghijklmnopqrstuvwxyzAB";

struct Fx {
    srv: MockServer,
    home: tempfile::TempDir,
    spaces: Spaces,
    space: String,
    mcp: McpServer,
}

fn runtime(home: &std::path::Path) -> Spaces {
    cua_spaces_ext::register(
        Spaces::builder().home(home),
        cua_volume::Drive::open_local(home),
        None,
    )
    .build()
}

async fn setup() -> Fx {
    let srv = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let spaces = runtime(home.path());
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

/// A tool call's JSON, or its error `(kind, message)`.
async fn call(mcp: &McpServer, tool: &str, args: Value) -> Result<Value, (String, String)> {
    // Boxed: the tool futures are large in a debug build.
    let out = Box::pin(mcp.call(tool, args)).await;
    cua_spaces::extension::outcome_json(out).map_err(|e| (e.tag().to_string(), e.to_string()))
}

async fn call_as(
    mcp: &McpServer,
    caller: Caller,
    tool: &str,
    args: Value,
) -> Result<Value, (String, String)> {
    caller.scope(call(mcp, tool, args)).await
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

fn agents_json(fx: &Fx) -> std::path::PathBuf {
    fx.home.path().join("persistent").join("agents.json")
}

/// Edits the stored record of `name` (as a daemon would have left it).
fn edit_record(fx: &Fx, name: &str, f: impl FnOnce(&mut Value)) {
    let path = agents_json(fx);
    let mut all: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let rec = all.iter_mut().find(|r| r["name"] == name).unwrap();
    f(rec);
    std::fs::write(&path, serde_json::to_vec_pretty(&all).unwrap()).unwrap();
}

/// A run's meta and state, as the runner writes them.
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

/// Appends runner events `(type, turn, message)`.
fn events(fx: &Fx, evs: &[(&str, u32, Option<&str>)]) {
    let path = format!("{RUNS}/{RUN}/events.jsonl");
    let mut log = fx
        .srv
        .state
        .file(&path)
        .map(|b| String::from_utf8(b).unwrap())
        .unwrap_or_default();
    let seq = log.lines().count();
    for (i, (kind, turn, msg)) in evs.iter().enumerate() {
        let mut e = json!({"seq": seq + i + 1, "ts": 0, "turn": turn, "type": kind});
        if let Some(m) = msg {
            e["message"] = json!(m);
        }
        if *kind == "message" {
            // An agent message chunk, as the runner records ACP updates.
            e = json!({"seq": seq + i + 1, "ts": 0, "turn": turn, "type": "update",
                "update": {"sessionUpdate": "agent_message_chunk",
                           "content": {"type": "text", "text": msg}}});
        }
        if *kind == "turn_ended" {
            e["stopReason"] = json!("end_turn");
        }
        log.push_str(&e.to_string());
        log.push('\n');
    }
    fx.srv.state.put_file(&path, log.into_bytes());
}

/// The run's process, alive until dropped by the mock.
async fn alive(fx: &Fx) -> cua_spacesd_client::ProcessHandle {
    let s = fx.spaces.space(&fx.space).await.unwrap();
    s.spacesd()
        .unwrap()
        .spawn(
            Command::new("sleep")
                .arg("600000")
                .tag(format!("cua-agent/{RUN}")),
        )
        .await
        .unwrap()
}

/// Prompts the runner was handed (its inbox), in order.
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
        .map(|(_, b)| {
            serde_json::from_slice::<Value>(&b).unwrap()["text"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// A member `ada` of the fixture's Space with an idle run at turn 1.
async fn member_with_idle_run(fx: &Fx) -> (Persistent, cua_spacesd_client::ProcessHandle) {
    let p = fx.spaces.persistent();
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code"}),
    )
    .await
    .unwrap();
    edit_record(fx, "ada", |r| r["run_id"] = json!(RUN));
    run_state(fx, json!({"status": "idle", "turn": 1}));
    events(
        fx,
        &[
            ("turn_started", 1, None),
            ("message", 1, Some("hello")),
            ("turn_ended", 1, None),
        ],
    );
    let proc = alive(fx).await;
    // The supervisor has seen turn 1 already.
    edit_record(fx, "ada", |r| {
        r["saved_turn"] = json!(1);
        r["cursor"] = json!(3);
    });
    (p, proc)
}

fn entries(v: &Value) -> Vec<Value> {
    v["entries"].as_array().unwrap().clone()
}

// ------------------------------------------------------------ compatibility

/// `agents.json` exactly as the daemon before members wrote it.
const MAIN_AGENTS_JSON: &str = r#"[
  {
    "name": "scout",
    "harness": "claude-code",
    "space": "direct:127.0.0.1:1",
    "model": "claude-sonnet",
    "env_from_host": ["ANTHROPIC_API_KEY"],
    "created_ms": 1761000000000,
    "paused": false,
    "space_state": "running",
    "run_id": "run-0000abcd",
    "cursor": 12,
    "saved_turn": 3,
    "notify": true,
    "saved_ms": 1761000100000,
    "a_field_from_a_newer_daemon": {"kept": true}
  }
]"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agents_json_from_before_members_loads_and_keeps_unknown_fields() {
    let fx = setup().await;
    std::fs::create_dir_all(agents_json(&fx).parent().unwrap()).unwrap();
    std::fs::write(agents_json(&fx), MAIN_AGENTS_JSON).unwrap();
    let p = fx.spaces.persistent();
    let rec = p.get("scout").unwrap();
    assert_eq!(rec.sessions, 1);
    assert!(!rec.policy.durable && !rec.policy.once && rec.policy.guard);
    assert_eq!(rec.policy.on, "local");
    assert_eq!(rec.added_by, None);
    assert_eq!(rec.run_id.as_deref(), Some("run-0000abcd"));
    assert_eq!(rec.cursor, 12);
    // A save of the file (another agent added) keeps every field.
    p.create(AgentSpec {
        name: "other".into(),
        harness: "claude-code".into(),
        space: fx.space.clone(),
        ..Default::default()
    })
    .unwrap();
    let saved: Vec<Value> =
        serde_json::from_slice(&std::fs::read(agents_json(&fx)).unwrap()).unwrap();
    let scout = saved.iter().find(|r| r["name"] == "scout").unwrap();
    assert_eq!(scout["a_field_from_a_newer_daemon"]["kept"], true);
    assert_eq!(scout["saved_turn"], 3);
    // The old list row keeps exactly its fields.
    let list = call(&fx.mcp, "persistent_agent_list", json!({}))
        .await
        .unwrap();
    let row = list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "scout")
        .unwrap()
        .clone();
    assert_eq!(
        keys(&row),
        [
            "base_url",
            "created_ms",
            "env_from_host",
            "harness",
            "last_error",
            "model",
            "name",
            "paused",
            "run_id",
            "saved_ms",
            "space",
            "space_state"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_tools_keep_their_result_fields_and_appear_in_the_thread() {
    let fx = setup().await;
    let (_p, _proc) = member_with_idle_run(&fx).await;
    // persistent_agent_send: same fields as before members.
    let d = call(
        &fx.mcp,
        "persistent_agent_send",
        json!({"name": "ada", "text": "check the logs"}),
    )
    .await
    .unwrap();
    assert_eq!(keys(&d), ["run_id", "started"]);
    assert_eq!(d["run_id"], RUN);
    assert_eq!(inbox(&fx), ["check the logs"]);
    // ... and it is in the thread, authored by the caller, as a task.
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let e = entries(&t);
    assert_eq!(e[0]["text"], "check the logs");
    assert_eq!(
        e[0]["author"],
        json!({"kind": "person", "id": "owner", "name": "Owner"})
    );
    assert_eq!(e[0]["task"]["to"], "ada");
    // routine_add (agent_schedule_add) / routine_list rows are unchanged.
    let r = call(
        &fx.mcp,
        "routine_add",
        json!({"agent": "ada", "title": "Digest", "prompt": "summarize", "daily_at": "08:00"}),
    )
    .await
    .unwrap();
    let listed = call(&fx.mcp, "routine_list", json!({"agent": "ada"}))
        .await
        .unwrap();
    assert_eq!(listed["routines"][0]["id"], r["id"]);
    assert_eq!(keys(&listed["routines"][0]), keys(&r));
    let gone = call(&fx.mcp, "agent_schedule_remove", json!({"id": r["id"]}))
        .await
        .unwrap();
    assert_eq!(gone["removed"], r["id"]);
    let e = call(&fx.mcp, "routine_remove", json!({"id": r["id"]}))
        .await
        .unwrap_err();
    assert_eq!(e.0, "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_run_tools_take_a_member_name() {
    let fx = setup().await;
    let (_p, _proc) = member_with_idle_run(&fx).await;
    let st = call(
        &fx.mcp,
        "agent_status",
        json!({"space": fx.space, "name": "ada"}),
    )
    .await
    .unwrap();
    assert_eq!(st["run_id"], RUN);
    let ev = call(
        &fx.mcp,
        "agent_events",
        json!({"space": fx.space, "name": "ada"}),
    )
    .await
    .unwrap();
    assert_eq!(ev["run_id"], RUN);
    // agent_message by name: the old result fields, delivered, and recorded.
    let m = call(
        &fx.mcp,
        "agent_message",
        json!({"space": fx.space, "name": "ada", "text": "and the tests"}),
    )
    .await
    .unwrap();
    assert_eq!(
        keys(&m),
        ["delivered", "phase", "reason", "run_id", "status"]
    );
    assert_eq!(inbox(&fx), ["and the tests"]);
    // Both, or neither, is refused.
    for args in [
        json!({"space": fx.space, "name": "ada", "run_id": RUN}),
        json!({"space": fx.space}),
    ] {
        let e = call(&fx.mcp, "agent_status", args).await.unwrap_err();
        assert_eq!(e.0, "invalid_argument");
    }
    // A run id still works as before.
    let st = call(
        &fx.mcp,
        "agent_status",
        json!({"space": fx.space, "run_id": RUN}),
    )
    .await
    .unwrap();
    assert_eq!(st["run_id"], RUN);
}

// ------------------------------------------------------------ the thread

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authors_come_from_the_caller_never_the_arguments() {
    let fx = setup().await;
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code"}),
    )
    .await
    .unwrap();
    let spoof = json!({"kind": "agent", "id": "agent:ada", "name": "ada"});
    let e = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "hi", "author": spoof}),
    )
    .await
    .unwrap();
    assert_eq!(e["entry"]["author"]["id"], "owner");
    // Through the relay: the forwarded account.
    let carol = Caller::Relay {
        account: "acct_carol".into(),
        name: Some("Carol".into()),
        role: "shared".into(),
    };
    let e = call_as(
        &fx.mcp,
        carol,
        "thread_post",
        json!({"space": fx.space, "text": "hello", "author": spoof}),
    )
    .await
    .unwrap();
    assert_eq!(
        e["entry"]["author"],
        json!({"kind": "person", "id": "acct_carol", "name": "Carol"})
    );
    // A viewer may not post.
    let viewer = Caller::Relay {
        account: "acct_eve".into(),
        name: None,
        role: "viewer".into(),
    };
    let err = call_as(
        &fx.mcp,
        viewer,
        "thread_post",
        json!({"space": fx.space, "text": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, "permission_denied");
    // An agent through its bridge: always itself, in its own Space.
    let p = fx.spaces.persistent();
    let rec = p.get("ada").unwrap();
    let server = bridge::server(&fx.spaces, p.clone(), &rec, RUN);
    let reply = server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "thread_post", "arguments": {"text": "from ada", "author": {"id": "owner"}, "space": "local:elsewhere"}}}))
        .await
        .unwrap();
    let posted: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(posted["entry"]["author"]["id"], "agent:ada");
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(entries(&t).len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_and_oversized_posts_are_refused() {
    let fx = setup().await;
    let e = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": format!("the key is {SECRET}")}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
    assert!(
        e.1.contains("looks like a secret; use the Keyvault"),
        "{}",
        e.1
    );
    assert!(!e.1.contains(SECRET));
    let e = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "x".repeat(32 * 1024 + 1)}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
    let many: Vec<String> = (0..17).map(|i| format!("spaces/x/{i}.txt")).collect();
    let e = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "files", "attachments": many}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
    // Nothing was written.
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert!(entries(&t).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_cursors_unread_and_follow() {
    let fx = setup().await;
    for t in ["one", "two"] {
        call(
            &fx.mcp,
            "thread_post",
            json!({"space": fx.space, "text": t}),
        )
        .await
        .unwrap();
    }
    let carol = || Caller::Relay {
        account: "acct_carol".into(),
        name: Some("Carol".into()),
        role: "shared".into(),
    };
    let r = call_as(
        &fx.mcp,
        carol(),
        "thread_read",
        json!({"space": fx.space, "after": 0, "limit": 1}),
    )
    .await
    .unwrap();
    assert_eq!(r["unread"], 2);
    assert_eq!(r["read_seq"], 0);
    assert_eq!(r["cursor"], 1);
    assert_eq!(r["caught_up"], false);
    assert_eq!(
        r["me"],
        json!({"kind": "person", "id": "acct_carol", "name": "Carol"})
    );
    let r = call_as(
        &fx.mcp,
        carol(),
        "thread_read",
        json!({"space": fx.space, "after": 1, "mark_read": true}),
    )
    .await
    .unwrap();
    assert_eq!(r["cursor"], 2);
    assert_eq!(r["caught_up"], true);
    assert_eq!(r["unread"], 0);
    assert_eq!(r["read_seq"], 2);
    // The owner's own posts are never unread for the owner.
    let r = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(r["unread"], 0);
    // A follow returns as soon as someone posts.
    let mcp = fx.mcp.clone();
    let space = fx.space.clone();
    let t0 = std::time::Instant::now();
    let follow = tokio::spawn(async move {
        call(
            &mcp,
            "thread_read",
            json!({"space": space, "after": 2, "follow_ms": 20000}),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "three"}),
    )
    .await
    .unwrap();
    let r = tokio::time::timeout(Duration::from_secs(10), follow)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(entries(&r)[0]["text"], "three");
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_cannot_write_its_spaces_thread_through_the_volume() {
    let fx = setup().await;
    let drive = fx
        .spaces
        .extension::<cua_spaces_ext::DriveExtension>()
        .unwrap()
        .drive()
        .clone();
    let folder = cua_volume::path::space_folder(&fx.space);
    for ctx in [
        cua_volume::Context::agent("ada", Some(&fx.space)),
        cua_volume::Context::space(&fx.space),
    ] {
        let e = drive
            .session(ctx)
            .write(
                &format!("{folder}thread/entries/000000000001.json"),
                b"{}".to_vec(),
                cua_volume::Condition::None,
            )
            .await
            .unwrap_err();
        assert_eq!(e.tag(), "forbidden");
        assert!(e.to_string().contains("(read-only)"), "{e}");
    }
    // The rest of its folder is still its own.
    drive
        .session(cua_volume::Context::agent("ada", Some(&fx.space)))
        .write(
            &format!("{folder}notes.md"),
            b"ok".to_vec(),
            cua_volume::Condition::None,
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imports_are_owner_only_idempotent_and_scanned() {
    let fx = setup().await;
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code"}),
    )
    .await
    .unwrap();
    let batch = json!({"space": fx.space, "source": "cua-bots", "entries": [
        {"import_id": "m1", "ts_ms": 1000, "author_kind": "person", "text": "hi bot"},
        {"import_id": "m2", "ts_ms": 2000, "author_kind": "agent", "agent_name": "ada", "text": "hi"},
    ]});
    let r = call(&fx.mcp, "thread_import", batch.clone()).await.unwrap();
    assert_eq!(r["imported"], 2);
    assert_eq!(r["note"]["text"], "imported 2 messages from cua-bots");
    let again = call(&fx.mcp, "thread_import", batch.clone()).await.unwrap();
    assert_eq!(again["imported"], 0);
    assert_eq!(again["skipped"], 2);
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let e = entries(&t);
    assert_eq!(e.len(), 3);
    assert_eq!(e[0]["ts_ms"], 1000);
    assert_eq!(e[0]["imported"], true);
    assert_eq!(e[1]["author"]["id"], "agent:ada");
    // Relay and agent callers may not import.
    for caller in [
        Caller::Relay {
            account: "acct_owner".into(),
            name: None,
            role: "owner".into(),
        },
        Caller::Agent {
            name: "ada".into(),
            space: fx.space.clone(),
        },
    ] {
        let err = call_as(&fx.mcp, caller, "thread_import", batch.clone())
            .await
            .unwrap_err();
        assert_eq!(err.0, "permission_denied");
    }
    // A secret, or an agent that is not a member, refuses the whole batch.
    for entry in [
        json!({"import_id": "s", "ts_ms": 1, "author_kind": "person", "text": SECRET}),
        json!({"import_id": "n", "ts_ms": 1, "author_kind": "agent", "agent_name": "bob", "text": "x"}),
    ] {
        let err = call(
            &fx.mcp,
            "thread_import",
            json!({"space": fx.space, "source": "cua-bots", "entries": [entry]}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, "invalid_argument");
    }
}

// ------------------------------------------------------------ wake on message

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mention_wakes_the_member_and_its_answer_is_the_reply() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    let posted = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada run the e2e suite"}),
    )
    .await
    .unwrap();
    let task = posted["entry"]["seq"].as_u64().unwrap();
    assert_eq!(posted["entry"]["task"]["to"], "ada");
    assert_eq!(posted["entry"]["mentions"], json!(["ada"]));
    // An @name that is not a member is only text.
    let plain = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "cc @bob"}),
    )
    .await
    .unwrap();
    assert_eq!(plain["entry"]["task"], Value::Null);

    let inbox_tasks = call(
        &fx.mcp,
        "thread_inbox",
        json!({"space": fx.space, "member": "ada"}),
    )
    .await
    .unwrap();
    assert_eq!(inbox_tasks["tasks"][0]["state"], "queued");

    // The supervisor delivers it as the next turn.
    Box::pin(p.tick()).await;
    let delivered = inbox(&fx);
    assert_eq!(delivered.len(), 1);
    assert!(
        delivered[0].starts_with(&format!(
            "[thread #{task} from Owner] @ada run the e2e suite"
        )),
        "{}",
        delivered[0]
    );
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(t["tasks"][0]["state"], "working");
    assert_eq!(p.get("ada").unwrap().task.unwrap().seq, task);
    // Mid-turn nothing else is delivered, and a second task waits.
    run_state(&fx, json!({"status": "running", "turn": 2}));
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada and then lint"}),
    )
    .await
    .unwrap();
    Box::pin(p.tick()).await;
    assert_eq!(inbox(&fx).len(), 1);

    // The turn ends with an answer: it becomes the reply, the task is done
    // and the notification points at the reply.
    events(
        &fx,
        &[
            ("turn_started", 2, None),
            ("message", 2, Some("2 failures: a, b")),
            ("turn_ended", 2, None),
        ],
    );
    run_state(&fx, json!({"status": "idle", "turn": 2}));
    // (Saving the home afterwards needs a real guest; the mock refuses it,
    // which must not hold up the thread.)
    Box::pin(p.tick()).await;
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let reply = entries(&t)
        .into_iter()
        .find(|e| e["reply_to"] == task && e["kind"] == "message")
        .expect("a reply");
    assert_eq!(reply["text"], "2 failures: a, b");
    assert_eq!(reply["author"]["id"], "agent:ada");
    let state = |t: &Value, seq: u64| {
        t["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["seq"] == seq)
            .map(|x| x["state"].clone())
    };
    assert_eq!(state(&t, task), None, "the task is closed");
    // The same pass delivered the waiting task.
    assert_eq!(inbox(&fx).len(), 2);
    assert!(inbox(&fx)[1].contains("and then lint"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_that_replies_itself_is_not_answered_twice() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    let posted = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada summarize"}),
    )
    .await
    .unwrap();
    let task = posted["entry"]["seq"].as_u64().unwrap();
    Box::pin(p.tick()).await;
    // The agent replies through its bridge mid-turn.
    let rec = p.get("ada").unwrap();
    let server = bridge::server(&fx.spaces, p.clone(), &rec, RUN);
    let out = server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "thread_reply", "arguments": {"task": task, "text": "summary: fine"}}}))
        .await
        .unwrap();
    assert_eq!(out["result"]["isError"], false, "{out}");
    events(
        &fx,
        &[("message", 2, Some("ignored")), ("turn_ended", 2, None)],
    );
    run_state(&fx, json!({"status": "idle", "turn": 2}));
    Box::pin(p.tick()).await;
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let replies: Vec<Value> = entries(&t)
        .into_iter()
        .filter(|e| e["reply_to"] == task && e["kind"] == "message")
        .collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["text"], "summary: fine");
    assert!(p.get("ada").unwrap().task.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paused_member_keeps_its_task_queued_and_says_so() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    edit_record(&fx, "ada", |r| r["paused"] = json!(true));
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada please"}),
    )
    .await
    .unwrap();
    Box::pin(p.tick()).await;
    assert!(inbox(&fx).is_empty());
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let e = entries(&t);
    assert_eq!(e[1]["kind"], "system");
    assert!(e[1]["text"].as_str().unwrap().contains("ada is paused"));
    assert_eq!(t["tasks"][0]["state"], "queued");
    let members = call(&fx.mcp, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(members["members"][0]["status"], json!({"state": "paused"}));
    assert_eq!(members["members"][0]["open_tasks"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_schedule_posts_a_system_task_and_skips_a_busy_member() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    let r = call(
        &fx.mcp,
        "agent_schedule_add",
        json!({"agent": "ada", "title": "Digest", "prompt": "summarize the day", "every_minutes": 60}),
    )
    .await
    .unwrap();
    let routine = p
        .routines(Some("ada"))
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == r["id"].as_str().unwrap())
        .unwrap();
    let fired = p.fire_schedule(&routine).await.unwrap();
    assert!(fired.is_ok(), "{fired:?}");
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let e = entries(&t);
    assert_eq!(
        e[0]["author"]["id"],
        format!("system:schedule:{}", routine.id)
    );
    assert_eq!(e[0]["task"]["to"], "ada");
    assert_eq!(inbox(&fx).len(), 1);
    // Busy: the slot is skipped and no task is left queued.
    run_state(&fx, json!({"status": "running", "turn": 2}));
    edit_record(&fx, "ada", |r| r["task"] = Value::Null);
    let skipped = p.fire_schedule(&routine).await.unwrap();
    assert!(skipped.unwrap_err().contains("middle of a turn"));
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert!(
        t["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["state"] != "queued"),
        "{t}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_add_refuses_what_is_not_available_and_remove_closes_tasks() {
    let fx = setup().await;
    for (extra, why) in [
        (
            json!({"on": "auto"}),
            format!(
                "invalid argument: on: auto needs a durable Space, and {} is not one; name a new Space to create it as a durable Space",
                fx.space
            ),
        ),
        (
            json!({"sessions": 0}),
            "invalid argument: sessions: 1 to 8".into(),
        ),
        (
            json!({"sessions": 9}),
            "invalid argument: sessions: 1 to 8".into(),
        ),
    ] {
        let mut args = json!({"space": fx.space, "name": "ada", "harness": "claude-code"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let e = call(&fx.mcp, "agent_add", args).await.unwrap_err();
        assert_eq!(e, ("invalid_argument".to_string(), why), "{extra}");
    }
    let added = call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code",
               "daily_at": "08:00", "prompt": "plan the day"}),
    )
    .await
    .unwrap();
    assert_eq!(added["added_by"]["id"], "owner");
    assert_eq!(added["sessions"], 1);
    assert_eq!(added["schedule"]["botID"], "ada");
    // Managing members is the owner's.
    let e = call_as(
        &fx.mcp,
        Caller::Relay {
            account: "acct_carol".into(),
            name: None,
            role: "shared".into(),
        },
        "agent_add",
        json!({"space": fx.space, "name": "bob", "harness": "claude-code"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "permission_denied");
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada hi"}),
    )
    .await
    .unwrap();
    let removed = call(&fx.mcp, "agent_remove", json!({"name": "ada"}))
        .await
        .unwrap();
    assert_eq!(removed["home_deleted"], false);
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert!(t["tasks"].as_array().unwrap().is_empty());
    assert!(
        fx.spaces
            .persistent()
            .routines(Some("ada"))
            .await
            .unwrap()
            .is_empty()
    );
}

// ------------------------------------------------------------ the mirror

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mirror_is_create_only_and_rebuilds_a_lost_thread() {
    let fx = setup().await;
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code"}),
    )
    .await
    .unwrap();
    let drive = fx
        .spaces
        .extension::<cua_spaces_ext::DriveExtension>()
        .unwrap()
        .drive()
        .clone();
    let user = drive.session(cua_volume::Context::user());
    let folder = cua_volume::path::space_folder(&fx.space);
    // An object already at entry 1's key is never overwritten.
    let first = format!("{folder}thread/entries/000000000001.json");
    for t in ["one", "two", "three"] {
        call(
            &fx.mcp,
            "thread_post",
            json!({"space": fx.space, "text": t}),
        )
        .await
        .unwrap();
    }
    let original = user.read(&first, None).await.err();
    assert!(original.is_some(), "nothing shipped before the tick");
    user.write(&first, b"{\"pre\":1}".to_vec(), cua_volume::Condition::None)
        .await
        .unwrap();
    let p = fx.spaces.persistent();
    Box::pin(p.tick()).await;
    let (b, _) = user.read(&first, None).await.unwrap();
    assert_eq!(b, b"{\"pre\":1}");
    for seq in [2, 3] {
        let (b, _) = user
            .read(&format!("{folder}thread/entries/{seq:012}.json"), None)
            .await
            .unwrap();
        let e: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(e["seq"], seq);
    }
    let (m, _) = user
        .read(&format!("{folder}thread/members.json"), None)
        .await
        .unwrap();
    let m: Value = serde_json::from_slice(&m).unwrap();
    assert_eq!(m["members"][0]["name"], "ada");
    assert!(m.to_string().find("env").is_none());
    let store = p.thread_store(&fx.space);
    assert_eq!(store.head().unwrap().mirrored_seq, 3);

    // The owner loses its thread; a new daemon on the same home rebuilds it
    // from the Volume (entry 1 is the foreign object, so it is skipped).
    std::fs::remove_dir_all(fx.home.path().join("threads")).unwrap();
    let fresh = runtime(fx.home.path());
    let mcp = McpServer::new(fresh.clone());
    fresh
        .add(
            &format!("http://{}", fx.srv.addr),
            None,
            Some("room".into()),
        )
        .await
        .ok();
    let t = call(&mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let texts: Vec<Value> = entries(&t).iter().map(|e| e["text"].clone()).collect();
    assert_eq!(texts, [json!("two"), json!("three")]);
    // New posts continue after the rebuilt ones.
    let e = call(
        &mcp,
        "thread_post",
        json!({"space": fx.space, "text": "four"}),
    )
    .await
    .unwrap();
    assert_eq!(e["entry"]["seq"], 4);
}

#[test]
fn the_daemon_reports_the_thread_feature() {
    use cua_daemon::extension::DaemonExtension as _;
    let f = cua_spaces_ext::daemon::CuaSpacesDaemon::default().features();
    assert_eq!(f, vec![cua_spaces_ext::thread::FEATURE.to_string()]);
    assert_eq!(cua_spaces_ext::thread::FEATURE, "thread.v1");
}

/// `meta.json` of the idle run, as a runner that stored the hash of the
/// credentials it started with writes it.
fn run_meta_with_hash(fx: &Fx, hash: &str) {
    let meta = json!({"run_id": RUN, "harness": "claude-code", "prompt": "hi",
                      "cwd": "/home/cua/work", "created_at": 0.0, "cred_hash": hash});
    fx.srv.state.put_file(
        &format!("{RUNS}/{RUN}/meta.json"),
        meta.to_string().into_bytes(),
    );
}

async fn run_alive(fx: &Fx) -> bool {
    let s = fx.spaces.space(&fx.space).await.unwrap();
    s.spacesd()
        .unwrap()
        .list_processes(false)
        .await
        .unwrap()
        .iter()
        .any(|p| p.tag == format!("cua-agent/{RUN}"))
}

fn forwarded_hash(key: &str) -> String {
    cua_agents::runs::credential_hash(
        "claude-code",
        &std::collections::BTreeMap::from([("ANTHROPIC_API_KEY".to_string(), key.to_string())]),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_reaches_the_idle_run_while_its_credentials_are_the_ones_forwarded() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    edit_record(&fx, "ada", |r| {
        r["env"] = json!({"ANTHROPIC_API_KEY": "sk-one"})
    });
    run_meta_with_hash(&fx, &forwarded_hash("sk-one"));
    let d = p.send("ada", "again").await.unwrap();
    assert!(!d.started, "the idle run takes it");
    assert_eq!(inbox(&fx), vec!["again".to_string()]);
    assert!(run_alive(&fx).await);
}

/// Reconnecting in Settings must take effect on the next message: a run that
/// began with other credentials is ended, not reused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_credential_ends_the_idle_run_instead_of_reusing_it() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    edit_record(&fx, "ada", |r| {
        r["env"] = json!({"ANTHROPIC_API_KEY": "sk-new"})
    });
    run_meta_with_hash(&fx, &forwarded_hash("sk-old"));
    // The mock runs no shell, so the fresh start itself can't finish here;
    // what matters is that the old run did not get the message and is gone.
    let _ = p.send("ada", "again").await;
    // (Stopping a run leaves a control message, not a prompt.)
    assert!(
        inbox(&fx).iter().all(|t| t.is_empty()),
        "the old run was not reused: {:?}",
        inbox(&fx)
    );
    assert!(!run_alive(&fx).await, "the old run was ended");
}

/// A run whose last turn failed to authenticate is not reused either, even
/// with the same credentials (the provider rejected them).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_that_failed_to_authenticate_is_not_reused() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    edit_record(&fx, "ada", |r| {
        r["env"] = json!({"ANTHROPIC_API_KEY": "sk-one"})
    });
    run_meta_with_hash(&fx, &forwarded_hash("sk-one"));
    events(
        &fx,
        &[
            ("turn_started", 2, None),
            (
                "error",
                2,
                Some("Failed to authenticate. API Error: 401 OAuth access token is invalid"),
            ),
            ("turn_ended", 2, None),
        ],
    );
    run_state(
        &fx,
        json!({"status": "idle", "turn": 2, "stopReason": "error"}),
    );
    run_meta_with_hash(&fx, &forwarded_hash("sk-one"));
    let _ = p.send("ada", "again").await;
    assert!(inbox(&fx).iter().all(|t| t.is_empty()), "{:?}", inbox(&fx));
    assert!(!run_alive(&fx).await);
}

/// A second Space of the same machine (its own mock guest).
async fn second_space(fx: &Fx) -> (MockServer, String) {
    let srv = MockServer::start(MockAuth::default()).await;
    let info = fx
        .spaces
        .add(
            &format!("http://{}", srv.addr),
            None,
            Some("room-two".into()),
        )
        .await
        .unwrap();
    (srv, info.id)
}

async fn add_claude(fx: &Fx, space: &str) -> Result<Value, (String, String)> {
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": space, "name": "claude", "harness": "claude-code"}),
    )
    .await
}

async fn member_names(fx: &Fx, space: &str) -> Vec<(String, String)> {
    let m = call(&fx.mcp, "agent_members", json!({"space": space}))
        .await
        .unwrap();
    m["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_string(),
                r["handle"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Names were keys on the machine: the first Space with a `claude` owned the
/// name and `@claude` in any other Space failed with "exists with another
/// harness or Space". Each Space's `@claude` is its own now.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_spaces_each_get_their_own_claude() {
    let fx = setup().await;
    let (_srv2, two) = second_space(&fx).await;
    add_claude(&fx, &fx.space).await.unwrap();
    add_claude(&fx, &two).await.unwrap();
    // The first keeps the plain name; the second is stored apart and still answers to `claude`.
    assert_eq!(
        member_names(&fx, &fx.space).await,
        [("claude".to_string(), "claude".to_string())]
    );
    assert_eq!(
        member_names(&fx, &two).await,
        [("claude-2".to_string(), "claude".to_string())]
    );
    // Adding again in either Space is the same member, not a third.
    add_claude(&fx, &two).await.unwrap();
    assert_eq!(member_names(&fx, &two).await.len(), 1);
    // `@claude` in the second Space is a task for the second Space's member.
    let posted = call(
        &fx.mcp,
        "thread_post",
        json!({"space": two, "text": "@claude hello"}),
    )
    .await
    .unwrap();
    assert_eq!(posted["entry"]["task"]["to"], "claude-2");
    assert_eq!(posted["entry"]["mentions"], json!(["claude-2"]));
    let first = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@claude hello"}),
    )
    .await
    .unwrap();
    assert_eq!(first["entry"]["task"]["to"], "claude");
}

/// Deleting a Space frees its members' names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_spaces_name_is_free_for_a_new_space() {
    let fx = setup().await;
    let (_srv2, two) = second_space(&fx).await;
    add_claude(&fx, &fx.space).await.unwrap();
    fx.spaces.remove(&fx.space).await.unwrap();
    // The first Space is gone: `claude` is the second Space's, under its own plain name.
    add_claude(&fx, &two).await.unwrap();
    assert_eq!(
        member_names(&fx, &two).await,
        [("claude".to_string(), "claude".to_string())]
    );
}

/// A record from before Spaces had their own names (name only) keeps working.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_name_only_record_still_answers_in_its_space() {
    let fx = setup().await;
    std::fs::create_dir_all(agents_json(&fx).parent().unwrap()).unwrap();
    let old = json!([{
        "name": "claude", "harness": "claude-code", "space": fx.space,
        "created_ms": 1761000000000u64, "paused": false, "space_state": "running",
        "cursor": 0, "saved_turn": 0, "notify": true, "saved_ms": 0
    }]);
    std::fs::write(agents_json(&fx), old.to_string()).unwrap();
    assert_eq!(
        member_names(&fx, &fx.space).await,
        [("claude".to_string(), "claude".to_string())]
    );
    // Adding it again in its own Space is that record, not a new one.
    add_claude(&fx, &fx.space).await.unwrap();
    assert_eq!(member_names(&fx, &fx.space).await.len(), 1);
    let posted = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@claude hi"}),
    )
    .await
    .unwrap();
    assert_eq!(posted["entry"]["task"]["to"], "claude");
    // And a second Space can have its own.
    let (_srv2, two) = second_space(&fx).await;
    add_claude(&fx, &two).await.unwrap();
    assert_eq!(
        member_names(&fx, &two).await,
        [("claude-2".to_string(), "claude".to_string())]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_turn_fails_its_task_and_the_notification_points_at_it() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    let posted = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada deploy"}),
    )
    .await
    .unwrap();
    let task = posted["entry"]["seq"].as_u64().unwrap();
    Box::pin(p.tick()).await;
    events(
        &fx,
        &[
            ("turn_started", 2, None),
            ("error", 2, Some("Authentication required")),
            ("turn_ended", 2, None),
        ],
    );
    run_state(&fx, json!({"status": "idle", "turn": 2}));
    Box::pin(p.tick()).await;
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let e = entries(&t);
    let reply = e
        .iter()
        .find(|x| x["reply_to"] == task && x["kind"] == "message")
        .expect("a reply");
    // Plain words from the agent; the complete error rides along typed, and
    // the harness's own words are never the summary.
    assert_eq!(
        reply["text"],
        "I can't reach Claude yet. Connect Claude, then try again."
    );
    assert_eq!(reply["failure"]["class"], "needs_connect");
    assert_eq!(reply["failure"]["provider"], "anthropic");
    assert_eq!(reply["failure"]["message"], "Authentication required");
    assert_eq!(reply["failure"]["retryable"], true);
    let update = e.iter().rev().find(|x| x["kind"] == "task_update").unwrap();
    assert_eq!(update["update"]["state"], "failed");
    let n = p
        .feed()
        .list(false, None)
        .unwrap()
        .into_iter()
        .find(|n| n.kind == "error")
        .expect("an error notification");
    assert_eq!(n.entry_seq, reply["seq"].as_u64());
    assert!(p.get("ada").unwrap().task.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_without_after_is_the_newest_page_and_pages_back() {
    let fx = setup().await;
    for i in 1..=5 {
        call(
            &fx.mcp,
            "thread_post",
            json!({"space": fx.space, "text": format!("m{i}")}),
        )
        .await
        .unwrap();
    }
    let seqs = |v: &Value| -> Vec<u64> {
        entries(v)
            .iter()
            .map(|e| e["seq"].as_u64().unwrap())
            .collect()
    };
    let r = call(
        &fx.mcp,
        "thread_read",
        json!({"space": fx.space, "limit": 2}),
    )
    .await
    .unwrap();
    assert_eq!(seqs(&r), [4, 5]);
    assert_eq!(r["has_more_before"], true);
    assert_eq!(r["cursor"], 5);
    assert_eq!(r["caught_up"], true);
    let r = call(
        &fx.mcp,
        "thread_read",
        json!({"space": fx.space, "limit": 2, "before": 4}),
    )
    .await
    .unwrap();
    assert_eq!(seqs(&r), [2, 3]);
    assert_eq!(r["has_more_before"], true);
    let r = call(
        &fx.mcp,
        "thread_read",
        json!({"space": fx.space, "limit": 2, "before": 2}),
    )
    .await
    .unwrap();
    assert_eq!(seqs(&r), [1]);
    assert_eq!(r["has_more_before"], false);
    let e = call(
        &fx.mcp,
        "thread_read",
        json!({"space": fx.space, "after": 1, "before": 3}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_show_what_each_is_doing() {
    let fx = setup().await;
    let (p, _proc) = member_with_idle_run(&fx).await;
    let m = call(&fx.mcp, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(m["members"][0]["status"], json!({"state": "idle"}));
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada look at the flaky test"}),
    )
    .await
    .unwrap();
    Box::pin(p.tick()).await;
    run_state(&fx, json!({"status": "running", "turn": 2}));
    let m = call(&fx.mcp, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    let row = &m["members"][0];
    assert_eq!(row["status"]["state"], "working");
    assert_eq!(
        row["status"]["activity"],
        "task #1: @ada look at the flaky test"
    );
    assert_eq!(row["added_by"]["id"], "owner");
    assert_eq!(row["open_tasks"], 1);
}

/// A post keeps what the person was looking at (`view`), and the thread and
/// the agent's inbox give it back, so the agent knows which window "this"
/// is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_post_keeps_the_view_it_was_sent_from() {
    let fx = setup().await;
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code"}),
    )
    .await
    .unwrap();
    let view =
        json!({"kind": "window", "app": "Firefox", "title": "Pull requests", "window_id": "w42"});
    let posted = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada review this tab", "view": view}),
    )
    .await
    .unwrap();
    assert_eq!(posted["entry"]["view"], view);
    let seq = posted["entry"]["seq"].as_u64().unwrap();
    let read = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    let entry = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["seq"].as_u64() == Some(seq))
        .unwrap()
        .clone();
    assert_eq!(entry["view"], view);
    assert_eq!(read["tasks"][0]["view"], view);
    let inbox = call(
        &fx.mcp,
        "thread_inbox",
        json!({"space": fx.space, "member": "ada"}),
    )
    .await
    .unwrap();
    assert_eq!(inbox["tasks"][0]["view"]["app"], "Firefox");
    assert_eq!(inbox["tasks"][0]["view"]["window_id"], "w42");

    // Without a view, nothing is stored; a desktop view is just its kind.
    let plain = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "hello"}),
    )
    .await
    .unwrap();
    assert!(plain["entry"].get("view").is_none(), "{plain}");
    let desktop = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "look", "view": {"kind": "desktop", "app": "Finder"}}),
    )
    .await
    .unwrap();
    assert_eq!(desktop["entry"]["view"], json!({"kind": "desktop"}));

    // A view that is not the desktop or a window is refused, and nothing is posted.
    let before = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap()["entries"]
        .as_array()
        .unwrap()
        .len();
    let e = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "x", "view": {"kind": "tab"}}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "invalid_argument", "{e:?}");
    let after = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap()["entries"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(before, after);
}
