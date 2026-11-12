//! `ShareService`: `cua share` over the daemon's gRPC.

use crate::server::{R, Svc, ok, st};
use cua_proto::daemon::v1 as pb;
use pb::share_service_server::ShareService;

#[cfg(feature = "spaces")]
pub(crate) mod real {
    use super::*;
    use crate::convert::{from_dur, ts};
    use crate::{Error, Runtime};
    use cua_share::{Access, Share, Target};
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    pub(crate) fn hub(rt: &Runtime) -> Result<&std::sync::Arc<crate::share_hub::ShareHub>, Error> {
        rt.share_hub().ok_or_else(|| {
            Error::Unsupported(
                "this daemon is not serving shares (its share gateway did not start)".into(),
            )
        })
    }

    pub(super) fn access_from_pb(v: i32) -> Access {
        match pb::ShareAccess::try_from(v).unwrap_or(pb::ShareAccess::Unspecified) {
            pb::ShareAccess::Team => Access::Team,
            pb::ShareAccess::Public => Access::Public,
            _ => Access::Private,
        }
    }

    fn access_to_pb(a: Access) -> pb::ShareAccess {
        match a {
            Access::Private => pb::ShareAccess::Private,
            Access::Team => pb::ShareAccess::Team,
            Access::Public => pb::ShareAccess::Public,
        }
    }

    pub(super) fn target_from_pb(t: &pb::ShareTarget) -> Result<Target, Error> {
        let port = u16::try_from(t.port)
            .map_err(|_| Error::InvalidArgument(format!("port {} is not 1-65535", t.port)))?;
        match pb::ShareTargetKind::try_from(t.kind).unwrap_or(pb::ShareTargetKind::Unspecified) {
            pb::ShareTargetKind::Port => Ok(Target::Port { port }),
            pb::ShareTargetKind::SpacePort => {
                if t.space.is_empty() {
                    return Err(Error::InvalidArgument(
                        "a Space port needs the Space".into(),
                    ));
                }
                Ok(Target::SpacePort {
                    space: t.space.clone(),
                    port,
                })
            }
            pb::ShareTargetKind::File => Ok(Target::File {
                path: PathBuf::from(&t.path),
            }),
            pb::ShareTargetKind::Dir => Ok(Target::Dir {
                path: PathBuf::from(&t.path),
            }),
            pb::ShareTargetKind::Unspecified => Err(Error::InvalidArgument(
                "share what? the target has no kind".into(),
            )),
        }
    }

    fn target_to_pb(t: &Target) -> pb::ShareTarget {
        match t {
            Target::Port { port } => pb::ShareTarget {
                kind: pb::ShareTargetKind::Port as i32,
                port: *port as u32,
                ..Default::default()
            },
            Target::SpacePort { space, port } => pb::ShareTarget {
                kind: pb::ShareTargetKind::SpacePort as i32,
                space: space.clone(),
                port: *port as u32,
                ..Default::default()
            },
            Target::File { path } => pb::ShareTarget {
                kind: pb::ShareTargetKind::File as i32,
                path: path.to_string_lossy().into_owned(),
                ..Default::default()
            },
            Target::Dir { path } => pb::ShareTarget {
                kind: pb::ShareTargetKind::Dir as i32,
                path: path.to_string_lossy().into_owned(),
                ..Default::default()
            },
        }
    }

    pub(crate) fn share_to_pb(s: &Share) -> pb::Share {
        let at = |ms: u64| ts(UNIX_EPOCH + std::time::Duration::from_millis(ms));
        pb::Share {
            id: s.handle.clone(),
            url: s.url.clone(),
            name: s.name.clone().unwrap_or_default(),
            target: Some(target_to_pb(&s.target)),
            access: access_to_pb(s.access) as i32,
            owner: s
                .owner
                .email
                .clone()
                .unwrap_or_else(|| s.owner.account.clone()),
            created_at: Some(at(s.created_ms)),
            expires_at: s.expires_ms.map(at),
            notices: cua_share::notices::notices(s),
            tcp: s.tcp.as_ref().map(|t| pb::ShareTcp {
                address: t.address.clone(),
                host: t.host.clone(),
                port: t.port as u32,
                srv_name: t.srv_name.clone().unwrap_or_default(),
                service: t.service.clone().unwrap_or_default(),
                route: t.route.clone(),
                route_line: t.route_line.clone(),
                cloud: t.cloud.clone().unwrap_or_default(),
                allow: t.allow.clone(),
                check_host: t.check_host,
                max_bytes: t.max_bytes,
            }),
            origin: s.origin.clone().unwrap_or_default(),
            waiting: s.waiting.as_ref().map(|w| pb::ShareWaiting {
                reason: w.reason.clone(),
                text: w.text.clone(),
                route_line: w.route_line.clone(),
            }),
        }
    }

