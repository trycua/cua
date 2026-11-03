// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The `tunnel`, `hotspot` and `teleport` groups against an in-process
//! cua-spacesd core. Downloads and the teleport home are temp directories;
//! the hotspot and every listener bind loopback only; no bundle is ever
//! imported into a real profile (the server's host effects are refused via
//! `CUA_ENV_TEST_SANDBOX=1`, and the checks never send an importable one).

use std::time::Duration;

use cua_spacesd_client::diagnose::Status;
use cua_spacesd_client::{pb, SpacesdClient};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};

const TOKEN: &str = "doctor-test-token";

struct Server {
    url: String,
    dir: tempfile::TempDir,
}

async fn start() -> Server {
    std::env::set_var("CUA_ENV_TEST_SANDBOX", "1");
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
    let addr = cua_spacesd_server::spawn_local(ServerBuilder::new(ctx).build())
        .await
        .unwrap();
    Server {
        url: format!("http://{addr}"),
        dir,
    }
}

async fn client(server: &Server) -> SpacesdClient {
    SpacesdClient::connect_url(&server.url, Some(TOKEN.into()))
        .await
        .unwrap()
}

fn teleport_apps(caps: &pb::GetCapabilitiesResponse) -> Vec<String> {
    caps.features
        .iter()
        .filter(|f| f.supported)
        .filter_map(|f| f.name.strip_prefix("teleport.").map(str::to_owned))
        .filter(|name| name != "wipe")
        .collect()
}

