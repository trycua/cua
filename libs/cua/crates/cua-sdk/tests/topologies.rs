#![allow(deprecated)] // also exercises the deprecated `apply_pool` wrapper
//! One suite, three topologies: embedded, daemon over the Unix socket, and
//! daemon over loopback + token. Everything runs against in-process fakes
//! (`MockServer` spacesd with a scripted `/media` socket, `FakeFleet`);
//! nothing touches host apps. Every wait loop is bounded.

use cua_daemon::{
    Runtime, RuntimeConfig,
    fixtures::{self, SpacesdFixture},
    server::{self, DaemonHandle, ServerConfig},
};
use cua_fleet::testing::FakeFleet;
use cua_sdk::{
    AudioPacket, AudioSink, Cua, CuaError, CuaMode, FrameSink, MediaEvent, MediaOpenOptions,
    MediaSession, ProcessEventKind, ReadinessProbe, SandboxCreateOptions, SpacesdCommand,
    VideoFrame,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const TOKEN: &str = "env-token";
const POOL: &str = "cua-e2e-gw";

struct World {
    env: SpacesdFixture,
    gw: SpacesdFixture,
    fake: FakeFleet,
    _dirs: tempfile::TempDir,
    daemon: Option<DaemonHandle>,
}

#[derive(Clone, Copy, Debug)]
enum Topology {
    Embedded,
    // Unix sockets only; Windows runs the loopback topology.
    #[cfg_attr(not(unix), allow(dead_code))]
    DaemonSocket,
    DaemonLoopback,
}

async fn world(t: Topology) -> (World, Arc<Cua>) {
    let env = fixtures::start_env(Some(TOKEN), None).await;
    // Port forwards on a direct sandbox go over the driver's /tunnel.
    env.mock
        .state
        .advertise(&[cua_spacesd_client::TUNNEL_FORWARD_FEATURE]);
    let gw = fixtures::start_env(None, Some(fixtures::fake_gateway(POOL, POOL))).await;
    let fake = FakeFleet::new();
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        // Never the real ~/.cua registry.
        spaces_home: Some(dirs.path().join("cua")),
        fleet_client: Some(fake.client_with_base(&gw.url)),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    let (cua, daemon) = match t {
        Topology::Embedded => {
            // Host local public URLs in-process: never reach a daemon the
            // user may be running.
            runtime.mark_share_host();
            (Cua::from_runtime(runtime), None)
        }
        Topology::DaemonSocket | Topology::DaemonLoopback => {
            let cfg = ServerConfig {
                socket_path: Some(dirs.path().join("cua.sock")),
                loopback: Some("127.0.0.1:0".parse().unwrap()),
                token: "daemon-token".into(),
                discovery_path: Some(dirs.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            };
            let h = server::start(runtime, cfg).await.unwrap();
            let cua = match t {
                Topology::DaemonSocket => Cua::connect(
                    Some(h.socket_path.as_ref().unwrap().display().to_string()),
                    None,
                )
                .unwrap(),
                _ => Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap(),
            };
            (cua, Some(h))
        }
    };
    (
        World {
            env,
            gw,
            fake,
            _dirs: dirs,
            daemon,
        },
        cua,
    )
}

fn direct(url: &str, name: &str, token: Option<&str>) -> SandboxCreateOptions {
    SandboxCreateOptions {
        on: Some(format!("direct:{url}")),
        kind: None,
        runtime: None,
        image: String::new(),
        name: Some(name.into()),
        token: token.map(str::to_string),
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

fn cmd(program: &str, args: &[&str]) -> SpacesdCommand {
    SpacesdCommand {
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: HashMap::new(),
        cwd: None,
        user: None,
        timeout_ms: None,
        tag: None,
        stdin: false,
        pty: None,
    }
}

#[derive(Default)]
struct Collect {
    frames: Mutex<Vec<VideoFrame>>,
    events: Mutex<Vec<MediaEvent>>,
    audio: Mutex<Vec<AudioPacket>>,
}

impl FrameSink for Collect {
    fn on_frame(&self, frame: VideoFrame) {
        self.frames.lock().unwrap().push(frame);
    }
    fn on_event(&self, event: MediaEvent) {
        self.events.lock().unwrap().push(event);
    }
}

impl AudioSink for Collect {
    fn on_audio(&self, packet: AudioPacket) {
        self.audio.lock().unwrap().push(packet);
    }
}

/// Polls `cond` every 20 ms for at most 5 s.
async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..250 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn assert_media(sink: &Arc<Collect>) {
    eventually("two frames and one audio packet", || {
        sink.frames.lock().unwrap().len() >= 2 && !sink.audio.lock().unwrap().is_empty()
    })
    .await;
    let frames = sink.frames.lock().unwrap().clone();
    assert_eq!(frames[0].sequence, 7);
    assert!(frames[0].keyframe);
    assert_eq!((frames[0].width, frames[0].height), (64, 48));
    assert_eq!(frames[0].codec, "h264");
    assert!(frames[0].data.starts_with(b"\x00\x00\x00\x01\x67"));
    assert_eq!(frames[1].sequence, 8);
    let a = sink.audio.lock().unwrap()[0].clone();
    assert_eq!((a.track_id, a.sequence, a.frame_samples), (3, 11, 960));
    let kinds: Vec<String> = sink
        .events
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.kind.clone())
        .collect();
    assert_eq!(&kinds[..2], ["hello", "session_opened"]);
}

async fn suite(t: Topology) {
    let (w, cua) = world(t).await;
    let expected_mode = match t {
        Topology::Embedded => CuaMode::Embedded,
        _ => CuaMode::Daemon,
    };
    assert_eq!(cua.mode(), expected_mode);
    let info = cua.info().await.unwrap();
    assert_eq!(info.mode, expected_mode);
    assert!(info.features.contains(&"env".to_string()));

    let sbx = cua.sandboxes();

    // ---------------------------------------------------------- direct
    let sb = sbx
        .create(direct(&w.env.url, "direct-1", Some(TOKEN)))
        .await
        .unwrap();
    assert_eq!(sb.name(), "direct-1");
    assert_eq!(sb.location(), "direct");
    assert_eq!(sb.runtime_type(), "direct");
    // The default lists everything, each row with its location; a direct
    // URL is not a local sandbox.
    let listed = sbx.list(None).await.unwrap();
    let row = listed.iter().find(|s| s.name == "direct-1");
    assert_eq!(
        row.map(|s| s.location.as_str()),
        Some("direct"),
        "{listed:?}"
    );
    assert!(
        !sbx.list(Some("local".into()))
            .await
            .unwrap()
            .iter()
            .any(|s| s.name == "direct-1")
    );
    assert!(
        sbx.list(Some("direct".into()))
            .await
            .unwrap()
            .iter()
            .any(|s| s.name == "direct-1")
    );

    let env = sb.spacesd(Some(5_000)).await.unwrap();
    let caps = env.capabilities().await.unwrap();
    assert!(!caps.version.is_empty());
    assert!(caps.json.contains(&caps.version));

    let out = env.run(cmd("echo", &["hi", "there"])).await.unwrap();
    assert!(out.exit.success, "{out:?}");
    assert_eq!(out.stdout, b"hi there\n");
    let out = env.sh("fail 3".into(), None).await.unwrap();
    assert_eq!(out.exit.code, Some(3));

    // Streaming process events (bounded).
    let p = env.spawn(cmd("ticker", &["3", "5"])).await.unwrap();
    assert!(p.pid() > 0);
    let mut ticks = 0;
    let mut exited = false;
    for _ in 0..50 {
        match p.next_event().await.unwrap() {
            Some(e) if e.kind == ProcessEventKind::Exit => {
                assert!(e.exit.unwrap().success);
                exited = true;
            }
            Some(e) => ticks += String::from_utf8_lossy(&e.data).matches("tick").count(),
            None => break,
        }
    }
    assert!(exited);
    assert_eq!(ticks, 3);

    // stdin round trip.
    let mut c = cmd("cat", &[]);
    c.stdin = true;
    let p = env.spawn(c).await.unwrap();
    p.write_stdin(b"abc".to_vec()).await.unwrap();
    p.close_stdin().await.unwrap();
    let out = p.wait().await.unwrap();
    assert_eq!(out.stdout, b"abc");
    assert!(matches!(p.wait().await, Err(CuaError::Closed(_))));

    // Files.
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let up = env
        .upload("/tmp/sdk/blob.bin".into(), data.clone(), None)
        .await
        .unwrap();
    assert_eq!(up.size, data.len() as u64);
    assert_eq!(
        env.download("/tmp/sdk/blob.bin".into()).await.unwrap(),
        data
    );
    let st = env.stat("/tmp/sdk/blob.bin".into()).await.unwrap();
    assert_eq!(st.size, data.len() as u64);
    assert!(matches!(
        env.download("/nope".into()).await,
        Err(CuaError::NotFound(_))
    ));

    // Computer.
    env.click(10.0, 20.0).await.unwrap();
    env.type_text("hello".into()).await.unwrap();
    env.hotkey(vec!["ctrl".into(), "c".into()]).await.unwrap();
    env.set_clipboard("clip".into()).await.unwrap();
    assert_eq!(env.get_clipboard().await.unwrap().as_deref(), Some("clip"));
    let _ = env.cursor_position().await.unwrap();
    assert!(matches!(
        env.press("definitely-not-a-key".into()).await,
        Err(CuaError::Env(_))
    ));

    // JSON escape hatch.
    let health = env
        .call_json("SystemService/Health".into(), "{}".into())
        .await
        .unwrap();
    assert!(health.starts_with('{'), "{health}");
    assert!(matches!(
        env.call_json("ProcessService/StartProcess".into(), "{}".into())
            .await,
        Err(CuaError::InvalidArgument(_))
    ));
    assert!(env.json_methods().len() > 60);

    // Media: frames and audio through SpacesdClient.open_media (embedded:
    // straight to the spacesd; daemon: through the passthrough).
    let sink = Arc::new(Collect::default());
    let session = env
        .open_media_with_audio(
            MediaOpenOptions {
                display: None,
                window_handle: None,
                max_fps: 30,
                max_dimension: 0,
                audio: true,
                disable_video: false,
                request_json: None,
            },
            sink.clone(),
            sink.clone(),
        )
        .await
        .unwrap();
    assert_eq!(session.session_id(), "media-1");
    assert_eq!(session.codec(), "h264");
    assert!(!session.open_response_json().contains("ticket-abc"));
    assert_media(&sink).await;
    session
        .send_control(r#"{"type":"request_keyframe","payload":{}}"#.into())
        .unwrap();
    eventually("echo", || {
        sink.events.lock().unwrap().iter().any(|e| e.kind == "echo")
    })
    .await;
    session.request_keyframe().await.unwrap();
    session.close().await.unwrap();
    eventually("closed", || session.is_closed()).await;
    {
        let m = w.env.media.lock().unwrap();
        assert_eq!(m.attaches, 1);
        // The daemon token never reaches the guest.
        assert_eq!(m.last_authorization, None, "{m:?}");
    }

    // Port forward and readiness.
    let port: u16 = w.env.url.rsplit(':').next().unwrap().parse().unwrap();
    let fwd = sb.forward(port).await.unwrap();
    let local = fwd.local_addr().expect("tcp forward");
    tokio::net::TcpStream::connect(&local).await.unwrap();
    fwd.close().await.unwrap();
    sb.wait_ready(
        vec![ReadinessProbe {
            port,
            http_path: None,
            http_status: None,
            service: None,
        }],
        5_000,
    )
    .await
    .unwrap();

    // Service URLs and public URLs (local: a token-gated loopback proxy
    // hosted by the daemon, or in-process in the embedded test runtime).
    let env_svc = sb.service("env".into()).unwrap();
    assert_eq!(
        env_svc.url().await.unwrap(),
        w.env.url.trim_end_matches('/')
    );
    assert!(matches!(
        sb.public_url("env".into(), Some(10), None).await,
        Err(CuaError::InvalidArgument(_))
    ));
    let public = sb
        .public_url("env".into(), Some(120), Some("topology".into()))
        .await
        .unwrap();
    assert!(
        public.url.starts_with("http://127.0.0.1:") && public.url.contains("/s/"),
        "{public:?}"
    );
    assert!(public.expires_at_unix > 0);
    let mut o = cua_spacesd_client::ConnectOptions::parse(&public.url)
        .unwrap()
        .transport(cua_spacesd_client::TransportPreference::GrpcWeb);
    o.token = Some(TOKEN.into());
    let shared = cua_spacesd_client::SpacesdClient::connect(o.clone())
        .await
        .unwrap();
    assert_eq!(
        shared.run("echo shared").await.unwrap().stdout_str(),
        "shared\n"
    );
    sb.revoke_public_url(public.id.clone()).await.unwrap();
    assert!(
        cua_spacesd_client::SpacesdClient::connect(o).await.is_err(),
        "revoked"
    );

    // Wrong token and no driver: the same typed errors in every topology.
    let bad = sbx
        .create(direct(&w.env.url, "bad-token", Some("wrong")))
        .await
        .unwrap();
    assert!(
        matches!(
            bad.spacesd(Some(3_000)).await,
            Err(CuaError::Unauthenticated(_))
        ),
        "{t:?}"
    );
    // One machine, one ref: the latest connection to an address (and its
    // token) is the one `direct:<host:port>` reaches. Reconnect with the
    // right token.
    assert_eq!(bad.id(), sb.id());
    let sb = sbx
        .create(direct(&w.env.url, "direct-1", Some(TOKEN)))
        .await
        .unwrap();
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_url = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    let none = sbx
        .create(direct(&dead_url, "no-driver", None))
        .await
        .unwrap();
    assert!(
        matches!(
            none.spacesd(Some(1_000)).await,
            Err(CuaError::SpacesdNotAvailable(_))
        ),
        "{t:?}"
    );

    // ----------------------------------------------------------- fleet
    // An explicit, user-owned pool (the gateway fixture knows its name).
    w.fake
        .client()
        .apply_pool(&cua_fleet::PoolSpec::new(
            POOL,
            "ghcr.io/trycua/cua-desktop-linux:test",
        ))
        .await
        .unwrap();
    let mut o = direct("", POOL, None);
    o.on = Some("cloud".into());
    o.pool = Some(POOL.into());
    o.image = "ghcr.io/trycua/cua-desktop-linux:test".into();
    let fsb = sbx.create(o).await.unwrap();
    assert_eq!(fsb.location(), "cloud");
    assert!(w.fake.exists("pool", POOL, POOL));
    let fenv = fsb.spacesd(Some(5_000)).await.unwrap();
    // Embedded: gRPC-Web through the gateway. Daemon: native gRPC to the
    // daemon, which speaks gRPC-Web upstream.
    let expected = if matches!(t, Topology::Embedded) {
        "grpc-web"
    } else {
        "grpc"
    };
    assert_eq!(fenv.transport(), expected);
    let out = fenv.run(cmd("echo", &["gw"])).await.unwrap();
    assert_eq!(out.stdout, b"gw\n");
    let svc = fsb.service("env".into()).unwrap();
    // Cloud: signed service URLs, no Fleet vocabulary outside the details.
    let signed = svc.url().await.unwrap();
    assert!(signed.starts_with("https://signed.fleet.test/"), "{signed}");
    assert_eq!(svc.url().await.unwrap(), signed, "reused until near expiry");
    let p = fsb.public_url("env".into(), Some(600), None).await.unwrap();
    // The same from the service handle.
    let sp = svc.public_url(Some(600), None).await.unwrap();
    assert!(sp.url.starts_with("https://signed.fleet.test/"), "{sp:?}");
    assert_eq!(sp.service, "env");
    assert!(p.url.starts_with("https://signed.fleet.test/"), "{p:?}");
    assert_eq!(p.service, "env");
    assert_eq!(
        p.provider_details.get("claim").map(String::as_str),
        Some(POOL)
    );
    let finfo = fsb.info();
    assert_eq!(finfo.location, "cloud");
    // Listing: the default includes live cloud claims; `Local` never does,
    // and `Fleet` lists only cloud ones.
    let all = sbx.list_with_warnings(None).await.unwrap();
    assert!(all.warnings.is_empty(), "{all:?}");
    assert!(
        all.sandboxes
            .iter()
            .any(|s| s.name == finfo.name && s.location == "cloud"),
        "{all:?}"
    );
    assert!(
        !sbx.list(Some("local".into()))
            .await
            .unwrap()
            .iter()
            .any(|s| s.location == "cloud")
    );
    let cloud = sbx.list(Some("cloud".into())).await.unwrap();
    assert!(cloud.iter().all(|s| s.location == "cloud") && !cloud.is_empty());
    // Deprecated alias.
    assert_eq!(sbx.list_all().await.unwrap().len(), all.sandboxes.len());
    assert_eq!(finfo.phase, cua_sdk::SandboxPhase::Ready);
    assert_eq!(
        finfo.provider_details.get("pool").map(String::as_str),
        Some(POOL)
    );
    let hdr = |n: &str, v: &str| cua_sdk::HttpHeader {
        name: n.into(),
        value: v.into(),
    };
    // Service requests go straight to the gateway (the pipe itself is
    // tested in tests/mcp.rs): the route carries the Fleet bearer and claim.
    let ep = svc.endpoint().await.unwrap();
    if matches!(t, Topology::Embedded) {
        assert!(ep.url.ends_with("-env"), "{ep:?}");
        assert!(
            ep.headers
                .iter()
                .any(|h| h.name == "x-cua-fleet-claim" && h.value == POOL)
        );
    } else {
        assert!(
            ep.url.ends_with("/v1/sandboxes/cloud:cua-e2e-gw/svc/env"),
            "daemon passthrough: {ep:?}"
        );
    }
    // The gateway owns authorization.
    assert!(matches!(
        svc.request(
            "GET".into(),
            "/status".into(),
            None,
            Some(5_000),
            Some(vec![hdr("authorization", "Bearer x")]),
        )
        .await,
        Err(CuaError::InvalidArgument(_))
    ));
    // Fleet media carries the Fleet bearer and claim upstream.
    let fsink = Arc::new(Collect::default());
    let fs = fenv
        .open_media(
            MediaOpenOptions {
                display: None,
                window_handle: None,
                max_fps: 0,
                max_dimension: 0,
                audio: false,
                disable_video: false,
                request_json: None,
            },
            fsink.clone(),
        )
        .await
        .unwrap();
    eventually("fleet frames", || fsink.frames.lock().unwrap().len() >= 2).await;
    fs.close().await.unwrap();
    {
        let m = w.gw.media.lock().unwrap();
        assert_eq!(
            m.last_authorization.as_deref(),
            Some("Bearer fake-fleet-token")
        );
        assert_eq!(m.last_claim.as_deref(), Some(POOL));
    }
    fsb.delete().await.unwrap();
    assert!(
        !w.fake.exists("claim", POOL, POOL),
        "claim released on delete"
    );
    assert!(w.fake.exists("pool", POOL, POOL), "explicit pools stay");

    // Managed pool (no `pool`): same API in every topology; the runtime
    // (in daemon mode, the daemon) holds the claim and its heartbeat.
    let mut o = direct("", "", None);
    o.on = Some("cloud".into());
    o.name = None;
    o.image = "ghcr.io/trycua/cua-desktop-linux:test".into();
    // The runtime defaults from the image's manifest (a fixture here).
    cua_fleet::testing::set_image_variant(&o.image, cua_fleet::ImageVariant::ContainerDisk);
    o.warm = Some(true);
    o.max_pool_size = Some(3);
    o.fleet_ttl_seconds = Some(120);
    let msb = sbx.create(o).await.unwrap();
    assert!(msb.is_ephemeral());
    let managed: Vec<String> = w
        .fake
        .all_namespaces()
        .into_iter()
        .filter(|n| n.starts_with("cua-auto-"))
        .collect();
    assert_eq!(managed.len(), 1, "{managed:?}");
    let mp = &managed[0];
    let pool = w.fake.object("pool", mp, mp).unwrap();
    assert_eq!(pool["spec"]["replicas"], 1);
    assert_eq!(pool["spec"]["autoscaling"]["maxPoolSize"], 3);
    let claims = w.fake.names("claim", mp);
    assert_eq!(claims.len(), 1);
    let claim = w.fake.object("claim", mp, &claims[0]).unwrap();
    assert_eq!(claim["spec"]["ttlSecondsAfterCreated"], 120);
    msb.delete().await.unwrap();
    assert!(w.fake.names("claim", mp).is_empty(), "claim released");
    assert!(w.fake.exists("pool", mp, mp), "managed pool kept for reuse");

    // Fleet's per-account size cap refuses a bigger sandbox: the caller
    // gets the typed denial with Fleet's own message, in every topology.
    w.fake.faults.lock().unwrap().size_cap = Some((8, 32 * 1024));
    let mut big = direct("", "", None);
    big.on = Some("cloud".into());
    big.name = None;
    big.image = "ghcr.io/trycua/cua-desktop-linux:test".into();
    big.cpus = Some(16);
    big.memory_mb = Some(64 * 1024);
    match sbx.create(big).await {
        Err(CuaError::FleetAdmissionDenied(m)) => {
            assert!(m.contains(cua_fleet::testing::SIZE_LIMIT_MESSAGE), "{m}");
        }
        Err(e) => panic!("{t:?}: expected FleetAdmissionDenied, got {e:?}"),
        Ok(_) => panic!("{t:?}: a 16 vCPU sandbox was admitted over the size cap"),
    }
    w.fake.faults.lock().unwrap().size_cap = None;
    if matches!(t, Topology::Embedded) {
        let pools = cua.fleet().unwrap().pools();
        let listed = pools.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(&listed[0].name, mp);
        assert!(listed[0].managed);
        let r = pools.gc(Some(0)).await.unwrap();
        assert_eq!(r.deleted_pools, vec![mp.clone()]);
        assert!(!w.fake.exists("pool", mp, mp));
    }

    // ---------------------------------------------------------- errors
    assert!(matches!(
        sbx.get("nope".into()).await,
        Err(CuaError::NotFound(_))
    ));
    let mut local = direct("", "loc", None);
    local.on = Some("local".into());
    local.image = "img".into();
    assert!(matches!(
        sbx.create(local).await,
        Err(CuaError::ProviderNotConfigured(_))
    ));
    // Overlays are validated before anything is created (a real overlay
    // needs a guest; the mock spacesd runs commands on this host, so none
    // is applied here).
    let mut bad = direct(&w.env.url, "overlay-bad", Some(TOKEN));
    bad.overlays = vec![cua_sdk::Overlay {
        name: "tool".into(),
        path: "./tool".into(),
        target: None,
        source: None,
    }];
    assert!(matches!(
        sbx.create(bad).await,
        Err(CuaError::InvalidArgument(m)) if m.contains("guest path")
    ));
    assert!(matches!(
        sbx.get("overlay-bad".into()).await,
        Err(CuaError::NotFound(_))
    ));
    let report = cua.local().doctor().await.unwrap();
    assert!(report.checks.iter().any(|c| c.name == "qemu"), "{report:?}");
    assert!(report.report_json.contains("\"backends\""));
    assert!(matches!(
        cua.local().setup(vec!["nope".into()], true).await,
        Err(CuaError::InvalidArgument(_))
    ));

    // ------------------------------------------------ daemon-only extras
    if let Some(d) = &w.daemon {
        let bridge = sb.open_media_bridge(None).await.unwrap();
        assert!(bridge.ws_url.starts_with("ws://127.0.0.1:"));
        assert!(!bridge.open_media_response_json.contains("ticket-abc"));
        let bsink = Arc::new(Collect::default());
        let bs = MediaSession::attach_url(
            bridge.ws_url.clone(),
            vec![],
            bsink.clone(),
            Some(bsink.clone()),
        )
        .await
        .unwrap();
        assert_media(&bsink).await;
        bs.close().await.unwrap();
        // A forged ticket is refused before any frame.
        let forged = bridge.ws_url.replace(&bridge.ticket, "forged");
        assert!(matches!(
            MediaSession::attach_url(forged, vec![], bsink.clone(), None).await,
            Err(CuaError::Unauthenticated(_))
        ));
        // Spaces registry (daemon-hosted).
        let spaces = cua.spaces();
        let space = spaces
            .add(w.env.url.clone(), Some(TOKEN.into()), Some("dev".into()))
            .await
            .unwrap();
        assert!(space.id.starts_with("direct:127.0.0.1:"), "{}", space.id);
        assert_eq!(spaces.list().await.unwrap().len(), 1);
        spaces.remove(space.id).await.unwrap();
        assert!(d.loopback_url.is_some());
    } else {
        assert!(matches!(
            sb.open_media_bridge(None).await,
            Err(CuaError::Unsupported(_))
        ));
    }

    sb.delete().await.unwrap();
    assert!(
        !sbx.list(None)
            .await
            .unwrap()
            .iter()
            .any(|s| s.name == "direct-1")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded() {
    tokio::time::timeout(Duration::from_secs(120), suite(Topology::Embedded))
        .await
        .expect("suite timed out");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_over_unix_socket() {
    tokio::time::timeout(Duration::from_secs(120), suite(Topology::DaemonSocket))
        .await
        .expect("suite timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_over_loopback_token() {
    tokio::time::timeout(Duration::from_secs(120), suite(Topology::DaemonLoopback))
        .await
        .expect("suite timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_rejects_wrong_token_and_reports_missing_daemon() {
    let (w, _) = world(Topology::DaemonLoopback).await;
    let d = w.daemon.as_ref().unwrap();
    let wrong = Cua::connect(d.loopback_url.clone(), Some("nope".into())).unwrap();
    assert!(matches!(
        wrong.info().await,
        Err(CuaError::Unauthenticated(_))
    ));
    // Nothing listens: a missing Unix socket, or on Windows (no Unix
    // sockets) a loopback port that was just released.
    let dir = tempfile::tempdir().unwrap();
    let address = if cfg!(unix) {
        dir.path().join("none.sock").display().to_string()
    } else {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        format!("http://127.0.0.1:{port}")
    };
    let token = (!cfg!(unix)).then(|| "t".to_string());
    let missing = Cua::connect(Some(address), token).unwrap();
    assert!(matches!(
        missing.info().await,
        Err(CuaError::DaemonNotRunning(_))
    ));
    // Shutdown over RPC stops the listeners.
    let cua = Cua::connect(d.loopback_url.clone(), Some(d.token.clone())).unwrap();
    cua.shutdown_daemon().await.unwrap();
    let h = { w }.daemon.unwrap();
    tokio::time::timeout(Duration::from_secs(5), h.wait())
        .await
        .expect("daemon stopped");
}
