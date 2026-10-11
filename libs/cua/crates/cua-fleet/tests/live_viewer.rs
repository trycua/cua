//! Live check of the cua-spacesd HTML5 viewer on Fleet, through the gateway.
//! Skipped unless `CUA_E2E_FLEET=1` and `CUA_E2E_FLEET_VIEWER_IMAGE` are set
//! (and Fleet credentials).
//!
//! One `cua-e2e-viewer-<rand>` pool (gVisor by default) with per-claim
//! Secrets, one claim with a fresh claim token, then what a browser does:
//!
//! - a viewer ticket from `SystemService.CreateViewerTicket` (the root
//!   token only on this side);
//! - a signed service URL for `env` (no Fleet bearer in the browser);
//! - `GET <signed>/viewer/` with no credentials;
//! - gRPC-Web with only the viewer ticket (`x-cua-env-authorization`):
//!   capabilities allowed, `ProcessService` refused, `OpenMedia` allowed;
//! - the `/media` WebSocket through the signed URL: `hello`,
//!   `session_opened` and a keyframe.
//!
//! With `CUA_E2E_FLEET_VIEWER_URL_FILE` the viewer link is written there and
//! the claim is held for `CUA_E2E_FLEET_VIEWER_HOLD_SECS` (a browser run can
//! open it). The pool and its namespace are deleted whatever happened.
//!
//! ```sh
//! set -a; source ~/.env; set +a
//! CUA_E2E_FLEET=1 CUA_E2E_FLEET_VIEWER_IMAGE=<amd64 image> \
//!   cargo test -p cua-fleet --test live_viewer -- --nocapture
//! ```

use std::time::{Duration, Instant};

use cua_fleet::{
    ClaimOptions, FleetClient, PoolOptions, RuntimeKind, SandboxSpec,
    claim_secrets::generate_claim_token,
};
use cua_spacesd_client::{ConnectOptions, Endpoint, SpacesdClient, TransportPreference, pb};
use futures_util::StreamExt as _;

fn image() -> Option<String> {
    if std::env::var("CUA_E2E_FLEET").as_deref() != Ok("1") {
        return None;
    }
    std::env::var("CUA_E2E_FLEET_VIEWER_IMAGE")
        .ok()
        .filter(|s| !s.is_empty())
}

fn runtime() -> RuntimeKind {
    match std::env::var("CUA_E2E_FLEET_VIEWER_RUNTIME").as_deref() {
        Ok("kubevirt") => RuntimeKind::Kubevirt,
        _ => RuntimeKind::Gvisor,
    }
}

