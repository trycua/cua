// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A cloud whose sign-in expired (or went missing, or reaches another
//! account) never causes harm, in every scenario: against the scripted
//! cloud. Each test asserts what applies of:
//!
//! - records are kept: a forced disconnect keeps Cua's records, named for
//!   their account, until a sweep after sign-in (or the same account
//!   connected again) takes them;
//! - cleanup still happens: every resource keeps its tags and expiry, so it
//!   ends itself even if Cua never comes back;
//! - no wrong-account actions: nothing acts through another account;
//! - no false clean state: nothing reads "clean" while something may run;
//! - budgets are honest: running resources stay counted;
//! - retries stay bounded, with clear status.

use std::collections::BTreeMap;
use std::sync::Arc;

use cua_byoc::model::{self, Resource, Tier};
use cua_byoc::testing::FakeProvider;
use cua_byoc::{RelayAccess, Store, Target};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::byoc::signin::{DETAILS, UNCHECKED, split};
use cua_sandbox_core::byoc::DETAIL_RELAY_MACHINE;
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};

const TOKEN: &str = "acct-token-0123456789";
/// How the scripted cloud words an expired sign-in (like the AWS SDK).
const EXPIRED: &str = "dispatch failure: failed to refresh cached Login token: Your session has expired. Please reauthenticate.";
const LINE: &str = "Your Fake sign-in expired. Sign in with `fake login`, then run this again.";

struct World {
    dir: tempfile::TempDir,
    relay: Arc<FakeRelay>,
    sandboxes: Sandboxes,
    fake: Arc<FakeProvider>,
}

fn sandboxes_over(dir: &std::path::Path, relay: &FakeRelay, fake: &Arc<FakeProvider>) -> Sandboxes {
    let access = RelayAccess::new(
        relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken(TOKEN.into())),
    );
    cua_byoc::install_with(
        Sandboxes::builder().state_dir(dir.join("sandboxes")),
        dir,
        vec![fake.clone()],
        access,
    )
    .build()
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("aws", Tier::Vm);
    let sandboxes = sandboxes_over(dir.path(), &relay, &fake);
    let r = relay.clone();
    fake.on_boot(Arc::new(move |spec, _| {
        r.set_online(&spec.tags[model::tag::MACHINE], true, "0.1.0");
    }));
    World {
        dir,
        relay,
        sandboxes,
        fake,
    }
}

fn aws(profile: &str) -> Target {
    Target {
        provider: "aws".into(),
        region: Some("us-west-2".into()),
        profile: Some(profile.into()),
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

fn store(w: &World) -> Store {
    Store::new(w.dir.path())
}

fn sign_out(w: &World) {
    w.fake.account.lock().unwrap().signed_out = Some(EXPIRED.into());
}

fn sign_in(w: &World) {
    w.fake.account.lock().unwrap().signed_out = None;
}

fn deletes(w: &World) -> usize {
    w.fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("delete"))
        .count()
}

fn audit_has(w: &World, action: &str, outcome: &str) -> bool {
    store(w)
        .audit_entries(1000)
        .unwrap()
        .iter()
        .any(|e| e.action == action && e.outcome == outcome)
}

/// A connected cloud with one running Space; its cloud id.
async fn one_space(w: &World) -> String {
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws("sandbox"), true, None).await.unwrap();
    w.sandboxes.create(create("research")).await.unwrap();
    inventory(w).keys().next().unwrap().clone()
}

/// Every resource still carries what ends it without Cua: the managed and
/// owner tags and an expiry.
fn ends_on_its_own(r: &Resource, owner: &str) {
    assert!(model::is_ours(&r.tags, owner), "{r:?}");
    assert!(model::tag_expires(&r.tags) > model::now(), "{r:?}");
}

// -- the line people read ----------------------------------------------

