//! The API a Space reaches on its spawn channel: `SpaceService` and
//! `DaemonService` of the cua daemon, so `cua`, the SDKs and the agent's
//! tools work in a Space unchanged, but answered for this Space's own
//! subtree and nothing else.
//!
//! - Every call names Spaces inside the caller's subtree. A sibling, an
//!   ancestor, another tree or a guessed id is "not found", word for word
//!   what a missing Space gets.
//! - A create or fork makes a child of the caller: one level deeper (up to
//!   its tree's limit), on this machine, with the caller's egress rules,
//!   and a lifetime no longer than the caller's (4 hours by default).
//! - Everything else (other machines, clouds, volumes, shares, the
//!   daemon's own token and settings) is refused.
//!
//! The caller is the Space whose channel the request came in on (the host
//! opened it), and the capability it presents must be that Space's live
//! one ([`super::capability`]).

use std::sync::Arc;
use std::time::Duration;

use cua_proto::daemon::v1::{
    self as pb, daemon_service_server::DaemonService, space_service_server::SpaceService,
};
use serde_json::Value;
use tonic::{Request, Response, Status};

use super::capability::Claims;
use super::tree::{self, Tree};
use crate::server::{Shared, Svc};

type R<T> = std::result::Result<Response<T>, Status>;

/// Labels that carry the tree until the registry holds it.
pub const LABEL_PARENT: &str = "cua.parent";
/// The root of the tree.
pub const LABEL_ROOT: &str = "cua.root";
/// The depth (0 for a Space a person made).
pub const LABEL_DEPTH: &str = "cua.depth";

/// A child's lifetime when its parent has none.
pub const DEFAULT_CHILD_LIFETIME: Duration = Duration::from_secs(4 * 3600);

/// The one answer for a Space outside the caller's subtree.
pub fn not_found(query: &str) -> Status {
    Status::not_found(format!("no Space {query:?} among this Space's children"))
}

fn refused(what: &str) -> Status {
    Status::permission_denied(format!(
        "a Space can only {what}; ask the owner to do this outside the Space"
    ))
}

fn deny<T>() -> R<T> {
    Err(refused("create, list and manage its own children"))
}

/// The scoped services of one Space's channel.
#[derive(Clone)]
pub struct Scoped {
    pub(crate) shared: Arc<Shared>,
    /// The Space the channel belongs to.
    pub(crate) space: String,
}

/// The verified capability of a request (put there by the channel's auth).
fn claims<T>(req: &Request<T>) -> Result<Claims, Status> {
    req.extensions()
        .get::<Claims>()
        .cloned()
        .ok_or_else(|| Status::unauthenticated(super::capability::Refusal::Malformed.message()))
}

impl Scoped {
    fn owner(&self) -> Svc {
        Svc(self.shared.clone())
    }

    fn spaces(&self) -> &cua_spaces::Spaces {
        self.shared.runtime.spaces()
    }

    /// Parent links of every registered Space.
    pub(crate) fn links(&self) -> tree::Links {
        links_of(self.spaces())
    }

    /// The registered records below the caller.
    fn subtree_records(&self) -> Result<Vec<pb::Space>, Status> {
        let links = self.links();
        let below: std::collections::BTreeSet<String> =
            tree::subtree(&links, &self.space).into_iter().collect();
        let mut records: Vec<pb::Space> = self
            .spaces()
            .registry()
            .list()
            .map_err(|e| Status::internal(e.to_string()))?
            .into_iter()
            .filter(|r| below.contains(&r.id))
            .collect();
        for r in &mut records {
            (r.power, r.power_state) = self.spaces().power_of(&r.id);
            r.stop_reason = self.spaces().stop_reason_of(&r.id);
        }
        self.spaces().fill_records(&mut records);
        crate::spaces_svc::imp::mark_children(self.spaces(), &mut records);
        Ok(records)
    }

