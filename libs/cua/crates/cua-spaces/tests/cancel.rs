// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cancelling a create (`Spaces::cancel_create`) against fakes: a local
//! runtime whose boot never finishes and Cua Cloud's fake control plane.
//! What the create made is removed, nothing else is, and a second cancel
//! (or one after a daemon restart) is harmless.

use async_trait::async_trait;
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::placement::On;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::{CancelState, SpaceCreate, Spaces};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A runtime whose instances boot forever (a download or boot a cancel
/// cuts off): `start` records the instance, then never returns.
#[derive(Default)]
struct HangingRuntime {
    running: Mutex<BTreeMap<String, bool>>,
    deleted: Mutex<Vec<String>>,
    specs: Mutex<Vec<LocalStartSpec>>,
    /// `start` returns at once (the instance is up), and its cua-spacesd
    /// never answers: the create waits to connect.
    boots: std::sync::atomic::AtomicBool,
    /// The guest port: a listener that accepts and never answers (the
    /// readiness probe passes, the cua-spacesd handshake does not).
    port: std::sync::atomic::AtomicU16,
}

#[async_trait]
impl LocalRuntime for HangingRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        use cua_sandbox_core::progress::{Phase, Progress, Transfer, report};
        self.specs.lock().unwrap().push(spec.clone());
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        report(Progress::phase(Phase::Pulling).bytes(Transfer {
            done: 1,
            total: 10,
            per_second: None,
        }));
        if self.boots.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(LocalInstance {
                name: spec.name.clone(),
                backend: "fake".into(),
                status: InstanceStatus::Running,
                endpoints: self.endpoints(&spec.name).await?,
            });
        }
        std::future::pending::<()>().await;
        unreachable!()
    }
    async fn stop(&self, _name: &str) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().get(name) {
            Some(_) => Ok(InstanceStatus::Running),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.deleted.lock().unwrap().push(name.into());
        match self.running.lock().unwrap().remove(name) {
            Some(_) => Ok(()),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn endpoints(&self, _name: &str) -> Result<LocalEndpoints, RuntimeError> {
        // cua-spacesd never answers there.
        let port = match self.port.load(std::sync::atomic::Ordering::SeqCst) {
            0 => 9,
            p => p,
        };
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, port)].into(),
            ..Default::default()
        })
    }
}

struct Env {
    _reg: tempfile::TempDir,
    _state: tempfile::TempDir,
    rt: Arc<HangingRuntime>,
    spaces: Spaces,
}

fn env() -> Env {
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let rt = Arc::new(HangingRuntime::default());
    let spaces = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    Env {
        _reg: reg,
        _state: state,
        rt,
        spaces,
    }
}

fn local(name: Option<&str>, create_id: &str) -> SpaceCreate {
    SpaceCreate {
        on: Some(On::Local),
        image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
        spacesd: Some(true),
        name: name.map(str::to_string),
        create_id: Some(create_id.into()),
        gpu: Some("auto".into()),
        ..Default::default()
    }
}

async fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn journals(spaces: &Spaces) -> usize {
    std::fs::read_dir(spaces.home_dir().join("creating"))
        .map(|d| d.count())
        .unwrap_or(0)
}

