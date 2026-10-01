// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! JSON-RPC 2.0 framing of `/mcp`: every request's response carries its
//! `id` (errors included), notifications get none, and malformed messages
//! are `-32600`. Regression: an MCP client that opens with `server/discover`
//! (Google Antigravity's) aborted on an id-less `-32600`.

mod common;

use common::*;
use serde_json::{json, Value};

async fn post(t: &Target, body: Value) -> (http::StatusCode, Value) {
    let request = http::Request::post(t.endpoint().http_url("/mcp"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header(
            cua_proto::metadata::ENV_AUTHORIZATION,
            format!("Bearer {}", t.token),
        )
        .body(full(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = http_client().request(request).await.unwrap();
    let status = response.status();
    let bytes = body_bytes(response, 1 << 20).await;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

#[tokio::test]
async fn server_discover_answers_with_the_request_id() {
    let t = target().await;
    if t.local.is_none() {
        return; // exercises the in-process server only
    }
    // A modern client: discovery succeeds.
    let (status, v) = post(
        &t,
        json!({"jsonrpc": "2.0", "id": 7, "method": "server/discover",
               "params": {"_meta": modern_meta()}}),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{v}");
    assert_eq!(v["id"], json!(7), "{v}");
    assert!(v["result"]["supportedVersions"].is_array(), "{v}");

    // Without (or with foreign) per-request metadata it is an error the
    // client can correlate, never an id-less -32600.
    for (id, params) in [
        (json!(8), json!({})),
        (json!("disc-9"), Value::Null),
        (
            json!(10),
            json!({"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2025-01-01",
                "io.modelcontextprotocol/clientCapabilities": {}
            }}),
        ),
    ] {
        let mut body = json!({"jsonrpc": "2.0", "id": id, "method": "server/discover"});
        if !params.is_null() {
            body["params"] = params;
        }
        let (status, v) = post(&t, body).await;
        assert_eq!(status, http::StatusCode::OK, "{v}");
        assert_eq!(v["id"], id, "{v}");
        let code = v["error"]["code"].as_i64().unwrap_or_else(|| panic!("{v}"));
        assert_ne!(code, -32600, "{v}");
    }
    t.cleanup().await;
}

#[tokio::test]
async fn unknown_methods_notifications_and_malformed_messages() {
    let t = target().await;
    if t.local.is_none() {
        return;
    }
    // Unknown method: -32601 with the id.
    let (status, v) = post(&t, json!({"jsonrpc": "2.0", "id": 3, "method": "no/such"})).await;
    assert_eq!(status, http::StatusCode::OK, "{v}");
    assert_eq!(
        (v["id"].clone(), v["error"]["code"].clone()),
        (json!(3), json!(-32601)),
        "{v}"
    );

    // Notifications, known or not: 202 and no body.
    for method in [
        "notifications/initialized",
        "notifications/cancelled",
        "no/such",
    ] {
        let (status, v) = post(&t, json!({"jsonrpc": "2.0", "method": method})).await;
        assert_eq!(status, http::StatusCode::ACCEPTED, "{method}");
        assert!(v.is_null(), "{method}: {v}");
    }

    // Not a request object: -32600, with the id when it can be read.
    let (_, v) = post(&t, json!({"jsonrpc": "2.0", "id": 4})).await;
    assert_eq!(
        (v["id"].clone(), v["error"]["code"].clone()),
        (json!(4), json!(-32600)),
        "{v}"
    );
    let (_, v) = post(&t, json!(42)).await;
    assert_eq!(
        (v["id"].clone(), v["error"]["code"].clone()),
        (Value::Null, json!(-32600)),
        "{v}"
    );
    let (_, v) = post(&t, json!([])).await;
    assert_eq!(v["error"]["code"], json!(-32600), "{v}");

    // A batch answers each request by id and skips its notifications.
    let (status, v) = post(
        &t,
        json!([
            {"jsonrpc": "2.0", "id": "a", "method": "ping"},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": "b", "method": "no/such"}
        ]),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{v}");
    let items = v.as_array().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(items.len(), 2, "{v}");
    assert_eq!(items[0]["id"], json!("a"), "{v}");
    assert!(items[0]["result"].is_object(), "{v}");
    assert_eq!(
        (items[1]["id"].clone(), items[1]["error"]["code"].clone()),
        (json!("b"), json!(-32601)),
        "{v}"
    );
    t.cleanup().await;
}
