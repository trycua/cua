//! E2B through the core against the schema mock of its API
//! (`cua_contrib::testing::MockE2b`, fields from E2B's published OpenAPI
//! spec; not recorded traffic). Hermetic: loopback only.

mod common;

use cua_contrib::{
    common::Secret,
    e2b::{E2b, E2bConfig},
    image_config::FixedConfigSource,
    testing::MockE2b,
};
use cua_sandbox_core::{CreateOptions, Error, ProviderKind, Sandboxes, Status};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::{sync::Arc, time::Duration};

const KEY: &str = "e2b_test_key_not_real";

struct Env {
    _dir: tempfile::TempDir,
    api: MockE2b,
    _spacesd: MockServer,
    sbx: Sandboxes,
}

async fn env(key: Option<&str>) -> Env {
    common::registry();
    let dir = tempfile::tempdir().unwrap();
    let api = MockE2b::start(KEY).await;
    // Port URLs go to the mock spacesd under a `/p<port>` prefix.
    let spacesd = MockServer::start(MockAuth {
        prefix: Some("/p3211".into()),
        ..Default::default()
    })
    .await;
    let provider = E2b::new(E2bConfig {
        api_key: key.map(Secret::new),
        api_url: api.url(),
        domain: "e2b.test".into(),
        port_url: Some(format!("{}/p{{port}}", spacesd.url().trim_end_matches('/'))),
        build_poll: Duration::from_millis(10),
    })
    .with_image_configs(Arc::new(FixedConfigSource(common::image_config())));
    let sbx = Sandboxes::builder()
        .provider(Arc::new(provider))
        .state_dir(dir.path().join("sandboxes"))
        .build();
    Env {
        _dir: dir,
        api,
        _spacesd: spacesd,
        sbx,
    }
}

fn opts(image: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Contrib, image);
    o.contrib = Some("e2b".into());
    o.ready_timeout = Duration::from_secs(30);
    o
}

fn routes(api: &MockE2b, prefix: &str) -> Vec<cua_contrib::testing::Seen> {
    api.seen()
        .into_iter()
        .filter(|s| s.route.starts_with(prefix))
        .collect()
}

#[tokio::test]
async fn builds_a_template_once_per_digest_and_runs_sandboxes_from_it() {
    let e = env(Some(KEY)).await;
    let sb = e
        .sbx
        .create(opts(common::IMAGE).name("cua-e2e-e2b-a"))
        .await
        .unwrap();
    assert_eq!(sb.id(), "e2b:cua-e2e-e2b-a");
    assert_eq!(sb.status().await.unwrap(), Status::Running);

    // One template, built from the pinned image with the image's
    // entrypoint as the start command and a readiness check on 3211.
    let made = routes(&e.api, "POST /v3/templates");
    assert_eq!(made.len(), 1);
    let name = made[0].body["name"].as_str().unwrap().to_string();
    assert!(name.starts_with("cua-") && name.len() == 20, "{name}");
    assert_eq!(made[0].body["cpuCount"], 2);
    assert_eq!(made[0].body["memoryMB"], 4096);
    let build = &routes(&e.api, "POST /v2/templates/")[0].body;
    let from = build["fromImage"].as_str().unwrap();
    assert!(from.starts_with("ghcr.io/trycua/linux@sha256:"), "{from}");
    assert_eq!(build["steps"][0]["type"], "USER");
    assert_eq!(build["steps"][0]["args"][0], "root");
    let start = build["startCmd"].as_str().unwrap();
    assert!(start.contains("nohup /entrypoint.sh"), "{start}");
    assert!(start.ends_with('&'), "{start}");
    assert!(build["readyCmd"].as_str().unwrap().contains("/3211"));

    // The sandbox: that template, a TTL backstop and cua labels; no token
    // in the environment (E2B snapshots the entrypoint at build time; the
    // SDK installs the token with Init).
    let created = &routes(&e.api, "POST /v2/sandboxes")[0].body;
    assert_eq!(created["timeout"], 3600);
    assert_eq!(created["metadata"]["cua.managed"], "true");
    assert_eq!(created["metadata"]["cua.name"], "cua-e2e-e2b-a");
    assert!(created["envVars"].get("CUA_ENV_TOKEN").is_none());

    // cua-spacesd is reachable over the port URL.
    sb.spacesd().await.unwrap().health().await.unwrap();

    // Same image again: the template is reused, not rebuilt.
    let eph = e.sbx.create(opts(common::IMAGE)).await.unwrap();
    assert_eq!(routes(&e.api, "POST /v3/templates").len(), 1);
    eph.delete().await.unwrap();

    // A moved tag (new digest) builds a new template.
    let moved = e.sbx.create(opts(common::MOVED)).await.unwrap();
    assert_eq!(e.api.templates().len(), 2);
    moved.delete().await.unwrap();

    // Lifecycle by name: keep-alive, suspend (pause), resume (connect).
    e.sbx
        .keep_alive("cua-e2e-e2b-a", Duration::from_secs(1800))
        .await
        .unwrap();
    let t = &routes(&e.api, "POST /sandboxes/")
        .into_iter()
        .find(|s| s.route.ends_with("/timeout"))
        .unwrap();
    assert_eq!(t.body["timeout"], 1800);
    e.sbx.suspend("cua-e2e-e2b-a").await.unwrap();
    assert_eq!(
        e.sbx.get("cua-e2e-e2b-a").await.unwrap().status,
        Status::Suspended
    );
    e.sbx.resume("cua-e2e-e2b-a").await.unwrap();
    assert_eq!(
        e.sbx.get("cua-e2e-e2b-a").await.unwrap().status,
        Status::Running
    );

    // Reattach from the state file, then delete: nothing left.
    let again = e.sbx.connect("cua-e2e-e2b-a").await.unwrap();
    again.delete().await.unwrap();
    assert!(e.api.sandboxes().is_empty());
    // Deleting a sandbox that is already gone is fine.
    assert!(e.sbx.connect("cua-e2e-e2b-a").await.is_err());
}

