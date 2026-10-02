//! `SpacesdClient::forward_tcp` against the mock driver's `TunnelService` and
//! `/tunnel` WebSocket, directly and through the emulated Fleet gateway.
//! Every read is bounded by a timeout and an exact byte count.

use cua_spacesd_client::{
    ConnectOptions, Error, ForwardOptions, SpacesdClient, StaticBearer, TUNNEL_FORWARD_FEATURE, pb,
    testing::{MockAuth, MockGateway, MockServer},
};
use std::{net::SocketAddr, sync::Arc, sync::atomic::Ordering, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const TOKEN: &str = "tunnel-token";
const STEP: Duration = Duration::from_secs(10);

/// A loopback echo server (each connection echoes until EOF).
async fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
    });
    addr
}

fn payload(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// Sends `data` through `addr` and reads exactly as many bytes back.
async fn round_trip(addr: SocketAddr, data: Vec<u8>) -> Vec<u8> {
    let stream = tokio::time::timeout(STEP, TcpStream::connect(addr))
        .await
        .expect("connect timed out")
        .unwrap();
    let (mut r, mut w) = stream.into_split();
    let len = data.len();
    let writer = tokio::spawn(async move {
        w.write_all(&data).await.unwrap();
        w
    });
    let mut got = vec![0u8; len];
    tokio::time::timeout(STEP, r.read_exact(&mut got))
        .await
        .expect("echo timed out")
        .unwrap();
    drop(writer.await.unwrap());
    got
}

async fn direct() -> (MockServer, SpacesdClient) {
    let srv = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    srv.state.advertise(&[TUNNEL_FORWARD_FEATURE]);
    let env = SpacesdClient::connect_url(&srv.url(), Some(TOKEN.into()))
        .await
        .unwrap();
    (srv, env)
}

#[tokio::test]
async fn forwards_bytes_and_concurrent_connections() {
    let echo = echo_server().await;
    let (srv, env) = direct().await;
    assert!(env.supports_tunnel_forward().await.unwrap());
    let fwd = env
        .forward_tcp(ForwardOptions::new(echo.port()))
        .await
        .unwrap();
    assert!(fwd.local_addr().ip().is_loopback());
    assert_eq!(fwd.guest_port(), echo.port());

    // One large round trip (bigger than a relay chunk).
    let big = payload(1_000_000, 7);
    assert_eq!(round_trip(fwd.local_addr(), big.clone()).await, big);

    // Several at once, each with its own bytes.
    let addr = fwd.local_addr();
    let tasks: Vec<_> = (0..8u8)
        .map(|i| {
            tokio::spawn(async move {
                let data = payload(100_000 + i as usize * 1000, i);
                (round_trip(addr, data.clone()).await, data)
            })
        })
        .collect();
    for t in tasks {
        let (got, want) = t.await.unwrap();
        assert_eq!(got, want);
    }
    let stats = fwd.stats();
    assert_eq!(stats.connections.load(Ordering::Relaxed), 9);
    assert_eq!(stats.failed.load(Ordering::Relaxed), 0);
    assert!(stats.bytes_from_guest.load(Ordering::Relaxed) >= big.len() as u64);
    // One ticket served every connection.
    let attaches = srv.state.tunnel_attaches();
    assert_eq!(attaches.len(), 9);
    assert!(attaches.iter().all(|a| a.accepted));
    assert!(
        attaches
            .iter()
            .all(|a| a.path.starts_with("/tunnel?ticket="))
    );
    assert_eq!(srv.state.open_forwards().len(), 1);

    // Clean shutdown: the listener goes away and the forward is revoked.
    let id = fwd.forward_id().await;
    fwd.close().await.unwrap();
    assert!(srv.state.open_forwards().is_empty(), "{id} still open");
    let refused = match TcpStream::connect(addr).await {
        Err(_) => true,
        Ok(mut s) => {
            let mut b = [0u8; 1];
            matches!(
                tokio::time::timeout(STEP, s.read(&mut b)).await,
                Ok(Ok(0)) | Ok(Err(_))
            )
        }
    };
    assert!(refused, "closed forward still relays");
}

#[tokio::test]
async fn open_connections_end_when_the_forward_closes() {
    let echo = echo_server().await;
    let (_srv, env) = direct().await;
    let fwd = env
        .forward_tcp(ForwardOptions::new(echo.port()))
        .await
        .unwrap();
    let mut s = TcpStream::connect(fwd.local_addr()).await.unwrap();
    s.write_all(b"ping").await.unwrap();
    let mut got = [0u8; 4];
    tokio::time::timeout(STEP, s.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&got, b"ping");
    fwd.close().await.unwrap();
    let mut rest = [0u8; 16];
    let n = tokio::time::timeout(STEP, s.read(&mut rest))
        .await
        .expect("relayed connection outlived close()");
    assert!(matches!(n, Ok(0) | Err(_)), "{n:?}");
}

#[tokio::test]
async fn revoked_ticket_is_renewed() {
    let echo = echo_server().await;
    let (srv, env) = direct().await;
    let fwd = env
        .forward_tcp(ForwardOptions::new(echo.port()))
        .await
        .unwrap();
    let first = fwd.forward_id().await;
    env.tunnel()
        .close_forward(pb::CloseForwardRequest {
            forward_id: first.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        round_trip(fwd.local_addr(), b"again".to_vec()).await,
        b"again"
    );
    let second = fwd.forward_id().await;
    assert_ne!(first, second);
    let attaches = srv.state.tunnel_attaches();
    assert_eq!(
        attaches.iter().map(|a| a.accepted).collect::<Vec<_>>(),
        [false, true]
    );
    fwd.close().await.unwrap();
    assert!(srv.state.open_forwards().is_empty());
}

#[tokio::test]
async fn through_the_fleet_gateway() {
    let echo = echo_server().await;
    let prefix = "/api/svc/cua-e2e-pool/sbx-1-env";
    let srv = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        gateway: Some(MockGateway {
            prefix: prefix.into(),
            bearer: "fleet-bearer".into(),
            claim: "claim-abc".into(),
        }),
        prefix: None,
    })
    .await;
    srv.state.advertise(&[TUNNEL_FORWARD_FEATURE]);
    let endpoint =
        cua_spacesd_client::Endpoint::fleet_service(&srv.url(), "cua-e2e-pool", "sbx-1", "env")
            .unwrap();
    let env = SpacesdClient::connect(ConnectOptions::new(endpoint).token(TOKEN).fleet_gateway(
        Arc::new(StaticBearer("fleet-bearer".into())),
        Some("claim-abc".into()),
    ))
    .await
    .unwrap();
    let fwd = env
        .forward_tcp(ForwardOptions::new(echo.port()))
        .await
        .unwrap();
    let data = payload(300_000, 3);
    assert_eq!(round_trip(fwd.local_addr(), data.clone()).await, data);
    let attaches = srv.state.tunnel_attaches();
    assert_eq!(attaches.len(), 1);
    let a = &attaches[0];
    assert!(a.accepted, "{a:?}");
    assert!(
        a.path.starts_with(&format!("{prefix}/tunnel?ticket=")),
        "{a:?}"
    );
    assert_eq!(a.authorization.as_deref(), Some("Bearer fleet-bearer"));
    assert_eq!(a.claim.as_deref(), Some("claim-abc"));
    assert_eq!(
        a.env_authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    fwd.close().await.unwrap();
}

#[tokio::test]
async fn unsupported_without_the_capability() {
    let srv = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    let env = SpacesdClient::connect_url(&srv.url(), Some(TOKEN.into()))
        .await
        .unwrap();
    assert!(!env.supports_tunnel_forward().await.unwrap());
    let err = env.forward_tcp(ForwardOptions::new(80)).await.unwrap_err();
    match err {
        Error::FeatureUnsupported { feature, details } => {
            assert_eq!(feature, TUNNEL_FORWARD_FEATURE);
            assert!(details.message.contains("tunnel.forward"), "{details}");
        }
        other => panic!("{other:?}"),
    }
    assert!(srv.state.open_forwards().is_empty());
    assert!(srv.state.tunnel_attaches().is_empty());
}
