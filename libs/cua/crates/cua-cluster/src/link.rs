//! How a coordinator reaches a host: one connection per host, never one
//! per Space or job.
//!
//! - [`GrpcLink`] reaches the host's cua-spacesd (which forwards
//!   `HostJobsService` and `HostSpacesService` to the host's cua daemon)
//!   at its direct address (a Tailscale, LAN or VPN address) or through a
//!   relay at `<relay>/m/<machine>`, a self-hosted `cua-relay` in static
//!   token mode included. Either way it authenticates with the host's env
//!   token. HTTP/2 multiplexes every call over the one connection.
//! - [`Target::Mesh`] reaches the same machine over the relay's
//!   WireGuard®-based mesh (`cua-mesh`): end to end encrypted, direct when
//!   the two networks allow it, relayed as ciphertext otherwise. This
//!   process pairs with the machine by proving its env token (the relay
//!   never sees the token).
//! - [`LocalLink`] is a runner in this process (an elastic provider node,
//!   tests).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_proto::env::v1 as pb;
use cua_proto::env::v1::host_jobs_service_client::HostJobsServiceClient;
use cua_proto::env::v1::host_spaces_service_client::HostSpacesServiceClient;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};

use crate::node::NodeJobs;

/// Why a call to a host failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    /// The host did not answer (down, partitioned, restarting).
    #[error("unreachable: {0}")]
    Unreachable(String),
    /// The host does not know the thing asked for.
    #[error("not found: {0}")]
    NotFound(String),
    /// The host answered and refused (credentials, policy, bad request).
    #[error("refused: {0}")]
    Refused(String),
}

impl LinkError {
    fn from_status(s: &tonic::Status) -> Self {
        use tonic::Code;
        let msg = s.message().to_string();
        match s.code() {
            Code::NotFound => Self::NotFound(msg),
            Code::Unavailable
            | Code::DeadlineExceeded
            | Code::Unknown
            | Code::Cancelled
            | Code::Aborted
            | Code::Internal => Self::Unreachable(format!("{:?}: {msg}", s.code())),
            _ => Self::Refused(format!("{:?}: {msg}", s.code())),
        }
    }
}

/// The calls a coordinator makes to a host.
#[async_trait]
pub trait NodeLink: Send + Sync {
    /// `RunHostJobs`.
    async fn run(&self, req: pb::RunHostJobsRequest) -> Result<pb::RunHostJobsResponse, LinkError>;
    /// `GetHostJob` (`NotFound` when the host never saw it).
    async fn get(&self, job_id: &str) -> Result<pb::GetHostJobResponse, LinkError>;
    /// `GetHostJob` with the job's live output from byte `from`
    /// (`output_chunk`, `output_size`). A link that cannot ask for it
    /// answers with the record only.
    async fn get_output(
        &self,
        job_id: &str,
        _from: u64,
    ) -> Result<pb::GetHostJobResponse, LinkError> {
        self.get(job_id).await
    }
    /// `ListHostJobs` (a long poll with `wait_ms`).
    async fn list(
        &self,
        req: pb::ListHostJobsRequest,
    ) -> Result<pb::ListHostJobsResponse, LinkError>;
    /// `CancelHostJob`.
    async fn cancel(
        &self,
        req: pb::CancelHostJobRequest,
    ) -> Result<pb::CancelHostJobResponse, LinkError>;
    /// `GetHostSpaces` (name, OS, settings, node status).
    async fn host(&self) -> Result<pb::GetHostSpacesResponse, LinkError>;
    /// `CreateHostSpace` (an interactive Space placed by the cluster).
    async fn create_space(
        &self,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace, LinkError>;
    /// `DeleteHostSpace` (`local:<name>` on the host).
    async fn delete_space(&self, space: &str) -> Result<String, LinkError>;
    /// Where it goes, for people (`direct 100.64.0.7:3211`, `relay ...`).
    fn describe(&self) -> String;
}

/// Where a [`GrpcLink`] connects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// `host:port` of the host's direct listener.
    Direct(String),
    /// The host's machine on a relay.
    Relay {
        /// Relay base URL.
        url: String,
        /// The host's machine id there.
        machine: String,
    },
    /// The host's machine on a relay's mesh (see the module docs).
    Mesh {
        /// Relay base URL.
        url: String,
        /// The host's machine id there.
        machine: String,
    },
}

