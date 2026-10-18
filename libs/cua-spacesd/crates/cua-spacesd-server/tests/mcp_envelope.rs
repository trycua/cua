// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Typed cua-driver envelopes over `/mcp` (`ai.cua.driver.envelopes` v1),
//! against the in-process server and its fake tool provider. Wire contract:
//! libs/cua-driver/docs/mcp-envelope-carrier.md.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::*;
use serde_json::{json, Value};

struct Mcp {
    url: String,
    token: String,
    session: Option<String>,
    next: u64,
}

impl Mcp {
    async fn post(&mut self, body: Value, token: Option<&str>) -> (http::StatusCode, Value) {
        let mut request = http::Request::post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(token) = token {
            request = request.header(
                cua_proto::metadata::ENV_AUTHORIZATION,
                format!("Bearer {token}"),
            );
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        let response = http_client()
            .request(
                request
                    .body(full(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        if self.session.is_none() {
            self.session = response
                .headers()
                .get("mcp-session-id")
                .map(|v| v.to_str().unwrap().to_owned());
        }
        let bytes = body_bytes(response, 16 << 20).await;
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, value)
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let token = self.token.clone();
        let (status, value) = self
            .post(
                json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params}),
                Some(&token),
            )
            .await;
        assert_eq!(status, http::StatusCode::OK, "{value}");
        assert_eq!(value["id"], json!(self.next), "{value}");
        value
    }

    async fn delete(&self) -> http::StatusCode {
        let request = http::Request::delete(&self.url)
            .header(
                cua_proto::metadata::ENV_AUTHORIZATION,
                format!("Bearer {}", self.token),
            )
            .header("mcp-session-id", self.session.clone().unwrap())
            .body(full(Vec::new()))
            .unwrap();
        http_client().request(request).await.unwrap().status()
    }
}

async fn initialized(t: &Target) -> (Mcp, Value) {
    let mut mcp = Mcp {
        url: t.endpoint().http_url("/mcp"),
        token: t.token.clone(),
        session: None,
        next: 0,
    };
    let init = mcp
        .rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
        )
        .await;
    let token = mcp.token.clone();
    let (status, _) = mcp
        .post(
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Some(&token),
        )
        .await;
    assert_eq!(status, http::StatusCode::ACCEPTED);
    (mcp, init)
}

fn deadline() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 60_000
}

fn envelope(id: &str, operation: &str, name: Option<&str>) -> Value {
    let mut envelope = json!({
        "envelope_version": 1,
        "request_id": id,
        "operation": operation,
        "deadline_unix_ms": deadline(),
    });
    if let Some(name) = name {
        envelope["name"] = json!(name);
        envelope["arguments"] = json!({});
    }
    envelope
}

fn code(response: &Value) -> i64 {
    response["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected an error: {response}"))
}

