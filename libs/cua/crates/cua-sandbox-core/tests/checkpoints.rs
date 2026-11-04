//! Checkpoint records: pins (round trip, delete refusal, prune), the
//! shared-memory flag the runtime reports, and forkable creates (copied
//! into the start spec, never a warm-pool claim). Fakes only (temp state
//! dir, no host processes).

use async_trait::async_trait;
use cua_sandbox_core::{
    CreateOptions, Error, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime,
    LocalStartSpec, LocalState, LocalSummary, ProviderKind, RuntimeError, RuntimeResult,
    SandboxState, Sandboxes,
};
use std::sync::{Arc, Mutex};

/// A runtime whose checkpoints share memory (`shared`), that logs pin and
/// delete calls, and that fails to delete the checkpoints in `undeletable`.
#[derive(Default)]
struct Runtime {
    shared: bool,
    undeletable: Vec<String>,
    /// Checkpoints the engine has lost: pin and delete say not found.
    vanished: Vec<String>,
    /// Instances `list` reports (a QEMU checkpoint is listed as a stopped one).
    listed: Vec<String>,
    /// Instances other than `bake` do not exist (so fork names are free).
    only_bake: bool,
    calls: Mutex<Vec<String>>,
}

impl Runtime {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "qemu".into()
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::Other(format!("unused: {}", spec.name)))
    }

    async fn stop(&self, _name: &str) -> RuntimeResult<()> {
        Ok(())
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::NotFound(name.into()))
    }

    async fn checkpoint(&self, name: &str, checkpoint: &str) -> RuntimeResult<()> {
        self.log(format!("checkpoint {name} {checkpoint}"));
        Ok(())
    }

    async fn checkpoint_shared(&self, name: &str, checkpoint: &str) -> RuntimeResult<bool> {
        self.checkpoint(name, checkpoint).await?;
        Ok(self.shared)
    }

    async fn pin_checkpoint(
        &self,
        runtime_type: &str,
        checkpoint: &str,
        pinned: bool,
    ) -> RuntimeResult<()> {
        self.log(format!("pin {runtime_type} {checkpoint} {pinned}"));
        if self.vanished.iter().any(|n| n == checkpoint) {
            return Err(RuntimeError::NotFound(checkpoint.into()));
        }
        Ok(())
    }

    async fn delete_checkpoint(&self, runtime_type: &str, checkpoint: &str) -> RuntimeResult<()> {
        self.log(format!("delete {runtime_type} {checkpoint}"));
        if self.vanished.iter().any(|n| n == checkpoint) {
            return Err(RuntimeError::NotFound(checkpoint.into()));
        }
        if self.undeletable.iter().any(|n| n == checkpoint) {
            return Err(RuntimeError::Other(format!("{checkpoint} is busy")));
        }
        Ok(())
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(self
            .listed
            .iter()
            .map(|n| LocalSummary {
                name: n.clone(),
                backend: "qemu".into(),
                status: InstanceStatus::Stopped,
            })
            .collect())
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        if self.only_bake && name != "bake" {
            return Err(RuntimeError::NotFound(name.into()));
        }
        Ok(InstanceStatus::Running)
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.log(format!("delete-instance {name}"));
        Ok(())
    }

    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        Err(RuntimeError::NotFound(name.into()))
    }
}

fn manager(rt: Arc<Runtime>, dir: &tempfile::TempDir) -> Sandboxes {
    let sbx = Sandboxes::builder().local(rt).state_dir(dir.path()).build();
    sbx.state()
        .save(&SandboxState::Local(LocalState {
            name: "bake".into(),
            runtime_type: "qemu".into(),
            host: "127.0.0.1".into(),
            api_port: 1,
            status: "running".into(),
            ..Default::default()
        }))
        .unwrap();
    sbx
}

#[tokio::test]
async fn a_checkpoint_records_whether_its_forks_share_memory() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime {
        shared: true,
        ..Default::default()
    });
    let sbx = manager(rt.clone(), &dir);
    let rec = sbx.checkpoint("bake", "world").await.unwrap();
    assert!(rec.shared_memory);
    assert!(!rec.pinned);
    assert_eq!(rt.calls(), ["checkpoint bake world"]);
    assert_eq!(sbx.checkpoint_record("world").unwrap(), rec);

    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    let sbx = manager(rt, &dir);
    assert!(!sbx.checkpoint("bake", "world").await.unwrap().shared_memory);
}

