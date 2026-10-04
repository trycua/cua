// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `SpaceService`, the per-Space env passthrough and the daemon's MCP
//! streamable-HTTP endpoint, against a real cua-spacesd server core
//! in-process (`cua_spaces_e2e`: temp guest HOME/PATH, temp Downloads,
//! FakeHost teleport receiver, fake driver tools). Every wait is bounded.

use cua_daemon::{
    Runtime, RuntimeConfig,
    server::{self, DaemonHandle, ServerConfig},
};
use cua_proto::daemon::v1 as pb;
use cua_spaces::client::model::SpaceProvider;
use cua_spaces::client::transport::decode_tool_result;
use cua_spaces::client::{Connection, ToolTransport};
use cua_spaces_e2e::{self as testing, TOKEN};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

const DAEMON_TOKEN: &str = "daemon-token";

async fn daemon(dir: &std::path::Path) -> DaemonHandle {
    daemon_with_runtime(dir).await.1
}

async fn daemon_with_runtime(dir: &std::path::Path) -> (Runtime, DaemonHandle) {
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dir.join("sandboxes")),
        spaces_home: Some(dir.join("cua")),
        teleport_home: Some(dir.join("host-home")),
        env_probe_timeout: Some(Duration::from_secs(5)),
        // The Cua Spaces build of the daemon: teleport, the Cua
        // Drive, persistent agents and the Keyvault.
        extensions: vec![std::sync::Arc::new(
            cua_spaces_ext::daemon::CuaSpacesDaemon::default(),
        )],
        ..Default::default()
    })
    .unwrap();
    let h = server::start(
        runtime.clone(),
        ServerConfig {
            socket_path: None,
            loopback: Some("127.0.0.1:0".parse().unwrap()),
            token: DAEMON_TOKEN.into(),
            discovery_path: None,
            bridge_ticket_ttl: Duration::from_secs(30),
        },
    )
    .await
    .unwrap();
    (runtime, h)
}

