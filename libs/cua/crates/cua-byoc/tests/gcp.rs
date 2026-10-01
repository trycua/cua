// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Compute Engine provider against a fake Compute REST server
//! (hermetic): what it creates and how (labels, no service account, the
//! time limit, the dedicated network with no firewall rules), that it
//! reuses its network and refuses one that is not its own, that it never
//! touches an instance without this owner's labels, and that `test`
//! creates nothing.
#![cfg(feature = "gcp")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use cua_byoc::api::{CloudApi, Target};
use cua_byoc::gcp::{Gcp, TokenSource};
use cua_byoc::model::{self, Connection, ProvisionSpec, Resource};
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    networks: BTreeMap<String, Value>,
    instances: BTreeMap<String, Value>,
    firewalls: Vec<Value>,
    calls: Vec<String>,
    tokens: Vec<String>,
    /// Answer the next call with 401 (an expired token).
    expire_next: bool,
}

type S = Arc<Mutex<Fake>>;

fn op(url: &str, done: bool) -> Value {
    json!({"status": if done {"DONE"} else {"RUNNING"}, "selfLink": format!("{url}/operations/op-1")})
}

async fn handle(
    State(s): State<S>,
    method: Method,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
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
    if f.expire_next {
        f.expire_next = false;
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error": {"message": "expired"}})),
        )
            .into_response();
    }
    if method == Method::POST && body.is_null() && !headers.contains_key("content-length") {
        // Google answers a POST without a length with 411.
        return (
            StatusCode::LENGTH_REQUIRED,
            axum::Json(json!({"error": {"message": "411"}})),
        )
            .into_response();
    }
    let base = "http://fake/compute/v1/projects/p";
    let ok = |v: Value| (StatusCode::OK, axum::Json(v)).into_response();
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error": {"message": "not found"}})),
        )
            .into_response()
    };
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method.as_str(), parts.as_slice()) {
        ("POST", ["crm", "v1", "projects", p]) if p.ends_with(":testIamPermissions") => {
            ok(json!({"permissions": body["permissions"]}))
        }
        ("GET", ["compute", "v1", "projects", "p"]) => ok(json!({"name": "p", "id": "256"})),
        (
            "GET",
            [
                "compute",
                "v1",
                "projects",
                "ubuntu-os-cloud",
                "global",
                "images",
                "family",
                _,
            ],
        ) => ok(json!({"name": "ubuntu-2404-noble"})),
        (
            "GET",
            [
                "compute",
                "v1",
                "projects",
                "p",
                "zones",
                "us-central1-a",
                "machineTypes",
                _,
            ],
        ) => ok(json!({"name": "e2"})),
        ("GET", ["compute", "v1", "projects", "p", "regions", "us-central1"]) => {
            ok(json!({"quotas": [
            {"metric": "CPUS", "limit": 24.0, "usage": 0.0},
            {"metric": "IN_USE_ADDRESSES", "limit": 8.0, "usage": 0.0}]}))
        }
        ("GET", ["compute", "v1", "projects", "p", "global", "networks", n]) => {
            match f.networks.get(*n) {
                Some(v) => ok(v.clone()),
                None => not_found(),
            }
        }
        ("POST", ["compute", "v1", "projects", "p", "global", "networks"]) => {
            let name = body["name"].as_str().unwrap().to_string();
            f.networks.insert(name, body.clone());
            ok(op(&format!("{base}/global"), true))
        }
        ("POST", ["compute", "v1", "projects", "p", "global", "firewalls"]) => {
            f.firewalls.push(body);
            ok(op(&format!("{base}/global"), true))
        }
        ("DELETE", ["compute", "v1", "projects", "p", "global", "networks", n]) => {
            f.networks.remove(*n);
            ok(op(&format!("{base}/global"), true))
        }
        (
            "POST",
            [
                "compute",
                "v1",
                "projects",
                "p",
                "zones",
                "us-central1-a",
                "instances",
            ],
        ) => {
            let name = body["name"].as_str().unwrap().to_string();
            let mut i = body.clone();
            i["status"] = json!("PROVISIONING");
            i["creationTimestamp"] = json!("2026-09-30T12:00:00.000-07:00");
            f.instances.insert(name, i);
            // Not done yet: the provider waits on it.
            ok(op(&format!("{base}/zones/us-central1-a"), false))
        }
        (
            "POST",
            [
                "compute",
                "v1",
                "projects",
                "p",
                "zones",
                _,
                "operations",
                _,
                "wait",
            ],
        )
        | (
            "POST",
            [
                "compute",
                "v1",
                "projects",
                "p",
                "global",
                "operations",
                _,
                "wait",
            ],
        ) => ok(
            json!({"status": "DONE", "selfLink": format!("{base}/zones/us-central1-a/operations/op-1")}),
        ),
        (
            "GET",
            [
                "compute",
                "v1",
                "projects",
                "p",
                "zones",
                "us-central1-a",
                "instances",
            ],
        ) => {
            let q = uri.query().unwrap_or("");
            let filter = url_decode(q);
            let owner = filter
                .split("labels.cua-owner=\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap_or("")
                .to_string();
            // Like the real API: the filter is applied on the server, and
            // one untagged row is returned anyway to prove the client checks.
            let mut items: Vec<Value> = f
                .instances
                .values()
                .filter(|i| i["labels"]["cua-owner"] == owner.as_str() || i["labels"].is_null())
                .cloned()
                .collect();
            items.sort_by_key(|i| i["name"].as_str().unwrap_or("").to_string());
            ok(json!({"items": items}))
        }
        (
            "GET",
            [
                "compute",
                "v1",
                "projects",
                "p",
                "zones",
                "us-central1-a",
                "instances",
                n,
            ],
        ) => match f.instances.get(*n) {
            Some(v) => {
                let mut v = v.clone();
                if v["status"] == "PROVISIONING" {
                    v["status"] = json!("RUNNING");
                }
                ok(v)
            }
            None => not_found(),
        },
        (
            "POST",
            [
                "compute",
                "v1",
                "projects",
                "p",
                "zones",
                "us-central1-a",
                "instances",
                n,
                verb,
            ],
        ) => {
            let status = if *verb == "stop" {
                "TERMINATED"
            } else {
                "RUNNING"
            };
            match f.instances.get_mut(*n) {
                Some(i) => {
                    i["status"] = json!(status);
                    ok(op(&format!("{base}/zones/us-central1-a"), true))
                }
                None => not_found(),
            }
        }
        (
            "DELETE",
            [
                "compute",
                "v1",
                "projects",
                "p",
                "zones",
                "us-central1-a",
                "instances",
                n,
            ],
        ) => {
            f.instances.remove(*n);
            ok(op(&format!("{base}/zones/us-central1-a"), true))
        }
        _ => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": {"message": format!("fake: no route {method} {path}")}})),
        )
            .into_response(),
    }
}

