// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in lab: the pool coordinator with a real host job runner, in one
//! process, no account and no relay. A session's driver runs as a job in a
//! real Space on this machine's container engine, so the re-driver (epochs,
//! backoff, fencing) can be exercised end to end:
//!
//! - [`cua_spaces_cluster::coordinator::Coordinator`] with a provider node
//!   `lab` whose link is a [`cua_cluster::link::LocalLink`] to a
//!   [`cua_cluster::NodeJobs`] running [`cua_spaces::host_jobs::SpacesExecutor`];
//! - the coordinator's request surface (`session_*`, `submit`, `durable_*`,
//!   ...) over JSON lines, on stdin/stdout and, when
//!   `CUA_LAB_CONTROL_PORT` is set, on `127.0.0.1:<port>`.
//!
//! One request per line: `{"id": 1, "method": "session_start", "params": {...}}`;
//! one answer per line: `{"id": 1, "status": 200, "body": {...}}`. A campaign
//! harness starts this as a long-running process (`CUA_LAB_SERVE=1`) and
//! kills the driver's container (`docker rm -f <space>`) to test resumes.
//! The line `{"method": "quit"}` or the end of stdin stops it.
//!
//! ```text
//! # self-test: a driver that crashes at epoch 1 and finishes at epoch 2
//! CUA_HOME=/tmp/throwaway CUA_LAB_COORD_LIVE=1 \
//!   cargo test -p cua-spaces-cluster --test lab_coordinator_live -- --ignored --nocapture
//! # serve for a campaign
//! CUA_HOME=/tmp/throwaway CUA_LAB_COORD_LIVE=1 CUA_LAB_SERVE=1 CUA_LAB_CONTROL_PORT=47200 \
//!   cargo test -p cua-spaces-cluster --test lab_coordinator_live -- --ignored --nocapture
//! ```
//!
//! With `CUA_LAB_DIRECT_IP=<ip>` the lab node also provides Spaces the way a
//! `cua host setup --direct --provide-spaces` machine does: a real
//! [`cua_spaces::host_spaces::HostSpacesServer`] over the same runtime, served
//! as `cua.env.v1.HostSpacesService` over gRPC on `<ip>:<port>`, each Space it
//! creates forwarded on a port of `<ip>`. The coordinator reaches it with a
//! [`cua_cluster::link::GrpcLink`] (a node with a direct address), so it can
//! place a durable Space (`durable_create`) with a direct `ADDR` and `TOKEN`,
//! and a session with `spec.space` takes that Space's lease. Use an address the
//! job Spaces can reach (the Docker bridge, `172.17.0.1`); jobs still run
//! in-process through the same runtime.
//!
//! Needs Docker (runsc for gVisor), network for the image and a throwaway
//! `CUA_HOME` (never `~/.cua`). Image `CUA_JOB_SPACES_IMAGE` (default
//! `ghcr.io/trycua/linux:24.04-slim`).

#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_cluster::link::{GrpcLink, LinkError, LocalLink, NodeLink, Target};
use cua_cluster::node::{NodeConfig, NodeJobs};
use cua_daemon::{Runtime, RuntimeConfig, local::VmmLocal};
use cua_proto::env::v1 as pb;
use cua_proto::env::v1::host_spaces_service_server::{HostSpacesService, HostSpacesServiceServer};
use cua_spaces::host_spaces::{HostCaller, HostSpacesServer, to_status};
use cua_spaces_cluster::coordinator::requests::{Caller, answer};
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::coordinator::{Config, Coordinator, LinkFactory};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Answers one JSON line: `None` for `quit`.
async fn control(coord: &Coordinator, line: &str) -> Option<String> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(json!({"status": 400, "body": {"error": e.to_string()}}).to_string());
        }
    };
    let method = req["method"].as_str().unwrap_or_default();
    if method == "quit" {
        return None;
    }
    let (status, body) = answer(coord, method, req["params"].clone(), &Caller::local()).await;
    Some(json!({"id": req["id"], "status": status, "body": body}).to_string())
}

