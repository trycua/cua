// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Reverse tunnel end to end, in process: spacesd ⇄ `join` client ⇄
//! cua-relay ⇄ SDK client at `http://relay/m/<machine-id>`. Covers native
//! gRPC and gRPC-Web through the relay, ≥32 parallel streams, WebSocket
//! tunnels and signed URLs through the relay, auth refusals, and killing the
//! relay mid-stream: the machine reconnects and `ConnectProcess` by tag
//! replays scrollback.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use cua_proto::env::v1::process_data::Output;
use cua_proto::env::v1::process_event::Event;
use cua_proto::env::v1::process_selector::Selector;
use cua_proto::env::v1::*;
use cua_relay::server::{Relay, RelayConfig};
use cua_spacesd_client::TransportPreference;
use tokio_util::sync::CancellationToken;

use common::*;

const RELAY_TOKEN: &str = "relay-registration-token";

async fn start_relay(addr: Option<SocketAddr>) -> (Relay, SocketAddr, tokio::task::JoinHandle<()>) {
    let relay = Relay::new(RelayConfig {
        tokens: vec![RELAY_TOKEN.into()],
        admin_token: Some("admin".into()),
        ..RelayConfig::default()
    });
    let listener = match addr {
        Some(addr) => {
            // The old listener may still be closing; retry briefly.
            let mut bound = None;
            for _ in 0..50 {
                if let Ok(l) = tokio::net::TcpListener::bind(addr).await {
                    bound = Some(l);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            bound.expect("rebind relay port")
        }
        None => tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
    };
    let addr = listener.local_addr().unwrap();
    let serving = relay.clone();
    let task = tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    (relay, addr, task)
}

async fn wait_for_machine(relay: &Relay, id: &str) {
    for _ in 0..200 {
        if relay.machine_ids().iter().any(|m| m == id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("machine {id} never registered");
}

struct Setup {
    target: Target,
    relay: Relay,
    relay_addr: SocketAddr,
    relay_task: tokio::task::JoinHandle<()>,
    machine_id: String,
    shutdown: CancellationToken,
}

async fn setup() -> Setup {
    let driver = target().await;
    assert!(driver.local.is_some(), "the relay test runs in-process");
    let local: SocketAddr = driver.url.trim_start_matches("http://").parse().unwrap();
    let (relay, relay_addr, relay_task) = start_relay(None).await;
    let machine_id = uuid::Uuid::new_v4().simple().to_string();
    let mut config = cua_relay::client::JoinConfig::new(
        format!("ws://{relay_addr}"),
        RELAY_TOKEN.into(),
        machine_id.clone(),
        local,
    );
    config.heartbeat = Duration::from_secs(2);
    config.max_backoff = Duration::from_secs(2);
    let shutdown = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(config, shutdown.clone()));
    wait_for_machine(&relay, &machine_id).await;
    let target = Target {
        url: format!("http://{relay_addr}/m/{machine_id}"),
        token: driver.token.clone(),
        scratch: driver.scratch.clone(),
        local: driver.local,
    };
    Setup {
        target,
        relay,
        relay_addr,
        relay_task,
        machine_id,
        shutdown,
    }
}

#[tokio::test]
async fn relay_refuses_bad_machine_tokens_and_anonymous_clients() {
    let (relay, addr, _task) = start_relay(None).await;
    let mut config = cua_relay::client::JoinConfig::new(
        format!("ws://{addr}"),
        "wrong".into(),
        "machine-0001".into(),
        "127.0.0.1:1".parse().unwrap(),
    );
    config.max_backoff = Duration::from_millis(100);
    let end = cua_relay::client::session(&config, &CancellationToken::new()).await;
    assert!(
        matches!(end, cua_relay::client::SessionEnd::Refused(ref r) if r.contains("401")),
        "{end:?}"
    );
    assert!(relay.machine_ids().is_empty());

    let s = setup().await;
    let http = http_client();
    // No env credential: the relay does not forward. gRPC callers get a
    // real gRPC status, plain HTTP callers a 401.
    for transport in TRANSPORTS {
        let anonymous = s.target.client_with_token(transport, None).await;
        let error = anonymous
            .system()
            .health(HealthRequest {})
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unauthenticated, "{transport:?}");
        let info = cua_spacesd_server::error::error_info(&error).expect("relay sends ErrorInfo");
        assert_eq!(info.reason, ErrorReason::Unauthenticated as i32);
    }
    let anonymous = http
        .get(
            format!(
                "http://{}/m/{}/files?path=/etc/passwd",
                s.relay_addr, s.machine_id
            )
            .parse()
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), http::StatusCode::UNAUTHORIZED);
    // A wrong env token reaches the driver and is refused end to end.
    let wrong = s
        .target
        .client_with_token(TransportPreference::GrpcWeb, Some("nope".into()))
        .await;
    assert_eq!(
        wrong
            .system()
            .health(HealthRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    // Unknown machine.
    let unknown = http
        .get(
            format!("http://{}/m/0000000000000000/health", s.relay_addr)
                .parse()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), http::StatusCode::BAD_GATEWAY);
    // /health through the relay needs no credential.
    let health = http
        .get(
            format!("http://{}/m/{}/health", s.relay_addr, s.machine_id)
                .parse()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), http::StatusCode::NO_CONTENT);
    // Admin status reports byte counters.
    let status = http
        .request(
            http::Request::get(format!("http://{}/relay/v1/machines", s.relay_addr))
                .header("authorization", "Bearer admin")
                .body(full(bytes::Bytes::new()))
                .unwrap(),
        )
        .await
        .unwrap();
    let rows: Vec<cua_relay::server::MachineStatus> =
        serde_json::from_slice(&body_bytes(status, 1 << 20).await).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].bytes_from_machine > 0);
    s.shutdown.cancel();
}

