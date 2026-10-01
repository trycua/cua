//! A macOS sandbox cloned from a cached Lume base (`cua-base-*`) gets the
//! requested CPU and memory, not the base's.
//!
//! * Hermetic (always runs): a fake `lume serve` records the API calls and
//!   the test asserts the `PATCH /lume/vms/:name` (`lume set`) lands between
//!   the clone and the first boot.
//! * Live (opt-in): clones a real, already cached base with 4096 MB and reads
//!   `sysctl hw.memsize` inside the guest. Never pulls a base; deletes its VM.
//!
//!   `CUA_LUME_LIVE_CLONE_REF='ghcr.io/trycua/macos:26@sha256:…' \
//!    cargo test -p cua-vmm --test lume_clone_resources -- --nocapture`

#![cfg(feature = "lume")]

#[allow(dead_code)]
#[path = "support/fake_lume.rs"]
mod fake_lume;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_vmm::lume::{LumeConfig, LumeRuntime, base_vm_name};
use cua_vmm::{ExecRequest, GuestOs, ImageSource, Runtime, StartSpec, VmmError};
use fake_lume::{FakeLume, serve, temp_home};
use serde_json::json;

const REF: &str = "ghcr.io/trycua/macos:26";

async fn fake_runtime(fail_set: bool) -> (LumeRuntime, Arc<Mutex<FakeLume>>) {
    let mut st = FakeLume {
        fail_set,
        ..Default::default()
    };
    let base = base_vm_name(REF);
    st.vms.insert(
        base.clone(),
        json!({
            "name": base, "os": "macOS", "cpuCount": 4, "memorySize": 8u64 << 30,
            "diskSize": { "allocated": 1u64 << 30, "total": 150u64 << 30 },
            "display": "1024x768", "status": "stopped"
        }),
    );
    let state = Arc::new(Mutex::new(st));
    let url = serve(state.clone()).await;
    (fake_lume::runtime(url), state)
}

fn spec(name: &str, reference: &str) -> StartSpec {
    StartSpec::new(
        name,
        ImageSource::Oci {
            reference: reference.into(),
        },
    )
    .os(GuestOs::Macos)
    .cpus(2)
    .memory_mb(4096)
}

#[tokio::test]
async fn clone_is_set_to_the_requested_resources_before_boot() {
    let (rt, state) = fake_runtime(false).await;
    rt.start(&spec("cua-e2e-clone", REF)).await.unwrap();

    let st = state.lock().unwrap();
    let seq: Vec<String> = st
        .calls
        .iter()
        .filter(|(m, _, _)| m != "GET")
        .map(|(m, p, _)| format!("{m} {p}"))
        .collect();
    assert_eq!(
        seq,
        [
            "POST /lume/vms/clone",
            "PATCH /lume/vms/cua-e2e-clone",
            "POST /lume/vms/cua-e2e-clone/run",
        ],
        "{:#?}",
        st.calls
    );
    let (_, _, set) = st.calls.iter().find(|(m, _, _)| m == "PATCH").unwrap();
    assert_eq!(set, &json!({ "cpu": 2, "memory": "4096MB" }));
    let vm = &st.vms["cua-e2e-clone"];
    assert_eq!(vm["memorySize"], json!(4u64 << 30));
    assert_eq!(vm["cpuCount"], json!(2));
    // The base is untouched.
    assert_eq!(st.vms[&base_vm_name(REF)]["memorySize"], json!(8u64 << 30));
}

#[tokio::test]
async fn a_refused_set_deletes_the_clone_and_never_boots() {
    let (rt, state) = fake_runtime(true).await;
    let err = rt.start(&spec("cua-e2e-refused", REF)).await.unwrap_err();
    assert!(
        matches!(&err, VmmError::Lume { status: 400, message } if message == "set refused"),
        "{err:?}"
    );
    let st = state.lock().unwrap();
    assert!(!st.vms.contains_key("cua-e2e-refused"));
    assert!(!st.calls.iter().any(|(_, p, _)| p.ends_with("/run")));
}

/// Opt-in live check against the real `lume serve`; see the module docs.
#[tokio::test]
async fn live_clone_boots_with_the_requested_memory() {
    let Some(reference) = std::env::var("CUA_LUME_LIVE_CLONE_REF")
        .ok()
        .filter(|r| !r.is_empty())
    else {
        eprintln!("skipped: set CUA_LUME_LIVE_CLONE_REF to a cached lume image reference");
        return;
    };
    let mut cfg = LumeConfig {
        spawn_serve: false,
        ..LumeConfig::default()
    };
    cfg.root = temp_home().join("lume-root");
    let rt = LumeRuntime::new(cfg);
    let base = base_vm_name(&reference);
    let Some(b) = rt.client().get(&base).await.unwrap() else {
        panic!("{base} is not cached for {reference}; this test never pulls");
    };
    assert_eq!(b.status, "stopped", "{base} must be stopped");

    let name = format!("cua-e2e-lumemem-{}", std::process::id());
    let res = async {
        rt.start(&spec(&name, &reference).ready_timeout(Duration::from_secs(600)))
            .await?;
        let exec = rt.guest_exec(&name).await?.expect("macOS guest exec");
        let mut last = None;
        // `lume ssh` can refuse for a while after the IP appears: bounded retries.
        for _ in 0..30 {
            match exec
                .exec(
                    ExecRequest::sh("sysctl -n hw.memsize hw.ncpu")
                        .timeout(Duration::from_secs(60)),
                )
                .await
            {
                Ok(o) if o.exit_code == 0 => return Ok::<_, VmmError>(o.stdout_str().to_string()),
                other => last = Some(format!("{other:?}")),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Err(VmmError::other(format!(
            "guest exec never succeeded: {last:?}"
        )))
    }
    .await;
    let _ = rt.delete(&name).await;
    let out = res.unwrap();
    eprintln!("guest sysctl hw.memsize hw.ncpu: {out:?}");
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("4294967296"), "{out}");
    assert_eq!(lines.next(), Some("2"), "{out}");
}
