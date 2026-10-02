//! Turning sandboxes off and on: each provider's power control (suspend,
//! stop, none), the state it leaves behind, and turning on a suspended, a
//! stopped and a running one. Fakes only (temp state dir, an in-memory
//! runtime and provider, no host processes).

use async_trait::async_trait;
use cua_sandbox_core::{
    Error, FleetState, ImageMode, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime,
    LocalStartSpec, LocalState, LocalSummary, PortExposure, PowerControl, PowerState, Provider,
    ProviderCapabilities, ProviderCreate, ProviderInstance, ProviderKind, Result, RunKind,
    RuntimeError, RuntimeResult, SandboxState, Sandboxes, ServiceEndpoint, Status,
};
use serde_json::json;
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
    fn set(&self, name: &str, status: InstanceStatus) {
        self.instances.lock().unwrap().insert(name.into(), status);
    }

    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }

    fn call(&self, op: &str, name: &str) {
        self.calls.lock().unwrap().push(format!("{op} {name}"));
    }
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "fake".into()
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::Other(format!("unused: {}", spec.name)))
    }

    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        self.call("stop", name);
        self.set(name, InstanceStatus::Stopped);
        Ok(())
    }

    async fn suspend(&self, name: &str) -> RuntimeResult<()> {
        self.call("suspend", name);
        self.set(name, InstanceStatus::Paused);
        Ok(())
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        self.call("resume", name);
        self.set(name, InstanceStatus::Running);
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints::default(),
        })
    }

    fn power_control(&self, runtime_type: &str) -> Option<PowerControl> {
        match runtime_type {
            "runc" => Some(PowerControl::Suspend),
            "lume" => Some(PowerControl::Stop),
            _ => None,
        }
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
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
        self.instances.lock().unwrap().remove(name);
        Ok(())
    }

    async fn endpoints(&self, _name: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(LocalEndpoints::default())
    }
}

fn local(name: &str, runtime_type: &str) -> SandboxState {
    SandboxState::Local(LocalState {
        name: name.into(),
        runtime_type: runtime_type.into(),
        host: "127.0.0.1".into(),
        api_port: 1,
        status: "running".into(),
        ..Default::default()
    })
}

fn env() -> (tempfile::TempDir, Arc<Runtime>, Sandboxes) {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    (dir, rt, sbx)
}

#[tokio::test]
async fn a_container_suspends_and_resumes() {
    let (_dir, rt, sbx) = env();
    sbx.state().save(&local("box", "runc")).unwrap();
    rt.set("box", InstanceStatus::Running);
    assert_eq!(sbx.power_control("box"), Some(PowerControl::Suspend));
    assert_eq!(sbx.power_state("box"), Some(PowerState::Running));

    assert_eq!(sbx.power_off("box").await.unwrap(), PowerState::Suspended);
    assert_eq!(rt.calls(), ["suspend box"]);
    assert_eq!(sbx.power_state("box"), Some(PowerState::Suspended));

    sbx.power_on("box").await.unwrap();
    assert_eq!(rt.calls(), ["resume box"]);
    assert_eq!(sbx.power_state("box"), Some(PowerState::Running));
}

#[tokio::test]
async fn a_vm_without_a_pause_stops_and_boots_again() {
    let (_dir, rt, sbx) = env();
    sbx.state().save(&local("mac", "lume")).unwrap();
    rt.set("mac", InstanceStatus::Running);
    assert_eq!(sbx.power_control("mac"), Some(PowerControl::Stop));

    assert_eq!(sbx.power_off("mac").await.unwrap(), PowerState::Stopped);
    assert_eq!(rt.calls(), ["stop mac"]);
    assert_eq!(sbx.power_state("mac"), Some(PowerState::Stopped));

    sbx.power_on("mac").await.unwrap();
    assert_eq!(rt.calls(), ["resume mac"]);
    assert_eq!(sbx.power_state("mac"), Some(PowerState::Running));
}

#[tokio::test]
async fn turning_on_a_running_sandbox_only_records_it() {
    let (_dir, rt, sbx) = env();
    // The record says suspended; the runtime says it runs (resumed
    // elsewhere): nothing is called, and the record catches up.
    let mut s = local("box", "runc");
    if let SandboxState::Local(l) = &mut s {
        l.status = "suspended".into();
    }
    sbx.state().save(&s).unwrap();
    rt.set("box", InstanceStatus::Running);
    sbx.power_on("box").await.unwrap();
    assert!(rt.calls().is_empty());
    assert_eq!(sbx.power_state("box"), Some(PowerState::Running));
}