fn url_decode(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                out.push(
                    u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap(), 16).unwrap(),
                );
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
}

struct StaticToken;

#[async_trait]
impl TokenSource for StaticToken {
    async fn token(&self, account: Option<&str>) -> cua_byoc::Result<String> {
        Ok(format!("tok-{}", account.unwrap_or("active")))
    }
}

async fn start() -> (Gcp, S) {
    let s: S = Arc::default();
    let app = axum::Router::new().fallback(handle).with_state(s.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let gcp = Gcp::with_endpoints(
        &format!("http://{addr}/compute/v1"),
        &format!("http://{addr}/crm/v1"),
        Arc::new(StaticToken),
        None,
    );
    (gcp, s)
}

fn conn(gcp: &Gcp) -> Connection {
    gcp.resolve(&Target {
        provider: "gcp".into(),
        project: Some("p".into()),
        profile: Some("ada@example.com".into()),
        ..Default::default()
    })
    .unwrap()
}

fn spec(name: &str, owner: &str) -> ProvisionSpec {
    ProvisionSpec {
        name: name.into(),
        family: "linux".into(),
        image: "ghcr.io/trycua/linux:24.04".into(),
        kind: "container".into(),
        machine_type: "e2-medium".into(),
        disk_gb: 30,
        user_data: "#cloud-config\n".into(),
        tags: model::tags(owner, "box", "cloud-1", 1_900_000_000),
        ttl_secs: 7200,
        arch: "amd64".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_creates_nothing_and_checks_everything_create_needs() {
    let (gcp, s) = start().await;
    let c = conn(&gcp);
    let t = gcp.test(&c).await;
    assert!(t.ok(), "{:?}", t.checks);
    let names: Vec<&str> = t.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "credentials",
            "project",
            "permissions",
            "image",
            "region",
            "quota"
        ]
    );
    assert_eq!(t.account, "p");
    let f = s.lock().unwrap();
    assert!(
        f.calls
            .iter()
            .all(|c| c.starts_with("GET ") || c.contains(":testIamPermissions")),
        "{:?}",
        f.calls
    );
    assert!(f.networks.is_empty() && f.instances.is_empty());
    assert!(f.tokens.iter().all(|t| t == "Bearer tok-ada@example.com"));
}

#[tokio::test]
async fn provision_labels_everything_in_its_own_network_and_ends_itself() {
    let (gcp, s) = start().await;
    let c = conn(&gcp);
    let made = gcp
        .provision(&c, &spec("cua-gcp-aaaa", "owner1"))
        .await
        .unwrap();
    assert_eq!(made[0].id, "cua-gcp-aaaa");
    assert_eq!(made[0].resource_type, "instance");
    assert_eq!(made[1].id, "cua-sb-owner1");
    assert_eq!(made[1].resource_type, "network");
    {
        let f = s.lock().unwrap();
        let i = &f.instances["cua-gcp-aaaa"];
        assert_eq!(i["labels"]["cua-managed"], "true");
        assert_eq!(i["labels"]["cua-owner"], "owner1");
        assert_eq!(
            i["disks"][0]["initializeParams"]["labels"]["cua-managed"],
            "true"
        );
        assert_eq!(i["serviceAccounts"], json!([]));
        assert_eq!(i["scheduling"]["maxRunDuration"]["seconds"], "7200");
        assert_eq!(i["scheduling"]["instanceTerminationAction"], "DELETE");
        assert_eq!(
            i["networkInterfaces"][0]["network"],
            "global/networks/cua-sb-owner1"
        );
        assert_eq!(i["metadata"]["items"][0]["key"], "user-data");
        assert!(
            i["disks"][0]["initializeParams"]["sourceImage"]
                .as_str()
                .unwrap()
                .ends_with("ubuntu-2404-lts-amd64")
        );
        let n = &f.networks["cua-sb-owner1"];
        assert_eq!(n["autoCreateSubnetworks"], true);
        assert_eq!(n["mtu"], 1500);
        assert!(
            n["description"]
                .as_str()
                .unwrap()
                .contains("cua-managed=true cua-owner=owner1")
        );
        // No firewall rule at all: nothing reaches the VMs from outside.
        assert!(f.firewalls.is_empty());
    }
    // A second sandbox reuses the network.
    gcp.provision(&c, &spec("cua-gcp-bbbb", "owner1"))
        .await
        .unwrap();
    let creates = s
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.ends_with("/global/networks"))
        .count();
    assert_eq!(creates, 1);

    // Stop, start and the states.
    let inst = made[0].clone();
    gcp.stop(&c, &inst, "owner1").await.unwrap();
    assert_eq!(gcp.describe(&c, &inst).await.unwrap().unwrap().0, "stopped");
    gcp.start(&c, &inst, "owner1").await.unwrap();
    assert_eq!(gcp.describe(&c, &inst).await.unwrap().unwrap().0, "running");

    // list_owned: this owner's instances and network, never anyone else's.
    let listed = gcp.list_owned(&c, "owner1").await.unwrap();
    let ids: Vec<&str> = listed.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["cua-gcp-aaaa", "cua-gcp-bbbb", "cua-sb-owner1"]);
    assert!(listed[0].created > 0);

    // Delete both instances, then the network.
    for r in &listed {
        gcp.delete(&c, r, "owner1").await.unwrap();
    }
    let f = s.lock().unwrap();
    assert!(f.instances.is_empty() && f.networks.is_empty());
}

