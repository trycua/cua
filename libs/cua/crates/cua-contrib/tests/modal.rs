//! Modal through the core, with `cua-contrib-fake-modal-helper` standing in
//! for `cua-modal-helper` (same JSON protocol, sandboxes in a state file).
//! The helper's own mapping onto Modal's Go SDK is tested in
//! `modal-helper/main_test.go`. Hermetic: loopback and a temp dir only.

mod common;

use cua_contrib::{
    common::Secret,
    image_config::{FixedConfigSource, ImageConfig},
    modal::{Modal, ModalConfig},
};
use cua_sandbox_core::{CreateOptions, Error, Provider, ProviderKind, Sandboxes, Status};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};

const FAKE: &str = env!("CARGO_BIN_EXE_cua-contrib-fake-modal-helper");

fn provider(helper: Option<PathBuf>, secret: &str) -> Modal {
    Modal::new(ModalConfig {
        token_id: Some(Secret::new("tok-id")),
        token_secret: Some(Secret::new(secret)),
        helper,
        app: "cua-e2e-sandboxes".into(),
    })
    .with_image_configs(Arc::new(FixedConfigSource(ImageConfig {
        entrypoint: vec!["/entrypoint.sh".into()],
        cmd: vec!["--desktop".into()],
        ..Default::default()
    })))
}

fn opts(image: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Contrib, image);
    o.contrib = Some("modal".into());
    o.ready_timeout = Duration::from_secs(60);
    o
}

fn requests(state: &std::path::Path) -> Vec<Value> {
    let v: Value = serde_json::from_slice(&std::fs::read(state).unwrap()).unwrap();
    v["requests"].as_array().unwrap().clone()
}

#[tokio::test]
async fn modal_sandboxes_through_the_helper_protocol() {
    common::registry();
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("modal-state.json");
    let spacesd = MockServer::start(MockAuth::default()).await;
    // SAFETY: this binary's only test that sets them; the fake helper
    // (a child process) reads them.
    unsafe {
        std::env::set_var("CUA_FAKE_MODAL_STATE", &state);
        std::env::set_var("CUA_FAKE_MODAL_TUNNEL", spacesd.url().trim_end_matches('/'));
    }
    let sbx = Sandboxes::builder()
        .provider(Arc::new(provider(Some(FAKE.into()), "tok-secret")))
        .state_dir(dir.path().join("sandboxes"))
        .build();

    let sb = sbx
        .create(
            opts(common::IMAGE)
                .name("cua-e2e-modal-a")
                .service("novnc", 6080),
        )
        .await
        .unwrap();
    assert_eq!(sb.id(), "modal:cua-e2e-modal-a");
    sb.spacesd().await.unwrap().health().await.unwrap();
    assert_eq!(
        sb.service("novnc").unwrap().endpoint().await.unwrap().url,
        format!("https://{}-6080.w.modal.host", sb.provider_details()["id"])
    );

    let create = requests(&state)
        .into_iter()
        .find(|r| r["op"] == "create")
        .unwrap();
    assert_eq!(create["tokens"], true);
    assert_eq!(create["app"], "cua-e2e-sandboxes");
    let image = create["image"].as_str().unwrap();
    assert!(image.starts_with("ghcr.io/trycua/linux@sha256:"), "{image}");
    // The image's ENTRYPOINT + CMD, as the whole command (the helper clears
    // Modal's kept ENTRYPOINT).
    assert_eq!(
        create["argv"],
        serde_json::json!(["/entrypoint.sh", "--desktop"])
    );
    assert_eq!(
        (create["cpus"].as_f64(), create["memory_mib"].as_u64()),
        (Some(2.0), Some(4096))
    );
    assert_eq!(create["timeout_secs"], 3600);
    let ports: Vec<u64> = create["ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_u64().unwrap())
        .collect();
    assert!(ports.contains(&3211) && ports.contains(&6080), "{ports:?}");
    // Modal runs the command with the sandbox environment: the token is
    // delivered at create.
    assert_eq!(
        create["env"]["CUA_ENV_TOKEN"].as_str().map(str::len),
        Some(32)
    );
    assert_eq!(create["tags"]["cua.name"], "cua-e2e-modal-a");
    // Token values never reach the recorded request.
    let raw = std::fs::read_to_string(&state).unwrap();
    assert!(!raw.contains("tok-secret"));

    // A command replaces the image's.
    let cmd = sbx
        .create(opts(common::IMAGE).command(["sleep", "infinity"]))
        .await
        .unwrap();
    let last = requests(&state)
        .into_iter()
        .rfind(|r| r["op"] == "create")
        .unwrap();
    assert_eq!(last["argv"], serde_json::json!(["sleep", "infinity"]));
    cmd.delete().await.unwrap();

    // Reattach, list, suspend refused, delete.
    let again = sbx.connect("cua-e2e-modal-a").await.unwrap();
    assert_eq!(again.status().await.unwrap(), Status::Running);
    let modal = sbx.contrib_provider("modal").unwrap().clone();
    assert_eq!(modal.list().await.unwrap().len(), 1);
    assert!(matches!(
        sbx.suspend("cua-e2e-modal-a").await.unwrap_err(),
        Error::Unsupported { .. }
    ));
    again.delete().await.unwrap();
    assert!(modal.list().await.unwrap().is_empty());
    // Deleting what is gone is fine.
    modal.delete("sb-000001").await.unwrap();

    // Wrong tokens: the helper's auth error, typed and named.
    let bad = provider(Some(FAKE.into()), "wrong");
    let err = bad.list().await.unwrap_err();
    assert!(matches!(err, Error::ContribNotConfigured(_)), "{err:?}");
    assert!(err.to_string().contains("MODAL_TOKEN_ID"), "{err}");
    assert!(!err.to_string().contains("wrong"), "{err}");
}

#[tokio::test]
async fn a_missing_helper_or_tokens_are_named_before_any_call() {
    let missing = provider(Some("/nonexistent/cua-modal-helper".into()), "tok-secret");
    let err = missing.check_configured().unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err:?}");
    assert!(err.to_string().contains("go build"), "{err}");

    let no_tokens = Modal::new(ModalConfig {
        token_id: None,
        token_secret: None,
        helper: Some(FAKE.into()),
        app: "x".into(),
    });
    let err = no_tokens.check_configured().unwrap_err();
    assert!(err.to_string().contains("MODAL_TOKEN_SECRET"), "{err}");
}
