// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spaces in your own cloud account (`on="aws"`, `"gcp"`, `"hetzner"`, `"modal"`).
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
    CloudAuditEntry, CloudCheck, CloudConnected, CloudCredentials, CloudDisconnected, CloudKind,
    CloudProvider, CloudResource, CloudStatusReport, CloudSweepItem, CloudSweepReport, CloudTarget,
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
        // Every way here (an explicit `on`, `auto`, overflow onto a Spaces
        // supplier) keeps the Space's network or refuses it before anything
        // is made: never a Space that runs more open than it asked.
        let settings = opts.network_settings.clone().unwrap_or_else(|| {
            crate::network::NetworkSettings::migrate(
                &opts.egress,
                opts.network == cua_sandbox_core::NetworkMode::None,
            )
        });
        let provider = self.inner.sandboxes.contrib_provider(word).ok().cloned();
        cloud_network(
            word,
            &settings,
            provider.as_ref().is_some_and(|p| p.takes_network_rules()),
            provider.as_ref().is_some_and(|p| p.keeps_internet_only()),
            crate::network::Mode::from_env(),
        )?;
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
                .unwrap_or_else(|| self.fresh_name()),
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
        // The Space's network rules go to the cloud with it: a cloud that
        // enforces them keeps them (Modal), any other refuses the create.
        // They are never dropped, whether the cloud was named, picked by
        // `auto` or reached by overflow.
        create.egress = settings.custom.clone();
        create.network = if settings.default_drops() {
            cua_sandbox_core::NetworkMode::None
        } else {
            opts.network
        };
        create.network_settings = Some(settings.clone());
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
                    "{word} sandbox {} did not join the relay (no relay computer)",
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
        // Provisioning has already succeeded. A directory outage must not
        // conceal a recorded target or imply that creation should be retried.
        let unavailable = || {
            Error::Relay(cua_host::Error::Relay(
                concat!(
                    "The cloud sandbox was created, but its Space details could not be read. ",
                    "Inspect the existing sandbox before retrying creation."
                )
                .into(),
            ))
        };
        self.find(&id).await.map_err(|_| unavailable())
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

/// Whether cloud `word` can keep a Space's network `settings`, refusing
/// (before anything is made) what it can't:
///
/// - `takes`: the cloud enforces rules itself (Modal's allow-list), so
///   internet, none and custom address lists are kept;
/// - `floor`: the cloud machine's own firewall keeps every Space to the
///   internet (a VM per Space), so the default is kept and nothing else;
/// - otherwise only No protection (or the old default, with no rules).
///
/// A Space in a cloud never reaches this computer or the user's network, so
/// `localhost` and `lan` are refused everywhere.
fn cloud_network(
    word: &str,
    settings: &crate::network::NetworkSettings,
    takes: bool,
    floor: bool,
    mode: crate::network::Mode,
) -> Result<()> {
    use crate::network::{Mode, NetworkPreset as P};
    let lists = !settings.custom.is_empty();
    let none = settings.default_drops();
    let rules_refused = || {
        crate::egress::refused(
            word,
            "a Space in your own cloud account runs on a machine with no host-side point cua \
             controls; create it on this machine or one of yours",
        )
    };
    let none_refused = || {
        Error::invalid(format!(
            "network=none applies to VMs on your computers and to Modal, not Spaces in this \
             cloud ({word})"
        ))
    };
    if mode == Mode::Legacy || settings.preset == P::Unprotected {
        return match () {
            _ if takes => Ok(()),
            _ if none && settings.preset != P::Unprotected => Err(none_refused()),
            _ if lists && settings.preset != P::Unprotected => Err(rules_refused()),
            _ => Ok(()),
        };
    }
    match settings.preset {
        P::Localhost | P::Lan | P::LocalhostLan => Err(Error::invalid(format!(
            "a Space in your cloud account ({word}) can't reach this computer or your local \
             network; use internet (the default), none or custom"
        ))),
        P::Internet if !lists && (takes || floor) => Ok(()),
        _ if takes => Ok(()),
        _ if none => Err(none_refused()),
        _ if lists => Err(rules_refused()),
        _ => Err(Error::invalid(format!(
            "Spaces in your cloud account ({word}) can't protect your network yet; choose No \
             protection (network unprotected) to create it there"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SpaceCreate {
        SpaceCreate {
            image: Some("linux".into()),
            ..Default::default()
        }
    }

    /// Overflow and `auto` reach a Spaces supplier through `create_in_cloud`
    /// with the caller's options: rules or `network=none` must be refused
    /// there, before anything is made, not dropped.
    #[tokio::test]
    async fn a_supplier_refuses_network_rules_instead_of_dropping_them() {
        let dir = tempfile::tempdir().unwrap();
        let s = Spaces::builder().home(dir.path()).build();

        let mut o = opts();
        o.egress = crate::egress::EgressPolicy {
            allow: vec![],
            deny: vec!["all".into()],
        };
        let e = s.create_in_cloud("modal", o).await.unwrap_err();
        assert_eq!(e.tag(), "wrong_provider", "{e}");
        assert!(e.to_string().contains("egress policy"), "{e}");

        let mut o = opts();
        o.network = cua_sandbox_core::NetworkMode::None;
        let e = s.create_in_cloud("modal", o).await.unwrap_err();
        assert_eq!(e.tag(), "invalid_argument", "{e}");
        assert!(e.to_string().contains("network=none"), "{e}");
    }

    #[test]
    fn a_cloud_keeps_what_it_can_and_refuses_the_rest() {
        use crate::network::{Mode, NetworkPreset as P, NetworkSettings as S};
        let ok = |s: &S, takes, floor| cloud_network("c", s, takes, floor, Mode::Secure).is_ok();
        let internet = S::internet();
        let none = S::none();
        let lan = S::preset(P::Lan);
        let open = S::preset(P::Unprotected);
        let mut custom = S::internet();
        custom.custom.deny = vec!["1.2.3.0/24".into()];
        // Modal (takes rules): internet, none and lists; never this computer.
        assert!(ok(&internet, true, false));
        assert!(ok(&none, true, false));
        assert!(ok(&custom, true, false));
        assert!(!ok(&lan, true, false));
        // A VM per Space behind the computer's own firewall: the default only.
        assert!(ok(&internet, false, true));
        assert!(!ok(&none, false, true));
        assert!(!ok(&custom, false, true));
        // Nothing kept: only No protection.
        assert!(!ok(&internet, false, false));
        assert!(ok(&open, false, false));
        // The old default: no rules go anywhere, rules only where kept.
        assert!(cloud_network("c", &internet, false, false, Mode::Legacy).is_ok());
        assert!(cloud_network("c", &custom, false, false, Mode::Legacy).is_err());
    }
}
