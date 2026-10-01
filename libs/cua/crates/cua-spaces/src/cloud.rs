// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spaces in your own cloud account (`on="aws"`, `"gcp"`, `"modal"`).
//!
//! A cloud Space is a sandbox from that provider (the sandbox layer owns
//! the cloud: provisioning, tags, records, stop and start, cleanup; see
//! `cua_sandbox_core::byoc`) whose cua-spacesd joined the cua.ai relay as a
//! machine of the signed-in account. The Space is that relay machine,
//! `relay:<machine>`; deleting it deletes the sandbox, and stop_space /
//! start_space stop and start it ([`Spaces::stop`]).

use crate::relay::RelayMachine;
use crate::{Error, Result, SpaceCreate, SpaceInfo, Spaces};
use cua_proto::env::v1 as pb;
use cua_sandbox_core::byoc::DETAIL_RELAY_MACHINE;
use cua_sandbox_core::byoc::meta;
use cua_sandbox_core::placement::Kind;
use cua_sandbox_core::{CreateOptions, ProviderKind};

pub use cua_sandbox_core::byoc::{
    CloudCheck, CloudConnected, CloudCredentials, CloudDisconnected, CloudKind, CloudProvider,
    CloudResource, CloudStatusReport, CloudSweepItem, CloudSweepReport, CloudTarget,
    CloudTestReport,
};

/// `SpaceInfo::cloud_delete`: this home created it and deletes it.
pub const DELETE_HERE: &str = "here";
/// `SpaceInfo::cloud_delete`: only the device that created it can.
pub const DELETE_ELSEWHERE: &str = "elsewhere";

/// Whether a relay machine is a Space in someone's own cloud (its relay
/// metadata names the provider).
pub fn is_cloud_machine(m: &RelayMachine) -> bool {
    m.meta.get(meta::PROVIDER).is_some_and(|p| !p.is_empty())
}

impl Spaces {
    /// Deletes a Space in your cloud that another device of the account
    /// created: that device does it through its own ownership records when
    /// it is a host the relay reaches (`DeleteCloudSpace`); otherwise this
    /// refuses and says where to delete it. Nothing here holds that cloud's
    /// credentials.
    pub(crate) async fn delete_cloud_elsewhere(&self, m: &RelayMachine) -> Result<String> {
        let id = format!("relay:{}", m.id);
        let place = m
            .meta
            .get(meta::PLACE)
            .cloned()
            .unwrap_or_else(|| m.meta.get(meta::PROVIDER).cloned().unwrap_or_default());
        let device = m
            .meta
            .get(meta::DEVICE)
            .filter(|d| !d.is_empty())
            .cloned()
            .unwrap_or_else(|| "the device that created it".into());
        let Some(host) = m.meta.get(meta::HOST).filter(|h| !h.is_empty()) else {
            return Err(Error::host(
                "the device that created it",
                format!(
                    "{id} runs in {place} and was created on {device}, which keeps its cloud \
                     records: delete it there, or remove it from this list"
                ),
            ));
        };
        let row = self
            .relay_machines()
            .await?
            .into_iter()
            .find(|r| &r.id == host);
        match row {
            Some(r) if r.online => {}
            _ => {
                return Err(Error::host(
                    "the device that created it",
                    format!(
                        "{id} runs in {place} and {device}, which created it, is offline: turn \
                         it on and try again, or remove it from this list"
                    ),
                ));
            }
        }
        let host_space = self.space(&format!("relay:{host}")).await?;
        let message = host_space
            .spacesd()?
            .host_spaces()
            .delete_cloud_space(pb::DeleteCloudSpaceRequest {
                space: m.id.clone(),
            })
            .await
            .map(|r| r.into_inner().message)
            .map_err(|e| crate::host_spaces::from_host(cua_spacesd_client::Error::from(e)))?;
        Ok(format!("{message} (on {device})"))
    }

    /// Creates a Space in your cloud `word`: a sandbox there, then its
    /// relay machine as the Space.
    pub(crate) async fn create_in_cloud(&self, word: &str, opts: SpaceCreate) -> Result<SpaceInfo> {
        self.relay()?;
        let image = opts
            .image
            .clone()
            .filter(|i| !i.trim().is_empty())
            .unwrap_or_else(|| "linux".to_string());
        let mut create = CreateOptions::new(ProviderKind::Contrib, image);
        create.contrib = Some(word.to_string());
        create.name = Some(
            opts.name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .map(|n| crate::spaces::sanitize_label(&n))
                .unwrap_or_else(|| format!("space-{}", crate::spaces::random_suffix())),
        );
        if opts.kind != Kind::Auto {
            create.kind = opts.kind;
        }
        create.runtime = opts.runtime.clone();
        if let Some(c) = opts.cpus {
            create.cpus = c;
        }
        if let Some(m) = opts.memory_mb {
            create.memory_mb = m;
        }
        if let Some(t) = opts.timeout {
            create.ready_timeout = t;
            create.ready_timeout_given = true;
        }
        create.env = opts.env.clone();
        create.command = opts.command.clone();
        create.services = opts.services.clone();
        let sandbox = self.inner.sandboxes.create(create).await?;
        let machine = sandbox
            .provider_details()
            .get(DETAIL_RELAY_MACHINE)
            .cloned()
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{word} sandbox {} did not join the relay (no relay machine)",
                    sandbox.name()
                ))
            })?;
        let id = format!("relay:{machine}");
        // Connect it once, as a local create does: the handshake records its
        // capabilities, and extensions start what a connected Space gets
        // (the Cua Volume mount in its guest).
        if let Err(e) = self.space(&id).await {
            tracing::warn!(space = %id, "the new cloud Space did not answer its handshake yet: {e}");
        }
        let info = self
            .list_all()
            .await?
            .into_iter()
            .find(|i| i.id == id)
            .ok_or_else(|| Error::NotFound(format!("{id} on the relay")))?;
        Ok(info)
    }

    /// The name of the cloud sandbox behind a `relay:<machine>` Space, if
    /// it is one.
    pub fn cloud_sandbox_of(&self, space: &str) -> Result<Option<String>> {
        match self.resolve(space)? {
            crate::SpaceId::Relay { machine_id } => {
                Ok(self.inner.sandboxes.by_relay_machine(&machine_id)?)
            }
            _ => Ok(None),
        }
    }
}
