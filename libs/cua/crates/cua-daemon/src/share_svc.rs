//! `ShareService`: `cua share` over the daemon's gRPC.

use crate::server::{R, Svc, ok, st};
use cua_proto::daemon::v1 as pb;
use pb::share_service_server::ShareService;

#[cfg(feature = "spaces")]
mod real {
    use super::*;
    use crate::convert::{from_dur, ts};
    use crate::{Error, Runtime};
    use cua_share::{Access, Share, Target};
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    fn hub(rt: &Runtime) -> Result<&std::sync::Arc<crate::share_hub::ShareHub>, Error> {
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
        }
    }

    #[tonic::async_trait]
    impl ShareService for Svc {
        async fn create_share(
            &self,
            req: tonic::Request<pb::CreateShareRequest>,
        ) -> R<pb::CreateShareResponse> {
            let r = req.into_inner();
            let target = target_from_pb(r.target.as_ref().ok_or_else(|| {
                st(Error::InvalidArgument(
                    "share what? the request has no target".into(),
                ))
            })?)
            .map_err(st)?;
            let hub = hub(self.rt()).map_err(st)?;
            let share = hub
                .create(
                    self.rt(),
                    crate::share_hub::CreateShare {
                        target,
                        access: access_from_pb(r.access),
                        ttl: r.ttl.as_ref().map(from_dur),
                        name: (!r.name.is_empty()).then_some(r.name),
                    },
                )
                .await
                .map_err(st)?;
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
    }
}
