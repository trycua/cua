//! unified-sandbox-api (Rust). Scenario definition:
//! ../python/test_unified_api.py; the assertions here are the same.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use cua_sdk_e2e::cua::{
    CloudOptions, HttpHeader, ReadinessProbe, Sandbox, SandboxCreateOptions,
    SandboxPhase,
};
use cua_sdk_e2e::*;

const MIN: Duration = Duration::from_secs(60);
const ACCEPT: &str = "application/json, text/event-stream";

fn server() -> String {
    std::fs::read_to_string(repo().join("tests/e2e/cua-sdk/fixtures/mcp_probe_server.py")).unwrap()
}

fn hdr(name: &str, value: &str) -> HttpHeader {
    HttpHeader {
        name: name.into(),
        value: value.into(),
    }
}

fn rpc(method: &str, id: u32, params: Option<serde_json::Value>) -> Vec<u8> {
    let mut v = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method});
    if let Some(p) = params {
        v["params"] = p;
    }
    v.to_string().into_bytes()
}

/// A plain HTTP/1.1 request to a loopback `http://` URL (no SDK
/// credentials): (status, lowercased header block, body).
fn http(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Res<(u16, String, String)> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("not a loopback http URL: {url}"))?;
    let (addr, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut s = TcpStream::connect(addr)?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    for _ in 0..1024 {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Ok((status, head.to_ascii_lowercase(), body.to_string()))
}

fn join(url: &str, path: &str) -> String {
    format!("{}{path}", url.trim_end_matches('/'))
}

fn plain_checks(url: &str) -> Res {
    let (status, _, body) = http("GET", &join(url, "/health"), &[], "")?;
    assert_eq!(status, 200, "{body}");
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
    let (status, head, body) = http(
        "POST",
        &join(url, "/mcp"),
        &[("accept", ACCEPT), ("content-type", "application/json")],
        init,
    )?;
    assert_eq!(status, 200, "{body}");
    assert!(head.contains("mcp-session-id:"), "{head}");
    Ok(())
}

async fn mcp_through_service(sb: &Arc<Sandbox>, env_expected: bool) -> Res {
    let svc = sb.service("mcp".into())?;
    let ct = hdr("content-type", "application/json");
    let bare = svc
        .request(
            "POST".into(),
            "/mcp".into(),
            Some(rpc("x", 0, None)),
            Some(30_000),
            Some(vec![ct.clone()]),
        )
        .await?;
    assert_eq!(bare.status, 400, "the server refuses dropped headers");
    let init = svc
        .request(
            "POST".into(),
            "/mcp".into(),
            Some(rpc("initialize", 1, None)),
            Some(30_000),
            Some(vec![ct.clone(), hdr("accept", ACCEPT)]),
        )
        .await?;
    assert_eq!(init.status, 200);
    let sid = init
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("mcp-session-id"))
        .map(|h| h.value.clone())
        .ok_or("no mcp-session-id")?;
    let heads = vec![ct, hdr("accept", ACCEPT), hdr("mcp-session-id", &sid)];
    let call = svc
        .request(
            "POST".into(),
            "/mcp".into(),
            Some(rpc(
                "tools/call",
                2,
                Some(serde_json::json!({"name": "add", "arguments": {"a": 2, "b": 3}})),
            )),
            Some(30_000),
            Some(heads.clone()),
        )
        .await?;
    let v: serde_json::Value = serde_json::from_slice(&call.body)?;
    assert_eq!(v["result"]["content"][0]["text"], "5");
    if env_expected {
        let env = svc
            .request(
                "POST".into(),
                "/mcp".into(),
                Some(rpc(
                    "tools/call",
                    3,
                    Some(serde_json::json!({"name": "env"})),
                )),
                Some(30_000),
                Some(heads),
            )
            .await?;
        let v: serde_json::Value = serde_json::from_slice(&env.body)?;
        assert_eq!(v["result"]["content"][0]["text"], "hello");
    }
    Ok(())
}

