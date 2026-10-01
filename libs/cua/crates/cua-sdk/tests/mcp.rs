#![allow(deprecated)] // also exercises the deprecated `apply_pool` wrapper
//! MCP and the service pipe through the exported API, embedded and through
//! `cua daemon` (loopback): `Sandbox.mcp_config` / `Sandbox.mcp` (rmcp),
//! `Service.endpoint` streaming through the daemon passthrough (SSE that
//! never ends, headers, a large binary body), a Fleet sandbox behind an
//! emulated gateway, and a Space added by its MCP URL. Servers are
//! in-process (`cua_sandbox_core::testing`); waits are bounded.

use cua_daemon::{
    Runtime, RuntimeConfig,
    server::{self, ServerConfig},
};
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::testing::{McpTestServer, PipeTestServer, expected_result, png_bytes};
use cua_sdk::{Cua, CuaError, SandboxCreateOptions};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};

const IMAGE: &str = "docker.io/library/python:3.12-slim";
const CLAIM: &str = "cua-e2e-sdk-mcp";

fn opts(on: &str, name: &str) -> SandboxCreateOptions {
    SandboxCreateOptions {
        on: Some(on.into()),
        kind: None,
        runtime: None,
        image: String::new(),
        name: Some(name.into()),
        token: None,
        pool: None,
        os: None,
        cpus: None,
        memory_mb: None,
        ports: vec![],
        services: HashMap::new(),
        wait_for: vec![],
        ready_timeout_ms: None,
        env: HashMap::new(),
        fleet_replicas: None,
        fleet_ttl_seconds: None,
        warm: None,
        max_pool_size: None,
        command: None,
        cloud: None,
        sidecars: vec![],
        registry_secret: None,
        build: None,
        network: None,
        overlays: vec![],
        keep_on_failure: false,
        gpu: None,
    }
}

/// Every tool's result is the server's JSON, byte for byte.
async fn exercise(mcp: &cua_sdk::McpClient) {
    let tools: Value = serde_json::from_str(&mcp.list_tools().await.unwrap()).unwrap();
    assert_eq!(tools.as_array().unwrap().len(), 5);
    let info: Value = serde_json::from_str(&mcp.server_info_json().unwrap()).unwrap();
    assert!(
        info["protocolVersion"].as_str().unwrap().starts_with("20"),
        "{info}"
    );
    for (name, args) in [
        ("add", json!({"a": 2, "b": 3})),
        ("image", json!({"bytes": 1_500_000})),
        ("audio", json!({})),
        ("resources", json!({})),
        ("fail", json!({})),
    ] {
        let got: Value = serde_json::from_str(
            &mcp.call_tool(name.into(), Some(args.to_string()))
                .await
                .unwrap(),
        )
        .unwrap();
        let want = expected_result(name, args);
        assert_eq!(got["content"], want["content"], "{name}");
        assert_eq!(got.get("structuredContent"), want.get("structuredContent"));
    }
    assert!(matches!(
        mcp.call_tool("add".into(), Some("[1]".into())).await,
        Err(CuaError::InvalidArgument(_))
    ));
    mcp.close().await.unwrap();
}

/// Streams through `endpoint` (the daemon passthrough in daemon mode).
async fn assert_pipe(endpoint: cua_sdk::ServiceEndpoint) {
    let ep = cua_sandbox_core::ServiceEndpoint {
        url: endpoint.url,
        headers: endpoint
            .headers
            .into_iter()
            .map(|h| (h.name, h.value))
            .collect(),
    };
    let open = |method: &'static str, path: &'static str, headers: Vec<(String, String)>| {
        let ep = ep.clone();
        async move {
            cua_sandbox_core::http::open(
                &ep,
                method,
                path,
                &headers,
                cua_sandbox_core::http::full(b"ping".to_vec()),
                Some(Duration::from_secs(10)),
            )
            .await
            .unwrap()
        }
    };
    // A never-ending SSE stream arrives event by event.
    let resp = open("GET", "/sse", vec![]).await;
    assert_eq!(resp.status(), 200);
    let mut body = resp.into_body();
    let mut seen = String::new();
    for _ in 0..50 {
        if seen.matches("\n\n").count() >= 3 {
            break;
        }
        let f = tokio::time::timeout(Duration::from_secs(3), body.frame())
            .await
            .expect("an event within 3 s")
            .expect("open")
            .unwrap();
        if let Ok(d) = f.into_data() {
            seen.push_str(&String::from_utf8_lossy(&d));
        }
    }
    assert!(seen.matches("\n\n").count() >= 3, "{seen}");
    drop(body);
    // Methods, MCP headers and bodies pass through.
    for method in ["POST", "GET", "DELETE"] {
        let resp = open(
            method,
            "/echo",
            vec![
                ("mcp-session-id".into(), "s-9".into()),
                ("mcp-protocol-version".into(), "2025-11-25".into()),
                ("last-event-id".into(), "3".into()),
            ],
        )
        .await;
        let h = |k: &str| {
            resp.headers()
                .get(k)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        assert_eq!(h("x-echo-method").as_deref(), Some(method));
        assert_eq!(h("x-echo-mcp-session-id").as_deref(), Some("s-9"));
        assert_eq!(
            h("x-echo-mcp-protocol-version").as_deref(),
            Some("2025-11-25")
        );
        assert_eq!(h("x-echo-last-event-id").as_deref(), Some("3"));
        assert_eq!(
            h("x-echo-authorization"),
            None,
            "the daemon bearer stays in the daemon"
        );
        let b = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&b[..], b"ping");
    }
    // A large binary body, byte-exact.
    let n = 6 * 1024 * 1024 + 1;
    let resp = cua_sandbox_core::http::open(
        &ep,
        "GET",
        &format!("/big?bytes={n}"),
        &[],
        cua_sandbox_core::http::full(Vec::new()),
        None,
    )
    .await
    .unwrap();
    let b = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(b.to_vec() == png_bytes(n));
}