#[tokio::test]
async fn a_pinned_checkpoint_round_trips_and_is_pinned_in_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    let sbx = manager(rt.clone(), &dir);
    let rec = sbx.checkpoint_with("bake", "world", true).await.unwrap();
    assert!(rec.pinned);
    assert!(sbx.checkpoint_record("world").unwrap().pinned);
    assert_eq!(rt.calls(), ["checkpoint bake world", "pin qemu world true"]);

    let rec = sbx.pin_checkpoint("world", false).await.unwrap();
    assert!(!rec.pinned);
    assert!(!sbx.checkpoint_record("world").unwrap().pinned);
    let rec = sbx.pin_checkpoint("world", true).await.unwrap();
    assert!(rec.pinned);
    assert!(sbx.checkpoints()[0].pinned);
    assert_eq!(
        rt.calls()[2..],
        ["pin qemu world false", "pin qemu world true"]
    );
    assert!(matches!(
        sbx.pin_checkpoint("nope", true).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn delete_refuses_a_pinned_checkpoint_until_it_is_unpinned() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    let sbx = manager(rt.clone(), &dir);
    sbx.checkpoint_with("bake", "world", true).await.unwrap();
    match sbx.delete_checkpoint("world").await {
        Err(Error::InvalidArgument(m)) => assert!(
            m.contains("checkpoint world is pinned; unpin it first"),
            "{m}"
        ),
        other => panic!("expected a pinned refusal, got {other:?}"),
    }
    assert!(sbx.checkpoint_record("world").is_some());
    assert!(!rt.calls().iter().any(|c| c.starts_with("delete")));

    sbx.pin_checkpoint("world", false).await.unwrap();
    sbx.delete_checkpoint("world").await.unwrap();
    assert!(sbx.checkpoint_record("world").is_none());
    assert_eq!(rt.calls().last().unwrap(), "delete qemu world");
}

#[tokio::test]
async fn prune_deletes_every_unpinned_checkpoint_and_continues_past_failures() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime {
        undeletable: vec!["busy".into()],
        ..Default::default()
    });
    let sbx = manager(rt.clone(), &dir);
    sbx.checkpoint("bake", "a").await.unwrap();
    sbx.checkpoint_with("bake", "keep", true).await.unwrap();
    sbx.checkpoint("bake", "busy").await.unwrap();
    sbx.checkpoint("bake", "b").await.unwrap();

    let mut deleted = sbx.prune_checkpoints().await.unwrap();
    deleted.sort();
    assert_eq!(deleted, ["a", "b"]);
    let mut left: Vec<String> = sbx.checkpoints().into_iter().map(|c| c.name).collect();
    left.sort();
    assert_eq!(left, ["busy", "keep"]);
    assert!(!rt.calls().iter().any(|c| c == "delete qemu keep"));
}

#[test]
fn a_record_from_before_pins_reads_as_unpinned_and_unshared() {
    let rec: cua_sandbox_core::CheckpointRecord = serde_json::from_value(serde_json::json!({
        "name": "old",
        "source": "bake",
        "runtime_type": "qemu",
        "created_at": "2026-01-01T00:00:00Z",
        "template": serde_json::to_value(LocalState::default()).unwrap(),
    }))
    .unwrap();
    assert!(!rec.pinned);
    assert!(!rec.shared_memory);
}

#[test]
fn a_forkable_create_starts_cold_with_a_forkable_spec() {
    let mut o = CreateOptions::new(ProviderKind::Local, "ghcr.io/trycua/linux:24.04").name("bake");
    let plain = Sandboxes::local_start_spec(&o, "bake", None);
    assert!(!plain.forkable);
    assert_eq!(plain.warm, None);
    // Not sent to an external runtime unless set.
    assert!(
        serde_json::to_value(&plain)
            .unwrap()
            .get("forkable")
            .is_none()
    );

    o.forkable = true;
    let spec = Sandboxes::local_start_spec(&o, "bake", None);
    assert!(spec.forkable);
    assert_eq!(
        spec.warm,
        Some(false),
        "a fork source never claims a pool member"
    );
    assert_eq!(serde_json::to_value(&spec).unwrap()["forkable"], true);
}

/// A pinned checkpoint whose engine instance is gone (its files were removed)
/// can be unpinned and deleted: the stale record must not stick forever.
#[tokio::test]
async fn a_pinned_checkpoint_the_engine_lost_can_be_unpinned_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime {
        vanished: vec!["world".into()],
        ..Default::default()
    });
    let sbx = manager(rt.clone(), &dir);
    // Recorded while the engine still had it, then lost.
    sbx.checkpoint_with("bake", "world", false).await.unwrap();
    sbx.pin_checkpoint("world", true).await.unwrap_err();
    assert!(!sbx.checkpoint_record("world").unwrap().pinned);
    // Pin it in the record as it was before the engine lost it.
    let path = dir.path().join(".checkpoints").join("world.json");
    let mut rec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    rec["pinned"] = true.into();
    std::fs::write(&path, serde_json::to_vec(&rec).unwrap()).unwrap();
    assert!(sbx.checkpoint_record("world").unwrap().pinned);

    let rec = sbx.pin_checkpoint("world", false).await.unwrap();
    assert!(!rec.pinned);
    sbx.delete_checkpoint("world").await.unwrap();
    assert!(sbx.checkpoint_record("world").is_none());
}