#[tokio::test]
async fn grpc_web_files_and_tunnels_through_the_relay() {
    let s = setup().await;
    let t = &s.target;
    for transport in TRANSPORTS {
        let c = t.client(transport).await;
        let caps = c
            .system()
            .get_capabilities(GetCapabilitiesRequest {})
            .await
            .unwrap()
            .into_inner();
        assert_eq!(caps.protocol_version, cua_proto::ENV_PROTOCOL_VERSION);
        let (code, out) = run_sh(&c, "echo via-relay").await;
        assert_eq!((code, out.as_slice()), (Some(0), &b"via-relay\n"[..]));
    }
    // ≥32 concurrent streams share the one tunnel.
    let mut clients = Vec::new();
    for i in 0..40u32 {
        let transport = if i % 2 == 0 {
            TransportPreference::Native
        } else {
            TransportPreference::GrpcWeb
        };
        clients.push((i, t.client(transport).await));
    }
    let runs = clients.iter().map(|(i, c)| async move {
        let (code, out) = run_sh(c, &format!("yes relay-{i} | head -c 262144")).await;
        assert_eq!(code, Some(0));
        assert_eq!(out.len(), 262_144, "stream {i}");
    });
    tokio::time::timeout(
        Duration::from_secs(120),
        futures_util::future::join_all(runs),
    )
    .await
    .expect("parallel streams through the relay");

    // Signed URL PUT + GET through the relay path prefix.
    let c = t.client(TransportPreference::GrpcWeb).await;
    let path = t.path("relay.bin");
    let mut data = vec![0u8; 3 * 1024 * 1024 + 5];
    Pattern::new(11).fill(&mut data);
    let put = c
        .filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path: path.clone(),
            method: SignedUrlMethod::Put as i32,
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 60,
                nanos: 0,
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let http = http_client();
    let response = http
        .request(
            http::Request::put(t.endpoint().http_url(&put.url_path))
                .body(full(data.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::CREATED);
    let get = c
        .filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path,
            method: SignedUrlMethod::Get as i32,
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 60,
                nanos: 0,
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let response = http
        .get(t.endpoint().http_url(&get.url_path).parse().unwrap())
        .await
        .unwrap();
    assert_eq!(body_bytes(response, 8 << 20).await, data);

    // WebSocket tunnel through the relay to a guest TCP echo (python).
    let port: u16 = {
        let (_, out) = run_sh(
            &c,
            "python3 -c 'import socket; s=socket.socket(); s.bind((\"127.0.0.1\",0)); print(s.getsockname()[1])'",
        )
        .await;
        String::from_utf8(out).unwrap().trim().parse().unwrap()
    };
    let script = format!(
        "import socket\ns=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('127.0.0.1',{port})); s.listen(1)\nc,_=s.accept()\nwhile True:\n  d=c.recv(65536)\n  if not d: break\n  c.sendall(d)\n"
    );
    let _echo = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(ProcessConfig {
                command: "python3".into(),
                args: vec!["-c".into(), script],
                ..Default::default()
            }),
            kill_on_disconnect: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let forward = c
        .tunnel()
        .forward(ForwardRequest {
            port: port as u32,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    use futures_util::{SinkExt, StreamExt};
    let mut echoed = Vec::new();
    for _ in 0..50 {
        let Ok((mut ws, _)) =
            tokio_tungstenite::connect_async(t.endpoint().ws_url(&forward.ws_path)).await
        else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        ws.send(tokio_tungstenite::tungstenite::Message::Binary(
            b"ping-through-relay".to_vec(),
        ))
        .await
        .unwrap();
        if let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b)))) =
            tokio::time::timeout(Duration::from_secs(5), ws.next()).await
        {
            echoed = b;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(echoed, b"ping-through-relay");
    s.shutdown.cancel();
}

#[tokio::test]
async fn killing_the_relay_mid_stream_then_reattaching_replays_scrollback() {
    let s = setup().await;
    let name = format!("relay-kill-{}", uuid::Uuid::new_v4().simple());
    let c = s.target.client(TransportPreference::GrpcWeb).await;
    let mut stream = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(ProcessConfig {
                command: SH.into(),
                args: vec![
                    "-c".into(),
                    "i=0; while [ $i -lt 12 ]; do echo line $i; i=$((i+1)); sleep 0.5; done".into(),
                ],
                ..Default::default()
            }),
            tag: name.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    // Read a couple of lines, then kill the relay under the stream.
    let mut seen = String::new();
    for _ in 0..100 {
        let m = tokio::time::timeout(Duration::from_secs(10), stream.message())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Some(Event::Data(d)) = m.event.and_then(|e| e.event) {
            if let Some(Output::Stdout(b)) = d.output {
                seen.push_str(&String::from_utf8_lossy(&b));
            }
        }
        if seen.contains("line 1\n") {
            break;
        }
    }
    s.relay_task.abort();
    s.relay.disconnect_all();
    let broken = tokio::time::timeout(Duration::from_secs(20), async {
        for _ in 0..1000 {
            match stream.message().await {
                Ok(Some(_)) => continue,
                _ => return true,
            }
        }
        false
    })
    .await
    .unwrap_or(true);
    assert!(broken, "client stream ends when the relay dies");

    // Relay comes back on the same address; the machine rejoins by itself.
    let (relay, _addr, _task) = start_relay(Some(s.relay_addr)).await;
    wait_for_machine(&relay, &s.machine_id).await;
    let c = s.target.client(TransportPreference::Native).await;
    let mut stream = c
        .process()
        .connect_process(ConnectProcessRequest {
            process: Some(ProcessSelector {
                selector: Some(Selector::Tag(name)),
            }),
            replay_from_offset: Some(0),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut all = String::new();
    let mut exit = None;
    for _ in 0..10_000 {
        let Some(m) = tokio::time::timeout(Duration::from_secs(30), stream.message())
            .await
            .unwrap()
            .unwrap()
        else {
            break;
        };
        match m.event.and_then(|e| e.event) {
            Some(Event::Data(d)) => {
                if let Some(Output::Stdout(b)) = d.output {
                    all.push_str(&String::from_utf8_lossy(&b));
                }
            }
            Some(Event::End(e)) => {
                exit = e.exit_code;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(exit, Some(0));
    for i in 0..12 {
        assert!(
            all.contains(&format!("line {i}\n")),
            "line {i} missing after reattach: {all:?}"
        );
    }
    s.shutdown.cancel();
}
