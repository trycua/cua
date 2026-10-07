//! `cua.daemon.v1.SpaceService`: a thin adapter over the runtime's
//! [`cua_spaces::Spaces`]. The registry, Fleet claims, local provisioning
//! and every MCP tool are `cua-spaces` code; this module only converts
//! messages. Without the `spaces` feature every call is `Unsupported`.

use crate::{
    Error,
    server::{R, Svc, st},
};
use cua_proto::daemon::v1::{self as pb, space_service_server::SpaceService};

#[cfg(feature = "spaces")]
mod imp {
    use super::*;
    use crate::convert::from_dur;
    use crate::passthrough;
    use crate::server::ok;
    use cua_sandbox_core::placement::{Kind, On, Runtime};
    use prost::Message;

    fn record(spaces: &cua_spaces::Spaces, id: &str) -> Result<pb::Space, Error> {
        if let Some(r) = spaces.registry().get(id).map_err(Error::from)? {
            return Ok(r);
        }
        // Relay machines are not registered; they come from the account's
        // directory listing.
        spaces
            .list()
            .map_err(Error::from)?
            .iter()
            .find(|i| i.id == id)
            .map(info_record)
            .ok_or_else(|| Error::NotFound(format!("Space {id}")))
    }

    /// A relay machine (or any listed Space) as a SpaceService record.
    fn info_record(info: &cua_spaces::SpaceInfo) -> pb::Space {
        pb::Space {
            id: info.id.clone(),
            name: info.name.clone(),
            spacesd_version: info.spacesd_version.clone(),
            features: info.features.clone(),
            os: info.os.clone(),
            os_name: info.os_name.clone(),
            os_pretty_name: info.os_pretty_name.clone(),
            image: info.image.clone(),
            image_digest: info.image_digest.clone(),
            kind: info.kind.clone(),
            arch: info.arch.clone(),
            services: info.services.clone(),
            added_at: None,
            host: info.host.clone(),
            host_name: info.host_name.clone(),
            power: info.power.clone(),
            power_state: info.power_state.clone(),
            cloud: info.cloud.clone(),
            cloud_place: info.cloud_place.clone(),
            cloud_delete: info.cloud_delete.clone(),
        }
    }

    fn opt(s: String) -> Option<String> {
        (!s.is_empty()).then_some(s)
    }

    /// A `CreateSpaceRequest` as the runtime's create options.
    fn create_args(r: pb::CreateSpaceRequest) -> Result<cua_spaces::SpaceCreate, tonic::Status> {
        let on = opt(r.location)
            .map(|l| On::parse(&l))
            .transpose()
            .map_err(|e| st(e.into()))?;
        let kind = Kind::parse(&r.kind).map_err(|e| st(e.into()))?;
        let runtime = Runtime::parse(&r.runtime).map_err(|e| st(e.into()))?;
        Ok(cua_spaces::SpaceCreate {
            image: opt(r.image),
            on,
            kind,
            runtime,
            name: opt(r.name),
            cpus: (r.cpus > 0).then_some(r.cpus),
            memory_mb: (r.memory_mb > 0).then_some(r.memory_mb),
            disk_gb: (r.disk_gb > 0).then_some(r.disk_gb),
            timeout: r.timeout.as_ref().map(from_dur),
            wait: r.wait,
            reuse: r.reuse,
            command: (!r.command.is_empty()).then_some(r.command),
            env: r.env.into_iter().collect(),
            services: ports(r.services).map_err(st)?,
            spacesd: r.spacesd,
            env_token: None,
            progress: None,
            create_id: opt(r.create_id),
            gpu: opt(r.gpu),
        })
    }

    /// A runtime's GPU options, as the wire sends them.
    pub(crate) fn gpu_support_pb(g: cua_sandbox_core::gpu::GpuSupport) -> pb::GpuSupport {
        pb::GpuSupport {
            runtime: g.runtime,
            options: g
                .options
                .into_iter()
                .map(|o| pb::GpuOption {
                    id: o.id,
                    label: o.label,
                    experimental: o.experimental,
                    supported: o.supported,
                    reason: o.reason,
                    learn_more: o.learn_more.unwrap_or_default(),
                    usd_per_hour: o.usd_per_hour,
                })
                .collect(),
            reason: g.reason,
        }
    }