#[tokio::test]
async fn envelopes_open_exchange_cancel_close() {
    let t = target().await;
    if t.local.is_none() {
        return; // exercises the in-process fake tools only
    }
    let (mut mcp, init) = initialized(&t).await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "{init}");
    assert_eq!(
        init["result"]["capabilities"]["experimental"]["ai.cua.driver.envelopes"],
        json!({"version": 1}),
        "{init}"
    );

    let open = mcp.rpc("cua/driver/v1/open", json!({})).await["result"].clone();
    let connection = open["connection_id"].as_str().unwrap().to_owned();
    let generation = open["generation"].as_str().unwrap().to_owned();
    assert_eq!(open["public_session"].as_str(), mcp.session.as_deref());
    assert_eq!(
        open["capabilities"],
        json!({"minimum_envelope_version": 1, "maximum_envelope_version": 1, "supports_cancellation": true})
    );
    let binding = json!({"connection_id": connection, "generation": generation});

    // metadata carries the linked cua-driver contract versions.
    let mut params = binding.clone();
    params["envelope"] = envelope("m-1", "metadata", None);
    let metadata = mcp.rpc("cua/driver/v1/exchange", params).await["result"].clone();
    assert_eq!(metadata["ok"], true, "{metadata}");
    assert_eq!(
        metadata["result"]["contract_version"],
        cua_driver_contract::CONTRACT_VERSION
    );
    assert_eq!(metadata["result"]["embedded"], false);

    // list advertises only the remote desktop subset.
    let mut params = binding.clone();
    params["envelope"] = envelope("l-1", "list", None);
    let list = mcp.rpc("cua/driver/v1/exchange", params).await["result"].clone();
    assert_eq!(list["ok"], true, "{list}");
    let names: Vec<_> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["get_screen_size"], "{list}");

    // call returns the MCP CallToolResult verbatim (the SDK's raw_json).
    let mut params = binding.clone();
    params["envelope"] = envelope("c-1", "call", Some("get_screen_size"));
    let call = mcp.rpc("cua/driver/v1/exchange", params).await["result"].clone();
    assert_eq!(call["ok"], true, "{call}");
    assert_eq!(call["request_id"], "c-1");
    assert_eq!(call["completion_known"], true);
    assert_ne!(call["result"]["isError"], true, "{call}");
    assert_eq!(call["result"]["structuredContent"]["width"], 1280, "{call}");

    // A tool outside the remote subset is refused without dispatch.
    let mut params = binding.clone();
    params["envelope"] = envelope("c-2", "call", Some("echo"));
    let refused = mcp.rpc("cua/driver/v1/exchange", params).await["result"].clone();
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error_code"], "invalid_request", "{refused}");

    // A duplicate request identity is never replayed.
    let mut params = binding.clone();
    params["envelope"] = envelope("c-1", "call", Some("get_screen_size"));
    let duplicate = mcp.rpc("cua/driver/v1/exchange", params).await["result"].clone();
    assert_eq!(duplicate["error_code"], "duplicate_request", "{duplicate}");

    // Cancel acknowledges; a cancelled identity never dispatches afterwards.
    let mut params = binding.clone();
    params["request_id"] = json!("c-3");
    let cancel = mcp.rpc("cua/driver/v1/cancel", params).await;
    assert_eq!(cancel["result"], json!({"ok": true}), "{cancel}");
    let mut params = binding.clone();
    params["envelope"] = envelope("c-3", "call", Some("get_screen_size"));
    let late = mcp.rpc("cua/driver/v1/exchange", params).await["result"].clone();
    assert_eq!(late["ok"], false, "{late}");

    // Close; the closed receiver refuses further exchanges.
    let close = mcp.rpc("cua/driver/v1/close", binding.clone()).await;
    assert_eq!(close["result"], json!({"ok": true}), "{close}");
    let mut params = binding.clone();
    params["envelope"] = envelope("c-4", "call", Some("get_screen_size"));
    let after = mcp.rpc("cua/driver/v1/exchange", params).await["result"].clone();
    assert_eq!(after["ok"], false, "{after}");
    assert_eq!(after["error_code"], "connection_closed", "{after}");

    // Deleting the MCP session drops its receivers.
    assert_eq!(mcp.delete().await, http::StatusCode::NO_CONTENT);
    let mut params = binding.clone();
    params["envelope"] = envelope("c-5", "list", None);
    assert_eq!(
        code(&mcp.rpc("cua/driver/v1/exchange", params).await),
        -32404
    );
    t.cleanup().await;
}

#[tokio::test]
async fn envelope_errors_match_the_wire_contract() {
    let t = target().await;
    if t.local.is_none() {
        return;
    }
    let (mut mcp, _) = initialized(&t).await;
    let open = mcp.rpc("cua/driver/v1/open", json!({})).await["result"].clone();
    let connection = open["connection_id"].as_str().unwrap().to_owned();
    let generation = open["generation"].as_str().unwrap().to_owned();

    // Malformed parameters: -32602.
    for (method, params) in [
        ("cua/driver/v1/open", json!({"extra": 1})),
        ("cua/driver/v1/open", Value::Null),
        (
            "cua/driver/v1/close",
            json!({"connection_id": "not-a-uuid", "generation": generation}),
        ),
        (
            "cua/driver/v1/close",
            json!({"connection_id": connection, "generation": generation, "extra": true}),
        ),
        (
            "cua/driver/v1/exchange",
            json!({"connection_id": connection, "generation": generation}),
        ),
        ("cua/driver/v1/unknown", json!({})),
    ] {
        let response = mcp.rpc(method, params.clone()).await;
        assert_eq!(code(&response), -32602, "{method} {params}: {response}");
    }

    // Stale generation: -32409.
    let stale = uuid::Uuid::new_v4().to_string();
    let mut params = json!({"connection_id": connection, "generation": stale});
    params["envelope"] = envelope("x-1", "list", None);
    assert_eq!(
        code(&mcp.rpc("cua/driver/v1/exchange", params).await),
        -32409
    );

    // A connection of another MCP session is foreign: -32404.
    let (mut other, _) = initialized(&t).await;
    assert_ne!(other.session, mcp.session);
    let mut params = json!({"connection_id": connection, "generation": generation});
    params["envelope"] = envelope("x-2", "list", None);
    assert_eq!(
        code(&other.rpc("cua/driver/v1/exchange", params).await),
        -32404
    );

    // A malformed envelope is the receiver service's 400.
    let params =
        json!({"connection_id": connection, "generation": generation, "envelope": {"bogus": 1}});
    assert_eq!(
        code(&mcp.rpc("cua/driver/v1/exchange", params).await),
        -32400
    );

    // Authentication still applies.
    let (status, _) = mcp
        .post(
            json!({"jsonrpc": "2.0", "id": 99, "method": "cua/driver/v1/open", "params": {}}),
            None,
        )
        .await;
    assert_eq!(status, http::StatusCode::UNAUTHORIZED);
    t.cleanup().await;
}
