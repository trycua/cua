// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Hetzner provider against a fake Hetzner Cloud REST server
//! (hermetic): what it creates and how (labels, the rule-less firewall, no
//! SSH key, password login off, the cheapest fitting type), that it never
//! touches a server without this owner's labels, that `test` creates
//! nothing and tells a read-only token from a read-write one, that the
//! token is saved only when the checks pass, and that 429s are retried.
#![cfg(feature = "hetzner")]

use std::sync::{Arc, Mutex};

use cua_byoc::api::{CloudApi, Target};
use cua_byoc::hetzner::{Hetzner, TokenStore};
use cua_byoc::model::{self, Connection, ProvisionSpec, Resource};
use serde_json::json;

#[path = "support/fake_hetzner.rs"]
mod fake_hetzner;
use fake_hetzner::{S, handle};

#[derive(Default)]
struct MemStore(Mutex<Option<String>>);

impl TokenStore for MemStore {
    fn load(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
    fn save(&self, t: &str) -> std::io::Result<()> {
        *self.0.lock().unwrap() = Some(t.into());
        Ok(())
    }
    fn clear(&self) -> bool {
        self.0.lock().unwrap().take().is_some()
    }
}

async fn start() -> (Hetzner, S, Arc<MemStore>) {
    let s: S = Arc::default();
    let app = axum::Router::new().fallback(handle).with_state(s.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let store = Arc::new(MemStore::default());
    let h = Hetzner::with_endpoint(&format!("http://{addr}/v1"), store.clone(), None)
        .with_poll(std::time::Duration::from_millis(10));
    (h, s, store)
}

fn target(token: Option<&str>) -> Target {
    Target {
        provider: "hetzner".into(),
        token: token.map(Into::into),
        ..Default::default()
    }
}

fn conn(h: &Hetzner) -> Connection {
    h.resolve(&target(None)).unwrap()
}

fn spec(name: &str, owner: &str) -> ProvisionSpec {
    ProvisionSpec {
        name: name.into(),
        family: "linux".into(),
        image: "ghcr.io/trycua/linux:24.04".into(),
        kind: "container".into(),
        machine_type: "cx23".into(),
        disk_gb: 30,
        user_data: "#cloud-config\nwrite_files: []\n".into(),
        tags: model::tags(owner, "box", "cloud-1", 1_900_000_000),
        ttl_secs: 7200,
        arch: "amd64".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_creates_nothing_and_checks_everything_create_needs() {
    let (h, s, _) = start().await;
    h.stage_secret(&target(Some("tok-1")));
    let c = conn(&h);
    assert_eq!(
        (c.provider.as_str(), c.region.as_str()),
        ("hetzner", "fsn1")
    );
    let t = h.test(&c).await;
    assert!(t.ok(), "{:?}", t.checks);
    let names: Vec<&str> = t.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["credentials", "permissions", "region", "machine", "image"]
    );
    let f = s.lock().unwrap();
    assert!(f.servers.is_empty() && f.firewalls.is_empty());
    assert!(f.tokens.iter().all(|t| t == "Bearer tok-1"));
    // The cheapest x86 type is cx23 (the deprecated cx22 is skipped), the
    // cheapest Arm one cax11; prices carry the IPv4 address.
    assert!(
        t.checks[3].detail.contains("amd64: cx23 $0.0114/h"),
        "{:?}",
        t.checks[3]
    );
    assert!(
        t.checks[3].detail.contains("arm64: cax11"),
        "{:?}",
        t.checks[3]
    );
}

#[tokio::test]
async fn a_read_only_or_rejected_token_fails_the_checks_with_advice() {
    let (h, s, _) = start().await;
    h.stage_secret(&target(Some("ro")));
    s.lock().unwrap().read_only = true;
    let t = h.test(&conn(&h)).await;
    assert!(!t.ok());
    let p = t.checks.iter().find(|c| c.name == "permissions").unwrap();
    assert!(!p.ok && p.detail.contains("Read & Write"), "{p:?}");
    s.lock().unwrap().bad_token = true;
    let t = h.test(&conn(&h)).await;
    assert!(
        !t.ok() && t.checks[0].detail.contains("Read & Write"),
        "{:?}",
        t.checks
    );
    // Without any token the check says how to give one.
    let (h2, _, _) = start().await;
    if std::env::var("HCLOUD_TOKEN").is_err() {
        let t = h2.test(&conn(&h2)).await;
        assert!(
            !t.ok() && t.checks[0].detail.contains("--token-stdin"),
            "{:?}",
            t.checks
        );
        assert!(!h2.detect().found);
    }
}

#[tokio::test]
async fn the_token_is_saved_only_after_a_passing_connect_and_removed_on_disconnect() {
    let (h, _, store) = start().await;
    h.stage_secret(&target(Some("tok-2")));
    // Staged and tested, not saved.
    assert!(store.0.lock().unwrap().is_none());
    h.persist_secret().unwrap();
    assert_eq!(store.0.lock().unwrap().as_deref(), Some("tok-2"));
    // A later process finds it in the store.
    assert!(h.detect().found);
    h.forget_secret();
    assert!(store.0.lock().unwrap().is_none());
    // Bad locations are refused before anything else.
    let e = h
        .resolve(&Target {
            region: Some("mars1".into()),
            ..target(None)
        })
        .unwrap_err();
    assert!(e.to_string().contains("unknown location"), "{e}");
    // The token never shows in Debug output.
    let dbg = format!("{:?}", target(Some("sekrit-value")));
    assert!(
        !dbg.contains("sekrit-value") && dbg.contains("redacted"),
        "{dbg}"
    );
}

#[tokio::test]
async fn provision_labels_everything_behind_a_rule_less_firewall() {
    let (h, s, store) = start().await;
    *store.0.lock().unwrap() = Some("saved".into());
    let c = conn(&h);
    let made = h
        .provision(&c, &spec("cua-hetzner-aaaa", "owner1"))
        .await
        .unwrap();
    assert_eq!(made[0].resource_type, "instance");
    assert_eq!(made[0].extra["machine_type"], "cx23");
    assert_eq!(made[1].resource_type, "firewall");
    assert_eq!(made[1].name, "cua-sb-owner1");
    {
        let f = s.lock().unwrap();
        let body = &f.creates[0];
        assert_eq!(body["labels"]["cua-managed"], "true");
        assert_eq!(body["labels"]["cua-owner"], "owner1");
        assert_eq!(body["image"], "docker-ce");
        assert_eq!(body["location"], "fsn1");
        assert!(body.get("ssh_keys").is_none(), "no SSH key is added");
        assert_eq!(body["public_net"]["enable_ipv4"], true);
        assert_eq!(
            body["public_net"]["enable_ipv6"], false,
            "IPv6 would use a second primary IP"
        );
        // The user data goes through byte for byte.
        assert_eq!(body["user_data"], "#cloud-config\nwrite_files: []\n");
        assert_eq!(
            body["firewalls"][0]["firewall"],
            made[1].id.parse::<u64>().unwrap()
        );
        let fw = f.firewalls.values().next().unwrap();
        assert_eq!(
            fw["rules"],
            json!([]),
            "no rule: nothing reaches the servers"
        );
        assert_eq!(fw["labels"]["cua-owner"], "owner1");
        assert!(f.tokens.iter().all(|t| t == "Bearer saved"));
    }
    // A second server reuses the firewall and gets exactly the type asked
    // for, in the location asked for.
    let mut arm = spec("cua-hetzner-bbbb", "owner1");
    arm.machine_type = "cax11".into();
    arm.arch = "arm64".into();
    arm.region = "hel1".into();
    let m2 = h.provision(&c, &arm).await.unwrap();
    assert_eq!(m2[0].extra["machine_type"], "cax11");
    assert_eq!(m2[0].region, "hel1");
    {
        let f = s.lock().unwrap();
        assert_eq!(f.creates[1]["server_type"], "cax11");
        assert_eq!(f.creates[1]["location"], "hel1");
        assert_eq!(f.firewalls.len(), 1);
    }
    let mut big = spec("cua-hetzner-cccc", "owner1");
    big.machine_type = "cx33".into();
    let m3 = h.provision(&c, &big).await.unwrap();
    assert_eq!(m3[0].extra["machine_type"], "cx33");
    assert!(m3[0].extra["public_ipv4"].starts_with("203.0.113."));
    assert_eq!(s.lock().unwrap().firewalls.len(), 1);

    // Stop, start and the states.
    let inst = made[0].clone();
    h.stop(&c, &inst, "owner1").await.unwrap();
    assert_eq!(h.describe(&c, &inst).await.unwrap().unwrap().0, "stopped");
    h.start(&c, &inst, "owner1").await.unwrap();
    assert_eq!(h.describe(&c, &inst).await.unwrap().unwrap().0, "running");

    // list_owned: this owner's servers and firewall, never anyone else's.
    let listed = h.list_owned(&c, "owner1").await.unwrap();
    assert_eq!(listed.len(), 4);
    assert!(listed.iter().all(|r| model::is_ours(&r.tags, "owner1")));
    assert!(
        listed
            .iter()
            .any(|r| r.resource_type == "instance" && r.created > 0 && r.expires == 1_900_000_000)
    );
    assert!(h.list_owned(&c, "owner2").await.unwrap().is_empty());

    for r in listed.iter().filter(|r| r.resource_type == "instance") {
        h.delete(&c, r, "owner1").await.unwrap();
    }
    for r in listed.iter().filter(|r| r.resource_type == "firewall") {
        h.delete(&c, r, "owner1").await.unwrap();
    }
    let f = s.lock().unwrap();
    assert!(f.servers.is_empty() && f.firewalls.is_empty());
}

#[tokio::test]
async fn an_unavailable_host_type_fails_naming_it_and_creates_nothing() {
    let (h, s, store) = start().await;
    *store.0.lock().unwrap() = Some("t".into());
    s.lock().unwrap().sold_out = vec!["cx23".into()];
    let mut sp = spec("cua-hetzner-dddd", "o");
    sp.role = "host".into();
    let e = h.provision(&conn(&h), &sp).await.unwrap_err();
    assert!(matches!(e, cua_sandbox_core::Error::Cloud(_)), "{e:?}");
    let m = e.to_string();
    assert!(m.contains("cx23 not available in fsn1"), "{m}");
    // Never another type: one attempt, no server.
    let f = s.lock().unwrap();
    assert_eq!(f.creates.len(), 1);
    assert!(f.servers.is_empty());
}

#[tokio::test]
async fn the_one_server_per_space_path_takes_the_next_cheapest_when_sold_out() {
    let (h, s, store) = start().await;
    *store.0.lock().unwrap() = Some("t".into());
    s.lock().unwrap().sold_out = vec!["cx23".into()];
    // Role empty: the older path, which only names a rough size.
    let made = h
        .provision(&conn(&h), &spec("cua-hetzner-eeee", "o"))
        .await
        .unwrap();
    assert_eq!(made[0].extra["machine_type"], "cx33");
    assert_eq!(s.lock().unwrap().creates.len(), 2);
    // An Arm image never gets an x86 type.
    let mut arm = spec("cua-hetzner-ffff", "o");
    arm.arch = "arm64".into();
    let m = h.provision(&conn(&h), &arm).await.unwrap();
    assert_eq!(m[0].extra["machine_type"], "cax11");
}

#[tokio::test]
async fn host_role_machines_carry_their_role_and_are_listed_with_it() {
    let (h, s, store) = start().await;
    *store.0.lock().unwrap() = Some("t".into());
    let c = conn(&h);
    let mut sp = spec("cua-hetzner-host1", "owner1");
    sp.role = "host".into();
    sp.machine_type = "cx43".into();
    sp.tags.insert("cua-role".into(), "host".into());
    sp.tags
        .insert("cua-space".into(), "host-cua-hetzner-1".into());
    sp.user_data = "#cloud-config\nruncmd: [[echo, \"é ü\\n\"]]\n".into();
    let made = h.provision(&c, &sp).await.unwrap();
    {
        let f = s.lock().unwrap();
        let body = &f.creates[0];
        assert_eq!(body["user_data"], sp.user_data.as_str(), "byte for byte");
        assert_eq!(body["labels"]["cua-role"], "host");
        assert_eq!(body["server_type"], "cx43");
        assert!(body.get("ssh_keys").is_none());
    }
    let listed = h.list_owned(&c, "owner1").await.unwrap();
    let host = listed.iter().find(|r| r.id == made[0].id).unwrap();
    assert_eq!(host.tags["cua-role"], "host");
}

#[tokio::test]
async fn capabilities_and_host_types_say_what_hetzner_supplies() {
    let (h, _, store) = start().await;
    *store.0.lock().unwrap() = Some("t".into());
    let c = conn(&h);
    let caps = h.capabilities(&c);
    assert_eq!(caps.supplies, cua_byoc::model::Supplies::Hosts);
    assert_eq!(
        (
            caps.billing.granularity_secs,
            caps.billing.minimum_secs,
            caps.billing.stopped_bills
        ),
        (3600, 3600, true)
    );
    assert_eq!(caps.regions[0], "fsn1");
    assert!(caps.regions.contains(&"sin".to_string()));
    assert!(caps.max_machines >= 1 && !caps.self_deletes);
    let types = h.host_types(&c).await.unwrap();
    let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
    // Cheapest first; deprecated (cx22) and fully sold out (cax21) hidden.
    assert_eq!(names, ["cx23", "cax11", "cx33", "cx43", "cpx22", "ccx13"]);
    assert!(
        types
            .windows(2)
            .all(|w| w[0].usd_per_hour <= w[1].usd_per_hour)
    );
    for t in &types {
        assert!(!t.kvm, "{}: no /dev/kvm on Hetzner Cloud", t.name);
        assert!(t.vcpus > 0 && t.memory_mb > 0 && t.usd_per_hour > 0.0);
        assert!(t.arch == "amd64" || t.arch == "arm64");
        assert!(t.regions.iter().all(|r| caps.regions.contains(r)));
    }
    // The IPv4 address every server carries is in the price.
    assert!((types[0].usd_per_hour - 0.0114).abs() < 1e-9);
    assert_eq!(types[0].memory_mb, 4096);
    assert_eq!(
        types.iter().find(|t| t.name == "cax11").unwrap().arch,
        "arm64"
    );
}

#[tokio::test]
async fn nothing_without_this_owners_labels_is_touched() {
    let (h, s, store) = start().await;
    *store.0.lock().unwrap() = Some("t".into());
    let c = conn(&h);
    {
        let mut f = s.lock().unwrap();
        // The project already had: an unlabelled server with a Cua-looking
        // name, another owner's server, and a firewall of our name that is
        // not Cua's.
        f.servers.insert(
            1,
            json!({"id": 1, "name": "cua-hetzner-lookalike", "status": "running"}),
        );
        f.servers.insert(
            2,
            json!({"id": 2, "name": "theirs", "status": "running",
            "labels": {"cua-managed": "true", "cua-owner": "other"}}),
        );
        f.firewalls.insert(
            3,
            json!({"id": 3, "name": "cua-sb-owner1", "labels": {}, "rules": []}),
        );
    }
    let before = {
        let f = s.lock().unwrap();
        (f.servers.clone(), f.firewalls.clone())
    };
    assert!(h.list_owned(&c, "owner1").await.unwrap().is_empty());
    for id in ["1", "2"] {
        let r = Resource {
            provider: "hetzner".into(),
            id: id.into(),
            resource_type: "instance".into(),
            ..Default::default()
        };
        for e in [
            h.delete(&c, &r, "owner1").await.unwrap_err(),
            h.stop(&c, &r, "owner1").await.unwrap_err(),
            h.start(&c, &r, "owner1").await.unwrap_err(),
        ] {
            assert!(
                e.to_string().contains("not labelled as this Cua owner's"),
                "{e}"
            );
        }
    }
    // A firewall of our name that is not ours is refused, not adopted.
    let e = h
        .provision(&c, &spec("cua-hetzner-eeee", "owner1"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("left alone"), "{e}");
    {
        let f = s.lock().unwrap();
        assert_eq!((f.servers.clone(), f.firewalls.clone()), before);
        assert!(f.creates.is_empty());
    }
    // Deleting one already gone is not an error.
    let gone = Resource {
        provider: "hetzner".into(),
        id: "999".into(),
        resource_type: "instance".into(),
        ..Default::default()
    };
    h.delete(&c, &gone, "owner1").await.unwrap();
}

#[tokio::test]
async fn rate_limits_are_waited_out() {
    let (h, s, store) = start().await;
    *store.0.lock().unwrap() = Some("t".into());
    s.lock().unwrap().throttle = 2;
    let t = h.test(&conn(&h)).await;
    assert!(t.ok(), "{:?}", t.checks);
}

#[tokio::test]
async fn kinds_say_honestly_what_runs() {
    let (h, _, _) = start().await;
    let k = h.kinds(&conn(&h));
    let get = |i: &str| k.iter().find(|x| x.image == i).unwrap();
    assert!(get("linux").supported && get("linux-slim").supported);
    assert!(get("linux").usd_per_hour > 0.01 && get("linux").usd_per_hour < 0.02);
    for i in ["windows", "omarchy", "macos"] {
        assert!(!get(i).supported, "{i}");
    }
    assert!(get("windows").reason.contains("/dev/kvm"));
}

/// Live: the real Hetzner Cloud API, opt-in (`CUA_BYOC_LIVE_HETZNER=1` and
/// `HCLOUD_TOKEN`; `CUA_BYOC_LIVE_OWNER` names the owner label). Creates one
/// server and the rule-less firewall, reads, stops and starts the server,
/// then deletes both and checks nothing of this owner is left. Costs a
/// fraction of a cent.
#[tokio::test]
#[ignore = "live: creates a Hetzner server (set CUA_BYOC_LIVE_HETZNER=1 and HCLOUD_TOKEN)"]
async fn live_create_list_stop_start_delete() {
    if std::env::var("CUA_BYOC_LIVE_HETZNER").as_deref() != Ok("1") {
        eprintln!("skipped: CUA_BYOC_LIVE_HETZNER is not 1");
        return;
    }
    let owner = std::env::var("CUA_BYOC_LIVE_OWNER").unwrap_or_else(|_| "cua-live-test".into());
    let h = Hetzner::with_endpoint(cua_byoc::hetzner::API, Arc::new(MemStore::default()), None);
    let c = conn(&h);
    let t = h.test(&c).await;
    assert!(t.ok(), "{:?}", t.checks);
    let expires = model::now() + 1800;
    let mut sp = spec(&format!("cua-hetzner-live{:04x}", rand_u16()), &owner);
    sp.tags = model::tags(&owner, "live-test", "", expires);
    // Boot, do nothing, power off at once: nothing to pull.
    sp.user_data = "#cloud-config\nruncmd:\n  - [shutdown, -h, +20]\n".into();
    let made = h.provision(&c, &sp).await.unwrap();
    let inst = made[0].clone();
    let result = async {
        let (state, labels) = h.describe(&c, &inst).await.unwrap().unwrap();
        assert!(matches!(state.as_str(), "pending" | "running"), "{state}");
        assert!(model::is_ours(&labels, &owner));
        let listed = h.list_owned(&c, &owner).await.unwrap();
        assert!(listed.iter().any(|r| r.id == inst.id), "{listed:?}");
        h.stop(&c, &inst, &owner).await.unwrap();
        assert_eq!(h.describe(&c, &inst).await.unwrap().unwrap().0, "stopped");
        h.start(&c, &inst, &owner).await.unwrap();
    }
    .await;
    for r in &made {
        h.delete(&c, r, &owner).await.unwrap();
    }
    assert!(h.describe(&c, &inst).await.unwrap().is_none());
    assert!(h.list_owned(&c, &owner).await.unwrap().is_empty());
    result
}

fn rand_u16() -> u16 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        % 65536) as u16
}

// ---- the token sealed to the account's devices ---------------------------

#[cfg(feature = "federation")]
mod sealed_support;

#[cfg(feature = "federation")]
mod sealed {
    use super::sealed_support::{FakeIssuer, FakePool, FakeSealer};
    use super::*;
    use cua_byoc::Clouds;
    use cua_byoc::federation::IssuerClient;
    use cua_byoc::sealed::{PoolSealer, token_aad};
    use cua_sandbox_core::byoc::{CloudManager, ConnectRequest};

    const TOKEN: &str = "hcloud-SEALED-token-0123456789";

    async fn serve() -> (String, S) {
        let s: S = Arc::default();
        let app = axum::Router::new().fallback(handle).with_state(s.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/v1"), s)
    }

    /// One device of the account: its own home, token store and pool keys,
    /// the same Hetzner project and issuer.
    struct Device {
        _home: tempfile::TempDir,
        clouds: Clouds,
        api: Arc<Hetzner>,
        store: Arc<MemStore>,
    }

    fn device(url: &str, issuer: &Arc<FakeIssuer>, sealer: Option<Arc<FakeSealer>>) -> Device {
        let home = tempfile::tempdir().unwrap();
        let store = Arc::new(MemStore::default());
        let api = Arc::new(
            Hetzner::with_endpoint(url, store.clone(), None)
                .with_poll(std::time::Duration::from_millis(10)),
        );
        let mut clouds = Clouds::new(
            home.path(),
            vec![api.clone() as Arc<dyn CloudApi>],
            Arc::new(cua_byoc::Store::new(home.path())),
        )
        .with_issuer(issuer.clone() as Arc<dyn IssuerClient>);
        if let Some(s) = sealer {
            clouds = clouds.with_sealer(s as Arc<dyn PoolSealer>);
        }
        Device {
            _home: home,
            clouds,
            api,
            store,
        }
    }

    fn ask(token: Option<&str>, auth: Option<&str>) -> ConnectRequest {
        ConnectRequest {
            target: Target {
                provider: "hetzner".into(),
                region: Some("fsn1".into()),
                token: token.map(Into::into),
                ..Default::default()
            },
            auth: auth.map(Into::into),
            budget_usd_month: Some(50.0),
            max_size: Some("4x8".into()),
            ..Default::default()
        }
    }

    fn bearers(s: &S) -> Vec<String> {
        std::mem::take(&mut s.lock().unwrap().tokens)
    }

    #[tokio::test]
    async fn the_token_is_sealed_to_the_devices_and_used_from_memory_only() {
        let (url, s) = serve().await;
        let issuer = Arc::new(FakeIssuer::default());
        let pool = FakePool::new();
        let laptop = device(&url, &issuer, Some(pool.join("laptop")));

        let c = laptop
            .clouds
            .connect_with(&ask(Some(TOKEN), None))
            .await
            .unwrap();
        assert_eq!(c.provider.status, "connected", "{:?}", c.next);
        assert_eq!(c.provider.auth, "sealed");
        assert_eq!(c.provider.holders, vec!["laptop".to_string()]);
        assert_eq!(c.provider.budget_usd_month, Some(50.0));
        assert_eq!(c.provider.max_size, "4x8");
        // The checks ran with the token given, before anything was stored.
        assert!(bearers(&s).iter().all(|b| *b == format!("Bearer {TOKEN}")));

        // Nothing readable is kept: not in this device's token store, not
        // in its state, not on the issuer.
        assert!(laptop.store.0.lock().unwrap().is_none());
        let server = issuer.only("hetzner");
        assert_eq!(server.auth, "sealed");
        assert_eq!(server.identifiers["project"], "default");
        let blob = server.sealed_token.clone().unwrap();
        assert_eq!(blob.generation, 1);
        let needle = TOKEN.as_bytes();
        assert!(!blob.ciphertext.windows(needle.len()).any(|w| w == needle));
        // Every file under the home: the link names the connection only.
        let files = read_all(laptop._home.path());
        assert!(!files.contains(TOKEN), "{files}");
        assert!(files.contains(&server.id), "{files}");

        // A machine is made with the token opened in memory.
        let conn = laptop
            .clouds
            .store()
            .connection("hetzner")
            .unwrap()
            .unwrap();
        assert!(
            matches!(conn.auth, cua_byoc::Auth::Sealed { ref conn_id } if *conn_id == server.id)
        );
        laptop
            .api
            .provision(&conn, &spec("cua-hetzner-seal", "owner1"))
            .await
            .unwrap();
        let used = bearers(&s);
        assert!(!used.is_empty());
        assert!(
            used.iter().all(|b| *b == format!("Bearer {TOKEN}")),
            "{used:?}"
        );
        assert_eq!(
            laptop.api.detect().source,
            "the account's token, sealed to your devices"
        );

        // No tool output, Debug or status carries it.
        let status = laptop.clouds.status(Some("hetzner")).await.unwrap();
        let p = &status.providers[0];
        assert_eq!(
            (p.auth.as_str(), p.holders.clone()),
            ("sealed", vec!["laptop".to_string()])
        );
        for out in [
            serde_json::to_string(&status).unwrap(),
            serde_json::to_string(&c).unwrap(),
            format!("{c:?}"),
            format!("{:?}", laptop.clouds.sealed_tokens()),
            format!("{:?}", issuer.conns.lock().unwrap()),
            format!("{:?}", laptop.clouds.audit(100).await.unwrap()),
        ] {
            assert!(!out.contains(TOKEN), "{out}");
        }
    }

    fn read_all(dir: &std::path::Path) -> String {
        let mut out = String::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.push_str(&read_all(&p));
            } else if let Ok(t) = std::fs::read_to_string(&p) {
                out.push_str(&t);
            }
        }
        out
    }

    #[tokio::test]
    async fn another_device_opens_it_and_a_revoked_one_cannot_once_it_is_resealed() {
        let (url, s) = serve().await;
        let issuer = Arc::new(FakeIssuer::default());
        let pool = FakePool::new();
        let laptop = device(&url, &issuer, Some(pool.join("laptop")));
        laptop
            .clouds
            .connect_with(&ask(Some(TOKEN), None))
            .await
            .unwrap();

        // The desktop links the account's connection with no token of its
        // own: it opens the sealed one and checks it.
        let desktop_keys = pool.join("desktop");
        let desktop = device(&url, &issuer, Some(desktop_keys.clone()));
        let c = desktop
            .clouds
            .connect_with(&ask(None, Some("sealed")))
            .await
            .unwrap();
        assert_eq!(c.provider.status, "connected", "{:?}", c.next);
        assert!(desktop.store.0.lock().unwrap().is_none());
        bearers(&s);
        let conn = desktop
            .clouds
            .store()
            .connection("hetzner")
            .unwrap()
            .unwrap();
        assert!(desktop.api.test(&conn).await.ok());
        assert!(bearers(&s).iter().all(|b| *b == format!("Bearer {TOKEN}")));
        let mut holders = c.provider.holders.clone();
        holders.sort();
        assert_eq!(holders, ["desktop", "laptop"]);

        // The desktop is revoked: the pool moves to generation 2, and the
        // laptop's next call re-seals the token under it (compare-and-swap
        // on generation 1).
        pool.revoke("desktop");
        let laptop2 = device(&url, &issuer, Some(pool.join("laptop")));
        // (the same device, a new process: nothing in memory)
        let conn = laptop
            .clouds
            .store()
            .connection("hetzner")
            .unwrap()
            .unwrap();
        laptop2.clouds.store().connect(conn.clone()).unwrap();
        assert!(laptop2.api.test(&conn).await.ok());
        let server = issuer.only("hetzner");
        assert_eq!(server.sealed_token.as_ref().unwrap().generation, 2);
        assert!(
            issuer
                .changes
                .lock()
                .unwrap()
                .iter()
                .any(|(w, a)| w == "seal 2" && a.is_none()),
            "re-sealed by the device itself, not an agent"
        );

        // The revoked desktop, in a new process, cannot open it.
        let revoked = device(&url, &issuer, Some(desktop_keys.clone()));
        let dconn = desktop
            .clouds
            .store()
            .connection("hetzner")
            .unwrap()
            .unwrap();
        revoked.clouds.store().connect(dconn.clone()).unwrap();
        bearers(&s);
        let t = revoked.api.test(&dconn).await;
        assert!(!t.ok());
        assert!(
            t.checks[0].detail.contains("sealed token"),
            "{:?}",
            t.checks
        );
        assert!(bearers(&s).is_empty(), "no call reached Hetzner");
        let blob = server.sealed_token.clone().unwrap();
        assert!(
            desktop_keys
                .open(&blob, &token_aad(&server.account, &server.id, "hetzner", 2))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_blob_only_opens_for_its_own_account_connection_and_cloud() {
        let (url, _s) = serve().await;
        let issuer = Arc::new(FakeIssuer::default());
        let pool = FakePool::new();
        let keys = pool.join("laptop");
        let laptop = device(&url, &issuer, Some(keys.clone()));
        laptop
            .clouds
            .connect_with(&ask(Some(TOKEN), None))
            .await
            .unwrap();
        let server = issuer.only("hetzner");
        let blob = server.sealed_token.clone().unwrap();
        let ok = token_aad(&server.account, &server.id, "hetzner", 1);
        assert_eq!(
            keys.open(&blob, &ok).await.unwrap().as_slice(),
            TOKEN.as_bytes()
        );
        for aad in [
            token_aad("zzzzzzzzzzzz", &server.id, "hetzner", 1),
            token_aad(&server.account, "conn_other", "hetzner", 1),
            token_aad(&server.account, &server.id, "modal", 1),
            token_aad(&server.account, &server.id, "hetzner", 2),
        ] {
            assert!(keys.open(&blob, &aad).await.is_err());
        }
        // The same ciphertext moved onto another connection of the account
        // does not open there.
        let mut moved = server.clone();
        moved.id = "conn_9".into();
        moved.identifiers.insert("project".into(), "other".into());
        issuer.conns.lock().unwrap().push(moved.clone());
        let e = laptop
            .clouds
            .sealed_tokens()
            .open_server(&moved)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("does not open"), "{e}");
    }

    #[tokio::test]
    async fn a_swap_from_a_stale_read_conflicts() {
        let (url, _s) = serve().await;
        let issuer = Arc::new(FakeIssuer::default());
        let pool = FakePool::new();
        let laptop = device(&url, &issuer, Some(pool.join("laptop")));
        laptop
            .clouds
            .connect_with(&ask(Some(TOKEN), None))
            .await
            .unwrap();
        let read = issuer.only("hetzner");
        let tokens = laptop.clouds.sealed_tokens();
        // Another holder swaps first...
        tokens
            .put(&read, &cua_byoc::Secret::new("hcloud-new-1"), None)
            .await
            .unwrap();
        // ...so this one, from the same read, is refused and stores nothing.
        let e = tokens
            .put(&read, &cua_byoc::Secret::new("hcloud-new-2"), None)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("changed at the same time"), "{e}");
        let now = issuer.only("hetzner");
        assert_eq!(
            tokens.open_server(&now).await.unwrap().expose(),
            "hcloud-new-1"
        );
    }

    #[tokio::test]
    async fn an_agents_token_waits_for_a_person_and_disconnect_drops_the_blob() {
        let (url, s) = serve().await;
        let issuer = Arc::new(FakeIssuer::default());
        let pool = FakePool::new();
        let laptop = device(&url, &issuer, Some(pool.join("laptop")));
        let mut req = ask(Some(TOKEN), None);
        req.agent = Some("researcher".into());
        let c = laptop.clouds.connect_with(&req).await.unwrap();
        assert_eq!(c.provider.status, "waiting-for-approval");
        assert_eq!(c.provider.approval, "connection");
        // Until a person approves, the token is not used.
        let conn = laptop
            .clouds
            .store()
            .connection("hetzner")
            .unwrap()
            .unwrap();
        bearers(&s);
        let t = laptop.api.test(&conn).await;
        assert!(
            !t.ok() && t.checks[0].detail.contains("approves"),
            "{:?}",
            t.checks
        );
        assert!(bearers(&s).is_empty());
        issuer.approve(&issuer.only("hetzner").id);
        assert!(laptop.api.test(&conn).await.ok());

        // Disconnect revokes the account's link, which drops the blob.
        let id = issuer.only("hetzner").id;
        let d = laptop.clouds.disconnect("hetzner", false).await.unwrap();
        assert!(d.disconnected);
        let server = issuer.get(&id);
        assert_eq!(server.status, "revoked");
        assert!(server.sealed_token.is_none());
        assert!(
            laptop
                .clouds
                .store()
                .connection("hetzner")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_local_token_keeps_working_and_sealed_seals_it() {
        let (url, s) = serve().await;
        let issuer = Arc::new(FakeIssuer::default());

        // A cua without pool keys keeps the token on the device (as before).
        let old = device(&url, &issuer, None);
        let c = old
            .clouds
            .connect_with(&ask(Some("tok-local"), None))
            .await
            .unwrap();
        assert_eq!(
            (c.provider.status.as_str(), c.provider.auth.as_str()),
            ("connected", "cli")
        );
        assert_eq!(old.store.0.lock().unwrap().as_deref(), Some("tok-local"));
        assert!(issuer.conns.lock().unwrap().is_empty());

        // With pool keys, --no-sealed (auth cli) does the same; without a
        // budget the default keeps it local too.
        let pool = FakePool::new();
        let d = device(&url, &issuer, Some(pool.join("laptop")));
        let mut nobudget = ask(Some("tok-local"), None);
        nobudget.budget_usd_month = None;
        nobudget.max_size = None;
        let c = d.clouds.connect_with(&nobudget).await.unwrap();
        assert_eq!(c.provider.auth, "cli");
        let c = d
            .clouds
            .connect_with(&ask(Some("tok-local"), Some("cli")))
            .await
            .unwrap();
        assert_eq!(c.provider.auth, "cli");
        assert!(issuer.conns.lock().unwrap().is_empty());
        // A link made before sealed tokens (no `auth` in the file) reads as
        // the local token and uses it.
        let conn = d.clouds.store().connection("hetzner").unwrap().unwrap();
        assert!(conn.auth.is_cli());
        bearers(&s);
        assert!(d.api.test(&conn).await.ok());
        assert!(bearers(&s).iter().all(|b| b == "Bearer tok-local"));

        // `--sealed` with no token seals the one this device has.
        let c = d
            .clouds
            .connect_with(&ask(None, Some("sealed")))
            .await
            .unwrap();
        assert_eq!(
            (c.provider.status.as_str(), c.provider.auth.as_str()),
            ("connected", "sealed")
        );
        let server = issuer.only("hetzner");
        assert_eq!(
            d.clouds
                .sealed_tokens()
                .open_server(&server)
                .await
                .unwrap()
                .expose(),
            "tok-local"
        );
        // Sealed without a budget: the agent is told to ask for one.
        let pool2 = FakePool::new();
        let e = device(
            &url,
            &Arc::new(FakeIssuer::default()),
            Some(pool2.join("x")),
        );
        let mut r = ask(Some(TOKEN), Some("sealed"));
        r.budget_usd_month = None;
        let c = e.clouds.connect_with(&r).await.unwrap();
        assert_eq!(c.provider.status, "failed");
        assert!(
            c.next
                .as_ref()
                .unwrap()
                .run
                .as_deref()
                .unwrap()
                .contains("--budget <USD>"),
            "{:?}",
            c.next
        );
    }
}
