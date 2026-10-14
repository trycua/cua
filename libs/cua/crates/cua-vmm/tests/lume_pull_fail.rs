//! A macOS create whose base image download stops partway fails within
//! seconds, with the reason, and leaves nothing behind; the next try pulls
//! again and finishes.
//!
//! Hermetic: a fake `lume serve` runs the async pull (`POST
//! /lume/pull/start`, then `GET /lume/vms/:name` while it pulls) and ends it
//! one of three ways: Lume reports the pull failed, the pull is cancelled
//! outside the SDK (Lume no longer knows the VM), or `lume serve` dies with
//! the pull inside it. Without the fix each waited out the pull's idle
//! timeout (10 minutes in the daemon, 10 s here).

#![cfg(feature = "lume")]

#[allow(dead_code)]
#[path = "support/fake_lume.rs"]
mod fake_lume;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_vmm::lume::base_vm_name;
use cua_vmm::{GuestOs, ImageSource, Runtime, StartSpec, VmmError};
use fake_lume::{FakeLume, FakePull, PullEnd, serve, temp_home};

/// One image per test: tests run in parallel and share the owned-VM records.
const REF_FAILED: &str = "registry.example/test/macos:failed";
const REF_VANISHED: &str = "registry.example/test/macos:vanished";
const REF_DIED: &str = "registry.example/test/macos:died";

/// Plenty of free disk, so the pull's disk guard never decides the test.
struct Roomy;
impl cua_vmm::disk::SpaceProbe for Roomy {
    fn space(&self, _: &std::path::Path) -> std::io::Result<cua_vmm::disk::Space> {
        Ok(cua_vmm::disk::Space {
            available: 1 << 50,
            total: 1 << 50,
        })
    }
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

fn pull(end: PullEnd) -> FakePull {
    FakePull {
        polls: 4,
        step: 1 << 30,
        total: 30 << 30,
        end,
    }
}

async fn fake(end: PullEnd) -> (Arc<Mutex<FakeLume>>, String) {
    temp_home();
    cua_vmm::disk::set_probe(Some(Arc::new(Roomy)));
    let state = Arc::new(Mutex::new(FakeLume {
        pull: Some(pull(end)),
        ..Default::default()
    }));
    let url = serve(state.clone()).await;
    (state, url)
}

fn owns(name: &str) -> bool {
    cua_vmm::lume::OwnedVms::default()
        .list()
        .iter()
        .any(|r| r.name == name)
}

/// Starts `name`, expecting the pull to stop: the error, and how long the
/// create took to fail.
async fn failed_create(url: String, name: &str, reference: &str) -> (VmmError, Duration) {
    let rt = fake_lume::runtime(url);
    let t = Instant::now();
    let err = rt.start(&spec(name, reference)).await.unwrap_err();
    (err, t.elapsed())
}

#[tokio::test]
async fn a_pull_lume_reports_failed_says_why_and_leaves_nothing() {
    let (state, url) = fake(PullEnd::Fails("The network connection was lost.".into())).await;
    let (err, took) = failed_create(url, "cua-pf-failed", REF_FAILED).await;
    assert!(took < Duration::from_secs(5), "failed after {took:?}");
    assert!(matches!(err, VmmError::PullStopped { .. }), "{err}");
    assert!(
        err.to_string()
            .contains("stopped before it finished (The network connection was lost)"),
        "{err}"
    );
    let st = state.lock().unwrap();
    assert!(st.vms.is_empty(), "no VM is left: {:?}", st.vms.keys());
    assert!(!owns(&base_vm_name(REF_FAILED)), "no base record is left");
}

#[tokio::test]
async fn a_pull_cancelled_outside_fails_within_seconds() {
    let (state, url) = fake(PullEnd::Vanishes).await;
    let (err, took) = failed_create(url, "cua-pf-vanished", REF_VANISHED).await;
    assert!(took < Duration::from_secs(6), "failed after {took:?}");
    assert!(
        err.to_string().contains("(it was cancelled outside Cua)"),
        "{err}"
    );
    assert!(state.lock().unwrap().vms.is_empty());
    assert!(!owns(&base_vm_name(REF_VANISHED)));
}

/// `lume serve` dies mid-pull: the create fails within seconds saying so,
/// nothing is left, and the next try (a restarted serve) pulls again from
/// the start of the request and boots the Space.
#[tokio::test]
async fn a_serve_that_dies_mid_pull_fails_fast_and_try_again_finishes() {
    let (state, url) = fake(PullEnd::ServeDies).await;
    let (err, took) = failed_create(url, "cua-pf-died", REF_DIED).await;
    assert!(took < Duration::from_secs(6), "failed after {took:?}");
    assert!(err.to_string().contains("(Lume stopped running)"), "{err}");
    assert!(state.lock().unwrap().dead, "the fake serve died mid-pull");
    assert!(state.lock().unwrap().vms.is_empty());
    assert!(!owns("cua-pf-died"));
    assert!(!owns(&base_vm_name(REF_DIED)));

    // Try again: Lume is serving again and this pull finishes.
    let again = {
        let mut st = state.lock().unwrap();
        st.dead = false;
        st.pull = Some(pull(PullEnd::Completes));
        st.calls.clear();
        state.clone()
    };
    let url = serve(again).await;
    let rt = fake_lume::runtime(url);
    rt.start(&spec("cua-pf-died", REF_DIED)).await.unwrap();
    let st = state.lock().unwrap();
    let base = base_vm_name(REF_DIED);
    let starts = st
        .calls
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/lume/pull/start")
        .count();
    assert_eq!(starts, 1, "the retry asks Lume to pull again");
    assert!(st.vms.contains_key(&base), "the base is pulled");
    assert_eq!(st.vms["cua-pf-died"]["status"], "running");
}
