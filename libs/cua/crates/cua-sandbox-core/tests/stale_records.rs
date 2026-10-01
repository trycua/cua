//! Stale local records: a state file whose VM or container is gone is
//! listed as `missing` and `delete` clears it; a local instance with no
//! state file (an orphan) still deletes. Fakes only (temp state dir, no
//! host processes).

use async_trait::async_trait;
use cua_sandbox_core::{
    Error, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalState,
    LocalSummary, MISSING, RuntimeError, RuntimeResult, SandboxState, Sandboxes, Status,
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

fn record(name: &str) -> SandboxState {
    SandboxState::Local(LocalState {
        name: name.into(),
        runtime_type: "qemu".into(),
        host: "127.0.0.1".into(),
        api_port: 1,
        status: "running".into(),
        ..Default::default()
    })
}

fn status_of(list: &[cua_sandbox_core::SandboxInfo], name: &str) -> Status {
    list.iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("{name} not listed"))
        .status
        .clone()
}

#[tokio::test]
async fn a_record_whose_vm_is_gone_is_missing_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    rt.instances
        .lock()
        .unwrap()
        .insert("cua-e2e-live".into(), InstanceStatus::Running);
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    sbx.state().save(&record("cua-e2e-gone")).unwrap();
    sbx.state().save(&record("cua-e2e-live")).unwrap();

    let list = sbx.list().await.unwrap();
    assert_eq!(
        status_of(&list, "cua-e2e-gone"),
        Status::Unknown(MISSING.into())
    );
    assert_eq!(status_of(&list, "cua-e2e-live"), Status::Running);
    assert_eq!(
        sbx.get("cua-e2e-gone").await.unwrap().status,
        Status::Unknown(MISSING.into())
    );

    // The backing VM is gone: the delete still succeeds and drops the
    // record; a second delete has nothing left to remove.
    sbx.delete("cua-e2e-gone").await.unwrap();
    assert!(sbx.state().load("cua-e2e-gone").is_none());
    assert!(matches!(
        sbx.delete("cua-e2e-gone").await,
        Err(Error::NotFound(_))
    ));
    // The live sandbox is untouched.
    assert!(sbx.state().load("cua-e2e-live").is_some());
    assert!(rt.instances.lock().unwrap().contains_key("cua-e2e-live"));
}

#[tokio::test]
async fn an_unreachable_engine_is_not_reported_missing() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime {
        unreachable: vec!["cua-e2e-maybe".into()],
        ..Default::default()
    });
    let sbx = Sandboxes::builder().local(rt).state_dir(dir.path()).build();
    sbx.state().save(&record("cua-e2e-maybe")).unwrap();
    let list = sbx.list().await.unwrap();
    assert_eq!(status_of(&list, "cua-e2e-maybe"), Status::Running);
}

#[tokio::test]
async fn an_orphan_instance_without_a_record_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    rt.instances
        .lock()
        .unwrap()
        .insert("cua-e2e-orphan".into(), InstanceStatus::Stopped);
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    assert_eq!(
        status_of(&sbx.list().await.unwrap(), "cua-e2e-orphan"),
        Status::Stopped
    );
    sbx.delete("cua-e2e-orphan").await.unwrap();
    assert!(rt.instances.lock().unwrap().is_empty());
    assert!(matches!(
        sbx.delete("cua-e2e-orphan").await,
        Err(Error::NotFound(_))
    ));
}
