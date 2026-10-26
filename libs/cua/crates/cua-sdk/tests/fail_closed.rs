//! A Space or sandbox asked for `network: none` or an egress policy is
//! never handed back open. A daemon older than those fields drops them (an
//! unknown protobuf field) and creates the Space with outbound network, so
//! the SDK trusts only the daemon's answer: `CreateSpaceResponse.network`,
//! `CreateSandboxResponse.network` and `Space.egress`. Without it the
//! create is refused and what the daemon made is deleted. A current daemon
//! answers with them and the create works.
//!
//! The old daemon is served in-process and answers as one built before the
//! fields existed; the current one is the real daemon over a fake local
//! runtime whose instance is a mock spacesd. Nothing starts a container or
//! VM, and nothing touches `~/.cua`.

use async_trait::async_trait;
use cua_daemon::{
    Runtime, RuntimeConfig,
    fixtures::{self, SpacesdFixture},
    server::{self, DaemonHandle, ServerConfig},
};
use cua_proto::daemon::v1 as dpb;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError, RuntimeResult,
};
use cua_sdk::{Cua, CuaError, EgressPolicy, SandboxCreateOptions, SpaceCreateOptions};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

// ---------------------------------------------------------------------------
// An older daemon.
// ---------------------------------------------------------------------------

/// What the old daemon was asked and did.
#[derive(Default)]
struct Old {
    creates: Mutex<Vec<dpb::CreateSpaceRequest>>,
    sandbox_creates: Mutex<Vec<dpb::CreateSandboxRequest>>,
    deleted: Mutex<Vec<String>>,
    sandboxes_deleted: Mutex<Vec<String>>,
}

type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// One unary method of the old daemon.
struct Unary<F>(F);

impl<Req, Resp, F> tonic::server::UnaryService<Req> for Unary<F>
where
    F: FnMut(Req) -> Resp,
    Resp: Send + 'static,
{
    type Response = Resp;
    type Future = BoxFut<Result<tonic::Response<Resp>, tonic::Status>>;
    fn call(&mut self, request: tonic::Request<Req>) -> Self::Future {
        let r = (self.0)(request.into_inner());
        Box::pin(async move { Ok(tonic::Response::new(r)) })
    }
}

fn unimplemented() -> http::Response<tonic::body::Body> {
    let mut r = http::Response::new(tonic::body::Body::default());
    r.headers_mut().insert(
        "grpc-status",
        (tonic::Code::Unimplemented as i32)
            .to_string()
            .parse()
            .unwrap(),
    );
    r.headers_mut().insert(
        http::header::CONTENT_TYPE,
        "application/grpc".parse().unwrap(),
    );
    r
}

/// Serves `method` of a request, decoding `Req` and encoding what `f` gives.
fn serve<B, Req, Resp>(
    req: http::Request<B>,
    f: impl FnMut(Req) -> Resp + Send + 'static,
) -> BoxFut<Result<http::Response<tonic::body::Body>, std::convert::Infallible>>
where
    B: tonic::codegen::Body + Send + 'static,
    B::Error: Into<tonic::codegen::StdError> + Send + 'static,
    Req: prost::Message + Default + Send + 'static,
    Resp: prost::Message + Send + 'static,
{
    Box::pin(async move {
        let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::<Resp, Req>::default());
        Ok(grpc.unary(Unary(f), req).await)
    })
}

/// `cua.daemon.v1.SpaceService` and `SandboxService` as a daemon built
/// before `network` and `egress` answers: it creates the Space (open: it
/// never read either field) and its answers carry neither.
#[derive(Clone)]
struct OldDaemon<const SANDBOXES: bool>(Arc<Old>);

impl<const S: bool> tonic::server::NamedService for OldDaemon<S> {
    const NAME: &'static str = if S {
        "cua.daemon.v1.SandboxService"
    } else {
        "cua.daemon.v1.SpaceService"
    };
}

