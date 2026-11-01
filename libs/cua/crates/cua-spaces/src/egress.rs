//! Egress policies of Spaces: which destinations the guest may open
//! connections to, enforced by the host the Space runs on, outside the
//! guest ([`cua_vmm::egress`] has the model and the rules).
//!
//! A policy ([`EgressPolicy`]) has two lists of IPv4/IPv6 addresses and
//! CIDRs (the word `all` is `0.0.0.0/0` and `::/0`). Everything is allowed
//! unless a rule says otherwise: every `deny` entry blocks that
//! destination, every `allow` entry is an exception that wins over `deny`,
//! an `allow` list alone blocks nothing, and empty lists remove the policy.
//! Rules are by address (a hostname is refused); DNS works while the
//! resolver the guest uses is not blocked. Loopback inside the guest,
//! replies to connections made to the guest (cua-spacesd, published ports)
//! and IPv6 neighbor discovery are never filtered.
//!
//! **Where it is enforced** ([`SpaceInfo::egress_enforced`] says `host`):
//!
//! | Space | enforced by |
//! |---|---|
//! | container (runc, gVisor), any Docker engine (also the built-in runtime on a Mac) | nftables in the engine's own network namespace, on the Space's bridge port |
//! | Firecracker microVM (Linux) | iptables in the VM's pasta namespace |
//! | QEMU VM (Linux) | iptables in the VM's pasta namespace (a running VM moves into one) |
//! | a Space one of your machines provides (`on="host:..."`, `auto`) | that machine, the same way |
//! | macOS VM (Lume), Apple Containerization, QEMU on macOS or Windows | refused: no host-side point without root |
//! | Cua Cloud, your own cloud (AWS, GCP, Modal), a Space added by address | refused: no host-side point cua controls |
//!
//! Nothing in the guest, root included, can change the rules: none are
//! inside it. The policy applies from the guest's first instruction (it is
//! set before the guest starts), and survives stop and start, suspend and
//! resume, forks and checkpoints, and a restart of the daemon (the runtime
//! keeps it with the instance).
//!
//! [`SpaceInfo::egress_enforced`]: crate::SpaceInfo::egress_enforced

use crate::{Error, Result, SpaceId, Spaces};

pub use cua_vmm::egress::{EgressPolicy, Enforcement, MAX_ENTRIES, Net, PolicyError, Rules};

/// The refusal for a Space egress policies cannot apply to.
pub(crate) fn refused(provider: &str, why: &str) -> Error {
    Error::WrongProvider {
        op: format!("An egress policy ({why})"),
        provider: provider.into(),
    }
}

/// A sandbox error as a Spaces error: a runtime's refusal of an egress
/// policy becomes [`Error::WrongProvider`] naming the runtime and why.
pub(crate) fn sandbox_error(e: cua_sandbox_core::Error) -> Error {
    e.into()
}

/// Checks a policy's entries (addresses, CIDRs, `all`).
pub(crate) fn check(policy: &EgressPolicy) -> Result<Rules> {
    policy.parse().map_err(|e| Error::invalid(e.0))
}

impl Spaces {
    /// Replaces a Space's whole egress policy at once (an empty policy
    /// removes it). The host the Space runs on enforces it outside the
    /// guest, at once while it runs and from the first instruction of every
    /// later boot: this machine for a Space on it, the machine that
    /// provides it for one of your machines' Spaces. A kind or location with
    /// no host-side point refuses it ([`Error::WrongProvider`], saying why)
    /// and keeps what it had; see the module docs for which.
    pub async fn set_egress(&self, space: &str, policy: &EgressPolicy) -> Result<()> {
        check(policy)?;
        let id = self.resolve(space)?;
        let cloud = self.cloud_sandbox_of(&id.to_string())?;
        match &id {
            SpaceId::Local { name } => {
                self.inner
                    .sandboxes
                    .set_egress(name, policy)
                    .await
                    .map_err(sandbox_error)?;
            }
            SpaceId::Relay { .. } if cloud.is_some() => {
                return Err(refused(
                    "your cloud",
                    "a Space in your own cloud account runs on a machine with no host-side \
                     point cua controls",
                ));
            }
            SpaceId::Relay { machine_id } => {
                let host = self.relay_row(machine_id).await.and_then(|m| {
                    let host = m.host.clone().filter(|h| !h.is_empty())?;
                    Some((m, host))
                });
                match host {
                    Some((m, host)) => self.egress_on_host(&m, &host, policy).await?,
                    None => {
                        return Err(refused(
                            "relay",
                            "only a Space one of your machines provides; that machine enforces \
                             it",
                        ));
                    }
                }
            }
            SpaceId::Direct { .. } if self.inner.registry.get(&id.to_string())?.is_none() => {
                return Err(Error::NotFound(id.to_string()));
            }
            SpaceId::Direct { .. } => match self.inner.registry.direct_host_of(&id.to_string())? {
                Some((host, on_host)) => {
                    self.egress_on_direct_host(&host, &on_host, policy).await?
                }
                None => {
                    return Err(refused(
                        "direct",
                        "a machine added by address is not run by cua; only a Space one of \
                         your direct hosts created takes one",
                    ));
                }
            },
            SpaceId::Cloud { .. } => {
                return Err(refused(
                    "cloud",
                    "Cua Cloud Spaces have no host-side point cua controls",
                ));
            }
        }
        self.inner.registry.update_meta(&id.to_string(), |m| {
            m.egress = policy.clone();
        })
    }

    /// The egress policy last set on a Space (empty when it has none).
    pub fn egress(&self, space: &str) -> EgressPolicy {
        self.resolve(space)
            .ok()
            .and_then(|id| self.inner.registry.meta(&id.to_string()))
            .map(|m| m.egress)
            .unwrap_or_default()
    }

    /// Where a Space's egress policy is enforced: [`Enforcement::Host`]
    /// for a Space with one (a policy is only kept once its host enforces
    /// it), `None` without one.
    pub fn egress_enforced(&self, space: &str) -> Option<Enforcement> {
        (!self.egress(space).is_empty()).then_some(Enforcement::Host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_model_is_the_runtimes() {
        // The same type: what Spaces keeps is what the runtime enforces.
        let p: cua_vmm::egress::EgressPolicy = EgressPolicy {
            allow: vec!["1.1.1.1".into()],
            deny: vec!["all".into()],
        };
        assert!(check(&p).unwrap().blocks_anything());
        let e = check(&EgressPolicy {
            allow: vec![],
            deny: vec!["example.com".into()],
        })
        .unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        assert!(e.to_string().contains("by IP/CIDR"), "{e}");
    }

    #[test]
    fn a_runtime_refusal_is_a_wrong_provider_naming_the_kind_and_why() {
        let e = sandbox_error(cua_sandbox_core::Error::Runtime(
            cua_sandbox_core::RuntimeError::Unsupported {
                backend: "lume".into(),
                op: "an egress policy (macOS VMs share Apple's vmnet NAT, which only pf can \
                     filter, and pf needs root)"
                    .into(),
            },
        ));
        assert_eq!(e.tag(), "wrong_provider");
        let m = e.to_string();
        assert!(m.starts_with("An egress policy (macOS VMs"), "{m}");
        assert!(m.ends_with("is not supported for lume Spaces"), "{m}");
        // Other refusals keep their own kind.
        let other = sandbox_error(cua_sandbox_core::Error::Runtime(
            cua_sandbox_core::RuntimeError::Unsupported {
                backend: "lume".into(),
                op: "suspend".into(),
            },
        ));
        assert_eq!(other.tag(), "sandbox");
    }
}
