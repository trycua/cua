//! What a Space's network looks like, for the agent inside it and for
//! people: its tree's members with their names and addresses, the host's
//! name and the ports it may reach there, its policy and its exit.
//!
//! **Names.** Every member of a tree is `<space>.<tree>`: its own name and
//! the name of the tree's top-level Space, both as DNS labels (lowercase,
//! letters, digits and `-`). A Space reaches its parent as `<parent>.<tree>`
//! and a sibling as `<sibling>.<tree>`. The name `host` is this computer,
//! reachable only on the ports the Space's settings open (the `localhost`
//! preset), never anything else on it.
//!
//! The hint goes into the guest as `/etc/cua/network.json` (or
//! `/run/cua/network.json` where `/etc` isn't writable) whenever it changes,
//! and `cua network show --json` prints the same document.

use serde_json::{Value, json};

use crate::SpaceId;
use crate::error::Result;
use crate::network::NetworkSettings;
use crate::space::Space;
use cua_spacesd_client::pb;

/// The name a Space uses for the computer it runs on.
pub const HOST_NAME: &str = cua_vmm::tree::HOST_NAME;

/// Where a container's tree proxy answers on its own loopback
/// (`crate::tree_proxy`).
pub const TREE_PROXY_PORT: u16 = 41_920;

/// The capability of a guest that publishes host ports on its loopback
/// (`crate::host_ports`).
pub const HOST_PORTS_FEATURE: &str = "host_ports";

/// `s` as a DNS label: lowercase letters, digits and `-`, at most 63, never
/// empty (`space` instead).
pub fn dns_label(s: &str) -> String {
    cua_vmm::tree::dns_label(s)
}

/// The name of member `space` of tree `tree` (both Space names, not ids).
pub fn member_name(space: &str, tree: &str) -> String {
    format!("{}.{}", dns_label(space), dns_label(tree))
}

