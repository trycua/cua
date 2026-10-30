// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The elastic pool's host lifecycle gaps found by the just-works audit,
//! against a scripted cloud and a fake relay (nothing leaves the process):
//! a core limit teaches "fewer vCPUs" and the same create tries a smaller
//! type; `cua cloud status` shows the state the pool sees and prices a type
//! the linked region sells; a host that never joined and an expired host
//! that stopped are removed by the reconcile pass; hosts are audited; and a
//! sweep that deletes a host forgets its relay machine too.

use std::sync::Arc;
use std::time::Duration;

use cua_byoc::model::HostType;
use cua_byoc::model::{Connection, Tier};
use cua_byoc::state::Store;
use cua_byoc::testing::FakeProvider;
use cua_byoc::{CloudApi, RelayAccess};
use cua_host::relay::StaticToken;
use cua_host::testing::FakeRelay;
use cua_sandbox_core::Sandboxes;
use cua_sandbox_core::byoc::{AddHost, CloudManager};
use cua_sandbox_core::placement::On;
use cua_spaces::{SpaceCreate, Spaces};

struct World {
    _dir: tempfile::TempDir,
    relay: Arc<FakeRelay>,
    fake: Arc<FakeProvider>,
    spaces: Spaces,
}

/// Settings come from a test home, never the user's (one for the binary:
/// the environment is shared by its tests).
fn test_home() {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: set once, before any test of this binary reads it.
        unsafe { std::env::set_var("CUA_HOME", home.path()) };
        home
    });
}

async fn world(join: bool) -> World {
    test_home();
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("fake", Tier::Vm);
    // A host "boots" by redeeming its pairing code and coming online.
    if join {
        let relay = relay.clone();
        fake.on_boot(Arc::new(move |spec, _| {
            let code = spec
                .user_data
                .split("--pair '")
                .nth(1)
                .and_then(|s| s.split('\'').next())
                .expect("a pairing code in the user data")
                .to_string();
            let id: String = format!("host-{}", spec.name.trim_start_matches("cua-host-"))
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            let relay = relay.clone();
            tokio::spawn(async move {
                let client = cua_host::RelayClient::new(&relay.url).unwrap();
                client
                    .register(
                        &format!(
                            "{}{}",
                            cua_host::pairing::PAIRING_TOKEN_PREFIX,
                            code.replace('-', "")
                        ),
                        &cua_host::relay::RegisterRequest {
                            id: id.clone(),
                            name: "host".into(),
                            allow: vec![],
                            host: None,
                            meta: Default::default(),
                        },
                    )
                    .await
                    .unwrap();
                relay.set_online(&id, true, "0.0.0");
            });
        }));
    }
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
        .relay(cua_spaces::relay::RelayAccount::new(
            relay.url.clone(),
            Arc::new(cua_spaces::relay::StaticToken("acct-token".into())),
        ))
        .build();
    World {
        _dir: dir,
        relay,
        fake,
        spaces,
    }
}

