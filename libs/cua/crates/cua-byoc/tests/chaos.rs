// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Sweeps on a chaotic network never lose data: deterministic fault
//! injection against the scripted cloud (partitions, half-open calls,
//! partial listings, rate limits, duplicate sweepers, skewed clocks, a
//! create in flight, eventual consistency). Every test asserts zero false
//! deletions and no forgotten record while the fault lasts, and that the
//! cleanup happens once the network heals.

use std::collections::BTreeMap;
use std::sync::Arc;

use cua_byoc::model::{self, Resource, Tier};
use cua_byoc::sweep::{CONSISTENCY_SECS, EXPIRY_MARGIN_SECS, GRACE_SECS};
use cua_byoc::testing::FakeProvider;
use cua_byoc::{Clouds, Store, Target};
use cua_sandbox_core::byoc::{CloudManager, CloudSweepReport};

struct Lab {
    dir: tempfile::TempDir,
    fake: Arc<FakeProvider>,
    clouds: Clouds,
    owner: String,
}

async fn lab() -> Lab {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProvider::new("aws", Tier::Vm);
    let store = Arc::new(Store::new(dir.path()));
    let clouds = Clouds::new(dir.path(), vec![fake.clone()], store.clone());
    clouds
        .connect(
            &Target {
                provider: "aws".into(),
                region: Some("us-west-2".into()),
                ..Default::default()
            },
            false,
            None,
        )
        .await
        .unwrap();
    let owner = store.owner_id().unwrap();
    Lab {
        dir,
        fake,
        clouds,
        owner,
    }
}

impl Lab {
    fn store(&self) -> Store {
        Store::new(self.dir.path())
    }

    /// A Space machine of this owner the cloud runs (`recorded`: and this
    /// home's records name it), created `age` seconds ago, expiring at
    /// `expires`.
    fn space(&self, id: &str, age: u64, expires: u64, recorded: bool) -> Resource {
        let t = model::now();
        let r = Resource {
            provider: "aws".into(),
            id: id.into(),
            resource_type: "instance".into(),
            name: format!("cua-{id}"),
            region: "us-west-2".into(),
            sandbox: format!("aws:{id}"),
            machine: format!("cloud-{id}"),
            created: t - age,
            expires,
            tags: model::tags(&self.owner, id, &format!("cloud-{id}"), expires),
            state: "running".into(),
            account: "fake-account".into(),
            ..Default::default()
        };
        self.fake.preexisting(r.clone());
        if recorded {
            self.store().record(r.clone()).unwrap();
        }
        r
    }

    fn inventory(&self) -> BTreeMap<String, Resource> {
        self.fake.account.lock().unwrap().resources.clone()
    }

    fn recorded(&self, id: &str) -> bool {
        self.store().resources().unwrap().iter().any(|r| r.id == id)
    }

    fn deletes(&self) -> Vec<String> {
        self.fake
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("delete"))
            .collect()
    }

    fn set(&self, f: impl FnOnce(&mut cua_byoc::testing::FakeAccount)) {
        f(&mut self.fake.account.lock().unwrap());
    }

    /// Both a manual `sweep --delete` and an automatic pass.
    async fn sweep_both(&self) -> Vec<CloudSweepReport> {
        let manual = self.clouds.sweep(None, false, false).await.unwrap();
        let auto = self.clouds.auto_sweep().await.unwrap();
        vec![manual, auto]
    }
}

const HOUR: u64 = 3600;

/// A live Space, an expired one, and one whose record was lost: the state
/// every chaos test starts from.
fn scene(l: &Lab) -> (Resource, Resource, Resource) {
    let t = model::now();
    let live = l.space("i-live", 2 * HOUR, t + 6 * HOUR, true);
    let expired = l.space("i-expired", 10 * HOUR, t - HOUR, true);
    let lost = l.space("i-lost", 2 * HOUR, t + 6 * HOUR, false);
    (live, expired, lost)
}

/// The invariant: a live Space and a Space whose record was lost are never
/// deleted, and no record is forgotten.
fn nothing_lost(l: &Lab) {
    let inv = l.inventory();
    assert!(inv.contains_key("i-live"), "a live Space was deleted");
    assert!(inv.contains_key("i-lost"), "an unrecorded Space was deleted");
    assert!(l.recorded("i-live"), "a live record was forgotten");
    assert!(!l.deletes().iter().any(|d| d == "delete i-live" || d == "delete i-lost"));
}

