// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Fake hosts over real gRPC on loopback, shared by the cluster tests: each
//! host is a real job runner in its own tokio runtime (killing the runtime is
//! killing the daemon), behind a TCP gate (closing it is a partition).

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::FakeExec;
use cua_cluster::link::{GrpcLink, JobsService, NodeLink, Target};
use cua_cluster::node::{NodeConfig, NodeJobs};
use cua_proto::env::v1 as pb;
use cua_proto::env::v1::host_jobs_service_server::HostJobsServiceServer;
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::coordinator::{Config, LinkFactory};

/// partition) until opened again.
pub struct Gate {
    pub port: u16,
    pub upstream: Arc<Mutex<u16>>,
    pub open: Arc<AtomicBool>,
    pub conns: Arc<Mutex<Vec<tokio::task::AbortHandle>>>,
}

impl Gate {
    pub async fn start() -> Self {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let upstream = Arc::new(Mutex::new(0u16));
        let open = Arc::new(AtomicBool::new(false));
        let conns: Arc<Mutex<Vec<tokio::task::AbortHandle>>> = Arc::default();
        let (u, o, c) = (upstream.clone(), open.clone(), conns.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut inbound, _)) = l.accept().await else {
                    continue;
                };
                if !o.load(Ordering::SeqCst) {
                    continue;
                }
                let up = *u.lock().unwrap();
                let task = tokio::spawn(async move {
                    if let Ok(mut out) = tokio::net::TcpStream::connect(("127.0.0.1", up)).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut out).await;
                    }
                });
                let mut g = c.lock().unwrap();
                g.retain(|h| !h.is_finished());
                g.push(task.abort_handle());
            }
        });
        Self {
            port,
            upstream,
            open,
            conns,
        }
    }

    pub fn open(&self, upstream: u16) {
        *self.upstream.lock().unwrap() = upstream;
        self.open.store(true, Ordering::SeqCst);
    }

    pub fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
        for h in self.conns.lock().unwrap().drain(..) {
            h.abort();
        }
    }
}

/// The interactive Spaces a fake host provides (they outlive its daemon,
/// as a VM or container does).
#[derive(Clone, Default)]
pub struct FakeSpaces(pub Arc<Mutex<FakeSpacesState>>);

#[derive(Default)]
pub struct FakeSpacesState {
    pub spaces: Vec<pb::HostSpace>,
    pub next_port: u32,
    pub deleted: Vec<String>,
}

pub struct FakeSpacesSvc {
    pub spaces: FakeSpaces,
    pub jobs: Arc<NodeJobs>,
    pub name: String,
}

#[tonic::async_trait]
impl pb::host_spaces_service_server::HostSpacesService for FakeSpacesSvc {
    async fn diagnose_host(
        &self,
        _req: tonic::Request<pb::DiagnoseHostRequest>,
    ) -> Result<tonic::Response<pb::DiagnoseHostResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("diagnose_host"))
    }

    async fn get_host_spaces(
        &self,
        _req: tonic::Request<pb::GetHostSpacesRequest>,
    ) -> Result<tonic::Response<pb::GetHostSpacesResponse>, tonic::Status> {
        Ok(tonic::Response::new(pb::GetHostSpacesResponse {
            name: self.name.clone(),
            os: "linux".into(),
            spaces: self.spaces.0.lock().unwrap().spaces.clone(),
            node: Some(self.jobs.status()),
            ..Default::default()
        }))
    }

    async fn create_host_space(
        &self,
        req: tonic::Request<pb::CreateHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CreateHostSpaceResponse>, tonic::Status> {
        let r = req.into_inner();
        let mut g = self.spaces.0.lock().unwrap();
        g.next_port += 1;
        // A direct Space gets a port on the host's address; another joins
        // the relay as a machine of its own.
        let space = if r.direct {
            pb::HostSpace {
                local_space: format!("local:{}", r.name),
                name: r.name.clone(),
                image: r.image.clone(),
                kind: "container".into(),
                direct_port: 40000 + g.next_port,
                direct_token: format!("tok-{}-{}", self.name, g.next_port),
                network: r.network.clone(),
                ..Default::default()
            }
        } else {
            pb::HostSpace {
                relay_machine: format!("space-{}-{}", self.name, g.next_port),
                local_space: format!("local:{}", r.name),
                name: r.name.clone(),
                image: r.image.clone(),
                kind: "container".into(),
                network: r.network.clone(),
                ..Default::default()
            }
        };
        g.spaces.push(space.clone());
        Ok(tonic::Response::new(pb::CreateHostSpaceResponse {
            space: Some(space),
        }))
    }

    async fn delete_host_space(
        &self,
        req: tonic::Request<pb::DeleteHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteHostSpaceResponse>, tonic::Status> {
        let id = req.into_inner().space;
        let mut g = self.spaces.0.lock().unwrap();
        let before = g.spaces.len();
        g.spaces.retain(|s| s.local_space != id);
        if g.spaces.len() == before {
            return Err(tonic::Status::not_found(format!("{id} is not here")));
        }
        g.deleted.push(id.clone());
        Ok(tonic::Response::new(pb::DeleteHostSpaceResponse {
            message: id,
        }))
    }

    async fn cancel_host_space(
        &self,
        _req: tonic::Request<pb::CancelHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CancelHostSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("fake"))
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
        Err(tonic::Status::unimplemented("fake"))
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
        Err(tonic::Status::unimplemented("fake"))
    }
}