#[tokio::test]
async fn a_signed_out_cloud_reads_as_one_line_and_keeps_the_raw_error_as_details() {
    let w = world().await;
    let id = one_space(&w).await;
    sign_out(&w);
    // Create and delete: each one line, nothing changed.
    let err = w.sandboxes.create(create("second")).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains(LINE), "{msg}");
    assert!(msg.contains(DETAILS) && msg.contains("Your session has expired"), "{msg}");
    assert_eq!(split(msg.trim_start_matches("your cloud: ")).0, LINE);
    // Nothing was made or recorded for the refused create.
    assert_eq!(inventory(&w).len(), 1);
    assert!(
        !store(&w)
            .resources()
            .unwrap()
            .iter()
            .any(|r| r.sandbox == "aws:second"),
        "a refused create leaves no record"
    );
    let err = w.sandboxes.delete("research").await.unwrap_err();
    assert!(err.to_string().contains(LINE), "{err}");
    assert!(inventory(&w).contains_key(&id), "nothing deleted");
    assert!(
        store(&w).resources().unwrap().iter().any(|r| r.id == id),
        "a failed delete keeps the record"
    );
    // Status still answers (from Cua's records) while signed out.
    let s = w.sandboxes.clouds().unwrap().status(None).await.unwrap();
    assert!(s.providers[0].connected);
    assert_eq!(s.resources.len(), 1);
    assert_eq!(deletes(&w), 0);
}

// -- disconnect while signed out ---------------------------------------

#[tokio::test]
async fn disconnect_while_signed_out_asks_first_then_force_keeps_the_records() {
    let w = world().await;
    let id = one_space(&w).await;
    let owner = store(&w).owner_id().unwrap();
    let clouds = w.sandboxes.clouds().unwrap();
    sign_out(&w);

    // Without force: refused, saying what Cua's records show and how to go on.
    let err = clouds.disconnect("aws", false).await.unwrap_err();
    let msg = err.to_string();
    let (plain, details) = split(&msg);
    assert!(plain.contains(&format!("{UNCHECKED}Fake for Spaces it started (your sign-in expired)")), "{plain}");
    assert!(plain.contains("Cua's records show 1 Space"), "{plain}");
    assert!(plain.contains("Disconnect anyway?"), "{plain}");
    assert!(plain.contains("(latest: in 8 h)"), "{plain}");
    assert!(plain.contains("run `cua cloud sweep aws --delete`"), "{plain}");
    assert!(plain.contains(LINE), "{plain}");
    assert!(plain.contains("`cua cloud disconnect aws --force`"), "{plain}");
    assert!(details.is_some_and(|d| d.contains("Your session has expired")), "{msg}");
    assert!(cua_sandbox_core::byoc::signin::is_unchecked(&msg));
    assert!(clouds.status(None).await.unwrap().providers[0].connected);
    assert!(audit_has(&w, "disconnect", "refused"));

    // Forced: disconnected, the records kept and named for their account.
    let d = clouds.disconnect_with("aws", false, true).await.unwrap();
    assert!(d.disconnected);
    let c = d.cleanup.expect("what may still run");
    assert_eq!((c.spaces, c.resources), (1, 1));
    assert_eq!(c.profile, "sandbox");
    assert_eq!(c.account, "fake-account");
    assert_eq!(c.message, "Fake may still have 1 Space Cua started. Sign in and clean up.");
    let s = clouds.status(None).await.unwrap();
    assert!(!s.providers[0].connected);
    assert!(s.default_on.is_empty(), "no longer the default");
    assert_eq!(s.cleanup.len(), 1, "no false clean state");
    assert_eq!(s.cleanup[0].message, c.message);
    let recs = store(&w).resources().unwrap();
    assert!(recs.iter().any(|r| r.id == id && r.account == "fake-account"));
    assert!(audit_has(&w, "disconnect", "forced"));
    // Nothing was touched in the cloud, and the Space ends on its own.
    assert_eq!(deletes(&w), 0);
    ends_on_its_own(&inventory(&w)[&id], &owner);
}

#[tokio::test]
async fn force_never_disconnects_a_cloud_that_can_be_checked_and_runs_spaces() {
    let w = world().await;
    one_space(&w).await;
    let clouds = w.sandboxes.clouds().unwrap();
    let err = clouds.disconnect_with("aws", false, true).await.unwrap_err();
    assert!(err.to_string().contains("was NOT disconnected"), "{err}");
    assert!(clouds.status(None).await.unwrap().providers[0].connected);
    // And --delete cannot be forced while signed out: it needs the cloud.
    sign_out(&w);
    let err = clouds.disconnect_with("aws", true, true).await.unwrap_err();
    assert!(err.to_string().contains(LINE), "{err}");
    assert!(clouds.status(None).await.unwrap().providers[0].connected);
    assert_eq!(deletes(&w), 0);
}

// -- sweeps while signed out, and after signing in ------------------------