    /// The Space `query` names among the caller's descendants: by id, then
    /// by name. Nothing outside the subtree is ever matched.
    pub(crate) fn resolve(&self, query: &str) -> Result<pb::Space, Status> {
        let q = query.trim();
        if q.is_empty() {
            return Err(Status::invalid_argument("name a Space"));
        }
        let records = self.subtree_records()?;
        if let Some(r) = records.iter().find(|r| r.id == q) {
            return Ok(r.clone());
        }
        let named: Vec<&pb::Space> = records
            .iter()
            .filter(|r| r.name == q || (!r.display_name.is_empty() && r.display_name == q))
            .collect();
        match named.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(not_found(q)),
            many => Err(Status::failed_precondition(format!(
                "{q:?} names {} children: use an id ({})",
                many.len(),
                many.iter()
                    .map(|r| r.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// The caller's own record (its egress, lifetime).
    fn own_meta(&self) -> Option<cua_spaces::registry::SpaceMeta> {
        self.spaces().registry().meta(&self.space)
    }

    /// The request a create of a child becomes: refused when it asks for
    /// more than a child may have, otherwise placed under the caller.
    pub(crate) fn child_request(
        &self,
        claims: &Claims,
        mut r: pb::CreateSpaceRequest,
    ) -> Result<pb::CreateSpaceRequest, Status> {
        let depth = claims.depth.saturating_add(1);
        if depth > claims.max_depth {
            return Err(Status::resource_exhausted(format!(
                "this Space is {} level(s) below its tree's root and the tree allows {}: \
                 it cannot make children",
                claims.depth, claims.max_depth
            )));
        }
        // Whether it may make children of its own is the tree's depth's
        // call ([`Scoped::adopt`]), never the request's.
        r.children = None;
        let location = r.location.trim().to_owned();
        match location.as_str() {
            "" | "local" => r.location = "local".into(),
            other => {
                return Err(Status::failed_precondition(format!(
                    "a child runs on the same machine as its parent; on={other:?} is not \
                     available to children yet"
                )));
            }
        }
        if r.reuse {
            return Err(Status::invalid_argument(
                "a child is always a new Space (reuse is not available here)",
            ));
        }
        if r.durable {
            return Err(Status::invalid_argument(
                "a child lives as long as its parent allows (durable is not available here)",
            ));
        }
        if !r.volumes.is_empty() {
            return Err(refused(
                "mount volumes in children that it was given itself",
            ));
        }
        if !r.instance_type.is_empty() || !r.region.is_empty() {
            return Err(Status::failed_precondition(
                "a child runs on the same machine as its parent (no cloud machine type or region)",
            ));
        }
        if let Some(k) = r.labels.keys().find(|k| k.starts_with("cua.")) {
            return Err(Status::invalid_argument(format!(
                "label {k:?} is reserved (labels starting with \"cua.\" are set by Cua)"
            )));
        }
        // Egress: the parent's rules, never wider.
        let own = self.own_meta().map(|m| m.egress).unwrap_or_default();
        let asked = r.egress.take().map(|e| cua_spaces::egress::EgressPolicy {
            allow: e.allow,
            deny: e.deny,
        });
        let egress = match asked {
            None => own,
            Some(a) if a.is_empty() => own,
            Some(a) if own.is_empty() || a == own => a,
            Some(_) => {
                return Err(Status::permission_denied(
                    "a child keeps its parent's egress rules; leave egress empty",
                ));
            }
        };
        if !egress.is_empty() {
            r.egress = Some(pb::EgressPolicy {
                allow: egress.allow,
                deny: egress.deny,
            });
        }
        if r.name.trim().is_empty() {
            r.name = self.next_child_name("c");
        }
        r.labels.insert(LABEL_PARENT.into(), self.space.clone());
        r.labels.insert(LABEL_ROOT.into(), claims.root.clone());
        r.labels.insert(LABEL_DEPTH.into(), depth.to_string());
        Ok(r)
    }

    /// `<parent name>.<prefix><n>`, the first one not taken.
    fn next_child_name(&self, prefix: &str) -> String {
        let base = self
            .space
            .split_once(':')
            .map(|(_, n)| n)
            .unwrap_or(&self.space)
            .to_owned();
        let taken: std::collections::BTreeSet<String> = self
            .spaces()
            .registry()
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.name)
            .collect();
        (1..)
            .map(|n| format!("{base}.{prefix}{n}"))
            .find(|n| !taken.contains(n))
            .expect("an unused name")
    }

    /// After a child exists: its lifetime (no longer than the caller's) and,
    /// when the tree allows another level, its own children turned on.
    pub(crate) fn adopt(&self, claims: &Claims, child: &str) {
        let spaces = self.spaces();
        let remaining = spaces.lifetime(&self.space).map(|l| {
            l.expires_at()
                .duration_since(std::time::SystemTime::now())
                .unwrap_or_default()
        });
        let life = remaining
            .map(|r| r.min(DEFAULT_CHILD_LIFETIME))
            .unwrap_or(DEFAULT_CHILD_LIFETIME)
            .max(Duration::from_secs(60));
        if let Err(e) =
            spaces.set_lifetime(child, life, Some(cua_sandbox_core::LocalExpiry::Delete))
        {
            tracing::warn!(child, error = %e, "could not set a child's lifetime");
        }
        if claims.depth.saturating_add(1) < claims.max_depth
            && let Err(e) = spaces.set_children(child, true)
        {
            tracing::warn!(child, error = %e, "could not turn on a child's own children");
        }
    }

    /// Deletes `id` and everything below it, deepest first.
    async fn delete_tree(&self, id: &str) -> Result<String, Status> {
        let links = self.links();
        let mut order = tree::subtree(&links, id);
        order.push(id.to_owned());
        let mut last = String::new();
        for victim in order {
            let r = self
                .owner()
                .delete_space(Request::new(pb::DeleteSpaceRequest {
                    space: victim.clone(),
                }))
                .await;
            if let Some(a) = self.shared.spawn_authority() {
                a.revoke(&victim);
            }
            let _ = self.spaces().set_children(&victim, false);
            match r {
                Ok(m) => last = m.into_inner().message,
                Err(s) if s.code() == tonic::Code::NotFound => {}
                Err(s) => return Err(s),
            }
        }
        Ok(last)
    }

    /// The tools a Space may call on its children, and the argument that
    /// names the child.
    fn tool_target(name: &str) -> Option<&'static str> {
        Some(match name {
            "stop_space" | "start_space" | "space_bash" | "space_write" | "upload" | "download"
            | "list_tools" | "call_tool" | "list_space_windows" => "space",
            _ => return None,
        })
    }

    /// Rewrites a tool call's Space argument to the child's id, refusing
    /// tools that are not a child's and Spaces outside the subtree.
    pub(crate) fn scoped_tool_args(&self, name: &str, args: &mut Value) -> Result<(), Status> {
        let Some(key) = Self::tool_target(name) else {
            return Err(refused("use its children's own tools"));
        };
        let q = args
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| Status::invalid_argument(format!("{name} needs `{key}`")))?
            .to_owned();
        let child = self.resolve(&q)?;
        args[key] = Value::String(child.id);
        Ok(())
    }
}

/// Parent links from the registry's records.
pub(crate) fn links_of(spaces: &cua_spaces::Spaces) -> tree::Links {
    let meta = spaces.registry().all_meta();
    let ids: Vec<String> = spaces
        .registry()
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.id)
        .collect();
    tree::Links(
        ids.into_iter()
            .map(|id| {
                let parent = meta
                    .get(&id)
                    .and_then(|m| m.labels.get(LABEL_PARENT).cloned());
                (id, parent)
            })
            .collect(),
    )
}

/// The tree position of `id`: (root, depth). A Space a person made is its
/// own root at depth 0.
pub(crate) fn position(spaces: &cua_spaces::Spaces, id: &str) -> (String, u32) {
    let meta = spaces.registry().meta(id);
    let label = |k: &str| meta.as_ref().and_then(|m| m.labels.get(k).cloned());
    match (
        label(LABEL_ROOT),
        label(LABEL_DEPTH).and_then(|d| d.parse().ok()),
    ) {
        (Some(root), Some(depth)) if label(LABEL_PARENT).is_some() => (root, depth),
        _ => (id.to_owned(), 0),
    }
}

#[tonic::async_trait]
impl SpaceService for Scoped {
    async fn add_space(&self, _: Request<pb::AddSpaceRequest>) -> R<pb::AddSpaceResponse> {
        deny()
    }

