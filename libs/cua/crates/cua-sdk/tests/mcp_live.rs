//! Live MCP runs through the pipe with the official Rust SDK (rmcp). Opt-in,
//! never on by default; everything is named `cua-e2e-*`, memory-capped and
//! deleted in the test.
//!
//! - `CUA_E2E_MCP_EVERYTHING_IMAGE=<image>`: the official reference
//!   "everything" server (build `tests/fixtures/mcp-everything`) as a local
//!   container sandbox (gVisor when the engine has runsc, 1 GiB). Reached
//!   embedded (loopback), through `cua daemon`'s passthrough, and as a Space.
//! - `CUA_E2E_MCP_DRIVER_IMAGE=<image>`: a linux container whose
//!   cua-spacesd serves cua-driver's own MCP at `/mcp`; a screenshot comes
//!   back as an intact image block. Never the host: the desktop is the
//!   container's own Xvfb.
//! - `CUA_E2E_MCP_FLEET=1` (+ Fleet credentials): the everything server on
//!   a Fleet gVisor Space (`node:22-slim` + `command`), through the gateway.

use base64::Engine;
use cua_daemon::server::{self, ServerConfig};
use cua_sdk::{
    Cua, CuaConfig, HttpHeader, McpClient, ReadinessProbe, SandboxCreateOptions, SpaceCreateOptions,
};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{collections::HashMap, panic::AssertUnwindSafe, sync::Arc, time::Duration};

fn suffix() -> String {
    format!(
        "{:08x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    )
}

fn cua(dirs: &tempfile::TempDir, fleet: bool) -> Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(dirs.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dirs.path().join("cua").display().to_string()),
        fleet_pool_home: Some(dirs.path().join("pools").display().to_string()),
        fleet_from_env: fleet,
        ..Default::default()
    })
    .unwrap()
}

fn local(
    image: &str,
    name: &str,
    port: u16,
    service: &str,
    env: HashMap<String, String>,
) -> SandboxCreateOptions {
    let mut o = SandboxCreateOptions::new("local", image);
    o.name = Some(name.into());
    o.cpus = Some(1);
    o.memory_mb = Some(1024);
    o.services = HashMap::from([(service.to_string(), port)]);
    o.wait_for = vec![ReadinessProbe::tcp(service)];
    o.ready_timeout_ms = Some(180_000);
    o.env = env;
    o
}

async fn call(mcp: &McpClient, tool: &str, args: Value) -> Value {
    serde_json::from_str(
        &mcp.call_tool(tool.into(), Some(args.to_string()))
            .await
            .unwrap(),
    )
    .unwrap()
}