#[tokio::test]
async fn nothing_without_this_owners_labels_is_touched() {
    let (gcp, s) = start().await;
    let c = conn(&gcp);
    {
        let mut f = s.lock().unwrap();
        // The project already had: an unlabelled VM with a Cua-looking
        // name, another owner's VM, and a network of our name that is not
        // Cua's.
        f.instances.insert(
            "cua-gcp-lookalike".into(),
            json!({"name": "cua-gcp-lookalike", "status": "RUNNING"}),
        );
        f.instances.insert(
            "theirs".into(),
            json!({"name": "theirs", "status": "RUNNING", "labels": {"cua-managed": "true", "cua-owner": "other"}}),
        );
        f.networks.insert(
            "cua-sb-owner1".into(),
            json!({"name": "cua-sb-owner1", "description": "someone's"}),
        );
    }
    let snapshot = |s: &S| {
        let f = s.lock().unwrap();
        (f.instances.clone(), f.networks.clone())
    };
    let before = snapshot(&s);
    let listed = gcp.list_owned(&c, "owner1").await.unwrap();
    assert!(listed.is_empty(), "{listed:?}");
    for id in ["cua-gcp-lookalike", "theirs"] {
        let r = Resource {
            provider: "gcp".into(),
            id: id.into(),
            resource_type: "instance".into(),
            ..Default::default()
        };
        let e = gcp.delete(&c, &r, "owner1").await.unwrap_err();
        assert!(
            e.to_string().contains("not tagged as this Cua owner's"),
            "{e}"
        );
        assert!(gcp.stop(&c, &r, "owner1").await.is_err());
    }
    // A network of our name that is not ours is refused, not reused.
    let e = gcp
        .provision(&c, &spec("cua-gcp-cccc", "owner1"))
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("not tagged as this Cua owner's"),
        "{e}"
    );
    let after = snapshot(&s);
    assert_eq!(before, after, "nothing changed");
    // Gone already: delete is fine.
    let gone = Resource {
        provider: "gcp".into(),
        id: "nope".into(),
        resource_type: "instance".into(),
        ..Default::default()
    };
    gcp.delete(&c, &gone, "owner1").await.unwrap();
}

#[tokio::test]
async fn an_expired_token_is_refreshed_once() {
    let (gcp, s) = start().await;
    let c = conn(&gcp);
    s.lock().unwrap().expire_next = true;
    let r = Resource {
        provider: "gcp".into(),
        id: "nope".into(),
        resource_type: "instance".into(),
        ..Default::default()
    };
    assert!(gcp.describe(&c, &r).await.unwrap().is_none());
    assert_eq!(s.lock().unwrap().calls.len(), 2, "one retry");
}
