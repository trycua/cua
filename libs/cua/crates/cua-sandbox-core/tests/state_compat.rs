//! State files are interchangeable with cua-sandbox's `sandbox_state.py`.
//!
//! The fixtures in `tests/fixtures/python-state/` were written by the real
//! Python module (`sandbox_state.save(...)`, `save_fleet_claim(...)`,
//! `update(..., status="stopped")`) with `HOME` pointed at a scratch dir and
//! `Image.from_registry(...).to_dict()` / `Image.linux().to_dict()` images.

use cua_sandbox_core::state::{
    FleetState, LocalState, SandboxState, StateStore, python_utc_now, registry_image_dict,
};
use serde_json::{Map, Value};
use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/python-state")
}

fn read(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(fixture_dir().join(format!("{name}.json"))).unwrap(),
    )
    .unwrap()
}

fn without_created_at(mut v: Value) -> Value {
    v.as_object_mut().unwrap().remove("created_at");
    v
}

#[test]
fn reads_python_written_files() {
    let store = StateStore::new(fixture_dir());
    let all = store.list_all();
    assert_eq!(
        all.iter().map(|s| s.name()).collect::<Vec<_>>(),
        ["py-docker-sbx", "py-fleet-claim", "py-qemu-sbx"]
    );
    match store.load("py-qemu-sbx").unwrap() {
        SandboxState::Local(l) => {
            assert_eq!(l.runtime_type, "qemu");
            assert_eq!(l.host, "127.0.0.1");
            assert_eq!(l.api_port, 18000);
            assert_eq!(l.exposed_ports.as_ref().unwrap()["3211"], 23211);
            assert_eq!(l.qmp_port, Some(14444));
            assert_eq!(l.grpc_port, None);
            assert_eq!(l.memory_mb, Some(4096));
            assert_eq!(
                l.image["registry"],
                "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34"
            );
            assert!(l.extra.is_empty());
        }
        other => panic!("expected local state, got {other:?}"),
    }
    match store.load("py-fleet-claim").unwrap() {
        SandboxState::Fleet(f) => {
            assert_eq!(f.runtime_type, "fleet");
            assert_eq!(f.pool_name, "cua-e2e-pool");
            assert_eq!(f.status, "running");
        }
        other => panic!("expected fleet state, got {other:?}"),
    }
    assert_eq!(store.load("py-docker-sbx").unwrap().status(), "stopped");
    assert!(store.load("missing").is_none());
}

#[test]
fn writes_the_python_shape() {
    let dir = tempfile::tempdir().unwrap();
    let store = StateStore::new(dir.path());

    // Same inputs as the Python fixture → same JSON (minus the timestamp).
    let local = SandboxState::Local(LocalState {
        name: "py-qemu-sbx".into(),
        runtime_type: "qemu".into(),
        image: registry_image_dict(
            "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34",
            "linux",
            Some("vm"),
        ),
        host: "127.0.0.1".into(),
        api_port: 18000,
        exposed_ports: Some([("3211".to_string(), 23211), ("8000".to_string(), 18000)].into()),
        vnc_port: Some(15900),
        qmp_port: Some(14444),
        disk_path: Some("/tmp/x.qcow2".into()),
        os_type: Some("linux".into()),
        memory_mb: Some(4096),
        cpu_count: Some(2),
        arch: Some("x86_64".into()),
        status: "running".into(),
        created_at: python_utc_now(),
        ..Default::default()
    });
    store.save(&local).unwrap();
    let ours: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("py-qemu-sbx.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        without_created_at(ours.clone()),
        without_created_at(read("py-qemu-sbx"))
    );
    // Key order matches the Python writer too (serde follows declaration order).
    let text = std::fs::read_to_string(dir.path().join("py-qemu-sbx.json")).unwrap();
    let keys: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("  \""))
        .map(|l| l.trim().split('"').nth(1).unwrap())
        .collect();
    let py = std::fs::read_to_string(fixture_dir().join("py-qemu-sbx.json")).unwrap();
    let py_keys: Vec<&str> = py
        .lines()
        .filter(|l| l.starts_with("  \""))
        .map(|l| l.trim().split('"').nth(1).unwrap())
        .collect();
    assert_eq!(keys, py_keys);

    store
        .save_fleet_claim("py-fleet-claim", "cua-e2e-pool")
        .unwrap();
    let fleet = store.load_raw("py-fleet-claim").unwrap();
    assert_eq!(
        without_created_at(Value::Object(fleet.clone())),
        without_created_at(read("py-fleet-claim"))
    );
    // `datetime.now(timezone.utc).isoformat()` shape.
    let ts = fleet["created_at"].as_str().unwrap();
    assert!(
        ts.ends_with("+00:00") && ts.len() == "2026-09-22T23:19:06.056535+00:00".len(),
        "{ts}"
    );
}

#[test]
fn update_preserves_unknown_fields_and_delete_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    for f in std::fs::read_dir(fixture_dir()).unwrap() {
        let f = f.unwrap().path();
        std::fs::copy(&f, dir.path().join(f.file_name().unwrap())).unwrap();
    }
    let store = StateStore::new(dir.path());
    let mut fields = Map::new();
    fields.insert("custom".into(), Value::String("kept".into()));
    store.update("py-fleet-claim", fields).unwrap();
    store.set_status("py-fleet-claim", "suspended").unwrap();
    match store.load("py-fleet-claim").unwrap() {
        SandboxState::Fleet(FleetState { status, extra, .. }) => {
            assert_eq!(status, "suspended");
            assert_eq!(extra["custom"], "kept");
        }
        other => panic!("{other:?}"),
    }
    store.delete("py-fleet-claim").unwrap();
    store.delete("py-fleet-claim").unwrap();
    assert!(store.load("py-fleet-claim").is_none());
    assert!(store.save_fleet_claim("../escape", "p").is_err());
}

#[test]
fn ephemeral_leases_live_beside_state_files_without_showing_up_as_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = StateStore::new(dir.path());
    store
        .write_lease("cua-eph-0000abcd", std::process::id())
        .unwrap();
    let leases = store.leases();
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].name, "cua-eph-0000abcd");
    assert_eq!(leases[0].pid, std::process::id());
    assert!(leases[0].created_at > 0);
    // State listings (and the Python reader's `*.json` glob) never see it.
    assert!(store.list_all().is_empty());
    assert!(store.write_lease("../escape", 1).is_err());
    store.remove_lease("cua-eph-0000abcd").unwrap();
    store.remove_lease("cua-eph-0000abcd").unwrap();
    assert!(store.leases().is_empty());
}

/// The host-safety guard: a test cannot write the user's real
/// `~/.cua/sandboxes`, even through the default store.
#[test]
fn a_test_cannot_write_real_sandbox_state() {
    let Some(real) = cua_home::real_cua_home() else {
        return;
    };
    let store = StateStore::new(real.join("sandboxes"));
    let name = "cua-home-guard-probe";
    let err = store.save_fleet_claim(name, "pool").unwrap_err();
    assert!(err.to_string().contains("CUA_HOME"), "{err}");
    assert!(store.write_lease(name, 1).is_err());
    assert!(!real.join("sandboxes").join(format!("{name}.json")).exists());
}
