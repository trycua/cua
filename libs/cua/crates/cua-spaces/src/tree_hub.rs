// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Containers in a VM's tree: the tree hub.
//!
//! VM members of a tree reach each other through their gateways' ingress
//! sockets ([`cua_vmm::tree`]); containers of a tree share a Docker network.
//! The hub joins the two on this computer: each running container of a tree
//! that has a VM member gets a tree address and a name in the book, and this
//! process answers on its ingress socket:
//!
//! - a VM member's gateway opens the socket and asks for a port
//!   (`CUATREE 1 <from> <port>`), after the VM's own rules allowed it;
//! - the hub checks that `<from>` is a running member of the same tree and
//!   that the container takes connections from its tree (its settings reach
//!   its tree), then connects to that port on the container's loopback over
//!   its own tunnel (`TunnelService.Forward`) and answers `OK`.
//!
//! The other way (a container connecting to a VM's tree address) can't use
//! a route: the container's network has none to the tree range, so a plain
//! connect fails (closed, never around the rules). A container reaches
//! members that are not containers through its tree proxy instead
//! ([`crate::tree_proxy`]).
//!
//! The hub also runs for containers whose tree has members on other computers
//! ([`crate::overlay`]): those machines' members reach the container through
//! the same ingress socket, carried over the mesh.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

#[cfg(unix)]
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;

use crate::error::Result;

/// One container's hub.
pub(crate) struct HubRun {
    key: String,
    #[allow(dead_code)]
    pub(crate) addr: Ipv4Addr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for HubRun {
    fn drop(&mut self) {
        self.task.abort();
        let _ = cua_vmm::tree::leave(&self.key);
    }
}

/// The hubs this process runs, by Space id.
pub(crate) type TreeHubTable = tokio::sync::Mutex<BTreeMap<String, HubRun>>;

/// The book key of container Space `name`.
pub fn hub_key(name: &str) -> String {
    cua_vmm::tree::key("container", name)
}

/// Parses `CUATREE 1 <from> <port>`: a tree address and a port.
pub fn parse_request(line: &str) -> Option<(Ipv4Addr, u16)> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() != 4 || f[0] != "CUATREE" || f[1] != "1" {
        return None;
    }
    let from: Ipv4Addr = f[2].parse().ok()?;
    let range = cua_vmm::egress::Net::parse(cua_vmm::tree::RANGE).ok()?;
    if !range.contains(from.into()) {
        return None;
    }
    let port: u16 = f[3].parse().ok().filter(|p| *p != 0)?;
    Some((from, port))
}

impl crate::Spaces {
    /// Starts or stops the hub of each running container on this computer
    /// whose tree has a VM member.
    pub async fn reconcile_tree_hub(&self) {
        let all = self.inner.registry.all_meta();
        let linked = self.linked_trees();
        let mut want: BTreeMap<String, (String, String, bool)> = BTreeMap::new();
        for (id, m) in &all {
            let Ok(crate::SpaceId::Local { name }) = crate::SpaceId::parse(id) else {
                continue;
            };
            if !self.is_running(id) {
                continue;
            }
            let tree = self.book_tree_of(id);
            // A tree with a member that isn't a container (a VM here or any
            // member on another computer): someone to carry for.
            let vms = cua_vmm::tree::running_members(&tree);
            if vms.iter().all(|k| k.starts_with("container:")) && !linked.contains(&tree) {
                continue;
            }
            let Ok(sb) = self.inner.sandboxes.connect(&name).await else {
                continue;
            };
            if crate::network::backend_kind(sb.runtime_type())
                != Some(cua_vmm::BackendKind::Container)
            {
                continue;
            }
            let settings = crate::network::settings_of(Some(m));
            let ingress = cua_vmm::tree::ingress_for(Some(&settings), false);
            want.insert(id.clone(), (name, tree, ingress));
        }
        let mut table = self.inner.tree_hubs.lock().await;
        table.retain(|id, run| want.contains_key(id) && !run.task.is_finished());
        for (id, (name, tree, ingress)) in want {
            if table.contains_key(&id) {
                let _ = cua_vmm::tree::set_ingress(&hub_key(&name), ingress);
                continue;
            }
            match self.start_hub(&id, &name, &tree, ingress).await {
                Ok(run) => {
                    table.insert(id, run);
                }
                Err(e) => tracing::warn!(space = %id, error = %e, "tree hub"),
            }
        }
    }