    async fn list_spaces(&self, _: Request<pb::ListSpacesRequest>) -> R<pb::ListSpacesResponse> {
        Ok(Response::new(pb::ListSpacesResponse {
            spaces: self.subtree_records()?,
            ..Default::default()
        }))
    }

    async fn remove_space(&self, _: Request<pb::RemoveSpaceRequest>) -> R<pb::RemoveSpaceResponse> {
        deny()
    }

    async fn list_volumes(&self, _: Request<pb::ListVolumesRequest>) -> R<pb::ListVolumesResponse> {
        deny()
    }

    async fn create_volume(
        &self,
        _: Request<pb::CreateVolumeRequest>,
    ) -> R<pb::CreateVolumeResponse> {
        deny()
    }

    async fn delete_volume(
        &self,
        _: Request<pb::DeleteVolumeRequest>,
    ) -> R<pb::DeleteVolumeResponse> {
        deny()
    }

    async fn resolve_space(
        &self,
        req: Request<pb::ResolveSpaceRequest>,
    ) -> R<pb::ResolveSpaceResponse> {
        let space = self.resolve(&req.into_inner().space)?;
        Ok(Response::new(pb::ResolveSpaceResponse {
            space: Some(space),
        }))
    }

    async fn provision_local_space(
        &self,
        _: Request<pb::ProvisionLocalSpaceRequest>,
    ) -> R<pb::ProvisionLocalSpaceResponse> {
        deny()
    }

