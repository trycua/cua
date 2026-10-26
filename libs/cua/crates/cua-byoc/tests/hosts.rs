// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The elastic pool's host lifecycle (`cua_byoc::hosts`) against a scripted
//! cloud and a fake relay, no network: a host is recorded and tagged before
//! it exists, "boots" by redeeming its one-time pairing code (as `cua host
//! setup --pair` does), is listed joined, cordoned through the relay, and
//! removed with its relay machine; a batch is all or none; pinned types,
//! regions and limits are kept.

use std::sync::Arc;
use std::time::Duration;

use cua_byoc::hosts::{HostOptions, Hosts};
use cua_byoc::model::{self, Connection, Tier};
use cua_byoc::state::Store;
use cua_byoc::testing::FakeProvider;
use cua_byoc::{CloudApi, RelayAccess};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::byoc::AddHost;

struct World {
    _dir: tempfile::TempDir,
    relay: Arc<FakeRelay>,
    fake: Arc<FakeProvider>,
    store: Arc<Store>,
    hosts: Hosts,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("fake", Tier::Vm);
    let store = Arc::new(Store::new(dir.path()));
    store
        .connect(Connection {
            provider: "fake".into(),
            region: "fake-1".into(),
            ..Default::default()
        })
        .unwrap();
    let access = RelayAccess::new(
        relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken("acct-token".into())),
    );
    let hosts = Hosts::new(
        vec![fake.clone() as Arc<dyn CloudApi>],
        store.clone(),
        Some(access),
    );
    World {
        _dir: dir,
        relay,
        fake,
        store,
        hosts,
    }
}

