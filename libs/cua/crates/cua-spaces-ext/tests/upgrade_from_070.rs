// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Upgrading from cua 0.7.0 (Cua Spaces 0.13.1) needs no step (CUA-2155).
//!
//! `tests/fixtures/upgrade/0.7.0/` holds state the released versions wrote
//! (how: its README):
//!
//! - `home/`: a cua home the released `cua` (the one in the Cua Spaces
//!   0.13.1 app) wrote through its daemon: persistent agents, routines
//!   (every/daily/weekly, one off), notifications (one read), the Volume
//!   (`fs` backend) with the agents' homes;
//! - `runs-home/`: the persistent state the 0.7.0 code wrote with live runs
//!   (agents with a current run and a turn cursor, a failed turn's
//!   notification, a watched `agent_start` run), against the spacesd mock;
//! - `golden/mcp.jsonl`: the 0.7.0 daemon's answers over `home/`.
//!
//! The new code reads all of it in place, answers the old tool names with
//! every field 0.7.0 answered (same values), the new names the same, and
//! what it saves still has every field 0.7.0 needs, with the old values, so
//! going back to 0.7.0 loses nothing it knew.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::SpacesPersistent as _;
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade/0.7.0")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), &dst).unwrap();
        }
    }
}

/// Every file under `dir` and its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.insert(
                    e.path().strip_prefix(dir).unwrap().to_path_buf(),
                    std::fs::read(e.path()).unwrap(),
                );
            }
        }
    }
    out
}

/// A copy of the fixture home `name` and the new code over it, as the
/// daemon builds it (the Volume on the `fs` backend in the home).
fn open(name: &str) -> (tempfile::TempDir, Spaces) {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&fixtures().join(name), tmp.path());
    let spaces = cua_spaces_ext::register(
        Spaces::builder().home(tmp.path()),
        cua_volume::Drive::open_local(tmp.path()),
        None,
    )
    .build();
    (tmp, spaces)
}

/// Calls `tool` over JSON-RPC `tools/call`, as an MCP client does.
async fn call(server: &McpServer, tool: &str, args: &Value) -> Value {
    let r = Box::pin(
        server.handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": tool, "arguments": args}})),
    )
    .await
    .unwrap();
    assert!(r.get("error").is_none(), "{tool}: {r}");
    let result = &r["result"];
    assert_ne!(result["isError"], true, "{tool} {args}: {result}");
    let text = result["content"][0]["text"].as_str().unwrap_or("");
    serde_json::from_str(text).unwrap_or(Value::String(text.into()))
}

/// The recorded 0.7.0 answers: (tool, args, parsed answer).
fn golden() -> Vec<(String, Value, Value)> {
    std::fs::read_to_string(fixtures().join("golden/mcp.jsonl"))
        .unwrap()
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            let text = v["response"]["content"][0]["text"].as_str().unwrap_or("");
            (
                v["tool"].as_str().unwrap().to_string(),
                v["args"].clone(),
                serde_json::from_str(text).unwrap_or(Value::String(text.into())),
            )
        })
        .collect()
}

/// Fields whose value depends on when the call is made.
const NOW_DEPENDENT: [&str; 1] = ["next_fire"];
/// Fields only the daemon adds (its Volume sync feed); this in-process
/// server has none. `upgrade_skew_live.rs` checks them against the daemon.
const DAEMON_ONLY: [&str; 1] = ["sync"];

/// `new` has every field `old` has, with the same value (arrays element by
/// element, in order); it may have more. Returns the differences.
fn superset(old: &Value, new: &Value, at: &str, out: &mut Vec<String>) {
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            for (k, ov) in o {
                let here = format!("{at}.{k}");
                match n.get(k) {
                    None if DAEMON_ONLY.contains(&k.as_str()) => {}
                    None => out.push(format!("{here}: missing (0.7.0: {ov})")),
                    Some(nv) if NOW_DEPENDENT.contains(&k.as_str()) => {
                        // Same presence and kind (a time, or null when off).
                        if ov.is_null() != nv.is_null() {
                            out.push(format!("{here}: {ov} became {nv}"));
                        }
                    }
                    Some(nv) => superset(ov, nv, &here, out),
                }
            }
        }
        (Value::Array(o), Value::Array(n)) => {
            if o.len() != n.len() {
                out.push(format!("{at}: {} items became {}", o.len(), n.len()));
                return;
            }
            for (i, (ov, nv)) in o.iter().zip(n).enumerate() {
                superset(ov, nv, &format!("{at}[{i}]"), out);
            }
        }
        _ if old == new => {}
        _ => out.push(format!("{at}: {old} became {new}")),
    }
}

