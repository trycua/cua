//! Resuming a stopped Lume VM (what `lume stop` or a host reboot leaves)
//! boots it again the way `start` boots an existing VM: a macOS guest gets
//! its setup share back (the env token its cua-spacesd starts with), and the
//! published ports the first start recorded. Hermetic: a fake `lume serve`
//! records the API calls.

#![cfg(feature = "lume")]

#[allow(dead_code)]
#[path = "support/fake_lume.rs"]
mod fake_lume;

use std::sync::{Arc, Mutex};

use cua_vmm::lume::{LumeRuntime, MACOS_SETUP_TOKEN_FILE};
use cua_vmm::{Runtime, Status, VmmError};
use fake_lume::{FakeLume, serve, temp_home};
use serde_json::json;

/// A fake with one macOS VM `name` in `status`.
async fn fake_with(name: &str, status: &str) -> (LumeRuntime, Arc<Mutex<FakeLume>>) {
    let mut st = FakeLume::default();
    st.vms.insert(
        name.into(),
        json!({
            "name": name, "os": "macOS", "cpuCount": 4, "memorySize": 4u64 << 30,
            "diskSize": { "allocated": 1u64 << 30, "total": 50u64 << 30 },
            "display": "1024x768", "status": status,
            "ipAddress": if status == "running" { json!("192.0.2.10") } else { json!(null) },
        }),
    );
    let state = Arc::new(Mutex::new(st));
    let url = serve(state.clone()).await;
    (fake_lume::runtime(url), state)
}

/// What an earlier `start` left under the runtime root: the setup share
/// with the env token, and the published ports.
fn earlier_start(name: &str) -> std::path::PathBuf {
    let dir = temp_home().join("lume-root").join(name);
    let setup = dir.join("setup");
    std::fs::create_dir_all(&setup).unwrap();
    std::fs::write(setup.join(MACOS_SETUP_TOKEN_FILE), "tok-1").unwrap();
    std::fs::write(dir.join("ports.json"), "[3211]").unwrap();
    setup
}

fn mutating_calls(st: &FakeLume) -> Vec<String> {
    st.calls
        .iter()
        .filter(|(m, _, _)| m != "GET")
        .map(|(m, p, _)| format!("{m} {p}"))
        .collect()
}

#[tokio::test]
async fn resume_boots_a_stopped_macos_vm_with_its_setup_share() {
    let name = "cua-e2e-resume-stopped";
    let setup = earlier_start(name);
    let (rt, state) = fake_with(name, "stopped").await;

    let inst = rt.resume(name).await.unwrap();
    assert_eq!(inst.status, Status::Running);
    assert_eq!(inst.endpoints.host, "192.0.2.10");
    assert_eq!(
        inst.endpoints.addr(3211).as_deref(),
        Some("192.0.2.10:3211")
    );

    let st = state.lock().unwrap();
    assert_eq!(
        mutating_calls(&st),
        [format!("POST /lume/vms/{name}/run")],
        "{:#?}",
        st.calls
    );
    let (_, _, run) = st.calls.iter().find(|(m, _, _)| m == "POST").unwrap();
    assert_eq!(
        run,
        &json!({
            "noDisplay": true,
            "sharedDirectories": [
                { "hostPath": setup.display().to_string(), "readOnly": true }
            ]
        })
    );
    assert_eq!(st.vms[name]["status"], "running");
}

#[tokio::test]
async fn resume_of_a_running_vm_does_not_boot_it_again() {
    let name = "cua-e2e-resume-running";
    earlier_start(name);
    let (rt, state) = fake_with(name, "running").await;

    let inst = rt.resume(name).await.unwrap();
    assert_eq!(inst.status, Status::Running);
    assert!(mutating_calls(&state.lock().unwrap()).is_empty());
}

#[tokio::test]
async fn stop_then_resume_restarts_the_vm() {
    let name = "cua-e2e-resume-restart";
    earlier_start(name);
    let (rt, state) = fake_with(name, "running").await;

    rt.stop(name).await.unwrap();
    rt.resume(name).await.unwrap();
    assert_eq!(
        mutating_calls(&state.lock().unwrap()),
        [
            format!("POST /lume/vms/{name}/stop"),
            format!("POST /lume/vms/{name}/run"),
        ]
    );
}

#[tokio::test]
async fn resume_of_an_unknown_vm_is_not_found() {
    let (rt, state) = fake_with("cua-e2e-resume-other", "stopped").await;
    let err = rt.resume("cua-e2e-resume-missing").await.unwrap_err();
    assert!(
        matches!(&err, VmmError::NotFound(n) if n == "cua-e2e-resume-missing"),
        "{err:?}"
    );
    assert!(mutating_calls(&state.lock().unwrap()).is_empty());
}
