// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The whole cloud flow against a scripted cloud and the fake relay,
//! through the sandbox layer: connect, create (the guest "joins" the relay
//! when the cloud boots it), find, stop and start, delete, rollback of a
//! failed create, and the sweeper leaving everything Cua did not create
//! exactly as it was.

use std::collections::BTreeMap;
use std::sync::Arc;

use cua_byoc::model::{self, Resource, Tier};
use cua_byoc::testing::FakeProvider;
use cua_byoc::{RelayAccess, Target};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::byoc::DETAIL_RELAY_MACHINE;
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};

struct World {
    dir: tempfile::TempDir,
    relay: Arc<FakeRelay>,
    sandboxes: Sandboxes,
    fake: Arc<FakeProvider>,
}

async fn world(tier: Tier) -> World {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("aws", tier);
    let access = RelayAccess::new(
        relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken("acct-token".into())),
    );
    let sandboxes = cua_byoc::install_with(
        Sandboxes::builder().state_dir(dir.path().join("sandboxes")),
        dir.path(),
        vec![fake.clone()],
        access,
    )
    .build();
    World {
        dir,
        relay,
        sandboxes,
        fake,
    }
}

fn aws() -> Target {
    Target {
        provider: "aws".into(),
        region: Some("us-west-2".into()),
        ..Default::default()
    }
}

/// When the cloud "boots" a machine, its cua-spacesd joins the relay.
fn guest_joins(w: &World) {
    let relay = w.relay.clone();
    w.fake.on_boot(Arc::new(move |spec, _| {
        relay.set_online(&spec.tags[model::tag::MACHINE], true, "0.1.0");
    }));
}

/// An image the registry lookup cannot resolve (hermetic: no network), run
/// as given; the family is Linux.
const IMAGE: &str = "registry.invalid/trycua/linux:24.04";

fn create(name: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Contrib, IMAGE);
    o.contrib = Some("aws".into());
    o.name = Some(name.into());
    o
}

#[tokio::test]
async fn a_cloud_sandbox_is_created_found_stopped_started_and_deleted() {
    let w = world(Tier::Vm).await;
    guest_joins(&w);
    // Not connected yet: says how to connect, creates nothing.
    let err = w.sandboxes.create(create("research")).await.unwrap_err();
    assert!(err.to_string().contains("cua cloud connect aws"), "{err}");
    assert!(w.fake.calls().is_empty());

    let clouds = w.sandboxes.clouds().unwrap();
    let c = clouds.connect(&aws(), false, Some(2)).await.unwrap();
    assert!(c.provider.connected);
    assert_eq!(c.provider.label, "Fake \u{b7} us-west-2");
    assert_eq!(c.provider.ttl_hours, 2);

    let sb = w.sandboxes.create(create("research")).await.unwrap();
    let details = sb.provider_details();
    let machine = details[DETAIL_RELAY_MACHINE].clone();
    assert!(machine.starts_with("cloud-"), "{machine}");
    // The VM was told to run the image joined to the relay, and end itself.
    let spec = w.fake.account.lock().unwrap().specs[0].clone();
    assert!(spec.user_data.starts_with("#cloud-config\n"));
    assert!(!spec.user_data.contains("cmt_"), "no token in clear text");
    assert_eq!(spec.tags[model::tag::MANAGED], "true");
    assert_eq!(spec.tags[model::tag::SPACE], "research");
    assert_eq!(spec.tags[model::tag::MACHINE], machine);
    assert_eq!(spec.ttl_secs, 2 * 3600);
    let owner = w.sandboxes.clouds().unwrap();
    let status = owner.status(None).await.unwrap();
    assert_eq!(status.resources.len(), 1);
    assert_eq!(status.resources[0].sandbox, "aws:research");
    assert_eq!(status.resources[0].machine, machine);

    // A listing from state shows where it runs and when it expires.
    let saved = w.sandboxes.persisted_details("research");
    assert_eq!(saved["place"], "Fake \u{b7} us-west-2");
    assert!(saved.contains_key("expires") && saved.contains_key("machine_type"));
    assert_eq!(
        w.sandboxes.cloud_of_relay_machine(&machine),
        Some(("aws".to_string(), "Fake \u{b7} us-west-2".to_string()))
    );

    // Spaces finds it by its relay machine.
    assert_eq!(
        w.sandboxes.by_relay_machine(&machine).unwrap().as_deref(),
        Some("research")
    );

    // Stop and start through the sandbox API.
    w.sandboxes.suspend("research").await.unwrap();
    assert!(w.fake.calls().iter().any(|c| c.starts_with("stop fake-")));
    w.sandboxes.resume("research").await.unwrap();
    assert!(w.fake.calls().iter().any(|c| c.starts_with("start fake-")));

    // Delete: the instance and the relay machine go, the record too.
    w.sandboxes.delete("research").await.unwrap();
    assert!(w.fake.calls().iter().any(|c| c.starts_with("delete fake-")));
    assert!(w.relay.machine(&machine).is_none());
    let status = w.sandboxes.clouds().unwrap().status(None).await.unwrap();
    assert!(status.resources.is_empty());
    assert!(w.fake.account.lock().unwrap().resources.is_empty());
}

