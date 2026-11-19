// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A create without a location overflows from this machine into a linked
//! cloud only when a budget is set (and `placement.overflow` is on), goes
//! to cloud machines only, and stops at the budget with the reason. Against
//! a scripted cloud; nothing leaves the process.

use std::sync::Arc;

use cua_byoc::model::{Connection, Tier};
use cua_byoc::state::Store;
use cua_byoc::testing::FakeProvider;
use cua_byoc::{CloudApi, RelayAccess};
use cua_host::relay::StaticToken;
use cua_host::testing::FakeRelay;
use cua_sandbox_core::Sandboxes;
use cua_spaces::Spaces;
use cua_spaces::elastic::ElasticRequest;

struct World {
    _dir: tempfile::TempDir,
    _relay: Arc<FakeRelay>,
    spaces: Spaces,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("fake", Tier::Vm);
    let token = Arc::new(StaticToken("acct-token".into()));
    let sandboxes = cua_byoc::install_with(
        Sandboxes::builder().state_dir(dir.path().join("sandboxes")),
        dir.path(),
        vec![fake.clone() as Arc<dyn CloudApi>],
        RelayAccess::new(relay.url.clone(), token),
    )
    .build();
    Store::new(dir.path())
        .connect(Connection {
            provider: "fake".into(),
            region: "fake-1".into(),
            ..Default::default()
        })
        .unwrap();
    let spaces = Spaces::builder()
        .home(dir.path())
        .sandboxes(sandboxes)
        .build();
    World {
        _dir: dir,
        _relay: relay,
        spaces,
    }
}

fn burst(n: u32) -> ElasticRequest {
    ElasticRequest {
        image: "ghcr.io/trycua/linux:24.04-slim".into(),
        kind: "container".into(),
        cpus: Some(1),
        memory_mb: Some(512),
        count: n,
        clouds_only: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn overflow_needs_a_budget_goes_to_clouds_only_and_stops_at_the_budget() {
    let w = world().await;
    // Settings come from this test's home, never the user's.
    // SAFETY: the only test in this binary; nothing else reads the env now.
    unsafe {
        std::env::set_var("CUA_HOME", w._dir.path());
        std::env::remove_var("CUA_PLACEMENT_OVERFLOW");
    }
    // A linked cloud but no budget set: a create without a location stays here.
    assert!(!w.spaces.overflow_allowed().await);
    let mut p = w.spaces.elastic_policy();
    p.budget_set = true;
    p.max_usd_per_hour = 4.0;
    w.spaces.set_elastic_policy(&p).unwrap();
    assert!(w.spaces.overflow_allowed().await);
    // `placement.overflow off`: this machine only, whatever the budget.
    unsafe { std::env::set_var("CUA_PLACEMENT_OVERFLOW", "off") };
    assert!(!w.spaces.overflow_allowed().await);
    unsafe { std::env::remove_var("CUA_PLACEMENT_OVERFLOW") };

    // The overflow solve places on cloud machines only, in the region you
    // linked (the cloud also sells fake-2).
    let plan = w.spaces.elastic_plan(&[burst(6)]).await.unwrap().plan;
    assert!(plan.unplaced.is_empty(), "{plan:#?}");
    assert!(
        plan.new_hosts.iter().all(|h| h.region == "fake-1"),
        "{plan:#?}"
    );
    assert!(!plan.assignments.is_empty());
    for a in &plan.assignments {
        let t = serde_json::to_value(&a.target).unwrap();
        assert_eq!(t["on"], "new", "{a:?}");
    }

    // A burst of checkpoint forks with no location (E2B sandboxes from a
    // snapshot) that this machine has no room for: the same overflow solve,
    // whole groups on Linux KVM cloud machines only.
    let forks = ElasticRequest {
        kind: "vm".into(),
        cpus: Some(1),
        memory_mb: Some(2048),
        count: 12,
        whole: true,
        caller: cua_cluster::elastic::detect::Caller::Batch,
        arch: "arm64".into(),
        clouds_only: true,
        ..Default::default()
    };
    let plan = w.spaces.elastic_plan(&[forks]).await.unwrap().plan;
    assert!(plan.unplaced.is_empty(), "{plan:#?}");
    assert!(!plan.new_hosts.is_empty(), "{plan:#?}");
    for a in &plan.assignments {
        let t = serde_json::to_value(&a.target).unwrap();
        assert_eq!(t["on"], "new", "{a:?}");
    }

    // Budget exhausted: nothing is bought, and the reason says so.
    p.max_usd_per_hour = 0.0;
    w.spaces.set_elastic_policy(&p).unwrap();
    let plan = w.spaces.elastic_plan(&[burst(6)]).await.unwrap().plan;
    assert!(plan.new_hosts.is_empty(), "{plan:#?}");
    assert!(
        plan.unplaced.iter().any(|u| u.reason.contains("cost cap")),
        "{:?}",
        plan.unplaced
    );
}