fn assert_superset(what: &str, old: &Value, new: &Value) {
    let mut diffs = Vec::new();
    superset(old, new, what, &mut diffs);
    assert!(diffs.is_empty(), "{what}:\n{}", diffs.join("\n"));
}

/// The old tools answer over the 0.7.0 home with every field 0.7.0 gave.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_tools_answer_the_070_home_as_070_did() {
    let (_home, spaces) = open("home");
    let server = McpServer::new(spaces);
    let mut checked = 0;
    for (tool, args, old) in golden() {
        // Host facts (installed harnesses) and the tool list are not data.
        if matches!(tool.as_str(), "agent_capabilities" | "LIST") {
            continue;
        }
        let new = call(&server, &tool, &args).await;
        assert_superset(&format!("{tool} {args}"), &old, &new);
        checked += 1;
    }
    assert!(checked >= 7, "only {checked} recorded calls checked");
}

/// The current names answer the same data as the names 0.7.0 had.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_names_read_the_same_070_data() {
    let (_home, spaces) = open("home");
    let server = McpServer::new(spaces);
    let old = call(&server, "routine_list", &json!({})).await;
    let new = call(&server, "agent_schedule_list", &json!({})).await;
    assert_eq!(old, new);
    assert_eq!(new["routines"].as_array().unwrap().len(), 3);
    // The off routine stays off, the others have a next firing.
    for r in new["routines"].as_array().unwrap() {
        assert_eq!(r["next_fire"].is_null(), r["isEnabled"] == false, "{r}");
    }
    let agents = call(&server, "persistent_agent_list", &json!({})).await;
    let names: Vec<&str> = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["ada", "scout", "rev"]);
}

/// The Volume 0.7.0 wrote is read in place, versions and all, and reading
/// rewrites nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_070_volume_is_read_in_place() {
    let (home, spaces) = open("home");
    let data = home.path().join("volume/data");
    let before = snapshot(&data);
    let server = McpServer::new(spaces);
    let memory = call(
        &server,
        "volume_read",
        &json!({"path": "agents/ada/memory/MEMORY.md"}),
    )
    .await;
    assert!(
        memory.to_string().contains("ada prefers short answers"),
        "{memory}"
    );
    let history = call(
        &server,
        "volume_history",
        &json!({"path": "agents/scout/notes.txt"}),
    )
    .await;
    assert!(
        history.to_string().contains("1792859158976-1a9ab2f9"),
        "the 0.7.0 version is listed: {history}"
    );
    let ls = call(&server, "volume_ls", &json!({"path": "agents/"})).await;
    for a in ["agents/ada/", "agents/scout/"] {
        assert!(ls.to_string().contains(a), "{a}: {ls}");
    }
    assert_eq!(before, snapshot(&data), "reading rewrote the Volume");
}

/// The fields 0.7.0 needs in each record to read a file at all (the ones
/// without a serde default in its structs).
const REQUIRED_070: [(&str, &[&str]); 4] = [
    ("persistent/agents.json", &["name", "harness", "space"]),
    (
        "persistent/routines.json",
        &["id", "botID", "title", "prompt", "schedule", "createdAt"],
    ),
    (
        "persistent/notifications.json",
        &["id", "at_ms", "kind", "title", "body"],
    ),
    (
        "persistent/watched-runs.json",
        &["space", "run_id", "agent", "added_ms"],
    ),
];

fn records(home: &Path, file: &str) -> Vec<Value> {
    match std::fs::read(home.join(file)) {
        Ok(b) => {
            let v: Value = serde_json::from_slice(&b).unwrap();
            match v {
                Value::Array(a) => a,
                Value::Object(ref o) => o
                    .values()
                    .find_map(|x| x.as_array().cloned())
                    .unwrap_or_default(),
                _ => vec![],
            }
        }
        Err(_) => vec![],
    }
}

fn key(file: &str, r: &Value) -> String {
    let k = if file.contains("agents") {
        "name"
    } else if file.contains("watched") {
        "run_id"
    } else {
        "id"
    };
    r[k].as_str().unwrap_or_default().to_string()
}