/// When the cloud "boots" a host, it redeems the pairing code in its user
/// data and comes online (what `cua host setup --pair` does on it).
fn hosts_join(w: &World) {
    let relay = w.relay.clone();
    w.fake.on_boot(Arc::new(move |spec, _r| {
        let code = spec
            .user_data
            .split("--pair '")
            .nth(1)
            .and_then(|s| s.split('\'').next())
            .expect("a pairing code in the user data")
            .to_string();
        let relay = relay.clone();
        let id = format!("host-{}", spec.name.trim_start_matches("cua-host-"));
        let id: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
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

fn req(t: &str) -> AddHost {
    AddHost {
        provider: "fake".into(),
        type_name: t.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_host_is_tagged_recorded_joins_and_is_removed_with_its_relay_machine() {
    let w = world().await;
    hosts_join(&w);
    let h = w
        .hosts
        .add(&req("fake.large"), &HostOptions::default())
        .await
        .unwrap();
    assert_eq!(h.name, "fake-1");
    assert_eq!(h.type_name, "fake.large");
    assert_eq!(h.state, "booting");
    assert!((h.usd_per_hour - 0.07).abs() < 1e-9);
    assert_eq!((h.billing_step_secs, h.billing_min_secs), (3600, 3600));
    // Exactly the asked type, as a host, with every tag in the create.
    let spec = w.fake.account.lock().unwrap().specs[0].clone();
    assert_eq!(spec.machine_type, "fake.large");
    assert!(spec.is_host());
    assert_eq!(spec.tags[model::tag::ROLE], "host");
    assert_eq!(spec.tags[model::tag::MANAGED], "true");
    assert!(spec.user_data.contains("--no-desktop --provide-spaces"));
    assert!(spec.user_data.contains("--dedicated"));
    // This cloud bills a stopped machine: no idle power-off, the TTL only.
    assert!(!spec.user_data.contains("cua-idle-poweroff"));
    assert!(spec.user_data.contains("this host reached its TTL"));
    // It joins.
    let joined = w
        .hosts
        .wait_joined(&["fake-1".into()], Duration::from_secs(5), |_| {})
        .await
        .unwrap();
    assert_eq!(joined[0].state, "joined", "{joined:?}");
    let machine = joined[0].machine.clone();
    assert!(!machine.is_empty());
    // Cordoned through the relay (it stops sharing), then opened again.
    w.hosts.cordon("fake-1", true).await.unwrap();
    assert_eq!(w.hosts.list().await.unwrap()[0].state, "cordoned");
    w.hosts.cordon("fake-1", false).await.unwrap();
    assert_eq!(w.hosts.list().await.unwrap()[0].state, "joined");
    // Removed: the machine, its relay machine and its records.
    w.hosts.remove("fake-1").await.unwrap();
    assert!(w.fake.account.lock().unwrap().resources.is_empty());
    assert!(w.relay.machine(&machine).is_none(), "relay machine removed");
    assert!(w.hosts.list().await.unwrap().is_empty());
    assert!(
        w.hosts.remove("fake-1").await.is_err(),
        "not this home's any more"
    );
}

#[tokio::test]
async fn offers_and_limits_follow_the_provider() {
    let w = world().await;
    let offers = w.hosts.offers().await.unwrap();
    // Two types, in the linked region only (fake-1; the cloud also sells
    // fake-2): the pool never puts a machine in a region you did not link.
    assert_eq!(offers.len(), 2, "{offers:#?}");
    assert!(offers.iter().all(|o| o.region == "fake-1"), "{offers:#?}");
    assert!(
        offers
            .iter()
            .all(|o| o.supplies == "hosts" && o.max_new == 5)
    );
    // Unknown type, region not offered: refused before anything is made.
    let e = w
        .hosts
        .add(&req("fake.huge"), &HostOptions::default())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no machine type"), "{e}");
    let mut r = req("fake.small");
    r.region = "elsewhere".into();
    let e = w.hosts.add(&r, &HostOptions::default()).await.unwrap_err();
    assert!(e.to_string().contains("not offered in elsewhere"), "{e}");
    assert!(w.fake.calls().is_empty());
    // A region it offers is honoured.
    r.region = "fake-2".into();
    let h = w.hosts.add(&r, &HostOptions::default()).await.unwrap();
    assert_eq!(h.region, "fake-2");
    // The limit (5) counts what this home runs there.
    for _ in 0..4 {
        w.hosts
            .add(&req("fake.small"), &HostOptions::default())
            .await
            .unwrap();
    }
    let e = w
        .hosts
        .add(&req("fake.small"), &HostOptions::default())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("allows 5 machines"), "{e}");
    let offers = w.hosts.offers().await.unwrap();
    assert!(offers.iter().all(|o| o.max_new == 0), "{offers:#?}");
}

#[tokio::test]
async fn a_batch_is_all_or_none() {
    let w = world().await;
    // The second create fails: the first is removed again.
    let fake = w.fake.clone();
    let n = std::sync::atomic::AtomicU32::new(0);
    let n = Arc::new(n);
    let n2 = n.clone();
    w.fake.on_boot(Arc::new(move |_, _| {
        if n2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            fake.account.lock().unwrap().fail_provision = true;
        }
    }));
    let e = w
        .hosts
        .add_many(&req("fake.small"), 3, &HostOptions::default())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("host 2 of 3"), "{e}");
    assert!(e.to_string().contains("were removed"), "{e}");
    assert!(w.fake.account.lock().unwrap().resources.is_empty());
    assert!(w.hosts.list().await.unwrap().is_empty());
    // No pending record is left behind either.
    assert!(w.store.resources().unwrap().is_empty());
}

#[tokio::test]
async fn a_spaces_supplier_adds_no_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProvider::new("fake", Tier::Sandbox);
    let store = Arc::new(Store::new(dir.path()));
    store
        .connect(Connection {
            provider: "fake".into(),
            region: "fake-1".into(),
            ..Default::default()
        })
        .unwrap();
    let hosts = Hosts::new(vec![fake.clone() as Arc<dyn CloudApi>], store, None);
    let offers = hosts.offers().await.unwrap();
    assert!(offers.iter().all(|o| o.supplies == "spaces"));
    let e = hosts
        .add(&req("fake.small"), &HostOptions::default())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("runs Spaces itself"), "{e}");
    assert!(fake.calls().is_empty());
}

#[tokio::test]
async fn linking_or_unlinking_a_cloud_changes_the_offers_at_once() {
    let w = world().await;
    assert_eq!(w.hosts.offers().await.unwrap().len(), 2);
    // Cached offers never outlive the link they came from.
    w.store.disconnect("fake").unwrap();
    assert!(w.hosts.offers().await.unwrap().is_empty());
    // Linked in another region: the offers move there.
    w.store
        .connect(Connection {
            provider: "fake".into(),
            region: "fake-2".into(),
            ..Default::default()
        })
        .unwrap();
    let offers = w.hosts.offers().await.unwrap();
    assert_eq!(offers.len(), 2);
    assert!(offers.iter().all(|o| o.region == "fake-2"), "{offers:#?}");
}

/// A host its cloud terminated outside Cua (`aws ec2 terminate-instances`)
/// is listed `gone` once its cloud says so, not `offline` for ever; the
/// record says so too, and removing it still ends its relay machine.
#[tokio::test]
async fn a_host_its_cloud_terminated_is_listed_gone() {
    let w = world().await;
    hosts_join(&w);
    w.hosts
        .add(&req("fake.large"), &HostOptions::default())
        .await
        .unwrap();
    let joined = w
        .hosts
        .wait_joined(&["fake-1".into()], Duration::from_secs(5), |_| {})
        .await
        .unwrap();
    let machine = joined[0].machine.clone();
    // Only offline (a partition, a reboot): not lost.
    w.relay.set_online(&machine, false, "0.0.0");
    assert_eq!(w.hosts.list().await.unwrap()[0].state, "offline");
    // Terminated at the cloud (after it was found alive while offline).
    w.fake.account.lock().unwrap().resources.clear();
    // Asked about again only after a while; a fresh view asks at once.
    let fresh = Hosts::new(
        vec![w.fake.clone() as Arc<dyn CloudApi>],
        w.store.clone(),
        Some(RelayAccess::new(
            w.relay.url.clone(),
            Arc::new(cua_host::relay::StaticToken("acct-token".into())),
        )),
    );
    assert_eq!(fresh.list().await.unwrap()[0].state, "gone");
    // Recorded: every view says gone now, with no cloud call.
    assert_eq!(w.hosts.list().await.unwrap()[0].state, "gone");
    fresh.remove("fake-1").await.unwrap();
    assert!(w.relay.machine(&machine).is_none());
    assert!(w.hosts.list().await.unwrap().is_empty());
}
