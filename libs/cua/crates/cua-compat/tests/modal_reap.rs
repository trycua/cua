//! Ephemeral Apps end with their Sandboxes when the client stops heartbeating.
//! Its own test binary: the grace and reap interval are process environment.

use cua_compat::backend::mock::MockBackend;
use cua_compat::modal::pb::api as pb;
use cua_compat::modal::pb::api::modal_client_client::ModalClientClient;
use cua_compat::{Gateway, GatewayOptions, Provider};
use std::sync::Arc;
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

struct Gw {
    gw: Gateway,
    chan: Channel,
    _dir: tempfile::TempDir,
}

async fn start() -> Gw {
    let dir = tempfile::tempdir().unwrap();
    // Image builds are logged, not run (the mock Space has no shell).
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: set once, before any gateway thread of this test binary reads it.
    ONCE.call_once(|| unsafe {
        std::env::set_var("CUA_MODAL_BUILD_DRY_RUN", "1");
        std::env::set_var("CUA_MODAL_HEARTBEAT_GRACE_S", "2");
        std::env::set_var("CUA_MODAL_REAP_INTERVAL_S", "1");
    });
    let mut o = GatewayOptions::new(Provider::Modal);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    let gw = Gateway::start(o, Some(Arc::new(MockBackend::new())))
        .await
        .unwrap();
    let chan = Channel::from_shared(format!("http://{}", gw.api_addr))
        .unwrap()
        .connect()
        .await
        .unwrap();
    Gw {
        gw,
        chan,
        _dir: dir,
    }
}

impl Gw {
    fn client(&self) -> ModalClientClient<Channel> {
        ModalClientClient::new(self.chan.clone())
    }

    fn token(&self) -> (String, String) {
        let env: std::collections::HashMap<_, _> = self.gw.env().into_iter().collect();
        (
            env["MODAL_TOKEN_ID"].clone(),
            env["MODAL_TOKEN_SECRET"].clone(),
        )
    }

    fn authed<T>(&self, msg: T) -> Request<T> {
        let (id, secret) = self.token();
        let mut r = Request::new(msg);
        r.metadata_mut()
            .insert("x-modal-token-id", MetadataValue::try_from(id).unwrap());
        r.metadata_mut().insert(
            "x-modal-token-secret",
            MetadataValue::try_from(secret).unwrap(),
        );
        r
    }
}

#[tokio::test]
async fn an_ephemeral_app_ends_with_its_sandboxes_when_heartbeats_stop() {
    let g = start().await;
    let mut c = g.client();
    // `app.run()`: an ephemeral App.
    let app = c
        .app_create(g.authed(pb::AppCreateRequest {
            description: "ephemeral".into(),
            app_state: pb::AppState::Ephemeral as i32,
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let image = c
        .image_get_or_create(g.authed(pb::ImageGetOrCreateRequest {
            app_id: app.app_id.clone(),
            image: Some(pb::Image {
                dockerfile_commands: vec!["FROM ghcr.io/trycua/linux:24.04".into()],
                ..Default::default()
            }),
            builder_version: "2025.06".into(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner()
        .image_id;
    let sb = c
        .sandbox_create(g.authed(pb::SandboxCreateRequest {
            app_id: app.app_id.clone(),
            definition: Some(pb::Sandbox {
                entrypoint_args: vec!["sleep".into(), "600000".into()],
                image_id: image,
                timeout_secs: 120,
                ..Default::default()
            }),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    // Heartbeats keep it alive well past the grace.
    for _ in 0..4 {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        c.app_heartbeat(g.authed(pb::AppHeartbeatRequest {
            app_id: app.app_id.clone(),
        }))
        .await
        .unwrap();
    }
    let wait = |c: &mut ModalClientClient<Channel>, secs: f32| {
        let r = g.authed(pb::SandboxWaitRequest {
            sandbox_id: sb.sandbox_id.clone(),
            timeout: secs,
        });
        let mut c = c.clone();
        async move { c.sandbox_wait(r).await.unwrap().into_inner() }
    };
    assert!(
        wait(&mut c, 0.0).await.result.is_none(),
        "ended while heartbeats came"
    );
    // Then they stop.
    let ended = wait(&mut c, 15.0).await;
    let result = ended.result.expect("the sandbox ended after the grace");
    assert_ne!(result.status, 0);
}