impl Target {
    /// The spacesd URL.
    pub fn url(&self) -> String {
        match self {
            Target::Direct(a) if a.contains("://") => a.clone(),
            Target::Direct(a) => format!("http://{a}"),
            Target::Relay { url, machine } | Target::Mesh { url, machine } => {
                format!("{}/m/{machine}", url.trim_end_matches('/'))
            }
        }
    }
}

/// One mesh client node per relay, shared by every link of this process.
#[derive(Default)]
struct Dialers {
    by_relay: tokio::sync::Mutex<
        std::collections::HashMap<
            String,
            Result<Arc<cua_mesh::dialer::Dialer>, std::time::Instant>,
        >,
    >,
}

static DIALERS: std::sync::LazyLock<Dialers> = std::sync::LazyLock::new(Dialers::default);

/// How long a relay without a mesh is not asked again.
const NO_MESH_RECHECK: Duration = Duration::from_secs(600);

impl Dialers {
    async fn get(&self, relay: &str) -> Result<Arc<cua_mesh::dialer::Dialer>, LinkError> {
        let mut map = self.by_relay.lock().await;
        match map.get(relay) {
            Some(Ok(d)) => return Ok(d.clone()),
            Some(Err(at)) if at.elapsed() < NO_MESH_RECHECK => {
                return Err(LinkError::Unreachable(format!("{relay} has no mesh")));
            }
            _ => {}
        }
        // The cluster's hosts are this build's channel's: they serve their
        // media on its mesh UDP port.
        match cua_mesh::dialer::Dialer::start_with_media_port(
            relay,
            cua_mesh::static_auth(Vec::new()),
            None,
            Duration::from_secs(5),
            cua_home::channel::CuaChannel::current().ports().mesh_udp,
        )
        .await
        {
            Ok(d) => {
                let d = Arc::new(d);
                map.insert(relay.to_owned(), Ok(d.clone()));
                Ok(d)
            }
            Err(e) => {
                map.insert(relay.to_owned(), Err(std::time::Instant::now()));
                Err(LinkError::Unreachable(format!("mesh {relay}: {e}")))
            }
        }
    }
}

/// Makes a connected client to one host (the pool reaches account
/// machines with the account's credentials this way, see
/// [`GrpcLink::with_connector`]).
pub type Connector = Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<SpacesdClient, LinkError>> + Send>,
        > + Send
        + Sync,
>;

/// A host's cua-spacesd over gRPC (direct or relayed).
pub struct GrpcLink {
    target: Target,
    token: String,
    probe: bool,
    connector: Option<(Connector, String)>,
    client: tokio::sync::Mutex<Option<SpacesdClient>>,
}

/// How long a dispatch, a lookup and a cancel may take.
const CALL: Duration = Duration::from_secs(30);

impl GrpcLink {
    /// A link to `target` with the host's env token.
    pub fn new(target: Target, token: impl Into<String>) -> Self {
        Self {
            target,
            token: token.into(),
            probe: true,
            connector: None,
            client: tokio::sync::Mutex::new(None),
        }
    }

    /// A link whose client comes from `connector` (called again after the
    /// connection breaks); `describe` says where it goes, for people.
    pub fn with_connector(connector: Connector, describe: impl Into<String>) -> Self {
        Self {
            target: Target::Direct(String::new()),
            token: String::new(),
            probe: true,
            connector: Some((connector, describe.into())),
            client: tokio::sync::Mutex::new(None),
        }
    }

    /// Native gRPC without the capabilities probe (a server that serves
    /// only the host services, in tests).
    pub fn without_probe(mut self) -> Self {
        self.probe = false;
        self
    }

    /// The target.
    pub fn target(&self) -> &Target {
        &self.target
    }

    async fn client(&self) -> Result<SpacesdClient, LinkError> {
        let mut g = self.client.lock().await;
        if let Some(c) = g.as_ref() {
            return Ok(c.clone());
        }
        if let Some((connect, _)) = &self.connector {
            let c = connect().await?;
            *g = Some(c.clone());
            return Ok(c);
        }
        let url = match &self.target {
            Target::Mesh { url, machine } => {
                let dialer = DIALERS.get(url).await?;
                let route = dialer
                    .route(machine, Some(&self.token), Duration::from_secs(5))
                    .await
                    .map_err(|e| match e {
                        cua_mesh::Error::Refused(r) => LinkError::Refused(r),
                        other => LinkError::Unreachable(other.to_string()),
                    })?;
                format!("http://{}", route.local)
            }
            other => other.url(),
        };
        let mut opts = ConnectOptions::parse(&url)
            .map_err(|e| LinkError::Refused(e.to_string()))?
            .token(self.token.clone())
            .probe_timeout(Duration::from_secs(10));
        if !self.probe {
            opts = opts.probe(false).transport(TransportPreference::Native);
        }
        opts.connect_timeout = Duration::from_secs(5);
        let c = SpacesdClient::connect(opts).await.map_err(|e| match e {
            cua_spacesd_client::Error::Unauthenticated(d) => LinkError::Refused(d.message),
            other => LinkError::Unreachable(other.to_string()),
        })?;
        *g = Some(c.clone());
        Ok(c)
    }

