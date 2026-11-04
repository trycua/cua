// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The doctor end to end against an in-process cua-spacesd core (no desktop
//! provider, no fixtures, effects off): the report is produced, the core
//! groups pass, and `SystemService.Diagnose` / `DiagnoseOnce` stream and
//! return the same shape. Nothing here touches the host desktop: the server
//! has no desktop services, and effectful checks are disabled.

use std::sync::Arc;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Report, Severity, Status};
use cua_spacesd_client::{pb, SpacesdClient};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};

const TOKEN: &str = "doctor-test-token";

async fn start(diagnoser: bool) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().join("data"),
        downloads_dir: Some(dir.path().join("downloads")),
        teleport_home: Some(dir.path().join("home")),
        media_quic_port: 0,
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, Some(TOKEN.into()));
    let mut builder = ServerBuilder::new(ctx);
    if diagnoser {
        builder = builder.diagnoser(Arc::new(cua_spacesd_doctor::ServerDiagnoser {
            manifest_path: Some(dir.path().join("no-manifest.json")),
            artifacts_dir: None,
        }));
    }
    let addr = cua_spacesd_server::spawn_local(builder.build())
        .await
        .unwrap();
    (format!("http://{addr}"), dir)
}

fn check<'a>(report: &'a Report, id: &str) -> &'a cua_spacesd_client::diagnose::Check {
    report
        .checks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| {
            panic!(
                "no check {id} in {:#?}",
                report.checks.iter().map(|c| &c.id).collect::<Vec<_>>()
            )
        })
}

#[tokio::test]
async fn core_groups_pass_against_an_in_process_server() {
    let (url, dir) = start(false).await;
    let options = cua_spacesd_doctor::Options {
        manifest_path: Some(dir.path().join("none.json")),
        timeout: Some(Duration::from_secs(120)),
        // `build` and `driver` probe the host's own cua-driver (the one on
        // PATH, run with the real HOME), and `auth.token_file` stats the real
        // ~/.cua/spacesd/token. None of them is about the in-process core, and
        // plain `cargo test` must not reach the developer's installation.
        skip: vec![
            "stream.encode".into(),
            "build".into(),
            "driver".into(),
            "auth.token_file".into(),
        ],
        ..Default::default()
    };
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<pb::DiagnoseResponse>(256);
    let collector = tokio::spawn(async move {
        let mut started = 0usize;
        let mut finished = 0usize;
        while let Some(event) = receiver.recv().await {
            match event.event {
                Some(pb::diagnose_response::Event::Started(_)) => started += 1,
                Some(pb::diagnose_response::Event::Check(_)) => finished += 1,
                _ => {}
            }
        }
        (started, finished)
    });
    let report =
        cua_spacesd_doctor::diagnose(&url, Some(TOKEN.into()), options, Some(sender)).await;
    let (_, finished) = collector.await.unwrap();
    assert_eq!(finished, report.checks.len(), "every check is streamed");

    for id in [
        "meta.spacesd.reachable",
        "meta.protocol_revision",
        "meta.health",
        "capabilities.limitations",
        "process.run",
        "process.exit_code",
        "files.roundtrip",
        "files.ops",
        "files.signed_url",
        "files.chunk_limit",
        "network.tunnel.forward",
        "auth.unauthenticated",
    ] {
        let c = check(&report, id);
        assert_eq!(c.status, Status::Pass, "{id}: {}", c.message);
    }
    #[cfg(unix)]
    for id in ["process.stdin", "process.pty", "process.signal"] {
        let c = check(&report, id);
        assert_eq!(c.status, Status::Pass, "{id}: {}", c.message);
    }
    // Core checks are required even without a manifest.
    assert_eq!(check(&report, "process.run").severity, Severity::Required);
    // No manifest and no desktop provider: the desktop checks are
    // informational (a skip on a headless runner, a downgraded warning where
    // the OS reports a display the in-process server cannot capture).
    let shot = check(&report, "screenshot.display");
    assert_eq!(shot.severity, Severity::Info);
    assert!(
        matches!(shot.status, Status::Skip | Status::Warn),
        "{shot:?}"
    );
    // Selection: the skipped groups never ran.
    for skipped in ["stream.encode.", "build.", "driver.", "auth.token_file"] {
        assert!(
            !report.checks.iter().any(|c| c.id.starts_with(skipped)),
            "{skipped} was skipped but ran"
        );
    }
    // A wrong token is refused before any check runs.
    assert_eq!(
        report.spacesd.as_ref().unwrap().protocol_revision,
        cua_proto::ENV_PROTOCOL_REVISION
    );
    assert_eq!(report.schema_version, 1);
    assert!(!report.fidelity.runtime.is_empty());
    assert_ne!(report.summary.status, Status::Fail, "{}", report.to_human());
    // The JSON form round-trips.
    assert_eq!(Report::from_json(&report.to_json()).unwrap(), report);
}

#[tokio::test]
async fn unreachable_targets_fail_with_one_required_check() {
    let report =
        cua_spacesd_doctor::diagnose("http://127.0.0.1:9", None, Default::default(), None).await;
    assert_eq!(report.summary.status, Status::Fail);
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.checks[0].id, "meta.spacesd.reachable");
    assert_eq!(report.exit_code(), 1);
}

#[tokio::test]
async fn diagnose_rpc_streams_checks_then_the_report() {
    let (url, _dir) = start(true).await;
    let client = SpacesdClient::connect_url(&url, Some(TOKEN.into()))
        .await
        .unwrap();
    let options = pb::DiagnoseOptions {
        only: vec!["meta".into(), "process.run".into()],
        timeout: Some(pbjson_types::Duration {
            seconds: 60,
            nanos: 0,
        }),
        ..Default::default()
    };
    let mut seen = Vec::new();
    let report = client
        .diagnose_report(options.clone(), |c| seen.push(c.id.clone()))
        .await
        .unwrap();
    assert!(seen.contains(&"process.run".to_owned()), "{seen:?}");
    assert!(report
        .checks
        .iter()
        .all(|c| c.group == "meta" || c.id == "process.run"));
    assert_eq!(check(&report, "process.run").status, Status::Pass);
    // Unary fallback: same checks.
    let once = client.diagnose_once(options).await.unwrap();
    let ids = |r: &Report| r.checks.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&once), ids(&report));

    // Diagnose requires the token.
    let anonymous = SpacesdClient::connect(
        cua_spacesd_client::ConnectOptions::parse(&url)
            .unwrap()
            .probe(false),
    )
    .await
    .unwrap();
    let error = anonymous
        .diagnose_once(Default::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(tonic::Code::Unauthenticated), "{error}");
}

#[tokio::test]
async fn diagnose_without_a_doctor_is_a_clear_precondition_failure() {
    let (url, _dir) = start(false).await;
    let client = SpacesdClient::connect_url(&url, Some(TOKEN.into()))
        .await
        .unwrap();
    let error = client.diagnose_once(Default::default()).await.unwrap_err();
    assert_eq!(
        error.code(),
        Some(tonic::Code::FailedPrecondition),
        "{error}"
    );
    assert!(error.to_string().contains("no doctor linked"), "{error}");
}
