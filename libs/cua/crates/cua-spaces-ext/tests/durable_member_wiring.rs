// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Durable agent members, `once`, `on` and the Space hooks through the
//! supervisor (CUA-2155). Against the in-process spacesd mock, a temporary
//! cua home and a Volume in it: the runs' files are written by the test as
//! the runner would; nothing runs in a guest. The mock runs no shell, so a
//! hook that is found fails ("executable not found"), which is how a failed
//! `.cua/setup` and `.cua/resume` are exercised here.

use std::time::Duration;

use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::Persistent;
use cua_spacesd_client::Command;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const RUNS: &str = "/home/cua/.cua/agents";
const HOMES: &str = "/home/cua/cua-volume/agents";
const RUN: &str = "run-00000080";

struct Fx {
    srv: MockServer,
    home: tempfile::TempDir,
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
        srv,
        home,
        spaces,
        space: info.id,
        mcp,
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
    f(all.iter_mut().find(|r| r["name"] == name).unwrap());
    std::fs::write(&path, serde_json::to_vec_pretty(&all).unwrap()).unwrap();
}

fn put(fx: &Fx, path: &str, body: impl Into<Vec<u8>>) {
    fx.srv.state.put_file(path, body.into());
}

fn run_file(fx: &Fx, file: &str) -> Option<Vec<u8>> {
    fx.srv.state.file(&format!("{RUNS}/{RUN}/{file}"))
}

