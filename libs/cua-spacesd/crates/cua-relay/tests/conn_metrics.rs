// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua_relay_connections_total` on `/metrics` (CUA-2231): outcomes of
//! machine connects and forwarded client requests, with fixed labels only.

use std::time::Duration;

use cua_relay::{
    client::JoinConfig,
    server::{Relay, RelayConfig},
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "machine-secret-canary";
const ID: &str = "metrics-machine";

async fn metrics(base: &str) -> String {
    reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

fn series(text: &str, kind: &str, outcome: &str) -> u64 {
    let key = format!("cua_relay_connections_total{{kind=\"{kind}\",outcome=\"{outcome}\"}} ");
    text.lines()
        .find_map(|l| l.strip_prefix(&key))
        .unwrap_or_else(|| panic!("missing {key}\n{text}"))
        .parse()
        .unwrap()
}

async fn start() -> (Relay, String, std::net::SocketAddr) {
    let relay = Relay::new(RelayConfig {
        tokens: vec![TOKEN.into()],
        ..RelayConfig::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = relay.clone();
    tokio::spawn(async move { serving.serve(listener, std::future::pending()).await });
    (relay, format!("http://{addr}"), addr)
}

#[tokio::test]
async fn a_fresh_relay_renders_all_series_at_zero_with_fixed_labels() {
    let (_relay, base, _) = start().await;
    let text = metrics(&base).await;
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("cua_relay_connections_total{"))
        .collect();
    assert_eq!(lines.len(), 7, "{text}");
    assert!(text.contains("# TYPE cua_relay_connections_total counter"));
    for (kind, outcomes) in [
        ("machine", &["ok", "rejected", "error"][..]),
        ("client", &["ok", "rejected", "offline", "error"][..]),
    ] {
        for outcome in outcomes {
            assert_eq!(series(&text, kind, outcome), 0, "{text}");
        }
    }
    // Scraping /metrics itself is not a connection.
    assert_eq!(series(&metrics(&base).await, "client", "ok"), 0);
}

#[tokio::test]
async fn outcomes_are_counted_by_kind() {
    let (relay, base, addr) = start().await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    // Machine rejected: wrong token.
    let bad = tokio_tungstenite::connect_async(
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!(
            "ws://{addr}{}",
            cua_relay::CONNECT_PATH
        ))
        .map(|mut r| {
            r.headers_mut()
                .insert("authorization", "Bearer wrong-token".parse().unwrap());
            r.headers_mut()
                .insert(cua_relay::MACHINE_ID_HEADER, ID.parse().unwrap());
            r
        })
        .unwrap(),
    )
    .await;
    assert!(bad.is_err());

    // Machine rejected: not a WebSocket handshake. Unrouted paths are not counted.
    let r = http
        .get(format!("{base}{}", cua_relay::CONNECT_PATH))
        .header(cua_relay::MACHINE_ID_HEADER, ID)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    assert_eq!(
        http.get(format!("{base}/favicon.ico"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    let text = metrics(&base).await;
    assert_eq!(series(&text, "machine", "rejected"), 2, "{text}");
    assert_eq!(series(&text, "client", "rejected"), 0, "{text}");
    assert_eq!(series(&text, "client", "ok"), 0, "{text}");

    // Machine ok and client ok (the computer's own 404 passes through as ok).
    let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    let local_task = tokio::spawn(async move {
        axum::serve(
            local,
            axum::Router::new().route("/health", axum::routing::get(|| async { "fine" })),
        )
        .await
        .unwrap();
    });
    let config = JoinConfig::new(
        base.replace("http://", "ws://"),
        TOKEN.into(),
        ID.into(),
        local_addr,
    );
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let join = tokio::spawn(async move { cua_relay::client::session(&config, &stopped).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !relay.machine_ids().contains(&ID.to_string()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("machine never came online");
    // Client rejected: no credential. Client offline: nobody is connected as this id.
    let r = http
        .get(format!("{base}/m/{ID}/data"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = http
        .get(format!("{base}/m/other-machine/health"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 502);
    for (path, status) in [("health", 200), ("missing", 404)] {
        let r = http
            .get(format!("{base}/m/{ID}/{path}"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), status);
    }

    let text = metrics(&base).await;
    assert_eq!(series(&text, "machine", "ok"), 1, "{text}");
    assert_eq!(series(&text, "client", "ok"), 2, "{text}");
    assert_eq!(series(&text, "client", "rejected"), 1, "{text}");
    assert_eq!(series(&text, "client", "offline"), 1, "{text}");
    assert_eq!(series(&text, "client", "error"), 0, "{text}");
    for line in text
        .lines()
        .filter(|l| l.starts_with("cua_relay_connections_total{"))
    {
        assert!(!line.contains(ID) && !line.contains("127.0.0.1"), "{line}");
    }
    stop.cancel();
    let _ = join.await;
    local_task.abort();
}