#[tokio::test]
async fn sandboxes_without_a_power_control_refuse_by_name() {
    let (_dir, rt, sbx) = env();
    sbx.state().save(&local("odd", "qemu-docker")).unwrap();
    rt.set("odd", InstanceStatus::Running);
    assert_eq!(sbx.power_control("odd"), None);
    match sbx.power_off("odd").await {
        Err(Error::Unsupported { provider, op }) => {
            assert_eq!(provider, ProviderKind::Local);
            assert_eq!(
                op,
                "turning odd off and on (qemu-docker cannot suspend or stop it)"
            );
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
    assert!(rt.calls().is_empty());

    // A cloud claim has no per-claim power.
    sbx.state()
        .save(&SandboxState::Fleet(FleetState {
            name: "claim".into(),
            runtime_type: "fleet".into(),
            pool_name: "pool".into(),
            status: "running".into(),
            created_at: String::new(),
            extra: Default::default(),
        }))
        .unwrap();
    assert_eq!(sbx.power_control("claim"), None);
    assert!(matches!(
        sbx.power_on("claim").await,
        Err(Error::Unsupported {
            provider: ProviderKind::Fleet,
            ..
        })
    ));
    // No such sandbox.
    assert_eq!(sbx.power_control("nope"), None);
    assert!(matches!(
        sbx.power_off("nope").await,
        Err(Error::NotFound(_))
    ));
}

/// A provider whose instances stop and start (a cloud VM platform).
#[derive(Default)]
struct StopStart {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl Provider for StopStart {
    fn name(&self) -> &'static str {
        "e2b"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            kinds: vec![RunKind::Vm],
            runtime: "fake",
            arches: vec!["amd64"],
            image_mode: ImageMode::Direct,
            ports: PortExposure::None,
            command: false,
            env_to_entrypoint: false,
            private_registry: false,
            suspend: false,
            max_cpus: None,
            max_memory_mb: None,
            credential_env: &[],
            gpus: vec![],
        }
    }

    fn check_configured(&self) -> Result<()> {
        Ok(())
    }

    async fn create(&self, spec: &ProviderCreate) -> Result<ProviderInstance> {
        Err(Error::NotFound(spec.name.clone()))
    }

    async fn get(&self, id: &str) -> Result<ProviderInstance> {
        Ok(ProviderInstance::new(id, id, Status::Running))
    }

    async fn list(&self) -> Result<Vec<ProviderInstance>> {
        Ok(vec![])
    }

    async fn delete(&self, _id: &str) -> Result<()> {
        Ok(())
    }

    fn endpoint(&self, _: &ProviderInstance, _port: u16) -> Result<ServiceEndpoint> {
        Err(Error::NotFound("no ports".into()))
    }

    fn power(&self) -> Option<PowerControl> {
        Some(PowerControl::Stop)
    }

    async fn stop(&self, id: &str) -> Result<()> {
        self.calls.lock().unwrap().push(format!("stop {id}"));
        Ok(())
    }

    async fn start(&self, id: &str) -> Result<ProviderInstance> {
        self.calls.lock().unwrap().push(format!("start {id}"));
        Ok(ProviderInstance::new(id, id, Status::Running))
    }
}

#[tokio::test]
async fn a_provider_reports_its_own_power_control() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(StopStart::default());
    let sbx = Sandboxes::builder()
        .provider(provider.clone())
        .state_dir(dir.path())
        .build();
    let mut s = local("vm", "e2b");
    if let SandboxState::Local(l) = &mut s {
        l.extra.insert("contrib_provider".into(), json!("e2b"));
        l.extra.insert("contrib_id".into(), json!("i-123"));
    }
    sbx.state().save(&s).unwrap();
    assert_eq!(sbx.power_control("vm"), Some(PowerControl::Stop));
    assert_eq!(sbx.power_off("vm").await.unwrap(), PowerState::Stopped);
    assert_eq!(sbx.power_state("vm"), Some(PowerState::Stopped));
    sbx.power_on("vm").await.unwrap();
    assert_eq!(sbx.power_state("vm"), Some(PowerState::Running));
    assert_eq!(
        *provider.calls.lock().unwrap(),
        ["stop i-123", "start i-123"]
    );
}
