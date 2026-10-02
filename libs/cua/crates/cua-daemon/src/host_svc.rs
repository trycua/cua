//! `cua.env.v1.HostSpacesService` in the daemon: the Spaces this machine
//! provides to its owner's devices ([`cua_spaces::host_spaces`]). The host's
//! cua-spacesd authenticates the relayed caller and forwards each call here
//! over the daemon's socket, with the caller in
//! [`cua_spaces::host_spaces::CALLER_METADATA`]; a call without it is this
//! machine's own (the owner).

use std::sync::Arc;

use cua_proto::env::v1::{self as pb, host_spaces_service_server::HostSpacesService};
use cua_spaces::host_spaces::{CALLER_METADATA, HostCaller, to_status};

use crate::server::Shared;

/// The service, over the daemon's shared state.
pub(crate) struct HostSvc(pub(crate) Arc<Shared>);

fn caller<T>(req: &tonic::Request<T>) -> Result<HostCaller, tonic::Status> {
    let value = req
        .metadata()
        .get(CALLER_METADATA)
        .map(|v| v.to_str())
        .transpose()
        .map_err(|_| tonic::Status::invalid_argument(format!("{CALLER_METADATA}: not text")))?;
    HostCaller::from_metadata(value).map_err(|e| to_status(&e))
}

#[tonic::async_trait]
impl HostSpacesService for HostSvc {
    async fn get_host_spaces(
        &self,
        req: tonic::Request<pb::GetHostSpacesRequest>,
    ) -> Result<tonic::Response<pb::GetHostSpacesResponse>, tonic::Status> {
        let who = caller(&req)?;
        self.0
            .host_spaces
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
}