    async fn forget(&self) {
        *self.client.lock().await = None;
    }

    async fn call<T, F, Fut>(&self, timeout: Duration, f: F) -> Result<T, LinkError>
    where
        F: FnOnce(SpacesdClient) -> Fut,
        Fut: std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        let c = self.client().await?;
        match tokio::time::timeout(timeout, f(c)).await {
            Ok(Ok(r)) => Ok(r.into_inner()),
            Ok(Err(s)) => {
                let e = LinkError::from_status(&s);
                if matches!(e, LinkError::Unreachable(_)) {
                    self.forget().await;
                }
                Err(e)
            }
            Err(_) => {
                self.forget().await;
                Err(LinkError::Unreachable(format!(
                    "no answer within {timeout:?}"
                )))
            }
        }
    }
}

impl GrpcLink {
    /// [`GrpcLink::call`] for a `HostJobsService` RPC, which only a Cua 0.8
    /// host serves: an older host is refused up front by its reported
    /// revision, and what one answers anyway (`unimplemented`, or the
    /// `unauthenticated` its access layer gives unknown methods) is the same
    /// refusal, not a permissions error.
    async fn call_jobs<T, F, Fut>(&self, timeout: Duration, f: F) -> Result<T, LinkError>
    where
        F: FnOnce(SpacesdClient) -> Fut,
        Fut: std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        use cua_spacesd_client::old_host;
        const WHAT: &str = "cluster jobs";
        let c = self.client().await?;
        if self.probe
            && let Ok(caps) = c.capabilities().await
            && let Some(old) = old_host::too_old(&caps)
        {
            return Err(LinkError::Refused(old_host::message(
                &self.describe(),
                Some(&old),
                WHAT,
            )));
        }
        self.call(timeout, f).await.map_err(|e| match e {
            LinkError::Refused(m)
                if m.starts_with("Unauthenticated:") || m.starts_with("Unimplemented:") =>
            {
                LinkError::Refused(old_host::message(&self.describe(), None, WHAT))
            }
            other => other,
        })
    }
}

fn jobs(c: &SpacesdClient) -> HostJobsServiceClient<cua_spacesd_client::GrpcChannel> {
    HostJobsServiceClient::new(c.channel())
        .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
        .max_encoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
}

fn spaces(c: &SpacesdClient) -> HostSpacesServiceClient<cua_spacesd_client::GrpcChannel> {
    HostSpacesServiceClient::new(c.channel())
        .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
}

#[async_trait]
impl NodeLink for GrpcLink {
    async fn run(&self, req: pb::RunHostJobsRequest) -> Result<pb::RunHostJobsResponse, LinkError> {
        self.call_jobs(CALL, |c| async move { jobs(&c).run_host_jobs(req).await })
            .await
    }

    async fn get(&self, job_id: &str) -> Result<pb::GetHostJobResponse, LinkError> {
        let req = pb::GetHostJobRequest {
            job_id: job_id.to_string(),
            output_from: None,
        };
        self.call_jobs(CALL, |c| async move { jobs(&c).get_host_job(req).await })
            .await
    }

    async fn get_output(
        &self,
        job_id: &str,
        from: u64,
    ) -> Result<pb::GetHostJobResponse, LinkError> {
        let req = pb::GetHostJobRequest {
            job_id: job_id.to_string(),
            output_from: Some(from),
        };
        self.call_jobs(CALL, |c| async move { jobs(&c).get_host_job(req).await })
            .await
    }

    async fn list(
        &self,
        req: pb::ListHostJobsRequest,
    ) -> Result<pb::ListHostJobsResponse, LinkError> {
        let t = Duration::from_millis(u64::from(req.wait_ms)) + Duration::from_secs(10);
        self.call_jobs(t, |c| async move { jobs(&c).list_host_jobs(req).await })
            .await
    }

