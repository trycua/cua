// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! GPU acceleration and cancelled creates on Lume, against a fake `lume
//! serve` and an in-memory host preference (never the host's `defaults`).

#![cfg(feature = "lume")]

#[allow(dead_code)]
#[path = "support/fake_lume.rs"]
mod fake_lume;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_vmm::lume::gpu::GpuPreference;
use cua_vmm::lume::{LumeRuntime, base_vm_name};
use cua_vmm::{GuestOs, ImageSource, Runtime, StartSpec, VmmError};
use fake_lume::{FakeLume, serve, temp_home};
use serde_json::json;

const REF: &str = "ghcr.io/trycua/macos:26";

/// The preference, logged into the fake's call list so its order against
/// the API calls shows.
struct Pref {
    on: Mutex<bool>,
    lume: Arc<Mutex<FakeLume>>,
}

impl std::fmt::Debug for Pref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pref")
    }
}

impl GpuPreference for Pref {
    fn get(&self) -> std::io::Result<bool> {
        Ok(*self.on.lock().unwrap())
    }
    fn set(&self, on: bool) -> std::io::Result<()> {
        *self.on.lock().unwrap() = on;
        self.lume
            .lock()
            .unwrap()
            .calls
            .push(("PREF".into(), "set".into(), json!(on)));
        Ok(())
    }
}

async fn fake() -> (LumeRuntime, Arc<Mutex<FakeLume>>, Arc<Pref>) {
    let mut st = FakeLume::default();
    let base = base_vm_name(REF);
    st.vms.insert(
        base.clone(),
        json!({
            "name": base, "os": "macOS", "cpuCount": 2, "memorySize": 4u64 << 30,
            "diskSize": { "allocated": 1u64 << 30, "total": 150u64 << 30 },
            "display": "1024x768", "status": "stopped"
        }),
    );
    let state = Arc::new(Mutex::new(st));
    let url = serve(state.clone()).await;
    let pref = Arc::new(Pref {
        on: Mutex::new(false),
        lume: state.clone(),
    });
    let rt = fake_lume::runtime(url).with_gpu_preference(pref.clone());
    (rt, state, pref)
}

fn spec(name: &str) -> StartSpec {
    StartSpec::new(
        name,
        ImageSource::Oci {
            reference: REF.into(),
        },
    )
    .os(GuestOs::Macos)
    .cpus(2)
    .memory_mb(4096)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[tokio::test]
async fn a_gpu_vm_turns_the_preference_on_before_it_boots_and_off_when_deleted() {
    temp_home();
    let (rt, state, pref) = fake().await;
    let mut s = spec("gpu-a");
    s.gpu = Some(cua_vmm::gpu::PARAVIRTUAL.into());
    rt.start(&s).await.unwrap();
    let calls: Vec<(String, String)> = state
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|(m, p, _)| (m.clone(), p.clone()))
        .collect();
    let at = |m: &str, p: &str| calls.iter().position(|c| c.0 == m && c.1 == p);
    let set = at("PREF", "set").expect("the preference was set");
    let run = at("POST", "/lume/vms/gpu-a/run").expect("it booted");
    assert!(set < run, "set before the boot: {calls:?}");
    assert!(*pref.on.lock().unwrap());
    // A plain VM leaves it alone and does not turn it off.
    rt.start(&spec("plain-a")).await.unwrap();
    rt.delete("plain-a").await.unwrap();
    assert!(*pref.on.lock().unwrap(), "gpu-a still runs with it");
    rt.delete("gpu-a").await.unwrap();
    assert!(!*pref.on.lock().unwrap(), "no GPU VM is left");
}

#[tokio::test]
async fn gpu_acceleration_is_for_macos_guests_only() {
    temp_home();
    let (rt, _state, pref) = fake().await;
    let mut s = spec("gpu-linux").os(GuestOs::Linux);
    s.gpu = Some(cua_vmm::gpu::PARAVIRTUAL.into());
    let err = rt.start(&s).await.unwrap_err();
    assert!(matches!(err, VmmError::Unsupported { .. }), "{err}");
    let mut s = spec("gpu-virgl");
    s.gpu = Some(cua_vmm::gpu::VIRGL.into());
    let err = rt.start(&s).await.unwrap_err();
    assert!(matches!(err, VmmError::Unsupported { .. }), "{err}");
    assert!(!*pref.on.lock().unwrap());
}

/// A create cut off during the first boot deletes the clone it made; the
/// base it cloned stays.
#[tokio::test]
async fn a_create_cut_off_after_the_clone_deletes_the_clone() {
    temp_home();
    let (rt, state, _pref) = fake().await;
    state.lock().unwrap().hang_run = true;
    let cut = tokio::time::timeout(Duration::from_millis(1500), rt.start(&spec("cut-a"))).await;
    assert!(cut.is_err(), "the boot hangs, so the start was cut off");
    assert!(cua_vmm::cleanup::settle(Duration::from_secs(30)).await);
    let st = state.lock().unwrap();
    assert!(
        !st.vms.contains_key("cut-a"),
        "the clone is gone: {:?}",
        st.vms.keys()
    );
    assert!(st.vms.contains_key(&base_vm_name(REF)), "the base stays");
    assert!(
        st.calls
            .iter()
            .any(|(m, p, _)| m == "DELETE" && p == "/lume/vms/cut-a"),
        "{:?}",
        st.calls
    );
}