async fn run(fleet: &FleetClient, name: &str, image: &str) -> Result<(), String> {
    let t0 = Instant::now();
    let spec = SandboxSpec {
        services: [("env".to_string(), 3211)].into(),
        claim_secrets: true,
        memory_mb: Some(4096),
        cpu: Some(2),
        ..SandboxSpec::new(image.to_string())
    };
    let options = PoolOptions {
        runtime: Some(runtime()),
        warm: Some(true),
        max_pool_size: Some(1),
        idle_ttl: Some(Duration::from_secs(3600)),
        pool_ttl: Some(Duration::from_secs(2 * 3600)),
        ..Default::default()
    };
    let handle = fleet
        .apply(name, &spec, &options)
        .await
        .map_err(|e| format!("apply: {e}"))?;
    fleet
        .wait_pool_ready(&handle, Duration::from_secs(1500))
        .await
        .map_err(|e| format!("warm: {e}"))?;
    println!(
        "[{:>4}s] warm replica ready ({:?})",
        t0.elapsed().as_secs(),
        runtime()
    );
    let token = generate_claim_token();
    let bound = fleet
        .acquire(
            &handle.pool,
            ClaimOptions {
                name: Some(format!("{name}-c")),
                claim_token: Some(token.clone()),
                ttl_seconds_after_created: Some(3600),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("acquire: {e}"))?;
    let root = SpacesdClient::connect(
        fleet
            .env_connect_options(&bound, "env", Some(token))
            .map_err(|e| format!("gateway options: {e}"))?,
    )
    .await
    .map_err(|e| format!("connect through the gateway: {e}"))?;
    let minted = root
        .system()
        .create_viewer_ticket(pb::CreateViewerTicketRequest {
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 1800,
                nanos: 0,
            }),
            clipboard: true,
            files_root: "~".into(),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("CreateViewerTicket through the gateway: {e}"))?
        .into_inner();
    let signed = fleet
        .create_signed_service_url(
            &bound,
            "env",
            Some("cua-e2e viewer".into()),
            Duration::from_secs(1800),
        )
        .await
        .map_err(|e| format!("signed service URL: {e}"))?;
    let base = signed.url.trim_end_matches('/').to_string();
    println!(
        "[{:>4}s] claim {} bound; signed env URL minted",
        t0.elapsed().as_secs(),
        bound.claim
    );

    // 1. The page, with no credentials at all.
    let http = reqwest::Client::new();
    let page = http
        .get(format!("{base}/viewer/"))
        .send()
        .await
        .map_err(|e| format!("GET /viewer/: {e}"))?;
    let status = page.status();
    let html = page.text().await.unwrap_or_default();
    if !status.is_success() || !html.contains("viewer.js") {
        return Err(format!(
            "GET /viewer/ through the signed URL: HTTP {status}"
        ));
    }
    let js = http
        .get(format!("{base}/viewer/viewer.js"))
        .send()
        .await
        .map_err(|e| format!("GET viewer.js: {e}"))?;
    if !js.status().is_success() {
        return Err(format!("GET /viewer/viewer.js: HTTP {}", js.status()));
    }
    println!(
        "[{:>4}s] page and script served through the gateway",
        t0.elapsed().as_secs()
    );

    // 2. gRPC-Web with only the viewer ticket, like the page.
    let mut viewer_opts = ConnectOptions::new(Endpoint::parse(&base).map_err(|e| e.to_string())?)
        .transport(TransportPreference::GrpcWeb)
        .probe(false);
    viewer_opts.token = Some(minted.ticket.clone());
    let viewer = SpacesdClient::connect(viewer_opts)
        .await
        .map_err(|e| format!("viewer connect: {e}"))?;
    viewer
        .system()
        .get_capabilities(pb::GetCapabilitiesRequest {})
        .await
        .map_err(|e| format!("GetCapabilities with the viewer ticket: {e}"))?;
    match viewer
        .process()
        .list_processes(pb::ListProcessesRequest::default())
        .await
    {
        Err(e) if format!("{:?}", e.code()) == "PermissionDenied" => {}
        other => {
            return Err(format!(
                "ListProcesses with a viewer ticket: {:?}",
                other.map(|_| "allowed")
            ));
        }
    }
    let media = viewer
        .stream()
        .open_media(pb::OpenMediaRequest {
            target: Some(pb::MediaTarget {
                target: Some(pb::media_target::Target::DisplayId("primary".into())),
            }),
            codecs: vec![pb::MediaCodec::H264 as i32, pb::MediaCodec::Png as i32],
            policy: pb::SessionPolicy::AllowActivation as i32,
            ..Default::default()
        })
        .await
        .map_err(|e| format!("OpenMedia with the viewer ticket: {e}"))?
        .into_inner();
    println!(
        "[{:>4}s] viewer-ticket gRPC-Web ok (process refused); media {}",
        t0.elapsed().as_secs(),
        media.media_session_id
    );

    // 3. The media socket through the signed URL.
    let ws_url = format!(
        "{}{}",
        base.replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1),
        media.ws_path
    );
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(30),
        tokio_tungstenite::connect_async(ws_url.as_str()),
    )
    .await
    .map_err(|_| "media socket: connect timed out".to_string())?
    .map_err(|e| format!("media socket: {e}"))?;
    let (mut hello, mut opened, mut keyframe) = (false, false, false);
    let deadline = Instant::now() + Duration::from_secs(30);
    for _ in 0..2000 {
        if keyframe || Instant::now() > deadline {
            break;
        }
        let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await
        else {
            continue;
        };
        match msg {
            tokio_tungstenite::tungstenite::Message::Text(t) => {
                hello |= t.contains("\"hello\"");
                opened |= t.contains("\"session_opened\"");
            }
            tokio_tungstenite::tungstenite::Message::Binary(b)
                if b.first() == Some(&0) && b.len() > 8 =>
            {
                let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
                if let Some(header) = b.get(8..8 + n) {
                    keyframe |= String::from_utf8_lossy(header).contains("\"keyframe\":true");
                }
            }
            _ => {}
        }
    }
    let _ = ws.close(None).await;
    if !(hello && opened && keyframe) {
        return Err(format!(
            "media socket: hello {hello}, session_opened {opened}, keyframe {keyframe}"
        ));
    }
    println!(
        "[{:>4}s] media socket through the gateway: hello, session_opened, keyframe",
        t0.elapsed().as_secs()
    );

    if let Ok(file) = std::env::var("CUA_E2E_FLEET_VIEWER_URL_FILE") {
        std::fs::write(&file, format!("{base}{}", minted.viewer_path))
            .map_err(|e| format!("write {file}: {e}"))?;
        let hold: u64 = std::env::var("CUA_E2E_FLEET_VIEWER_HOLD_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        println!("viewer link written to {file}; holding the claim {hold}s");
        tokio::time::sleep(Duration::from_secs(hold.min(1800))).await;
    }
    fleet
        .release(&bound.namespace, &bound.claim)
        .await
        .map_err(|e| format!("release: {e}"))?;
    Ok(())
}

#[tokio::test]
async fn live_viewer_through_the_gateway() {
    let Some(image) = image() else {
        eprintln!("skipped: set CUA_E2E_FLEET=1 and CUA_E2E_FLEET_VIEWER_IMAGE");
        return;
    };
    let fleet = FleetClient::from_env().expect("Fleet credentials");
    let name = format!("cua-e2e-viewer-{:06x}", rand::random::<u32>() & 0xff_ffff);
    let result = run(&fleet, &name, &image).await;
    for c in fleet.list_claims(&name).await.unwrap_or_default() {
        let _ = fleet.release(&name, &c.metadata.name).await;
    }
    match fleet.get_pool(&name).await {
        Ok(mut h) => {
            h.template = fleet
                .sdk()
                .get_template(name.clone(), name.clone())
                .await
                .ok();
            if let Err(e) = fleet.delete_pool(h).await {
                eprintln!("CLEANUP FAILED for {name}: {e}");
            } else {
                println!("deleted {name}");
            }
        }
        Err(e) => eprintln!("cleanup: pool {name}: {e}"),
    }
    result.unwrap();
}
