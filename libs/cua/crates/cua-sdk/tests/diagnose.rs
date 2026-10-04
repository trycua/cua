//! `SpacesdClient.diagnose` (SystemService.Diagnose through the SDK) against
//! the in-process mock spacesd: the report comes back as schema-v1 JSON, bad
//! options are a typed error, and a guest without a doctor says so. No host
//! effects.

use cua_sdk::{Cua, CuaConfig, CuaError};
use cua_spacesd_client::diagnose::{Check, Report, Status};
use cua_spacesd_client::testing::MockServer;

fn cua(dir: &tempfile::TempDir) -> std::sync::Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(dir.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dir.path().join("cua").display().to_string()),
        fleet_pool_home: Some(dir.path().join("pools").display().to_string()),
        fleet_from_env: false,
        fleet_from_session: false,
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnose_returns_the_report_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start(Default::default()).await;
    let mut report = Report {
        schema_version: 1,
        producer: "cua-spacesd".into(),
        checks: vec![Check::new("process.run", Status::Pass, "ok")],
        ..Report::default()
    };
    report.finalize(false, std::time::Duration::from_millis(5));
    server.state.set_diagnose_report(report.to_pb());

    let guest = cua(&dir).spacesd(server.url(), None).await.unwrap();
    let json = guest.diagnose("{\"strict\": true}".into()).await.unwrap();
    let back = Report::from_json(&json).unwrap();
    assert_eq!(back, report);
    // Empty options are the defaults.
    assert!(guest.diagnose(String::new()).await.is_ok());
    // Malformed options never reach the guest.
    let err = guest
        .diagnose("{\"strict\": \"yes\"}".into())
        .await
        .unwrap_err();
    assert!(matches!(err, CuaError::InvalidArgument(_)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_without_a_doctor_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start(Default::default()).await;
    let guest = cua(&dir).spacesd(server.url(), None).await.unwrap();
    let err = guest.diagnose("{}".into()).await.unwrap_err();
    assert!(err.to_string().contains("no diagnose report"), "{err}");
}
