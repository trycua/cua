// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Durable members against the in-process spacesd mock and a Volume in a
//! temp dir: the supervisor's view of a durable run (crashed, fenced,
//! permanent, lost), a re-drive at the next epoch with the Volume fence
//! raised first, the journal shipped from the run directory with the ack
//! written back into it, and a restart from the Volume that a failed
//! `.cua/setup` stops. The run's files are written by the test, as the
//! runner would; nothing runs in a guest.

use std::sync::Arc;

use cua_spaces::agents::Agents;
use cua_spaces_ext::durable_member::{self, AgentsEngine, Redrive, Step};
use cua_spaces_ext::hooks::{self, Lookup};
use cua_spaces_ext::session_journal::{self, GuestDir, Shipper};
use cua_spacesd_client::SpacesdClient;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use cua_volume::Backend;
use cua_volume::journal;
use serde_json::{Value, json};

const RUNS: &str = "/home/cua/.cua/agents";
const RUN: &str = "run-0000000a";

struct Lab {
    srv: MockServer,
    agents: Agents,
    backend: Arc<dyn Backend>,
    _vol: tempfile::TempDir,
}

async fn lab() -> Lab {
    let srv = MockServer::start(MockAuth::default()).await;
    let guest = SpacesdClient::connect_url(&format!("http://{}", srv.addr), None)
        .await
        .unwrap();
    let agents = Agents::new(guest).await.unwrap();
    let vol = tempfile::tempdir().unwrap();
    let backend: Arc<dyn Backend> = Arc::new(cua_volume::fs::FsBackend::new(vol.path()));
    Lab {
        srv,
        agents,
        backend,
        _vol: vol,
    }
}

impl Lab {
    fn put(&self, file: &str, body: impl Into<Vec<u8>>) {
        self.srv
            .state
            .put_file(&format!("{RUNS}/{RUN}/{file}"), body.into());
    }

    fn file(&self, file: &str) -> Option<Vec<u8>> {
        self.srv.state.file(&format!("{RUNS}/{RUN}/{file}"))
    }

    /// A durable run at epoch 1 whose process is gone.
    fn crashed_run(&self) {
        self.put(
            "meta.json",
            json!({"run_id": RUN, "harness": "claude-code", "prompt": "hi",
                   "cwd": "/home/cua/work", "created_at": 0.0})
            .to_string(),
        );
        self.put(
            "run.json",
            json!({"harness": "claude-code", "durable": {"epoch": 1, "resume": false,
                   "session": RUN, "ship": {"ackFile": "ship.ack", "ackTimeoutMs": 30000}}})
            .to_string(),
        );
        self.put("launch.sh", "#!/bin/sh\n");
        self.put(
            "state.json",
            json!({"status": "running", "turn": 1}).to_string(),
        );
    }