/// A fake host: a real runner and server in their own runtime.
pub struct Host {
    pub name: String,
    pub dir: std::path::PathBuf,
    pub exec: FakeExec,
    pub spaces: FakeSpaces,
    pub gate: Gate,
    pub cpus: u32,
    pub rt: Option<tokio::runtime::Runtime>,
}

impl Host {
    pub async fn new(name: &str, root: &std::path::Path, exec: FakeExec, cpus: u32) -> Self {
        let mut h = Self {
            name: name.into(),
            dir: root.join(name),
            exec,
            spaces: FakeSpaces::default(),
            gate: Gate::start().await,
            cpus,
            rt: None,
        };
        h.start();
        h
    }

    /// Starts the "daemon": opens the runner on its ledger and serves it.
    pub fn start(&mut self) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let cfg = NodeConfig {
            dir: self.dir.clone(),
            millicpus_total: self.cpus * 1000,
            memory_total_mb: 1 << 20,
            jobs_max: 0,
            min_available_mb: 0,
            retention: Duration::from_secs(3600),
            output_retention: Duration::from_secs(3600),
            boot_id: format!("{:x}", rand_u64()),
        };
        let exec = self.exec.clone();
        let spaces = self.spaces.clone();
        let name = self.name.clone();
        // Not on this (async) thread: a runtime cannot block inside one.
        let (rt, port) = std::thread::spawn(move || {
            let port = rt.block_on(async move {
                let jobs = NodeJobs::open(cfg, Arc::new(exec)).unwrap();
                let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = l.local_addr().unwrap().port();
                let svc = FakeSpacesSvc {
                    spaces,
                    jobs: jobs.clone(),
                    name,
                };
                tokio::spawn(
                    tonic::transport::Server::builder()
                        .add_service(HostJobsServiceServer::new(JobsService(jobs)))
                        .add_service(
                            pb::host_spaces_service_server::HostSpacesServiceServer::new(svc),
                        )
                        .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(l)),
                );
                port
            });
            (rt, port)
        })
        .join()
        .unwrap();
        self.rt = Some(rt);
        self.gate.open(port);
    }

    /// Cuts the network to the host (its daemon keeps running).
    pub fn partition(&self) {
        self.gate.close();
    }

    /// Restores the network after [`Host::partition`].
    pub fn heal(&self) {
        self.gate.open.store(true, Ordering::SeqCst);
    }

    /// Kills the "daemon": every task of it stops at once.
    pub fn kill(&mut self) {
        self.gate.close();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }

    pub fn row(&self) -> NodeRow {
        NodeRow {
            name: self.name.clone(),
            direct: format!("127.0.0.1:{}", self.gate.port),
            token: "t".into(),
            via: "direct".into(),
            state: "ready".into(),
            os: "linux".into(),
            ..Default::default()
        }
    }
}

pub fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(Instant::now().elapsed().as_nanos());
    h.finish()
}

pub fn links() -> LinkFactory {
    Arc::new(|row: &NodeRow| {
        Some(Arc::new(
            GrpcLink::new(Target::Direct(row.direct.clone()), row.token.clone()).without_probe(),
        ) as Arc<dyn NodeLink>)
    })
}

pub fn config(dir: &std::path::Path) -> Config {
    let mut c = Config::new(dir);
    c.poll_wait = Duration::from_millis(300);
    c.node_down_after = Duration::from_millis(1500);
    c.ack_grace = Duration::from_secs(2);
    c.run_grace = Duration::from_secs(10);
    c
}

pub async fn wait_for<T>(what: &str, within: Duration, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + within;
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