    pub(crate) use crate::share_types::{settings_from_pb, settings_to_pb};

    /// The request as the share hub takes it (`origin`: the asking Space,
    /// for a call that came in on a Space's spawn channel).
    pub(crate) fn create_from_pb(
        r: pb::CreateShareRequest,
        origin: Option<String>,
    ) -> Result<crate::share_hub::CreateShare, Error> {
        let target = target_from_pb(r.target.as_ref().ok_or_else(|| {
            Error::InvalidArgument("share what? the request has no target".into())
        })?)?;
        Ok(crate::share_hub::CreateShare {
            target,
            access: access_from_pb(r.access),
            ttl: r.ttl.as_ref().map(from_dur),
            name: (!r.name.is_empty()).then_some(r.name),
            tcp: r.tcp.then(|| crate::share_hub::TcpRequest {
                service: (!r.service.trim().is_empty())
                    .then(|| r.service.trim().to_ascii_lowercase()),
                route: r.route.trim().to_ascii_lowercase(),
                allow: (!r.allow.is_empty()).then(|| r.allow.clone()),
                check_host: r.check_host,
                consent: r.consent,
            }),
            origin,
        })
    }

    #[tonic::async_trait]
    impl ShareService for Svc {
        async fn create_share(
            &self,
            req: tonic::Request<pb::CreateShareRequest>,
        ) -> R<pb::CreateShareResponse> {
            let create = create_from_pb(req.into_inner(), None).map_err(st)?;
            let hub = hub(self.rt()).map_err(st)?;
            let share = hub.create(self.rt(), create).await.map_err(st)?;
            ok(pb::CreateShareResponse {
                share: Some(share_to_pb(&share)),
            })
        }

        async fn list_shares(
            &self,
            _: tonic::Request<pb::ListSharesRequest>,
        ) -> R<pb::ListSharesResponse> {
            let hub = hub(self.rt()).map_err(st)?;
            ok(pb::ListSharesResponse {
                shares: hub.list().iter().map(share_to_pb).collect(),
            })
        }

        async fn stop_share(
            &self,
            req: tonic::Request<pb::StopShareRequest>,
        ) -> R<pb::StopShareResponse> {
            let r = req.into_inner();
            let hub = hub(self.rt()).map_err(st)?;
            let gone = hub
                .stop(
                    self.rt(),
                    (!r.id.is_empty()).then_some(r.id.as_str()),
                    r.all,
                )
                .map_err(st)?;
            ok(pb::StopShareResponse {
                stopped: gone.into_iter().map(|s| s.handle).collect(),
            })
        }

        async fn check_share(
            &self,
            req: tonic::Request<pb::CheckShareRequest>,
        ) -> R<pb::CheckShareResponse> {
            let r = req.into_inner();
            let hub = hub(self.rt()).map_err(st)?;
            let c = hub.check(self.rt(), &r.id, &r.path).await.map_err(st)?;
            ok(pb::CheckShareResponse {
                status: c.status as u32,
                ok: c.ok,
                via: c.via,
                detail: c.detail,
            })
        }

        async fn approve_share(
            &self,
            req: tonic::Request<pb::ApproveShareRequest>,
        ) -> R<pb::ApproveShareResponse> {
            let r = req.into_inner();
            let hub = hub(self.rt()).map_err(st)?;
            let share = hub.approve(self.rt(), &r.id, r.always).await.map_err(st)?;
            ok(pb::ApproveShareResponse {
                share: Some(share_to_pb(&share)),
            })
        }

        async fn deny_share(
            &self,
            req: tonic::Request<pb::DenyShareRequest>,
        ) -> R<pb::DenyShareResponse> {
            let hub = hub(self.rt()).map_err(st)?;
            hub.deny(&req.into_inner().id).map_err(st)?;
            ok(pb::DenyShareResponse {})
        }

        async fn get_share_settings(
            &self,
            _: tonic::Request<pb::GetShareSettingsRequest>,
        ) -> R<pb::GetShareSettingsResponse> {
            let hub = hub(self.rt()).map_err(st)?;
            let (edge_cloud, edge_host) = hub.edge_info().unwrap_or_default();
            let edge_monthly_usd = if edge_cloud.is_empty() {
                0.0
            } else {
                cua_share::route::edge_cost(&edge_cloud).monthly_usd
            };
            ok(pb::GetShareSettingsResponse {
                settings: Some(settings_to_pb(&hub.settings())),
                used_today: hub.used_today(),
                edge_cloud,
                edge_host,
                edge_monthly_usd,
            })
        }

