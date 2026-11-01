//! Egress policies of container sandboxes, enforced on the engine's host
//! ([`crate::egress`]).
//!
//! The rules live in the engine's own network namespace (the Linux host,
//! or the engine VM of Docker Desktop, Colima or the built-in runtime),
//! written as root through the engine's privileged helper container
//! ([`super::snapshot::Helper`]): nothing in a container, gVisor or runc,
//! root or not, can reach them.
//!
//! Every container gets a MAC address of its own at create
//! ([`crate::egress::engine_mac`], recorded in [`LABEL_MAC`]), so its rules
//! are in place before the container starts, keyed by that MAC; once it
//! runs, they are also bound to its bridge port, which must carry that
//! MAC (no frame can pass for another container's). A container created
//! with a policy runs without `CAP_NET_RAW` (it could otherwise send frames
//! with another MAC in the moment between its start and that binding; ping
//! still works through ICMP datagram sockets). A container started with a
//! policy it was not created for is first recreated with both, from a
//! commit of its filesystem.
//!
//! The policy is kept per container under `~/.cua/docker/egress/<name>.json`:
//! every start applies it again (an engine or host restart drops the
//! rules), and forks inherit it.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{ContainerRuntime, engine_err};
use crate::egress::{self, BridgePort, EgressPolicy, EngineTarget};
use crate::error::{Result, VmmError};
use crate::host;

/// On a container: the MAC address it was created with.
pub const LABEL_MAC: &str = "ai.cua.mac";

/// The Docker network a container joins when it names none.
const DEFAULT_NETWORK: &str = "bridge";

/// How long one engine-host command may take.
const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

/// What is kept for one container.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    /// The policy (empty: none, set on purpose).
    pub policy: EgressPolicy,
    /// The nft table its rules were last written to, so a new MAC (a
    /// recreate) does not leave the old one behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
}

fn dir() -> PathBuf {
    host::cua_home().join("docker").join("egress")
}

fn path(name: &str) -> PathBuf {
    dir().join(format!("{name}.json"))
}

