#![allow(deprecated)] // also exercises the deprecated `apply_pool` wrapper
//! The service pipe and rmcp over it: protocol transparency (a never-ending
//! SSE stream, every method, end-to-end headers, a large binary body, a
//! streamed request body) locally and through an emulated Fleet gateway,
//! and the official Rust MCP client (rmcp) getting multimodal content
//! intact. Servers are in-process (`cua_sandbox_core::testing`); nothing
//! starts a VM, container or Fleet claim.

use async_trait::async_trait;
use base64::Engine;
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::testing::{
    McpTestServer, PipeTestServer, TestServer, expected_result, png_bytes,
};
use cua_sandbox_core::{
    CreateOptions, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec,
    LocalSummary, McpConfig, ProviderKind, RuntimeError, RuntimeResult, Sandbox, Sandboxes,
    http::full,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};

/// A local runtime whose guest ports map to loopback listeners.
#[derive(Default)]
struct PortMapRuntime {
    ports: Mutex<BTreeMap<u16, u16>>,
    instances: Mutex<HashMap<String, InstanceStatus>>,
}

impl PortMapRuntime {
    fn endpoints(&self) -> LocalEndpoints {
        LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: self.ports.lock().unwrap().clone(),
            ..Default::default()
        }
    }
}

#[async_trait]
impl LocalRuntime for PortMapRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        self.instances
            .lock()
            .unwrap()
            .insert(spec.name.clone(), InstanceStatus::Running);
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(),
        })
    }
    async fn stop(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(),
        })
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        self.instances
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.instances.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(self.endpoints())
    }
}

async fn local_sandbox(server: &TestServer, dir: &tempfile::TempDir) -> Sandbox {
    let rt = Arc::new(PortMapRuntime::default());
    rt.ports.lock().unwrap().insert(8765, server.port());
    Sandboxes::builder()
        .local(rt)
        .state_dir(dir.path())
        .build()
        .create(
            CreateOptions::new(ProviderKind::Local, "docker.io/library/python:3.12-slim")
                .name("cua-e2e-pipe")
                .service("svc", 8765),
        )
        .await
        .unwrap()
}

const IMAGE: &str = "docker.io/library/python:3.12-slim";
const CLAIM: &str = "cua-e2e-pipe";

/// A Fleet sandbox whose gateway base is a loopback server: the fake binds
/// claim X to sandbox `sbx-X`, so service `svc` lives at
/// `/api/svc/<pool>/sbx-<claim>-svc`.
async fn fleet_sandbox(
    start: impl AsyncFnOnce(&str) -> TestServer,
    dir: &tempfile::TempDir,
) -> (Sandbox, TestServer) {
    let prefix = format!("/api/svc/{CLAIM}/sbx-{CLAIM}-svc");
    let server = start(&prefix).await;
    let fleet = FakeFleet::new();
    cua_fleet::testing::set_image_variant(IMAGE, cua_fleet::ImageVariant::Rootfs);
    let client = fleet.client_with_base(&server.url);
    client
        .apply_pool(
            &cua_fleet::PoolSpec::new(CLAIM, IMAGE)
                .runtime(cua_fleet::RuntimeKind::Gvisor)
                .services([("svc", 8765u16)]),
        )
        .await
        .unwrap();
    let sbx = Sandboxes::builder()
        .fleet(client)
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Fleet, IMAGE).name(CLAIM);
    o.fleet.pool = Some(CLAIM.into());
    (sbx.create(o).await.unwrap(), server)
}

fn header<'a>(r: &'a http::Response<hyper::body::Incoming>, k: &str) -> Option<&'a str> {
    r.headers().get(k).and_then(|v| v.to_str().ok())
}

