// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cloud_cost`, budget alerts and `cloud_audit`'s cloud filter, through the
//! cloud manager every surface uses, against a scripted cloud.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cua_byoc::model::{self, Connection, Resource, Tier};
use cua_byoc::testing::FakeProvider;
use cua_byoc::{RelayAccess, Store};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::Sandboxes;
use cua_sandbox_core::byoc::{CloudBudgetAlert, CloudManager};

struct World {
    dir: tempfile::TempDir,
    _relay: Arc<FakeRelay>,
    sandboxes: Sandboxes,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let access = RelayAccess::new(
        relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken("acct-token".into())),
    );
    let sandboxes = cua_byoc::install_with(
        Sandboxes::builder().state_dir(dir.path().join("sandboxes")),
        dir.path(),
        vec![FakeProvider::new("aws", Tier::Vm)],
        access,
    )
    .build();
    World {
        dir,
        _relay: relay,
        sandboxes,
    }
}

fn machine(id: &str, space: &str, who: &str, created: u64, usd: &str) -> Resource {
    let mut r = Resource {
        provider: "aws".into(),
        id: id.into(),
        resource_type: "instance".into(),
        sandbox: format!("aws:{space}"),
        region: "us-west-2".into(),
        created,
        state: "running".into(),
        ..Default::default()
    };
    r.extra.insert("usd_per_hour".into(), usd.into());
    r.extra.insert(model::STARTED_BY.into(), who.into());
    r
}

fn clouds(w: &World) -> Arc<dyn CloudManager> {
    w.sandboxes.clouds().unwrap().clone()
}

#[tokio::test]
async fn cost_counts_machines_still_running_and_ones_already_gone() {
    let w = world().await;
    let store = Store::new(w.dir.path());
    let now = model::now();
    // Gone: ran one hour at $1, two hours ago.
    let mut gone = machine("i-gone", "build", "planner", now - 3 * 3600, "1.0");
    gone.expires = now - 2 * 3600;
    store.record(gone.clone()).unwrap();
    store.forget(&gone).unwrap();
    assert!(w.dir.path().join("cloud/usage.jsonl").exists());
    // Running: two hours at $2.
    store
        .record(machine("i-run", "research", "user", now - 2 * 3600, "2.0"))
        .unwrap();
    // Not a machine: never costed.
    store
        .record(Resource {
            provider: "aws".into(),
            id: "sg-1".into(),
            resource_type: "security_group".into(),
            created: now - 3600,
            ..Default::default()
        })
        .unwrap();

    let since = cua_byoc::usage::day_key(now - 4 * 3600);
    let c = clouds(&w);
    let by_space = c.cost(None, "space", Some(&since)).await.unwrap();
    let keys: Vec<(&str, u32)> = by_space
        .rows
        .iter()
        .map(|r| (r.key.as_str(), r.machines))
        .collect();
    assert_eq!(keys, vec![("research", 1), ("build", 1)], "{by_space:?}");
    assert!((by_space.rows[0].usd - 4.0).abs() < 0.02, "{by_space:?}");
    assert!((by_space.rows[1].usd - 1.0).abs() < 0.02, "{by_space:?}");
    assert!((by_space.total_usd - 5.0).abs() < 0.02);

    let by_agent = c.cost(Some("aws"), "agent", Some(&since)).await.unwrap();
    assert_eq!(by_agent.rows[0].key, "user");
    assert_eq!(by_agent.rows[1].key, "planner");

    let by_cloud = c.cost(None, "cloud", Some(&since)).await.unwrap();
    assert_eq!(by_cloud.rows.len(), 1);
    assert_eq!(by_cloud.rows[0].machines, 2);

    // Refusals name what is wrong.
    let e = c.cost(None, "week", None).await.unwrap_err();
    assert!(e.to_string().contains("space, agent, day, cloud"), "{e}");
    let e = c.cost(None, "day", Some("2026-02-30")).await.unwrap_err();
    assert!(e.to_string().contains("not a day"), "{e}");
    assert!(c.cost(Some("nope"), "day", None).await.is_err());
}

/// Alerts the sink saw, per cua home (tests share the process-wide sink).
fn sink() -> &'static Mutex<Vec<(PathBuf, CloudBudgetAlert)>> {
    static SEEN: std::sync::OnceLock<Mutex<Vec<(PathBuf, CloudBudgetAlert)>>> =
        std::sync::OnceLock::new();
    SEEN.get_or_init(|| {
        cua_sandbox_core::byoc::set_budget_alert_sink(Arc::new(|home, a| {
            sink().lock().unwrap().push((home.to_path_buf(), a.clone()));
        }));
        Mutex::new(Vec::new())
    })
}

fn seen(home: &std::path::Path) -> Vec<u32> {
    sink()
        .lock()
        .unwrap()
        .iter()
        .filter(|(h, _)| h == home)
        .map(|(_, a)| a.percent)
        .collect()
}

#[tokio::test]
async fn a_budget_share_is_said_once_through_status_and_the_audit_log() {
    let _ = sink();
    let w = world().await;
    let store = Store::new(w.dir.path());
    let now = model::now();
    store
        .record(machine("i-run", "research", "user", now - 2 * 3600, "2.0"))
        .unwrap();
    let spent = store.spent_month("aws", now).unwrap();
    assert!(spent > 0.0);
    // Spent is about 85% of the budget.
    store
        .connect(Connection {
            provider: "aws".into(),
            region: "us-west-2".into(),
            budget_usd_month: Some(spent / 0.85),
            ..Default::default()
        })
        .unwrap();
    let c = clouds(&w);
    c.status(None).await.unwrap();
    assert_eq!(seen(w.dir.path()), vec![80]);
    // Again, and from a fresh process's view: not said twice.
    c.status(None).await.unwrap();
    cua_byoc::alerts::check_all(&Store::new(w.dir.path()), model::now());
    assert_eq!(seen(w.dir.path()), vec![80]);
    let log = c.audit_for(Some("aws"), 50).await.unwrap();
    let alerts: Vec<_> = log.iter().filter(|e| e.action == "budget_alert").collect();
    assert_eq!(alerts.len(), 1, "{log:?}");
    assert!(alerts[0].detail.starts_with("80% of $"), "{:?}", alerts[0]);
    assert!(log.iter().all(|e| e.source == "device"));
}

#[tokio::test]
async fn no_budget_no_alerts() {
    let _ = sink();
    let w = world().await;
    let store = Store::new(w.dir.path());
    let now = model::now();
    store
        .record(machine("i-run", "research", "user", now - 2 * 3600, "50.0"))
        .unwrap();
    store
        .connect(Connection {
            provider: "aws".into(),
            ..Default::default()
        })
        .unwrap();
    clouds(&w).status(None).await.unwrap();
    assert!(seen(w.dir.path()).is_empty());
    assert!(!w.dir.path().join("cloud/alerts.json").exists());
}

#[tokio::test]
async fn audit_narrows_to_one_cloud() {
    let w = world().await;
    let store = Store::new(w.dir.path());
    store.audit(cua_byoc::audit::entry("connect", "aws", "user"));
    store.audit(cua_byoc::audit::entry("connect", "gcp", "user"));
    let c = clouds(&w);
    let all = c.audit_for(None, 50).await.unwrap();
    assert_eq!(all.len(), 2);
    // `gcp` is not a cloud this build has: refused, not silently empty.
    assert!(c.audit_for(Some("gcp"), 50).await.is_err());
    let aws = c.audit_for(Some("aws"), 50).await.unwrap();
    assert_eq!(aws.len(), 1);
    assert_eq!(aws[0].provider, "aws");
}