/// The name part of a canonical id (`local:studio` → `studio`).
pub fn short_name(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

/// One member of a tree, for the hint.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Member {
    /// Its id.
    pub id: String,
    /// `<space>.<tree>`.
    pub name: String,
    /// Its tree address, where VM members reach it (containers reach each
    /// other by name on their tree's network).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// `self`, `parent`, `child` or `member`.
    pub relation: String,
    /// The computer that runs it, when it is not this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

/// How the host's allowed ports are reached from inside the Space.
/// `at_localhost`: the Space's daemon publishes the host's ports on its own
/// loopback (`crate::host_ports`), so `localhost:<port>` works as written.
pub fn host_section(settings: &NetworkSettings, at_localhost: bool) -> Value {
    let ports: Vec<u16> = if settings.preset.opens_localhost() {
        settings.localhost_ports.clone()
    } else {
        Vec::new()
    };
    let all = settings.preset.opens_localhost() && settings.all_localhost_ports;
    json!({
        "name": HOST_NAME,
        "ports": ports,
        "all_ports": all,
        // Where to connect: `localhost:<port>` where the runtime publishes
        // the host's port on the Space's own loopback, else `host:<port>`.
        "reach": ports.iter().map(|p| if at_localhost {
            format!("localhost:{p}")
        } else {
            format!("{HOST_NAME}:{p}")
        }).collect::<Vec<_>>(),
        "at_localhost": at_localhost,
    })
}

impl crate::Spaces {
    /// The tree of Space `id` as this computer's book keeps it: the tree it
    /// was made for on another computer, else its root, else itself.
    pub(crate) fn book_tree_of(&self, id: &str) -> String {
        match self.inner.registry.meta(id) {
            Some(m) => m.overlay_tree.or(m.root).unwrap_or_else(|| id.to_string()),
            None => id.to_string(),
        }
    }

    /// The tree suffix of Space `key`: its tree's top-level Space's name as
    /// a DNS label.
    pub fn tree_suffix(&self, key: &str) -> Option<String> {
        Some(dns_label(short_name(&self.book_tree_of(key))))
    }

    /// The members of Space `key`'s tree (itself included), with names
    /// and tree addresses.
    pub fn topology_members(&self, key: &str) -> Vec<Member> {
        let all = self.inner.registry.all_meta();
        // The tree this computer's book keeps it in: for a Space made for a
        // tree that started on another computer, that tree.
        let root = self.book_tree_of(key);
        let tree = short_name(&root).to_string();
        let parent = all.get(key).and_then(|m| m.parent.clone());
        let addrs = cua_vmm::tree::addresses();
        let mut out: Vec<Member> = all
            .iter()
            .filter(|(id, m)| {
                *id == key || *id == &root || m.root.as_deref() == Some(root.as_str())
            })
            .map(|(id, m)| {
                let relation = if id == key {
                    "self"
                } else if parent.as_deref() == Some(id.as_str()) {
                    "parent"
                } else if m.parent.as_deref() == Some(key) {
                    "child"
                } else {
                    "member"
                };
                Member {
                    id: id.clone(),
                    name: member_name(short_name(id), &tree),
                    address: addrs.get(short_name(id)).map(|a| a.to_string()),
                    relation: relation.into(),
                    machine: None,
                }
            })
            .collect();
        // Members of this tree that run on other computers (the tree
        // overlay): their own tree address and name. `machine` holds the
        // machine's overlay id; [`Self::topology`] turns it into a name.
        let have: std::collections::BTreeSet<String> = out.iter().map(|m| m.name.clone()).collect();
        for r in cua_vmm::tree::remote_members()
            .into_iter()
            .filter(|r| r.tree == root)
        {
            let name = member_name(r.key.rsplit(':').next().unwrap_or(&r.key), &tree);
            if have.contains(&name) {
                continue;
            }
            out.push(Member {
                id: r.key.clone(),
                name,
                address: Some(r.addr.to_string()),
                relation: "member".into(),
                machine: Some(r.machine.clone()),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Whether Space `key` is a Linux VM (not a container, not macOS or
    /// Windows).
    pub(crate) async fn is_linux_vm(&self, key: &str, space: &Space) -> bool {
        if space.os_family() != pb::OsFamily::Linux {
            return false;
        }
        let Ok(SpaceId::Local { name }) = SpaceId::parse(key) else {
            return false;
        };
        match self.inner.sandboxes.connect(&name).await {
            Ok(sb) => crate::network::backend_kind(sb.runtime_type())
                .is_some_and(|k| k != cua_vmm::BackendKind::Container),
            Err(_) => false,
        }
    }

    /// Why Linux VM `key` can't see members of its tree that started after
    /// it, when that is so ([`cua_vmm::tree::relay_gap`]).
    pub(crate) async fn relay_note(&self, key: &str) -> Option<&'static str> {
        let name = match SpaceId::parse(key) {
            Ok(SpaceId::Local { name }) => name,
            _ => return None,
        };
        let book_key_has_gap = cua_vmm::tree::keys_named(&name)
            .into_iter()
            .any(|k| cua_vmm::tree::relay_gap(&k));
        if !book_key_has_gap {
            return None;
        }
        let space = self.space(key).await.ok()?;
        self.is_linux_vm(key, &space)
            .await
            .then_some(cua_vmm::tree::RELAY_LATE)
    }

    /// The topology hint of Space `key` (`/etc/cua/network.json`, `cua
    /// network show --json`'s `topology`).
    pub async fn topology(&self, key: &str) -> Result<Value> {
        let id = self.resolve(key)?.to_string();
        let meta = self.inner.registry.meta(&id).unwrap_or_default();
        let settings = crate::network::settings_of(Some(&meta));
        let at_localhost = match self.space(&id).await {
            Ok(s) => s.supports(HOST_PORTS_FEATURE),
            Err(_) => false,
        };
        let mut members = self.topology_members(&id);
        for m in &mut members {
            if let Some(overlay_id) = m.machine.take() {
                #[cfg(feature = "spaces-hotspot")]
                let named = self.overlay_machine_name(&overlay_id).await;
                #[cfg(not(feature = "spaces-hotspot"))]
                let named: Option<String> = {
                    let _ = &overlay_id;
                    None
                };
                m.machine = Some(named.unwrap_or_else(|| "another computer".to_string()));
            }
        }
        let me = members.iter().find(|m| m.id == id).cloned();
        #[cfg(feature = "spaces-hotspot")]
        let tree_proxy = self
            .host_ports_status(&id)
            .await
            .is_some_and(|s| s.tree_proxy);
        #[cfg(not(feature = "spaces-hotspot"))]
        let tree_proxy = false;
        let via = settings.via.as_ref();
        let relay_note = self.relay_note(&id).await;
        Ok(json!({
            "version": 1,
            "space": {
                "id": id,
                "name": me.as_ref().map(|m| m.name.clone()),
                "address": me.as_ref().and_then(|m| m.address.clone()),
            },
            "tree": {
                "root": self.book_tree_of(&id),
                "suffix": self.tree_suffix(&id),
                "members": members,
                // A Linux VM that started alone without cua-netgw can't see
                // the members that came after it: why, in plain words.
                "relay": relay_note.map(|note| json!({"state": "missing", "note": note})),
            },
            "policy": {
                "preset": settings.preset.as_str(),
                "summary": settings.summary(),
                "allow": settings.custom.allow,
                "deny": settings.custom.deny,
            },
            "via": via.map(|v| json!({
                "kind": match v {
                    crate::network::EgressVia::ThisMachine => "this-computer",
                    crate::network::EgressVia::Space(_) => "space",
                    crate::network::EgressVia::Socks5(_) => "socks5",
                },
                "label": v.label(),
                // Inside the Space every new process is told this too
                // (ALL_PROXY, HTTPS_PROXY, HTTP_PROXY and their lowercase
                // forms).
                "proxy_env": "ALL_PROXY",
            })),
            "host": host_section(&settings, at_localhost),
            // Members that aren't reachable by a route (a container's tree
            // has VMs, or members on other computers): connect through this
            // SOCKS5 proxy by name or tree address.
            "tree_proxy": tree_proxy.then(|| json!({
                "address": format!("127.0.0.1:{TREE_PROXY_PORT}"),
                "url": format!("socks5h://127.0.0.1:{TREE_PROXY_PORT}"),
            })),
        }))
    }

    /// Without the hotspot machinery there are no exits or host ports to
    /// keep (a Space with an exit set stays closed: it fails closed).
    #[cfg(not(feature = "spaces-hotspot"))]
    pub async fn reconcile_network(&self) {}

    /// [`Self::topology`] plus how it is doing now: the exit's state, the
    /// host ports answering at `localhost`, and the path to the computers
    /// of its tree (`cua network show --json`).
    pub async fn network_status(&self, key: &str) -> Result<Value> {
        let mut v = self.topology(key).await?;
        let id = self.resolve(key)?.to_string();
        #[cfg(feature = "spaces-hotspot")]
        {
            let exit = self.exits().await.into_iter().find(|e| e.space == id);
            v["exit"] = serde_json::to_value(exit).unwrap_or(Value::Null);
            let ports = self.host_ports_status(&id).await;
            v["host_ports"] = serde_json::to_value(ports).unwrap_or(Value::Null);
        }
        v["paths"] =
            serde_json::to_value(crate::paths::tree_paths(self, &id).await).unwrap_or(Value::Null);
        #[cfg(feature = "spaces-hotspot")]
        {
            v["overlay"] = serde_json::to_value(self.overlay_status().await).unwrap_or(Value::Null);
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_dns_labels() {
        assert_eq!(dns_label("Research Swarm"), "research-swarm");
        assert_eq!(dns_label("worker_1"), "worker-1");
        assert_eq!(dns_label("--"), "space");
        assert_eq!(
            member_name("worker_1", "Research Swarm"),
            "worker-1.research-swarm"
        );
        assert_eq!(short_name("local:studio"), "studio");
        assert_eq!(dns_label(&"a".repeat(80)).len(), 63);
    }

    #[test]
    fn the_host_section_lists_only_the_open_ports() {
        let mut s = NetworkSettings::preset(crate::network::NetworkPreset::Localhost);
        s.localhost_ports = vec![3000, 5432];
        let h = host_section(&s, true);
        assert_eq!(h["name"], "host");
        assert_eq!(h["ports"], json!([3000, 5432]));
        assert_eq!(h["reach"], json!(["localhost:3000", "localhost:5432"]));
        let named = host_section(&s, false);
        assert_eq!(named["reach"], json!(["host:3000", "host:5432"]));
        let internet = host_section(&NetworkSettings::internet(), true);
        assert_eq!(internet["ports"], json!([]));
    }
}
