// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The end of a `space_job` wakes the agent that started it: a task from the
//! daemon in the Space's thread, addressed to the agent, with the structured
//! result line; a person who started it gets a notification instead. Against
//! the in-process spacesd mock and a temporary cua home. The job's own
//! behavior (detached runner, logs, cancel) is tested where its scripts are,
//! in `cua_spaces::space_jobs`, and on real Spaces by the e2e suite.

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces::space_jobs::JobStatus;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::jobs::WatchedJob;
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
    let fx = Fx {
        _srv: srv,
        _home: home,
        spaces,
        space: info.id,
        mcp,
    };
    for name in ["ada", "bo"] {
        call(
            &fx,
            &Caller::Owner,
            "agent_add",
            json!({"space": fx.space, "name": name, "harness": "claude-code",
                   "env_from_host": ["ANTHROPIC_API_KEY"]}),
        )
        .await
        .unwrap();
    }
    fx
}

async fn call(fx: &Fx, caller: &Caller, tool: &str, args: Value) -> Result<Value, String> {
    let out = caller
        .clone()
        .scope(Box::pin(fx.mcp.call(tool, args)))
        .await;
    cua_spaces::extension::outcome_json(out).map_err(|e| e.to_string())
}

fn ended(job: &str, state: &str, code: Option<i32>) -> JobStatus {
    JobStatus {
        job: job.into(),
        name: "unit tests".into(),
        state: state.into(),
        exit_code: code,
        signal: None,
        started_at: Some(100),
        ended_at: Some(875),
        elapsed_s: 775,
        size: 2048,
        command: "run the tests".into(),
    }
}

fn agent(name: &str, fx: &Fx) -> Caller {
    Caller::Agent {
        name: name.into(),
        space: fx.space.clone(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_that_started_a_job_gets_its_end_as_a_task() {
    let fx = setup().await;
    let p = fx.spaces.persistent();
    let w = p
        .watch_job(&agent("ada", &fx), &fx.space, "j1", "unit tests", None)
        .unwrap();
    assert_eq!(w["wake"], "ada");
    // Watching twice follows the job once.
    p.watch_job(&agent("ada", &fx), &fx.space, "j1", "unit tests", None)
        .unwrap();
    let watched: Vec<WatchedJob> = p.watched_jobs().unwrap();
    assert_eq!(watched.len(), 1);
    assert_eq!(watched[0].agent.as_deref(), Some("ada"));

    p.post_job_end(
        &watched[0],
        &ended("j1", "exited", Some(0)),
        Some(("all 412 passed\n", 2048)),
    )
    .unwrap();

    let inbox = call(
        &fx,
        &Caller::Owner,
        "thread_inbox",
        json!({"space": fx.space, "member": "ada"}),
    )
    .await
    .unwrap();
    let tasks = inbox["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1, "{inbox}");
    let text = tasks[0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("JOB j1 exited exit=0 12m55s log=2048B | unit tests"),
        "{text}"
    );
    assert!(text.contains("all 412 passed"), "{text}");
    assert_eq!(tasks[0]["to"], "ada");
    // The other agent was not woken, and no person was notified: the agent
    // decides whether the person needs to know.
    let other = call(
        &fx,
        &Caller::Owner,
        "thread_inbox",
        json!({"space": fx.space, "member": "bo"}),
    )
    .await
    .unwrap();
    assert!(other["tasks"].as_array().unwrap().is_empty());
    assert!(p.feed().list(false, None).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_a_person_started_notifies_the_person() {
    let fx = setup().await;
    let p = fx.spaces.persistent();
    let w = p
        .watch_job(&Caller::Owner, &fx.space, "j2", "build", None)
        .unwrap();
    assert!(w["wake"].is_null());
    let watched = p.watched_jobs().unwrap();
    p.post_job_end(
        &watched[0],
        &ended("j2", "exited", Some(2)),
        Some(("boom\n", 5)),
    )
    .unwrap();
    let notes = p.feed().list(false, None).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].kind, "error");
    assert!(notes[0].title.ends_with("failed"), "{}", notes[0].title);
    assert!(notes[0].body.starts_with("JOB j2 exited exit=2"));
    // The thread has the note too, and nobody was given a task.
    let thread = call(
        &fx,
        &Caller::Owner,
        "thread_read",
        json!({"space": fx.space}),
    )
    .await
    .unwrap();
    let last = thread["entries"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert!(last["text"].as_str().unwrap().starts_with("JOB j2"));
    assert!(last["task"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waking_another_agent_is_checked_and_a_job_that_vanished_is_said() {
    let fx = setup().await;
    let p = fx.spaces.persistent();
    // `wake` names an agent member of this Space.
    let e = p
        .watch_job(&Caller::Owner, &fx.space, "j3", "x", Some("nobody"))
        .unwrap_err();
    assert_eq!(e.tag(), "not_found", "{e}");
    let w = p
        .watch_job(&agent("ada", &fx), &fx.space, "j3", "x", Some("bo"))
        .unwrap();
    assert_eq!(w["wake"], "bo");
    // A job whose record is gone ends as `lost`, in words.
    let watched = p.watched_jobs().unwrap();
    p.post_job_end(&watched[0], &ended("j3", "lost", None), None)
        .unwrap();
    let inbox = call(
        &fx,
        &Caller::Owner,
        "thread_inbox",
        json!({"space": fx.space, "member": "bo"}),
    )
    .await
    .unwrap();
    let text = inbox["tasks"][0]["text"].as_str().unwrap().to_string();
    assert!(text.starts_with("JOB j3 lost"), "{text}");
    assert!(text.contains("result is unknown"), "{text}");
}