impl<B, const S: bool> tower::Service<http::Request<B>> for OldDaemon<S>
where
    B: tonic::codegen::Body + Send + 'static,
    B::Error: Into<tonic::codegen::StdError> + Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = std::convert::Infallible;
    type Future = BoxFut<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let old = self.0.clone();
        match req.uri().path() {
            "/cua.daemon.v1.SpaceService/CreateSpace" => {
                serve(req, move |r: dpb::CreateSpaceRequest| {
                    let id = format!("local:{}", r.name);
                    let wait = r.wait != Some(false);
                    old.creates.lock().unwrap().push(r);
                    dpb::CreateSpaceResponse {
                        space: Some(dpb::Space {
                            id,
                            ..Default::default()
                        }),
                        phase: if wait { "ready" } else { "starting" }.into(),
                        ..Default::default()
                    }
                })
            }
            "/cua.daemon.v1.SpaceService/DeleteSpace" => {
                serve(req, move |r: dpb::DeleteSpaceRequest| {
                    old.deleted.lock().unwrap().push(r.space);
                    dpb::DeleteSpaceResponse {
                        message: "deleted".into(),
                    }
                })
            }
            "/cua.daemon.v1.SandboxService/CreateSandbox" => {
                serve(req, move |r: dpb::CreateSandboxRequest| {
                    let name = r.name.clone();
                    old.sandbox_creates.lock().unwrap().push(r);
                    dpb::CreateSandboxResponse {
                        sandbox: Some(dpb::Sandbox {
                            name,
                            location: "local".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                })
            }
            "/cua.daemon.v1.SandboxService/DeleteSandbox" => {
                serve(req, move |r: dpb::DeleteSandboxRequest| {
                    old.sandboxes_deleted.lock().unwrap().push(r.name);
                    dpb::DeleteSandboxResponse::default()
                })
            }
            // Everything else (progress streams, cancels, GetInfo) this old
            // daemon does not have.
            _ => Box::pin(async { Ok(unimplemented()) }),
        }
    }
}

/// An old daemon on loopback, and an SDK connected to it.
async fn old_daemon() -> (Arc<Old>, Arc<Cua>) {
    let old = Arc::new(Old::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(OldDaemon::<false>(old.clone()))
            .add_service(OldDaemon::<true>(old.clone()))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    let cua = Cua::connect(Some(format!("http://{addr}")), Some("token".into())).unwrap();
    (old, cua)
}

fn space(name: &str) -> SpaceCreateOptions {
    SpaceCreateOptions {
        image: Some("ghcr.io/trycua/linux:24.04".into()),
        on: Some("local".into()),
        kind: Some("container".into()),
        name: Some(name.into()),
        ..Default::default()
    }
}

fn deny_all() -> EgressPolicy {
    EgressPolicy {
        allow: vec!["1.1.1.1".into()],
        deny: vec!["all".into()],
    }
}

fn too_old(e: &CuaError, what: &str) {
    let CuaError::CapabilityMissing(m) = e else {
        panic!("expected CapabilityMissing, got {e:?}");
    };
    assert!(m.starts_with(what), "{m}");
    assert!(m.contains("too old to enforce"), "{m}");
    assert!(m.contains("update cua"), "{m}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_none_through_an_old_daemon_is_refused_and_deleted() {
    let (old, cua) = old_daemon().await;
    for wait in [None, Some(false)] {
        let name = format!("closed-{}", wait.is_some());
        let e = cua
            .spaces()
            .create(SpaceCreateOptions {
                network: Some("none".into()),
                wait,
                ..space(&name)
            })
            .await
            .expect_err("an open Space must not come back");
        too_old(&e, "network none");
        let CuaError::CapabilityMissing(m) = &e else {
            unreachable!()
        };
        assert!(m.contains(&format!("local:{name}")), "{m}");
        assert_eq!(
            old.deleted.lock().unwrap().last(),
            Some(&format!("local:{name}")),
            "what the old daemon made is deleted"
        );
    }
    let creates = old.creates.lock().unwrap();
    assert_eq!(creates.len(), 2);
    assert!(creates.iter().all(|c| c.network == "none"), "it was asked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn egress_through_an_old_daemon_is_refused_and_deleted() {
    let (old, cua) = old_daemon().await;
    let e = cua
        .spaces()
        .create(SpaceCreateOptions {
            egress: Some(deny_all()),
            ..space("ruled")
        })
        .await
        .expect_err("a Space without its rules must not come back");
    too_old(&e, "egress rules");
    assert_eq!(*old.deleted.lock().unwrap(), ["local:ruled"]);
    // Setting rules later on such a daemon says so too.
    let e = cua
        .spaces()
        .set_egress("local:ruled".into(), deny_all())
        .await
        .unwrap_err();
    let CuaError::CapabilityMissing(m) = &e else {
        panic!("{e:?}")
    };
    assert!(m.contains("too old"), "{m}");
}

/// What no daemon could keep is refused before any daemon is asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_cannot_be_kept_is_refused_before_the_daemon_is_asked() {
    let (old, cua) = old_daemon().await;
    let refused = |o: SpaceCreateOptions| {
        let cua = cua.clone();
        async move {
            match cua.spaces().create(o).await {
                Err(CuaError::InvalidArgument(m)) => m,
                other => panic!("expected InvalidArgument, got {other:?}"),
            }
        }
    };
    let m = refused(SpaceCreateOptions {
        egress: Some(deny_all()),
        wait: Some(false),
        ..space("a")
    })
    .await;
    assert!(m.contains("wait = false"), "{m}");
    for (reuse, durable) in [(true, false), (false, true)] {
        let m = refused(SpaceCreateOptions {
            network: Some("none".into()),
            reuse,
            durable,
            ..space("b")
        })
        .await;
        assert!(m.contains("network=none"), "{m}");
    }
    assert!(old.creates.lock().unwrap().is_empty(), "nothing was sent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sandbox_without_network_through_an_old_daemon_is_refused_and_deleted() {
    let (old, cua) = old_daemon().await;
    let mut o = SandboxCreateOptions::new("local", "ghcr.io/trycua/linux:24.04");
    o.kind = Some("container".into());
    o.name = Some("closed-sb".into());
    o.network = Some("none".into());
    let Err(e) = cua.sandboxes().create(o).await else {
        panic!("an open sandbox must not come back");
    };
    too_old(&e, "network none");
    assert_eq!(*old.sandboxes_deleted.lock().unwrap(), ["closed-sb"]);
    assert_eq!(old.sandbox_creates.lock().unwrap()[0].network, "none");
}

// ---------------------------------------------------------------------------
// A current daemon.
// ---------------------------------------------------------------------------

/// A runtime whose instances are the fixture's spacesd; it keeps the start
/// specs, to show the network was cut.
struct Fake {
    port: u16,
    starts: Arc<Mutex<Vec<LocalStartSpec>>>,
    running: Mutex<HashMap<String, bool>>,
}

#[async_trait]
impl LocalRuntime for Fake {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        self.starts.lock().unwrap().push(spec.clone());
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(&spec.name).await?,
        })
    }
    async fn stop(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(name).await?,
        })
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        match self.running.lock().unwrap().get(name) {
            Some(true) => Ok(InstanceStatus::Running),
            _ => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn endpoints(&self, _: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, self.port)].into(),
            ..Default::default()
        })
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
}