/// Adds a pool host and waits until it joined; returns its relay machine.
async fn joined_host(w: &World) -> String {
    let clouds = w.spaces.sandboxes().clouds().unwrap().clone();
    let h = clouds
        .add_host(&AddHost {
            provider: "fake".into(),
            type_name: "fake.large".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    for _ in 0..100 {
        if let Some(j) = clouds
            .hosts()
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.name == h.name && c.state == "joined")
        {
            return j.machine;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{} did not join", h.name);
}

fn ht(name: &str, vcpus: u32, usd: f64, regions: &[&str]) -> HostType {
    HostType {
        name: name.into(),
        arch: "amd64".into(),
        vcpus,
        memory_mb: u64::from(vcpus) * 2048,
        disk_gb: 60,
        usd_per_hour: usd,
        regions: regions.iter().map(|r| r.to_string()).collect(),
        boot_secs: 30,
        quota_pool: "shared".into(),
        ..Default::default()
    }
}

fn clouds(w: &World) -> Arc<dyn CloudManager> {
    w.spaces.sandboxes().clouds().unwrap().clone()
}

/// Rewrites the host record of this home (age and expiry), as time passing.
fn age_host(w: &World, created_ago: u64, expires_in: i64) {
    let store = Store::new(w._dir.path());
    let mut r = store
        .resources()
        .unwrap()
        .into_iter()
        .find(|r| r.extra.contains_key("host"))
        .expect("a host record");
    let now = cua_byoc::model::now();
    r.created = now - created_ago;
    r.expires = (now as i64 + expires_in) as u64;
    store.record(r).unwrap();
}

fn audit_actions(w: &World) -> Vec<String> {
    Store::new(w._dir.path())
        .audit_entries(1000)
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect()
}

#[tokio::test]
async fn a_core_limit_teaches_fewer_vcpus_and_the_same_create_tries_a_smaller_type() {
    let w = world(true).await;
    {
        let mut a = w.fake.account.lock().unwrap();
        a.types = Some(vec![
            ht("fake.large", 16, 0.01, &["fake-1"]),
            ht("fake.mid", 8, 0.02, &["fake-1"]),
        ]);
        a.core_limit = Some(8);
    }
    let spaces = w.spaces.clone();
    let create = tokio::spawn(async move {
        spaces
            .create(SpaceCreate {
                on: Some(On::Auto),
                image: Some("ghcr.io/trycua/linux:24.04-slim".into()),
                ..Default::default()
            })
            .await
    });
    let mut types = Vec::new();
    for _ in 0..600 {
        types = w
            .fake
            .account
            .lock()
            .unwrap()
            .specs
            .iter()
            .map(|s| s.machine_type.clone())
            .collect();
        if types.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    create.abort();
    // The 16 vCPU type was refused; the same call went on to the 8 vCPU one.
    assert_eq!(types[..2], ["fake.large", "fake.mid"], "{types:?}");
    // It learned "fewer than 16" for the pool, not "none".
    let q: serde_json::Value = serde_json::from_slice(
        &std::fs::read(w._dir.path().join("elastic").join("quotas.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(q["pools"]["fake/shared"][0], 15, "{q}");
}

#[tokio::test]
async fn status_shows_the_state_the_pool_sees_and_prices_a_type_the_region_sells() {
    let w = world(true).await;
    {
        let mut a = w.fake.account.lock().unwrap();
        a.types = Some(vec![
            ht("fake.cheap", 2, 0.001, &["fake-2"]),
            ht("fake.small", 2, 0.02, &["fake-1"]),
        ]);
        a.estimate = Some(("fake.cheap".into(), 0.001));
    }
    let machine = {
        let c = clouds(&w);
        c.add_host(&AddHost {
            provider: "fake".into(),
            type_name: "fake.small".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        for _ in 0..100 {
            if let Some(h) = c.hosts().await.unwrap().first()
                && h.state == "joined"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        c.hosts().await.unwrap()[0].machine.clone()
    };
    let st = clouds(&w).status(None).await.unwrap();
    let host = st
        .resources
        .iter()
        .find(|r| r.resource_type == "instance")
        .unwrap();
    assert_eq!(host.state, "joined", "{host:?}");
    // The relay says it is offline now: status follows.
    w.relay.set_online(&machine, false, "0.0.0");
    let st = clouds(&w).status(None).await.unwrap();
    assert_eq!(
        st.resources
            .iter()
            .find(|r| r.resource_type == "instance")
            .unwrap()
            .state,
        "offline"
    );
    // The price line quotes the type sold in the linked region.
    let fake = st.providers.iter().find(|p| p.name == "fake").unwrap();
    let k = fake
        .kinds
        .iter()
        .find(|k| k.supported && k.kind == "container")
        .unwrap();
    assert_eq!(k.machine_type, "fake.small", "{k:?}");
    assert!((k.usd_per_hour - 0.02).abs() < 1e-9);
}

#[tokio::test]
async fn a_host_that_never_joined_is_removed_with_its_pairing_code_and_ledgered() {
    let w = world(false).await;
    clouds(&w)
        .add_host(&AddHost {
            provider: "fake".into(),
            type_name: "fake.small".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let code = w.fake.account.lock().unwrap().specs[0]
        .user_data
        .split("--pair '")
        .nth(1)
        .and_then(|s| s.split('\'').next())
        .unwrap()
        .to_string();
    // Young: left alone to boot.
    w.spaces.elastic_reconcile().await.unwrap();
    assert_eq!(clouds(&w).hosts().await.unwrap().len(), 1);
    // Past the join wait: removed.
    age_host(&w, cua_spaces::elastic::JOIN_WAIT.as_secs() + 60, 3600);
    w.spaces.elastic_reconcile().await.unwrap();
    assert!(clouds(&w).hosts().await.unwrap().is_empty());
    assert!(w.fake.account.lock().unwrap().resources.is_empty());
    assert!(w.fake.calls().iter().any(|c| c.starts_with("delete ")));
    // Its one-time code no longer pairs.
    let client = cua_host::RelayClient::new(&w.relay.url).unwrap();
    let r = client
        .register(
            &format!(
                "{}{}",
                cua_host::pairing::PAIRING_TOKEN_PREFIX,
                code.replace('-', "")
            ),
            &cua_host::relay::RegisterRequest {
                id: "late-host".into(),
                name: "host".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await;
    assert!(r.is_err(), "the revoked code paired a host");
    let ledger = w.spaces.elastic_ledger();
    assert!(
        ledger
            .iter()
            .any(|l| l.event == "remove" && l.reason.contains("never joined")),
        "{ledger:?}"
    );
}

#[tokio::test]
async fn an_expired_host_that_stopped_is_deleted_because_a_stopped_host_still_bills() {
    let w = world(true).await;
    let machine = joined_host(&w).await;
    // Past its TTL the machine powered itself off: the relay lost it.
    age_host(&w, 7200, -60);
    w.relay.set_online(&machine, false, "0.0.0");
    w.spaces.elastic_reconcile().await.unwrap();
    assert!(clouds(&w).hosts().await.unwrap().is_empty());
    assert!(w.fake.account.lock().unwrap().resources.is_empty());
    assert!(
        w.spaces
            .elastic_ledger()
            .iter()
            .any(|l| l.event == "remove" && l.reason.contains("past its TTL"))
    );
}

#[tokio::test]
async fn hosts_are_audited_and_a_sweep_that_deletes_one_forgets_its_relay_machine() {
    let w = world(true).await;
    let machine = joined_host(&w).await;
    assert!(w.relay.machine(&machine).is_some());
    let actions = audit_actions(&w);
    for a in ["host_recorded", "host_created"] {
        assert!(actions.iter().any(|x| x == a), "{a} in {actions:?}");
    }
    // `cloud sweep --all --delete` deletes the machine and its record; the
    // relay machine must not stay behind as an offline entry.
    let report = clouds(&w).sweep(Some("fake"), false, true).await.unwrap();
    assert!(
        report.resources.iter().any(|r| r.action == "deleted"),
        "{report:?}"
    );
    assert!(w.relay.machine(&machine).is_none(), "relay machine left");
    // A host removed the pool's way is audited too.
    let machine = joined_host(&w).await;
    let h = clouds(&w).hosts().await.unwrap();
    let name = h
        .iter()
        .find(|h| h.machine == machine)
        .unwrap()
        .name
        .clone();
    clouds(&w).remove_host(&name).await.unwrap();
    assert!(audit_actions(&w).iter().any(|x| x == "host_removed"));
}
