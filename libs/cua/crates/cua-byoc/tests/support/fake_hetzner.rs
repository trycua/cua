// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A fake Hetzner Cloud REST server, shared by the Hetzner provider tests
//! and the upgrade tests.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

#[derive(Default)]
pub struct Fake {
    pub servers: BTreeMap<u64, Value>,
    pub firewalls: BTreeMap<u64, Value>,
    pub next: u64,
    pub calls: Vec<String>,
    pub tokens: Vec<String>,
    pub read_only: bool,
    pub bad_token: bool,
    /// Answer the next call with 429.
    pub throttle: u32,
    /// Types that answer "resource unavailable" on create.
    pub sold_out: Vec<String>,
    pub creates: Vec<Value>,
}

pub type S = Arc<Mutex<Fake>>;

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

pub async fn handle(
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

/// Serves the fake on a free loopback port: its `/v1` API root and state.
pub async fn serve() -> (String, S) {
    let s: S = Arc::default();
    let app = axum::Router::new().fallback(handle).with_state(s.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/v1"), s)
}