    /// What a create returned, as the wire response.
    fn create_response(
        spaces: &cua_spaces::Spaces,
        created: cua_spaces::SpaceCreated,
    ) -> Result<pb::CreateSpaceResponse, tonic::Status> {
        Ok(match created {
            cua_spaces::SpaceCreated::Ready { info, reused } => pb::CreateSpaceResponse {
                space: Some(record(spaces, &info.id).map_err(st)?),
                phase: "ready".into(),
                reused,
            },
            cua_spaces::SpaceCreated::Starting(p) => pb::CreateSpaceResponse {
                space: Some(pb::Space {
                    id: p.id,
                    ..Default::default()
                }),
                phase: p.phase.into(),
                reused: false,
            },
        })
    }

    /// Wire service ports (`uint32`) to guest ports.
    fn ports(
        m: std::collections::HashMap<String, u32>,
    ) -> Result<std::collections::BTreeMap<String, u16>, Error> {
        m.into_iter()
            .map(|(k, v)| {
                u16::try_from(v)
                    .ok()
                    .filter(|p| *p > 0)
                    .map(|p| (k.clone(), p))
                    .ok_or_else(|| Error::InvalidArgument(format!("service {k}: bad port {v}")))
            })
            .collect()
    }

    impl Svc {
        fn spaces(&self) -> &cua_spaces::Spaces {
            self.rt().spaces()
        }

        /// The Spaces MCP server, with the Keyvault broker wired into
        /// `teleport_app` when the daemon hosts one.
        fn mcp(&self) -> cua_spaces::mcp::McpServer {
            let server = cua_spaces::mcp::McpServer::new(self.spaces().clone());
            match self.rt().session_broker() {
                Some(broker) => server.with_session_broker(broker),
                None => server,
            }
        }
    }

    #[tonic::async_trait]
    impl SpaceService for Svc {
        async fn add_space(
            &self,
            req: tonic::Request<pb::AddSpaceRequest>,
        ) -> R<pb::AddSpaceResponse> {
            let r = req.into_inner();
            let info = self
                .spaces()
                .add_with_service(&r.url, opt(r.token), opt(r.name), opt(r.service))
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::AddSpaceResponse {
                space: Some(record(self.spaces(), &info.id).map_err(st)?),
            })
        }

        async fn list_spaces(
            &self,
            _: tonic::Request<pb::ListSpacesRequest>,
        ) -> R<pb::ListSpacesResponse> {
            let mut spaces = self.spaces().registry().list().map_err(|e| st(e.into()))?;
            for s in &mut spaces {
                (s.power, s.power_state) = self.spaces().power_of(&s.id);
            }
            // Plus the signed-in account's relay machines (refreshed).
            for info in self.spaces().list_all().await.map_err(|e| st(e.into()))? {
                if info.provider == cua_spaces::Provider::Relay
                    && !spaces.iter().any(|s| s.id == info.id)
                {
                    spaces.push(info_record(&info));
                }
            }
            ok(pb::ListSpacesResponse { spaces })
        }