#[tokio::test]
async fn a_cancelled_local_create_removes_what_it_made_once() {
    let e = env();
    // Progress names the Space (so a UI can cancel it) and carries bytes.
    let heard: Arc<Mutex<Vec<cua_spaces::CreateProgress>>> = Arc::default();
    let h = heard.clone();
    let mut opts = local(None, "pending:1");
    opts.progress = Some(cua_spaces::ProgressSink::new(move |p| {
        h.lock().unwrap().push(p.clone())
    }));
    let spaces = e.spaces.clone();
    let create = tokio::spawn(async move { spaces.create(opts).await });
    until("the instance exists", || {
        !e.rt.running.lock().unwrap().is_empty()
    })
    .await;
    let name = e.rt.running.lock().unwrap().keys().next().cloned().unwrap();
    let id = format!("local:{name}");
    assert_eq!(journals(&e.spaces), 1, "a journal while it runs");
    assert_eq!(
        e.rt.specs.lock().unwrap()[0].gpu.as_deref(),
        Some("auto"),
        "the GPU option reaches the runtime"
    );
    // By the caller's own key.
    let o = e.spaces.cancel_create("pending:1").await.unwrap();
    assert_eq!(o.state, CancelState::Cancelled, "{o:?}");
    assert_eq!(o.id, id);
    assert!(
        o.message.contains("removed its VM or container"),
        "{}",
        o.message
    );
    assert!(
        o.message.contains("finished image layers stay"),
        "{}",
        o.message
    );
    let err = create.await.unwrap().unwrap_err();
    assert_eq!(err.tag(), "cancelled", "{err}");
    assert!(
        e.rt.running.lock().unwrap().is_empty(),
        "the instance is gone"
    );
    assert_eq!(journals(&e.spaces), 0, "the journal is gone");
    assert!(e.spaces.list().unwrap().is_empty(), "nothing registered");
    {
        let heard = heard.lock().unwrap();
        assert!(heard.iter().all(|p| p.target == id), "{heard:?}");
        assert!(heard.iter().any(|p| p.bytes.is_some_and(|b| b.total == 10)));
    }
    // Twice: nothing left to do.
    let again = e.spaces.cancel_create(&id).await.unwrap();
    assert_eq!(again.state, CancelState::NotCreating);
}

/// A create under a name in use is cancelled without touching what had
/// that name; a create dropped mid-way (a closed request) is cleaned up
/// the same way as a cancelled one.
#[tokio::test]
async fn a_cancel_never_removes_what_existed_and_a_drop_cleans_up() {
    let e = env();
    e.rt.running.lock().unwrap().insert("theirs".into(), true);
    let spaces = e.spaces.clone();
    let create = tokio::spawn(async move { spaces.create(local(Some("theirs"), "p2")).await });
    until("the create runs", || e.rt.specs.lock().unwrap().len() == 1).await;
    let o = e.spaces.cancel_create("theirs").await.unwrap();
    assert_eq!(o.state, CancelState::Cancelled);
    assert!(o.message.contains("existed before"), "{}", o.message);
    assert!(create.await.unwrap().is_err());
    assert!(e.rt.running.lock().unwrap().contains_key("theirs"));
    assert!(e.rt.deleted.lock().unwrap().is_empty());

    // Dropped: the future goes away without a cancel.
    let spaces = e.spaces.clone();
    let dropped = tokio::spawn(async move { spaces.create(local(Some("dropme"), "p3")).await });
    until("the instance exists", || {
        e.rt.running.lock().unwrap().contains_key("dropme")
    })
    .await;
    dropped.abort();
    let _ = dropped.await;
    until("the dropped create is cleaned up", || {
        !e.rt.running.lock().unwrap().contains_key("dropme") && journals(&e.spaces) == 0
    })
    .await;
}

