//! Every cua-spacesd-client helper against the in-process mock spacesd, over native
//! gRPC (HTTP/2) and gRPC-Web (HTTP/1.1).

use bytes::Bytes;
use cua_spacesd_client::{
    Command, ConnectOptions, Error, Keepalive, ProcessEvent, ProcessRef, Replay, RetryPolicy,
    SpacesdClient, StaticBearer, Transport, TransportPreference, UploadOptions, pb,
    testing::{CutProxy, MockAuth, MockGateway, MockServer},
};
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const TOKEN: &str = "secret-token";

fn pref(t: Transport) -> TransportPreference {
    match t {
        Transport::Native => TransportPreference::Native,
        Transport::GrpcWeb => TransportPreference::GrpcWeb,
    }
}

async fn server() -> MockServer {
    MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await
}

async fn connect_to(url: &str, t: Transport) -> SpacesdClient {
    let opts = ConnectOptions::parse(url)
        .unwrap()
        .token(TOKEN)
        .transport(pref(t))
        .retry(RetryPolicy {
            max_attempts: 20,
            initial_backoff: Duration::from_millis(20),
            max_backoff: Duration::from_millis(500),
        });
    let env = SpacesdClient::connect(opts).await.expect("connect");
    assert_eq!(env.transport(), t);
    env
}

fn sha(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Pseudo-random, incompressible test data.
fn data(len: usize, seed: u64) -> Bytes {
    let mut out = vec![0u8; len];
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for chunk in out.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let b = x.to_le_bytes();
        chunk.copy_from_slice(&b[..chunk.len()]);
    }
    out.into()
}

macro_rules! both_transports {
    ($($name:ident),* $(,)?) => {
        mod native {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $name() { super::$name(cua_spacesd_client::Transport::Native).await }
            )*
        }
        mod grpc_web {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $name() { super::$name(cua_spacesd_client::Transport::GrpcWeb).await }
            )*
        }
    };
}

both_transports!(
    capabilities_and_transport,
    run_collects_output_and_exit_codes,
    run_timeout,
    spawn_reattach_by_tag_with_scrollback,
    stdin_roundtrip,
    small_upload_download_and_typed_errors,
    computer_wrappers,
    open_media_returns_ws_url,
    principal_header_is_sent,
    bad_token_is_unauthenticated,
    upload_download_256_mib_sha,
    upload_resumes_after_disconnect,
    download_resumes_after_disconnect,
    long_running_stream_with_keepalive,
    process_stream_resumes_after_disconnect,
    relay_url_prefix,
);

async fn capabilities_and_transport(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let caps = env.capabilities().await.unwrap();
    assert_eq!(caps.protocol_version, 1);
    assert!(env.has_feature("pty").await.unwrap());
    assert!(!env.has_feature("a11y").await.unwrap());
    let health = env.health().await.unwrap();
    assert_eq!(health.status, pb::HealthStatus::Serving as i32);
    let web = srv.state.observed.grpc_web_requests.load(Ordering::Relaxed);
    let native = srv.state.observed.grpc_requests.load(Ordering::Relaxed);
    match t {
        Transport::Native => assert!(native > 0 && web == 0, "native={native} web={web}"),
        Transport::GrpcWeb => assert!(web > 0 && native == 0, "native={native} web={web}"),
    }
}

async fn run_collects_output_and_exit_codes(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let out = env.run("echo hello world").await.unwrap();
    assert!(out.success());
    assert_eq!(out.stdout_str(), "hello world\n");
    let out = env.run(Command::new("fail").arg("3")).await.unwrap();
    assert_eq!(out.status.code, Some(3));
    assert_eq!(out.stderr_str(), "failed\n");
    let out = env.run("no-such-binary").await.unwrap();
    assert!(!out.success());
    assert!(out.status.error.unwrap().contains("not found"));
}

async fn run_timeout(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let out = env
        .run(
            Command::new("sleep")
                .arg("5000")
                .timeout(Duration::from_millis(200)),
        )
        .await
        .unwrap();
    assert!(out.status.timed_out);
}