        async fn remove_space(
            &self,
            req: tonic::Request<pb::RemoveSpaceRequest>,
        ) -> R<pb::RemoveSpaceResponse> {
            self.spaces()
                .remove(&req.into_inner().id)
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::RemoveSpaceResponse {})
        }

        async fn resolve_space(
            &self,
            req: tonic::Request<pb::ResolveSpaceRequest>,
        ) -> R<pb::ResolveSpaceResponse> {
            let id = self
                .spaces()
                .resolve(&req.into_inner().space)
                .map_err(|e| st(e.into()))?;
            ok(pb::ResolveSpaceResponse {
                space: Some(record(self.spaces(), &id.to_string()).map_err(st)?),
            })
        }

        async fn claim_fleet_space(
            &self,
            req: tonic::Request<pb::ClaimFleetSpaceRequest>,
        ) -> R<pb::ClaimFleetSpaceResponse> {
            let r = req.into_inner();
            let runtime = match opt(r.runtime) {
                Some(v) => {
                    match cua_spaces::fleet_runtime::parse_runtime(&v).map_err(|e| st(e.into()))? {
                        cua_spaces::contract::inputs::FleetRuntime::Gvisor => Runtime::Gvisor,
                        cua_spaces::contract::inputs::FleetRuntime::Kubevirt => Runtime::Kubevirt,
                    }
                }
                None => Runtime::Auto,
            };
            let created = self
                .spaces()
                .create(cua_spaces::SpaceCreate {
                    on: Some(On::Cloud),
                    image: opt(r.image),
                    runtime,
                    name: opt(r.name),
                    wait: r.wait,
                    reuse: r.reuse,
                    command: (!r.command.is_empty()).then_some(r.command),
                    env: r.env.into_iter().collect(),
                    services: ports(r.services).map_err(st)?,
                    spacesd: r.spacesd,
                    ..Default::default()
                })
                .await
                .map_err(|e| st(e.into()))?;
            match created {
                cua_spaces::SpaceCreated::Ready { info, reused } => {
                    ok(pb::ClaimFleetSpaceResponse {
                        space: Some(record(self.spaces(), &info.id).map_err(st)?),
                        pending_json: String::new(),
                        reused,
                    })
                }
                cua_spaces::SpaceCreated::Starting(pending) => ok(pb::ClaimFleetSpaceResponse {
                    space: None,
                    pending_json: serde_json::to_string(&pending)
                        .map_err(|e| st(Error::Internal(e.to_string())))?,
                    reused: false,
                }),
            }
        }

        async fn provision_local_space(
            &self,
            req: tonic::Request<pb::ProvisionLocalSpaceRequest>,
        ) -> R<pb::ProvisionLocalSpaceResponse> {
            let r = req.into_inner();
            let info = self
                .spaces()
                .create(cua_spaces::SpaceCreate {
                    on: Some(On::Local),
                    image: opt(r.image),
                    name: opt(r.name),
                    cpus: (r.cpus > 0).then_some(r.cpus),
                    memory_mb: (r.memory_mb > 0).then_some(r.memory_mb),
                    timeout: r.timeout.as_ref().map(from_dur),
                    command: (!r.command.is_empty()).then_some(r.command),
                    env: r.env.into_iter().collect(),
                    services: ports(r.services).map_err(st)?,
                    spacesd: r.spacesd,
                    ..Default::default()
                })
                .await
                .map_err(|e| st(e.into()))?
                .ready()
                .map_err(|p| st(Error::Internal(format!("{} is still starting", p.id))))?;
            ok(pb::ProvisionLocalSpaceResponse {
                space: Some(record(self.spaces(), &info.id).map_err(st)?),
            })
        }

        async fn release_space(
            &self,
            req: tonic::Request<pb::ReleaseSpaceRequest>,
        ) -> R<pb::ReleaseSpaceResponse> {
            let message = self
                .spaces()
                .delete(&req.into_inner().space)
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::ReleaseSpaceResponse { message })
        }

        async fn create_space(
            &self,
            req: tonic::Request<pb::CreateSpaceRequest>,
        ) -> R<pb::CreateSpaceResponse> {
            let create = create_args(req.into_inner())?;
            let created = self
                .spaces()
                .create(create)
                .await
                .map_err(|e| st(e.into()))?;
            ok(create_response(self.spaces(), created)?)
        }

        type CreateSpaceStreamStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = std::result::Result<pb::CreateSpaceStreamResponse, tonic::Status>,
                    > + Send,
            >,
        >;

        async fn create_space_stream(
            &self,
            req: tonic::Request<pb::CreateSpaceStreamRequest>,
        ) -> R<Self::CreateSpaceStreamStream> {
            use pb::create_space_stream_response::Event;
            let mut r = req.into_inner().create.unwrap_or_default();
            // The stream runs until the Space is ready.
            r.wait = Some(true);
            let mut create = create_args(r)?;
            // Bounded: a slow reader drops progress, never the result (which
            // goes out on the same channel after every report).
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            let progress = tx.clone();
            create.progress = Some(cua_spaces::ProgressSink::new(move |p| {
                let _ = progress.try_send(Ok(pb::CreateSpaceStreamResponse {
                    event: Some(Event::Progress(pb::CreateSpaceProgress {
                        phase: p.phase.as_str().into(),
                        fraction: p.fraction,
                        detail: p.detail.clone(),
                        bytes_done: p.bytes.map(|b| b.done),
                        bytes_total: p.bytes.map(|b| b.total).filter(|t| *t > 0),
                        bytes_per_second: p.bytes.and_then(|b| b.per_second),
                        space: p.target.clone(),
                    })),
                }));
            }));
            let spaces = self.spaces().clone();
            tokio::spawn(async move {
                let result = match spaces.create(create).await {
                    Ok(created) => {
                        create_response(&spaces, created).map(|r| pb::CreateSpaceStreamResponse {
                            event: Some(Event::Result(r)),
                        })
                    }
                    Err(e) => Err(st(e.into())),
                };
                let _ = tx.send(result).await;
            });
            ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }

        async fn cancel_create_space(
            &self,
            req: tonic::Request<pb::CancelCreateSpaceRequest>,
        ) -> R<pb::CancelCreateSpaceResponse> {
            let o = self
                .spaces()
                .cancel_create(&req.into_inner().space)
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::CancelCreateSpaceResponse {
                id: o.id,
                state: o.state.as_str().into(),
                message: o.message,
            })
        }

        async fn get_gpu_support(
            &self,
            req: tonic::Request<pb::GetGpuSupportRequest>,
        ) -> R<pb::GetGpuSupportResponse> {
            let location = req.into_inner().location;
            let on = if location.trim().is_empty() {
                On::Local
            } else {
                On::parse(&location).map_err(|e| st(e.into()))?
            };
            let runtimes = self
                .spaces()
                .gpu_support(&on)
                .await
                .into_iter()
                .map(gpu_support_pb)
                .collect();
            ok(pb::GetGpuSupportResponse { runtimes })
        }

        async fn delete_space(
            &self,
            req: tonic::Request<pb::DeleteSpaceRequest>,
        ) -> R<pb::DeleteSpaceResponse> {
            let message = self
                .spaces()
                .delete(&req.into_inner().space)
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::DeleteSpaceResponse { message })
        }

        async fn connect_space(
            &self,
            req: tonic::Request<pb::ConnectSpaceRequest>,
        ) -> R<pb::ConnectSpaceResponse> {
            let space = self
                .spaces()
                .space(&req.into_inner().space)
                .await
                .map_err(|e| st(e.into()))?;
            let id = space.id().to_string();
            let (base, token) = {
                let info = self.0.info.lock().unwrap();
                (info.loopback_url.clone(), info.loopback_token.clone())
            };
            if base.is_empty() {
                return Err(st(Error::Unsupported(
                    "the Space env passthrough needs the daemon's loopback listener".into(),
                )));
            }
            let key = passthrough::space_key(&id);
            let services = space
                .declared_services()
                .iter()
                .map(|(name, svc)| pb::SpaceServiceEndpoint {
                    name: name.clone(),
                    url: format!(
                        "{base}/v1/spaces/{key}/svc/{}",
                        passthrough::encode_segment(name)
                    ),
                    mcp_path: svc.mcp_path.clone(),
                })
                .collect();
            ok(pb::ConnectSpaceResponse {
                space: Some(record(self.spaces(), &id).map_err(st)?),
                env_url: if space.has_spacesd() {
                    format!("{base}/v1/spaces/{key}/env")
                } else {
                    String::new()
                },
                token,
                capabilities: space.capabilities().encode_to_vec(),
                services,
            })
        }

        async fn list_space_tools(
            &self,
            _: tonic::Request<pb::ListSpaceToolsRequest>,
        ) -> R<pb::ListSpaceToolsResponse> {
            let server = self.mcp();
            ok(pb::ListSpaceToolsResponse {
                tools_json: server.tools_list().to_string(),
            })
        }

        async fn call_space_tool(
            &self,
            req: tonic::Request<pb::CallSpaceToolRequest>,
        ) -> R<pb::CallSpaceToolResponse> {
            let r = req.into_inner();
            if cua_spaces::contract::tool(&r.name).is_none() {
                return Err(st(Error::NotFound(format!("Spaces tool {}", r.name))));
            }
            let args: serde_json::Value = if r.arguments_json.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&r.arguments_json)
                    .map_err(|e| st(Error::InvalidArgument(format!("arguments_json: {e}"))))?
            };
            let server = self.mcp();
            let out = server.call(&r.name, args).await;
            ok(pb::CallSpaceToolResponse {
                content_json: serde_json::Value::Array(out.content).to_string(),
                structured_json: out.structured.map(|s| s.to_string()).unwrap_or_default(),
                is_error: out.is_error,
                meta_json: out.meta.map(|m| m.to_string()).unwrap_or_default(),
            })
        }

        async fn list_hosts(
            &self,
            _: tonic::Request<pb::ListHostsRequest>,
        ) -> R<pb::ListHostsResponse> {
            let hosts = self
                .spaces()
                .hosts()
                .await
                .map_err(|e| st(e.into()))?
                .into_iter()
                .map(|h| pb::SpacesHost {
                    id: h.id,
                    name: h.name,
                    via: h.via,
                    online: h.online,
                    os: h.os,
                    limits: h
                        .limits
                        .into_iter()
                        .map(|l| pb::SpacesHostLimit {
                            resource: l.resource,
                            used: l.used,
                            limit: l.limit,
                            reason: l.reason,
                        })
                        .collect(),
                })
                .collect();
            ok(pb::ListHostsResponse { hosts })
        }

        async fn get_space_thumbnail(
            &self,
            req: tonic::Request<pb::GetSpaceThumbnailRequest>,
        ) -> R<pb::GetSpaceThumbnailResponse> {
            let r = req.into_inner();
            let t = self
                .spaces()
                .thumbnail(&r.space, r.max_age.as_ref().map(from_dur))
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::GetSpaceThumbnailResponse {
                image: t.image,
                format: t.format,
                width: t.width,
                height: t.height,
                captured_at: Some(crate::convert::ts(t.captured_at)),
            })
        }
    }
}