#[tokio::test]
async fn a_failed_create_leaves_nothing_behind() {
    let w = world(Tier::Vm).await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), false, None).await.unwrap();
    // The cloud refuses: nothing recorded, the relay machine forgotten,
    // the error is the cloud's.
    w.fake.account.lock().unwrap().fail_provision = true;
    let err = w.sandboxes.create(create("a")).await.unwrap_err();
    assert!(matches!(err, cua_sandbox_core::Error::Cloud(_)), "{err}");
    assert!(clouds.status(None).await.unwrap().resources.is_empty());

    // A guest that never joins (no boot hook) and a machine that dies:
    // the instance is deleted and nothing is left.
    w.fake.on_boot(Arc::new({
        let acct = w.fake.account.clone();
        move |_, r| {
            acct.lock().unwrap().resources.get_mut(&r.id).unwrap().state = "terminated".into();
        }
    }));
    let err = w.sandboxes.create(create("b")).await.unwrap_err();
    assert!(err.to_string().contains("terminated"), "{err}");
    assert!(w.fake.calls().iter().any(|c| c.starts_with("delete fake-")));
    assert!(w.fake.account.lock().unwrap().resources.is_empty());
    assert!(clouds.status(None).await.unwrap().resources.is_empty());

    // An image family the cloud cannot run is refused before any call.
    let before = w.fake.calls().len();
    let mut mac = create("c");
    mac.image = "registry.invalid/trycua/macos:26".into();
    let err = w.sandboxes.create(mac).await.unwrap_err();
    assert!(err.to_string().contains("not offered"), "{err}");
    assert_eq!(w.fake.calls().len(), before);
}

fn inventory(w: &World) -> BTreeMap<String, Resource> {
    w.fake.account.lock().unwrap().resources.clone()
}

#[tokio::test]
async fn the_sweeper_touches_only_what_this_owner_tagged_and_recorded() {
    let w = world(Tier::Vm).await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), false, None).await.unwrap();
    let store = cua_byoc::Store::new(w.dir.path());
    let owner = store.owner_id().unwrap();
    let t = model::now();
    let old = t - 3600 * 24;
    let put = |id: &str, ty: &str, tags: BTreeMap<String, String>| {
        w.fake.preexisting(Resource {
            provider: "aws".into(),
            id: id.into(),
            resource_type: ty.into(),
            name: id.into(),
            created: old,
            tags,
            state: "running".into(),
            ..Default::default()
        });
    };
    // What the account already had: untagged, another owner's (even
    // expired), one that only looks like Cua's by name, a default group.
    put("i-untagged", "instance", BTreeMap::new());
    put(
        "i-other-owner",
        "instance",
        model::tags("someone-else", "x", "cloud-x", t - 10),
    );
    put(
        "cua-aws-lookalike",
        "instance",
        BTreeMap::from([("Name".to_string(), "cua-aws-1".to_string())]),
    );
    put("sg-default", "security_group", BTreeMap::new());
    let untouched = inventory(&w);

    // Cua's own: one expired, one orphan (no record), one live with its
    // sandbox recorded, and the shared security group.
    // The expired one's relay machine is registered (it joined once).
    RelayAccess::new(
        w.relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken("acct-token".into())),
    )
    .register("cloud-00000000000000c1", "s1")
    .await
    .unwrap();
    put(
        "i-expired",
        "instance",
        model::tags(&owner, "s1", "cloud-00000000000000c1", t - 1),
    );
    put(
        "i-orphan",
        "instance",
        model::tags(&owner, "s2", "c2", t + 3600),
    );
    put(
        "i-live",
        "instance",
        model::tags(&owner, "s3", "c3", t + 3600),
    );
    put("sg-cua", "security_group", model::tags(&owner, "", "", 0));
    store
        .record(Resource {
            provider: "aws".into(),
            id: "i-live".into(),
            resource_type: "instance".into(),
            sandbox: "aws:s3".into(),
            state: "running".into(),
            created: old,
            expires: t + 3600,
            ..Default::default()
        })
        .unwrap();

    // Dry run: says what it would do, changes nothing.
    let before = inventory(&w);
    let r = clouds.sweep(Some("aws"), true, false).await.unwrap();
    assert!(r.dry_run);
    let action = |id: &str| {
        r.resources
            .iter()
            .find(|i| i.resource.id == id)
            .map(|i| i.action.clone())
    };
    assert_eq!(action("i-expired").as_deref(), Some("delete"));
    assert_eq!(action("i-orphan").as_deref(), Some("delete"));
    assert_eq!(action("i-live").as_deref(), Some("keep"));
    assert_eq!(action("sg-cua").as_deref(), Some("keep"));
    for id in untouched.keys() {
        assert_eq!(action(id), None, "{id} is not even listed");
    }
    assert_eq!(inventory(&w), before, "a dry run changes nothing");

    // For real: only Cua's expired and orphaned instances go.
    clouds.sweep(Some("aws"), false, false).await.unwrap();
    let after = inventory(&w);
    assert!(!after.contains_key("i-expired") && !after.contains_key("i-orphan"));
    assert!(after.contains_key("i-live") && after.contains_key("sg-cua"));
    for (id, r) in &untouched {
        assert_eq!(after.get(id), Some(r), "{id} unchanged");
    }

    // Deleting a sandbox's machine also removes its relay machine.
    assert!(w.relay.machine("cloud-00000000000000c1").is_none());
    assert!(
        !store
            .resources()
            .unwrap()
            .iter()
            .any(|r| r.resource_type == model::RELAY_MACHINE)
    );

    // all: every Cua resource, the shared group last; still nothing else.
    clouds.sweep(None, false, true).await.unwrap();
    assert_eq!(
        inventory(&w),
        untouched,
        "only what the account already had"
    );
    let deletes: Vec<String> = w
        .fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("delete "))
        .collect();
    assert_eq!(
        deletes,
        vec![
            "delete i-expired",
            "delete i-orphan",
            "delete i-live",
            "delete sg-cua"
        ]
    );
}