#[tokio::test]
async fn tunnel_lifecycle_and_tickets_pass() {
    let server = start().await;
    let client = client(&server).await;
    let c = cua_spacesd_doctor::checks::tunnel::forward_lifecycle(&client).await;
    assert_eq!(c.status, Status::Pass, "{}", c.message);
    assert_eq!(
        c.facts.get("foreign_host_refused").map(String::as_str),
        Some("true")
    );
    let c = cua_spacesd_doctor::checks::tunnel::forged_tickets(&client, true).await;
    assert_eq!(c.status, Status::Pass, "{}", c.message);
    // Nothing left open.
    let listed = client
        .tunnel()
        .list_forwards(pb::ListForwardsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(listed.forwards.is_empty(), "{listed:?}");
}

#[tokio::test]
async fn hotspot_egress_round_trips_and_stops() {
    let server = start().await;
    let client = client(&server).await;
    let c = cua_spacesd_doctor::checks::tunnel::hotspot_status(&client).await;
    assert_eq!(c.status, Status::Pass, "{}", c.message);
    assert_eq!(c.facts.get("state").map(String::as_str), Some("stopped"));
    let c = tokio::time::timeout(
        Duration::from_secs(60),
        cua_spacesd_doctor::checks::tunnel::hotspot_egress(&client, "t1"),
    )
    .await
    .unwrap();
    assert_eq!(c.status, Status::Pass, "{}", c.message);
    // The peer dialed the echo once and refused the foreign destination.
    assert_eq!(c.facts.get("peer_dials").map(String::as_str), Some("1"));
    assert_eq!(c.facts.get("peer_refused").map(String::as_str), Some("1"));
    let after = client
        .tunnel()
        .get_hotspot_status(pb::GetHotspotStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(after.state, pb::HotspotState::Stopped as i32);
}

#[tokio::test]
async fn hotspot_egress_never_replaces_a_live_hotspot() {
    let server = start().await;
    let client = client(&server).await;
    let started = client
        .tunnel()
        .start_hotspot(pb::StartHotspotRequest {
            socks_port: {
                let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                l.local_addr().unwrap().port() as u32
            },
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let c = cua_spacesd_doctor::checks::tunnel::hotspot_egress(&client, "t2").await;
    assert_eq!(c.status, Status::Skip, "{}", c.message);
    assert_eq!(
        c.facts.get("skip_reason").map(String::as_str),
        Some("hotspot_in_use")
    );
    let status = client
        .tunnel()
        .get_hotspot_status(pb::GetHotspotStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        status.hotspot_id, started.hotspot_id,
        "the live hotspot survived"
    );
    client
        .tunnel()
        .stop_hotspot(pb::StopHotspotRequest {
            hotspot_id: started.hotspot_id,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn teleport_manifest_and_import_verification_pass_without_importing() {
    let server = start().await;
    let client = client(&server).await;
    let caps = client.refresh_capabilities().await.unwrap();
    let apps = teleport_apps(&caps);
    assert!(!apps.is_empty(), "the core advertises teleport providers");
    let c = cua_spacesd_doctor::checks::teleport::manifest(&client, &apps).await;
    assert_eq!(c.status, Status::Pass, "{}", c.message);
    let c = cua_spacesd_doctor::checks::teleport::import_verify(&client, &apps[0], "t3").await;
    assert_eq!(c.status, Status::Pass, "{}", c.message);
    // Nothing was imported: the teleport home is untouched and the staging
    // area holds no bundle.
    let home = server.dir.path().join("home");
    let written = std::fs::read_dir(&home).map(|d| d.count()).unwrap_or(0);
    assert_eq!(written, 0, "the teleport home was written");
    let staging = server.dir.path().join("data/teleport");
    let staged = std::fs::read_dir(&staging).map(|d| d.count()).unwrap_or(0);
    assert_eq!(staged, 0, "a staged bundle was left behind");
    // A mismatched manifest set is reported.
    let c = cua_spacesd_doctor::checks::teleport::manifest(&client, &["nope".into()]).await;
    assert_eq!(c.status, Status::Fail, "{}", c.message);
}

#[test]
fn teleport_fixture_import_lands_in_the_throwaway_home_only() {
    let home = tempfile::tempdir().unwrap();
    let c = cua_spacesd_doctor::checks::teleport::import_fixture(home.path());
    assert_eq!(c.status, Status::Pass, "{}", c.message);
}

#[tokio::test]
async fn teleport_receive_files_round_trips_and_cleans_up() {
    let server = start().await;
    let client = client(&server).await;
    let c = tokio::time::timeout(
        Duration::from_secs(60),
        cua_spacesd_doctor::checks::teleport::receive_files(&client, "t4"),
    )
    .await
    .unwrap();
    assert_eq!(c.status, Status::Pass, "{}", c.message);
    let downloads = server.dir.path().join("downloads");
    let left: Vec<_> = std::fs::read_dir(&downloads)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "left in Downloads: {left:?}");
}

#[tokio::test]
async fn full_run_gates_the_effectful_checks() {
    let server = start().await;
    let options = cua_spacesd_doctor::Options {
        manifest_path: Some(server.dir.path().join("none.json")),
        timeout: Some(Duration::from_secs(120)),
        only: vec!["tunnel".into(), "hotspot".into(), "teleport".into()],
        ..Default::default()
    };
    let report = cua_spacesd_doctor::diagnose(&server.url, Some(TOKEN.into()), options, None).await;
    let get = |id: &str| {
        report
            .checks
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no {id}: {}", report.to_human()))
    };
    for id in [
        "tunnel.lifecycle",
        "tunnel.tickets",
        "hotspot.status",
        "teleport.manifest",
        "teleport.import.verify",
    ] {
        assert_eq!(get(id).status, Status::Pass, "{id}: {}", get(id).message);
    }
    // Effects are off by default: the effectful checks skip with the reason.
    for id in ["hotspot.egress", "teleport.receive_files"] {
        let c = get(id);
        assert_eq!(c.status, Status::Skip, "{id}: {}", c.message);
        assert_eq!(
            c.facts.get("skip_reason").map(String::as_str),
            Some("effects_disabled"),
            "{id}"
        );
    }
    // Only the selected groups ran.
    assert!(report
        .checks
        .iter()
        .all(|c| matches!(c.group.as_str(), "tunnel" | "hotspot" | "teleport")));
}