#[cfg(not(feature = "spaces"))]
mod imp {
    use super::*;

    fn off<T>() -> R<T> {
        Err(st(Error::Unsupported(
            "this daemon was built without Spaces (feature `spaces`)".into(),
        )))
    }

    #[tonic::async_trait]
    impl SpaceService for Svc {
        async fn add_space(
            &self,
            _: tonic::Request<pb::AddSpaceRequest>,
        ) -> R<pb::AddSpaceResponse> {
            off()
        }
        async fn list_spaces(
            &self,
            _: tonic::Request<pb::ListSpacesRequest>,
        ) -> R<pb::ListSpacesResponse> {
            off()
        }
        async fn remove_space(
            &self,
            _: tonic::Request<pb::RemoveSpaceRequest>,
        ) -> R<pb::RemoveSpaceResponse> {
            off()
        }
        async fn resolve_space(
            &self,
            _: tonic::Request<pb::ResolveSpaceRequest>,
        ) -> R<pb::ResolveSpaceResponse> {
            off()
        }
        async fn claim_fleet_space(
            &self,
            _: tonic::Request<pb::ClaimFleetSpaceRequest>,
        ) -> R<pb::ClaimFleetSpaceResponse> {
            off()
        }
        async fn provision_local_space(
            &self,
            _: tonic::Request<pb::ProvisionLocalSpaceRequest>,
        ) -> R<pb::ProvisionLocalSpaceResponse> {
            off()
        }
        async fn release_space(
            &self,
            _: tonic::Request<pb::ReleaseSpaceRequest>,
        ) -> R<pb::ReleaseSpaceResponse> {
            off()
        }

