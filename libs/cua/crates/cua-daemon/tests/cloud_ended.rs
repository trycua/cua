//! Cua Cloud has ended: the runtime never builds or uses a Fleet client,
//! whatever its config carries. A Fleet configured by base URL, token or an
//! injected client is ignored, a cloud create fails with the one message,
//! and nothing connects to the configured Fleet.

use cua_daemon::{CreateRequest, Error, Runtime, RuntimeConfig};
use cua_fleet::FleetConfig;
use cua_fleet::testing::FakeFleet;

#[tokio::test]
async fn a_configured_fleet_is_ignored_and_never_called() {
    let fleet = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    fleet.set_nonblocking(true).unwrap();
    let dirs = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        fleet: Some(FleetConfig {
            base_url: format!("http://{}", fleet.local_addr().unwrap()),
            fleet_token: Some("not-a-token".into()),
            ..Default::default()
        }),
        fleet_client: Some(fake.client()),
        ..Default::default()
    })
    .unwrap();

    // Creating in the cloud fails with the one message.
    for req in [
        CreateRequest {
            location: Some("cloud".into()),
            image: "linux".into(),
            ..Default::default()
        },
        CreateRequest {
            location: Some("cloud".into()),
            pool: Some("a-pool".into()),
            ..Default::default()
        },
    ] {
        let e = rt.create(req).await.unwrap_err();
        assert!(
            matches!(&e, Error::Unsupported(m) if m == cua_sandbox_core::CLOUD_ENDED),
            "{e:?}"
        );
    }

    // Listings never ask Fleet, and say nothing about it.
    let l = rt.list_filtered(None, true).await.unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert!(l.sandboxes.iter().all(|s| s.location != "cloud"));
    assert!(rt.adopt_managed_fleet_sandboxes().await.is_empty());

    assert!(
        matches!(fleet.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "something connected to the Fleet URL"
    );
}
