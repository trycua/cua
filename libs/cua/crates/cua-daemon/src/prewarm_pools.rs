//! This machine's warm pools for prewarms ([`cua_spaces::prewarm`]): a
//! prewarm's pool is a named pool (`prewarm-<id>`) of the daemon's warm
//! pool machinery, `count` members kept ready, none after its TTL without
//! a claim.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use cua_proto::env::v1 as pb;

use crate::runtime::{CreateRequest, Runtime};

/// [`cua_spaces::prewarm::LocalPools`] over the daemon's runtime.
pub(crate) struct DaemonPools(pub(crate) Runtime);

impl cua_spaces::prewarm::LocalPools for DaemonPools {
    fn set_warm<'a>(
        &'a self,
        r: &'a pb::SetHostWarmRequest,
    ) -> Pin<Box<dyn Future<Output = cua_spaces::Result<pb::SetHostWarmResponse>> + Send + 'a>>
    {
        Box::pin(async move {
            if r.count == 0 {
                self.0
                    .delete_pool(&r.name)
                    .map_err(|e| cua_spaces::Error::host("prewarm", e.to_string()))?;
                return Ok(pb::SetHostWarmResponse::default());
            }
            let spec = CreateRequest {
                location: Some("local".into()),
                kind: Some(r.kind.clone()).filter(|k| !k.is_empty()),
                runtime: Some(r.runtime.clone()).filter(|k| !k.is_empty()),
                image: r.image.clone(),
                cpus: Some(r.cpus).filter(|c| *c > 0),
                memory_mb: Some(r.memory_mb).filter(|m| *m > 0),
                // A Space waits for its cua-spacesd: the probe is part of
                // the shape, so members must carry it to be claimed.
                wait_for: vec![cua_sandbox_core::Probe::Tcp(
                    cua_proto::SPACESD_DEFAULT_PORT,
                )],
                ..Default::default()
            };
            let ttl = Duration::from_secs(if r.ttl_secs == 0 { 3600 } else { r.ttl_secs });
            let p = self
                .0
                .create_pool(Some(&r.name), spec, r.count, r.count, Some(ttl))
                .await
                .map_err(|e| cua_spaces::Error::host("prewarm", e.to_string()))?;
            Ok(pb::SetHostWarmResponse {
                ready: p.ready,
                starting: p.starting,
                reason: p.status,
            })
        })
    }
}
