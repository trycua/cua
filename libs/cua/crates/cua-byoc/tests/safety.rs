// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Disconnect never orphans what Cua created, the automatic sweep deletes
//! only what `cloud sweep --delete` would, and the audit log records what
//! happened, all against the scripted cloud.

use std::collections::BTreeMap;
use std::sync::Arc;

use cua_byoc::model::{self, Resource, Tier};
use cua_byoc::testing::FakeProvider;
use cua_byoc::{Clouds, RelayAccess, Store, Target};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::byoc::{CloudAuditEntry, CloudManager, DETAIL_RELAY_MACHINE};
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};

const TOKEN: &str = "acct-token-0123456789";

struct World {
    dir: tempfile::TempDir,
    relay: Arc<FakeRelay>,
    sandboxes: Sandboxes,
    fake: Arc<FakeProvider>,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("aws", Tier::Vm);
    let access = RelayAccess::new(
        relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken(TOKEN.into())),
    );
    let sandboxes = cua_byoc::install_with(
        Sandboxes::builder().state_dir(dir.path().join("sandboxes")),
        dir.path(),
        vec![fake.clone()],
        access,
    )
    .build();
    let w = World {
        dir,
        relay,
        sandboxes,
        fake,
    };
    let relay = w.relay.clone();
    w.fake.on_boot(Arc::new(move |spec, _| {
        relay.set_online(&spec.tags[model::tag::MACHINE], true, "0.1.0");
    }));
    w
}

fn aws() -> Target {
    Target {
        provider: "aws".into(),
        region: Some("us-west-2".into()),
        ..Default::default()
    }
}

fn create(name: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Contrib, "registry.invalid/trycua/linux:24.04");
    o.contrib = Some("aws".into());
    o.name = Some(name.into());
    o
}

fn inventory(w: &World) -> BTreeMap<String, Resource> {
    w.fake.account.lock().unwrap().resources.clone()
}

fn audit(w: &World) -> Vec<CloudAuditEntry> {
    Store::new(w.dir.path()).audit_entries(1000).unwrap()
}

/// `(action, actor, outcome)` of every audit line.
fn trail(w: &World) -> Vec<(String, String, String)> {
    audit(w)
        .into_iter()
        .map(|e| (e.action, e.actor, e.outcome))
        .collect()
}

fn has(w: &World, action: &str, actor: &str, outcome: &str) -> bool {
    trail(w)
        .iter()
        .any(|(a, b, c)| a == action && b == actor && c == outcome)
}

/// Puts a resource Cua tagged for this home's owner (or `owner`).
fn put(w: &World, id: &str, tags: BTreeMap<String, String>, created: u64) {
    w.fake.preexisting(Resource {
        provider: "aws".into(),
        id: id.into(),
        resource_type: "instance".into(),
        name: id.into(),
        created,
        tags,
        state: "running".into(),
        ..Default::default()
    });
}

// -- A: disconnect never orphans ---------------------------------------

#[tokio::test]
async fn disconnect_refuses_while_cua_resources_run_and_says_how_to_proceed() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), true, None).await.unwrap();
    w.sandboxes.create(create("research")).await.unwrap();
    let id = inventory(&w).keys().next().unwrap().clone();
    // What the account already had never blocks and is never listed.
    put(&w, "i-untagged", BTreeMap::new(), 1);

    let err = clouds.disconnect("aws", false).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains(&id) && msg.contains("instance"), "{msg}");
    assert!(msg.contains("1 resource"), "{msg}");
    assert!(!msg.contains("i-untagged"), "{msg}");
    assert!(msg.contains("cua cloud disconnect aws --delete"), "{msg}");
    assert!(msg.contains("cua cloud sweep --all --delete"), "{msg}");
    // Nothing changed: still connected, still the default, still running.
    let s = clouds.status(None).await.unwrap();
    assert!(s.providers[0].connected && s.default_on == "aws");
    assert!(inventory(&w).contains_key(&id));
    assert!(has(&w, "disconnect", "user", "refused"));
    assert!(!has(&w, "disconnect", "user", "ok"));
}