/// The reference server's content types all arrive intact.
async fn exercise_everything(mcp: &McpClient, via: &str) {
    let info: Value = serde_json::from_str(&mcp.server_info_json().unwrap()).unwrap();
    eprintln!(
        "[{via}] server {} (MCP {})",
        info["serverInfo"], info["protocolVersion"]
    );
    let tools: Value = serde_json::from_str(&mcp.list_tools().await.unwrap()).unwrap();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"get-tiny-image"), "{names:?}");

    let r = call(mcp, "get-tiny-image", json!({})).await;
    let img = r["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "image")
        .expect("image block");
    assert_eq!(img["mimeType"], "image/png");
    let png = base64::engine::general_purpose::STANDARD
        .decode(img["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "[{via}] PNG intact");

    let r = call(mcp, "get-resource-links", json!({"count": 3})).await;
    let links = r["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["type"] == "resource_link")
        .count();
    assert_eq!(links, 3, "[{via}] {r}");

    let r = call(
        mcp,
        "get-structured-content",
        json!({"location": "New York"}),
    )
    .await;
    assert!(r["structuredContent"].is_object(), "[{via}] {r}");

    let r = call(mcp, "echo", json!({"message": "héllo ✓"})).await;
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("héllo ✓")
    );

    // A long-running call streams progress over SSE before its result.
    let r = call(
        mcp,
        "trigger-long-running-operation",
        json!({"duration": 2, "steps": 4}),
    )
    .await;
    assert!(!r["isError"].as_bool().unwrap_or(false), "[{via}] {r}");

    let res: Value = serde_json::from_str(&mcp.list_resources().await.unwrap()).unwrap();
    let uri = res[0]["uri"].as_str().unwrap().to_string();
    let read: Value = serde_json::from_str(&mcp.read_resource(uri).await.unwrap()).unwrap();
    assert!(
        read["contents"][0]
            .get("text")
            .or(read["contents"][0].get("blob"))
            .is_some()
    );
    let prompts: Value = serde_json::from_str(&mcp.list_prompts().await.unwrap()).unwrap();
    assert!(!prompts.as_array().unwrap().is_empty());
    mcp.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn everything_server_local_daemon_and_space() {
    let Ok(image) = std::env::var("CUA_E2E_MCP_EVERYTHING_IMAGE") else {
        eprintln!("skipped: set CUA_E2E_MCP_EVERYTHING_IMAGE");
        return;
    };
    let dirs = tempfile::tempdir().unwrap();
    let cua = cua(&dirs, false);
    let name = format!("cua-e2e-mcp-everything-{}", suffix());
    let sb = cua
        .sandboxes()
        .create(local(&image, &name, 3001, "mcp", HashMap::new()))
        .await
        .unwrap();
    let run = AssertUnwindSafe(async {
        // Embedded: the loopback route.
        exercise_everything(&sb.mcp("mcp".into(), None).await.unwrap(), "local").await;

        // Through a cua daemon over the same state: its streaming passthrough.
        let rt = cua_daemon::Runtime::new(cua_daemon::RuntimeConfig {
            state_dir: Some(dirs.path().join("sandboxes")),
            spaces_home: Some(dirs.path().join("cua-daemon")),
            vmm: Some(Arc::new(cua_daemon::local::VmmLocal::default())),
            ..Default::default()
        })
        .unwrap();
        let h = server::start(
            rt,
            ServerConfig {
                socket_path: None,
                loopback: Some("127.0.0.1:0".parse().unwrap()),
                token: format!("{:032x}", rand_u128()),
                discovery_path: Some(dirs.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            },
        )
        .await
        .unwrap();
        let client = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
        let dsb = client.sandboxes().connect(name.clone()).await.unwrap();
        let config = dsb.mcp_config("mcp".into(), None).await.unwrap();
        assert!(config.url.contains("/v1/sandboxes/"), "{}", config.url);
        exercise_everything(&cua_sdk::mcp_connect(config).await.unwrap(), "daemon").await;

        // As a Space added by its MCP URL: call_tool forwards blocks verbatim.
        let url = sb.mcp_config("mcp".into(), None).await.unwrap().url;
        let spaces = cua.spaces();
        let info = spaces
            .add(url, None, Some(format!("{name}-space")))
            .await
            .unwrap();
        let space = spaces.space(info.id.clone()).await.unwrap();
        let r = space
            .call_tool(
                "get-tiny-image".into(),
                None,
                Some("mcp".into()),
                Some(30_000),
            )
            .await
            .unwrap();
        let content: Value = serde_json::from_str(&r.content_json).unwrap();
        assert!(
            content
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["type"] == "image"),
            "{content}"
        );
        spaces.remove(info.id).await.unwrap();
    })
    .catch_unwind()
    .await;
    sb.delete().await.unwrap();
    if let Err(p) = run {
        std::panic::resume_unwind(p);
    }
}

fn rand_u128() -> u128 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    t.as_nanos() ^ 0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C834
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cua_driver_mcp_returns_an_intact_screenshot() {
    let Ok(image) = std::env::var("CUA_E2E_MCP_DRIVER_IMAGE") else {
        eprintln!("skipped: set CUA_E2E_MCP_DRIVER_IMAGE (a linux image)");
        return;
    };
    let dirs = tempfile::tempdir().unwrap();
    let cua = cua(&dirs, false);
    let name = format!("cua-e2e-mcp-driver-{}", suffix());
    let token = format!("{:032x}", rand_u128());
    let mut o = local(
        &image,
        &name,
        3211,
        "env",
        HashMap::from([("CUA_ENV_TOKEN".into(), token.clone())]),
    );
    o.memory_mb = Some(2048);
    o.token = Some(token.clone());
    let sb = cua.sandboxes().create(o).await.unwrap();
    let run = AssertUnwindSafe(async {
        let mut config = sb.mcp_config("env".into(), None).await.unwrap();
        config.headers.push(HttpHeader {
            name: "authorization".into(),
            value: format!("Bearer {token}"),
        });
        // The driver's desktop comes up a moment after its port.
        let mut last = String::new();
        for _ in 0..30 {
            match cua_sdk::mcp_connect(config.clone()).await {
                Ok(mcp) => {
                    let tools: Value =
                        serde_json::from_str(&mcp.list_tools().await.unwrap()).unwrap();
                    let shot = tools
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|t| t["name"].as_str())
                        .find(|n| *n == "screenshot" || *n == "get_desktop_state")
                        .unwrap_or_else(|| {
                            let names: Vec<&str> = tools
                                .as_array()
                                .unwrap()
                                .iter()
                                .filter_map(|t| t["name"].as_str())
                                .collect();
                            panic!("a screenshot tool in {names:?}")
                        })
                        .to_string();
                    let r = call(&mcp, &shot, json!({})).await;
                    if let Some(img) = r["content"]
                        .as_array()
                        .and_then(|a| a.iter().find(|c| c["type"] == "image"))
                    {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(img["data"].as_str().unwrap())
                            .unwrap();
                        let mime = img["mimeType"].as_str().unwrap_or_default();
                        eprintln!("screenshot: {mime}, {} bytes", bytes.len());
                        match mime {
                            "image/png" => {
                                assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
                                let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
                                let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
                                eprintln!("screenshot: {w}x{h}");
                                assert!(w >= 320 && h >= 200, "a real desktop, not a placeholder");
                            }
                            "image/jpeg" => assert_eq!(&bytes[..2], b"\xff\xd8"),
                            other => panic!("unexpected mime {other}"),
                        }
                        mcp.close().await.unwrap();
                        return;
                    }
                    last = r.to_string();
                    mcp.close().await.unwrap();
                }
                Err(e) => last = e.to_string(),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        panic!("no screenshot: {last}");
    })
    .catch_unwind()
    .await;
    sb.delete().await.unwrap();
    if let Err(p) = run {
        std::panic::resume_unwind(p);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn everything_server_on_fleet_through_the_gateway() {
    if std::env::var("CUA_E2E_MCP_FLEET").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_E2E_MCP_FLEET=1 with Fleet credentials");
        return;
    }
    let dirs = tempfile::tempdir().unwrap();
    let ns = format!("cua-e2e-mcp-{}", suffix());
    // SAFETY: set before the runtime reads it; the other tests never do.
    unsafe { std::env::set_var("CUA_SPACES_NAMESPACE", &ns) };
    let cua = cua(&dirs, true);
    let spaces = cua.spaces();
    let claim = format!("{ns}-claim");
    let image = "docker.io/library/node:22-slim";
    let command: Vec<String> = [
        "npx",
        "-y",
        "@modelcontextprotocol/server-everything@2026.8.31",
        "streamableHttp",
    ]
    .map(String::from)
    .to_vec();
    let pool = cua_spaces::Spaces::builder()
        .home(dirs.path().join("names"))
        .fleet_namespace(ns.clone())
        .build()
        .fleet_pool_name(
            cua_spaces::contract::inputs::FleetRuntime::Gvisor,
            &cua_spaces::pool_key(
                image,
                Some(&command),
                &Default::default(),
                &[("mcp".to_string(), 3001u16)].into(),
            ),
        );
    let r = spaces
        .create(SpaceCreateOptions {
            on: Some("cloud".into()),
            image: Some(image.into()),
            runtime: Some("gvisor".into()),
            name: Some(claim.clone()),
            wait: Some(true),
            command: Some(command.clone()),
            services: HashMap::from([("mcp".to_string(), 3001u16)]),
            spacesd: Some(false),
            ..Default::default()
        })
        .await;
    let run = AssertUnwindSafe(async {
        let info = r.unwrap().space.expect("bound");
        let space = spaces.space(info.id.clone()).await.unwrap();
        let r = space
            .call_tool(
                "get-tiny-image".into(),
                None,
                Some("mcp".into()),
                Some(60_000),
            )
            .await
            .unwrap();
        let content: Value = serde_json::from_str(&r.content_json).unwrap();
        let img = content
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["type"] == "image")
            .expect("image");
        let png = base64::engine::general_purpose::STANDARD
            .decode(img["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        spaces.delete(info.id).await.unwrap();
    })
    .catch_unwind();
    let run = tokio::time::timeout(Duration::from_secs(300), run).await;
    let fleet = cua_fleet::FleetClient::from_env().unwrap();
    let _ = fleet.release(&pool, &claim).await;
    match fleet.get_pool(&pool).await {
        Ok(h) => match fleet.delete_pool(h).await {
            Ok(()) => eprintln!("deleted pool {pool}"),
            Err(e) => eprintln!("LEFT BEHIND: pool {pool}: {e}"),
        },
        Err(e) => eprintln!("pool {pool}: {e}"),
    }
    if let Err(p) = run.expect("Fleet run timed out") {
        std::panic::resume_unwind(p);
    }
}
