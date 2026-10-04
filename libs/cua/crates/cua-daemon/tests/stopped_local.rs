//! `cua sb info|resume|restart` on a local sandbox whose VM is stopped (a
//! `lume stop` or a host reboot) through the daemon runtime, which both the
//! daemon and the CLI's embedded backend use: info reports the record as
//! stopped, and resume and restart go through the local runtime instead of
//! first connecting (which wants the instance running). Fakes only (temp
//! dirs, no host processes).

use async_trait::async_trait;
use cua_daemon::{Error, Runtime as DaemonRuntime, RuntimeConfig};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalState,
    LocalSummary, MISSING, RuntimeError, RuntimeResult, SandboxState, Status,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Instances by name, and every lifecycle call made.
#[derive(Default)]
struct Runtime {
    instances: Mutex<HashMap<String, InstanceStatus>>,
    calls: Mutex<Vec<String>>,
}

impl Runtime {
    fn with(name: &str, status: InstanceStatus) -> Arc<Self> {
        let rt = Arc::new(Self::default());
        rt.instances.lock().unwrap().insert(name.into(), status);
        rt
    }

    fn set(&self, name: &str, status: InstanceStatus) -> RuntimeResult<()> {
        match self.instances.lock().unwrap().get_mut(name) {
            Some(s) => {
                *s = status;
                Ok(())
            }
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "lume".into()
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::Other(format!("unused: {}", spec.name)))
    }

    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        self.calls.lock().unwrap().push(format!("stop {name}"));
        self.set(name, InstanceStatus::Stopped)
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        self.calls.lock().unwrap().push(format!("resume {name}"));
        self.set(name, InstanceStatus::Running)?;
        Ok(LocalInstance {
            name: name.into(),
            backend: "lume".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(name).await?,
        })
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(self
            .instances
            .lock()
            .unwrap()
            .iter()
            .map(|(n, s)| LocalSummary {
                name: n.clone(),
                backend: "lume".into(),
                status: s.clone(),
            })
            .collect())
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
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

    async fn endpoints(&self, _name: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(LocalEndpoints {
            host: "192.0.2.10".into(),
            ports: [(3211, 3211)].into(),
            vnc: None,
            qmp: None,
            ssh: None,
            serial_log: None,
            container_id: None,
        })
    }
}

fn daemon(dir: &std::path::Path, rt: Arc<Runtime>, name: &str) -> DaemonRuntime {
    let d = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.join("sandboxes")),
        spaces_home: Some(dir.to_path_buf()),
        local: Some(rt),
        ..Default::default()
    })
    .unwrap();
    d.sandboxes()
        .state()
        .save(&SandboxState::Local(LocalState {
            name: name.into(),
            runtime_type: "lume".into(),
            host: "192.0.2.10".into(),
            api_port: 3211,
            status: "running".into(),
            ..Default::default()
        }))
        .unwrap();
    d
}

fn recorded_status(d: &DaemonRuntime, name: &str) -> String {
    d.sandboxes()
        .state()
        .load(name)
        .unwrap()
        .status()
        .to_string()
}

#[tokio::test]
async fn info_reports_a_stopped_vm_from_its_record() {
    let dir = tempfile::tempdir().unwrap();
    let name = "cua-e2e-stopped-info";
    let d = daemon(
        dir.path(),
        Runtime::with(name, InstanceStatus::Stopped),
        name,
    );
    let rec = d.record(name).await.unwrap();
    assert_eq!(rec.status, Status::Stopped);
    assert_eq!(rec.id, format!("local:{name}"));
    assert_eq!(rec.runtime_type, "lume");
}

#[tokio::test]
async fn resume_boots_a_stopped_vm_and_records_it_running() {
    let dir = tempfile::tempdir().unwrap();
    let name = "cua-e2e-stopped-resume";
    let rt = Runtime::with(name, InstanceStatus::Stopped);
    let d = daemon(dir.path(), rt.clone(), name);
    d.sandboxes().state().set_status(name, "stopped").unwrap();

    d.resume(name).await.unwrap();
    assert_eq!(rt.calls(), [format!("resume {name}")]);
    assert_eq!(recorded_status(&d, name), "running");
    // Running again: a plain connect works.
    let rec = d.connect(name).await.unwrap();
    assert_eq!(rec.status, Status::Running);
}

#[tokio::test]
async fn restart_works_on_a_stopped_and_on_a_running_vm() {
    let dir = tempfile::tempdir().unwrap();
    let name = "cua-e2e-stopped-restart";
    let rt = Runtime::with(name, InstanceStatus::Stopped);
    let d = daemon(dir.path(), rt.clone(), name);

    d.restart(name).await.unwrap();
    assert_eq!(
        rt.calls(),
        [format!("stop {name}"), format!("resume {name}")]
    );
    assert_eq!(rt.status(name).await.unwrap(), InstanceStatus::Running);

    // Now running (and connected): restart through the handle.
    d.connect(name).await.unwrap();
    d.restart(name).await.unwrap();
    assert_eq!(
        rt.calls(),
        [
            format!("stop {name}"),
            format!("resume {name}"),
            format!("stop {name}"),
            format!("resume {name}"),
        ]
    );
    assert_eq!(recorded_status(&d, name), "running");
}

#[tokio::test]
async fn resume_of_a_record_whose_vm_is_gone_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let name = "cua-e2e-stopped-gone";
    let rt = Arc::new(Runtime::default());
    let d = daemon(dir.path(), rt.clone(), name);
    assert_eq!(
        d.record(name).await.unwrap().status,
        Status::Unknown(MISSING.into())
    );
    let err = d.resume(name).await.unwrap_err();
    assert!(
        matches!(&err, Error::Runtime(m) if m == &format!("instance {name} not found")),
        "{err:?}"
    );
    assert!(rt.calls().is_empty());
}