#[tokio::test]
async fn disconnect_with_delete_deletes_what_cua_made_confirms_then_forgets() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), true, None).await.unwrap();
    let machines: Vec<String> = [
        w.sandboxes.create(create("one")).await.unwrap(),
        w.sandboxes.create(create("two")).await.unwrap(),
    ]
    .iter()
    .map(|s| s.provider_details()[DETAIL_RELAY_MACHINE].clone())
    .collect();
    assert!(machines.iter().all(|m| w.relay.machine(m).is_some()));
    put(&w, "i-untagged", BTreeMap::new(), 1);
    put(
        &w,
        "i-other-owner",
        model::tags("someone-else", "x", "cloud-x", model::now() - 10),
        1,
    );
    let foreign = inventory(&w)
        .into_iter()
        .filter(|(k, _)| k.starts_with("i-"))
        .collect::<BTreeMap<_, _>>();
    let made: Vec<String> = inventory(&w)
        .keys()
        .filter(|k| k.starts_with("fake-"))
        .cloned()
        .collect();
    assert_eq!(made.len(), 2);

    let d = clouds.disconnect("AWS", true).await.unwrap();
    assert!(d.disconnected);
    // The receipt names the cloud resources and the relay machines that
    // went with them.
    let mut receipt: Vec<String> = d
        .deleted
        .iter()
        .filter(|r| r.resource_type != model::RELAY_MACHINE)
        .map(|r| r.id.clone())
        .collect();
    receipt.sort();
    assert_eq!(receipt, made);
    let mut gone: Vec<&str> = d
        .deleted
        .iter()
        .filter(|r| r.resource_type == model::RELAY_MACHINE)
        .map(|r| r.id.as_str())
        .collect();
    gone.sort();
    let mut expected: Vec<&str> = machines.iter().map(String::as_str).collect();
    expected.sort();
    assert_eq!(gone, expected);
    assert!(d.left.is_empty());
    // The cloud holds only what it had before Cua; nothing was touched.
    assert_eq!(inventory(&w), foreign);
    assert!(!clouds.status(None).await.unwrap().providers[0].connected);
    let cfg = std::fs::read_to_string(w.dir.path().join("config.toml")).unwrap_or_default();
    assert!(!cfg.contains("aws"), "{cfg}");
    assert!(clouds.status(None).await.unwrap().resources.is_empty());
    // The relay machines went with them.
    assert!(machines.iter().all(|m| w.relay.machine(m).is_none()));
    assert!(has(&w, "resource_deleted", "disconnect", "ok"));
    assert!(has(&w, "disconnect", "user", "ok"));
}

#[tokio::test]
async fn if_the_cloud_still_has_something_the_connection_is_kept() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), false, None).await.unwrap();
    w.sandboxes.create(create("stuck")).await.unwrap();
    let id = inventory(&w).keys().next().unwrap().clone();
    w.fake.account.lock().unwrap().fail_delete = true;

    let err = clouds.disconnect("aws", true).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("NOT disconnected") && msg.contains(&id),
        "{msg}"
    );
    assert!(clouds.status(None).await.unwrap().providers[0].connected);
    assert!(inventory(&w).contains_key(&id));
    assert!(has(&w, "resource_deleted", "disconnect", "failed"));
    assert!(has(&w, "disconnect", "user", "failed"));

    // Retry once the cloud lets go: works, and says what it deleted.
    w.fake.account.lock().unwrap().fail_delete = false;
    let d = clouds.disconnect("aws", true).await.unwrap();
    assert!(d.disconnected);
    assert_eq!(
        d.deleted
            .iter()
            .filter(|r| r.resource_type == "instance")
            .count(),
        1
    );
    assert!(inventory(&w).is_empty());
}