        async fn set_share_settings(
            &self,
            req: tonic::Request<pb::SetShareSettingsRequest>,
        ) -> R<pb::SetShareSettingsResponse> {
            let hub = hub(self.rt()).map_err(st)?;
            let s = req.into_inner().settings.unwrap_or_default();
            let saved = hub.set_settings(settings_from_pb(&s)).map_err(st)?;
            ok(pb::SetShareSettingsResponse {
                settings: Some(settings_to_pb(&saved)),
            })
        }

        async fn remove_share_edge(
            &self,
            _: tonic::Request<pb::RemoveShareEdgeRequest>,
        ) -> R<pb::RemoveShareEdgeResponse> {
            let hub = hub(self.rt()).map_err(st)?;
            let gone = hub.remove_edge(self.rt()).await.map_err(st)?;
            ok(pb::RemoveShareEdgeResponse {
                stopped: gone.into_iter().map(|s| s.handle).collect(),
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn share_settings_round_trip_through_the_wire() {
            let s = cua_share::route::ShareSettings {
                landing: cua_share::route::Landing::OnlyMyCloud,
                relay_fallback: false,
                daily_cap_mib: 512,
                default_allow: vec!["203.0.113.0/24".into()],
                always_allow_spaces: vec!["local:mc".into()],
                edge_consent: vec!["aws".into()],
                ..Default::default()
            };
            assert_eq!(settings_from_pb(&settings_to_pb(&s)), s);
            let d = cua_share::route::ShareSettings::default();
            assert_eq!(settings_from_pb(&settings_to_pb(&d)), d);
        }

        #[test]
        fn a_create_carries_the_route_allowlist_host_check_and_consent() {
            let r = pb::CreateShareRequest {
                target: Some(pb::ShareTarget {
                    kind: pb::ShareTargetKind::SpacePort as i32,
                    space: "local:mc".into(),
                    port: 25565,
                    ..Default::default()
                }),
                access: pb::ShareAccess::Public as i32,
                tcp: true,
                service: " Minecraft ".into(),
                route: "Edge".into(),
                allow: vec!["203.0.113.7".into()],
                check_host: true,
                consent: true,
                ..Default::default()
            };
            let c = create_from_pb(r, None).unwrap();
            let t = c.tcp.unwrap();
            assert_eq!(t.service.as_deref(), Some("minecraft"));
            assert_eq!(t.route, "edge");
            assert_eq!(t.allow, Some(vec!["203.0.113.7".to_string()]));
            assert!(t.check_host && t.consent);
        }
    }
}

#[cfg(not(feature = "spaces"))]
mod real {
    use super::*;

    fn unavailable<T>() -> R<T> {
        Err(tonic::Status::unimplemented(
            "this daemon was built without shares",
        ))
    }

    #[tonic::async_trait]
    impl ShareService for Svc {
        async fn create_share(
            &self,
            _: tonic::Request<pb::CreateShareRequest>,
        ) -> R<pb::CreateShareResponse> {
            unavailable()
        }
        async fn list_shares(
            &self,
            _: tonic::Request<pb::ListSharesRequest>,
        ) -> R<pb::ListSharesResponse> {
            unavailable()
        }
        async fn stop_share(
            &self,
            _: tonic::Request<pb::StopShareRequest>,
        ) -> R<pb::StopShareResponse> {
            unavailable()
        }
        async fn check_share(
            &self,
            _: tonic::Request<pb::CheckShareRequest>,
        ) -> R<pb::CheckShareResponse> {
            unavailable()
        }
        async fn approve_share(
            &self,
            _: tonic::Request<pb::ApproveShareRequest>,
        ) -> R<pb::ApproveShareResponse> {
            unavailable()
        }
        async fn deny_share(
            &self,
            _: tonic::Request<pb::DenyShareRequest>,
        ) -> R<pb::DenyShareResponse> {
            unavailable()
        }
        async fn get_share_settings(
            &self,
            _: tonic::Request<pb::GetShareSettingsRequest>,
        ) -> R<pb::GetShareSettingsResponse> {
            unavailable()
        }
        async fn set_share_settings(
            &self,
            _: tonic::Request<pb::SetShareSettingsRequest>,
        ) -> R<pb::SetShareSettingsResponse> {
            unavailable()
        }
        async fn remove_share_edge(
            &self,
            _: tonic::Request<pb::RemoveShareEdgeRequest>,
        ) -> R<pb::RemoveShareEdgeResponse> {
            unavailable()
        }
    }
}
