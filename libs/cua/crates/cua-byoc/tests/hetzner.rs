// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Hetzner provider against a fake Hetzner Cloud REST server
//! (hermetic): what it creates and how (labels, the rule-less firewall, no
//! SSH key, password login off, the cheapest fitting type), that it never
//! touches a server without this owner's labels, that `test` creates
//! nothing and tells a read-only token from a read-write one, that the
//! token is saved only when the checks pass, and that 429s are retried.
#![cfg(feature = "hetzner")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use cua_byoc::api::{CloudApi, Target};
use cua_byoc::hetzner::{Hetzner, TokenStore};
use cua_byoc::model::{self, Connection, ProvisionSpec, Resource};
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    servers: BTreeMap<u64, Value>,
    firewalls: BTreeMap<u64, Value>,
    next: u64,
    calls: Vec<String>,
    tokens: Vec<String>,
    read_only: bool,
    bad_token: bool,
    /// Answer the next call with 429.
    throttle: u32,
    /// Types that answer "resource unavailable" on create.
    sold_out: Vec<String>,
    creates: Vec<Value>,
}

type S = Arc<Mutex<Fake>>;

fn types() -> Value {
    // (name, arch, cpu_type, cores, mem GiB, hourly, deprecated, locations available now)
    #[allow(clippy::type_complexity)]
    let rows: [(&str, &str, &str, u64, f64, &str, bool, &[&str]); 8] = [
        (
            "cpx22",
            "x86",
            "shared",
            2,
            4.0,
            "0.0368",
            false,
            &["fsn1", "hel1"],
        ),
        (
            "cx23",
            "x86",
            "shared",
            2,
            4.0,
            "0.0104",
            false,
            &["fsn1", "hel1"],
        ),
        ("cx22", "x86", "shared", 2, 4.0, "0.0088", true, &["fsn1"]),
        (
            "cx33",
            "x86",
            "shared",
            4,
            8.0,
            "0.0160",
            false,
            &["fsn1", "hel1"],
        ),
        (
            "ccx13",
            "x86",
            "dedicated",
            2,
            8.0,
            "0.0809",
            false,
            &["fsn1"],
        ),
        (
            "cax11",
            "arm",
            "shared",
            2,
            4.0,
            "0.0112",
            false,
            &["fsn1", "hel1"],
        ),
        ("cax21", "arm", "shared", 4, 8.0, "0.0200", false, &[]),
        ("cx43", "x86", "shared", 8, 16.0, "0.0296", false, &["fsn1"]),
    ];
    let t: Vec<Value> = rows
        .iter()
        .map(|(name, arch, cpu, cores, mem, price, dep, avail)| {
            let locations: Vec<Value> = ["fsn1", "hel1"]
                .iter()
                .map(|l| json!({"name": l, "available": avail.contains(l), "deprecation": null}))
                .collect();
            json!({"name": name, "architecture": arch, "cpu_type": cpu, "cores": cores,
               "memory": mem, "disk": 40, "deprecated": dep,
               "prices": [{"location": "fsn1", "price_hourly": {"gross": price}},
                          {"location": "hel1", "price_hourly": {"gross": price}}],
               "locations": locations})
        })
        .collect();
    json!({"server_types": t})
}

fn selector_matches(sel: &str, labels: &Value) -> bool {
    sel.split(',').filter(|kv| !kv.is_empty()).all(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        labels[k] == v
    })
}

fn query(q: &str, key: &str) -> Option<String> {
    q.split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.replace("%3D", "=").replace("%2C", ","))
}

