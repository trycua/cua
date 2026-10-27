//! `DeleteSpaces` through the daemon runtime: a running Space is turned off
//! before it is deleted, and one Space that fails does not stop the rest.
//! Fakes only (temp dirs, no host processes).

use async_trait::async_trait;
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalState,
    LocalSummary, PowerControl, RuntimeError, RuntimeResult, SandboxState,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Instances by name, and every stop and delete in order.
#[derive(Default)]
struct Runtime {
    instances: Mutex<HashMap<String, InstanceStatus>>,
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "qemu".into()
    }

    fn power_control(&self, _runtime_type: &str) -> Option<PowerControl> {
        Some(PowerControl::Stop)
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::Other(format!("unused: {}", spec.name)))
    }

    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        self.calls.lock().unwrap().push(format!("stop {name}"));
        if let Some(s) = self.instances.lock().unwrap().get_mut(name) {
            *s = InstanceStatus::Stopped;
        }
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
        self.instances
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.calls.lock().unwrap().push(format!("delete {name}"));
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

#[tokio::test]
async fn running_spaces_are_turned_off_first_and_a_failure_does_not_stop_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    let d = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().to_path_buf()),
        local: Some(rt.clone()),
        ..Default::default()
    })
    .unwrap();
    for (name, status) in [("ci-runner", "running"), ("old-box", "stopped")] {
        rt.instances.lock().unwrap().insert(
            name.into(),
            if status == "running" {
                InstanceStatus::Running
            } else {
                InstanceStatus::Stopped
            },
        );
        d.sandboxes()
            .state()
            .save(&SandboxState::Local(LocalState {
                name: name.into(),
                runtime_type: "qemu".into(),
                host: "127.0.0.1".into(),
                api_port: 1,
                status: status.into(),
                ..Default::default()
            }))
            .unwrap();
        d.spaces()
            .registry()
            .upsert(
                cua_proto::daemon::v1::Space {
                    id: format!("local:{name}"),
                    name: name.into(),
                    ..Default::default()
                },
                Default::default(),
            )
            .unwrap();
    }
    let ids = vec![
        "local:ci-runner".to_string(),
        "no-such-space".to_string(),
        "local:old-box".to_string(),
    ];
    let results = d.spaces().delete_many(&ids).await;
    assert_eq!(
        results
            .iter()
            .map(|r| (r.id.as_str(), r.ok))
            .collect::<Vec<_>>(),
        vec![
            ("local:ci-runner", true),
            ("no-such-space", false),
            ("local:old-box", true),
        ]
    );
    let calls = rt.calls.lock().unwrap().clone();
    let stop = calls.iter().position(|c| c == "stop ci-runner");
    let delete = calls.iter().position(|c| c == "delete ci-runner");
    assert!(
        matches!((stop, delete), (Some(s), Some(d)) if s < d),
        "the running Space is turned off before it is deleted: {calls:?}"
    );
    // A stopped one is not turned off again.
    assert!(!calls.contains(&"stop old-box".to_string()), "{calls:?}");
    assert!(calls.contains(&"delete old-box".to_string()), "{calls:?}");
    assert!(d.spaces().list().unwrap().is_empty());
}