#[tokio::test]
async fn a_checkpoint_is_not_a_sandbox_to_list_or_delete() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime {
        listed: vec!["bake".into(), "world".into()],
        ..Default::default()
    });
    let sbx = manager(rt.clone(), &dir);
    sbx.checkpoint_with("bake", "world", true).await.unwrap();

    // The runtime lists the checkpoint as a stopped entry; the sandbox list
    // (what a client that predates checkpoints reads) leaves it out.
    let names: Vec<String> = sbx
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.name)
        .collect();
    assert_eq!(names, ["bake"]);

    // An old client deletes it as a sandbox: refused, with the way forward.
    match sbx.delete("world").await {
        Err(Error::InvalidArgument(m)) => {
            assert!(m.contains("world is a checkpoint"), "{m}");
            assert!(m.contains("cua sb checkpoints rm world"), "{m}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(sbx.checkpoint_record("world").is_some());
    assert!(!rt.calls().iter().any(|c| c.starts_with("delete-instance")));

    // A real sandbox still deletes.
    sbx.delete("bake").await.unwrap();
    assert!(rt.calls().iter().any(|c| c == "delete-instance bake"));
}

/// Forking a running sandbox many times goes through one implied
/// checkpoint that its forks hold: it stays while any fork exists, prune
/// keeps it, and it is deleted with the last fork.
#[tokio::test]
async fn an_implied_checkpoint_is_held_by_its_forks_and_goes_with_the_last() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime {
        shared: true,
        ..Default::default()
    });
    let sbx = manager(rt.clone(), &dir);
    let rec = sbx.checkpoint_for_forks("bake").await.unwrap();
    assert!(rec.release_with_forks);
    assert!(rec.name.starts_with("bake-forks-"), "{}", rec.name);
    assert!(rec.holders.is_empty());

    // Two forks hold it (as `Sandboxes::fork` records them).
    let path = dir
        .path()
        .join(cua_sandbox_core::CHECKPOINT_DIR)
        .join(format!("{}.json", rec.name));
    let mut held = rec.clone();
    held.holders = ["f1".to_string(), "f2".to_string()].into_iter().collect();
    std::fs::write(&path, serde_json::to_vec(&held).unwrap()).unwrap();

    // Prune leaves a held checkpoint.
    assert!(sbx.prune_checkpoints().await.unwrap().is_empty());
    sbx.release_holds("f1").await;
    let now = sbx.checkpoint_record(&rec.name).unwrap();
    assert_eq!(now.holders.len(), 1);
    assert!(!rt.calls().iter().any(|c| c.starts_with("delete ")));
    // The last fork goes: so does the checkpoint, in the engine and record.
    sbx.release_holds("f2").await;
    assert!(sbx.checkpoint_record(&rec.name).is_none());
    assert!(
        rt.calls().contains(&format!("delete qemu {}", rec.name)),
        "{:?}",
        rt.calls()
    );
}

/// A fork that fails to start does not keep an implied checkpoint, and a
/// batch whose forks all failed leaves none behind.
#[tokio::test]
async fn failed_forks_release_their_hold() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime {
        only_bake: true,
        ..Default::default()
    });
    let sbx = manager(rt.clone(), &dir);
    let rec = sbx.checkpoint_for_forks("bake").await.unwrap();
    // The fake runtime cannot start forks.
    let err = sbx.fork(&rec.name, "f1").await.unwrap_err();
    assert!(err.to_string().contains("fork"), "{err}");
    // The failed fork's hold went, and with no holder left the checkpoint
    // went with it.
    assert!(
        sbx.checkpoint_record(&rec.name).is_none(),
        "{:?}",
        rt.calls()
    );

    // An explicit checkpoint is never released by its forks.
    sbx.checkpoint("bake", "world").await.unwrap();
    assert!(sbx.fork("world", "f2").await.is_err());
    assert!(sbx.checkpoint_record("world").is_some());
    assert!(!sbx.release_if_unheld("world").await);
}

#[test]
fn records_without_holds_read_as_explicit_checkpoints() {
    let rec: cua_sandbox_core::CheckpointRecord = serde_json::from_value(serde_json::json!({
        "name": "old",
        "source": "bake",
        "runtime_type": "qemu",
        "created_at": "2026-01-01T00:00:00Z",
        "template": serde_json::to_value(LocalState::default()).unwrap(),
    }))
    .unwrap();
    assert!(!rec.release_with_forks);
    assert!(rec.holders.is_empty());
    // And serializing an explicit one adds no new keys (old readers).
    let v = serde_json::to_value(&rec).unwrap();
    assert!(v.get("release_with_forks").is_none() && v.get("holders").is_none());
}