/// A create a dead process left (a daemon restart mid-create) is undone by
/// a cancel from the next one, and by the recovery when it was cancelled.
#[tokio::test]
async fn a_cancel_after_a_restart_undoes_the_journal() {
    let e = env();
    let dead_pid = {
        let mut c = std::process::Command::new("true").spawn().unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    };
    let dir = e.spaces.home_dir().join("creating");
    std::fs::create_dir_all(&dir).unwrap();
    let journal = |name: &str, fresh: bool| {
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::json!({
                "name": name, "token": "t0k", "pid": dead_pid, "spacesd": true, "started": 0,
                "kind": "local", "id": format!("local:{name}"), "create_id": format!("app:{name}"),
                "made": [{"type": "local_sandbox", "name": name, "fresh": fresh}],
            })
            .to_string(),
        )
        .unwrap();
        e.rt.running.lock().unwrap().insert(name.into(), true);
    };
    journal("orphan", true);
    let o = e.spaces.cancel_create("app:orphan").await.unwrap();
    assert_eq!(o.state, CancelState::Cancelled, "{o:?}");
    assert!(!e.rt.running.lock().unwrap().contains_key("orphan"));
    assert!(!dir.join("orphan.json").exists());
    // A cancel marker left for a process that then died: the recovery
    // removes instead of registering; a name that was in use stays.
    journal("marked", true);
    std::fs::write(dir.join("marked.cancel"), b"cancel\n").unwrap();
    journal("borrowed", false);
    std::fs::write(dir.join("borrowed.cancel"), b"cancel\n").unwrap();
    let got = e
        .spaces
        .recover_interrupted_creates(Duration::from_secs(1))
        .await;
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(!e.rt.running.lock().unwrap().contains_key("marked"));
    assert!(e.rt.running.lock().unwrap().contains_key("borrowed"));
    assert_eq!(journals(&e.spaces), 0);
}

/// A cloud create cancelled while its claim waits to bind releases the
/// claim it made.
#[tokio::test]
async fn a_cancelled_cloud_create_releases_its_claim() {
    let reg = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let spaces = Spaces::builder()
        .home(reg.path())
        .fleet(fake.client())
        .fleet_namespace("cua-e2e-cx")
        .build();
    let pool = spaces.fleet_pool_name(
        cua_spaces::contract::inputs::FleetRuntime::Kubevirt,
        "ghcr.io/trycua/linux:24.04-disk",
    );
    // No replica ever becomes ready: the claim waits to bind.
    let s = spaces.clone();
    let create = tokio::spawn(async move {
        s.create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some("ghcr.io/trycua/linux:24.04-disk".into()),
            name: Some("cua-e2e-cancel".into()),
            ..Default::default()
        })
        .await
    });
    until("the claim exists", || {
        fake.exists("claim", &pool, "cua-e2e-cancel")
    })
    .await;
    let o = spaces.cancel_create("cloud:cua-e2e-cancel").await.unwrap();
    assert_eq!(o.state, CancelState::Cancelled, "{o:?}");
    assert!(
        o.message.contains("released its cloud sandbox"),
        "{}",
        o.message
    );
    assert_eq!(create.await.unwrap().unwrap_err().tag(), "cancelled");
    assert!(!fake.exists("claim", &pool, "cua-e2e-cancel"), "released");
    assert!(spaces.list().unwrap().is_empty());
}

/// Cancelled after the instance is up, while the create waits for its
/// cua-spacesd: the Spaces layer removes the instance it made.
#[tokio::test]
async fn a_cancel_while_connecting_removes_the_running_instance() {
    let e = env();
    e.rt.boots.store(true, std::sync::atomic::Ordering::SeqCst);
    // Accepts (the TCP probe passes) and never speaks (no handshake).
    let mute = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    e.rt.port.store(
        mute.local_addr().unwrap().port(),
        std::sync::atomic::Ordering::SeqCst,
    );
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = mute.accept().await {
            held.push(sock);
        }
    });
    let spaces = e.spaces.clone();
    let mut opts = local(Some("connecting"), "p4");
    opts.timeout = Some(Duration::from_secs(120));
    let create = tokio::spawn(async move { spaces.create(opts).await });
    until("the instance runs", || {
        e.rt.running.lock().unwrap().contains_key("connecting")
    })
    .await;
    // Past the start and the probe: the sandbox layer's create has
    // returned, the Spaces layer waits for the handshake.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let o = e.spaces.cancel_create("p4").await.unwrap();
    assert_eq!(o.state, CancelState::Cancelled, "{o:?}");
    assert!(
        o.message.contains("removed its VM or container"),
        "{}",
        o.message
    );
    assert!(create.await.unwrap().is_err());
    assert!(!e.rt.running.lock().unwrap().contains_key("connecting"));
    assert_eq!(journals(&e.spaces), 0);
}