    async fn start_hub(&self, id: &str, name: &str, tree: &str, ingress: bool) -> Result<HubRun> {
        let key = hub_key(name);
        let joined = cua_vmm::tree::join_hub(&key, tree, ingress)
            .map_err(|e| crate::Error::host("tree networking", e.to_string()))?;
        #[cfg(unix)]
        let task = {
            let _ = std::fs::remove_file(&joined.sock);
            let listener = tokio::net::UnixListener::bind(&joined.sock)
                .map_err(|e| crate::Error::host("tree networking", e.to_string()))?;
            let spaces = self.clone();
            let id = id.to_string();
            let tree = tree.to_string();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let spaces = spaces.clone();
                    let id = id.clone();
                    let tree = tree.clone();
                    tokio::spawn(async move {
                        serve_one(&spaces, &id, &tree, stream).await;
                    });
                }
            })
        };
        #[cfg(not(unix))]
        let task = {
            let _ = id;
            tokio::spawn(async {})
        };
        Ok(HubRun {
            key,
            addr: joined.addr,
            task,
        })
    }
}

#[cfg(unix)]
async fn serve_one(spaces: &crate::Spaces, id: &str, tree: &str, stream: tokio::net::UnixStream) {
    let mut stream = tokio::io::BufReader::new(stream);
    let mut line = String::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_line(&mut line),
    )
    .await;
    if !matches!(read, Ok(Ok(n)) if n > 0 && line.len() < 256) {
        return;
    }
    let Some((from, port)) = parse_request(line.trim()) else {
        let _ = stream.get_mut().write_all(b"NO not a tree request\n").await;
        return;
    };
    let settings = crate::network::settings_of(spaces.inner.registry.meta(id).as_ref());
    if !cua_vmm::tree::ingress_for(Some(&settings), false) {
        let _ = stream
            .get_mut()
            .write_all(b"NO this Space takes no connections from its tree\n")
            .await;
        return;
    }
    if !cua_vmm::tree::is_running_member(tree, from) {
        let _ = stream
            .get_mut()
            .write_all(b"NO not a member of this tree\n")
            .await;
        return;
    }
    carry_into_guest(spaces, id, &mut stream, port).await;
}

/// Connects to `port` on Space `id`'s loopback over its tunnel, answers
/// `OK` (or `NO <why>`) on `stream` and carries the connection. The caller
/// has already checked that the connection may be made.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) async fn carry_into_guest<S>(spaces: &crate::Spaces, id: &str, stream: &mut S, port: u16)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let guest = match spaces.space(id).await {
        Ok(s) => s.open_loopback(port).await,
        Err(e) => Err(e),
    };
    let mut guest = match guest {
        Ok(g) => g,
        Err(e) => {
            let msg = format!("NO {}\n", e.to_string().replace('\n', " "));
            let _ = stream.write_all(msg.as_bytes()).await;
            return;
        }
    };
    if stream.write_all(b"OK\n").await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(stream, &mut guest).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tree_requests_parse() {
        assert_eq!(
            parse_request("CUATREE 1 198.19.0.4 8080"),
            Some((Ipv4Addr::new(198, 19, 0, 4), 8080))
        );
        assert_eq!(
            parse_request("CUATREE 1 10.0.0.4 8080"),
            None,
            "outside the range"
        );
        assert_eq!(parse_request("CUATREE 1 198.19.0.4 0"), None);
        assert_eq!(parse_request("CUATREE 2 198.19.0.4 80"), None);
        assert_eq!(parse_request("GET / HTTP/1.1"), None);
        assert_eq!(hub_key("db"), "container:db");
    }
}
