//! The macOS Space with developer tools (`macos:26-xcode`) is pulled like the
//! other macOS images; when it cannot be, or the user prefers their own
//! Xcode (`CUA_DEV_IMAGE=local`), it is built on their Mac from the full
//! image's base (held, else pulled) with a disk check before anything starts.
//! A built base is cloned like any other. Nothing here builds with a real
//! Xcode.
//!
//! Hermetic: a fake `lume serve` records the API calls; the build itself
//! (`derive-local.sh`) is covered by `lume::derive`'s tests and the live run.

#![cfg(feature = "lume")]

#[allow(dead_code)]
#[path = "support/fake_lume.rs"]
mod fake_lume;

use std::sync::{Arc, Mutex};

use cua_vmm::lume::{OwnedKind, OwnedVms, base_vm_name};
use cua_vmm::{GuestOs, ImageSource, Runtime, StartSpec, VmmError};
use fake_lume::{FakeLume, FakePull, PullEnd, serve, temp_home};
use serde_json::json;

const DEV: &str = "ghcr.io/trycua/macos:26-xcode";
/// A full image base the SDK pulled earlier, under its dated pin.
fn held_full() -> String {
    format!(
        "ghcr.io/trycua/macos:26-{}-fca2193@sha256:{}",
        "20261021",
        "a".repeat(64)
    )
}

struct Space(u64);
impl cua_vmm::disk::SpaceProbe for Space {
    fn space(&self, _: &std::path::Path) -> std::io::Result<cua_vmm::disk::Space> {
        Ok(cua_vmm::disk::Space {
            available: self.0,
            total: 1 << 50,
        })
    }
}

fn vm(name: &str) -> serde_json::Value {
    json!({
        "name": name, "os": "macOS", "cpuCount": 4, "memorySize": 8u64 << 30,
        "diskSize": { "allocated": 1u64 << 30, "total": 150u64 << 30 },
        "display": "1024x768", "status": "stopped"
    })
}

fn spec(name: &str) -> StartSpec {
    StartSpec::new(
        name,
        ImageSource::Oci {
            reference: DEV.into(),
        },
    )
    .os(GuestOs::Macos)
    .cpus(2)
    .memory_mb(4096)
}

fn pull(end: PullEnd) -> FakePull {
    FakePull {
        polls: 2,
        step: 1 << 30,
        total: 4 << 30,
        end,
    }
}

fn pulls(st: &FakeLume) -> Vec<String> {
    st.calls
        .iter()
        .filter(|(_, p, _)| p == "/lume/pull/start")
        .map(|(_, _, b)| b["image"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One test, in order: the probe, the environment and the owned-VM records are
/// process-wide.
#[tokio::test]
async fn the_published_image_is_pulled_building_is_the_fallback() {
    // About which image is pulled, not the network gateway Secure Spaces (on by default) needs.
    let _open = cua_vmm::network::hold_mode(cua_vmm::network::Mode::Legacy);
    temp_home();
    let held = base_vm_name(&held_full());
    let dev = base_vm_name(DEV);
    OwnedVms::default().mark(&held, OwnedKind::Base, Some(&held_full()));

    // 1. Published: a create pulls macos:26-xcode into the base and clones it.
    cua_vmm::disk::set_probe(Some(Arc::new(Space(1 << 50))));
    let state = Arc::new(Mutex::new(FakeLume {
        pull: Some(pull(PullEnd::Completes)),
        ..Default::default()
    }));
    let rt = fake_lume::runtime(serve(state.clone()).await);
    rt.start(&spec("cua-dev-pulled")).await.unwrap();
    {
        let st = state.lock().unwrap();
        assert_eq!(pulls(&st), ["macos:26-xcode"], "{:#?}", st.calls);
        assert!(st.vms.contains_key(&dev) && st.vms.contains_key("cua-dev-pulled"));
    }

    // 2. Not published (the pull fails) and the full image is held: it falls
    //    back to building, which checks its disk (more than a pull) before
    //    anything starts. The held full image is reused, not pulled.
    cua_vmm::disk::set_probe(Some(Arc::new(Space(38 << 30))));
    let mut st = FakeLume {
        pull: Some(pull(PullEnd::Fails("manifest unknown".into()))),
        ..Default::default()
    };
    st.vms.insert(held.clone(), vm(&held));
    let state = Arc::new(Mutex::new(st));
    let rt = fake_lume::runtime(serve(state.clone()).await);
    let err = rt.start(&spec("cua-dev-refused")).await.unwrap_err();
    assert!(matches!(err, VmmError::InsufficientDisk(_)), "{err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains("build the macOS Space with developer tools")
            && msg.contains("cua cache prune"),
        "{msg}"
    );
    {
        let st = state.lock().unwrap();
        assert_eq!(
            pulls(&st),
            ["macos:26-xcode"],
            "only the published image was tried"
        );
        assert!(!st.vms.contains_key(&dev) && !st.vms.contains_key("cua-dev-refused"));
    }
    assert!(
        !OwnedVms::default().owns(&dev),
        "no record of a build that never began"
    );

    // 3. A Mac that prefers its own Xcode never tries the pull.
    // SAFETY: the only test in this binary; no other thread reads the environment.
    unsafe { std::env::set_var("CUA_DEV_IMAGE", "local") };
    // (the fake keeps a failed pull's record; a real Lume removes it)
    state.lock().unwrap().pulling.clear();
    state.lock().unwrap().calls.clear();
    let err = rt.start(&spec("cua-dev-local")).await.unwrap_err();
    assert!(matches!(err, VmmError::InsufficientDisk(_)), "{err:?}");
    assert!(
        pulls(&state.lock().unwrap()).is_empty(),
        "no pull when local is preferred"
    );
    unsafe { std::env::remove_var("CUA_DEV_IMAGE") };

    // 4. The base exists (pulled or built earlier): a new Space is a clone of
    //    it. No pull, no build.
    cua_vmm::disk::set_probe(Some(Arc::new(Space(1 << 50))));
    state.lock().unwrap().vms.insert(dev.clone(), vm(&dev));
    state.lock().unwrap().calls.clear();
    rt.start(&spec("cua-dev-clone")).await.unwrap();
    let st = state.lock().unwrap();
    assert!(pulls(&st).is_empty(), "{:#?}", st.calls);
    let writes: Vec<String> = st
        .calls
        .iter()
        .filter(|(m, _, _)| m != "GET")
        .map(|(m, p, _)| format!("{m} {p}"))
        .collect();
    assert_eq!(
        writes,
        [
            "POST /lume/vms/clone",
            "PATCH /lume/vms/cua-dev-clone",
            "POST /lume/vms/cua-dev-clone/run",
        ],
        "{:#?}",
        st.calls
    );
    let (_, _, clone) = st
        .calls
        .iter()
        .find(|(_, p, _)| p == "/lume/vms/clone")
        .unwrap();
    assert_eq!(clone["name"], json!(dev), "cloned from the base: {clone}");
}