fn client(h: &DaemonHandle) -> cua_daemon::client::DaemonClient {
    let addr = cua_daemon::client::DaemonAddress::resolve(
        h.loopback_url.as_deref(),
        Some(DAEMON_TOKEN.into()),
    )
    .unwrap();
    cua_daemon::client::DaemonClient::new(addr).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_service_delegates_to_cua_spaces_and_the_passthrough_carries_credentials() {
    let d = testing::driver().await;
    d.confine_direct().await;
    let dir = tempfile::tempdir().unwrap();
    let h = daemon(dir.path()).await;
    let c = client(&h);
    let mut svc = c.spaces();

    let added = svc
        .add_space(pb::AddSpaceRequest {
            url: d.url.clone(),
            token: TOKEN.into(),
            name: "svc".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .space
        .unwrap();
    assert!(added.id.starts_with("direct:"), "{}", added.id);
    assert!(added.features.iter().any(|f| f == "driver"));
    // The registry is cua-spaces' (tokens in the 0600 credentials file).
    let json = std::fs::read_to_string(dir.path().join("cua/spaces.json")).unwrap();
    assert!(json.contains(&added.id) && !json.contains(TOKEN));

    let listed = svc
        .list_spaces(pb::ListSpacesRequest {})
        .await
        .unwrap()
        .into_inner()
        .spaces;
    assert_eq!(listed, vec![added.clone()]);
    let resolved = svc
        .resolve_space(pb::ResolveSpaceRequest {
            space: "svc".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resolved.space.unwrap().id, added.id);

    // ConnectSpace: a passthrough that needs only the daemon token.
    let conn = svc
        .connect_space(pb::ConnectSpaceRequest {
            space: added.id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(conn.env_url.contains("/v1/spaces/"), "{}", conn.env_url);
    assert_eq!(conn.token, DAEMON_TOKEN);
    let caps = <cua_proto::env::v1::GetCapabilitiesResponse as prost::Message>::decode(
        conn.capabilities.as_ref(),
    )
    .unwrap();
    assert!(caps.features.iter().any(|f| f.name == "driver"));
    let mut o = cua_spacesd_client::ConnectOptions::parse(&conn.env_url).unwrap();
    o.token = Some(conn.token.clone());
    let env = cua_spacesd_client::SpacesdClient::connect(o).await.unwrap();
    let out = env
        .run(cua_spacesd_client::Command::shell("printf passthrough"))
        .await
        .unwrap();
    assert_eq!(out.stdout_str(), "passthrough");
    // Without the daemon token the passthrough refuses.
    let mut o = cua_spacesd_client::ConnectOptions::parse(&conn.env_url).unwrap();
    o.token = Some("wrong".into());
    assert!(cua_spacesd_client::SpacesdClient::connect(o).await.is_err());

    // CallSpaceTool is the MCP implementation.
    let r = svc
        .call_space_tool(pb::CallSpaceToolRequest {
            name: "space_bash".into(),
            arguments_json: json!({"space": added.id, "command": "echo via-tool"}).to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!r.is_error);
    let content: Value = serde_json::from_str(&r.content_json).unwrap();
    assert_eq!(content[0]["text"], "via-tool\n[exit 0]");
    let r = svc
        .call_space_tool(pb::CallSpaceToolRequest {
            name: "stream_endpoint".into(),
            arguments_json: json!({"space": added.id}).to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(r.is_error);
    let structured: Value = serde_json::from_str(&r.structured_json).unwrap();
    assert_eq!(structured["error"]["kind"], "capability_missing");
    let e = svc
        .call_space_tool(pb::CallSpaceToolRequest {
            name: "nope".into(),
            arguments_json: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), tonic::Code::NotFound);
    let tools: Value = serde_json::from_str(
        &svc.list_space_tools(pb::ListSpaceToolsRequest {})
            .await
            .unwrap()
            .into_inner()
            .tools_json,
    )
    .unwrap();
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, testing::contract_tool_names());

    // Errors carry the Spaces kind.
    let e = svc
        .add_space(pb::AddSpaceRequest {
            url: d.url.clone(),
            token: "wrong".into(),
            name: String::new(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(
        cua_daemon::Error::from_status(&e),
        cua_daemon::Error::Unauthenticated(_)
    ));

    let msg = svc
        .delete_space(pb::DeleteSpaceRequest {
            space: added.id.clone(),
        })
        .await
        .unwrap()
        .into_inner()
        .message;
    assert!(msg.contains(&added.id));
    assert!(
        svc.list_spaces(pb::ListSpacesRequest {})
            .await
            .unwrap()
            .into_inner()
            .spaces
            .is_empty()
    );
    let e = svc
        .remove_space(pb::RemoveSpaceRequest { id: added.id })
        .await
        .unwrap_err();
    assert_eq!(e.code(), tonic::Code::NotFound);
}

/// MCP streamable HTTP, blocking (the typed client is synchronous).
struct HttpMcp {
    addr: String,
    bearer: Option<String>,
    session: std::sync::Mutex<Option<String>>,
    next: std::sync::atomic::AtomicI64,
}

impl HttpMcp {
    fn post(&self, body: &Value) -> (u16, Vec<(String, String)>, String) {
        let body = body.to_string();
        let mut s = std::net::TcpStream::connect(&self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let mut req = format!(
            "POST /mcp HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\naccept: application/json, text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n",
            self.addr,
            body.len()
        );
        if let Some(b) = &self.bearer {
            req.push_str(&format!("authorization: Bearer {b}\r\n"));
        }
        if let Some(sid) = self.session.lock().unwrap().as_ref() {
            req.push_str(&format!("mcp-session-id: {sid}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(&body);
        s.write_all(req.as_bytes()).unwrap();
        let mut raw = Vec::new();
        // Bounded: connection: close ends the read; responses are small.
        s.take(16 << 20).read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let mut lines = head.lines();
        let status: u16 = lines
            .next()
            .and_then(|l| l.split(' ').nth(1))
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect::<Vec<_>>();
        let chunked = headers
            .iter()
            .any(|(k, v)| k == "transfer-encoding" && v.contains("chunked"));
        let body = if chunked {
            dechunk(body)
        } else {
            body.to_string()
        };
        (status, headers, body)
    }

    fn rpc(&self, method: &str, params: Value) -> Value {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (status, headers, body) =
            self.post(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        assert_eq!(status, 200, "{method}: {body}");
        if let Some((_, sid)) = headers.iter().find(|(k, _)| k == "mcp-session-id") {
            *self.session.lock().unwrap() = Some(sid.clone());
        }
        serde_json::from_str(&body).unwrap()
    }
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    for _ in 0..10_000 {
        let Some((size, tail)) = rest.split_once("\r\n") else {
            break;
        };
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || tail.len() < n {
            break;
        }
        out.push_str(&tail[..n]);
        rest = tail[n..].trim_start_matches("\r\n");
    }
    out
}

impl ToolTransport for HttpMcp {
    fn call_tool(&self, name: &str, arguments: &Value) -> cua_spaces::client::Result<Value> {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": arguments}));
        decode_tool_result(name, &r["result"])
    }

    fn available_tools(&self) -> cua_spaces::client::Result<Vec<String>> {
        let r = self.rpc("tools/list", json!({}));
        Ok(r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_over_streamable_http_decodes_through_the_typed_client() {
    let d = testing::driver().await;
    d.confine_direct().await;
    let dir = tempfile::tempdir().unwrap();
    let h = daemon(dir.path()).await;
    let addr = h
        .loopback_url
        .clone()
        .unwrap()
        .trim_start_matches("http://")
        .to_string();
    let url = d.url.clone();
    tokio::task::spawn_blocking(move || {
        // No bearer: the loopback listener refuses.
        let anon = HttpMcp {
            addr: addr.clone(),
            bearer: None,
            session: Default::default(),
            next: 1.into(),
        };
        let (status, _, _) = anon.post(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
        assert_eq!(status, 401);

        let t = Arc::new(HttpMcp {
            addr,
            bearer: Some(DAEMON_TOKEN.into()),
            session: Default::default(),
            next: 1.into(),
        });
        let init = t.rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "cua-spaces");
        assert!(t.session.lock().unwrap().is_some(), "session minted");

        let c = Arc::new(Connection::new(t.clone()));
        let info = c.add_space(&url, Some(TOKEN), Some("http")).unwrap();
        assert_eq!(info.provider, SpaceProvider::Direct);
        assert_eq!(c.available_tools().unwrap(), testing::contract_tool_names());
        let space = c.attach(&info.id, true).unwrap();
        let out = space.bash("echo over-http").unwrap();
        assert!(out.contains("over-http") && out.contains("[exit 0]"), "{out}");
        let parts = space.call_service_tool("get_screen_size", None, "{}").unwrap();
        assert!(format!("{parts:?}").contains("1280x800"));
        let e = space.stream_endpoint(true).unwrap_err();
        assert_eq!(e.tag(), "ToolFailed");
        c.remove_space(&info.id).unwrap();
        assert!(c.spaces().unwrap().is_empty());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn media_bridge_resolves_a_fleet_space_with_its_gateway_headers() {
    use cua_daemon::fixtures;
    use futures_util::StreamExt;
    const IMAGE: &str = "ghcr.io/trycua/linux:24.04-disk";
    const CLAIM: &str = "cua-e2e-bridge";
    let dir = tempfile::tempdir().unwrap();
    let fake = cua_fleet::testing::FakeFleet::new();
    // The pool name the Spaces runtime will pick, to aim the fake gateway.
    let pool = cua_spaces::Spaces::builder()
        .home(dir.path().join("probe"))
        .fleet(fake.client())
        .fleet_namespace("cua-e2e-sp")
        .build()
        .fleet_pool_name(cua_spaces::contract::inputs::FleetRuntime::Kubevirt, IMAGE);
    let gw = fixtures::start_env(None, Some(fixtures::fake_gateway(&pool, CLAIM))).await;
    // SAFETY: tests in this binary never read CUA_SPACES_NAMESPACE concurrently.
    unsafe { std::env::set_var("CUA_SPACES_NAMESPACE", "cua-e2e-sp") };
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().join("cua")),
        teleport_home: Some(dir.path().join("host-home")),
        fleet_client: Some(fake.client_with_base(&gw.url)),
        env_probe_timeout: Some(Duration::from_secs(5)),
        // The Cua Spaces build of the daemon: teleport, the Cua
        // Drive, persistent agents and the Keyvault.
        extensions: vec![std::sync::Arc::new(
            cua_spaces_ext::daemon::CuaSpacesDaemon::default(),
        )],
        ..Default::default()
    })
    .unwrap();
    let h = server::start(
        runtime,
        ServerConfig {
            socket_path: None,
            loopback: Some("127.0.0.1:0".parse().unwrap()),
            token: DAEMON_TOKEN.into(),
            discovery_path: None,
            bridge_ticket_ttl: Duration::from_secs(30),
        },
    )
    .await
    .unwrap();
    let c = client(&h);
    let claimed = c
        .spaces()
        .create_space(pb::CreateSpaceRequest {
            image: IMAGE.into(),
            location: "cloud".into(),
            name: CLAIM.into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .space
        .unwrap();
    assert_eq!(claimed.id, format!("cloud:{CLAIM}"));

    let bridge = c.open_media_bridge(&claimed.id, None).await.unwrap();
    assert!(bridge.ws_url.contains("/v1/bridge/media?ticket="));
    let (mut ws, _) = tokio_tungstenite::connect_async(bridge.ws_url.as_str())
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(first.to_text().unwrap().contains("hello"), "{first:?}");
    let log = gw.media.lock().unwrap().clone();
    assert_eq!(log.attaches, 1);
    assert_eq!(
        log.last_authorization.as_deref(),
        Some(format!("Bearer {}", fixtures::FAKE_FLEET_TOKEN).as_str())
    );
    assert_eq!(log.last_claim.as_deref(), Some(CLAIM));
    drop(ws);
    c.spaces()
        .delete_space(pb::DeleteSpaceRequest { space: claimed.id })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_service_lists_the_accounts_relay_machines() {
    let relay = cua_host::testing::FakeRelay::start().await;
    relay.add_account("owner-token", "user-1", Some("ada@example.com"));
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "owner-token",
            &cua_host::relay::RegisterRequest {
                id: "0123abcd4567ef89".into(),
                name: "studio".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    relay.set_online("0123abcd4567ef89", true, "1.2.3");
    let dir = tempfile::tempdir().unwrap();
    let (rt, h) = daemon_with_runtime(dir.path()).await;
    rt.spaces().set_relay(Some(cua_spaces::RelayAccount::new(
        relay.url.clone(),
        Arc::new(cua_spaces::relay::StaticToken("owner-token".into())),
    )));
    let c = client(&h);
    let listed = c
        .spaces()
        .list_spaces(pb::ListSpacesRequest {})
        .await
        .unwrap()
        .into_inner()
        .spaces;
    let row = listed
        .iter()
        .find(|s| s.id == "relay:0123abcd4567ef89")
        .unwrap_or_else(|| panic!("relay machine missing: {listed:?}"));
    assert_eq!(row.name, "studio");
    assert_eq!(row.spacesd_version, "1.2.3");
    // ResolveSpace finds it by name without a registry entry.
    let resolved = c
        .spaces()
        .resolve_space(pb::ResolveSpaceRequest {
            space: "studio".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .space
        .unwrap();
    assert_eq!(resolved.id, row.id);
    // Signed out: the listing still works, without relay rows.
    rt.spaces().set_relay(Some(cua_spaces::RelayAccount::new(
        relay.url.clone(),
        Arc::new(cua_spaces::relay::NoAccount),
    )));
    let listed = c
        .spaces()
        .list_spaces(pb::ListSpacesRequest {})
        .await
        .unwrap()
        .into_inner()
        .spaces;
    assert!(listed.iter().all(|s| !s.id.starts_with("relay:")));
}