#[tokio::test]
async fn a_failed_build_is_a_typed_error_and_leaves_no_sandbox() {
    let e = env(Some(KEY)).await;
    e.api.fail_builds(true);
    let err = e.sbx.create(opts(common::IMAGE)).await.unwrap_err();
    assert!(matches!(err, Error::UnsupportedImage(_)), "{err:?}");
    assert!(err.to_string().contains("manifest unknown"), "{err}");
    assert!(e.api.sandboxes().is_empty());
    // The next create rebuilds under the same name.
    e.api.fail_builds(false);
    let sb = e.sbx.create(opts(common::IMAGE)).await.unwrap();
    assert_eq!(routes(&e.api, "POST /v3/templates").len(), 2);
    assert_eq!(e.api.templates().len(), 1);
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn credentials_errors_name_the_variable_and_never_the_key() {
    // No key: refused before any request.
    let e = env(None).await;
    let err = e.sbx.create(opts(common::IMAGE)).await.unwrap_err();
    assert!(matches!(err, Error::ContribNotConfigured(_)), "{err:?}");
    assert!(err.to_string().contains("E2B_API_KEY"), "{err}");
    assert!(e.api.seen().is_empty());

    // A wrong key: the API's 401, named, with no key material.
    let e = env(Some("e2b_wrong_key_value")).await;
    let err = e.sbx.create(opts(common::IMAGE)).await.unwrap_err();
    assert!(matches!(err, Error::ContribNotConfigured(_)), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("E2B_API_KEY") && msg.contains("401"), "{msg}");
    assert!(!msg.contains("e2b_wrong_key_value"), "{msg}");
}

#[tokio::test]
async fn vm_images_are_refused_before_any_request() {
    let e = env(Some(KEY)).await;
    let err = e
        .sbx
        .create(opts("ghcr.io/example/vm-only:1"))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::UnsupportedImage(_)), "{err:?}");
    assert!(e.api.seen().is_empty());
}

#[tokio::test]
async fn list_shows_only_cua_sandboxes() {
    let e = env(Some(KEY)).await;
    let sb = e
        .sbx
        .create(opts(common::IMAGE).name("cua-e2e-e2b-list"))
        .await
        .unwrap();
    let provider = e.sbx.contrib_provider("e2b").unwrap().clone();
    let listed = provider.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "cua-e2e-e2b-list");
    let list_req = routes(&e.api, "GET /v2/sandboxes");
    assert_eq!(list_req.len(), 1);
    sb.delete().await.unwrap();
}