    async fn release_space(
        &self,
        _: Request<pb::ReleaseSpaceRequest>,
    ) -> R<pb::ReleaseSpaceResponse> {
        deny()
    }

    async fn create_space(
        &self,
        req: Request<pb::CreateSpaceRequest>,
    ) -> R<pb::CreateSpaceResponse> {
        let claims = claims(&req)?;
        let child = self.child_request(&claims, req.into_inner())?;
        let resp = self.owner().create_space(Request::new(child)).await?;
        if let Some(s) = &resp.get_ref().space {
            self.adopt(&claims, &s.id);
        }
        Ok(resp)
    }

    type CreateSpaceStreamStream = <Svc as SpaceService>::CreateSpaceStreamStream;

    async fn create_space_stream(
        &self,
        req: Request<pb::CreateSpaceStreamRequest>,
    ) -> R<Self::CreateSpaceStreamStream> {
        use tokio_stream::StreamExt as _;
        let claims = claims(&req)?;
        let inner = req.into_inner();
        let child = self.child_request(&claims, inner.create.unwrap_or_default())?;
        let stream = self
            .owner()
            .create_space_stream(Request::new(pb::CreateSpaceStreamRequest {
                create: Some(child),
            }))
            .await?
            .into_inner();
        let me = self.clone();
        let adopted = stream.map(move |item| {
            if let Ok(pb::CreateSpaceStreamResponse {
                event: Some(pb::create_space_stream_response::Event::Result(r)),
            }) = &item
                && let Some(s) = &r.space
            {
                me.adopt(&claims, &s.id);
            }
            item
        });
        Ok(Response::new(Box::pin(adopted)))
    }

    async fn cancel_create_space(
        &self,
        req: Request<pb::CancelCreateSpaceRequest>,
    ) -> R<pb::CancelCreateSpaceResponse> {
        let space = self.resolve(&req.into_inner().space)?;
        self.owner()
            .cancel_create_space(Request::new(pb::CancelCreateSpaceRequest {
                space: space.id,
            }))
            .await
    }

    async fn get_gpu_support(
        &self,
        _: Request<pb::GetGpuSupportRequest>,
    ) -> R<pb::GetGpuSupportResponse> {
        deny()
    }

    async fn delete_space(
        &self,
        req: Request<pb::DeleteSpaceRequest>,
    ) -> R<pb::DeleteSpaceResponse> {
        let space = self.resolve(&req.into_inner().space)?;
        let message = self.delete_tree(&space.id).await?;
        Ok(Response::new(pb::DeleteSpaceResponse { message }))
    }

    async fn delete_spaces(
        &self,
        req: Request<pb::DeleteSpacesRequest>,
    ) -> R<pb::DeleteSpacesResponse> {
        let mut results = Vec::new();
        for q in req.into_inner().spaces {
            let r = match self.resolve(&q) {
                Ok(s) => self.delete_tree(&s.id).await,
                Err(e) => Err(e),
            };
            results.push(match r {
                Ok(message) => pb::SpaceBulkResult {
                    space: q,
                    ok: true,
                    message,
                    ..Default::default()
                },
                Err(e) => pb::SpaceBulkResult {
                    space: q,
                    ok: false,
                    error: e.message().to_owned(),
                    ..Default::default()
                },
            });
        }
        Ok(Response::new(pb::DeleteSpacesResponse { results }))
    }

    async fn leave_spaces(&self, _: Request<pb::LeaveSpacesRequest>) -> R<pb::LeaveSpacesResponse> {
        deny()
    }

    async fn rename_space(
        &self,
        req: Request<pb::RenameSpaceRequest>,
    ) -> R<pb::RenameSpaceResponse> {
        let r = req.into_inner();
        let space = self.resolve(&r.space)?;
        self.owner()
            .rename_space(Request::new(pb::RenameSpaceRequest {
                space: space.id,
                name: r.name,
            }))
            .await
    }