/// The pipe is transparent: returns the headers the service saw.
async fn assert_pipe(sb: &Sandbox, gateway: bool) {
    let svc = sb.service("svc").unwrap();

    // 1. A never-ending SSE stream arrives event by event (the pipe does
    //    not wait for the end of the body).
    let resp = tokio::time::timeout(
        Duration::from_secs(5),
        svc.open(
            "GET",
            "/sse",
            &[
                ("accept".into(), "text/event-stream".into()),
                ("last-event-id".into(), "7".into()),
            ],
            full(Vec::new()),
            Some(Duration::from_secs(5)),
        ),
    )
    .await
    .expect("head arrives")
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "content-type"), Some("text/event-stream"));
    assert_eq!(header(&resp, "x-accel-buffering"), Some("no"));
    let mut body = resp.into_body();
    let mut seen = String::new();
    for _ in 0..50 {
        if seen.matches("\n\n").count() >= 3 {
            break;
        }
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .expect("an event within 2 s")
            .expect("stream open")
            .unwrap();
        if let Ok(data) = frame.into_data() {
            seen.push_str(&String::from_utf8_lossy(&data));
        }
    }
    assert!(seen.starts_with("id: 0\n"), "{seen}");
    assert!(seen.matches("\n\n").count() >= 3, "{seen}");
    drop(body);

    // 2. Every method and every end-to-end header, both ways.
    let headers: Vec<(String, String)> = [
        ("accept", "application/json, text/event-stream"),
        ("content-type", "application/json"),
        ("mcp-session-id", "s-1"),
        ("mcp-protocol-version", "2026-07-28"),
        ("mcp-method", "tools/call"),
        ("mcp-name", "add"),
        ("mcp-param-region", "us-west1"),
        ("last-event-id", "42"),
        (
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        ),
        ("connection", "x-drop-me"),
        ("x-drop-me", "hop"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for method in ["POST", "GET", "DELETE", "PUT"] {
        let resp = svc
            .open(method, "/echo", &headers, full(b"{\"x\":1}".to_vec()), None)
            .await
            .unwrap();
        assert_eq!(header(&resp, "x-echo-method"), Some(method));
        for (k, v) in &headers[..9] {
            assert_eq!(
                header(&resp, &format!("x-echo-{k}")),
                Some(v.as_str()),
                "{method} {k}"
            );
        }
        assert_eq!(
            header(&resp, "x-echo-connection"),
            None,
            "hop-by-hop dropped"
        );
        assert_eq!(
            header(&resp, "x-echo-x-drop-me"),
            None,
            "Connection-listed dropped"
        );
        if gateway {
            assert_eq!(
                header(&resp, "x-echo-authorization"),
                Some("Bearer fake-fleet-token")
            );
            assert_eq!(header(&resp, "x-echo-x-cua-fleet-claim"), Some(CLAIM));
        }
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"{\"x\":1}");
    }

    // 3. A streamed request body and a large binary response, byte-exact.
    let chunks = futures_util::stream::iter((0..16).map(|i| {
        Ok::<_, std::io::Error>(hyper::body::Frame::data(bytes::Bytes::from(vec![
            i as u8;
            4096
        ])))
    }));
    let streamed = http_body_util::StreamBody::new(chunks)
        .map_err(|e| Box::new(e) as cua_sandbox_core::http::BodyError)
        .boxed_unsync();
    let resp = svc
        .open("POST", "/echo", &[], streamed, None)
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.len(), 16 * 4096);
    assert_eq!(body[4096 * 15], 15);

    let n = 8 * 1024 * 1024 + 3;
    let r = svc
        .request(
            "GET",
            &format!("/big?bytes={n}"),
            None,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert_eq!(r.body.len(), n);
    assert!(r.body == png_bytes(n), "binary body intact");
}