    async fn cancel(
        &self,
        req: pb::CancelHostJobRequest,
    ) -> Result<pb::CancelHostJobResponse, LinkError> {
        self.call_jobs(CALL, |c| async move { jobs(&c).cancel_host_job(req).await })
            .await
    }

    async fn host(&self) -> Result<pb::GetHostSpacesResponse, LinkError> {
        self.call(Duration::from_secs(15), |c| async move {
            spaces(&c)
                .get_host_spaces(pb::GetHostSpacesRequest {})
                .await
        })
        .await
    }

    async fn create_space(
        &self,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace, LinkError> {
        self.call(Duration::from_secs(20 * 60), |c| async move {
            spaces(&c).create_host_space(req).await
        })
        .await?
        .space
        .ok_or_else(|| LinkError::Refused("the host answered without the Space".into()))
    }

    async fn delete_space(&self, space: &str) -> Result<String, LinkError> {
        let req = pb::DeleteHostSpaceRequest {
            space: space.to_string(),
        };
        self.call(Duration::from_secs(10 * 60), |c| async move {
            spaces(&c).delete_host_space(req).await
        })
        .await
        .map(|r| r.message)
    }

    fn describe(&self) -> String {
        if let Some((_, d)) = &self.connector {
            return d.clone();
        }
        match &self.target {
            Target::Direct(a) => format!("direct {a}"),
            Target::Relay { url, machine } => format!("relay {url} ({machine})"),
            Target::Mesh { url, machine } => format!("mesh {url} ({machine})"),
        }
    }
}

/// Two ways to one host (its direct address, then the relay): a call that
/// finds the current way unreachable tries the other, which becomes the
/// current way when it answers.
pub struct FailoverLink {
    ways: Vec<Arc<dyn NodeLink>>,
    current: std::sync::atomic::AtomicUsize,
    /// When each way last failed: a way that failed recently is tried
    /// after the others, and the preferred ways get another chance once
    /// [`FailoverLink::RETRY_PREFERRED`] has passed.
    failed_at: std::sync::Mutex<Vec<Option<std::time::Instant>>>,
}

impl FailoverLink {
    /// How long a way that failed waits before it is preferred again.
    pub const RETRY_PREFERRED: std::time::Duration = std::time::Duration::from_secs(20);

    /// The ways, preferred first.
    pub fn new(ways: Vec<Arc<dyn NodeLink>>) -> Self {
        assert!(!ways.is_empty(), "a link needs a way");
        let n = ways.len();
        Self {
            ways,
            current: std::sync::atomic::AtomicUsize::new(0),
            failed_at: std::sync::Mutex::new(vec![None; n]),
        }
    }

    /// The order to try the ways in: every way in preference order, those
    /// that failed in the last [`Self::RETRY_PREFERRED`] last. A host whose
    /// preferred (direct) way failed once under load goes back to it, so
    /// one bad fallback cannot hold every call.
    fn order(&self) -> Vec<usize> {
        let failed = self.failed_at.lock().unwrap_or_else(|p| p.into_inner());
        let recent = |i: usize| failed[i].is_some_and(|t| t.elapsed() < Self::RETRY_PREFERRED);
        let mut order: Vec<usize> = (0..self.ways.len()).filter(|&i| !recent(i)).collect();
        order.extend((0..self.ways.len()).filter(|&i| recent(i)));
        order
    }

    async fn each<T, F, Fut>(&self, f: F) -> Result<T, LinkError>
    where
        F: Fn(Arc<dyn NodeLink>) -> Fut,
        Fut: std::future::Future<Output = Result<T, LinkError>>,
    {
        use std::sync::atomic::Ordering;
        let was = self.current.load(Ordering::Relaxed);
        let mut last = None;
        for i in self.order() {
            match f(self.ways[i].clone()).await {
                Err(LinkError::Unreachable(e)) => {
                    self.failed_at.lock().unwrap_or_else(|p| p.into_inner())[i] =
                        Some(std::time::Instant::now());
                    last = Some(LinkError::Unreachable(e));
                }
                other => {
                    if i != was {
                        tracing::info!(way = %self.ways[i].describe(), "reaching the host another way");
                        self.current.store(i, Ordering::Relaxed);
                    }
                    return other;
                }
            }
        }
        Err(last.unwrap_or_else(|| LinkError::Unreachable("no way to the host".into())))
    }
}

#[async_trait]
impl NodeLink for FailoverLink {
    async fn run(&self, req: pb::RunHostJobsRequest) -> Result<pb::RunHostJobsResponse, LinkError> {
        self.each(|l| {
            let req = req.clone();
            async move { l.run(req).await }
        })
        .await
    }