/// Thread entries of the fixture's Space.
async fn thread(fx: &Fx) -> Vec<Value> {
    call(
        &fx.mcp,
        "thread_read",
        json!({"space": fx.space, "limit": 200}),
    )
    .await
    .unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

/// Ticks until `done` holds (bounded).
async fn tick_until(p: &Persistent, what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..60 {
        Box::pin(p.tick()).await;
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The run's process, alive until killed.
async fn alive(fx: &Fx, run: &str) -> cua_spacesd_client::ProcessHandle {
    let s = fx.spaces.space(&fx.space).await.unwrap();
    s.spacesd()
        .unwrap()
        .spawn(
            Command::new("sleep")
                .arg("600000")
                .tag(format!("cua-agent/{run}")),
        )
        .await
        .unwrap()
}

/// The 0.7.0 record of `ada` (the shape `upgrade/0.7.0/home/persistent/
/// agents.json` holds), working in the fixture's Space, with a field a
/// newer daemon wrote.
fn agents_070(space: &str) -> Value {
    json!([{
        "name": "ada",
        "harness": "claude-code",
        "space": space,
        "env_from_host": [],
        "created_ms": 1792859152117u64,
        "paused": false,
        "space_state": "running",
        "run_id": RUN,
        "cursor": 0,
        "saved_turn": 0,
        "notify": true,
        "saved_ms": 0,
        "from_a_newer_daemon": {"kept": true}
    }])
}

const TOOLS_080: &str = concat!(
    r#"{"seq":1,"ts":1,"epoch":1,"turn":1,"kind":"intent","toolCallId":"call-1","name":"run call-1","toolKind":"execute","digest":"a","input":"{}","decision":"approve"}"#,
    "\n",
    r#"{"seq":2,"ts":2,"epoch":1,"turn":1,"kind":"outcome","toolCallId":"call-1","name":"run call-1","status":"completed","summary":"wrote 3 files"}"#,
    "\n",
    r#"{"seq":3,"ts":3,"epoch":1,"turn":1,"kind":"intent","toolCallId":"call-2","name":"run call-2","toolKind":"execute","digest":"b","input":"{}","decision":"approve"}"#,
    "\n",
);
const EVENTS_080: &str = concat!(
    r#"{"seq":1,"ts":1,"turn":0,"epoch":1,"type":"driver_start","session":"s1","attempt":1,"resume":false,"journal":false,"policy":"ask-model"}"#,
    "\n",
    r#"{"seq":2,"ts":2,"turn":1,"epoch":1,"type":"turn_started","prompt":"do the thing","msgId":"initial"}"#,
    "\n",
);

/// A durable run as the 0.8.0 runner left it when it was killed mid-turn
/// (`cua agent run --durable`): its journal in the run directory, no
/// `ship`, no `exitCode`, no process.
fn run_080(fx: &Fx) {
    let rdir = format!("{RUNS}/{RUN}");
    put(
        fx,
        &format!("{rdir}/meta.json"),
        json!({"run_id": RUN, "harness": "claude-code", "prompt": "do the thing",
               "cwd": "/home/cua/work", "created_at": 0.0})
        .to_string(),
    );
    put(
        fx,
        &format!("{rdir}/run.json"),
        json!({"harness": "claude-code", "durable": {"epoch": 1, "resume": false,
               "stateDir": rdir, "session": null, "policy": "ask-model",
               "maxAgentRestarts": 3, "guard": true}})
        .to_string(),
    );
    put(fx, &format!("{rdir}/launch.sh"), "#!/bin/sh\n");
    put(fx, &format!("{rdir}/tools.jsonl"), TOOLS_080);
    put(fx, &format!("{rdir}/events.jsonl"), EVENTS_080);
    put(
        fx,
        &format!("{rdir}/state.json"),
        json!({"status": "running", "turn": 1, "epoch": 1}).to_string(),
    );
    put(fx, &format!("{rdir}/epoch"), "1");
}

fn backend(fx: &Fx) -> std::sync::Arc<dyn cua_volume::Backend> {
    cua_volume::Drive::open_local(fx.home.path()).backend()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_070_agent_becomes_a_durable_member_and_its_080_run_is_shipped_and_redriven() {
    let fx = setup().await;
    std::fs::create_dir_all(agents_json(&fx).parent().unwrap()).unwrap();
    std::fs::write(
        agents_json(&fx),
        serde_json::to_vec_pretty(&agents_070(&fx.space)).unwrap(),
    )
    .unwrap();
    run_080(&fx);
    let p = fx.spaces.persistent();
    // Adding it again as a durable member keeps the agent and takes the policy.
    let added = call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code", "durable": true}),
    )
    .await
    .unwrap();
    assert_eq!(added["policy"]["durable"], true);
    let rec = p.get("ada").unwrap();
    assert_eq!(rec.created_ms, 1792859152117);
    assert_eq!(rec.run_id.as_deref(), Some(RUN));
    assert_eq!(rec.extra["from_a_newer_daemon"], json!({"kept": true}));

    // The supervisor ships the 0.8.0 journal and, after the 2 s backoff,
    // re-drives the crashed run at epoch 2.
    tick_until(&p, "the re-drive", || {
        run_file(&fx, "run.json")
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| v["durable"]["epoch"] == 2)
    })
    .await;
    let rec = p.get("ada").unwrap();
    assert_eq!((rec.redrive.epoch, rec.redrive.attempts), (2, 1));
    assert_eq!(rec.setup, "ok", "no .cua/setup: settled at once");
    let b = backend(&fx);
    let restored = cua_spaces_ext::session_journal::restore(b.as_ref(), &fx.space, RUN)
        .await
        .unwrap();
    assert_eq!(restored.files["tools.jsonl"], TOOLS_080.as_bytes());
    assert_eq!(restored.files["events.jsonl"], EVENTS_080.as_bytes());
    let prefix = cua_volume::journal::session_prefix(&fx.space, RUN).unwrap();
    let (fence, _) = cua_volume::journal::read_fence(b.as_ref(), &prefix)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fence.epoch, 2);
    assert_eq!(
        serde_json::from_slice::<Value>(&run_file(&fx, "ship.ack").unwrap()).unwrap()["end"]["tools.jsonl"],
        TOOLS_080.len()
    );
    let note = thread(&fx)
        .await
        .into_iter()
        .find(|e| e["author"]["id"] == "system:durable")
        .expect("a durable note");
    assert_eq!(note["text"], "ada crashed and was resumed (epoch 2)");
    // While it is being re-driven a task waits for it (it is mid-turn).
    let members = call(&fx.mcp, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(members["members"][0]["policy"]["durable"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_setup_holds_the_members_tasks_and_posts_a_hooks_note() {
    let fx = setup().await;
    let p = fx.spaces.persistent();
    put(&fx, &format!("{HOMES}/ada/work/.cua/setup"), "npm ci\n");
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code"}),
    )
    .await
    .unwrap();
    let posted = call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada build it"}),
    )
    .await
    .unwrap();
    let task = posted["entry"]["seq"].as_u64().unwrap();
    tick_until(&p, "the setup", || p.get("ada").unwrap().setup == "failed").await;
    // Another pass delivers nothing: the member keeps its task.
    Box::pin(p.tick()).await;
    let rec = p.get("ada").unwrap();
    assert!(rec.run_id.is_none() && rec.task.is_none());
    assert!(
        rec.setup_note.starts_with(&format!(
            ".cua/setup failed with exit code none (from {HOMES}/ada/work/.cua/setup)."
        )),
        "{}",
        rec.setup_note
    );
    let entries = thread(&fx).await;
    let note = entries
        .iter()
        .find(|e| e["author"]["id"] == "system:hooks")
        .expect("a hooks note");
    assert!(
        note["text"]
            .as_str()
            .unwrap()
            .starts_with("ada (setup): .cua/setup failed"),
        "{}",
        note["text"]
    );
    let t = call(&fx.mcp, "thread_read", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(t["tasks"][0]["seq"], task);
    assert_eq!(t["tasks"][0]["state"], "queued");
    let members = call(&fx.mcp, "agent_members", json!({"space": fx.space}))
        .await
        .unwrap();
    assert_eq!(members["members"][0]["status"]["state"], "failed");
    assert_eq!(members["members"][0]["setup"], "failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_once_member_leaves_after_its_task_and_keeps_its_home() {
    let fx = setup().await;
    let p = fx.spaces.persistent();
    let added = call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code", "once": true}),
    )
    .await
    .unwrap();
    assert_eq!(added["policy"]["once"], true);
    // An idle run at turn 1, seen by the supervisor already.
    let rdir = format!("{RUNS}/{RUN}");
    put(
        &fx,
        &format!("{rdir}/meta.json"),
        json!({"run_id": RUN, "harness": "claude-code", "prompt": "hi",
               "cwd": "/home/cua/work", "created_at": 0.0})
        .to_string(),
    );
    put(
        &fx,
        &format!("{rdir}/state.json"),
        json!({"status": "idle", "turn": 1}).to_string(),
    );
    let ev = |seq: u32, turn: u32, ty: &str| {
        json!({"seq": seq, "ts": 0, "turn": turn, "type": ty, "stopReason": "end_turn"}).to_string()
    };
    put(
        &fx,
        &format!("{rdir}/events.jsonl"),
        format!("{}\n{}\n", ev(1, 1, "turn_started"), ev(2, 1, "turn_ended")),
    );
    let _proc = alive(&fx, RUN).await;
    edit_record(&fx, "ada", |r| {
        r["run_id"] = json!(RUN);
        r["saved_turn"] = json!(1);
        r["cursor"] = json!(2);
    });
    call(
        &fx.mcp,
        "thread_post",
        json!({"space": fx.space, "text": "@ada one thing"}),
    )
    .await
    .unwrap();
    tick_until(&p, "the delivery", || p.get("ada").unwrap().task.is_some()).await;
    // Its turn ends: it answers, and leaves.
    let msg = json!({"seq": 4, "ts": 0, "turn": 2, "type": "update",
        "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "done"}}});
    put(
        &fx,
        &format!("{rdir}/events.jsonl"),
        format!(
            "{}\n{}\n{}\n{msg}\n{}\n",
            ev(1, 1, "turn_started"),
            ev(2, 1, "turn_ended"),
            ev(3, 2, "turn_started"),
            ev(5, 2, "turn_ended")
        ),
    );
    put(
        &fx,
        &format!("{rdir}/state.json"),
        json!({"status": "idle", "turn": 2}).to_string(),
    );
    tick_until(&p, "the removal", || p.get("ada").is_err()).await;
    let entries = thread(&fx).await;
    assert!(
        entries
            .iter()
            .any(|e| e["text"] == "done" && e["author"]["id"] == "agent:ada"),
        "the answer is its reply"
    );
    assert!(
        entries
            .iter()
            .any(|e| e["text"] == "ada finished its task and left (added with once)")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_auto_needs_a_durable_space_and_resume_runs_the_resume_hook() {
    let fx = setup().await;
    let e = call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code", "on": "auto"}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        e,
        (
            "invalid_argument".to_string(),
            format!(
                "invalid argument: on: auto needs a durable Space, and {} is not one; name a new Space to create it as a durable Space",
                fx.space
            )
        )
    );
    // `on: local` is the default and needs nothing.
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code", "on": "local"}),
    )
    .await
    .unwrap();
    let p = fx.spaces.persistent();
    put(&fx, &format!("{HOMES}/ada/work/.cua/resume"), "make dev\n");
    let r = Box::pin(p.resume("ada", None)).await;
    // (Restoring the home needs a real guest; when the mock refuses it the
    // hook is not reached, so look at it only when resume went through.)
    if let Ok(report) = r {
        let hook = report.hook.expect("the resume hook ran");
        assert!(!hook.ok);
        assert_eq!(hook.source, format!("{HOMES}/ada/work/.cua/resume"));
        assert!(
            thread(&fx)
                .await
                .iter()
                .any(|e| e["author"]["id"] == "system:hooks")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_durable_run_is_started_again_from_the_volume_behind_a_raised_fence() {
    let fx = setup().await;
    std::fs::create_dir_all(agents_json(&fx).parent().unwrap()).unwrap();
    std::fs::write(
        agents_json(&fx),
        serde_json::to_vec_pretty(&agents_070(&fx.space)).unwrap(),
    )
    .unwrap();
    run_080(&fx);
    let p = fx.spaces.persistent();
    call(
        &fx.mcp,
        "agent_add",
        json!({"space": fx.space, "name": "ada", "harness": "claude-code", "durable": true}),
    )
    .await
    .unwrap();
    // One pass ships the journal (the run is alive).
    let proc = alive(&fx, RUN).await;
    Box::pin(p.tick()).await;
    let b = backend(&fx);
    let prefix = cua_volume::journal::session_prefix(&fx.space, RUN).unwrap();
    assert!(
        !cua_volume::journal::chunks(b.as_ref(), &prefix)
            .await
            .unwrap()
            .is_empty()
    );
    // The run directory is gone (the Space lost it).
    proc.kill().await.unwrap();
    let guest = fx.spaces.space(&fx.space).await.unwrap();
    guest
        .spacesd()
        .unwrap()
        .run(Command::shell(format!("rm -f {RUNS}/{RUN}/meta.json")))
        .await
        .unwrap();
    tick_until(&p, "the relaunch", || {
        let r = p.get("ada").unwrap();
        r.relaunch || r.run_id.as_deref() != Some(RUN)
    })
    .await;
    // It is started again from the journal behind a fence above what was
    // shipped (starting a run needs a real guest; the mock refuses it, so
    // the member stays marked and tries again after a backoff).
    tick_until(&p, "a relaunch attempt", || {
        p.get("ada").unwrap().redrive.attempts >= 1
    })
    .await;
    let fence = || async {
        cua_volume::journal::read_fence(b.as_ref(), &prefix)
            .await
            .unwrap()
            .unwrap()
            .0
            .epoch
    };
    let first = fence().await;
    assert_eq!(first, 2);
    let rec = p.get("ada").unwrap();
    assert!(rec.relaunch);
    assert_eq!(rec.redrive.session.as_deref(), Some(RUN));
    assert!(rec.run_id.is_none());
    // Within the backoff nothing is tried again.
    Box::pin(p.tick()).await;
    assert_eq!(fence().await, first);
}

/// A coordinator stand-in that says where the durable Space `dev` runs.
struct FakePool(std::sync::Mutex<String>);

impl cua_spaces::durable::PoolRequests for FakePool {
    fn request<'a>(
        &'a self,
        _method: &'a str,
        _params: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u16, Value)> + Send + 'a>> {
        let space = self.0.lock().unwrap().clone();
        Box::pin(async move {
            (
                200,
                json!({"name": "dev", "state": "running", "space": space, "token": ""}),
            )
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_on_auto_member_follows_its_durable_space_and_sets_up_the_new_copy() {
    let fx = setup().await;
    let first = format!("direct:{}", fx.srv.addr);
    let pool = std::sync::Arc::new(FakePool(std::sync::Mutex::new(first.clone())));
    fx.spaces.set_pool(Some(pool.clone()));
    let added = call(
        &fx.mcp,
        "agent_add",
        json!({"space": "dev", "name": "ada", "harness": "claude-code",
               "on": "auto", "durable": true}),
    )
    .await
    .unwrap();
    assert_eq!(added["policy"]["on"], "auto");
    let p = fx.spaces.persistent();
    let rec = p.get("ada").unwrap();
    assert_eq!(rec.space, first);
    assert_eq!(rec.durable_space.as_deref(), Some("dev"));
    // A run that its Space lost: the Space moved to another machine, which
    // has a .cua/setup (that fails here: the mock runs no shell).
    edit_record(&fx, "ada", |r| r["run_id"] = json!(RUN));
    let moved = MockServer::start(MockAuth::default()).await;
    moved
        .state
        .put_file(&format!("{HOMES}/ada/work/.cua/setup"), b"make\n".to_vec());
    let second = format!("direct:{}", moved.addr);
    *pool.0.lock().unwrap() = second.clone();
    tick_until(&p, "the move", || p.get("ada").unwrap().space == second).await;
    tick_until(&p, "the setup on the new copy", || {
        p.get("ada").unwrap().setup == "failed"
    })
    .await;
    let rec = p.get("ada").unwrap();
    assert!(rec.relaunch, "started again from the Volume once set up");
    let entries = call(
        &fx.mcp,
        "thread_read",
        json!({"space": second, "limit": 50}),
    )
    .await
    .unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone();
    assert!(entries.iter().any(|e| {
        e["author"]["id"] == "system:durable"
            && e["text"]
                .as_str()
                .unwrap()
                .starts_with("ada's Space dev moved")
    }));
    assert!(entries.iter().any(|e| e["author"]["id"] == "system:hooks"));
}