    async fn fork_space(&self, req: Request<pb::ForkSpaceRequest>) -> R<pb::ForkSpaceResponse> {
        let claims = claims(&req)?;
        let r = req.into_inner();
        if claims.depth.saturating_add(1) > claims.max_depth {
            return Err(Status::resource_exhausted(format!(
                "this Space is {} level(s) below its tree's root and the tree allows {}: \
                 it cannot make children",
                claims.depth, claims.max_depth
            )));
        }
        // A Space may fork itself or any of its descendants.
        let source = if r.source.trim().is_empty() || r.source == self.space {
            self.space.clone()
        } else {
            self.resolve(&r.source)?.id
        };
        let name = if r.name.trim().is_empty() {
            self.next_child_name("f")
        } else {
            r.name
        };
        let resp = self
            .owner()
            .fork_space(Request::new(pb::ForkSpaceRequest { source, name }))
            .await?;
        if let Some(s) = &resp.get_ref().space {
            // A fork starts with its source's labels: it is the caller's
            // child whatever it was forked from.
            let depth = claims.depth.saturating_add(1);
            let _ = self.spaces().registry().update_meta(&s.id, |m| {
                m.labels.insert(LABEL_PARENT.into(), self.space.clone());
                m.labels.insert(LABEL_ROOT.into(), claims.root.clone());
                m.labels.insert(LABEL_DEPTH.into(), depth.to_string());
            });
            self.adopt(&claims, &s.id);
        }
        Ok(resp)
    }

    type BuildSpaceStream = <Svc as SpaceService>::BuildSpaceStream;

    async fn build_space(&self, _: Request<pb::BuildSpaceRequest>) -> R<Self::BuildSpaceStream> {
        deny()
    }

    async fn list_builds(&self, _: Request<pb::ListBuildsRequest>) -> R<pb::ListBuildsResponse> {
        deny()
    }

    async fn delete_build(&self, _: Request<pb::DeleteBuildRequest>) -> R<pb::DeleteBuildResponse> {
        deny()
    }

    async fn set_space_lifetime(
        &self,
        req: Request<pb::SetSpaceLifetimeRequest>,
    ) -> R<pb::SetSpaceLifetimeResponse> {
        let mut r = req.into_inner();
        let space = self.resolve(&r.space)?;
        // Never past the caller's own end.
        if let (Some(own), Some(d)) = (self.spaces().lifetime(&self.space), r.duration.as_ref()) {
            let left = own
                .expires_at()
                .duration_since(std::time::SystemTime::now())
                .unwrap_or_default();
            let asked = Duration::from_secs(u64::try_from(d.seconds).unwrap_or(0));
            if asked > left {
                return Err(Status::permission_denied(format!(
                    "a child cannot outlive its parent: this Space ends in {}s",
                    left.as_secs()
                )));
            }
        }
        r.space = space.id;
        self.owner().set_space_lifetime(Request::new(r)).await
    }

    async fn set_space_egress(
        &self,
        _: Request<pb::SetSpaceEgressRequest>,
    ) -> R<pb::SetSpaceEgressResponse> {
        Err(refused(
            "give children its own egress rules (set them on the parent)",
        ))
    }

    async fn list_orphans(&self, _: Request<pb::ListOrphansRequest>) -> R<pb::ListOrphansResponse> {
        deny()
    }

    async fn set_space_children(
        &self,
        req: Request<pb::SetSpaceChildrenRequest>,
    ) -> R<pb::SetSpaceChildrenResponse> {
        // A Space may turn its children's own children off, never on: the
        // tree's depth decides that.
        let r = req.into_inner();
        let mut space = self.resolve(&r.space)?;
        if r.enabled {
            return Err(refused(
                "let its children make children as deep as its tree allows (that is automatic)",
            ));
        }
        self.spaces()
            .set_children(&space.id, false)
            .map_err(|e| Status::internal(e.to_string()))?;
        if let Some(a) = self.shared.spawn_authority() {
            a.revoke(&space.id);
        }
        space.children = false;
        Ok(Response::new(pb::SetSpaceChildrenResponse {
            space: Some(space),
        }))
    }