    async fn get(&self, job_id: &str) -> Result<pb::GetHostJobResponse, LinkError> {
        self.each(|l| async move { l.get(job_id).await }).await
    }

    async fn get_output(
        &self,
        job_id: &str,
        from: u64,
    ) -> Result<pb::GetHostJobResponse, LinkError> {
        self.each(|l| async move { l.get_output(job_id, from).await })
            .await
    }

    async fn list(
        &self,
        req: pb::ListHostJobsRequest,
    ) -> Result<pb::ListHostJobsResponse, LinkError> {
        self.each(|l| {
            let req = req.clone();
            async move { l.list(req).await }
        })
        .await
    }

    async fn cancel(
        &self,
        req: pb::CancelHostJobRequest,
    ) -> Result<pb::CancelHostJobResponse, LinkError> {
        self.each(|l| {
            let req = req.clone();
            async move { l.cancel(req).await }
        })
        .await
    }

    async fn host(&self) -> Result<pb::GetHostSpacesResponse, LinkError> {
        self.each(|l| async move { l.host().await }).await
    }

    async fn create_space(
        &self,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace, LinkError> {
        self.each(|l| {
            let req = req.clone();
            async move { l.create_space(req).await }
        })
        .await
    }

    async fn delete_space(&self, space: &str) -> Result<String, LinkError> {
        self.each(|l| async move { l.delete_space(space).await })
            .await
    }

    fn describe(&self) -> String {
        let i = self.current.load(std::sync::atomic::Ordering::Relaxed);
        self.ways[i].describe()
    }
}

/// A runner in this process.
pub struct LocalLink {
    jobs: Arc<NodeJobs>,
    name: String,
    os: String,
    owner: Option<String>,
}

impl LocalLink {
    /// A link to `jobs`, which reports itself as host `name` on `os`.
    pub fn new(jobs: Arc<NodeJobs>, name: impl Into<String>, os: impl Into<String>) -> Self {
        Self {
            jobs,
            name: name.into(),
            os: os.into(),
            owner: None,
        }
    }

    /// A link that acts as account `owner` on a shared runner (the job ids
    /// the host's daemon gives that account's calls; see
    /// [`NodeJobs::run_batch_for`]).
    pub fn owned_by(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }
}

#[async_trait]
impl NodeLink for LocalLink {
    async fn run(&self, req: pb::RunHostJobsRequest) -> Result<pb::RunHostJobsResponse, LinkError> {
        let acks = match &self.owner {
            Some(o) => self.jobs.run_batch_for(o, req.jobs, &req.scheduler).await,
            None => self.jobs.run_batch(req.jobs, &req.scheduler).await,
        };
        Ok(pb::RunHostJobsResponse {
            acks,
            node: Some(self.jobs.status()),
        })
    }

    async fn get(&self, job_id: &str) -> Result<pb::GetHostJobResponse, LinkError> {
        let job = match &self.owner {
            Some(o) => self.jobs.get_for(o, job_id, true),
            None => self.jobs.get(job_id).map(|j| j.to_pb(true)),
        }
        .ok_or_else(|| LinkError::NotFound(job_id.to_string()))?;
        Ok(pb::GetHostJobResponse {
            job: Some(job),
            node: Some(self.jobs.status()),
            ..Default::default()
        })
    }

    async fn get_output(
        &self,
        job_id: &str,
        from: u64,
    ) -> Result<pb::GetHostJobResponse, LinkError> {
        let mut r = self.get(job_id).await?;
        let out = match &self.owner {
            Some(o) => self.jobs.output_for(o, job_id, from),
            None => self.jobs.output(job_id, from),
        };
        if let Some((chunk, size)) = out {
            r.output_chunk = chunk;
            r.output_size = size;
        }
        Ok(r)
    }

