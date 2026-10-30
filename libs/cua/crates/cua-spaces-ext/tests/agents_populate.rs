// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! How long the Agents page's reads take on the daemon (CUA-2849): a
//! dataset like a busy account's, 13 Spaces with six agents and long
//! threads, then the calls the page makes for each Space
//! (`agent_members {overview}`, `thread_read {limit: 1}`, and the DM's
//! `thread_read {with}`), timed one by one. The thresholds are generous
//! (several times the numbers on a laptop): they fail on a read that grows
//! with the whole thread for every agent (an N+1), not on a slow machine.
//! Against in-process spacesd mocks and a temporary home.

use std::time::{Duration, Instant};

use cua_spaces::Spaces;
use cua_spaces::extension::Caller;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::thread::{Author, Draft, EntryKind, TaskTo, ThreadStore};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Value, json};

const SPACES: usize = 13;
const LONG: usize = 3;
const AGENTS: [&str; 6] = ["claude", "codex", "hermes-test", "ada", "bo", "cy"];

fn entries(n: usize) -> Vec<Draft> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let agent = AGENTS[i % AGENTS.len()];
        if i % 3 == 0 {
            let mut d = Draft::new(
                Author::owner(),
                EntryKind::Message,
                format!(
                    "@{agent} task number {i}: please look at the failing build and report back"
                ),
            );
            d.mentions = vec![agent.into()];
            d.task = Some(TaskTo {
                to: agent.into(),
                session: None,
            });
            out.push(d);
        } else {
            out.push(Draft::new(
                Author::agent(agent),
                EntryKind::Message,
                format!("Reply {i}: I looked at it and the retry timeout is the cause, a longer one fixes it. ").repeat(3),
            ));
        }
    }
    out
}

async fn call(mcp: &McpServer, tool: &str, args: Value) -> (Value, Duration) {
    let t = Instant::now();
    let out = Caller::Owner.scope(Box::pin(mcp.call(tool, args))).await;
    let v = cua_spaces::extension::outcome_json(out).unwrap_or_else(|e| panic!("{tool}: {e}"));
    (v, t.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pages_reads_do_not_grow_with_the_whole_thread_for_every_agent() {
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home.path()),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let mcp = McpServer::new(spaces.clone());
    let mut servers = vec![];
    let mut ids = vec![];
    for i in 0..SPACES {
        let srv = MockServer::start(MockAuth::default()).await;
        let info = spaces
            .add(
                &format!("http://{}", srv.addr),
                None,
                Some(format!("space-{i}")),
            )
            .await
            .unwrap();
        servers.push(srv);
        ids.push(info.id);
    }
    for (i, id) in ids.iter().enumerate() {
        for name in AGENTS {
            call(
                &mcp,
                "agent_add",
                json!({"space": id, "name": name, "harness": "claude-code"}),
            )
            .await;
        }
        let n = if i < LONG { 2000 } else { 40 };
        ThreadStore::open(home.path(), id)
            .append_many(entries(n))
            .unwrap();
    }

    let mut worst_overview = Duration::ZERO;
    let mut worst_tail = Duration::ZERO;
    let mut worst_dm = Duration::ZERO;
    let started = Instant::now();
    for id in &ids {
        let (_, o) = call(
            &mcp,
            "agent_members",
            json!({"space": id, "overview": true}),
        )
        .await;
        let (_, t) = call(&mcp, "thread_read", json!({"space": id, "limit": 1})).await;
        worst_overview = worst_overview.max(o);
        worst_tail = worst_tail.max(t);
    }
    let list_total = started.elapsed();
    for id in ids.iter().take(LONG) {
        let (page, d) = call(
            &mcp,
            "thread_read",
            json!({"space": id, "with": "claude", "limit": 100}),
        )
        .await;
        assert!(page["entries"].as_array().unwrap().len() <= 100);
        worst_dm = worst_dm.max(d);
    }
    eprintln!(
        "agents_populate: {SPACES} spaces, {LONG} with 2000 entries: overview worst {worst_overview:?}, tail worst {worst_tail:?}, whole list serial {list_total:?}, dm worst {worst_dm:?}"
    );
    assert!(
        worst_overview < Duration::from_millis(1500),
        "agent_members overview {worst_overview:?}"
    );
    assert!(
        worst_tail < Duration::from_millis(1500),
        "thread_read tail {worst_tail:?}"
    );
    assert!(
        worst_dm < Duration::from_millis(1500),
        "thread_read dm {worst_dm:?}"
    );
    assert!(
        list_total < Duration::from_secs(8),
        "serial list {list_total:?}"
    );
    drop(servers);
}

/// Marks every agent of `ids` as running (a `run_id` on its record), as the
/// supervisor leaves them while their runs are up.
fn mark_running(home: &std::path::Path, ids: &[String]) {
    let path = std::fs::read_dir(home)
        .unwrap()
        .flatten()
        .map(|e| e.path().join("agents.json"))
        .find(|p| p.exists())
        .or_else(|| walk(home, "agents.json"))
        .expect("agents.json");
    let mut all: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for a in &mut all {
        if ids.iter().any(|i| a["space"] == i.as_str()) {
            a["run_id"] = json!(format!("run-{}", a["name"].as_str().unwrap()));
        }
    }
    std::fs::write(&path, serde_json::to_vec(&all).unwrap()).unwrap();
}

fn walk(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
        if p.is_dir()
            && let Some(f) = walk(&p, name)
        {
            return Some(f);
        }
    }
    None
}

/// A Space that can't be reached while its agents show a run (a stopped
/// machine, a machine that is off) must not hold the page's read: the
/// overview answers at once with what is known.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_space_with_running_agents_does_not_hold_the_overview() {
    let home = tempfile::tempdir().unwrap();
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(home.path()),
        cua_volume::Drive::open_local(home.path()),
        None,
    )
    .build();
    let mcp = McpServer::new(spaces.clone());
    let srv = MockServer::start(MockAuth::default()).await;
    let info = spaces
        .add(&format!("http://{}", srv.addr), None, Some("gone".into()))
        .await
        .unwrap();
    for name in AGENTS {
        call(
            &mcp,
            "agent_add",
            json!({"space": info.id, "name": name, "harness": "claude-code"}),
        )
        .await;
    }
    mark_running(home.path(), std::slice::from_ref(&info.id));
    drop(srv);
    let t = Instant::now();
    let r = spaces.space(&info.id).await.map(|_| ());
    eprintln!(
        "agents_populate: spaces.space() on it {:?} -> {:?}",
        t.elapsed(),
        r.err().map(|e| e.to_string())
    );
    let (_, d) = call(&mcp, "thread_read", json!({"space": info.id, "limit": 1})).await;
    eprintln!("agents_populate: thread_read on it {d:?}");
    let (_, d) = call(&mcp, "agent_members", json!({"space": info.id})).await;
    eprintln!("agents_populate: agent_members (no overview) on it {d:?}");
    let mut times = vec![];
    for _ in 0..3 {
        let (rows, d) = call(
            &mcp,
            "agent_members",
            json!({"space": info.id, "overview": true}),
        )
        .await;
        assert_eq!(rows["members"].as_array().unwrap().len(), AGENTS.len());
        times.push(d);
    }
    eprintln!(
        "agents_populate: unreachable space with {} running agents, overview x3: {times:?}",
        AGENTS.len()
    );
    for d in &times {
        assert!(
            *d < Duration::from_secs(4),
            "overview of an unreachable space took {d:?}"
        );
    }
}
