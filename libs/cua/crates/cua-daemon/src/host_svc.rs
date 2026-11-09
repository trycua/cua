//! `cua.env.v1.HostSpacesService` in the daemon: the Spaces this machine
//! provides to its owner's devices ([`cua_spaces::host_spaces`]). The host's
//! cua-spacesd authenticates the relayed caller and forwards each call here
//! over the daemon's socket, with the caller in
//! [`cua_spaces::host_spaces::CALLER_METADATA`]; a call without it is this
//! machine's own (the owner).
//!
//! `cua.env.v1.HostThreadService` (the threads of the Spaces this machine
//! provides, for the people it is shared with) arrives the same way and
//! runs the tool as the forwarded caller
//! ([`cua_spaces::host_spaces::HostSpacesServer::call_thread_tool`]).
//!
//! `cua.env.v1.HostJobsService` (cluster jobs, [`cua_spaces::host_jobs`])
//! arrives the same way and is answered by the host's job runner
//! ([`cua_spaces::host_spaces::HostSpacesServer::jobs_for`]).

use std::sync::Arc;

use std::time::Duration;

use cua_proto::env::v1::{
    self as pb, host_jobs_service_server::HostJobsService,
    host_snapshots_service_server::HostSnapshotsService,
    host_spaces_service_server::HostSpacesService, host_thread_service_server::HostThreadService,
};
use cua_spaces::host_spaces::{HostCaller, to_status};

use crate::server::Shared;

/// The service, over the daemon's shared state.
pub(crate) struct HostSvc(pub(crate) Arc<Shared>);

/// The forwarded caller of a host call. A Space thread grant reaches only
/// `HostThreadService` ([`HostThreadSvc`] reads its caller itself).
fn caller<T>(req: &tonic::Request<T>) -> Result<HostCaller, tonic::Status> {
    let who = HostCaller::from_grpc_metadata(req.metadata()).map_err(|e| to_status(&e))?;
    if who.space.is_some() {
        return Err(tonic::Status::permission_denied(
            "a Space shared with you reaches only its thread on this machine",
        ));
    }
    Ok(who)
}

#[tonic::async_trait]
impl HostSpacesService for HostSvc {
    async fn get_host_spaces(
        &self,
        req: tonic::Request<pb::GetHostSpacesRequest>,
    ) -> Result<tonic::Response<pb::GetHostSpacesResponse>, tonic::Status> {
        let who = caller(&req)?;
        let mut answer = self
            .0
            .host_spaces
            .get(&who)
            .await
            .map_err(|e| to_status(&e))?;
        if req.get_ref().disk {
            self.0.host_spaces.with_disks(&mut answer.spaces).await;
        }
        Ok(tonic::Response::new(answer))
    }

    async fn create_host_space(
        &self,
        req: tonic::Request<pb::CreateHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CreateHostSpaceResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
            .create(&who, req.into_inner())
            .await
            .map(|space| tonic::Response::new(pb::CreateHostSpaceResponse { space: Some(space) }))
            .map_err(|e| to_status(&e))
    }

    async fn cancel_host_space(
        &self,
        req: tonic::Request<pb::CancelHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CancelHostSpaceResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
            .cancel(&who, &req.into_inner().space)
            .await
            .map(|message| tonic::Response::new(pb::CancelHostSpaceResponse { message }))
            .map_err(|e| to_status(&e))
    }

    async fn delete_cloud_space(
        &self,
        req: tonic::Request<pb::DeleteCloudSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteCloudSpaceResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
            .delete_cloud(&who, &req.into_inner().space)
            .await
            .map(|message| tonic::Response::new(pb::DeleteCloudSpaceResponse { message }))
            .map_err(|e| to_status(&e))
    }

    async fn delete_host_space(
        &self,
        req: tonic::Request<pb::DeleteHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteHostSpaceResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
            .delete(&who, &req.into_inner().space)
            .await
            .map(|message| tonic::Response::new(pb::DeleteHostSpaceResponse { message }))
            .map_err(|e| to_status(&e))
    }

