// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `mcp`: the streamable-HTTP `/mcp` endpoint answers `initialize` and its
//! `tools/list` is the same registry `DriverService.ListTools` serves.

use std::collections::BTreeSet;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, HttpCall};
use serde_json::{json, Value};

use crate::{Ctx, Recorder};

async fn rpc(
    ctx: &Ctx,
    session: Option<&str>,
    body: Value,
) -> Result<(Value, Option<String>), String> {
    let mut headers = vec![
        ("content-type".to_owned(), "application/json".to_owned()),
        (
            "accept".to_owned(),
            "application/json, text/event-stream".to_owned(),
        ),
    ];
    if let Some(session) = session {
        headers.push(("mcp-session-id".to_owned(), session.to_owned()));
    }
    let path = ctx
        .caps
        .side_channels
        .as_ref()
        .map(|s| s.mcp_http_path.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| cua_proto::metadata::MCP_PATH.to_owned());
    let reply = ctx
        .client
        .http(HttpCall {
            method: "POST".into(),
            path,
            headers,
            body: body.to_string().into_bytes(),
            timeout: Some(Duration::from_secs(20)),
            max_response_bytes: Some(4 * 1024 * 1024),
        })
        .await
        .map_err(|e| e.to_string())?;
    if reply.status != 200 {
        return Err(format!(
            "HTTP {}: {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
                .chars()
                .take(200)
                .collect::<String>()
        ));
    }
    let session = reply
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("mcp-session-id"))
        .map(|(_, v)| v.clone());
    let value = serde_json::from_slice(&reply.body).map_err(|e| format!("not JSON: {e}"))?;
    Ok((value, session))
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("mcp") {
        return;
    }
    let claims: &[&str] = &["feature:driver"];
    let enabled = ctx
        .caps
        .side_channels
        .as_ref()
        .is_some_and(|s| !s.mcp_http_path.is_empty());
    if !enabled {
        rec.skip(
            "mcp.initialize",
            claims,
            "mcp_disabled",
            "GetCapabilities advertises no /mcp endpoint (--no-mcp, or no registry)".into(),
        )
        .await;
        return;
    }
    let mut session = None;
    rec.run("mcp.initialize", claims, Duration::from_secs(25), async {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "cua-spacesd-doctor", "version": env!("CARGO_PKG_VERSION")}}});
        match rpc(ctx, None, body).await {
            Ok((value, sid)) => {
                session = sid;
                let name = value["result"]["serverInfo"]["name"]
                    .as_str()
                    .unwrap_or_default();
                let version = value["result"]["serverInfo"]["version"]
                    .as_str()
                    .unwrap_or_default();
                if ctx.driver_version.lock().unwrap().is_empty() {
                    *ctx.driver_version.lock().unwrap() = version.to_owned();
                }
                Check::new(
                    "mcp.initialize",
                    super::verdict(
                        name == "cua-driver"
                            && value["result"]["capabilities"]["tools"].is_object(),
                    ),
                    format!(
                        "serverInfo {name} {version}, protocol {}",
                        value["result"]["protocolVersion"]
                    ),
                )
                .fact("server_version", version)
            }
            Err(error) => Check::new(
                "mcp.initialize",
                Status::Fail,
                format!("initialize: {error}"),
            ),
        }
    })
    .await;

    rec.run("mcp.tools_list", claims, Duration::from_secs(25), async {
        let listed = match rpc(
            ctx,
            session.as_deref(),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        )
        .await
        {
            Ok((value, _)) => value,
            Err(error) => {
                return Check::new(
                    "mcp.tools_list",
                    Status::Fail,
                    format!("tools/list: {error}"),
                )
            }
        };
        let mcp: BTreeSet<String> = listed["result"]["tools"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let grpc: BTreeSet<String> = match ctx
            .client
            .driver()
            .list_tools(pb::ListToolsRequest {})
            .await
        {
            Ok(r) => r.into_inner().tools.into_iter().map(|t| t.name).collect(),
            Err(status) => {
                return Check::new(
                    "mcp.tools_list",
                    Status::Fail,
                    format!("ListTools: {}", status.message()),
                );
            }
        };
        let only_mcp: Vec<&String> = mcp.difference(&grpc).collect();
        let only_grpc: Vec<&String> = grpc.difference(&mcp).collect();
        Check::new(
            "mcp.tools_list",
            super::verdict(only_mcp.is_empty() && only_grpc.is_empty() && !mcp.is_empty()),
            if only_mcp.is_empty() && only_grpc.is_empty() {
                format!("/mcp and DriverService list the same {} tools", mcp.len())
            } else {
                format!("tool sets differ: only /mcp {only_mcp:?}, only gRPC {only_grpc:?}")
            },
        )
    })
    .await;
}