    fn engine(&self) -> AgentsEngine {
        AgentsEngine {
            agents: self.agents.clone(),
            backend: Some(self.backend.clone()),
            volume_space: "local:dev".into(),
            session: RUN.into(),
            holder: "this-mac/local:dev".into(),
            hooks: Lookup {
                workdirs: vec!["/home/cua/agents/a/work".into()],
                volume_space: Some("local:dev".into()),
                stage_dir: None,
            },
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crashed_durable_run_is_redriven_at_the_next_epoch_behind_the_volume_fence() {
    let lab = lab().await;
    lab.crashed_run();
    let engine = lab.engine();
    let mut st = Redrive::default();
    let t = 5_000_000;
    assert_eq!(
        durable_member::step(&engine, RUN, &mut st, t)
            .await
            .unwrap(),
        Step::Waiting { until_ms: t + 2000 }
    );
    let step = durable_member::step(&engine, RUN, &mut st, t + 2000)
        .await
        .unwrap();
    assert_eq!(
        step,
        Step::Redriven {
            epoch: 2,
            hook: None
        }
    );
    let run: Value = serde_json::from_slice(&lab.file("run.json").unwrap()).unwrap();
    assert_eq!(run["durable"]["epoch"], 2);
    assert_eq!(run["durable"]["resume"], true);
    let prefix = journal::session_prefix("local:dev", RUN).unwrap();
    let (fence, _) = journal::read_fence(lab.backend.as_ref(), &prefix)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (fence.epoch, fence.holder.as_str()),
        (2, "this-mac/local:dev")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fenced_permanent_and_lost_runs_are_not_redriven() {
    let lab = lab().await;
    lab.crashed_run();
    let engine = lab.engine();
    // Another host raised the local floor above this epoch.
    lab.put("epoch", "5");
    let mut st = Redrive::default();
    match durable_member::step(&engine, RUN, &mut st, 0)
        .await
        .unwrap()
    {
        Step::GaveUp { why } => assert_eq!(
            why,
            "epoch 5 drives this session elsewhere (this host had epoch 1)"
        ),
        other => panic!("{other:?}"),
    }
    // Exit 78: permanent.
    lab.put("epoch", "1");
    lab.put(
        "state.json",
        json!({"status": "failed", "exitCode": 78, "error": "the API key was rejected"})
            .to_string(),
    );
    let mut st = Redrive::default();
    assert_eq!(
        durable_member::step(&engine, RUN, &mut st, 0)
            .await
            .unwrap(),
        Step::GaveUp {
            why: "the run cannot succeed: the API key was rejected".into()
        }
    );
    // Any other failure (the runner gave up restarting the agent): a crash.
    lab.put(
        "state.json",
        json!({"status": "failed", "exitCode": 1, "error": "agent exited"}).to_string(),
    );
    let mut st = Redrive::default();
    assert!(matches!(
        durable_member::step(&engine, RUN, &mut st, 0)
            .await
            .unwrap(),
        Step::Waiting { .. }
    ));
    // A run that is not there: lost (restart it from the Volume).
    let mut st = Redrive::default();
    assert!(matches!(
        durable_member::step(&engine, "run-0000ffff", &mut st, 0)
            .await
            .unwrap(),
        Step::Lost { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_journal_ships_from_the_run_dir_and_a_failed_setup_stops_the_restart() {
    let lab = lab().await;
    lab.crashed_run();
    let tools = "{\"seq\":1,\"kind\":\"intent\",\"toolCallId\":\"t1\",\"epoch\":1}\n";
    let events =
        "{\"seq\":1,\"type\":\"turn_started\",\"turn\":1}\n{\"seq\":2,\"type\":\"permission\"}\n";
    lab.put("tools.jsonl", tools);
    lab.put("events.jsonl", events);
    let dir = GuestDir {
        guest: lab.agents.guest().clone(),
        dir: format!("{RUNS}/{RUN}"),
    };
    let mut ship = Shipper::new(lab.backend.clone(), "local:dev", RUN, 1).unwrap();
    let r = ship.tick(&dir).await.unwrap();
    assert_eq!(r.lines, 3);
    let ack: Value = serde_json::from_slice(&lab.file("ship.ack").unwrap()).unwrap();
    assert_eq!(
        ack,
        json!({"epoch": 1, "end": {"events.jsonl": events.len(), "tools.jsonl": tools.len()}})
    );
    // The Space is gone; its new copy's .cua/setup (from the Volume) fails
    // here (the mock runs no shell): nothing is started, the fence is
    // above every shipped epoch, and the journal was restored intact.
    lab.backend
        .put(
            &hooks::volume_key("local:dev", hooks::HookKind::Setup),
            b"#!/bin/sh\nexit 1\n".to_vec(),
            cua_volume::Condition::None,
        )
        .await
        .unwrap();
    let restart = durable_member::start_from_volume(
        &lab.agents,
        &lab.backend,
        "local:dev",
        RUN,
        "other-mac/relay:abc",
        "claude-code",
        Default::default(),
        Some(&Lookup {
            workdirs: vec!["/home/cua/work".into()],
            volume_space: Some("local:dev".into()),
            stage_dir: None,
        }),
    )
    .await
    .unwrap();
    assert!(restart.started.is_none());
    let setup = restart.setup.unwrap();
    assert!(!setup.ok);
    assert_eq!(setup.source, "volume:spaces/local-dev/.cua/setup");
    assert_eq!(restart.epoch, 2);
    assert_eq!(restart.restored.files["tools.jsonl"], tools.as_bytes());
    assert_eq!(restart.restored.files["events.jsonl"], events.as_bytes());
    // The old host's shipper is fenced now, and fences its runner.
    let r = ship.tick(&dir).await.unwrap();
    assert_eq!(r.fenced, Some(2));
    assert_eq!(lab.file("epoch").unwrap(), b"2");
    // The restore helper rebuilds the same files anywhere.
    let mem = session_journal::MemDir::default();
    session_journal::restore_into(lab.backend.as_ref(), "local:dev", RUN, &mem)
        .await
        .unwrap();
    assert_eq!(mem.get("tools.jsonl").unwrap(), tools.as_bytes());
}