    async fn set_host_warm(
        &self,
        req: tonic::Request<pb::SetHostWarmRequest>,
    ) -> Result<tonic::Response<pb::SetHostWarmResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
            .set_warm(&who, req.into_inner())
            .await
            .map(tonic::Response::new)
            .map_err(|e| to_status(&e))
    }

    async fn set_host_space_power(
        &self,
        req: tonic::Request<pb::SetHostSpacePowerRequest>,
    ) -> Result<tonic::Response<pb::SetHostSpacePowerResponse>, tonic::Status> {
        let who = caller(&req)?;
        let r = req.into_inner();
        self.0
            .host_spaces
            .set_power(&who, &r.space, r.on)
            .await
            .map(tonic::Response::new)
            .map_err(|e| to_status(&e))
    }

    async fn set_host_space_egress(
        &self,
        req: tonic::Request<pb::SetHostSpaceEgressRequest>,
    ) -> Result<tonic::Response<pb::SetHostSpaceEgressResponse>, tonic::Status> {
        let who = caller(&req)?;
        let r = req.into_inner();
        self.0
            .host_spaces
            .set_egress(&who, &r.space, r.egress)
            .await
            .map(tonic::Response::new)
            .map_err(|e| to_status(&e))
    }

    async fn diagnose_host(
        &self,
        req: tonic::Request<pb::DiagnoseHostRequest>,
    ) -> Result<tonic::Response<pb::DiagnoseHostResponse>, tonic::Status> {
        // The owner's own diagnostics: paths, app names, free space.
        let who = caller(&req)?;
        if !who.is_owner() {
            return Err(tonic::Status::permission_denied(
                "only this machine's owner can check its health",
            ));
        }
        let machine = req.into_inner().machine;
        if machine.trim().is_empty() {
            return Ok(tonic::Response::new(
                crate::machine_doctor::response().await,
            ));
        }
        // Another machine: asked from this one (the app or `cua doctor
        // --machine`), never on behalf of a relayed caller.
        if who.account != "local" {
            return Err(tonic::Status::permission_denied(
                "a machine's health is checked on that machine",
            ));
        }
        self.0
            .runtime
            .spaces()
            .machine_health(&machine)
            .await
            .map(tonic::Response::new)
            .map_err(|e| to_status(&e))
    }
}

/// `HostThreadService`, over the same shared state.
pub(crate) struct HostThreadSvc(pub(crate) Arc<Shared>);

#[tonic::async_trait]
impl HostThreadService for HostThreadSvc {
    async fn call_thread_tool(
        &self,
        req: tonic::Request<pb::CallThreadToolRequest>,
    ) -> Result<tonic::Response<pb::CallThreadToolResponse>, tonic::Status> {
        let who = HostCaller::from_grpc_metadata(req.metadata()).map_err(|e| to_status(&e))?;
        let r = req.into_inner();
        let out = match self
            .0
            .host_spaces
            .call_thread_tool(&who, &r.space, &r.name, &r.arguments_json)
            .await
        {
            Ok(out) => out,
            // A refusal is a tool result, as the MCP server answers it.
            Err(e) => cua_spaces::mcp::ToolOutcome::error(&e),
        };
        Ok(tonic::Response::new(pb::CallThreadToolResponse {
            content_json: serde_json::Value::Array(out.content).to_string(),
            structured_json: out.structured.map(|s| s.to_string()).unwrap_or_default(),
            is_error: out.is_error,
            meta_json: out.meta.map(|m| m.to_string()).unwrap_or_default(),
        }))
    }
}

/// `HostJobsService`, over the same shared state.
pub(crate) struct HostJobsSvc(pub(crate) Arc<Shared>);