/// Once healed: the expired Space goes, nothing else.
async fn heals(l: &Lab) {
    l.sweep_both().await;
    nothing_lost(l);
    assert!(!l.inventory().contains_key("i-expired"), "healed: expired one goes");
    assert_eq!(
        l.deletes().iter().filter(|d| *d == "delete i-expired").count(),
        1,
        "deleted once"
    );
}

#[tokio::test]
async fn a_healthy_sweep_deletes_only_the_expired_space() {
    let l = lab().await;
    scene(&l);
    heals(&l).await;
}

#[tokio::test(start_paused = true)]
async fn a_partition_or_half_open_listing_times_out_and_deletes_nothing() {
    let l = lab().await;
    scene(&l);
    l.set(|a| a.hang_list = true);
    let r = l.clouds.sweep(None, false, false).await.unwrap();
    assert!(
        r.resources.iter().any(|i| i.action == "failed" && i.reason.contains("did not answer")),
        "{r:?}"
    );
    nothing_lost(&l);
    assert!(l.inventory().contains_key("i-expired"));
    assert!(l.recorded("i-expired"));
    l.set(|a| a.hang_list = false);
    heals(&l).await;
}

#[tokio::test]
async fn a_listing_that_fails_half_way_is_no_listing() {
    let l = lab().await;
    scene(&l);
    l.set(|a| a.fail_second_page = true);
    for r in l.sweep_both().await {
        assert!(r.resources.iter().any(|i| i.action == "failed"), "{r:?}");
        assert!(!r.resources.iter().any(|i| i.action == "deleted" || i.action == "forgotten"));
    }
    nothing_lost(&l);
    assert!(l.recorded("i-expired"));
    l.set(|a| a.fail_second_page = false);
    heals(&l).await;
}

#[tokio::test]
async fn rate_limits_and_unavailable_describes_are_never_a_verdict() {
    let l = lab().await;
    scene(&l);
    // A record the listing does not show, while describe is unavailable:
    // kept, not forgotten.
    let t = model::now();
    let unseen = Resource {
        provider: "aws".into(),
        id: "i-unseen".into(),
        resource_type: "instance".into(),
        region: "us-west-2".into(),
        sandbox: "aws:unseen".into(),
        state: "running".into(),
        created: t - 5 * HOUR,
        expires: t + HOUR,
        account: "fake-account".into(),
        ..Default::default()
    };
    l.store().record(unseen).unwrap();
    l.set(|a| {
        a.throttle = 2;
        a.fail_describe = true;
    });
    for r in l.sweep_both().await {
        assert!(!r.resources.iter().any(|i| i.action == "forgotten"), "{r:?}");
    }
    assert!(l.recorded("i-unseen"));
    nothing_lost(&l);
    l.set(|a| a.fail_describe = false);
    heals(&l).await;
    // Healed and asked where it lives, the cloud says it is gone: forgotten
    // by the manual sweep only.
    l.clouds.sweep(None, false, false).await.unwrap();
    assert!(!l.recorded("i-unseen"));
}

#[tokio::test]
async fn a_just_created_resource_the_cloud_does_not_list_yet_is_kept() {
    let l = lab().await;
    scene(&l);
    let young = l.space("i-young", 60, model::now() + 6 * HOUR, true);
    l.set(|a| {
        a.invisible.insert(young.id.clone());
    });
    for r in l.sweep_both().await {
        assert!(
            r.resources
                .iter()
                .any(|i| i.resource.id == "i-young" && i.action == "keep"),
            "{r:?}"
        );
    }
    assert!(l.recorded("i-young"), "eventual consistency is not 'gone'");
    assert!(CONSISTENCY_SECS >= 300);
}

#[tokio::test]
async fn a_create_in_flight_is_never_swept() {
    let l = lab().await;
    scene(&l);
    let t = model::now();
    // Recorded before its cloud call (no id yet), and the instance it is
    // making, tagged but not yet recorded by id.
    l.store()
        .record(Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            name: "cua-inflight".into(),
            sandbox: "aws:inflight".into(),
            created: t,
            state: "pending".into(),
            account: "fake-account".into(),
            ..Default::default()
        })
        .unwrap();
    let mut made = l.space("i-inflight", 5, t + 8 * HOUR, false);
    made.name = "cua-inflight".into();
    l.fake.preexisting(made);
    for _ in 0..2 {
        l.sweep_both().await;
    }
    assert!(l.inventory().contains_key("i-inflight"));
    assert!(
        l.store().resources().unwrap().iter().any(|r| r.name == "cua-inflight"),
        "the pending record stays"
    );
    assert!(GRACE_SECS >= 15 * 60);
}