async fn handle(
    State(s): State<S>,
    method: Method,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let q = uri.query().unwrap_or("").to_string();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let mut f = s.lock().unwrap();
    f.tokens.push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
    );
    f.calls.push(format!("{method} {path}"));
    let ok = |v: Value| (StatusCode::OK, axum::Json(v)).into_response();
    let err = |c: StatusCode, code: &str, m: &str| {
        (
            c,
            axum::Json(json!({"error": {"code": code, "message": m}})),
        )
            .into_response()
    };
    if f.throttle > 0 {
        f.throttle -= 1;
        let mut r = err(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limit_exceeded",
            "slow down",
        );
        r.headers_mut()
            .insert("ratelimit-reset", "1".parse().unwrap());
        return r;
    }
    if f.bad_token {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "unable to authenticate",
        );
    }
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let sel = query(&q, "label_selector").unwrap_or_default();
    match (method.as_str(), parts.as_slice()) {
        ("GET", ["v1", "locations"]) => {
            ok(json!({"locations": [{"name": "fsn1"}, {"name": "hel1"}]}))
        }
        ("GET", ["v1", "server_types"]) => ok(types()),
        ("GET", ["v1", "images"]) => ok(json!({"images": [{"name": "docker-ce"}]})),
        ("GET", ["v1", "servers"]) => {
            let items: Vec<Value> = f
                .servers
                .values()
                .filter(|v| selector_matches(&sel, &v["labels"]))
                .cloned()
                .collect();
            // Like the real API, one unlabelled row comes back anyway when
            // there is no selector (proves the client checks labels itself).
            ok(
                json!({"servers": items, "meta": {"pagination": {"total_entries": f.servers.len(), "next_page": null}}}),
            )
        }
        ("POST", ["v1", "servers"]) => {
            if f.read_only {
                return err(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "insufficient permissions",
                );
            }
            if body["name"].is_null() {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_input",
                    "invalid input",
                );
            }
            let ty = body["server_type"].as_str().unwrap_or("").to_string();
            f.creates.push(body.clone());
            if f.sold_out.contains(&ty) {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "resource_unavailable",
                    "error during placement: resource unavailable",
                );
            }
            f.next += 1;
            let id = 1000 + f.next;
            let server = json!({"id": id, "name": body["name"], "status": "initializing",
                "created": "2026-10-21T05:00:00+00:00", "labels": body["labels"],
                "server_type": {"name": ty}, "location": {"name": body["location"]},
                "public_net": {"ipv4": {"ip": format!("203.0.113.{}", id % 250)}}});
            f.servers.insert(id, server.clone());
            ok(json!({"server": server, "root_password": "never-read"}))
        }
        ("GET", ["v1", "servers", id]) => match f.servers.get_mut(&id.parse().unwrap()) {
            Some(v) => {
                if v["status"] == "initializing" {
                    v["status"] = json!("running");
                }
                ok(json!({"server": v.clone()}))
            }
            None => err(StatusCode::NOT_FOUND, "not_found", "server not found"),
        },
        ("DELETE", ["v1", "servers", id]) => {
            f.servers.remove(&id.parse().unwrap());
            ok(json!({"action": {}}))
        }
        ("POST", ["v1", "servers", id, "actions", verb]) => {
            match f.servers.get_mut(&id.parse().unwrap()) {
                Some(v) => {
                    v["status"] = json!(if *verb == "poweron" { "running" } else { "off" });
                    ok(json!({"action": {}}))
                }
                None => err(StatusCode::NOT_FOUND, "not_found", "server not found"),
            }
        }
        ("GET", ["v1", "firewalls"]) => {
            let name = query(&q, "name");
            let items: Vec<Value> = f
                .firewalls
                .values()
                .filter(|v| name.as_deref().is_none_or(|n| v["name"] == n))
                .filter(|v| selector_matches(&sel, &v["labels"]))
                .cloned()
                .collect();
            ok(json!({"firewalls": items, "meta": {"pagination": {"next_page": null}}}))
        }
        ("POST", ["v1", "firewalls"]) => {
            f.next += 1;
            let id = 5000 + f.next;
            let fw = json!({"id": id, "name": body["name"], "labels": body["labels"], "rules": body["rules"]});
            f.firewalls.insert(id, fw.clone());
            ok(json!({"firewall": fw}))
        }
        ("GET", ["v1", "firewalls", id]) => match f.firewalls.get(&id.parse().unwrap()) {
            Some(v) => ok(json!({"firewall": v.clone()})),
            None => err(StatusCode::NOT_FOUND, "not_found", "firewall not found"),
        },
        ("DELETE", ["v1", "firewalls", id]) => {
            f.firewalls.remove(&id.parse().unwrap());
            ok(json!({}))
        }
        _ => err(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            &format!("fake: no route {method} {path}"),
        ),
    }
}

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