#[tokio::test]
async fn disconnect_is_plain_when_nothing_of_cua_s_is_there_or_the_cloud_is_not_connected() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    // Not connected: nothing to refuse.
    let d = clouds.disconnect("aws", false).await.unwrap();
    assert!(!d.disconnected && d.deleted.is_empty());
    clouds.connect(&aws(), false, None).await.unwrap();
    put(&w, "i-untagged", BTreeMap::new(), 1);
    let d = clouds.disconnect("aws", false).await.unwrap();
    assert!(d.disconnected);
    assert!(inventory(&w).contains_key("i-untagged"));
    // A cloud that cannot be asked is not guessed at: connected, refused.
    clouds.connect(&aws(), false, None).await.unwrap();
    w.fake.account.lock().unwrap().fail_list = true;
    let err = clouds.disconnect("aws", false).await.unwrap_err();
    assert!(err.to_string().contains("Cua couldn't check Fake"), "{err}");
    assert!(err.to_string().contains("records show nothing there"), "{err}");
    assert!(clouds.status(None).await.unwrap().providers[0].connected);
}

// -- B: the automatic sweep --------------------------------------------

#[tokio::test]
async fn one_automatic_pass_deletes_only_what_sweep_delete_would() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), false, None).await.unwrap();
    let store = Store::new(w.dir.path());
    let owner = store.owner_id().unwrap();
    let t = model::now();
    let old = t - 24 * 3600;
    // Past its expiry by more than the safety margin, by both clocks.
    put(&w, "i-expired", model::tags(&owner, "s1", "c1", t - 3600), old);
    put(&w, "i-live", model::tags(&owner, "s2", "c2", t + 3600), old);
    store
        .record(Resource {
            provider: "aws".into(),
            id: "i-live".into(),
            resource_type: "instance".into(),
            sandbox: "aws:s2".into(),
            state: "running".into(),
            created: old,
            expires: t + 3600,
            ..Default::default()
        })
        .unwrap();
    put(&w, "i-untagged", BTreeMap::new(), old);
    put(
        &w,
        "i-other-owner",
        model::tags("someone-else", "x", "cloud-x", t - 10),
        old,
    );
    let before = inventory(&w);

    let r = clouds.auto_sweep().await.unwrap();
    assert!(!r.dry_run);
    let after = inventory(&w);
    assert!(
        !after.contains_key("i-expired"),
        "expired Cua resource goes"
    );
    for id in ["i-live", "i-untagged", "i-other-owner"] {
        assert_eq!(after.get(id), before.get(id), "{id} untouched");
    }
    assert_eq!(after.len(), before.len() - 1);
    // The instance, and the relay machine it left (recorded when it went).
    let deleted: Vec<_> = r
        .resources
        .iter()
        .filter(|i| i.action == "deleted" && i.resource.resource_type == "instance")
        .collect();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].resource.id, "i-expired");
    assert!(has(&w, "resource_deleted", "auto-sweep", "ok"));

    // A second pass has nothing to do, and the summary line says nothing.
    let r = clouds.auto_sweep().await.unwrap();
    assert!(r.resources.iter().all(|i| i.action == "keep"));
    assert_eq!(cua_byoc::autosweep::summary(&r), None);
    assert_eq!(inventory(&w), after);
}