struct Current {
    _env: SpacesdFixture,
    _dirs: tempfile::TempDir,
    _daemon: DaemonHandle,
    cua: Arc<Cua>,
    starts: Arc<Mutex<Vec<LocalStartSpec>>>,
}

async fn current_daemon() -> Current {
    let env = fixtures::start_env(None, None).await;
    let port: u16 = env.url.rsplit(':').next().unwrap().parse().unwrap();
    let dirs = tempfile::tempdir().unwrap();
    let starts: Arc<Mutex<Vec<LocalStartSpec>>> = Arc::default();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        // Never the real ~/.cua registry.
        spaces_home: Some(dirs.path().join("cua")),
        local: Some(Arc::new(Fake {
            port,
            starts: starts.clone(),
            running: Mutex::default(),
        })),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    let h = server::start(
        runtime,
        ServerConfig {
            socket_path: None,
            loopback: Some("127.0.0.1:0".parse().unwrap()),
            token: "daemon-token".into(),
            discovery_path: Some(dirs.path().join("daemon.json")),
            bridge_ticket_ttl: Duration::from_secs(30),
        },
    )
    .await
    .unwrap();
    let cua = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
    Current {
        _env: env,
        _dirs: dirs,
        _daemon: h,
        cua,
        starts,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_current_daemon_cuts_the_network_and_says_so() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let w = current_daemon().await;
        let created = w
            .cua
            .spaces()
            .create(SpaceCreateOptions {
                network: Some("none".into()),
                ..space("cut")
            })
            .await
            .unwrap();
        assert_eq!(created.space.unwrap().id, "local:cut");
        let mut o = SandboxCreateOptions::new("local", "ghcr.io/trycua/linux:24.04");
        o.kind = Some("container".into());
        o.name = Some("cut-sb".into());
        o.network = Some("none".into());
        let sb = w.cua.sandboxes().create(o).await.unwrap();
        assert_eq!(sb.info().name, "cut-sb");
        let starts = w.starts.lock().unwrap();
        assert_eq!(starts.len(), 2, "{starts:?}");
        assert!(
            starts.iter().all(|s| s.restrict_network),
            "both were cut: {starts:?}"
        );
    })
    .await
    .expect("timed out");
}