/// Serves `control` on stdin/stdout and on a loopback port.
async fn serve(coord: Arc<Coordinator>, port: Option<u16>) {
    let (quit_tx, mut quit_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    if let Some(port) = port {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("control port");
        eprintln!("lab control: 127.0.0.1:{port}");
        let (coord, quit_tx) = (coord.clone(), quit_tx.clone());
        tokio::spawn(async move {
            while let Ok((sock, _)) = l.accept().await {
                let (coord, quit_tx) = (coord.clone(), quit_tx.clone());
                tokio::spawn(async move {
                    let (r, mut w) = sock.into_split();
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        match control(&coord, &line).await {
                            Some(a) => {
                                let _ = w.write_all(format!("{a}\n").as_bytes()).await;
                            }
                            None => {
                                let _ = quit_tx.send(());
                                return;
                            }
                        }
                    }
                });
            }
        });
    }
    let stdin_coord = coord.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match control(&stdin_coord, &line).await {
                Some(a) => println!("{a}"),
                None => break,
            }
        }
        let _ = quit_tx.send(());
    });
    quit_rx.recv().await;
}

/// `HostSpacesService` over a real [`HostSpacesServer`], as the daemon serves it
/// (the calls its owner makes: no caller metadata is the local owner).
struct RealSpacesSvc(Arc<HostSpacesServer>);

fn caller<T>(req: &tonic::Request<T>) -> Result<HostCaller, tonic::Status> {
    HostCaller::from_grpc_metadata(req.metadata()).map_err(|e| to_status(&e))
}

#[tonic::async_trait]
impl HostSpacesService for RealSpacesSvc {
    async fn diagnose_host(
        &self,
        _req: tonic::Request<pb::DiagnoseHostRequest>,
    ) -> Result<tonic::Response<pb::DiagnoseHostResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("diagnose_host"))
    }

    async fn get_host_spaces(
        &self,
        req: tonic::Request<pb::GetHostSpacesRequest>,
    ) -> Result<tonic::Response<pb::GetHostSpacesResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .get(&who)
            .await
            .map(tonic::Response::new)
            .map_err(|e| to_status(&e))
    }

    async fn create_host_space(
        &self,
        req: tonic::Request<pb::CreateHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CreateHostSpaceResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .create(&who, req.into_inner())
            .await
            .map(|space| tonic::Response::new(pb::CreateHostSpaceResponse { space: Some(space) }))
            .map_err(|e| to_status(&e))
    }

    async fn cancel_host_space(
        &self,
        _req: tonic::Request<pb::CancelHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CancelHostSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not used by the lab"))
    }

    async fn set_host_space_egress(
        &self,
        _req: tonic::Request<pb::SetHostSpaceEgressRequest>,
    ) -> Result<tonic::Response<pb::SetHostSpaceEgressResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("fake"))
    }

    async fn delete_cloud_space(
        &self,
        _req: tonic::Request<pb::DeleteCloudSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteCloudSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not used by the lab"))
    }

    async fn delete_host_space(
        &self,
        req: tonic::Request<pb::DeleteHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteHostSpaceResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .delete(&who, &req.into_inner().space)
            .await
            .map(|message| tonic::Response::new(pb::DeleteHostSpaceResponse { message }))
            .map_err(|e| to_status(&e))
    }

    async fn set_host_warm(
        &self,
        _: tonic::Request<pb::SetHostWarmRequest>,
    ) -> Result<tonic::Response<pb::SetHostWarmResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("warm pools"))
    }

    async fn set_host_space_power(
        &self,
        _req: tonic::Request<pb::SetHostSpacePowerRequest>,
    ) -> Result<tonic::Response<pb::SetHostSpacePowerResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not used by the lab"))
    }
}

/// The lab node: jobs run in this process, Spaces are provided by the real
/// host service over gRPC.
struct LabLink {
    jobs: LocalLink,
    spaces: GrpcLink,
}