async fn spawn_reattach_by_tag_with_scrollback(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let mut h = env
        .spawn(Command::new("ticker").args(["20", "10"]).tag("job-1"))
        .await
        .unwrap();
    assert_eq!(h.tag(), Some("job-1"));
    // Read two chunks, then detach (the process keeps running).
    for _ in 0..2 {
        assert!(matches!(
            h.next_event().await.unwrap(),
            Some(ProcessEvent::Stdout { .. })
        ));
    }
    h.detach();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let procs = env.list_processes(true).await.unwrap();
    assert!(procs.iter().any(|p| p.tag == "job-1"));

    // Full scrollback replay.
    let out = env
        .attach(ProcessRef::Tag("job-1".into()), Replay::All)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    let expected: String = (0..20).map(|i| format!("tick {i}\n")).collect();
    assert_eq!(out.stdout_str(), expected);
    assert!(out.success());

    // Tail replay: only the last line.
    let out = env
        .attach("job-1".into(), Replay::LastBytes("tick 19\n".len() as u64))
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(out.stdout_str(), "tick 19\n");
    assert!(matches!(
        env.attach("missing".into(), Replay::All).await.unwrap_err(),
        Error::ProcessNotFound(_)
    ));
}

async fn stdin_roundtrip(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let h = env.spawn(Command::new("cat").stdin(true)).await.unwrap();
    h.write_stdin(Bytes::from_static(b"hello ")).await.unwrap();
    h.write_stdin(Bytes::from_static(b"stdin")).await.unwrap();
    h.close_stdin().await.unwrap();
    let out = h.wait().await.unwrap();
    assert_eq!(out.stdout_str(), "hello stdin");
}

async fn small_upload_download_and_typed_errors(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let body = data(3 * 1024 * 1024 + 17, 1);
    let res = env
        .upload("/tmp/a.bin", body.clone(), UploadOptions::default())
        .await
        .unwrap();
    assert_eq!(res.sha256, sha(&body));
    assert_eq!(res.resumable, t == Transport::GrpcWeb);
    assert_eq!(srv.state.file("/tmp/a.bin").unwrap(), body.to_vec());
    let back = env.download("/tmp/a.bin").await.unwrap();
    assert_eq!(back, body);
    assert_eq!(
        env.stat("/tmp/a.bin").await.unwrap().size,
        body.len() as u64
    );
    assert_eq!(env.list_dir("/tmp", 1).await.unwrap().len(), 1);

    // Upload from a local file.
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("src.bin");
    std::fs::write(&local, &body[..1000]).unwrap();
    env.upload("/tmp/b.bin", local.as_path(), UploadOptions::default())
        .await
        .unwrap();
    let out = dir.path().join("out.bin");
    let r = env
        .download_to_file("/tmp/b.bin", &out, Default::default())
        .await
        .unwrap();
    assert_eq!(r.sha256, sha(&body[..1000]));
    assert_eq!(std::fs::read(&out).unwrap(), body[..1000].to_vec());

    // A failed download leaves an existing local file intact and no temp
    // file behind.
    let kept = dir.path().join("kept.bin");
    std::fs::write(&kept, b"precious").unwrap();
    env.download_to_file("/nope", &kept, Default::default())
        .await
        .unwrap_err();
    assert_eq!(std::fs::read(&kept).unwrap(), b"precious");
    let parts: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
        .collect();
    assert!(parts.is_empty(), "leftover temp files: {parts:?}");

    match env.download("/nope").await.unwrap_err() {
        Error::PathNotFound(d) => assert_eq!(d.metadata.get("path").unwrap(), "/nope"),
        other => panic!("unexpected {other:?}"),
    }
    env.remove("/tmp/b.bin", false).await.unwrap();
    assert!(matches!(
        env.remove("/tmp/b.bin", false).await.unwrap_err(),
        Error::PathNotFound(_)
    ));
}

async fn computer_wrappers(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let shot = env.screenshot(Default::default()).await.unwrap();
    assert_eq!(&shot.image[..4], b"\x89PNG");
    assert_eq!((shot.width, shot.height), (1280, 800));
    env.click(10.0, 20.0).await.unwrap();
    assert_eq!(env.cursor_position().await.unwrap(), (10.0, 20.0));
    env.move_to(5.0, 6.0).await.unwrap();
    env.scroll(0.0, 3.0).await.unwrap();
    env.drag((1.0, 1.0), (50.0, 60.0)).await.unwrap();
    assert_eq!(env.cursor_position().await.unwrap(), (50.0, 60.0));
    env.type_text("hi").await.unwrap();
    env.press("enter").await.unwrap();
    env.hotkey(&["ctrl", "shift", "t"]).await.unwrap();
    let kb = srv.state.observed.keyboard.lock().unwrap().clone();
    assert_eq!(kb.len(), 3);
    assert!(
        kb[1].contains("Named(") && kb[2].matches("Named(").count() == 3,
        "{kb:?}"
    );
    let ptr = srv.state.observed.pointer.lock().unwrap().clone();
    assert!(ptr[2].contains("delta_y: 3.0"), "{}", ptr[2]);
    assert_eq!(env.get_clipboard().await.unwrap(), None);
    env.set_clipboard("copied").await.unwrap();
    assert_eq!(
        env.get_clipboard().await.unwrap().as_deref(),
        Some("copied")
    );
    assert_eq!(env.displays().await.unwrap()[0].id, "primary");
}