    async fn list(
        &self,
        req: pb::ListHostJobsRequest,
    ) -> Result<pb::ListHostJobsResponse, LinkError> {
        let wait = Duration::from_millis(u64::from(req.wait_ms));
        self.jobs.release(self.owner.as_deref(), &req.release);
        Ok(match &self.owner {
            Some(o) => {
                self.jobs
                    .list_for(o, req.after_seq, req.limit, wait, !req.without_output)
                    .await
            }
            None => {
                self.jobs
                    .list(req.after_seq, req.limit, wait, !req.without_output)
                    .await
            }
        })
    }

    async fn cancel(
        &self,
        req: pb::CancelHostJobRequest,
    ) -> Result<pb::CancelHostJobResponse, LinkError> {
        let job = match &self.owner {
            Some(o) => {
                self.jobs
                    .cancel_for(o, &req.job_id, req.attempt, &req.reason)
                    .await
            }
            None => self
                .jobs
                .cancel(&req.job_id, req.attempt, &req.reason)
                .await
                .to_pb(false),
        };
        Ok(pb::CancelHostJobResponse { job: Some(job) })
    }

    async fn host(&self) -> Result<pb::GetHostSpacesResponse, LinkError> {
        Ok(pb::GetHostSpacesResponse {
            name: self.name.clone(),
            os: self.os.clone(),
            settings: Some(pb::HostSpacesSettings {
                provide_spaces: true,
                ..Default::default()
            }),
            node: Some(self.jobs.status()),
            ..Default::default()
        })
    }

    async fn create_space(
        &self,
        _req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace, LinkError> {
        Err(LinkError::Refused(format!(
            "{} runs jobs only, not interactive Spaces",
            self.name
        )))
    }

    async fn delete_space(&self, space: &str) -> Result<String, LinkError> {
        Err(LinkError::NotFound(format!(
            "{space} (an in-process host runs jobs only)"
        )))
    }

    fn describe(&self) -> String {
        format!("in process ({})", self.name)
    }
}

/// `HostJobsService` over a runner, without any caller checks: what a
/// test host or a trusted in-process server serves. The host's daemon
/// serves its own, which checks the caller and the host's settings first.
#[derive(Clone)]
pub struct JobsService(pub Arc<NodeJobs>);

#[tonic::async_trait]
impl pb::host_jobs_service_server::HostJobsService for JobsService {
    async fn run_host_jobs(
        &self,
        req: tonic::Request<pb::RunHostJobsRequest>,
    ) -> Result<tonic::Response<pb::RunHostJobsResponse>, tonic::Status> {
        let r = req.into_inner();
        let acks = self.0.run_batch(r.jobs, &r.scheduler).await;
        Ok(tonic::Response::new(pb::RunHostJobsResponse {
            acks,
            node: Some(self.0.status()),
        }))
    }

    async fn get_host_job(
        &self,
        req: tonic::Request<pb::GetHostJobRequest>,
    ) -> Result<tonic::Response<pb::GetHostJobResponse>, tonic::Status> {
        let r = req.into_inner();
        let id = r.job_id;
        let job = self
            .0
            .get(&id)
            .ok_or_else(|| tonic::Status::not_found(format!("no job {id} on this host")))?;
        let (output_chunk, output_size) = r
            .output_from
            .and_then(|from| self.0.output(&id, from))
            .unwrap_or_default();
        Ok(tonic::Response::new(pb::GetHostJobResponse {
            job: Some(job.to_pb(true)),
            node: Some(self.0.status()),
            output_chunk,
            output_size,
        }))
    }

    async fn list_host_jobs(
        &self,
        req: tonic::Request<pb::ListHostJobsRequest>,
    ) -> Result<tonic::Response<pb::ListHostJobsResponse>, tonic::Status> {
        let r = req.into_inner();
        self.0.release(None, &r.release);
        Ok(tonic::Response::new(
            self.0
                .list(
                    r.after_seq,
                    r.limit,
                    Duration::from_millis(u64::from(r.wait_ms)),
                    !r.without_output,
                )
                .await,
        ))
    }