#[async_trait::async_trait]
impl NodeLink for LabLink {
    async fn run(&self, req: pb::RunHostJobsRequest) -> Result<pb::RunHostJobsResponse, LinkError> {
        self.jobs.run(req).await
    }
    async fn get(&self, job_id: &str) -> Result<pb::GetHostJobResponse, LinkError> {
        self.jobs.get(job_id).await
    }
    async fn get_output(
        &self,
        job_id: &str,
        from: u64,
    ) -> Result<pb::GetHostJobResponse, LinkError> {
        self.jobs.get_output(job_id, from).await
    }
    async fn list(
        &self,
        req: pb::ListHostJobsRequest,
    ) -> Result<pb::ListHostJobsResponse, LinkError> {
        self.jobs.list(req).await
    }
    async fn cancel(
        &self,
        req: pb::CancelHostJobRequest,
    ) -> Result<pb::CancelHostJobResponse, LinkError> {
        self.jobs.cancel(req).await
    }
    async fn host(&self) -> Result<pb::GetHostSpacesResponse, LinkError> {
        // The Spaces and settings are the real host's; the job capacity is the runner's.
        let mut h = self.spaces.host().await?;
        h.node = self.jobs.host().await?.node;
        Ok(h)
    }
    async fn create_space(
        &self,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace, LinkError> {
        self.spaces.create_space(req).await
    }
    async fn delete_space(&self, space: &str) -> Result<String, LinkError> {
        self.spaces.delete_space(space).await
    }
    fn describe(&self) -> String {
        format!("lab: jobs in process, {}", self.spaces.describe())
    }
}

/// Provides Spaces on `<ip>` like a direct host: its policy, the real
/// `HostSpacesServer` and the gRPC service. Returns the service's `ip:port`.
async fn provide_spaces(rt: &Runtime, home: &std::path::Path, ip: &str) -> String {
    let host = cua_host::Host::new(home);
    let policy = host.paths().policy();
    std::fs::create_dir_all(policy.parent().unwrap()).unwrap();
    std::fs::write(
        &policy,
        json!({
            "owner": "local", "trust_relay_allowlist": false, "sharing": true,
            "share_desktop": false, "provide_spaces": true, "max_spaces": 16,
            "direct": {"listen": format!("{ip}:0"), "allow_any_address": false},
        })
        .to_string(),
    )
    .unwrap();
    let server = HostSpacesServer::new(rt.spaces().clone());
    // Spaces an earlier lab process created keep their ports (the containers outlive it).
    server.restore_direct_forwards().await;
    let listener = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(HostSpacesServiceServer::new(RealSpacesSvc(server)))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await;
    });
    addr
}

