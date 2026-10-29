//! An external runtime provider installed in `$CUA_HOME/runtime-providers` is a
//! local runtime like the built-in ones: `--runtime <name>` validates,
//! creates through the provider with the image passed through, persists
//! the provider's name as `runtime_type`, and listing, lookup and delete
//! route back to it. One test per binary: it points `CUA_HOME` at a temp
//! directory for the whole process.
#![cfg(unix)]

use async_trait::async_trait;
use cua_sandbox_core::placement::{self, Kind, On, Runtime};
use cua_sandbox_core::{
    CreateOptions, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec,
    LocalSummary, ProviderKind, RuntimeError, RuntimeResult, Sandboxes, Status,
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

/// Answers from `<verb>.json`; records each call.
const FIXTURE: &str = r#"#!/bin/sh
state='STATE'
verb="$1"
req="$(cat)"
printf '%s %s\n' "$verb" "$req" >> "$state/calls.log"
[ -f "$state/$verb.json" ] && cat "$state/$verb.json"
exit 0
"#;

/// The built-in runtime: nothing of the provider's may reach it.
struct Builtin;

#[async_trait]
impl LocalRuntime for Builtin {
    fn backend(&self) -> String {
        "vmm".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        panic!("the built-in runtime started {}", spec.name)
    }
    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        panic!("the built-in runtime stopped {name}")
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn status_on(&self, name: &str, runtime_type: &str) -> RuntimeResult<InstanceStatus> {
        panic!("the built-in runtime was asked about {name} on {runtime_type}")
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        panic!("the built-in runtime deleted {name}")
    }
    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        Err(RuntimeError::NotFound(name.into()))
    }
}

#[tokio::test]
async fn a_provider_in_the_cua_home_runs_spaces_like_a_builtin_runtime() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary, before any thread reads it.
    unsafe { std::env::set_var("CUA_HOME", home.path()) };
    let state_dir = home.path().join("sandboxes");

    // Nothing installed: the runtime word is unknown locally, and local
    // capabilities are exactly the built-in ones.
    assert_eq!(
        placement::local_capabilities(),
        placement::builtin_local_capabilities()
    );
    let e =
        placement::validate(&On::Local, Kind::Auto, &Runtime::Other("acme".into())).unwrap_err();
    assert!(
        e.message.contains("runtime acme is not available locally"),
        "{e}"
    );
    assert!(!e.valid.iter().any(|v| v == "acme"));

    // The runtimes cua installs itself live under `runtimes/`: never a
    // provider, even an executable named like one.
    let managed = home.path().join("runtimes").join("lume").join("0.6.0");
    std::fs::create_dir_all(&managed).unwrap();
    for dir in [&managed, &home.path().join("runtimes")] {
        let exe = dir.join("cua-runtime-managed1");
        std::fs::write(
            &exe,
            "#!/bin/sh\necho '{\"protocol\":1,\"name\":\"managed1\",\"kind\":\"vm\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(
        cua_sandbox_core::external::Registry::global()
            .providers()
            .is_empty()
    );
    assert!(
        placement::validate(&On::Local, Kind::Auto, &Runtime::Other("managed1".into())).is_err()
    );

    // Install it.
    let runtimes = home.path().join("runtime-providers");
    let state = home.path().join("fixture");
    std::fs::create_dir_all(&runtimes).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let exe = runtimes.join("cua-runtime-acme");
    std::fs::write(&exe, FIXTURE.replace("STATE", &state.display().to_string())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let answer = |verb: &str, v: serde_json::Value| {
        std::fs::write(state.join(format!("{verb}.json")), v.to_string()).unwrap()
    };
    answer(
        "info",
        json!({"protocol": 1, "name": "acme", "display_name": "macOS VMs", "kind": "vm",
               "os": ["macos"], "handles_images": true}),
    );
    answer(
        "start",
        json!({"name": "ext1", "status": "running",
               "endpoints": {"host": "127.0.0.1", "ports": {"3211": 50001}},
               "image": {"os": "macos", "spacesd": false}}),
    );
    answer(
        "list",
        json!([{"name": "ext1", "backend": "acme", "status": "running"}]),
    );
    answer("status", json!({"status": "running"}));

    // Picked up without a restart; it presents as a VM.
    assert_eq!(
        placement::validate(&On::Local, Kind::Auto, &Runtime::Other("acme".into())).unwrap(),
        Kind::Vm
    );
    let e = placement::validate(&On::Local, Kind::Container, &Runtime::Other("acme".into()))
        .unwrap_err();
    assert!(e.message.contains("runs vm sandboxes"), "{e}");
    assert_eq!(
        cua_sandbox_core::placement_of_backend("acme"),
        (Kind::Vm, Runtime::Other("acme".into()))
    );

    let sbx = Sandboxes::builder()
        .local(Arc::new(Builtin))
        .state_dir(&state_dir)
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "registry.example/acme/macos:15");
    o.name = Some("ext1".into());
    o.os = "macos".into();
    o.runtime = Runtime::Other("acme".into());
    let sandbox = sbx.create(o).await.unwrap();
    assert_eq!(
        sandbox.placement(),
        (Kind::Vm, Runtime::Other("acme".into()))
    );
    assert_eq!(sandbox.image_info().unwrap().spacesd, Some(false));

    let log = std::fs::read_to_string(state.join("calls.log")).unwrap();
    let start = log.lines().find(|l| l.starts_with("start ")).unwrap();
    let req: serde_json::Value = serde_json::from_str(&start["start ".len()..]).unwrap();
    assert_eq!(req["image"], "registry.example/acme/macos:15");
    assert_eq!(req["runtime"], "acme");

    // Persisted with the provider's name, so it routes back after a restart.
    let fresh = Sandboxes::builder()
        .local(Arc::new(Builtin))
        .state_dir(&state_dir)
        .build();
    let info = fresh.get("ext1").await.unwrap();
    assert_eq!(info.runtime_type, "acme");
    assert_eq!(info.status, Status::Running);
    let listed = fresh.list().await.unwrap();
    assert!(
        listed
            .iter()
            .any(|i| i.name == "ext1" && i.runtime_type == "acme")
    );
    fresh.delete("ext1").await.unwrap();
    assert!(fresh.state().load("ext1").is_none());
    let log = std::fs::read_to_string(state.join("calls.log")).unwrap();
    assert!(log.lines().any(|l| l.starts_with("delete ")), "{log}");
}
