//! `cua.daemon.v1.SpaceService`: a thin adapter over the runtime's
//! [`cua_spaces::Spaces`]. The registry, local provisioning
//! and every MCP tool are `cua-spaces` code; this module only converts
//! messages. Without the `spaces` feature every call is `Unsupported`.

use crate::{
    Error,
    server::{R, Svc, st},
};
use cua_proto::daemon::v1::{self as pb, space_service_server::SpaceService};

/// A create's progress report as the daemon streams it (`CreateSpaceStream`,
/// `CreateSandboxStream`).
pub(crate) fn progress_pb(p: &cua_sandbox_core::progress::Progress) -> pb::CreateSpaceProgress {
    pb::CreateSpaceProgress {
        phase: p.phase.as_str().into(),
        fraction: p.fraction,
        detail: p.detail.clone(),
        bytes_done: p.bytes.map(|b| b.done),
        bytes_total: p.bytes.map(|b| b.total).filter(|t| *t > 0),
        bytes_per_second: p.bytes.and_then(|b| b.per_second),
        space: p.target.clone(),
    }
}

/// The wire word of a guest network (`""` for the default), as a create
/// response says what it applied (`CreateSpaceResponse.network`,
/// `CreateSandboxResponse.network`).
/// `GetSpaceRouteResponse.transport`: what carries the daemon's
/// connection to `space`, or empty when it runs no cua-spacesd.
fn transport_of(space: &cua_spaces::Space) -> String {
    space
        .route()
        .map(|r| r.transport().as_str().to_owned())
        .unwrap_or_default()
}

pub(crate) fn network_word(n: cua_sandbox_core::NetworkMode) -> &'static str {
    match n {
        cua_sandbox_core::NetworkMode::Default => "",
        cua_sandbox_core::NetworkMode::None => "none",
    }
}

#[cfg(feature = "spaces")]
mod imp {
    use super::*;
    use crate::convert::from_dur;
    use crate::passthrough;
    use crate::server::ok;
    use cua_sandbox_core::placement::{Kind, On, Runtime};
    use prost::Message;