async fn open_media_returns_ws_url(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let m = env.open_media(Default::default()).await.unwrap();
    assert_eq!(m.ticket, "ticket-abc");
    assert_eq!(
        m.ws_url,
        format!("ws://{}/media?ticket=ticket-abc", srv.addr)
    );
}

async fn principal_header_is_sent(t: Transport) {
    let srv = server().await;
    let principal = pb::Principal {
        id: "u1".into(),
        display_name: "Ada".into(),
        color: "#ff0000".into(),
        kind: pb::PrincipalKind::Agent as i32,
    };
    let env = SpacesdClient::connect(
        ConnectOptions::parse(&srv.url())
            .unwrap()
            .token(TOKEN)
            .transport(pref(t))
            .principal(principal.clone()),
    )
    .await
    .unwrap();
    env.run("echo x").await.unwrap();
    assert_eq!(
        *srv.state.observed.principal.lock().unwrap(),
        Some(principal)
    );
}

async fn bad_token_is_unauthenticated(t: Transport) {
    let srv = server().await;
    let err = SpacesdClient::connect(
        ConnectOptions::parse(&srv.url())
            .unwrap()
            .token("wrong")
            .transport(pref(t)),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::Unauthenticated(_)), "{err:?}");
}

async fn upload_download_256_mib_sha(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let body = data(256 * 1024 * 1024, 7);
    let expected = sha(&body);
    let started = Instant::now();
    let res = env
        .upload("/big.bin", body.clone(), UploadOptions::default())
        .await
        .unwrap();
    let up = started.elapsed();
    assert_eq!(res.sha256, expected);
    assert_eq!(res.size, body.len() as u64);
    drop(body);
    let started = Instant::now();
    let mut sink = Vec::with_capacity(256 << 20);
    let r = env
        .download_with("/big.bin", Default::default(), &mut sink)
        .await
        .unwrap();
    let down = started.elapsed();
    assert_eq!(r.size, 256 << 20);
    assert_eq!(r.sha256, expected);
    assert_eq!(sha(&sink), expected);
    eprintln!("{t:?}: 256 MiB upload {up:?}, download {down:?}");
}