#[tokio::test]
async fn an_unreachable_cloud_does_not_stop_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let a = FakeProvider::new("aws", Tier::Vm);
    let g = FakeProvider::new("gcp", Tier::Vm);
    let store = Arc::new(Store::new(dir.path()));
    let clouds = Clouds::new(dir.path(), vec![a.clone(), g.clone()], store.clone());
    for p in ["aws", "gcp"] {
        clouds
            .connect(
                &Target {
                    provider: p.into(),
                    ..Default::default()
                },
                false,
                None,
            )
            .await
            .unwrap();
    }
    let owner = store.owner_id().unwrap();
    let t = model::now();
    let expired = |p: &str| Resource {
        provider: p.into(),
        id: format!("i-{p}"),
        resource_type: "instance".into(),
        name: format!("i-{p}"),
        created: t - 86_400,
        tags: model::tags(&owner, "s", "", t - 1),
        state: "running".into(),
        ..Default::default()
    };
    a.preexisting(expired("aws"));
    g.preexisting(expired("gcp"));
    a.account.lock().unwrap().fail_list = true;

    let r = clouds.auto_sweep().await.unwrap();
    assert!(g.account.lock().unwrap().resources.is_empty(), "gcp swept");
    assert!(a.account.lock().unwrap().resources.contains_key("i-aws"));
    let failed: Vec<_> = r
        .resources
        .iter()
        .filter(|i| i.action == "failed")
        .collect();
    assert_eq!(failed.len(), 1, "{:?}", r.resources);
    assert_eq!(failed[0].resource.provider, "aws");
    let line = cua_byoc::autosweep::summary(&r).unwrap();
    assert!(
        line.contains("deleted") && line.contains("failed"),
        "{line}"
    );
    assert!(line.contains("gcp instance i-gcp: expired"), "{line}");
    // Not connected: not swept at all.
    clouds.disconnect("gcp", false).await.unwrap();
    g.preexisting(expired("gcp"));
    clouds.auto_sweep().await.unwrap();
    assert!(g.account.lock().unwrap().resources.contains_key("i-gcp"));
}

#[test]
fn zero_turns_the_automatic_sweep_off() {
    use std::time::Duration;
    assert_eq!(cua_byoc::autosweep::interval(Some("0")), None);
    assert_eq!(
        cua_byoc::autosweep::interval(None),
        Some(Duration::from_secs(900))
    );
}

/// A recorded instance the account listing does not show.
fn unseen(w: &World, id: &str, region: &str) -> Resource {
    let r = Resource {
        provider: "aws".into(),
        id: id.into(),
        resource_type: "instance".into(),
        name: id.into(),
        region: region.into(),
        sandbox: format!("aws:{id}"),
        state: "running".into(),
        created: model::now() - 86_400,
        expires: model::now() + 3600,
        ..Default::default()
    };
    Store::new(w.dir.path()).record(r.clone()).unwrap();
    r
}

/// A live instance of this owner that the cloud has in `region` (and this
/// home recorded there).
fn running_in(w: &World, id: &str, region: &str) {
    let rec = unseen(w, id, region);
    let owner = Store::new(w.dir.path()).owner_id().unwrap();
    w.fake.preexisting(Resource {
        tags: model::tags(&owner, &format!("aws:{id}"), "", model::now() + 3600),
        ..rec
    });
}

#[tokio::test]
async fn a_record_the_listing_does_not_show_survives_an_automatic_pass() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), false, None).await.unwrap();
    // One placed in another region than the connection's (a listing of
    // us-west-2 cannot show it), one in the connection's region that the
    // cloud really no longer has.
    unseen(&w, "i-elsewhere", "eu-west-1");
    unseen(&w, "i-vanished", "us-west-2");
    let store = Store::new(w.dir.path());
    let ids = |s: &Store| {
        let mut v: Vec<String> = s.resources().unwrap().into_iter().map(|r| r.id).collect();
        v.sort();
        v
    };
    assert_eq!(ids(&store), ["i-elsewhere", "i-vanished"]);

    let r = clouds.auto_sweep().await.unwrap();
    assert_eq!(
        ids(&store),
        ["i-elsewhere", "i-vanished"],
        "nothing forgotten"
    );
    assert!(
        r.resources
            .iter()
            .all(|i| i.action != "forgotten" && i.action != "forget" && i.action != "deleted"),
        "{:?}",
        r.resources
    );
    assert!(!w.fake.calls().iter().any(|c| c.starts_with("delete ")));
    assert_eq!(cua_byoc::autosweep::summary(&r), None, "and says nothing");

    // A hand-run sweep asks each record's own region: it forgets only what
    // the cloud there confirms gone (i-elsewhere is not in eu-west-1 either).
    let r = clouds.sweep(Some("aws"), false, false).await.unwrap();
    assert!(ids(&store).is_empty(), "{:?}", r.resources);
    assert!(
        r.resources
            .iter()
            .any(|i| i.resource.id == "i-elsewhere" && i.action == "forgotten")
    );
    assert!(
        r.resources
            .iter()
            .any(|i| i.resource.id == "i-vanished" && i.action == "forgotten")
    );
}

