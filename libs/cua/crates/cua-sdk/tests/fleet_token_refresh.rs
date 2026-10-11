//! A cloud Space's `SpacesdClient` held past the Fleet access token's
//! lifetime: media sockets opened later must carry a fresh bearer (the
//! gateway answers 302 to an expired one). In-process fakes only
//! (`FakeFleet`, a `MockServer` behind the gateway emulation); every wait is
//! bounded.

use cua_daemon::{Runtime, RuntimeConfig, fixtures};
use cua_fleet::sdk::{AccessTokenProvider, AccessTokenProviderError};
use cua_fleet::testing::FakeFleet;
use cua_sdk::{Cua, FrameSink, MediaEvent, MediaOpenOptions, SandboxCreateOptions, VideoFrame};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const POOL: &str = "cua-e2e-gw";
const IMAGE: &str = "ghcr.io/trycua/linux:24.04";

/// The signed-in account's bearer: replaced when it expires.
struct Rotating(Arc<Mutex<String>>);

#[async_trait::async_trait]
impl AccessTokenProvider for Rotating {
    async fn get_access_token(&self, _force: bool) -> Result<String, AccessTokenProviderError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

struct Discard;

impl FrameSink for Discard {
    fn on_frame(&self, _: VideoFrame) {}
    fn on_event(&self, _: MediaEvent) {}
}

fn cloud(name: &str) -> SandboxCreateOptions {
    SandboxCreateOptions {
        on: Some("cloud".into()),
        kind: None,
        runtime: None,
        image: IMAGE.into(),
        name: Some(name.into()),
        token: None,
        pool: Some(POOL.into()),
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

fn bearer(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn a_held_cloud_spacesd_client_opens_media_with_a_fresh_fleet_bearer() {
    let gw = fixtures::start_env(None, Some(fixtures::fake_gateway(POOL, POOL))).await;
    let fake = FakeFleet::new();
    let token = Arc::new(Mutex::new(fixtures::FAKE_FLEET_TOKEN.to_string()));
    let fleet = cua_fleet::FleetClient::connect_with_token_provider(
        cua_fleet::FleetConfig {
            base_url: gw.url.clone(),
            pool_poll_interval_ms: 1,
            claim_poll_interval_ms: 1,
            claim_poll_limit: 50,
            ..Default::default()
        },
        Arc::new(Rotating(token.clone())),
        Some(Arc::new(fake.clone())),
    )
    .unwrap();
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        // Never the real ~/.cua registry.
        spaces_home: Some(dirs.path().join("cua")),
        fleet_client: Some(fleet.clone()),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    runtime.mark_share_host();
    let cua = Cua::from_runtime(runtime);
    #[allow(deprecated)] // the fixture pool, as in tests/topologies.rs
    fleet
        .apply_pool(&cua_fleet::PoolSpec::new(POOL, IMAGE))
        .await
        .unwrap();
    let sandbox = cua.sandboxes().create(cloud(POOL)).await.unwrap();
    assert_eq!(sandbox.location(), "cloud");

    // Held for the whole test, like an app keeping its connection object.
    let env = sandbox.spacesd(Some(5_000)).await.unwrap();
    let options = || MediaOpenOptions {
        display: None,
        window_handle: None,
        max_fps: 0,
        max_dimension: 0,
        audio: false,
        disable_video: false,
        request_json: None,
    };
    let session = env.open_media(options(), Arc::new(Discard)).await.unwrap();
    let first = gw.media.lock().unwrap().last_authorization.clone();
    assert_eq!(
        first.as_deref(),
        Some(format!("Bearer {}", fixtures::FAKE_FLEET_TOKEN).as_str())
    );
    drop(session);

    // The access token expires and the account's provider mints a new one.
    *token.lock().unwrap() = "rotated-fleet-token".into();
    let fresh = env.current_ws_headers().await.unwrap();
    assert_eq!(bearer(&fresh), Some("Bearer rotated-fleet-token"));
    assert!(
        fresh
            .iter()
            .any(|(k, v)| k == "x-cua-fleet-claim" && v == POOL),
        "{fresh:?}"
    );
    // What the client was created with is the expired bearer; the media
    // path must not use it.
    assert_eq!(
        bearer(&env.ws_headers()),
        Some(format!("Bearer {}", fixtures::FAKE_FLEET_TOKEN).as_str())
    );
}