impl HostJobsSvc {
    /// The runner and the account the caller acts as: each account's jobs
    /// are its own on this host (see `NodeJobs::run_batch_for`).
    async fn jobs<T>(
        &self,
        req: &tonic::Request<T>,
    ) -> Result<(Arc<cua_spaces::host_jobs::NodeJobs>, String), tonic::Status> {
        let who = caller(req)?;
        let jobs = self
            .0
            .host_spaces
            .jobs_for(&who)
            .await
            .map_err(|e| to_status(&e))?;
        Ok((jobs, who.account))
    }

    /// The runner for a call that only reads or stops the caller's own jobs:
    /// it works while the host stops taking new ones (provide Spaces
    /// turned off drains it).
    async fn own_jobs<T>(
        &self,
        req: &tonic::Request<T>,
    ) -> Result<(Arc<cua_spaces::host_jobs::NodeJobs>, String), tonic::Status> {
        let who = caller(req)?;
        let jobs = self
            .0
            .host_spaces
            .jobs_of(&who)
            .await
            .map_err(|e| to_status(&e))?;
        Ok((jobs, who.account))
    }
}

#[tonic::async_trait]
impl HostJobsService for HostJobsSvc {
    async fn run_host_jobs(
        &self,
        req: tonic::Request<pb::RunHostJobsRequest>,
    ) -> Result<tonic::Response<pb::RunHostJobsResponse>, tonic::Status> {
        let (jobs, owner) = self.jobs(&req).await?;
        let r = req.into_inner();
        let acks = jobs.run_batch_for(&owner, r.jobs, &r.scheduler).await;
        Ok(tonic::Response::new(pb::RunHostJobsResponse {
            acks,
            node: self.0.host_spaces.node_status().await,
        }))
    }

    async fn get_host_job(
        &self,
        req: tonic::Request<pb::GetHostJobRequest>,
    ) -> Result<tonic::Response<pb::GetHostJobResponse>, tonic::Status> {
        let (jobs, owner) = self.own_jobs(&req).await?;
        let r = req.into_inner();
        let id = r.job_id;
        let job = jobs
            .get_for(&owner, &id, true)
            .ok_or_else(|| tonic::Status::not_found(format!("no job {id} on this host")))?;
        let (output_chunk, output_size) = r
            .output_from
            .and_then(|from| jobs.output_for(&owner, &id, from))
            .unwrap_or_default();
        Ok(tonic::Response::new(pb::GetHostJobResponse {
            job: Some(job),
            node: self.0.host_spaces.node_status().await,
            output_chunk,
            output_size,
        }))
    }

    async fn list_host_jobs(
        &self,
        req: tonic::Request<pb::ListHostJobsRequest>,
    ) -> Result<tonic::Response<pb::ListHostJobsResponse>, tonic::Status> {
        let (jobs, owner) = self.own_jobs(&req).await?;
        let r = req.into_inner();
        jobs.release(Some(&owner), &r.release);
        let mut out = jobs
            .list_for(
                &owner,
                r.after_seq,
                r.limit,
                Duration::from_millis(u64::from(r.wait_ms)),
                !r.without_output,
            )
            .await;
        out.node = self.0.host_spaces.node_status().await;
        Ok(tonic::Response::new(out))
    }

    async fn cancel_host_job(
        &self,
        req: tonic::Request<pb::CancelHostJobRequest>,
    ) -> Result<tonic::Response<pb::CancelHostJobResponse>, tonic::Status> {
        let (jobs, owner) = self.own_jobs(&req).await?;
        let r = req.into_inner();
        let job = jobs
            .cancel_for(&owner, &r.job_id, r.attempt, &r.reason)
            .await;
        Ok(tonic::Response::new(pb::CancelHostJobResponse {
            job: Some(job),
        }))
    }
}

/// `HostSnapshotsService`: this machine's checkpoints, and copying them
/// between your machines ([`cua_spaces::snapshots`]).
pub(crate) struct HostSnapshotsSvc(pub(crate) Arc<Shared>);

type SyncStream<T> =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<T, tonic::Status>> + Send>>;