#[tokio::test]
async fn connect_can_make_it_the_default_and_disconnect_forgets_it() {
    let w = world(Tier::Vm).await;
    let clouds = w.sandboxes.clouds().unwrap();
    let c = clouds.connect(&aws(), true, None).await.unwrap();
    assert!(c.provider.default);
    let cfg = std::fs::read_to_string(w.dir.path().join("config.toml")).unwrap();
    assert!(cfg.contains("on = \"aws\""), "{cfg}");
    let s = clouds.status(Some("aws")).await.unwrap();
    assert_eq!(s.default_on, "aws");
    assert!(s.providers[0].connected && s.providers[0].credentials.found);
    let d = clouds.disconnect("aws").await.unwrap();
    assert!(d.disconnected);
    let cfg = std::fs::read_to_string(w.dir.path().join("config.toml")).unwrap_or_default();
    assert!(!cfg.contains("aws"), "{cfg}");
    assert!(!clouds.status(None).await.unwrap().providers[0].connected);
    let err = clouds.status(Some("nope")).await.unwrap_err();
    assert!(matches!(err, cua_sandbox_core::Error::InvalidArgument(_)));
}

#[tokio::test]
async fn the_sweeper_removes_relay_machines_a_failed_create_left() {
    let w = world(Tier::Vm).await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), false, None).await.unwrap();
    // A machine registered for a create whose rollback could not reach the
    // relay: recorded, not leaked.
    let access = RelayAccess::new(
        w.relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken("acct-token".into())),
    );
    access
        .register("cloud-00000000000000aa", "left")
        .await
        .unwrap();
    let store = cua_byoc::Store::new(w.dir.path());
    store
        .record(Resource {
            provider: "aws".into(),
            id: "cloud-00000000000000aa".into(),
            resource_type: model::RELAY_MACHINE.into(),
            name: "cloud-00000000000000aa".into(),
            state: "orphaned".into(),
            ..Default::default()
        })
        .unwrap();
    let dry = clouds.sweep(Some("aws"), true, false).await.unwrap();
    let row = dry
        .resources
        .iter()
        .find(|i| i.resource.id == "cloud-00000000000000aa")
        .unwrap();
    assert_eq!(row.action, "delete");
    assert!(w.relay.machine("cloud-00000000000000aa").is_some());
    clouds.sweep(Some("aws"), false, false).await.unwrap();
    assert!(w.relay.machine("cloud-00000000000000aa").is_none());
    assert!(store.resources().unwrap().is_empty());
}