async fn upload_resumes_after_disconnect(t: Transport) {
    let srv = server().await;
    let proxy = CutProxy::start(srv.addr).await;
    let env = connect_to(&proxy.url(), t).await;
    let body = data(24 * 1024 * 1024, 3);
    proxy.cut_after(9 * 1024 * 1024);
    let res = env
        .upload("/resume.bin", body.clone(), UploadOptions::default())
        .await
        .unwrap();
    assert_eq!(proxy.cuts.load(Ordering::SeqCst), 1);
    assert!(res.resumable, "a broken transfer must continue resumably");
    assert!(res.resumes >= 1);
    assert_eq!(res.sha256, sha(&body));
    assert_eq!(srv.state.file("/resume.bin").unwrap(), body.to_vec());

    // Forced resumable protocol, interrupted twice.
    proxy.cut_after(5 * 1024 * 1024);
    let body2 = data(12 * 1024 * 1024, 4);
    let progress = Arc::new(AtomicU64::new(0));
    let res = env
        .upload(
            "/resume2.bin",
            body2.clone(),
            UploadOptions {
                resumable: Some(true),
                progress: Some(progress.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(res.resumes >= 1);
    assert_eq!(progress.load(Ordering::Relaxed), body2.len() as u64);
    assert_eq!(srv.state.file("/resume2.bin").unwrap(), body2.to_vec());
    // Chunks were not all resent: the server received about one copy.
    let chunks = srv.state.observed.upload_chunks.load(Ordering::Relaxed);
    assert!(chunks < 60, "too many chunks resent: {chunks}");
}

async fn download_resumes_after_disconnect(t: Transport) {
    let srv = server().await;
    let proxy = Arc::new(CutProxy::start(srv.addr).await);
    let env = connect_to(&proxy.url(), t).await;
    let body = data(32 * 1024 * 1024, 5);
    srv.state.put_file("/dl.bin", body.to_vec());
    let progress = Arc::new(AtomicU64::new(0));
    let watcher = {
        let (p, proxy) = (progress.clone(), proxy.clone());
        tokio::spawn(async move {
            while p.load(Ordering::Relaxed) < 12 * 1024 * 1024 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            proxy.cut_now();
        })
    };
    let mut sink = Vec::new();
    let r = env
        .download_with(
            "/dl.bin",
            cua_spacesd_client::DownloadOptions {
                progress: Some(progress),
                chunk_size: 256 * 1024,
                ..Default::default()
            },
            &mut sink,
        )
        .await
        .unwrap();
    watcher.await.unwrap();
    assert_eq!(proxy.cuts.load(Ordering::SeqCst), 1);
    assert!(r.resumes >= 1, "expected a resume");
    assert_eq!(r.sha256, sha(&body));
    assert_eq!(sink, body.to_vec());
}

async fn long_running_stream_with_keepalive(t: Transport) {
    let srv = server().await;
    let env = connect_to(&srv.url(), t).await;
    let mut h = env
        .spawn(
            Command::new("ticker")
                .args(["6", "500"])
                .keepalive(Duration::from_millis(100)),
        )
        .await
        .unwrap();
    let mut ticks = 0;
    while let Some(ev) = h.next_event().await.unwrap() {
        match ev {
            ProcessEvent::Stdout { .. } => ticks += 1,
            ProcessEvent::Exit(s) => assert!(s.success()),
            _ => {}
        }
    }
    assert_eq!(ticks, 6);
    assert!(
        h.keepalives_seen() >= 10,
        "keepalives: {}",
        h.keepalives_seen()
    );
    assert_eq!(h.reconnects(), 0);
}

async fn process_stream_resumes_after_disconnect(t: Transport) {
    let srv = server().await;
    let proxy = CutProxy::start(srv.addr).await;
    let env = connect_to(&proxy.url(), t).await;
    let mut h = env
        .spawn(
            Command::new("ticker")
                .args(["30", "20"])
                .tag("resume-job")
                .keepalive(Duration::from_millis(200)),
        )
        .await
        .unwrap();
    let mut out = String::new();
    let mut n = 0;
    while let Some(ev) = h.next_event().await.unwrap() {
        if let ProcessEvent::Stdout { data, .. } = ev {
            out.push_str(std::str::from_utf8(&data).unwrap());
            n += 1;
            if n == 5 {
                proxy.cut_now();
            }
        }
    }
    let expected: String = (0..30).map(|i| format!("tick {i}\n")).collect();
    assert_eq!(out, expected, "output must be complete and not duplicated");
    assert!(h.reconnects() >= 1);
}

async fn relay_url_prefix(t: Transport) {
    let srv = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        prefix: Some("/m/machine-1".into()),
        ..Default::default()
    })
    .await;
    let url = format!("{}/m/machine-1", srv.url());
    let env = connect_to(&url, t).await;
    assert!(matches!(
        env.endpoint().kind(),
        cua_spacesd_client::EndpointKind::Relay { .. }
    ));
    assert_eq!(env.run("echo relay").await.unwrap().stdout_str(), "relay\n");
    let m = env.open_media(Default::default()).await.unwrap();
    assert!(m.ws_url.contains("/m/machine-1/media?ticket="));
}

// ---------------------------------------------------------------- one-offs

#[tokio::test]
async fn auto_prefers_native_for_direct() {
    let srv = server().await;
    let env = SpacesdClient::connect(ConnectOptions::parse(&srv.url()).unwrap().token(TOKEN))
        .await
        .unwrap();
    assert_eq!(env.transport(), Transport::Native);
}

#[tokio::test]
async fn fleet_gateway_uses_grpc_web_bearer_and_claim() {
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
    let endpoint =
        cua_spacesd_client::Endpoint::fleet_service(&srv.url(), "cua-e2e-pool", "sbx-1", "env")
            .unwrap();
    let opts = ConnectOptions::new(endpoint.clone())
        .token(TOKEN)
        .fleet_gateway(
            Arc::new(StaticBearer("fleet-bearer".into())),
            Some("claim-abc".into()),
        );
    let env = SpacesdClient::connect(opts).await.unwrap();
    assert_eq!(
        env.transport(),
        Transport::GrpcWeb,
        "gateway must auto-select gRPC-Web"
    );
    assert_eq!(
        env.run("echo via gateway").await.unwrap().stdout_str(),
        "via gateway\n"
    );
    env.upload("/gw.bin", data(2_500_000, 9), UploadOptions::default())
        .await
        .unwrap();
    assert!(srv.state.observed.upload_chunks.load(Ordering::Relaxed) >= 3);
    assert_eq!(
        srv.state
            .observed
            .write_file_streams
            .load(Ordering::Relaxed),
        0
    );

    // A token-provider closure also works, and a wrong claim is refused.
    let refreshes = Arc::new(AtomicU64::new(0));
    let counter = refreshes.clone();
    let provider = move |_force: bool| {
        counter.fetch_add(1, Ordering::Relaxed);
        async { Ok::<_, cua_spacesd_client::error::BoxError>("fleet-bearer".to_string()) }
    };
    let err = SpacesdClient::connect(
        ConnectOptions::new(endpoint)
            .token(TOKEN)
            .fleet_gateway(Arc::new(provider), Some("other-claim".into())),
    )
    .await
    .unwrap_err();
    assert!(refreshes.load(Ordering::Relaxed) >= 1);
    assert!(
        matches!(
            err,
            Error::SpacesdNotAvailable { .. } | Error::PermissionDenied(_)
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn not_an_spacesd() {
    let srv = server().await;
    srv.state.refuse_capabilities.store(true, Ordering::Relaxed);
    let err = SpacesdClient::connect(ConnectOptions::parse(&srv.url()).unwrap().token(TOKEN))
        .await
        .unwrap_err();
    match err {
        Error::SpacesdNotAvailable { reason, .. } => {
            assert!(
                reason.contains("gRPC: ") && reason.contains("gRPC-Web: "),
                "{reason}"
            );
            // The server answered: not reported as "still starting".
            assert!(!reason.contains("may still be starting"), "{reason}");
        }
        other => panic!("unexpected {other:?}"),
    }
    // Nothing listening at all.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let err = SpacesdClient::connect(
        ConnectOptions::parse(&format!("127.0.0.1:{port}"))
            .unwrap()
            .probe_timeout(Duration::from_secs(3)),
    )
    .await
    .unwrap_err();
    match err {
        // One readable line: what happened and what to do, never the
        // Debug dump of the transport error chain.
        Error::SpacesdNotAvailable { reason, .. } => {
            assert!(reason.contains("may still be starting"), "{reason}");
            assert!(!reason.contains("ChannelError"), "{reason}");
            assert!(!reason.contains("source:"), "{reason}");
            assert!(!reason.contains('\n'), "{reason}");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn keepalive_config_is_accepted() {
    let srv = server().await;
    let env = SpacesdClient::connect(
        ConnectOptions::parse(&srv.url())
            .unwrap()
            .token(TOKEN)
            .keepalive(Keepalive {
                interval: Duration::from_millis(200),
                timeout: Duration::from_millis(500),
            }),
    )
    .await
    .unwrap();
    // Idle across several PING intervals; the connection must stay usable.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(env.run("echo alive").await.unwrap().success());
}

#[tokio::test]
async fn pointer_input_that_reaches_nothing_fails_loudly() {
    let srv = server().await;
    let env = connect_to(&srv.url(), Transport::Native).await;
    env.click(10.0, 10.0).await.unwrap();
    srv.state.pointer_not_moved.store(true, Ordering::Relaxed);
    let err = env.click(10.0, 10.0).await.unwrap_err();
    assert!(
        matches!(&err, Error::DeliveryFailed(d) if d.message.contains("not delivered")),
        "{err:?}"
    );
    // A move to no point (the current position) is not judged.
    env.scroll(0.0, 1.0).await.unwrap();
}

#[tokio::test]
async fn desktop_readiness_follows_the_health_component() {
    let srv = server().await;
    let env = connect_to(&srv.url(), Transport::Native).await;
    // An older spacesd without the component: judged by its displays.
    let r = env.desktop_readiness().await.unwrap();
    assert!(r.ready && !r.reported, "{r:?}");
    *srv.state.desktop_health.lock().unwrap() = Some(pb::ComponentHealth {
        name: cua_spacesd_client::DESKTOP_COMPONENT.into(),
        status: pb::HealthStatus::NotServing as i32,
        detail: "no window manager is running yet".into(),
    });
    let err = env
        .wait_desktop_ready(Duration::from_millis(600))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::DesktopNotReady(m) if m.contains("no window manager")),
        "{err:?}"
    );
    // Becomes ready while waiting.
    let state = srv.state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Some(c) = state.desktop_health.lock().unwrap().as_mut() {
            c.status = pb::HealthStatus::Serving as i32;
            c.detail.clear();
        }
    });
    let r = env
        .wait_desktop_ready(Duration::from_secs(10))
        .await
        .unwrap();
    assert!(r.ready && r.reported, "{r:?}");
}