#[tokio::test]
async fn sweeps_while_signed_out_delete_nothing_and_signing_in_finishes_the_cleanup() {
    let w = world().await;
    let id = one_space(&w).await;
    let clouds = w.sandboxes.clouds().unwrap();
    sign_out(&w);
    clouds.disconnect_with("aws", false, true).await.unwrap();

    // Manual and automatic sweeps while signed out: a failed row with the
    // line, nothing deleted, nothing forgotten, the cleanup still listed.
    for _ in 0..3 {
        let r = clouds.sweep(None, false, false).await.unwrap();
        assert!(
            r.resources
                .iter()
                .any(|i| i.action == "failed" && i.reason.contains(LINE)),
            "{r:?}"
        );
        let r = clouds.auto_sweep().await.unwrap();
        assert!(r.resources.iter().all(|i| i.action == "failed"), "{r:?}");
    }
    assert_eq!(deletes(&w), 0);
    assert!(store(&w).resources().unwrap().iter().any(|r| r.id == id));
    assert_eq!(clouds.status(None).await.unwrap().cleanup.len(), 1);

    // Signed in: the automatic sweep still deletes only what expired (the
    // Space has not), so the cleanup stays listed.
    sign_in(&w);
    clouds.auto_sweep().await.unwrap();
    assert!(inventory(&w).contains_key(&id));
    assert_eq!(clouds.status(None).await.unwrap().cleanup.len(), 1);

    // A dry run says what it would do and changes nothing.
    let r = clouds.sweep(None, true, false).await.unwrap();
    assert!(r.resources.iter().any(|i| i.resource.id == id && i.action == "delete"));
    assert_eq!(clouds.status(None).await.unwrap().cleanup.len(), 1);

    // `cua cloud sweep --delete`: what the forced disconnect left goes, the
    // cloud confirms it is clean, and only then the line clears.
    let r = clouds.sweep(None, false, false).await.unwrap();
    assert!(r.resources.iter().any(|i| i.resource.id == id && i.action == "deleted"), "{r:?}");
    assert!(r.resources.iter().any(|i| i.action == "clean"), "{r:?}");
    assert!(inventory(&w).is_empty());
    let s = clouds.status(None).await.unwrap();
    assert!(s.cleanup.is_empty() && s.resources.is_empty(), "{s:?}");
    assert!(audit_has(&w, "cleanup_done", "ok"));
}

#[tokio::test]
async fn a_cleanup_stays_until_the_cloud_confirms_nothing_is_left() {
    let w = world().await;
    let id = one_space(&w).await;
    let clouds = w.sandboxes.clouds().unwrap();
    sign_out(&w);
    clouds.disconnect_with("aws", false, true).await.unwrap();
    sign_in(&w);
    // The delete fails: the record and the line stay.
    w.fake.account.lock().unwrap().fail_delete = true;
    let r = clouds.sweep(None, false, false).await.unwrap();
    assert!(r.resources.iter().any(|i| i.action == "failed"), "{r:?}");
    assert!(!r.resources.iter().any(|i| i.action == "clean"));
    assert_eq!(clouds.status(None).await.unwrap().cleanup.len(), 1);
    assert!(store(&w).resources().unwrap().iter().any(|r| r.id == id));
    // Retried later: done.
    w.fake.account.lock().unwrap().fail_delete = false;
    clouds.sweep(None, false, false).await.unwrap();
    assert!(clouds.status(None).await.unwrap().cleanup.is_empty());
}

// -- signing back in, other accounts --------------------------------------

#[tokio::test]
async fn connecting_the_same_account_again_takes_its_spaces_back_and_deletes_nothing() {
    let w = world().await;
    let id = one_space(&w).await;
    let clouds = w.sandboxes.clouds().unwrap();
    sign_out(&w);
    clouds.disconnect_with("aws", false, true).await.unwrap();
    sign_in(&w);
    clouds.connect(&aws("sandbox"), false, None).await.unwrap();
    let s = clouds.status(None).await.unwrap();
    assert!(s.cleanup.is_empty(), "the same account took them back");
    assert!(s.resources.iter().any(|r| r.id == id), "still recorded, still listed");
    assert!(inventory(&w).contains_key(&id));
    // The Space is usable again, and only the person deletes it.
    w.sandboxes.get("research").await.unwrap();
    assert_eq!(deletes(&w), 0);
    assert!(audit_has(&w, "cleanup_adopted", "ok"));
}