/// After the new code saves each file, every record 0.7.0 wrote keeps every
/// field and value it had (a downgrade reads them as before), and every
/// record has the fields 0.7.0 requires.
fn assert_downgrade_safe(original: &Path, saved: &Path) {
    for (file, required) in REQUIRED_070 {
        let before = records(original, file);
        let after = records(saved, file);
        for r in &after {
            for k in required {
                assert!(r.get(*k).is_some(), "{file}: {k} missing in {r}");
            }
        }
        for old in &before {
            let id = key(file, old);
            let Some(new) = after.iter().find(|n| key(file, n) == id) else {
                continue; // removed on purpose by the test
            };
            // Changed on purpose by the test: compared by the caller.
            let mut old = old.clone();
            let mut new = new.clone();
            for k in [
                "read",
                "isEnabled",
                "paused",
                "last_error",
                "space_state",
                "cursor",
                "notified_error",
                "notified_turn",
            ] {
                old.as_object_mut().unwrap().remove(k);
                new.as_object_mut().unwrap().remove(k);
            }
            assert_superset(&format!("{file} {id}"), &old, &new);
        }
    }
}

/// Writes through the new code (by old and new tool names) keep the 0.7.0
/// shapes: a downgrade back to 0.7.0 loses nothing it knew.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saving_with_the_new_code_keeps_the_070_shapes() {
    let (home, spaces) = open("home");
    let server = McpServer::new(spaces);
    let routines = call(&server, "routine_list", &json!({})).await;
    let weekly = routines["routines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Weekly report")
        .unwrap()["id"]
        .clone();
    // Each file saved once by the new code, by old and by new names.
    call(
        &server,
        "agent_schedule_set_enabled",
        &json!({"id": weekly, "enabled": true}),
    )
    .await;
    call(
        &server,
        "routine_add",
        &json!({"agent": "rev", "title": "Hourly", "prompt": "check", "every_minutes": 30}),
    )
    .await;
    call(
        &server,
        "agent_add",
        &json!({"name": "neo", "agent": "hermes", "space": "local:dev"}),
    )
    .await;
    call(
        &server,
        "notifications_ack",
        &json!({"ids": ["44d0fee7b23210f3"]}),
    )
    .await;
    call(
        &server,
        "notify_user",
        &json!({"title": "From the new code", "body": "hello"}),
    )
    .await;

    assert_downgrade_safe(&fixtures().join("home"), home.path());
    // The changes themselves, in 0.7.0's terms.
    let r = records(home.path(), "persistent/routines.json");
    assert_eq!(r.len(), 4);
    assert!(r.iter().all(|r| r["isEnabled"] == true), "{r:?}");
    let n = records(home.path(), "persistent/notifications.json");
    assert!(
        n.iter()
            .all(|n| n["read"] == true || n["title"] == "From the new code")
    );
    let a = records(home.path(), "persistent/agents.json");
    assert_eq!(
        a.iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["ada", "scout", "rev", "neo"]
    );
}

/// What 0.7.0 recorded about live runs (an agent's current run and turn
/// cursor, a failed turn's notification, a watched `agent_start` run) loads
/// as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_run_state_from_070_loads() {
    let (home, spaces) = open("runs-home");
    let p = spaces.persistent();
    let agents = p.list().unwrap();
    let kit = agents.iter().find(|a| a.name == "kit").unwrap();
    assert!(kit.run_id.is_some(), "{kit:?}");
    let lee = agents.iter().find(|a| a.name == "lee").unwrap();
    assert_eq!(lee.model.as_deref(), Some("gpt-5"));
    assert!(lee.notified_error.is_some(), "{lee:?}");
    let watched = p.watched_runs().unwrap();
    assert_eq!(watched.len(), 1);
    assert_eq!(watched[0].run_id, "run-0000wat1");
    let feed = p.feed().list(false, None).unwrap();
    assert!(
        feed.iter().any(
            |n| n.kind == "error" && n.title == "lee stopped: API Error: 401 invalid x-api-key"
        ),
        "{feed:?}"
    );
    // Listing rewrote nothing.
    assert_eq!(
        snapshot(&home.path().join("persistent")),
        snapshot(&fixtures().join("runs-home/persistent"))
    );
}
