// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Plans from the Spaces API's entitlements (CUA-1659), against a fake API:
//! `/v1/usage` shows Pro and pooled Teams allowances, unknown users stay on
//! Free, and an API outage keeps the last known plan (never Free, never a
//! refusal).
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode as AxStatus;
use cua_relay::meter::{parse_allowances, MeterConfig};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::org_access::OrgAccessConfig;
use cua_relay::plan_source::PlansSource;
use cua_relay::server::{Relay, RelayConfig};
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

#[derive(Default)]
struct Api {
    down: bool,
    answers: HashMap<String, Value>,
    calls: usize,
}

async fn fake_api(state: Arc<Mutex<Api>>) -> String {
    let app = axum::Router::new().route(
        "/internal/v1/entitlements/{sub}",
        axum::routing::get(
            move |axum::extract::Path(sub): axum::extract::Path<String>| {
                let state = state.clone();
                async move {
                    let mut s = state.lock().unwrap();
                    s.calls += 1;
                    if s.down {
                        return (AxStatus::SERVICE_UNAVAILABLE, axum::Json(json!({})));
                    }
                    let v = s
                        .answers
                        .get(&sub)
                        .cloned()
                        .unwrap_or_else(|| json!({"sub": sub, "plan": "free", "orgs": []}));
                    (AxStatus::OK, axum::Json(v))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    url
}

async fn usage(base: &str, token: &str) -> Value {
    let r = reqwest::Client::new()
        .get(format!("{base}/v1/usage"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "usage {}", r.status());
    r.json().await.unwrap()
}

#[tokio::test]
async fn usage_shows_the_entitled_plan_and_keeps_it_through_an_outage() {
    let issuer = FakeIssuer::new(ISSUER);
    let api = Arc::new(Mutex::new(Api::default()));
    {
        let mut a = api.lock().unwrap();
        a.answers.insert(
            "pat".into(),
            json!({"sub": "pat", "plan": "pro", "status": "active", "orgs": []}),
        );
        a.answers.insert(
            "tia".into(),
            json!({"sub": "tia", "plan": "teams", "status": "active", "orgs": [
                {"id": "org-1", "alias": "acme", "role": "member", "plan": "teams",
                 "status": "active", "seats": 3, "features": [], "limits": {}}
            ]}),
        );
    }
    let api_url = fake_api(api.clone()).await;
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        meter: MeterConfig {
            allowances: parse_allowances("free=2/20,pro=5/200,teams=5/200").unwrap(),
            ..MeterConfig::default()
        },
        org_access: OrgAccessConfig {
            api_url: Some(api_url),
            plans_source: PlansSource::Entitlements,
            ..Default::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });

    // Pro on the first read (the relay asks the API before answering).
    let pat = issuer.token("pat", Some("pat@example.com"), "cua-relay", 600);
    let v = usage(&base, &pat).await;
    assert_eq!(v["plan"], "pro", "{v}");
    assert_eq!(v["live_sessions_limit"], 5, "{v}");
    assert_eq!(v["hours_limit"], 200, "{v}");
    assert_eq!(v["soft_limit_reached"], false, "{v}");

    // A Teams member draws from the org's pool: per seat times seats.
    let tia = issuer.token("tia", Some("tia@example.com"), "cua-relay", 600);
    let v = usage(&base, &tia).await;
    assert_eq!(v["plan"], "teams", "{v}");
    assert_eq!(v["team"]["id"], "org-1", "{v}");
    assert_eq!(v["team"]["seats"], 3, "{v}");
    assert_eq!(v["live_sessions_limit"], 15, "{v}");
    assert_eq!(v["hours_limit"], 600, "{v}");

    // Nothing bought: Free.
    let zed = issuer.token("zed", Some("zed@example.com"), "cua-relay", 600);
    let v = usage(&base, &zed).await;
    assert_eq!(v["plan"], "free", "{v}");
    assert_eq!(v["live_sessions_limit"], 2, "{v}");

    // Cached: a second read does not ask again.
    let calls = api.lock().unwrap().calls;
    let v = usage(&base, &pat).await;
    assert_eq!(v["plan"], "pro", "{v}");
    assert_eq!(api.lock().unwrap().calls, calls);

    // The API goes down: the last known plan stays (fail open).
    api.lock().unwrap().down = true;
    let v = usage(&base, &pat).await;
    assert_eq!(v["plan"], "pro", "{v}");
    // Someone never answered for is metered on Free, still not refused.
    let ned = issuer.token("ned", Some("ned@example.com"), "cua-relay", 600);
    let v = usage(&base, &ned).await;
    assert_eq!(v["plan"], "free", "{v}");

    let metrics = reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains("cua_relay_plans_source{source=\"entitlements\"} 1"),
        "{metrics}"
    );
    assert!(
        metrics.contains("cua_relay_plans_users_cached{plan=\"pro\"} 1"),
        "{metrics}"
    );
    assert!(
        metrics.contains("cua_relay_plans_users_cached{plan=\"teams\"} 1"),
        "{metrics}"
    );
    assert!(!metrics.contains("pat"), "no account data on /metrics");
}