#[tokio::test]
async fn another_account_never_acts_on_the_first_one_s_records() {
    let w = world().await;
    let id = one_space(&w).await;
    let clouds = w.sandboxes.clouds().unwrap();
    sign_out(&w);
    clouds.disconnect_with("aws", false, true).await.unwrap();

    // Signed in again, but to another account (another profile, or keys in
    // the environment): connecting it is fine, and it is a separate account.
    {
        let mut a = w.fake.account.lock().unwrap();
        a.signed_out = None;
        a.identity = Some("other-account".into());
    }
    clouds.connect(&aws("other"), false, None).await.unwrap();
    let s = clouds.status(None).await.unwrap();
    assert_eq!(s.cleanup.len(), 1, "the first account's cleanup stays");
    assert_eq!(s.cleanup[0].account, "fake-account");

    // A sweep: the live connection sees no record of its own to judge; the
    // cleanup's pass refuses (the credentials reach the other account).
    let r = clouds.sweep(None, false, false).await.unwrap();
    assert!(
        r.resources.iter().any(|i| i.action == "failed"
            && i.reason.contains("reaches other-account, not fake-account")),
        "{r:?}"
    );
    assert_eq!(deletes(&w), 0);
    assert!(store(&w).resources().unwrap().iter().any(|r| r.id == id), "never forgotten");
    // Acting on the first account's Space through the second connection is
    // refused, not read as "not found, so deleted".
    let err = w.sandboxes.delete("research").await.unwrap_err();
    assert!(err.to_string().contains("account fake-account"), "{err}");
    assert!(inventory(&w).contains_key(&id));
}

#[tokio::test]
async fn credentials_that_drift_to_another_account_are_refused_everywhere() {
    let w = world().await;
    let id = one_space(&w).await;
    let clouds = w.sandboxes.clouds().unwrap();
    // The profile now reaches another account (AWS_ACCESS_KEY_ID in the
    // environment, an edited profile): a "fix" Cua must never take.
    w.fake.account.lock().unwrap().identity = Some("other-account".into());
    let r = clouds.sweep(None, false, true).await.unwrap();
    assert!(
        r.resources
            .iter()
            .any(|i| i.action == "failed" && i.reason.contains("reaches other-account")),
        "{r:?}"
    );
    let err = clouds.sweep(Some("aws"), false, true).await.unwrap_err();
    assert!(err.to_string().contains("reaches other-account"), "{err}");
    let err = clouds.disconnect("aws", false).await.unwrap_err();
    assert!(cua_sandbox_core::byoc::signin::is_unchecked(&err.to_string()), "{err}");
    assert!(err.to_string().contains("signed in to another account"), "{err}");
    let err = w.sandboxes.create(create("two")).await.unwrap_err();
    assert!(err.to_string().contains("reaches other-account"), "{err}");
    assert_eq!(deletes(&w), 0);
    assert!(
        !w.fake.calls().iter().any(|c| c.starts_with("provision two")),
        "nothing created in the wrong account"
    );
    assert!(inventory(&w).contains_key(&id));
}

// -- in flight, restarts, budgets ----------------------------------------

#[tokio::test]
async fn a_forced_disconnect_during_a_create_still_finds_what_the_create_made() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws("sandbox"), false, None).await.unwrap();
    // The machine boots but has not joined yet when the disconnect comes.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = std::sync::Mutex::new(tx);
    w.fake.on_boot(Arc::new(move |spec, _| {
        let _ = tx
            .lock()
            .unwrap()
            .send(spec.tags[model::tag::MACHINE].clone());
    }));
    let sb = w.sandboxes.clone();
    let task = tokio::spawn(async move { sb.create(create("racing")).await });
    let machine = tokio::task::spawn_blocking(move || rx.recv().unwrap())
        .await
        .unwrap();
    sign_out(&w);
    let d = clouds.disconnect_with("aws", false, true).await.unwrap();
    assert!(d.disconnected);
    // The create finishes after the disconnect.
    w.relay.set_online(&machine, true, "0.1.0");
    let made = task.await.unwrap();
    let id = inventory(&w).keys().next().unwrap().clone();
    let rec = store(&w)
        .resources()
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .expect("recorded by the create");
    assert_eq!(rec.account, "fake-account", "named for the disconnected account");
    let _ = made;
    let s = clouds.status(None).await.unwrap();
    assert_eq!(s.cleanup.len(), 1);
    assert_eq!(s.cleanup[0].spaces, 1, "the in-flight Space is counted");
    // After sign-in the sweep finds and deletes it.
    sign_in(&w);
    clouds.sweep(None, false, false).await.unwrap();
    assert!(inventory(&w).is_empty());
    assert!(clouds.status(None).await.unwrap().cleanup.is_empty());
}