/// The policy kept for container `name`; `None` when none was ever set.
pub(crate) fn kept(name: &str) -> Option<Kept> {
    let bytes = std::fs::read(path(name)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Keeps `k` for container `name` (atomically).
pub(crate) fn keep(name: &str, k: &Kept) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = dir().join(format!(".{name}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(k)?)?;
    std::fs::rename(&tmp, path(name))?;
    Ok(())
}

/// Forgets what is kept for container `name`.
pub(crate) fn forget(name: &str) {
    let _ = std::fs::remove_file(path(name));
}

/// Gives container `to` the policy of `from` (a fork, a rename).
pub(crate) fn copy(from: &str, to: &str) -> Result<()> {
    match kept(from) {
        Some(k) => keep(
            to,
            &Kept {
                policy: k.policy,
                table: None,
            },
        ),
        None => Ok(()),
    }
}

/// Whether the policy kept for `name` blocks anything.
pub(crate) fn blocks(name: &str) -> bool {
    kept(name).is_some_and(|k| policy_blocks(&k.policy))
}

/// Whether `policy` blocks any destination (an allow list alone does not).
pub(crate) fn policy_blocks(policy: &EgressPolicy) -> bool {
    policy.parse().is_ok_and(|r| r.blocks_anything())
}

/// The MAC address a new container named `name` gets.
pub(crate) fn mac_for(name: &str) -> String {
    egress::engine_mac(&format!("{}:{name}", super::home_tag()))
}

/// The engine-host script that reports the bridge's MAC address (`gw=`),
/// the host end of container process `$2`'s `eth0` (`veth=`, when `$2` is
/// not 0), and whether nft is there (`nft=missing`).
const PROBE: &str = r#"br="$1"; pid="$2"
ip -o link show dev "$br" 2>/dev/null | sed -n 's/.*link\/ether \([0-9a-fA-F:]*\).*/gw=\1/p'
if [ "$pid" != 0 ]; then
  idx=$(nsenter -t "$pid" -n ip -o link show dev eth0 2>/dev/null | sed -n 's/^[0-9]*: eth0@if\([0-9]*\):.*/\1/p')
  [ -n "$idx" ] && ip -o link | sed -n "s/^$idx: \([^:@]*\)[@:].*/veth=\1/p"
fi
command -v nft >/dev/null 2>&1 || echo nft=missing
"#;

/// The engine-host script that feeds `$1` to `nft -f -`.
const APPLY: &str = r#"printf '%s' "$1" | nft -f -"#;

impl ContainerRuntime {
    /// Runs `script` with `args` as root in the engine host's own network
    /// namespace (the helper's chroot, then `nsenter` into pid 1's).
    async fn engine_sh(&self, script: &str, args: &[&str]) -> Result<super::snapshot::HelperOut> {
        let helper = self.helper();
        helper.ensure().await?;
        let mut argv = vec!["nsenter", "-t", "1", "-n", "--", "sh", "-c", script, "sh"];
        argv.extend_from_slice(args);
        helper.run(&argv, HELPER_TIMEOUT).await
    }

    /// The bridge device of Docker network `net` (`docker0` for `bridge`).
    async fn bridge_device(&self, net: &str) -> Result<String> {
        let n = self
            .docker
            .inspect_network(net, None)
            .await
            .map_err(engine_err)?;
        if n.driver.as_deref() != Some("bridge") {
            return Err(VmmError::Unsupported {
                backend: "container",
                op: "an egress policy on a network other than a bridge (the policy is enforced \
                     on the container's bridge port)",
            });
        }
        let dev = n
            .options
            .as_ref()
            .and_then(|o| o.get("com.docker.network.bridge.name").cloned())
            .or_else(|| {
                n.id.as_deref()
                    .filter(|id| id.len() >= 12)
                    .map(|id| format!("br-{}", &id[..12]))
            })
            .ok_or_else(|| VmmError::Engine(format!("network {net} has no bridge device")))?;
        if !egress::valid_ifname(&dev) {
            return Err(VmmError::Engine(format!(
                "network {net}: unexpected bridge name {dev:?}"
            )));
        }
        Ok(dev)
    }

    /// The `NetworkingConfig` that gives a new container `name` its own MAC
    /// on the engine's default bridge, with the MAC (`None` when the engine
    /// has no such network: such an engine refuses policies).
    pub(crate) async fn mac_config(
        &self,
        name: &str,
    ) -> Option<(bollard::models::NetworkingConfig, String)> {
        let n = self
            .docker
            .inspect_network(DEFAULT_NETWORK, None)
            .await
            .ok()?;
        if n.driver.as_deref() != Some("bridge") {
            return None;
        }
        let mac = mac_for(name);
        let endpoint = bollard::models::EndpointSettings {
            mac_address: Some(mac.clone()),
            ..Default::default()
        };
        Some((
            bollard::models::NetworkingConfig {
                endpoints_config: Some(
                    [(DEFAULT_NETWORK.to_string(), endpoint)]
                        .into_iter()
                        .collect(),
                ),
            },
            mac,
        ))
    }

    /// Where container `i`'s rules go: its network, MAC and address, its
    /// bridge port when it runs (`running`), the bridge and its MAC.
    async fn engine_target(
        &self,
        i: &bollard::models::ContainerInspectResponse,
        running: bool,
    ) -> Result<EngineTarget> {
        let name = i
            .name
            .as_deref()
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_string();
        let mode = i
            .host_config
            .as_ref()
            .and_then(|h| h.network_mode.clone())
            .unwrap_or_default();
        let nets = i
            .network_settings
            .as_ref()
            .and_then(|n| n.networks.clone())
            .unwrap_or_default();
        let unsupported = |op: &'static str| VmmError::Unsupported {
            backend: "container",
            op,
        };
        if mode == "host" || mode == "none" || mode.starts_with("container:") {
            return Err(unsupported(
                "an egress policy on a container without a network of its own (host, none or \
                 another container's)",
            ));
        }
        if nets.len() != 1 {
            return Err(unsupported(
                "an egress policy on a container on several networks (the policy is enforced \
                 on its one bridge port)",
            ));
        }
        let (net, ep) = nets.into_iter().next().expect("one network");
        let mac = ep
            .mac_address
            .as_deref()
            .and_then(egress::normalize_mac)
            .ok_or_else(|| {
                VmmError::other(format!(
                    "container {name} has no MAC address on {net} yet; start it once, or \
                     recreate it"
                ))
            })?;
        let bridge = self.bridge_device(&net).await?;
        let pid = if running {
            i.state.as_ref().and_then(|s| s.pid).unwrap_or(0)
        } else {
            0
        };
        let out = self.engine_sh(PROBE, &[&bridge, &pid.to_string()]).await?;
        let field = |k: &str| {
            out.stdout
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")).map(str::to_string))
        };
        if field("nft").as_deref() == Some("missing") {
            return Err(VmmError::missing(
                "nft (nftables) on the container engine's host",
                "install nftables where the engine runs (Ubuntu: sudo apt-get install nftables); \
                 egress policies are enforced there",
            ));
        }
        let gateway_mac = field("gw")
            .as_deref()
            .and_then(egress::normalize_mac)
            .ok_or_else(|| {
                VmmError::Engine(format!(
                    "could not read the MAC address of bridge {bridge} on the engine's host: {}",
                    out.stderr.trim()
                ))
            })?;
        let port = if running {
            let veth = field("veth")
                .filter(|v| egress::valid_ifname(v))
                .ok_or_else(|| {
                    VmmError::Engine(format!(
                        "could not find the bridge port of container {name} on the engine's \
                         host: {}",
                        out.stderr.trim()
                    ))
                })?;
            Some(BridgePort {
                veth,
                ipv4: ep
                    .ip_address
                    .as_deref()
                    .and_then(|a| a.parse::<Ipv4Addr>().ok()),
            })
        } else {
            None
        };
        Ok(EngineTarget {
            table: egress::engine_table(&mac),
            bridge,
            gateway_mac,
            mac,
            port,
        })
    }

    /// Writes `text` with nft on the engine host.
    async fn engine_apply(&self, text: &str) -> Result<()> {
        let out = self.engine_sh(APPLY, &[text]).await?;
        if out.code != 0 {
            return Err(VmmError::Engine(format!(
                "the engine's host refused the egress rules: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    /// Writes `policy` for container `name` on the engine host (keyed by
    /// its MAC; bound to its bridge port as well when it runs) and keeps it.
    pub(crate) async fn police(&self, name: &str, policy: &EgressPolicy) -> Result<()> {
        let rules = egress::check(crate::BackendKind::Container, policy)?;
        let i = self
            .inspect(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        let previous = kept(name).and_then(|k| k.table);
        if !rules.blocks_anything() {
            // Nothing to filter: remove what was there.
            if let Some(t) = &previous {
                self.engine_apply(&egress::engine_remove(t)).await?;
            }
            return keep(
                name,
                &Kept {
                    policy: policy.clone(),
                    table: None,
                },
            );
        }
        let running = matches!(
            Self::status_from(&i),
            crate::Status::Running | crate::Status::Paused
        );
        let target = self.engine_target(&i, running).await?;
        let mut text = String::new();
        if let Some(t) = previous.as_ref().filter(|t| **t != target.table) {
            text.push_str(&egress::engine_remove(t));
        }
        text.push_str(&egress::engine_ruleset(&target, &rules));
        self.engine_apply(&text).await?;
        keep(
            name,
            &Kept {
                policy: policy.clone(),
                table: Some(target.table),
            },
        )
    }

    /// Removes container `name`'s rules from the engine host and forgets
    /// its policy (the container is gone).
    pub(crate) async fn unpolice(&self, name: &str) {
        if let Some(t) = kept(name).and_then(|k| k.table)
            && let Err(e) = self.engine_apply(&egress::engine_remove(&t)).await
        {
            tracing::warn!(container = name, error = %e, "could not remove a deleted container's egress rules");
        }
        forget(name);
    }

    /// Whether stopped container `i` must be recreated before it may start
    /// under the policy kept for it: it lacks its own MAC or still has
    /// `CAP_NET_RAW`.
    pub(crate) fn needs_recreate(
        &self,
        name: &str,
        i: &bollard::models::ContainerInspectResponse,
    ) -> bool {
        if !blocks(name) {
            return false;
        }
        let labelled = i
            .config
            .as_ref()
            .and_then(|c| c.labels.as_ref())
            .and_then(|l| l.get(LABEL_MAC))
            .and_then(|m| egress::normalize_mac(m));
        let configured = i
            .network_settings
            .as_ref()
            .and_then(|n| n.networks.as_ref())
            .and_then(|n| n.values().next())
            .and_then(|e| e.mac_address.as_deref())
            .and_then(egress::normalize_mac);
        let raw_dropped = i
            .host_config
            .as_ref()
            .and_then(|h| h.cap_drop.as_ref())
            .is_some_and(|c| c.iter().any(|x| x.trim_start_matches("CAP_") == "NET_RAW"));
        labelled.is_none() || labelled != configured || !raw_dropped
    }

    /// Starts container `name` under the policy kept for it: the rules are
    /// written (keyed by its MAC) before it starts and bound to its bridge
    /// port once it runs. A container whose rules cannot be written does
    /// not start, and one whose port cannot be bound is killed.
    pub(crate) async fn start_policed(&self, name: &str) -> Result<()> {
        let policy = kept(name).map(|k| k.policy).filter(policy_blocks);
        if let Some(p) = &policy {
            self.police(name, p).await?;
        }
        self.docker
            .start_container(name, None)
            .await
            .map_err(engine_err)?;
        if let Some(p) = &policy
            && let Err(e) = self.police(name, p).await
        {
            let _ = self
                .docker
                .kill_container(
                    name,
                    None::<bollard::query_parameters::KillContainerOptions>,
                )
                .await;
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_and_apply_scripts_take_their_arguments_positionally() {
        // Nothing of the caller's is pasted into the scripts: names and
        // the ruleset arrive as $1/$2.
        assert!(PROBE.contains(r#"br="$1"; pid="$2""#));
        assert!(APPLY.contains(r#""$1""#));
    }

    #[test]
    fn a_mac_per_container_name() {
        assert_eq!(mac_for("web"), mac_for("web"));
        assert_ne!(mac_for("web"), mac_for("db"));
        assert!(egress::normalize_mac(&mac_for("web")).is_some());
    }

    #[test]
    fn policy_blocks_only_with_a_deny_entry() {
        let p = |a: &[&str], d: &[&str]| EgressPolicy {
            allow: a.iter().map(|s| s.to_string()).collect(),
            deny: d.iter().map(|s| s.to_string()).collect(),
        };
        assert!(policy_blocks(&p(&[], &["all"])));
        assert!(policy_blocks(&p(&["1.1.1.1"], &["10.0.0.0/8"])));
        assert!(!policy_blocks(&p(&["1.1.1.1"], &[])));
        assert!(!policy_blocks(&p(&[], &[])));
        assert!(!policy_blocks(&p(&[], &["example.com"])));
    }
}
