//! `cua spaces delete <name>` deletes a sandbox Cua made on this machine
//! that no Space record names (a `cua sb fork`, a `cua sb create`), and
//! never a VM the local runtime has that Cua did not make.

use async_trait::async_trait;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::Spaces;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Runtime {
    vms: Mutex<BTreeSet<String>>,
    deleted: Mutex<Vec<String>>,
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        Err(RuntimeError::NotFound(spec.name.clone()))
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
        if self.vms.lock().unwrap().contains(name) {
            Ok(InstanceStatus::Running)
        } else {
            Err(RuntimeError::NotFound(name.into()))
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.deleted.lock().unwrap().push(name.into());
        if self.vms.lock().unwrap().remove(name) {
            Ok(())
        } else {
            Err(RuntimeError::NotFound(name.into()))
        }
    }
    async fn endpoints(&self, _name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(LocalEndpoints::default())
    }
}

/// The state record a local sandbox create or fork leaves.
fn record(dir: &std::path::Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let state = serde_json::json!({
        "name": name, "runtime_type": "fake", "image": {}, "host": "127.0.0.1",
        "api_port": 9, "status": "running", "created_at": "2026-10-27T00:00:00+00:00",
    });
    std::fs::write(dir.join(format!("{name}.json")), state.to_string()).unwrap();
}

#[tokio::test]
async fn a_forked_sandbox_deletes_by_name_but_a_foreign_vm_does_not() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    rt.vms
        .lock()
        .unwrap()
        .extend(["forked".to_string(), "someone-elses-vm".to_string()]);
    record(state.path(), "forked");
    let spaces = Spaces::builder()
        .home(home.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();

    let msg = spaces.delete("forked").await.unwrap();
    assert!(msg.contains("local:forked"), "{msg}");
    assert!(rt.deleted.lock().unwrap().contains(&"forked".to_string()));

    let err = spaces.delete("someone-elses-vm").await.unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
    assert!(
        !rt.deleted
            .lock()
            .unwrap()
            .contains(&"someone-elses-vm".to_string())
    );
    assert!(rt.vms.lock().unwrap().contains("someone-elses-vm"));
}
