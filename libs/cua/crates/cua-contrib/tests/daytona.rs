//! Daytona through the core against the schema mock of its API
//! (`cua_contrib::testing::MockDaytona`, fields from Daytona's published
//! OpenAPI spec; not recorded traffic). Hermetic: loopback only.

mod common;

use cua_contrib::{
    common::Secret,
    daytona::{Daytona, DaytonaConfig},
    testing::MockDaytona,
};
use cua_sandbox_core::{CreateOptions, Error, ProviderKind, Sandboxes, Status};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::{sync::Arc, time::Duration};

const KEY: &str = "dtn_test_key_not_real";

struct Env {
    _dir: tempfile::TempDir,
    api: MockDaytona,
    _spacesd: MockServer,
    sbx: Sandboxes,
}

async fn env(key: Option<&str>) -> Env {
    common::registry();
    let dir = tempfile::tempdir().unwrap();
    let api = MockDaytona::start(KEY).await;
    let spacesd = MockServer::start(MockAuth::default()).await;
    api.route_port(3211, &spacesd.url());
    let provider = Daytona::new(DaytonaConfig {
        api_key: key.map(Secret::new),
        api_url: api.url(),
        target: None,
        poll: Duration::from_millis(10),
    });
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
    o.contrib = Some("daytona".into());
    o.ready_timeout = Duration::from_secs(30);
    o
}

fn bodies(api: &MockDaytona, route: &str) -> Vec<serde_json::Value> {
    api.seen()
        .into_iter()
        .filter(|s| s.route == route)
        .map(|s| s.body)
        .collect()
}

#[tokio::test]
async fn snapshots_once_per_digest_and_runs_public_sandboxes_with_the_token() {
    let e = env(Some(KEY)).await;
    let sb = e
        .sbx
        .create(
            opts(common::IMAGE)
                .name("cua-e2e-daytona-a")
                .service("novnc", 6080),
        )
        .await
        .unwrap();
    assert_eq!(sb.id(), "daytona:cua-e2e-daytona-a");

    let snaps = bodies(&e.api, "POST /snapshots");
    assert_eq!(snaps.len(), 1);
    let image = snaps[0]["imageName"].as_str().unwrap();
    assert!(image.starts_with("ghcr.io/trycua/linux@sha256:"), "{image}");
    assert_eq!(snaps[0]["cpu"], 2);
    assert_eq!(snaps[0]["memory"], 4);
    assert!(snaps[0].get("entrypoint").is_none());

    let created = &bodies(&e.api, "POST /sandbox")[0];
    assert_eq!(created["public"], true);
    assert_eq!(created["ttlMinutes"], 60);
    assert_eq!(created["labels"]["cua.name"], "cua-e2e-daytona-a");
    // Daytona runs the image's entrypoint with the sandbox environment:
    // the spacesd token is delivered at create.
    assert_eq!(
        created["env"]["CUA_ENV_TOKEN"].as_str().map(str::len),
        Some(32)
    );
    assert_eq!(created["snapshot"], snaps[0]["name"]);

    sb.spacesd().await.unwrap().health().await.unwrap();
    // Other declared ports use Daytona's preview URLs.
    assert_eq!(
        sb.service("novnc").unwrap().endpoint().await.unwrap().url,
        format!(
            "https://6080-{}.proxy.daytona.works",
            sb.provider_details()["id"]
        )
    );

    // Same image: the snapshot is reused.
    let eph = e.sbx.create(opts(common::IMAGE)).await.unwrap();
    assert_eq!(bodies(&e.api, "POST /snapshots").len(), 1);
    eph.delete().await.unwrap();
    // A command replaces the entrypoint: a different snapshot.
    let cmd = e
        .sbx
        .create(opts(common::IMAGE).command(["sleep", "infinity"]))
        .await
        .unwrap();
    let snaps = bodies(&e.api, "POST /snapshots");
    assert_eq!(snaps.len(), 2);
    assert_eq!(
        snaps[1]["entrypoint"],
        serde_json::json!(["sleep", "infinity"])
    );
    cmd.delete().await.unwrap();

    // Lifecycle by name.
    e.sbx
        .keep_alive("cua-e2e-daytona-a", Duration::from_secs(7200))
        .await
        .unwrap();
    assert!(e.api.seen().iter().any(|s| s.route.ends_with("/ttl/120")));
    e.sbx.suspend("cua-e2e-daytona-a").await.unwrap();
    assert_eq!(
        e.sbx.get("cua-e2e-daytona-a").await.unwrap().status,
        Status::Stopped
    );
    e.sbx.resume("cua-e2e-daytona-a").await.unwrap();
    let again = e.sbx.connect("cua-e2e-daytona-a").await.unwrap();
    again.spacesd().await.unwrap().health().await.unwrap();
    again.delete().await.unwrap();
    assert!(e.api.sandboxes().is_empty());
}

#[tokio::test]
async fn failed_snapshots_are_typed_and_rebuilt() {
    let e = env(Some(KEY)).await;
    e.api.fail_snapshots(true);
    let err = e.sbx.create(opts(common::IMAGE)).await.unwrap_err();
    assert!(matches!(err, Error::UnsupportedImage(_)), "{err:?}");
    assert!(err.to_string().contains("Failed to pull image"), "{err}");
    assert!(e.api.sandboxes().is_empty());
    // The failed snapshot is deleted and built again.
    e.api.fail_snapshots(false);
    let sb = e.sbx.create(opts(common::IMAGE)).await.unwrap();
    assert!(
        e.api
            .seen()
            .iter()
            .any(|s| s.route.starts_with("DELETE /snapshots/"))
    );
    assert_eq!(e.api.snapshots().len(), 1);
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn credentials_errors_name_the_variable_and_never_the_key() {
    let e = env(None).await;
    let err = e.sbx.create(opts(common::IMAGE)).await.unwrap_err();
    assert!(err.to_string().contains("DAYTONA_API_KEY"), "{err}");
    assert!(e.api.seen().is_empty());

    let e = env(Some("dtn_wrong_value")).await;
    let err = e.sbx.create(opts(common::IMAGE)).await.unwrap_err();
    assert!(matches!(err, Error::ContribNotConfigured(_)), "{err:?}");
    assert!(!err.to_string().contains("dtn_wrong_value"));
}

#[tokio::test]
async fn private_registries_are_refused_typed() {
    let e = env(Some(KEY)).await;
    let mut o = opts(common::IMAGE);
    o.registry_credentials = Some(cua_sandbox_core::RegistryCredentials {
        registry: Some("ghcr.io".into()),
        username: "u".into(),
        password: "p".into(),
    });
    let err = e.sbx.create(o).await.unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err:?}");
    assert!(e.api.seen().is_empty());
}