#[tokio::test]
async fn local_pipe_is_transparent() {
    let server = PipeTestServer::start("").await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sb = local_sandbox(&server, &dir).await;
    assert_eq!(
        sb.service("svc").unwrap().endpoint().await.unwrap().url,
        server.url
    );
    assert_pipe(&sb, false).await;
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn fleet_gateway_pipe_is_transparent() {
    let dir = tempfile::tempdir().unwrap();
    let (sb, _server) = fleet_sandbox(
        async |p: &str| PipeTestServer::start(p).await.unwrap(),
        &dir,
    )
    .await;
    let ep = sb.service("svc").unwrap().endpoint().await.unwrap();
    assert!(
        ep.url
            .ends_with(&format!("/api/svc/{CLAIM}/sbx-{CLAIM}-svc")),
        "{ep:?}"
    );
    assert_pipe(&sb, true).await;
    sb.delete().await.unwrap();
}

/// rmcp over the pipe: every content block survives, byte-exact.
async fn assert_mcp(sb: &Sandbox) {
    let config = sb.mcp_config("svc", None).await.unwrap();
    assert!(config.url.ends_with("/mcp"), "{config:?}");
    let client = sb.mcp("svc", None).await.unwrap();
    let peer = client.peer();
    let tools = peer.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 5);

    for (name, args) in [
        ("add", json!({"a": 2, "b": 3})),
        ("image", json!({"bytes": 3 * 1024 * 1024})),
        ("audio", json!({})),
        ("resources", json!({})),
        ("fail", json!({})),
    ] {
        let r = peer
            .call_tool(
                rmcp::model::CallToolRequestParams::new(name.to_string())
                    .with_arguments(args.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        let got = serde_json::to_value(&r).unwrap();
        let want = expected_result(name, args);
        assert_eq!(
            got["content"], want["content"],
            "{name}: content blocks lossless"
        );
        assert_eq!(
            got.get("structuredContent"),
            want.get("structuredContent"),
            "{name}"
        );
        assert_eq!(
            got.get("isError").and_then(Value::as_bool).unwrap_or(false),
            want.get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "{name}"
        );
        if name == "image" {
            let data = got["content"][1]["data"].as_str().unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap();
            assert_eq!(bytes, png_bytes(3 * 1024 * 1024));
            assert_eq!(got["content"][1]["_meta"]["cua.test/source"], "png_bytes");
        }
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn rmcp_over_a_local_service() {
    let server = McpTestServer::start("").await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sb = local_sandbox(&server, &dir).await;
    assert_mcp(&sb).await;
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn rmcp_through_the_fleet_gateway() {
    let dir = tempfile::tempdir().unwrap();
    let (sb, _server) =
        fleet_sandbox(async |p: &str| McpTestServer::start(p).await.unwrap(), &dir).await;
    let config = sb.mcp_config("svc", None).await.unwrap();
    assert!(
        config
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer fake-fleet-token")
    );
    assert!(
        config
            .headers
            .iter()
            .any(|(k, v)| k == "x-cua-fleet-claim" && v == CLAIM)
    );
    assert_mcp(&sb).await;
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn mcp_config_for_a_plain_url() {
    let server = McpTestServer::start("").await.unwrap();
    let config = McpConfig::for_url(&server.url, vec![]).unwrap();
    assert_eq!(config.url, format!("{}/mcp", server.url));
    let client = config.connect().await.unwrap();
    assert_eq!(client.peer().list_all_tools().await.unwrap().len(), 5);
    client.cancel().await.unwrap();
    assert!(
        McpConfig::for_url("http://127.0.0.1:9/mcp", vec![])
            .unwrap()
            .connect()
            .await
            .is_err()
    );
}

/// The `env` service is cua-spacesd: its endpoint (and so `sb.mcp("env")`,
/// `cua sb mcp REF env ...`) carries the sandbox's spacesd token, which a
/// local sandbox minted at create; other services get no token.
#[tokio::test]
async fn env_service_carries_the_spacesd_token() {
    let server = PipeTestServer::start("").await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(PortMapRuntime::default());
    rt.ports.lock().unwrap().insert(3211, server.port());
    rt.ports.lock().unwrap().insert(8765, server.port());
    let mut options = CreateOptions::new(ProviderKind::Local, IMAGE)
        .name("cua-e2e-env-token")
        .service("svc", 8765);
    options.env_token = Some("local-env-token".into());
    let sb = Sandboxes::builder()
        .local(rt)
        .state_dir(dir.path())
        .build()
        .create(options)
        .await
        .unwrap();
    let want = "Bearer local-env-token";
    let env = sb.service("env").unwrap().endpoint().await.unwrap();
    assert!(
        env.headers
            .iter()
            .any(|(k, v)| k == "x-cua-env-authorization" && v == want),
        "{:?}",
        env.headers.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );
    // The token reaches the service on a request (what `/mcp` checks).
    let r = sb
        .service("env")
        .unwrap()
        .request(
            "POST",
            "/echo",
            Some(b"{}".to_vec()),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    let seen = r
        .headers
        .iter()
        .find(|(k, _)| k == "x-echo-x-cua-env-authorization")
        .map(|(_, v)| v.as_str());
    assert_eq!(seen, Some(want));
    // Any other service: no spacesd token.
    let svc = sb.service("svc").unwrap().endpoint().await.unwrap();
    assert!(
        !svc.headers
            .iter()
            .any(|(k, _)| k == "x-cua-env-authorization"),
        "a non-env service must not get the spacesd token"
    );
    sb.delete().await.unwrap();
}