#[tonic::async_trait]
impl HostSnapshotsService for HostSnapshotsSvc {
    async fn list_host_snapshots(
        &self,
        req: tonic::Request<pb::ListHostSnapshotsRequest>,
    ) -> Result<tonic::Response<pb::ListHostSnapshotsResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
            .snapshots_list(&who)
            .await
            .map(tonic::Response::new)
            .map_err(|e| to_status(&e))
    }

    async fn export_host_snapshot(
        &self,
        req: tonic::Request<pb::ExportHostSnapshotRequest>,
    ) -> Result<tonic::Response<pb::ExportHostSnapshotResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
            .snapshot_export(&who, req.into_inner())
            .await
            .map(tonic::Response::new)
            .map_err(|e| to_status(&e))
    }

    type PullHostSnapshotStream = SyncStream<pb::PullHostSnapshotResponse>;

    async fn pull_host_snapshot(
        &self,
        req: tonic::Request<pb::PullHostSnapshotRequest>,
    ) -> Result<tonic::Response<Self::PullHostSnapshotStream>, tonic::Status> {
        let who = caller(&req)?;
        let r = req.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let progress = tx.clone();
        let report: cua_snapshot_sync::ProgressFn = Arc::new(move |p| {
            let _ = progress.try_send(Ok(pb::PullHostSnapshotResponse {
                progress: Some(cua_spaces::snapshots::progress_to_pb(&p)),
            }));
        });
        let host = self.0.host_spaces.clone();
        // A copy runs to completion (or failure) even if the caller goes
        // away: a later batch finds the files here.
        tokio::spawn(async move {
            // The copy reports its own last message (done, verified).
            if let Err(e) = host.snapshot_pull(&who, r, Some(report)).await {
                let _ = tx.send(Err(to_status(&e))).await;
            }
        });
        Ok(tonic::Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    async fn create_host_snapshot(
        &self,
        req: tonic::Request<pb::CreateHostSnapshotRequest>,
    ) -> Result<tonic::Response<pb::CreateHostSnapshotResponse>, tonic::Status> {
        let who = caller(&req)?;
        let r = req.into_inner();
        let host = self.0.host_spaces.clone();
        // Like a local checkpoint, it runs to completion even if the caller
        // goes away (a paused VM must not stay paused).
        tokio::spawn(async move { host.snapshot_create(&who, r).await })
            .await
            .map_err(|e| tonic::Status::internal(format!("checkpoint task: {e}")))?
            .map(|s| tonic::Response::new(pb::CreateHostSnapshotResponse { snapshot: Some(s) }))
            .map_err(|e| to_status(&e))
    }

    async fn delete_host_snapshot(
        &self,
        req: tonic::Request<pb::DeleteHostSnapshotRequest>,
    ) -> Result<tonic::Response<pb::DeleteHostSnapshotResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
            .snapshot_delete(&who, req.into_inner())
            .await
            .map(|message| tonic::Response::new(pb::DeleteHostSnapshotResponse { message }))
            .map_err(|e| to_status(&e))
    }

    type PublishHostSnapshotStream = SyncStream<pb::PublishHostSnapshotResponse>;

    async fn publish_host_snapshot(
        &self,
        req: tonic::Request<pb::PublishHostSnapshotRequest>,
    ) -> Result<tonic::Response<Self::PublishHostSnapshotStream>, tonic::Status> {
        let who = caller(&req)?;
        let r = req.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let progress = tx.clone();
        let report: cua_snapshot_sync::ProgressFn = Arc::new(move |p| {
            let _ = progress.try_send(Ok(pb::PublishHostSnapshotResponse {
                progress: Some(cua_spaces::snapshots::progress_to_pb(&p)),
            }));
        });
        let host = self.0.host_spaces.clone();
        tokio::spawn(async move {
            if let Err(e) = host.snapshot_publish(&who, r, Some(report)).await {
                let _ = tx.send(Err(to_status(&e))).await;
            }
        });
        Ok(tonic::Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}
