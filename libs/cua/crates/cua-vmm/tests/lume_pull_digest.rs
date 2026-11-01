//! A Lume base is named for the digest its image resolved to
//! (`repo:tag@sha256:…`), but Lume pulls by tag. When the tag names another
//! digest by the time of the pull (it moved, or the reference pinned an
//! older one), the pulled base is removed and the create fails saying so,
//! instead of every later clone inheriting the other image.
//!
//! Hermetic: a fake `lume serve` completes the pull, and the test writes the
//! `.manifest-digest` Lume records for it, under a temp `LUME_HOME`.

#![cfg(feature = "lume")]

#[allow(dead_code)]
#[path = "support/fake_lume.rs"]
mod fake_lume;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use cua_vmm::lume::base_vm_name;
use cua_vmm::{GuestOs, ImageSource, Runtime, StartSpec, VmmError};
use fake_lume::{FakeLume, FakePull, PullEnd, serve, temp_home};

const X: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const Y: &str = "sha256:2222222222222222222222222222222222222222222222222222222222222222";

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

/// Local's Lume keeps its VMs under `LUME_HOME`: a temp directory, set
/// once before any runtime reads it.
fn lume_home() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = temp_home().join("lume");
        // SAFETY: set once, first thing in every test, before other threads
        // of this test read the environment.
        unsafe {
            std::env::set_var("CUA_CHANNEL", "local");
            std::env::set_var("LUME_HOME", &d);
        }
        d
    })
}

/// What Lume writes once it pulled `digest` into VM `name`.
fn lume_pulled(name: &str, digest: &str) {
    let dir = lume_home().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".manifest-digest"), digest).unwrap();
}

async fn fake() -> (Arc<Mutex<FakeLume>>, String) {
    lume_home();
    cua_vmm::disk::set_probe(Some(Arc::new(Roomy)));
    let state = Arc::new(Mutex::new(FakeLume {
        pull: Some(FakePull {
            polls: 1,
            step: 1 << 30,
            total: 1 << 30,
            end: PullEnd::Completes,
        }),
        ..Default::default()
    }));
    let url = serve(state.clone()).await;
    (state, url)
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

fn owns(name: &str) -> bool {
    cua_vmm::lume::OwnedVms::default()
        .list()
        .iter()
        .any(|r| r.name == name)
}

#[tokio::test]
async fn a_base_pulled_at_another_digest_is_removed_and_refused() {
    let (state, url) = fake().await;
    let reference = format!("registry.example/test/macos:moved@{X}");
    let base = base_vm_name(&reference);
    lume_pulled(&base, Y);
    let err = fake_lume::runtime(url)
        .start(&spec("cua-pd-moved", &reference))
        .await
        .unwrap_err();
    let VmmError::Invalid(m) = &err else {
        panic!("expected an invalid request, got {err:?}");
    };
    assert!(
        m.contains(&format!(
            "registry.example/test/macos:moved names {Y} now, not {X}"
        )),
        "{m}"
    );
    let st = state.lock().unwrap();
    assert!(
        st.vms.is_empty(),
        "the base is removed: {:?}",
        st.vms.keys()
    );
    assert!(!owns(&base), "no base record is left");
}

#[tokio::test]
async fn a_base_pulled_at_its_digest_is_kept() {
    let (state, url) = fake().await;
    let reference = format!("registry.example/test/macos:kept@{X}");
    let base = base_vm_name(&reference);
    lume_pulled(&base, X);
    fake_lume::runtime(url)
        .start(&spec("cua-pd-kept", &reference))
        .await
        .unwrap();
    assert!(state.lock().unwrap().vms.contains_key(&base));
    assert!(owns(&base));
}
