//! A third macOS VM is refused up front with a capacity error that says
//! why (Apple allows two at once per Mac), instead of Lume booting it and
//! Apple Virtualization stopping it ("stopped while booting"). Linux VMs on
//! Lume do not count. Hermetic: a fake `lume serve`.

#![cfg(feature = "lume")]

#[allow(dead_code)]
#[path = "support/fake_lume.rs"]
mod fake_lume;

use std::sync::{Arc, Mutex};

use cua_vmm::{Runtime, VmmError};
use fake_lume::{FakeLume, serve};
use serde_json::{Value, json};

fn vm(name: &str, os: &str, status: &str) -> Value {
    json!({
        "name": name, "os": os, "cpuCount": 4, "memorySize": 4u64 << 30,
        "diskSize": { "allocated": 1u64 << 30, "total": 50u64 << 30 },
        "display": "1024x768", "status": status,
        "ipAddress": if status == "running" { json!("192.0.2.10") } else { json!(null) },
    })
}

async fn fake(vms: &[(&str, &str, &str)]) -> (cua_vmm::lume::LumeRuntime, Arc<Mutex<FakeLume>>) {
    let mut st = FakeLume::default();
    for (n, os, s) in vms {
        st.vms.insert((*n).into(), vm(n, os, s));
    }
    let state = Arc::new(Mutex::new(st));
    let url = serve(state.clone()).await;
    (fake_lume::runtime(url), state)
}

fn runs(st: &FakeLume) -> usize {
    st.calls
        .iter()
        .filter(|(m, p, _)| m == "POST" && p.ends_with("/run"))
        .count()
}

#[tokio::test]
async fn a_third_macos_vm_is_refused_before_it_boots() {
    let (rt, state) = fake(&[
        ("a", "macOS", "running"),
        ("b", "macOS", "running"),
        ("target", "macOS", "stopped"),
    ])
    .await;
    let err = rt.resume("target").await.unwrap_err();
    match &err {
        VmmError::HostCapacity(m) => {
            assert!(m.contains("already runs 2 macOS VMs"), "{m}");
            assert!(m.contains("a, b"), "{m}");
            assert!(m.contains("Apple allows 2"), "{m}");
        }
        other => panic!("expected HostCapacity, got {other:?}"),
    }
    assert_eq!(runs(&state.lock().unwrap()), 0, "nothing was booted");
}

#[tokio::test]
async fn linux_vms_and_stopped_macos_vms_do_not_count() {
    let (rt, state) = fake(&[
        ("a", "macOS", "running"),
        ("l", "linux", "running"),
        ("s", "macOS", "stopped"),
        ("target", "macOS", "stopped"),
    ])
    .await;
    // The fake never brings the VM up, so the resume itself may fail later;
    // what matters is that it was not refused for capacity and Lume was
    // asked to run it.
    let r = tokio::time::timeout(std::time::Duration::from_secs(2), rt.resume("target")).await;
    if let Ok(Err(VmmError::HostCapacity(m))) = r {
        panic!("refused for capacity: {m}");
    }
    assert_eq!(runs(&state.lock().unwrap()), 1);
}