async fn suite(daemon: bool) {
    let dirs = tempfile::tempdir().unwrap();
    let mcp_server = McpTestServer::start("").await.unwrap();
    let pipe_server = PipeTestServer::start("").await.unwrap();
    // Fleet: the gateway base is a loopback MCP server under the claim's
    // service path (the fake binds claim X to sandbox `sbx-X`).
    let gw = McpTestServer::start(&format!("/api/svc/{CLAIM}/sbx-{CLAIM}-mcp"))
        .await
        .unwrap();
    let fleet = FakeFleet::new();
    cua_fleet::testing::set_image_variant(IMAGE, cua_fleet::ImageVariant::Rootfs);
    fleet
        .client_with_base(&gw.url)
        .apply_pool(
            &cua_fleet::PoolSpec::new(CLAIM, IMAGE)
                .runtime(cua_fleet::RuntimeKind::Gvisor)
                .services([("mcp", 8765u16)]),
        )
        .await
        .unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        fleet_client: Some(fleet.client_with_base(&gw.url)),
        env_probe_timeout: Some(Duration::from_secs(2)),
        ..Default::default()
    })
    .unwrap();
    let (cua, handle) = if daemon {
        let h = server::start(
            runtime,
            ServerConfig {
                socket_path: None,
                loopback: Some("127.0.0.1:0".parse().unwrap()),
                token: "daemon-token".into(),
                discovery_path: Some(dirs.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            },
        )
        .await
        .unwrap();
        (
            Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap(),
            Some(h),
        )
    } else {
        (Cua::from_runtime(runtime), None)
    };
    let sbx = cua.sandboxes();

    // A direct sandbox at the MCP server: its origin is the `env` service.
    let o = opts(&format!("direct:{}", mcp_server.url), "cua-e2e-direct-mcp");
    let sb = sbx.create(o).await.unwrap();
    let config = sb.mcp_config("env".into(), None).await.unwrap();
    if let Some(h) = &handle {
        let base = h.loopback_url.clone().unwrap();
        let authority = mcp_server.url.trim_start_matches("http://");
        assert_eq!(
            config.url,
            format!("{base}/v1/sandboxes/direct:{authority}/svc/env/mcp")
        );
        assert_eq!(config.headers[0].value, "Bearer daemon-token");
    } else {
        assert_eq!(config.url, format!("{}/mcp", mcp_server.url));
    }
    exercise(&sb.mcp("env".into(), None).await.unwrap()).await;
    exercise(&cua_sdk::mcp_connect(config).await.unwrap()).await;
    exercise(&sb.service("env".into()).unwrap().mcp(None).await.unwrap()).await;
    sb.delete().await.unwrap();

    // The raw pipe (through the daemon passthrough in daemon mode).
    let o = opts(
        &format!("direct:{}", pipe_server.url),
        "cua-e2e-direct-pipe",
    );
    let sb = sbx.create(o).await.unwrap();
    assert_pipe(sb.service("env".into()).unwrap().endpoint().await.unwrap()).await;
    sb.delete().await.unwrap();

    // Fleet, through the (emulated) gateway.
    let mut o = opts("cloud", CLAIM);
    o.pool = Some(CLAIM.into());
    o.image = IMAGE.into();
    let fsb = sbx.create(o).await.unwrap();
    exercise(&fsb.mcp("mcp".into(), None).await.unwrap()).await;
    fsb.delete().await.unwrap();

    exercise(
        &cua_sdk::mcp_connect_url(format!("{}/mcp", mcp_server.url), None)
            .await
            .unwrap(),
    )
    .await;

    // A Space from a plain MCP URL (daemon mode: the Space service
    // passthrough). Content blocks are forwarded verbatim.
    let spaces = cua.spaces();
    let info = spaces
        .add(
            format!("{}/mcp", mcp_server.url),
            None,
            Some("cua-e2e-mcp-space".into()),
        )
        .await
        .unwrap();
    assert!(info.features.is_empty(), "{info:?}");
    assert_eq!(info.services, ["mcp"]);
    let space = spaces.space(info.id.clone()).await.unwrap();
    assert_eq!(space.services(), ["mcp"]);
    assert_eq!(space.list_tools(None).await.unwrap().len(), 5);
    let r = space
        .call_tool(
            "image".into(),
            Some(r#"{"bytes":200000}"#.into()),
            Some("mcp".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&r.content_json).unwrap(),
        expected_result("image", json!({"bytes": 200000}))["content"]
    );
    assert!(matches!(
        space.bash("uname".into(), None).await,
        Err(CuaError::CapabilityMissing(_))
    ));
    spaces.remove(info.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded() {
    tokio::time::timeout(Duration::from_secs(120), suite(false))
        .await
        .expect("suite timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_over_loopback() {
    tokio::time::timeout(Duration::from_secs(120), suite(true))
        .await
        .expect("suite timed out");
}
