//! `cua sb rm` against stale local records through the daemon runtime (the
//! CLI's path): a record whose VM is gone and an orphan instance with no
//! state file both delete. Fakes only (temp dirs, no host processes).

use async_trait::async_trait;
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalState,
    LocalSummary, MISSING, RuntimeError, RuntimeResult, SandboxState, Status,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Instances by name; `unreachable` names answer with a non-not-found
/// error (an engine that is down).
#[derive(Default)]
struct Runtime {
    instances: Mutex<HashMap<String, InstanceStatus>>,
    unreachable: Vec<String>,
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

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(self
            .instances
            .lock()
            .unwrap()
            .iter()
            .map(|(n, s)| LocalSummary {
                name: n.clone(),
                backend: "qemu".into(),
                status: s.clone(),
            })
            .collect())
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        if self.unreachable.iter().any(|n| n == name) {
            return Err(RuntimeError::Other("engine unreachable".into()));
        }
        self.instances
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.instances
            .lock()
            .unwrap()
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }

    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        Err(RuntimeError::NotFound(name.into()))
    }
}

fn daemon(dir: &std::path::Path, rt: Arc<Runtime>) -> DaemonRuntime {
    DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.join("sandboxes")),
        spaces_home: Some(dir.to_path_buf()),
        local: Some(rt),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn rm_clears_a_record_whose_vm_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let d = daemon(dir.path(), Arc::new(Runtime::default()));
    d.sandboxes()
        .state()
        .save(&SandboxState::Local(LocalState {
            name: "cua-e2e-gone".into(),
            runtime_type: "qemu".into(),
            host: "127.0.0.1".into(),
            api_port: 1,
            status: "running".into(),
            ..Default::default()
        }))
        .unwrap();
    let rec = d.record("cua-e2e-gone").await.unwrap();
    assert_eq!(rec.status, Status::Unknown(MISSING.into()));
    d.delete("cua-e2e-gone").await.unwrap();
    assert!(d.sandboxes().state().load("cua-e2e-gone").is_none());
    assert!(d.delete("cua-e2e-gone").await.is_err());
}

#[tokio::test]
async fn rm_removes_an_orphan_instance_with_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    rt.instances
        .lock()
        .unwrap()
        .insert("cua-e2e-orphan".into(), InstanceStatus::Stopped);
    let d = daemon(dir.path(), rt.clone());
    d.delete("cua-e2e-orphan").await.unwrap();
    assert!(rt.instances.lock().unwrap().is_empty());
    assert!(d.delete("cua-e2e-orphan").await.is_err());
}
