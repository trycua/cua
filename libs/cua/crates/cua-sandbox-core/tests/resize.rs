//! Resizing a local sandbox: the runtime is asked for the new shape, the
//! state file records it, and a runtime without a resize (or a sandbox that
//! is not local) is refused as unsupported, never ignored. Fakes only.

use async_trait::async_trait;
use cua_sandbox_core::{
    Error, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalState,
    LocalSummary, Resize, RuntimeError, RuntimeResult, SandboxState, Sandboxes,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Rt {
    shapes: Mutex<Vec<(String, Resize)>>,
    can_resize: bool,
}

#[async_trait]
impl LocalRuntime for Rt {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::Other(format!("unused: {}", spec.name)))
    }
    async fn stop(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::Other(format!("unused: {name}")))
    }
    async fn resize(&self, name: &str, shape: &Resize) -> RuntimeResult<LocalInstance> {
        if !self.can_resize {
            return Err(RuntimeError::Unsupported {
                backend: "fake".into(),
                op: format!("resize {name}"),
            });
        }
        self.shapes
            .lock()
            .unwrap()
            .push((name.into(), shape.clone()));
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints::default(),
        })
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, _: &str) -> RuntimeResult<InstanceStatus> {
        Ok(InstanceStatus::Running)
    }
    async fn delete(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(LocalEndpoints::default())
    }
}

fn env(can_resize: bool) -> (tempfile::TempDir, Arc<Rt>, Sandboxes) {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Rt {
        can_resize,
        ..Default::default()
    });
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    sbx.state()
        .save(&SandboxState::Local(LocalState {
            name: "vm".into(),
            runtime_type: "qemu".into(),
            host: "127.0.0.1".into(),
            api_port: 1,
            status: "stopped".into(),
            cpu_count: Some(2),
            memory_mb: Some(2048),
            ..Default::default()
        }))
        .unwrap();
    (dir, rt, sbx)
}

#[tokio::test]
async fn a_resize_reaches_the_runtime_and_the_state_file() {
    let (_dir, rt, sbx) = env(true);
    let shape = Resize {
        cpus: Some(4),
        memory_mb: Some(4096),
        disk_gb: Some(30),
        probes: vec![],
    };
    sbx.resize("vm", &shape).await.unwrap();
    assert_eq!(
        rt.shapes.lock().unwrap().as_slice(),
        [("vm".to_string(), shape)]
    );
    let Some(SandboxState::Local(l)) = sbx.state().load("vm") else {
        panic!("state");
    };
    assert_eq!(
        (l.cpu_count, l.memory_mb, l.status.as_str()),
        (Some(4), Some(4096), "running")
    );
}

#[tokio::test]
async fn unset_fields_keep_the_recorded_shape() {
    let (_dir, _rt, sbx) = env(true);
    sbx.resize(
        "vm",
        &Resize {
            memory_mb: Some(8192),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let Some(SandboxState::Local(l)) = sbx.state().load("vm") else {
        panic!("state");
    };
    assert_eq!((l.cpu_count, l.memory_mb), (Some(2), Some(8192)));
}

#[tokio::test]
async fn a_runtime_without_resize_is_refused_and_nothing_changes() {
    let (_dir, _rt, sbx) = env(false);
    let e = sbx
        .resize(
            "vm",
            &Resize {
                cpus: Some(4),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            Error::Runtime(RuntimeError::Unsupported { .. }) | Error::Unsupported { .. }
        ),
        "{e:?}"
    );
    let Some(SandboxState::Local(l)) = sbx.state().load("vm") else {
        panic!("state");
    };
    assert_eq!((l.cpu_count, l.status.as_str()), (Some(2), "stopped"));
}

#[tokio::test]
async fn a_sandbox_that_is_not_local_is_refused() {
    let (_dir, _rt, sbx) = env(true);
    let e = sbx
        .resize(
            "nope",
            &Resize {
                cpus: Some(4),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(e, Error::NotFound(_)), "{e:?}");
}