    async fn record(spaces: &cua_spaces::Spaces, id: &str) -> Result<pb::Space, Error> {
        if let Some(r) = spaces.registry().get(id).map_err(Error::from)? {
            let mut r = [r];
            spaces.fill_records(&mut r);
            let [r] = r;
            return Ok(r);
        }
        Ok(info_record(&spaces.find(id).await.map_err(Error::from)?))
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
            durable: info.durable.clone(),
            stop_reason: info.stop_reason.clone(),
            image_config: String::new(),
            labels: info.labels.clone().into_iter().collect(),
            expires_at_unix: i64::try_from(info.expires_at_unix).unwrap_or(0),
            on_expire: info.on_expire.clone(),
            created_at_unix: i64::try_from(info.created_at_unix).unwrap_or(0),
            egress: (!info.egress.is_empty()).then(|| pb::EgressPolicy {
                allow: info.egress.allow.clone(),
                deny: info.egress.deny.clone(),
            }),
            egress_enforced: info
                .egress_enforced
                .map(|e| e.as_str().to_string())
                .unwrap_or_default(),
            volumes: info.volumes.iter().map(Into::into).collect(),
        }
    }

    fn named_volume(v: &serde_json::Value) -> pb::NamedVolume {
        pb::NamedVolume {
            name: v["name"].as_str().unwrap_or_default().to_string(),
            mounted_in: v["mounted_in"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
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
            durable: r.durable,
            // Host directories are for this machine's own job runner, never
            // a caller's to ask for.
            mounts: Vec::new(),
            headless: r.headless,
            forkable: r.forkable,
            volumes: r.volumes.into_iter().map(Into::into).collect(),
            network: cua_sandbox_core::NetworkMode::parse(&r.network)
                .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?,
            labels: r.labels.into_iter().collect(),
            instance_type: opt(r.instance_type),
            region: opt(r.region),
            egress: r
                .egress
                .map(|e| cua_spaces::egress::EgressPolicy {
                    allow: e.allow,
                    deny: e.deny,
                })
                .unwrap_or_default(),
        })
    }

    /// A build event as the daemon streams it.
    fn build_progress_pb(e: &cua_spaces::build::BuildEvent) -> pb::BuildProgress {
        use cua_spaces::build::BuildEvent as E;
        let mut p = pb::BuildProgress::default();
        match e {
            E::Note { text } => {
                p.event = "note".into();
                p.text = text.clone();
            }
            E::Step {
                index,
                total,
                text,
                cached,
            } => {
                p.event = "step".into();
                p.index = *index;
                p.total = *total;
                p.text = text.clone();
                p.cached = *cached;
            }
            E::Output { index, line } => {
                p.event = "output".into();
                p.index = *index;
                p.text = line.clone();
            }
            E::StepDone {
                index,
                seconds,
                checkpoint,
            } => {
                p.event = "step_done".into();
                p.index = *index;
                p.seconds = *seconds;
                p.checkpoint = checkpoint.clone().unwrap_or_default();
            }
            E::Finished { cached, seconds } => {
                p.event = "finished".into();
                p.cached = *cached;
                p.seconds = *seconds;
            }
        }
        p
    }

    fn build_result_pb(b: cua_spaces::build::BuildResult) -> pb::BuildResult {
        pb::BuildResult {
            checkpoint: b.checkpoint,
            tag: b.tag.unwrap_or_default(),
            key: b.key,
            cached: b.cached,
            steps: b
                .steps
                .into_iter()
                .map(|s| pb::BuildStepResult {
                    index: s.index,
                    text: s.text,
                    cached: s.cached,
                    seconds: s.seconds,
                    checkpoint: s.checkpoint.unwrap_or_default(),
                })
                .collect(),
            config_json: serde_json::to_string(&b.config).unwrap_or_default(),
            seconds: b.seconds,
        }
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

    /// What a create returned, as the wire response, with the guest
    /// network it applied (`network`): a client checks it, since a daemon
    /// that predates `network` never reads it and answers without it.
    async fn create_response(
        spaces: &cua_spaces::Spaces,
        created: cua_spaces::SpaceCreated,
        network: cua_sandbox_core::NetworkMode,
    ) -> Result<pb::CreateSpaceResponse, tonic::Status> {
        let network = network_word(network).to_string();
        Ok(match created {
            cua_spaces::SpaceCreated::Ready { info, reused } => pb::CreateSpaceResponse {
                space: Some(record(spaces, &info.id).await.map_err(st)?),
                phase: "ready".into(),
                reused,
                // A reused Space was not made by this create.
                network: if reused { String::new() } else { network },
            },
            cua_spaces::SpaceCreated::Starting(p) => pb::CreateSpaceResponse {
                space: Some(pb::Space {
                    id: p.id,
                    ..Default::default()
                }),
                phase: p.phase.into(),
                reused: false,
                network,
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
                space: Some(record(self.spaces(), &info.id).await.map_err(st)?),
            })
        }

        async fn list_spaces(
            &self,
            req: tonic::Request<pb::ListSpacesRequest>,
        ) -> R<pb::ListSpacesResponse> {
            let known_now = req.into_inner().known_now;
            let mut spaces = self.spaces().registry().list().map_err(|e| st(e.into()))?;
            for s in &mut spaces {
                (s.power, s.power_state) = self.spaces().power_of(&s.id);
                s.stop_reason = self.spaces().stop_reason_of(&s.id);
            }
            self.spaces().fill_records(&mut spaces);
            // Plus the signed-in account's relay machines: refreshed, or
            // (`known_now`) as far as they answered, never holding up this
            // machine's own Spaces.
            let (others, progress, creating) = if known_now {
                let l = self
                    .spaces()
                    .list_known(cua_spaces::LIST_WAIT)
                    .await
                    .map_err(|e| st(e.into()))?;
                (l.spaces, l.progress, l.creating)
            } else {
                let all = self.spaces().list_all().await.map_err(|e| st(e.into()))?;
                (all, cua_spaces::ListProgress::default(), Vec::new())
            };
            for info in others {
                if info.provider == cua_spaces::Provider::Relay
                    && !spaces.iter().any(|s| s.id == info.id)
                {
                    spaces.push(info_record(&info));
                }
            }
            ok(pb::ListSpacesResponse {
                spaces,
                incomplete: progress.incomplete,
                reaching: progress.reaching,
                stale: progress.stale,
                notice: progress.notice,
                creating: creating
                    .into_iter()
                    .map(|c| pb::CreatingSpace {
                        machine_id: c.machine_id,
                        machine_name: c.machine_name,
                        name: c.name,
                        os: c.os,
                        phase: c.phase,
                        percent: u32::from(c.percent),
                        updated_at_ms: c.updated_at_ms,
                    })
                    .collect(),
            })
        }

        async fn list_volumes(
            &self,
            _: tonic::Request<pb::ListVolumesRequest>,
        ) -> R<pb::ListVolumesResponse> {
            let v = self
                .spaces()
                .call_extension("Cua Volume", "volumes.list", serde_json::json!({}))
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::ListVolumesResponse {
                volumes: v["volumes"]
                    .as_array()
                    .map(|a| a.iter().map(named_volume).collect())
                    .unwrap_or_default(),
            })
        }

        async fn create_volume(
            &self,
            req: tonic::Request<pb::CreateVolumeRequest>,
        ) -> R<pb::CreateVolumeResponse> {
            let name = req.into_inner().name;
            self.spaces()
                .call_extension(
                    "Cua Volume",
                    "volumes.create",
                    serde_json::json!({ "name": name }),
                )
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::CreateVolumeResponse {
                volume: Some(pb::NamedVolume {
                    name,
                    mounted_in: vec![],
                }),
            })
        }

        async fn delete_volume(
            &self,
            req: tonic::Request<pb::DeleteVolumeRequest>,
        ) -> R<pb::DeleteVolumeResponse> {
            let r = req.into_inner();
            let v = self
                .spaces()
                .call_extension(
                    "Cua Volume",
                    "volumes.delete",
                    serde_json::json!({ "name": r.name, "force": r.force }),
                )
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::DeleteVolumeResponse {
                files: v["files"].as_u64().unwrap_or(0),
            })
        }

        async fn fork_space(
            &self,
            req: tonic::Request<pb::ForkSpaceRequest>,
        ) -> R<pb::ForkSpaceResponse> {
            let r = req.into_inner();
            let info = self
                .spaces()
                .fork_local(&r.source, opt(r.name))
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::ForkSpaceResponse {
                space: Some(record(self.spaces(), &info.id).await.map_err(st)?),
            })
        }

        type BuildSpaceStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = std::result::Result<pb::BuildSpaceResponse, tonic::Status>,
                    > + Send,
            >,
        >;

        async fn build_space(
            &self,
            req: tonic::Request<pb::BuildSpaceRequest>,
        ) -> R<Self::BuildSpaceStream> {
            use pb::build_space_response::Event;
            let r = req.into_inner();
            let steps: Vec<cua_spaces::build::BuildStep> = if r.steps_json.trim().is_empty() {
                vec![]
            } else {
                serde_json::from_str(&r.steps_json)
                    .map_err(|e| st(Error::InvalidArgument(format!("steps_json: {e}"))))?
            };
            let kind = Kind::parse(&r.kind).map_err(|e| st(e.into()))?;
            let on = opt(r.location)
                .map(|l| On::parse(&l))
                .transpose()
                .map_err(|e| st(e.into()))?;
            // Bounded: a slow reader drops output lines, never the result
            // (which goes out on the same channel after every report).
            let (tx, rx) = tokio::sync::mpsc::channel(256);
            let progress = tx.clone();
            let sink: cua_spaces::build::BuildSink = std::sync::Arc::new(move |e| {
                let _ = progress.try_send(Ok(pb::BuildSpaceResponse {
                    event: Some(Event::Progress(build_progress_pb(e))),
                }));
            });
            let options = cua_spaces::build::BuildOptions {
                recipe: None,
                dockerfile: opt(r.dockerfile),
                context_dir: opt(r.context_dir).map(std::path::PathBuf::from),
                base: opt(r.base),
                steps,
                build_args: r.build_args.into_iter().collect(),
                tag: opt(r.tag),
                kind,
                on,
                cpus: (r.cpus > 0).then_some(r.cpus),
                memory_mb: (r.memory_mb > 0).then_some(r.memory_mb),
                headless: r.headless,
                no_cache: r.no_cache,
                step_timeout: r.step_timeout.as_ref().map(from_dur),
                progress: Some(sink),
            };
            let spaces = self.spaces().clone();
            tokio::spawn(async move {
                let result = match spaces.build(options).await {
                    Ok(b) => Ok(pb::BuildSpaceResponse {
                        event: Some(Event::Result(build_result_pb(b))),
                    }),
                    Err(e) => Err(st(e.into())),
                };
                let _ = tx.send(result).await;
            });
            ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }

        async fn list_builds(
            &self,
            _: tonic::Request<pb::ListBuildsRequest>,
        ) -> R<pb::ListBuildsResponse> {
            let builds = self.spaces().builds().map_err(|e| st(e.into()))?;
            ok(pb::ListBuildsResponse {
                builds: builds
                    .into_iter()
                    .map(|b| pb::BuildInfo {
                        checkpoint: b.checkpoint,
                        key: b.key,
                        tags: b.tags,
                        base: b.base,
                        layers: b.layers,
                        created_at: b.created_at,
                        config_json: serde_json::to_string(&b.config).unwrap_or_default(),
                    })
                    .collect(),
            })
        }

        async fn delete_build(
            &self,
            req: tonic::Request<pb::DeleteBuildRequest>,
        ) -> R<pb::DeleteBuildResponse> {
            let deleted = self
                .spaces()
                .delete_build(&req.into_inner().build)
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::DeleteBuildResponse { deleted })
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
                space: Some(record(self.spaces(), &id.to_string()).await.map_err(st)?),
            })
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
                space: Some(record(self.spaces(), &info.id).await.map_err(st)?),
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
            let network = create.network;
            let created = self
                .spaces()
                .create(create)
                .await
                .map_err(|e| st(e.into()))?;
            ok(create_response(self.spaces(), created, network).await?)
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
                    event: Some(Event::Progress(super::progress_pb(p))),
                }));
            }));
            let spaces = self.spaces().clone();
            let network = create.network;
            tokio::spawn(async move {
                let result = match spaces.create(create).await {
                    Ok(created) => create_response(&spaces, created, network).await.map(|r| {
                        pb::CreateSpaceStreamResponse {
                            event: Some(Event::Result(r)),
                        }
                    }),
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

        async fn set_space_lifetime(
            &self,
            req: tonic::Request<pb::SetSpaceLifetimeRequest>,
        ) -> R<pb::SetSpaceLifetimeResponse> {
            let r = req.into_inner();
            let d = r
                .duration
                .as_ref()
                .map(crate::convert::from_dur)
                .unwrap_or_default();
            let action = if r.on_expire.is_empty() {
                None
            } else {
                Some(
                    cua_sandbox_core::LocalExpiry::parse(&r.on_expire).ok_or_else(|| {
                        tonic::Status::invalid_argument(format!(
                            "on_expire {:?}: expected \"delete\" or \"suspend\"",
                            r.on_expire
                        ))
                    })?,
                )
            };
            let at = self
                .spaces()
                .set_lifetime(&r.space, d, action)
                .map_err(|e| st(e.into()))?;
            ok(pb::SetSpaceLifetimeResponse {
                expires_at: Some(crate::convert::ts(at)),
            })
        }

        async fn set_space_egress(
            &self,
            req: tonic::Request<pb::SetSpaceEgressRequest>,
        ) -> R<pb::SetSpaceEgressResponse> {
            let r = req.into_inner();
            let policy = r
                .egress
                .map(|e| cua_spaces::egress::EgressPolicy {
                    allow: e.allow,
                    deny: e.deny,
                })
                .unwrap_or_default();
            self.spaces()
                .set_egress(&r.space, &policy)
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::SetSpaceEgressResponse {})
        }

        async fn delete_space(
            &self,
            req: tonic::Request<pb::DeleteSpaceRequest>,
        ) -> R<pb::DeleteSpaceResponse> {
            let space = req.into_inner().space;
            let message = self
                .spaces()
                .delete(&space)
                .await
                .map_err(|e| st(e.into()))?;
            // Drop the connection an exec or a call cached for it: a new
            // Space of the same name must not reach the deleted one.
            self.rt().detach(&space).await;
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
                space: Some(record(self.spaces(), &id).await.map_err(st)?),
                env_url: if space.has_spacesd() {
                    format!("{base}/v1/spaces/{key}/env")
                } else {
                    String::new()
                },
                token,
                capabilities: space.capabilities().encode_to_vec(),
                services,
                route: space
                    .route()
                    .map(cua_spaces::ChannelRoute::describe)
                    .unwrap_or_default(),
                bulk: space
                    .bulk_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                transport: transport_of(&space),
                fallback_reason: space.fallback_reason().unwrap_or_default().to_owned(),
            })
        }

        async fn get_space_route(
            &self,
            req: tonic::Request<pb::GetSpaceRouteRequest>,
        ) -> R<pb::GetSpaceRouteResponse> {
            let space = self
                .spaces()
                .space(&req.into_inner().space)
                .await
                .map_err(|e| st(e.into()))?;
            let route = space.route();
            ok(pb::GetSpaceRouteResponse {
                route: route
                    .map(cua_spaces::ChannelRoute::describe)
                    .unwrap_or_default(),
                direct: route.is_some_and(cua_spaces::ChannelRoute::is_direct),
                transport: transport_of(&space),
                fallback_reason: space.fallback_reason().unwrap_or_default().to_owned(),
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
            let app_method = cua_spaces::agents::keys::is_app_method(&r.name);
            if !app_method && cua_spaces::contract::tool(&r.name).is_none() {
                return Err(st(Error::NotFound(format!("Spaces tool {}", r.name))));
            }
            let args: serde_json::Value = if r.arguments_json.trim().is_empty() {
                serde_json::json!({})
            } else {
                // Never quote the arguments: agent_keys.set carries a key.
                serde_json::from_str(&r.arguments_json)
                    .map_err(|_| st(Error::InvalidArgument("arguments_json is not JSON".into())))?
            };
            // The app's Settings → Agents methods: not contract tools, so no
            // MCP server lists or forwards them to an agent.
            let out = if app_method {
                cua_spaces::agents::keys::app_tool(&r.name, args).unwrap_or_else(|| {
                    cua_spaces::mcp::ToolOutcome::error_message("not_found", r.name.clone())
                })
            } else {
                self.mcp().call(&r.name, args).await
            };
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
                    load: h.node.map(|n| pb::SpacesHostLoad {
                        millicpus_total: n.millicpus_total,
                        millicpus_free: n.millicpus_free,
                        memory_total_mb: n.memory_total_mb,
                        memory_free_mb: n.memory_free_mb,
                        spaces: n.running.values().sum(),
                        problem: n.problem,
                        kinds: n.kinds,
                    }),
                })
                .collect();
            ok(pb::ListHostsResponse { hosts })
        }

        async fn pool_request(
            &self,
            r: tonic::Request<pb::PoolRequestRequest>,
        ) -> R<pb::PoolRequestResponse> {
            let r = r.into_inner();
            let target = if r.target.is_empty() {
                "local"
            } else {
                r.target.as_str()
            };
            if !matches!(target, "local" | "pool") {
                return Err(st(Error::InvalidArgument(format!(
                    "target {target:?}: use local or pool"
                ))));
            }
            let params: serde_json::Value = if r.params_json.trim().is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_str(&r.params_json)
                    .map_err(|e| st(Error::InvalidArgument(format!("params_json: {e}"))))?
            };
            for a in self.rt().attached_extensions() {
                if let Some(fut) = a.pool_request(target, &r.method, params.clone()) {
                    let (status, body) = fut.await;
                    return ok(pb::PoolRequestResponse {
                        status: u32::from(status),
                        body_json: body.to_string(),
                    });
                }
            }
            Err(st(Error::Unsupported(
                "batch runs on your machines ship with Cua Spaces (this daemon has none)".into(),
            )))
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

        async fn get_resources(
            &self,
            req: tonic::Request<pb::GetResourcesRequest>,
        ) -> R<pb::GetResourcesResponse> {
            let snapshot = self
                .spaces()
                .resources(req.into_inner().visible)
                .await
                .map_err(|e| st(e.into()))?;
            ok(pb::GetResourcesResponse {
                snapshot: Some((&snapshot).into()),
            })
        }

        type WatchResourcesStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = std::result::Result<pb::WatchResourcesResponse, tonic::Status>,
                    > + Send,
            >,
        >;

        async fn watch_resources(
            &self,
            req: tonic::Request<pb::WatchResourcesRequest>,
        ) -> R<Self::WatchResourcesStream> {
            let mut watch = self.spaces().watch_resources(req.into_inner().visible);
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            // The watch is the interest: it ends with the stream, so the
            // sampler stops when the last reader is gone.
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        event = watch.next() => {
                            let Some(event) = event else { break };
                            if tx.send(Ok((&event).into())).await.is_err() {
                                break;
                            }
                        }
                        _ = tx.closed() => break,
                    }
                }
            });
            ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }

        type WatchSpacesStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = std::result::Result<pb::WatchSpacesResponse, tonic::Status>,
                    > + Send,
            >,
        >;

        async fn watch_spaces(
            &self,
            _: tonic::Request<pb::WatchSpacesRequest>,
        ) -> R<Self::WatchSpacesStream> {
            let mut watch = self.spaces().watch_list();
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            // The subscription is the interest: it ends with the stream, so
            // the watcher stops when the last reader is gone.
            tokio::spawn(async move {
                let mut generation = watch.generation();
                loop {
                    if tx
                        .send(Ok(pb::WatchSpacesResponse { generation }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    tokio::select! {
                        g = watch.changed() => match g {
                            Some(g) => generation = g,
                            None => break,
                        },
                        _ = tx.closed() => break,
                    }
                }
            });
            ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
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
        async fn list_volumes(
            &self,
            _: tonic::Request<pb::ListVolumesRequest>,
        ) -> R<pb::ListVolumesResponse> {
            off()
        }
        async fn create_volume(
            &self,
            _: tonic::Request<pb::CreateVolumeRequest>,
        ) -> R<pb::CreateVolumeResponse> {
            off()
        }
        async fn delete_volume(
            &self,
            _: tonic::Request<pb::DeleteVolumeRequest>,
        ) -> R<pb::DeleteVolumeResponse> {
            off()
        }
        async fn resolve_space(
            &self,
            _: tonic::Request<pb::ResolveSpaceRequest>,
        ) -> R<pb::ResolveSpaceResponse> {
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

        type BuildSpaceStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = std::result::Result<pb::BuildSpaceResponse, tonic::Status>,
                    > + Send,
            >,
        >;

        async fn build_space(
            &self,
            _: tonic::Request<pb::BuildSpaceRequest>,
        ) -> R<Self::BuildSpaceStream> {
            off()
        }

        async fn list_builds(
            &self,
            _: tonic::Request<pb::ListBuildsRequest>,
        ) -> R<pb::ListBuildsResponse> {
            off()
        }

        async fn delete_build(
            &self,
            _: tonic::Request<pb::DeleteBuildRequest>,
        ) -> R<pb::DeleteBuildResponse> {
            off()
        }

        async fn set_space_lifetime(
            &self,
            _: tonic::Request<pb::SetSpaceLifetimeRequest>,
        ) -> R<pb::SetSpaceLifetimeResponse> {
            off()
        }

        async fn set_space_egress(
            &self,
            _: tonic::Request<pb::SetSpaceEgressRequest>,
        ) -> R<pb::SetSpaceEgressResponse> {
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
        async fn pool_request(
            &self,
            _: tonic::Request<pb::PoolRequestRequest>,
        ) -> R<pb::PoolRequestResponse> {
            off()
        }
        async fn get_space_thumbnail(
            &self,
            _: tonic::Request<pb::GetSpaceThumbnailRequest>,
        ) -> R<pb::GetSpaceThumbnailResponse> {
            off()
        }
        async fn get_space_route(
            &self,
            _: tonic::Request<pb::GetSpaceRouteRequest>,
        ) -> R<pb::GetSpaceRouteResponse> {
            off()
        }

        async fn get_resources(
            &self,
            _: tonic::Request<pb::GetResourcesRequest>,
        ) -> R<pb::GetResourcesResponse> {
            off()
        }

        type WatchResourcesStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = std::result::Result<pb::WatchResourcesResponse, tonic::Status>,
                    > + Send,
            >,
        >;

        async fn watch_resources(
            &self,
            _: tonic::Request<pb::WatchResourcesRequest>,
        ) -> R<Self::WatchResourcesStream> {
            off()
        }

        type WatchSpacesStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = std::result::Result<pb::WatchSpacesResponse, tonic::Status>,
                    > + Send,
            >,
        >;

        async fn watch_spaces(
            &self,
            _: tonic::Request<pb::WatchSpacesRequest>,
        ) -> R<Self::WatchSpacesStream> {
            off()
        }
    }
}