#[tokio::test]
async fn disconnect_asks_each_record_in_its_own_region() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), false, None).await.unwrap();
    // Running in another region than the connection's: asked there, it is
    // seen and blocks a disconnect.
    running_in(&w, "i-elsewhere", "eu-west-1");
    for delete in [false] {
        let err = clouds
            .disconnect("aws", delete)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("i-elsewhere"), "{err}");
        assert!(clouds.status(None).await.unwrap().providers[0].connected);
        assert_eq!(
            Store::new(w.dir.path()).resources().unwrap().len(),
            1,
            "still recorded"
        );
    }
    assert!(!has(&w, "disconnect", "user", "ok"));
    // A record the cloud, asked in the right region, says is gone does not
    // block.
    w.fake
        .account
        .lock()
        .unwrap()
        .resources
        .remove("i-elsewhere");
    unseen(&w, "i-vanished", "us-west-2");
    let d = clouds.disconnect("aws", false).await.unwrap();
    assert!(d.disconnected);
}

// -- C: the audit log --------------------------------------------------

#[tokio::test]
async fn the_log_follows_a_create_delete_sweep_flow_and_holds_no_secret() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws(), false, Some(2)).await.unwrap();
    w.sandboxes.create(create("research")).await.unwrap();
    let id = inventory(&w).keys().next().unwrap().clone();
    w.sandboxes.delete("research").await.unwrap();
    // A leftover this owner made, swept by hand.
    let owner = Store::new(w.dir.path()).owner_id().unwrap();
    put(
        &w,
        "i-left",
        model::tags(&owner, "gone", "c", model::now() - 1),
        model::now() - 86_400,
    );
    clouds.sweep(Some("aws"), false, false).await.unwrap();
    // A failed create.
    w.fake.account.lock().unwrap().fail_provision = true;
    w.sandboxes.create(create("nope")).await.unwrap_err();

    let t = trail(&w);
    let seq: Vec<&str> = t.iter().map(|(a, _, _)| a.as_str()).collect();
    assert_eq!(seq[0], "connect");
    let pos = |a: &str| seq.iter().position(|x| *x == a).unwrap();
    assert!(pos("resource_recorded") < pos("resource_created"));
    assert!(pos("resource_created") < pos("resource_confirmed"));
    assert!(pos("resource_confirmed") < pos("resource_deleted"));
    assert!(has(&w, "resource_deleted", "user", "ok"));
    assert!(has(&w, "resource_deleted", "sweep", "ok"));
    assert!(has(&w, "resource_created", "user", "failed"));

    let entries = audit(&w);
    let created = entries
        .iter()
        .find(|e| e.action == "resource_created")
        .unwrap();
    assert_eq!(
        (
            created.provider.as_str(),
            created.kind.as_str(),
            created.id.as_str()
        ),
        ("aws", "instance", id.as_str())
    );
    assert_eq!(created.space, "research");
    let swept = entries
        .iter()
        .find(|e| e.actor == "sweep" && e.action == "resource_deleted")
        .unwrap();
    assert_eq!(
        (swept.id.as_str(), swept.detail.as_str()),
        ("i-left", "expired")
    );
    assert_eq!(entries[0].detail, "Fake \u{b7} us-west-2 ttl 2h");

    // On disk: one JSON object per line with exactly the documented keys,
    // 0600, and nothing secret anywhere.
    let path = w.dir.path().join("cloud").join("audit.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    for l in text.lines() {
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "action", "actor", "detail", "id", "kind", "outcome", "provider", "space", "ts"
            ]
        );
    }
    assert!(!text.contains(TOKEN) && !text.contains("cmt_"));
    for word in ["token", "secret", "password", "Bearer"] {
        assert!(!text.contains(word), "{word} in the log");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // The manager reads it back, newest last, limited.
    let last2 = clouds.audit(2).await.unwrap();
    assert_eq!(last2, entries[entries.len() - 2..]);
}