/// The coordinator over one provider node backed by the real job runner.
async fn lab(home: &std::path::Path) -> Arc<Coordinator> {
    let vmm = Arc::new(VmmLocal::default());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sandboxes")),
        vmm: Some(vmm),
        spaces_home: Some(home.to_path_buf()),
        ..Default::default()
    })
    .unwrap();
    let exec = cua_spaces::host_jobs::SpacesExecutor::local(rt.spaces().clone()).await;
    let jobs = NodeJobs::open(
        NodeConfig {
            dir: home.join("host/jobs"),
            millicpus_total: 4000,
            memory_total_mb: 8192,
            jobs_max: 0,
            min_available_mb: 0,
            retention: Duration::from_secs(3600),
            output_retention: Duration::from_secs(3600),
            boot_id: format!("lab-{}", std::process::id()),
        },
        Arc::new(exec),
    )
    .unwrap();
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let jobs_link = LocalLink::new(jobs, "lab", os);
    // With CUA_LAB_DIRECT_IP the node also hosts Spaces (a direct address, a real host service).
    let direct = match std::env::var("CUA_LAB_DIRECT_IP") {
        Ok(ip) if !ip.is_empty() => Some(provide_spaces(&rt, home, &ip).await),
        _ => None,
    };
    let link: Arc<dyn NodeLink> = match &direct {
        Some(addr) => Arc::new(LabLink {
            jobs: jobs_link,
            spaces: GrpcLink::new(Target::Direct(addr.clone()), "lab").without_probe(),
        }),
        None => Arc::new(jobs_link),
    };
    let factory: LinkFactory =
        Arc::new(move |row: &NodeRow| (row.via == "provider").then(|| link.clone()));
    let mut cfg = Config::new(home.join("coordinator"));
    cfg.poll_wait = Duration::from_millis(500);
    cfg.session_tick = Duration::from_millis(500);
    let coord = Coordinator::start(cfg, Some(factory)).await.unwrap();
    coord
        .add_node(NodeRow {
            name: "lab".into(),
            via: "provider".into(),
            provider: "lab".into(),
            direct: direct.clone().unwrap_or_default(),
            state: "ready".into(),
            os: os.into(),
            max_jobs: 4,
            ..Default::default()
        })
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !coord.nodes().iter().any(|n| n.health == "ready") {
        assert!(Instant::now() < deadline, "the lab node never became ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    coord
}

async fn session(coord: &Coordinator, name: &str) -> Value {
    let (st, v) = answer(
        coord,
        "session_get",
        json!({"name": name}),
        &Caller::local(),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs real Spaces on this machine's container engine; set CUA_LAB_COORD_LIVE=1 and a throwaway CUA_HOME"]
async fn a_session_driver_is_resumed_in_real_spaces() {
    if std::env::var("CUA_LAB_COORD_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_LAB_COORD_LIVE=1");
        return;
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home"),
    );
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    let coord = lab(&home).await;

    if std::env::var("CUA_LAB_SERVE").as_deref() == Ok("1") {
        let port = std::env::var("CUA_LAB_CONTROL_PORT")
            .ok()
            .and_then(|p| p.parse().ok());
        serve(coord.clone(), port).await;
        coord.stop();
        return;
    }

    let image = std::env::var("CUA_JOB_SPACES_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    let me = Caller::local();
    // Crashes at epoch 1, finishes at epoch 2.
    let (st, v) = answer(
        &coord,
        "session_start",
        json!({"name": "lab", "max_attempts": 3, "spec": {
            "argv": ["sh", "-c",
                "echo epoch=$CUA_SESSION_EPOCH resume=$CUA_SESSION_RESUME; [ \"$CUA_SESSION_EPOCH\" -ge 2 ]"],
            "image": image, "kind": "container",
        }}),
        &me,
    )
    .await;
    assert_eq!(st, 200, "{v}");
    let deadline = Instant::now() + Duration::from_secs(900);
    let v = loop {
        let v = session(&coord, "lab").await;
        if v["state"] == "done" || v["state"] == "failed" {
            break v;
        }
        assert!(Instant::now() < deadline, "not done: {v}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    eprintln!("{}", serde_json::to_string_pretty(&v).unwrap());
    assert_eq!(v["state"], "done", "{v}");
    assert_eq!(
        (v["epoch"].clone(), v["attempts"].clone()),
        (json!(2), json!(2))
    );
    let (st, o) = answer(&coord, "session_output", json!({"name": "lab"}), &me).await;
    assert_eq!(st, 200, "{o}");
    let text = o["text"].as_str().unwrap_or_default();
    assert!(text.contains("epoch=2 resume=1"), "{o}");

    // A driver that writes a line and then goes silent (a frozen process
    // writes nothing): stall detection cancels it and the next epoch finishes.
    let (st, v) = answer(
        &coord,
        "session_start",
        json!({"name": "lab-stall", "max_attempts": 3, "spec": {
            "argv": ["sh", "-c",
                "echo epoch=$CUA_SESSION_EPOCH hb=$CUA_SESSION_HEARTBEAT_SECS; [ \"$CUA_SESSION_EPOCH\" -ge 2 ] || sleep 3600"],
            "image": image, "kind": "container", "stall_secs": 8,
        }}),
        &me,
    )
    .await;
    assert_eq!(st, 200, "{v}");
    let deadline = Instant::now() + Duration::from_secs(900);
    let v = loop {
        let v = session(&coord, "lab-stall").await;
        if v["state"] == "done" || v["state"] == "failed" {
            break v;
        }
        assert!(Instant::now() < deadline, "not done: {v}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert_eq!(v["state"], "done", "{v}");
    let events: Vec<&str> = v["history"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| h["event"].as_str())
        .collect();
    assert_eq!(events, ["started", "stalled", "started", "done"], "{v}");
    coord.stop();
}
