// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Client TCP forwarding (`cua_spacesd_client::SpacesdClient::forward_tcp` and the SDK's
//! `Sandbox.forward` for url: sandboxes) against a real cua-spacesd
//! server core in-process: its `TunnelService.Forward` and ticketed
//! `/tunnel` WebSocket. The forward target is a loopback echo server in this
//! test; nothing else on the host is reached. Every read is bounded.

use cua_spaces_e2e::{TOKEN, driver};
use cua_spacesd_client::{ForwardOptions, SpacesdClient, TUNNEL_FORWARD_FEATURE, pb};
use std::{net::SocketAddr, sync::atomic::Ordering, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const STEP: Duration = Duration::from_secs(15);

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
        .map(|i| (i as u8).wrapping_mul(13).wrapping_add(seed))
        .collect()
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn env_client_forwards_through_the_real_driver() {
    let d = driver().await;
    let env = SpacesdClient::connect_url(&d.url, Some(TOKEN.into()))
        .await
        .unwrap();
    d.confine_env(&env).await;
    assert!(env.has_feature(TUNNEL_FORWARD_FEATURE).await.unwrap());
    let echo = echo_server().await;
    let fwd = env
        .forward_tcp(ForwardOptions::new(echo.port()))
        .await
        .unwrap();

    let big = payload(2_000_000, 1);
    assert_eq!(round_trip(fwd.local_addr(), big.clone()).await, big);

    let addr = fwd.local_addr();
    let tasks: Vec<_> = (0..8u8)
        .map(|i| {
            tokio::spawn(async move {
                let data = payload(200_000 + i as usize, i);
                (round_trip(addr, data.clone()).await, data)
            })
        })
        .collect();
    for t in tasks {
        let (got, want) = t.await.unwrap();
        assert_eq!(got, want);
    }
    assert_eq!(fwd.stats().connections.load(Ordering::Relaxed), 9);
    assert_eq!(fwd.stats().failed.load(Ordering::Relaxed), 0);

    // The driver reports the forward, then forgets it after close().
    let id = fwd.forward_id().await;
    let listed = env
        .tunnel()
        .list_forwards(pb::ListForwardsRequest {})
        .await
        .unwrap()
        .into_inner()
        .forwards;
    let info = listed.iter().find(|f| f.forward_id == id).expect("listed");
    assert_eq!(info.port, echo.port() as u32);
    assert!(info.bytes_in >= big.len() as u64, "{info:?}");
    fwd.close().await.unwrap();
    let listed = env
        .tunnel()
        .list_forwards(pb::ListForwardsRequest {})
        .await
        .unwrap()
        .into_inner()
        .forwards;
    assert!(listed.iter().all(|f| f.forward_id != id), "{listed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_forward_is_renewed_by_the_client() {
    let d = driver().await;
    let env = SpacesdClient::connect_url(&d.url, Some(TOKEN.into()))
        .await
        .unwrap();
    d.confine_env(&env).await;
    let echo = echo_server().await;
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
    // The driver answers 410 for the revoked forward; the client mints a
    // new one and the connection goes through.
    assert_eq!(
        round_trip(fwd.local_addr(), b"renewed".to_vec()).await,
        b"renewed"
    );
    assert_ne!(fwd.forward_id().await, first);
    fwd.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_url_sandbox_forward_uses_the_tunnel() {
    let d = driver().await;
    d.confine_direct().await;
    let dirs = tempfile::tempdir().unwrap();
    let runtime = cua_daemon::Runtime::new(cua_daemon::RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        env_probe_timeout: Some(Duration::from_secs(5)),
        // The Cua Spaces build of the daemon: teleport, the Cua
        // Drive, persistent agents and the Keyvault.
        extensions: vec![std::sync::Arc::new(
            cua_spaces_ext::daemon::CuaSpacesDaemon::default(),
        )],
        ..Default::default()
    })
    .unwrap();
    let cua = cua_sdk::Cua::from_runtime(runtime);
    let sb = cua
        .sandboxes()
        .connect_url(d.url.clone(), Some(TOKEN.into()), None)
        .await
        .unwrap();
    let echo = echo_server().await;
    let fwd = sb.forward(echo.port()).await.unwrap();
    let local: SocketAddr = fwd
        .local_addr()
        .expect("a loopback forward")
        .parse()
        .unwrap();
    assert_eq!(fwd.url(), Some(format!("http://{local}")));
    let data = payload(500_000, 9);
    assert_eq!(round_trip(local, data.clone()).await, data);
    fwd.close().await.unwrap();
    // The listener is gone once closed.
    let gone = match TcpStream::connect(local).await {
        Err(_) => true,
        Ok(mut s) => {
            let mut b = [0u8; 1];
            matches!(
                tokio::time::timeout(STEP, s.read(&mut b)).await,
                Ok(Ok(0)) | Ok(Err(_))
            )
        }
    };
    assert!(gone);
}