    async fn cancel_host_job(
        &self,
        req: tonic::Request<pb::CancelHostJobRequest>,
    ) -> Result<tonic::Response<pb::CancelHostJobResponse>, tonic::Status> {
        let r = req.into_inner();
        let job = self.0.cancel(&r.job_id, r.attempt, &r.reason).await;
        Ok(tonic::Response::new(pb::CancelHostJobResponse {
            job: Some(job.to_pb(false)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A way that answers `host()` while `up`, and counts its calls.
    struct Way {
        name: &'static str,
        up: std::sync::atomic::AtomicBool,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Way {
        fn new(name: &'static str, up: bool) -> Arc<Self> {
            Arc::new(Self {
                name,
                up: up.into(),
                calls: 0.into(),
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl NodeLink for Way {
        async fn run(
            &self,
            _: pb::RunHostJobsRequest,
        ) -> Result<pb::RunHostJobsResponse, LinkError> {
            Err(LinkError::Unreachable("x".into()))
        }
        async fn get(&self, _: &str) -> Result<pb::GetHostJobResponse, LinkError> {
            Err(LinkError::Unreachable("x".into()))
        }
        async fn list(
            &self,
            _: pb::ListHostJobsRequest,
        ) -> Result<pb::ListHostJobsResponse, LinkError> {
            Err(LinkError::Unreachable("x".into()))
        }
        async fn cancel(
            &self,
            _: pb::CancelHostJobRequest,
        ) -> Result<pb::CancelHostJobResponse, LinkError> {
            Err(LinkError::Unreachable("x".into()))
        }
        async fn host(&self) -> Result<pb::GetHostSpacesResponse, LinkError> {
            use std::sync::atomic::Ordering::SeqCst;
            self.calls.fetch_add(1, SeqCst);
            if self.up.load(SeqCst) {
                Ok(pb::GetHostSpacesResponse {
                    name: self.name.into(),
                    ..Default::default()
                })
            } else {
                Err(LinkError::Unreachable(format!("{} down", self.name)))
            }
        }
        async fn create_space(
            &self,
            _: pb::CreateHostSpaceRequest,
        ) -> Result<pb::HostSpace, LinkError> {
            Err(LinkError::Unreachable("x".into()))
        }
        async fn delete_space(&self, _: &str) -> Result<String, LinkError> {
            Err(LinkError::Unreachable("x".into()))
        }
        fn describe(&self) -> String {
            self.name.into()
        }
    }

    #[tokio::test]
    async fn a_failover_goes_back_to_the_preferred_way() {
        use std::sync::atomic::Ordering::SeqCst;
        let direct = Way::new("direct", false);
        let mesh = Way::new("mesh", true);
        let link = FailoverLink::new(vec![direct.clone(), mesh.clone()]);
        // Direct fails: the call is answered the other way.
        assert_eq!(link.host().await.unwrap().name, "mesh");
        // While direct failed recently, calls go straight to the fallback.
        assert_eq!(link.host().await.unwrap().name, "mesh");
        assert_eq!((direct.calls(), mesh.calls()), (1, 2));
        // The fallback dies too: direct is tried again even though it
        // failed recently, and answers now.
        mesh.up.store(false, SeqCst);
        direct.up.store(true, SeqCst);
        assert_eq!(link.host().await.unwrap().name, "direct");
        // Once direct's failure is old, it is preferred again.
        mesh.up.store(true, SeqCst);
        link.failed_at.lock().unwrap()[0] =
            Some(std::time::Instant::now() - FailoverLink::RETRY_PREFERRED * 2);
        let before = mesh.calls();
        assert_eq!(link.host().await.unwrap().name, "direct");
        assert_eq!(mesh.calls(), before);
    }

    #[test]
    fn targets_are_spacesd_urls() {
        assert_eq!(
            Target::Direct("100.64.0.7:3211".into()).url(),
            "http://100.64.0.7:3211"
        );
        assert_eq!(
            Target::Relay {
                url: "http://relay.lan:8080/".into(),
                machine: "abcd1234".into()
            }
            .url(),
            "http://relay.lan:8080/m/abcd1234"
        );
    }

    #[test]
    fn mesh_targets_describe_themselves() {
        let t = Target::Mesh {
            url: "https://relay.example".into(),
            machine: "abcd1234".into(),
        };
        assert_eq!(t.url(), "https://relay.example/m/abcd1234");
        assert_eq!(
            GrpcLink::new(t, "t").describe(),
            "mesh https://relay.example (abcd1234)"
        );
    }

    #[test]
    fn statuses_map_to_link_errors() {
        assert!(matches!(
            LinkError::from_status(&tonic::Status::unavailable("x")),
            LinkError::Unreachable(_)
        ));
        assert!(matches!(
            LinkError::from_status(&tonic::Status::not_found("x")),
            LinkError::NotFound(_)
        ));
        assert!(matches!(
            LinkError::from_status(&tonic::Status::permission_denied("x")),
            LinkError::Refused(_)
        ));
    }
}