#[tokio::test]
async fn a_restart_keeps_the_cleanup_and_a_create_under_way() {
    let w = world().await;
    let id = one_space(&w).await;
    sign_out(&w);
    w.sandboxes
        .clouds()
        .unwrap()
        .disconnect_with("aws", false, true)
        .await
        .unwrap();
    // A create recorded before its cloud call, then the app died.
    store(&w)
        .record(Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            name: "cua-aws-pending".into(),
            sandbox: "aws:half".into(),
            created: model::now(),
            state: "pending".into(),
            account: "fake-account".into(),
            ..Default::default()
        })
        .unwrap();
    // The app and its daemon start again.
    let again = sandboxes_over(w.dir.path(), &w.relay, &w.fake);
    let clouds = again.clouds().unwrap();
    let s = clouds.status(None).await.unwrap();
    assert_eq!(s.cleanup.len(), 1);
    assert_eq!(s.cleanup[0].resources, 2);
    sign_in(&w);
    // The automatic sweep after the restart: the young pending record and
    // the unexpired Space stay.
    clouds.auto_sweep().await.unwrap();
    assert!(inventory(&w).contains_key(&id));
    assert!(store(&w).resources().unwrap().iter().any(|r| r.name == "cua-aws-pending"));
    assert_eq!(deletes(&w), 0);
}

#[tokio::test]
async fn costs_stay_counted_while_signed_out() {
    let w = world().await;
    one_space(&w).await;
    let clouds = w.sandboxes.clouds().unwrap();
    sign_out(&w);
    let d = clouds.disconnect_with("aws", false, true).await.unwrap();
    let c = d.cleanup.unwrap();
    assert!(c.usd_per_hour > 0.0, "the Space's cost is still counted: {c:?}");
    assert!(!c.latest_expires.is_empty());
    // And the relay machine it joined as is not removed while it may run.
    let m = store(&w).resources().unwrap()[0].machine.clone();
    assert!(w.relay.machine(&m).is_some());
}

#[tokio::test]
async fn a_pool_host_whose_relay_lease_expired_is_offline_not_gone_while_its_cloud_runs_it() {
    let w = world().await;
    let clouds = w.sandboxes.clouds().unwrap();
    clouds.connect(&aws("sandbox"), false, None).await.unwrap();
    let owner = store(&w).owner_id().unwrap();
    let t = model::now();
    let host = Resource {
        provider: "aws".into(),
        id: "i-host".into(),
        resource_type: "instance".into(),
        name: "cua-host-1".into(),
        region: "us-west-2".into(),
        sandbox: "host:aws-1".into(),
        machine: "m-gone-from-relay".into(),
        created: t - 3600,
        expires: t + 3600,
        tags: model::tags(&owner, "host:aws-1", "m-gone-from-relay", t + 3600),
        state: "running".into(),
        account: "fake-account".into(),
        extra: BTreeMap::from([
            ("cua-role".to_string(), "host".to_string()),
            ("host".to_string(), "aws-1".to_string()),
            ("usd_per_hour".to_string(), "0.05".to_string()),
        ]),
        ..Default::default()
    };
    w.fake.preexisting(host.clone());
    store(&w).record(host).unwrap();
    // The relay does not know its machine (its lease expired in a
    // partition), but the cloud runs it: offline, still counted.
    let hosts = clouds.hosts().await.unwrap();
    let h = hosts.iter().find(|h| h.id == "i-host").expect("listed");
    assert_ne!(h.state, "gone", "{h:?}");
    assert!(h.usd_per_hour > 0.0);
    // Signed out: still not gone.
    sign_out(&w);
    let hosts = clouds.hosts().await.unwrap();
    assert_ne!(hosts.iter().find(|h| h.id == "i-host").unwrap().state, "gone");
    assert_eq!(deletes(&w), 0);
    let _ = DETAIL_RELAY_MACHINE;
}