    async fn connect_space(
        &self,
        req: Request<pb::ConnectSpaceRequest>,
    ) -> R<pb::ConnectSpaceResponse> {
        // The child's commands go through this channel with the caller's
        // own capability.
        let bearer = req
            .extensions()
            .get::<super::Presented>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let origin = req
            .extensions()
            .get::<super::Origin>()
            .map(|o| o.0.clone())
            .unwrap_or_default();
        let space = self.resolve(&req.into_inner().space)?;
        let mut resp = self
            .owner()
            .connect_space(Request::new(pb::ConnectSpaceRequest {
                space: space.id.clone(),
            }))
            .await?
            .into_inner();
        // Through this channel's own passthrough, never the daemon's
        // loopback listener, its token or another machine's route.
        let key = crate::passthrough::space_key(&space.id);
        resp.env_url = if resp.env_url.is_empty() || origin.is_empty() {
            String::new()
        } else {
            format!("http://{origin}/v1/spaces/{key}/env")
        };
        resp.token = bearer;
        resp.services.clear();
        resp.bulk = String::new();
        Ok(Response::new(resp))
    }

    async fn list_space_tools(
        &self,
        _: Request<pb::ListSpaceToolsRequest>,
    ) -> R<pb::ListSpaceToolsResponse> {
        let all = self
            .owner()
            .list_space_tools(Request::new(pb::ListSpaceToolsRequest {}))
            .await?
            .into_inner();
        let tools: Vec<Value> = serde_json::from_str(&all.tools_json).unwrap_or_default();
        let kept: Vec<Value> = tools
            .into_iter()
            .filter(|t| {
                t.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| Self::tool_target(n).is_some())
            })
            .collect();
        Ok(Response::new(pb::ListSpaceToolsResponse {
            tools_json: Value::Array(kept).to_string(),
        }))
    }

    async fn call_space_tool(
        &self,
        req: Request<pb::CallSpaceToolRequest>,
    ) -> R<pb::CallSpaceToolResponse> {
        let r = req.into_inner();
        let mut args: Value = if r.arguments_json.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(&r.arguments_json)
                .map_err(|e| Status::invalid_argument(format!("arguments_json: {e}")))?
        };
        self.scoped_tool_args(&r.name, &mut args)?;
        self.owner()
            .call_space_tool(Request::new(pb::CallSpaceToolRequest {
                name: r.name,
                arguments_json: args.to_string(),
            }))
            .await
    }

    async fn list_hosts(&self, _: Request<pb::ListHostsRequest>) -> R<pb::ListHostsResponse> {
        deny()
    }

    async fn pool_request(&self, _: Request<pb::PoolRequestRequest>) -> R<pb::PoolRequestResponse> {
        deny()
    }

    async fn get_space_thumbnail(
        &self,
        req: Request<pb::GetSpaceThumbnailRequest>,
    ) -> R<pb::GetSpaceThumbnailResponse> {
        let mut r = req.into_inner();
        r.space = self.resolve(&r.space)?.id;
        self.owner().get_space_thumbnail(Request::new(r)).await
    }

    async fn get_space_route(
        &self,
        req: Request<pb::GetSpaceRouteRequest>,
    ) -> R<pb::GetSpaceRouteResponse> {
        let mut r = req.into_inner();
        r.space = self.resolve(&r.space)?.id;
        self.owner().get_space_route(Request::new(r)).await
    }

    async fn get_resources(
        &self,
        _: Request<pb::GetResourcesRequest>,
    ) -> R<pb::GetResourcesResponse> {
        deny()
    }

    type WatchResourcesStream = <Svc as SpaceService>::WatchResourcesStream;

    async fn watch_resources(
        &self,
        _: Request<pb::WatchResourcesRequest>,
    ) -> R<Self::WatchResourcesStream> {
        deny()
    }

    type WatchSpacesStream = <Svc as SpaceService>::WatchSpacesStream;

    async fn watch_spaces(&self, _: Request<pb::WatchSpacesRequest>) -> R<Self::WatchSpacesStream> {
        deny()
    }
}

#[tonic::async_trait]
impl DaemonService for Scoped {
    async fn get_info(&self, _: Request<pb::GetInfoRequest>) -> R<pb::GetInfoResponse> {
        // Who answers, and nothing of the daemon's own (no token, socket
        // or loopback address).
        Ok(Response::new(pb::GetInfoResponse {
            version: crate::VERSION.into(),
            features: vec!["spaces".into(), super::CHILDREN_FEATURE.into()],
            ..Default::default()
        }))
    }

    async fn open_media_bridge(
        &self,
        _: Request<pb::OpenMediaBridgeRequest>,
    ) -> R<pb::OpenMediaBridgeResponse> {
        deny()
    }

    async fn shutdown(&self, _: Request<pb::ShutdownRequest>) -> R<pb::ShutdownResponse> {
        deny()
    }
}