#[tokio::test]
async fn unified_api_fake_fleet_and_daemon_public_url() {
    e2e(
        "unified-sandbox-api",
        "hermetic",
        "fake Fleet command/services/URLs + daemon public URL",
        5 * MIN,
        None,
        || async {
            let fx = Fixtures::start()?;
            let c = embedded_fleet(Some((&fx.fleet_base_url, &fx.fleet_token)));
            let sb = c
                .sandboxes()
                .create(SandboxCreateOptions {
                    image: "registry.example/mcp:docker-e2e".into(),
                    command: Some(vec!["python".into(), "/srv.py".into()]),
                    services: HashMap::from([("mcp".to_string(), 8765u16)]),
                    runtime: Some("gvisor".into()),
                    cloud: Some(CloudOptions {
                        max_pool_size: Some(2),
                        ..Default::default()
                    }),
                    ready_timeout_ms: Some(120_000),
                    ..SandboxCreateOptions::new("cloud", "")
                })
                .await?;
            let pool = sb
                .info()
                .provider_details
                .get("pool")
                .cloned()
                .unwrap_or_default();
            let res = async {
                let info = sb.info();
                assert_eq!(
                    (info.location.as_str(), info.phase),
                    ("cloud", SandboxPhase::Ready)
                );
                assert!(pool.starts_with("cua-auto-"), "{pool}");
                let url = sb.service("mcp".into())?.url().await?;
                assert!(url.starts_with("https://signed.fleet.test/"), "{url}");
                let p = sb.public_url("mcp".into(), Some(600), None).await?;
                assert!(p.url.starts_with("https://signed.fleet.test/"));
                assert!(p.provider_details.contains_key("claim"));
                let r = sb
                    .service("mcp".into())?
                    .request(
                        "POST".into(),
                        "/mcp".into(),
                        Some(b"{}".to_vec()),
                        Some(30_000),
                        Some(vec![hdr("accept", ACCEPT), hdr("mcp-session-id", "s")]),
                    )
                    .await?;
                assert_eq!(r.status, 200);
                let fwd = sb.forward(8765).await?;
                let (status, _, body) = http("GET", &format!("{}/x", fwd.url().unwrap()), &[], "")?;
                fwd.close().await?;
                assert_eq!(status, 200);
                assert!(body.contains("-mcp/x"), "{body}");
                Res::Ok(())
            }
            .await;
            sb.delete().await?;
            if !pool.is_empty() {
                c.fleet()?.pools().gc_pools(vec![pool], Some(0)).await?;
            }
            res?;

            // Local public URL of a direct spacesd, served by the daemon.
            let d = Daemon::start()?;
            let direct = d
                .client()
                .sandboxes()
                .connect_url(fx.env_url.clone(), Some(fx.env_token.clone()), None)
                .await?;
            assert_eq!(
                direct.service("env".into())?.url().await?,
                fx.env_url.trim_end_matches('/')
            );
            let public = direct.public_url("env".into(), Some(120), None).await?;
            assert!(public.url.starts_with("http://127.0.0.1:") && public.url.contains("/s/"));
            let shared = c
                .sandboxes()
                .connect_url(public.url.clone(), Some(fx.env_token.clone()), None)
                .await?;
            let out = shared
                .spacesd(Some(5000))
                .await?
                .sh("echo shared".into(), None)
                .await?;
            assert_eq!(out.stdout, b"shared\n");
            direct.revoke_public_url(public.id.clone()).await?;
            assert_eq!(http("GET", &public.url, &[], "")?.0, 404);
            Ok(())
        },
    )
    .await;
}

#[tokio::test]
async fn unified_api_plain_image_gvisor() {
    e2e(
        "unified-sandbox-api",
        "container",
        "python:3.12-slim + command/services on gVisor through the daemon",
        10 * MIN,
        None,
        || async {
            if docker(&["image", "inspect", "python:3.12-slim"]).is_err() {
                docker(&["pull", "python:3.12-slim"])?;
            }
            let d = Daemon::start()?;
            let sb = d
                .client()
                .sandboxes()
                .create(SandboxCreateOptions {
                    name: Some(name("unified")),
                    cpus: Some(1),
                    memory_mb: Some(512),
                    command: Some(vec!["python".into(), "-c".into(), server()]),
                    env: HashMap::from([("GREETING".to_string(), "hello".to_string())]),
                    services: HashMap::from([("mcp".to_string(), 8765u16)]),
                    wait_for: vec![ReadinessProbe::http("mcp", "/health")],
                    ready_timeout_ms: Some(300_000),
                    ..SandboxCreateOptions::new("local", "container:python:3.12-slim")
                })
                .await?;
            let res = async {
                let info = sb.info();
                assert_eq!(
                    (info.location.as_str(), info.phase),
                    ("local", SandboxPhase::Ready)
                );
                assert_eq!(info.services.get("mcp"), Some(&8765));
                mcp_through_service(&sb, true).await?;
                plain_checks(&sb.service("mcp".into())?.url().await?)?;
                let public = sb
                    .public_url("mcp".into(), Some(600), Some("e2e".into()))
                    .await?;
                plain_checks(&public.url)?;
                let fwd = sb.forward(8765).await?;
                let url = fwd.url().ok_or("no forward url")?;
                assert!(url.starts_with("http://127.0.0.1:"), "{url}");
                plain_checks(&url)?;
                fwd.close().await?;
                sb.revoke_public_url(public.id).await?;
                Res::Ok(())
            }
            .await;
            sb.delete().await?;
            res
        },
    )
    .await;
}
