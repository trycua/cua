// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A coordinator's link to a host over the relay's mesh (`Target::Mesh`):
//! the host's job service behind a mesh machine node, a stand-in
//! coordinator, pairing by the env token, and a failover to the next way
//! when a relay has no mesh.

mod common;

use std::sync::Arc;
use std::time::Duration;

use cua_cluster::link::{FailoverLink, GrpcLink, JobsService, NodeLink, Target};
use cua_cluster::{NodeConfig, NodeJobs};
use cua_mesh::testing::FakeCoordinator;
use cua_mesh::{Mesh, MeshConfig, Mode, NodeKind};
use cua_proto::env::v1 as pb;

async fn host_jobs(dir: &std::path::Path) -> std::net::SocketAddr {
    let cfg = NodeConfig {
        dir: dir.to_path_buf(),
        millicpus_total: 4000,
        memory_total_mb: 1 << 16,
        jobs_max: 0,
        min_available_mb: 0,
        retention: Duration::from_secs(3600),
        output_retention: Duration::from_secs(3600),
        boot_id: "boot".into(),
    };
    let jobs = NodeJobs::open(cfg, Arc::new(common::FakeExec::new())).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(pb::host_jobs_service_server::HostJobsServiceServer::new(
                JobsService(jobs),
            ))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(l)),
    );
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_coordinator_reaches_its_host_over_the_mesh() {
    let dir = tempfile::tempdir().unwrap();
    let local = host_jobs(dir.path()).await;
    let coord = FakeCoordinator::start(Mode::Static).await;

    // The host's mesh node: accepts the env token "env-t", splices spacesd
    // streams to its (job) service.
    let mut cfg = MeshConfig::new(
        coord.url.clone(),
        NodeKind::Machine,
        Some(dir.path().join("host.json")),
        cua_mesh::static_auth(vec![("x-cua-machine-id".into(), "host-0001".into())]),
    );
    cfg.approved_path = Some(dir.path().join("approved.json"));
    cfg.pair_verifier = Some(Arc::new(|client, ts, proof| {
        cua_mesh::proto::verify_pair_proof(
            "env-t",
            "host-0001",
            client,
            ts,
            proof,
            cua_mesh::proto::now_secs(),
        )
    }));
    let host = Mesh::start(cfg).await.unwrap();
    host.wait_ready(Duration::from_secs(10)).await.unwrap();
    let serving = host.clone();
    tokio::spawn(async move {
        while let Some(i) = serving.accept().await {
            tokio::spawn(async move {
                let mut s = i.stream;
                if let Ok(mut tcp) = tokio::net::TcpStream::connect(local).await {
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut tcp).await;
                }
            });
        }
    });

    let link = GrpcLink::new(
        Target::Mesh {
            url: coord.url.clone(),
            machine: "host-0001".into(),
        },
        "env-t",
    )
    .without_probe();
    let listed = link
        .list(pb::ListHostJobsRequest::default())
        .await
        .expect("jobs over the mesh");
    assert!(listed.jobs.is_empty());
    let accepted = link
        .run(pb::RunHostJobsRequest {
            jobs: vec![common::spec("j1", 1, &["echo", "over the mesh"])],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(accepted.acks[0].outcome, "accepted");
    assert!(link.describe().starts_with("mesh "));

    // (A wrong env token never pairs: see cua-mesh's pairing tests; and
    // spacesd checks the token on every call regardless of the path.)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_without_a_mesh_fails_over_to_the_next_way() {
    let dir = tempfile::tempdir().unwrap();
    let local = host_jobs(dir.path()).await;
    // Nothing answers the mesh here (a closed port): the failover link
    // moves to the next way, as from the mesh to the relay's HTTPS path.
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_url = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    let link = FailoverLink::new(vec![
        Arc::new(
            GrpcLink::new(
                Target::Mesh {
                    url: dead_url,
                    machine: "host-0001".into(),
                },
                "t",
            )
            .without_probe(),
        ),
        Arc::new(GrpcLink::new(Target::Direct(local.to_string()), "t").without_probe()),
    ]);
    link.list(pb::ListHostJobsRequest::default())
        .await
        .expect("the next way answers");
}
