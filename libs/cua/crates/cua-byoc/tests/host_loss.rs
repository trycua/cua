// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A pool host lost mid-batch (its cloud terminated it outside Cua) takes
//! the Spaces it provided with it: a delete of one answers without asking
//! the host (so an owner's retry ends), the scale-in pass forgets the rest
//! and removes the host, and Spaces on the other hosts stay. A host added
//! by address that is removed by hand forgets the Spaces it created too.
//! Against a scripted cloud and a fake relay; nothing leaves the process.

use std::sync::Arc;
use std::time::Duration;

use cua_byoc::model::{Connection, Tier};
use cua_byoc::state::Store;
use cua_byoc::testing::FakeProvider;
use cua_byoc::{CloudApi, RelayAccess};
use cua_host::relay::StaticToken;
use cua_host::testing::FakeRelay;
use cua_proto::daemon::v1 as dpb;
use cua_sandbox_core::Sandboxes;
use cua_sandbox_core::byoc::AddHost;
use cua_spaces::Spaces;
use cua_spaces::registry::Credential;

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

async fn world() -> World {
    test_home();
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("fake", Tier::Vm);
    // A host "boots" by redeeming its pairing code and coming online.
    {
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

/// Records Space `n` as placed on pool host `machine` the routed way, as
/// a create there does: its row names the host, and the host's route
/// lists it.
fn placed(w: &World, machine: &str, n: u16) -> String {
    let host = format!("relay:{machine}");
    let id = format!("direct:{machine}.via.cua:{}", 41000 + n);
    let reg = w.spaces.registry();
    if !reg.direct_hosts().unwrap().iter().any(|h| h.space == host) {
        reg.upsert_direct_host(machine, &host).unwrap();
    }
    reg.upsert(
        dpb::Space {
            id: id.clone(),
            name: format!("rollout-{n}"),
            host: host.clone(),
            ..Default::default()
        },
        Credential {
            token: Some("space-token".into()),
            ..Default::default()
        },
    )
    .unwrap();
    reg.update_direct_hosts(|hosts| {
        let h = hosts.iter_mut().find(|h| h.space == host).unwrap();
        h.spaces.insert(id.clone(), format!("local:rollout-{n}"));
    })
    .unwrap();
    id
}

/// A delete of a forgotten Space succeeds (only forgotten) or finds
/// nothing: either ends an owner's retry, and neither reaches for a host.
async fn assert_ends_without_the_host(w: &World, id: &str) {
    match w.spaces.delete(id).await {
        Ok(m) => assert!(m.contains("only forgotten"), "{m}"),
        Err(e) => assert!(matches!(e, cua_spaces::Error::NotFound(_)), "{e}"),
    }
    assert!(!rows(w).contains(&id.to_string()));
}

fn rows(w: &World) -> Vec<String> {
    let mut ids: Vec<String> = w
        .spaces
        .registry()
        .list()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn a_host_terminated_mid_batch_takes_its_spaces_and_nothing_else() {
    let w = world().await;
    let a = joined_host(&w).await;
    let b = joined_host(&w).await;
    assert_ne!(a, b);
    // A batch spread over both hosts.
    let on_a: Vec<String> = (0..3).map(|n| placed(&w, &a, n)).collect();
    let on_b: Vec<String> = (3..5).map(|n| placed(&w, &b, n)).collect();

    // Mid-batch, host a's cloud terminates it (`aws ec2
    // terminate-instances`): its cloud says "terminated" and its relay
    // entry stops answering. (A host its cloud merely does not list yet,
    // seconds after its create, is still booting: hosts.rs tests that.)
    {
        let mut acct = w.fake.account.lock().unwrap();
        let r = acct
            .resources
            .values_mut()
            .find(|r| r.name.contains(a.trim_start_matches("host-")))
            .expect("host a's machine");
        r.state = "terminated".into();
    }
    w.relay.set_online(&a, false, "0.0.0");

    // Deleting one of its Spaces answers without the host (there is
    // nothing to ask), so a caller's retry ends.
    let msg = w.spaces.delete(&on_a[0]).await.unwrap();
    assert!(msg.contains("is gone"), "{msg}");
    assert!(!rows(&w).contains(&on_a[0]));

    // The scale-in pass forgets the rest and removes the host.
    w.spaces.elastic_reconcile().await.unwrap();
    let left = rows(&w);
    for id in &on_a {
        assert!(!left.contains(id), "{id} left behind: {left:?}");
    }
    for id in &on_b {
        assert!(left.contains(id), "{id} of a live host forgotten: {left:?}");
    }
    let clouds = w.spaces.sandboxes().clouds().unwrap().clone();
    let hosts = clouds.hosts().await.unwrap();
    assert!(
        hosts.iter().all(|h| h.machine != a),
        "lost host still listed: {hosts:?}"
    );
    assert!(hosts.iter().any(|h| h.machine == b));
    assert!(
        !w.spaces
            .registry()
            .direct_hosts()
            .unwrap()
            .iter()
            .any(|h| h.space == format!("relay:{a}"))
    );
    assert!(
        w.relay.machine(&a).is_none(),
        "lost host's relay entry left"
    );
    // A delete the caller retries now ends: nothing is left to reach.
    assert_ends_without_the_host(&w, &on_a[1]).await;
}

#[tokio::test]
async fn removing_a_host_added_by_address_forgets_its_spaces() {
    let w = world().await;
    let host = "direct:10.0.0.5:3211";
    let reg = w.spaces.registry();
    reg.upsert(
        dpb::Space {
            id: host.into(),
            name: "chaoshost".into(),
            ..Default::default()
        },
        Credential {
            token: Some("host-token".into()),
            ..Default::default()
        },
    )
    .unwrap();
    reg.upsert_direct_host("chaoshost", host).unwrap();
    let mut made = Vec::new();
    for n in 0..3u16 {
        let id = format!("direct:10.0.0.5:{}", 41000 + n);
        reg.upsert(
            dpb::Space {
                id: id.clone(),
                name: format!("e2b-{n}"),
                host: host.into(),
                ..Default::default()
            },
            Credential {
                token: Some("space-token".into()),
                ..Default::default()
            },
        )
        .unwrap();
        reg.update_direct_hosts(|h| {
            h[0].spaces.insert(id.clone(), format!("local:e2b-{n}"));
        })
        .unwrap();
        made.push(id);
    }
    // The host is gone (terminated); removing it by hand is what says so.
    w.spaces.remove("chaoshost").await.unwrap();
    assert!(rows(&w).is_empty(), "{:?}", rows(&w));
    assert!(reg.direct_hosts().unwrap().is_empty());
    // A delete of one of its Spaces (a journal's retry) ends instead of
    // failing to reach the host for ever.
    assert_ends_without_the_host(&w, &made[0]).await;
}

/// J-E10: a host deleted outside the scale-in pass (`cua cloud disconnect
/// --delete` deletes its machine, record and relay entry) leaves no Space
/// behind: a delete of one answers without the host, and the next pass
/// forgets the rest.
#[tokio::test]
async fn a_host_deleted_with_its_cloud_link_leaves_no_space_behind() {
    let w = world().await;
    let a = joined_host(&w).await;
    let on_a: Vec<String> = (0..2).map(|n| placed(&w, &a, n)).collect();
    // What disconnect --delete does: the machine, the host record and the
    // relay entry go.
    w.fake.account.lock().unwrap().resources.clear();
    let store = Store::new(w._dir.path());
    for r in store.resources().unwrap() {
        store.forget(&r).unwrap();
    }
    cua_host::RelayClient::new(&w.relay.url)
        .unwrap()
        .delete("acct-token", &a)
        .await
        .unwrap();
    let msg = w.spaces.delete(&on_a[0]).await.unwrap();
    assert!(msg.contains("is gone"), "{msg}");
    w.spaces.elastic_reconcile().await.unwrap();
    assert!(rows(&w).is_empty(), "{:?}", rows(&w));
    assert!(w.spaces.registry().direct_hosts().unwrap().is_empty());
}