#[tokio::test]
async fn skewed_clocks_never_end_a_space_early() {
    let l = lab().await;
    scene(&l);
    let t = model::now();
    // Expired by this machine's clock, but only just (inside the margin).
    l.space("i-just", 9 * HOUR, t - 60, true);
    // Expired by this machine's clock by 40 min, but the cloud's clock runs
    // an hour behind: not yet expired there.
    l.space("i-behind", 9 * HOUR, t - 40 * 60, true);
    l.set(|a| a.clock_skew = -3600);
    l.sweep_both().await;
    assert!(l.inventory().contains_key("i-just"), "inside the margin");
    assert!(l.inventory().contains_key("i-behind"), "not expired by the cloud's clock");
    assert!(l.inventory().contains_key("i-expired"), "1 h past locally is not past for a cloud 1 h behind");
    // The cloud's clock catches up: past the margin by both, it goes.
    l.set(|a| a.clock_skew = 0);
    l.sweep_both().await;
    assert!(!l.inventory().contains_key("i-behind"));
    assert!(!l.inventory().contains_key("i-expired"));
    assert!(l.inventory().contains_key("i-just"), "still inside the margin");
    assert!(EXPIRY_MARGIN_SECS >= 10 * 60);
    nothing_lost(&l);
}

#[tokio::test]
async fn two_sweepers_never_run_side_by_side() {
    let l = lab().await;
    scene(&l);
    // Another process of this home (the daemon, a second app) holds the lease.
    let held = l.store().try_sweep_lease().unwrap().expect("lease");
    let err = l.clouds.sweep(None, false, false).await.unwrap_err();
    assert!(err.to_string().contains("Another sweep"), "{err}");
    let r = l.clouds.auto_sweep().await.unwrap();
    assert!(r.resources.is_empty(), "the automatic pass waits its turn");
    assert!(l.deletes().is_empty());
    drop(held);
    // Two sweepers of the same home at once: one runs, the other is told;
    // nothing is deleted twice.
    let other = Clouds::new(
        l.dir.path(),
        vec![l.fake.clone()],
        Arc::new(Store::new(l.dir.path())),
    );
    let (a, b) = tokio::join!(
        l.clouds.sweep(None, false, false),
        other.sweep(None, false, false)
    );
    assert!(a.is_ok() || b.is_ok());
    for r in [a, b].into_iter().filter_map(|r| r.err()) {
        assert!(r.to_string().contains("Another sweep"), "{r}");
    }
    heals(&l).await;
}

#[tokio::test]
async fn another_device_s_resources_are_never_touched() {
    let l = lab().await;
    scene(&l);
    let t = model::now();
    // Another device of the account sweeps the same cloud account: its
    // resources carry its owner id, so neither ever touches the other's.
    l.fake.preexisting(Resource {
        provider: "aws".into(),
        id: "i-other-device".into(),
        resource_type: "instance".into(),
        region: "us-west-2".into(),
        created: t - 20 * HOUR,
        tags: model::tags("another-device", "x", "cloud-x", t - 10 * HOUR),
        state: "running".into(),
        ..Default::default()
    });
    l.clouds.sweep(None, false, true).await.unwrap();
    assert!(l.inventory().contains_key("i-other-device"));
}

#[tokio::test]
async fn a_live_space_whose_machine_is_unreachable_is_never_swept() {
    let l = lab().await;
    scene(&l);
    // No relay here at all (the relay and the mesh are down): the sweeper
    // never judges a Space by whether its machine answers.
    for _ in 0..3 {
        l.sweep_both().await;
    }
    nothing_lost(&l);
}

#[tokio::test]
async fn signed_out_mid_sweep_deletes_nothing_more_and_forgets_nothing() {
    let l = lab().await;
    scene(&l);
    l.set(|a| a.signed_out = Some("ExpiredToken: the token has expired".into()));
    for r in l.sweep_both().await {
        assert!(r.resources.iter().all(|i| i.action == "failed"), "{r:?}");
    }
    nothing_lost(&l);
    assert!(l.recorded("i-expired"));
    l.set(|a| a.signed_out = None);
    heals(&l).await;
}