        async fn create_space(
            &self,
            _: tonic::Request<pb::CreateSpaceRequest>,
        ) -> R<pb::CreateSpaceResponse> {
            off()
        }

        type CreateSpaceStreamStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = std::result::Result<pb::CreateSpaceStreamResponse, tonic::Status>,
                    > + Send,
            >,
        >;

        async fn create_space_stream(
            &self,
            _: tonic::Request<pb::CreateSpaceStreamRequest>,
        ) -> R<Self::CreateSpaceStreamStream> {
            off()
        }

        async fn delete_space(
            &self,
            _: tonic::Request<pb::DeleteSpaceRequest>,
        ) -> R<pb::DeleteSpaceResponse> {
            off()
        }
        async fn connect_space(
            &self,
            _: tonic::Request<pb::ConnectSpaceRequest>,
        ) -> R<pb::ConnectSpaceResponse> {
            off()
        }
        async fn list_space_tools(
            &self,
            _: tonic::Request<pb::ListSpaceToolsRequest>,
        ) -> R<pb::ListSpaceToolsResponse> {
            off()
        }
        async fn call_space_tool(
            &self,
            _: tonic::Request<pb::CallSpaceToolRequest>,
        ) -> R<pb::CallSpaceToolResponse> {
            off()
        }
        async fn list_hosts(
            &self,
            _: tonic::Request<pb::ListHostsRequest>,
        ) -> R<pb::ListHostsResponse> {
            off()
        }
        async fn get_space_thumbnail(
            &self,
            _: tonic::Request<pb::GetSpaceThumbnailRequest>,
        ) -> R<pb::GetSpaceThumbnailResponse> {
            off()
        }
    }
}
