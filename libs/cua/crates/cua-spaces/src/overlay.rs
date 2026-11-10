// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The tree overlay: the members of one tree reach each other no matter
//! which of your computers runs them.
//!
//! Every member of a tree has one tree address and one name
//! (`<space>.<tree>`, [`crate::topology`]). On one computer the tree's VMs
//! reach each other through their gateways and the tree hub carries
//! containers ([`crate::tree_hub`]). This module extends that across the
//! account's machines, over the mesh the computers already share
//! ([`cua_mesh`]: WireGuard end to end, direct on the same network or
//! across the internet when the two NATs allow it, through the relay as
//! ciphertext when nothing else answers):
//!
//! - **Directory.** Each computer's daemon listens on its own loopback at
//!   [`OVERLAY_PORT`], registered with the mesh node so only the account's
//!   other computers reach it ([`cua_mesh::gang`]). A peer asks it which
//!   tree members the computer runs ([`Dir`]).
//! - **Mirror.** A machine that has a reason to link a tree (one of its
//!   Spaces was made for a tree that started elsewhere, or a tree started
//!   here has a member on another computer: [`crate::Spaces::linked_trees`])
//!   asks every other online machine and records each member of that tree it
//!   hears about as a *remote* member in its tree book
//!   ([`cua_vmm::tree::join_remote`]), with the address the member's own
//!   machine gave it. The gateways, tree relays and name resolvers see a
//!   remote member as one more peer with an ingress socket; this daemon
//!   serves that socket.
//! - **Dial.** A connection to a remote member's tree address passes the
//!   source Space's rules first (platform denies, the user's lists, then the
//!   tree's allow, as for a member on the same machine), then reaches the
//!   member's ingress socket here. The daemon opens a stream to the member's
//!   machine (a mesh forward to its overlay port), says which tree, from
//!   which address, to which member and port, and the other machine checks
//!   again before it connects to the guest: the member runs there, it is
//!   in that tree, it takes tree connections, and the source is a member of
//!   that tree there too. Both ends must agree.
//!
//! **Paths.** Nothing here chooses a path. The mesh node probes the local
//! network, direct internet and the relay for each pair of machines, uses
//! the fastest that answers and fails over, and the connection above it
//! keeps going. `cua mesh status` and `cua network show` print the path it
//! chose and why; `--prefer lan|direct|relay|auto` pins it.
//!
//! **Fail closed.** No mesh on this computer, an old peer, a peer that stops
//! answering for three rounds, an address two computers both gave: the
//! members that can't be carried are not recorded, so connections to them
//! are refused and logged as for any address that isn't a member. Nothing
//! falls back to a path around the rules, and a tree never reaches another
//! tree, the host, the LAN or the metadata address through the overlay: it
//! carries tree members' own listening ports and nothing else.
//!
//! **Limits.** TCP only. A tree whose top-level Space runs on another
//! machine than the one that coordinates it is linked through its children
//! (the ones made here and the ones made there), which share the tree; the
//! top-level Space itself keeps the tree its own machine gave it and is not
//! a member of the shared one ([`linked_trees_of`]). The overlay port is a loopback port on
//! the computer like the job gang ports ([`cua_mesh::gang`]), so every
//! connection is checked for who made it ([`same_user`]): only a process
//! of the user that runs the Cua daemon (the mesh node that carries the
//! other computers' streams) is served; a connection from another user of
//! the computer is closed before a byte is read. A loopback TCP socket has
//! no peer credential of its own, so the owner is looked up from the
//! kernel's socket table (`/proc/net/tcp` on Linux, `lsof` on macOS) and
//! anything that can't be established is refused (`CUA_TREE_OVERLAY=0`
//! turns the overlay off).

use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::time::Duration;

use cua_vmm::tree::{LocalMember, qualify, unqualify};
use serde::{Deserialize, Serialize};

/// The port of the overlay endpoint on every machine's loopback (and in its
/// mesh port registry).
pub const OVERLAY_PORT: u16 = 47_419;

/// The wire version of the endpoint.
pub const PROTOCOL: u32 = 1;

/// The longest request line.
#[cfg_attr(not(unix), allow(dead_code))]
const MAX_LINE: u64 = 512;

/// Rounds a computer may miss before its members are dropped.
pub const MISSED_ROUNDS: u8 = 3;

/// How long a request line, a directory or a connect may take.
#[cfg_attr(not(unix), allow(dead_code))]
const STEP: Duration = Duration::from_secs(10);

/// The most overlay connections a computer carries at once.
pub const MAX_STREAMS: usize = 256;

/// The user id that owns the socket whose local end is `client` and whose
/// remote end is `server`, from the text of Linux's `/proc/net/tcp`.
/// `None` when the table has no such connection.
pub fn proc_net_tcp_owner(
    table: &str,
    client: std::net::SocketAddrV4,
    server: std::net::SocketAddrV4,
) -> Option<u32> {
    fn hex(a: std::net::SocketAddrV4) -> String {
        // The kernel prints the address as the host-order hex of the
        // network-order word, so 127.0.0.1 is 0100007F, then the port.
        let o = a.ip().octets();
        format!(
            "{:02X}{:02X}{:02X}{:02X}:{:04X}",
            o[3],
            o[2],
            o[1],
            o[0],
            a.port()
        )
    }
    let (local, remote) = (hex(client), hex(server));
    table.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        // sl local rem st tx:rx tr:when retrnsmt uid ...
        if f.len() > 7 && f[1] == local && f[2] == remote {
            f[7].parse().ok()
        } else {
            None
        }
    })
}

/// Whether `lsof -F n` output (restricted to this user's sockets) lists the
/// connection `client` -> `server`.
pub fn lsof_lists(
    out: &str,
    client: std::net::SocketAddrV4,
    server: std::net::SocketAddrV4,
) -> bool {
    let want = format!("{client}->{server}");
    out.lines()
        .any(|l| l.strip_prefix('n') == Some(want.as_str()))
}

/// Whether the process that opened the loopback connection `client` ->
/// `server` runs as the same user as this daemon. Fails closed: a lookup
/// that can't be made says no.
#[cfg(unix)]
pub async fn same_user(client: std::net::SocketAddr, server: std::net::SocketAddr) -> bool {
    let (std::net::SocketAddr::V4(c), std::net::SocketAddr::V4(s)) = (client, server) else {
        return false;
    };
    if !c.ip().is_loopback() {
        return false;
    }
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    let found = tokio::task::spawn_blocking(move || same_user_blocking(c, s, me))
        .await
        .unwrap_or(false);
    if !found {
        tracing::warn!(%client, "tree overlay: refused a connection that is not from this user's processes");
    }
    found
}

#[cfg(unix)]
fn same_user_blocking(c: std::net::SocketAddrV4, s: std::net::SocketAddrV4, me: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/net/tcp")
            .ok()
            .and_then(|t| proc_net_tcp_owner(&t, c, s))
            == Some(me)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // lsof run by an ordinary user lists only that user's processes;
        // `-u` makes it explicit. The connection is there only when one of
        // them owns the client end.
        let out = std::process::Command::new("/usr/sbin/lsof")
            .args(["-nP", "-a", "-u", &me.to_string(), "-Fn"])
            .arg(format!("-iTCP@127.0.0.1:{}", s.port()))
            .output();
        match out {
            Ok(o) => lsof_lists(&String::from_utf8_lossy(&o.stdout), c, s),
            Err(_) => false,
        }
    }
}

/// No overlay without Unix.
#[cfg(not(unix))]
pub async fn same_user(_client: std::net::SocketAddr, _server: std::net::SocketAddr) -> bool {
    false
}

/// One member a computer runs, as it tells its peers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirMember {
    /// Its tree, qualified ([`qualify`]).
    pub tree: String,
    /// Its Space's name.
    pub name: String,
    /// Its tree address.
    pub addr: Ipv4Addr,
    /// It takes connections from its tree.
    pub ingress: bool,
}

/// What a computer answers to a directory request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dir {
    /// [`PROTOCOL`].
    pub version: u32,
    /// The computer's overlay id ([`cua_vmm::tree::overlay_id`]).
    pub machine: String,
    /// The members it runs.
    pub members: Vec<DirMember>,
}

/// Trees (as this computer's book names them) that have, or should have,
/// members on other computers, from the Spaces this computer knows:
///
/// - the tree a Space here was made for on another computer
///   (`overlay_tree`);
/// - a tree started here (its top-level Space is local) with a member
///   registered as another computer's Space;
/// - a tree whose top-level Space runs on another computer (its root is a
///   Space registered here as `relay:<machine>`), as soon as a Space here
///   or registered here belongs to it. Its children made on that computer
///   joined it as `<this computer's overlay id>/<root id>`, and this
///   machine's own children keep the root's id: the same tree, so each side
///   records the other's members. The top-level Space itself keeps the tree
///   its own machine gave it (it is not a member of the shared one).
pub fn linked_trees_of(
    all: &std::collections::BTreeMap<String, crate::registry::SpaceMeta>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (id, m) in all {
        if id.starts_with("local:")
            && let Some(t) = &m.overlay_tree
        {
            out.insert(t.clone());
        }
        let Some(root) = &m.root else { continue };
        if root.starts_with("local:") {
            if !id.starts_with("local:") && all.contains_key(root) {
                out.insert(root.clone());
            }
        } else if root.starts_with("relay:") && m.overlay_tree.is_none() {
            // A member of a tree that started on another computer, as this
            // machine's book names it.
            out.insert(root.clone());
        }
    }
    out
}

/// The directory of the computer `me` that runs `local`.
pub fn dir_of(me: &str, local: &[LocalMember]) -> Dir {
    Dir {
        version: PROTOCOL,
        machine: me.to_string(),
        members: local
            .iter()
            .map(|m| DirMember {
                tree: m.tree.clone(),
                name: m
                    .key
                    .split_once(':')
                    .map_or(m.key.as_str(), |(_, n)| n)
                    .to_string(),
                addr: m.addr,
                ingress: m.ingress,
            })
            .collect(),
    }
}

/// A member of another computer this computer records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mirror {
    /// Its book key ([`cua_vmm::tree::remote_key`]).
    pub key: String,
    /// The computer that runs it (its overlay id).
    pub machine: String,
    /// Its tree as this computer's book keeps it.
    pub tree: String,
    /// Its name.
    pub name: String,
    /// Its tree address.
    pub addr: Ipv4Addr,
    /// It takes connections from its tree.
    pub ingress: bool,
}

/// The members of `dir` this computer (`me`) records: those of a tree in
/// `linked` (as this computer's book names it). The computer's own members
/// and members of trees nothing here is part of are left out.
pub fn plan_mirror(me: &str, linked: &BTreeSet<String>, dir: &Dir) -> Vec<Mirror> {
    if dir.machine == me || dir.version != PROTOCOL {
        return Vec::new();
    }
    let range = cua_vmm::egress::Net::parse(cua_vmm::tree::RANGE).expect("a valid range");
    dir.members
        .iter()
        .filter_map(|m| {
            let tree = unqualify(me, &m.tree);
            (linked.contains(&tree) && range.contains(m.addr.into())).then(|| Mirror {
                key: cua_vmm::tree::remote_key(&dir.machine, &m.name),
                machine: dir.machine.clone(),
                tree,
                name: m.name.clone(),
                addr: m.addr,
                ingress: m.ingress,
            })
        })
        .collect()
}

/// A request to the endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `CUAOVL 1 DIR`: the computer's members.
    Dir,
    /// `CUAOVL 1 DIAL <tree> <from> <to> <port>`: a connection from tree
    /// address `from` to member `to` on `port`, in `tree`.
    Dial {
        /// The tree, qualified.
        tree: String,
        /// The source's tree address.
        from: Ipv4Addr,
        /// The member's tree address.
        to: Ipv4Addr,
        /// The member's port (its own loopback).
        port: u16,
    },
}

/// A tree id as one word on the wire.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'_' | b'/' | b'-') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// [`escape`] undone; `None` for a malformed escape.
pub fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Parses a request line.
pub fn parse_request(line: &str) -> Option<Request> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 3 || f[0] != "CUAOVL" || f[1] != "1" {
        return None;
    }
    match (f[2], f.len()) {
        ("DIR", 3) => Some(Request::Dir),
        ("DIAL", 7) => {
            let range = cua_vmm::egress::Net::parse(cua_vmm::tree::RANGE).ok()?;
            let from: Ipv4Addr = f[4].parse().ok()?;
            let to: Ipv4Addr = f[5].parse().ok()?;
            if !range.contains(from.into()) || !range.contains(to.into()) {
                return None;
            }
            Some(Request::Dial {
                tree: unescape(f[3])?,
                from,
                to,
                port: f[6].parse().ok().filter(|p| *p != 0)?,
            })
        }
        _ => None,
    }
}

/// The request line for a connection.
pub fn dial_line(tree: &str, from: Ipv4Addr, to: Ipv4Addr, port: u16) -> String {
    format!("CUAOVL 1 DIAL {} {from} {to} {port}\n", escape(tree))
}

/// What a computer checks before it connects a dialed connection to a
/// member's guest: the member runs here at `to`, in that tree, takes tree
/// connections, and `from` is a running member of the tree here too (the
/// source's machine said so; both ends must agree). `is_member(tree, addr)`
/// asks this computer's book. Returns the member to connect to.
pub fn check_dial(
    me: &str,
    local: &[LocalMember],
    tree: &str,
    from: Ipv4Addr,
    to: Ipv4Addr,
    is_member: impl Fn(&str, Ipv4Addr) -> bool,
) -> Result<LocalMember, String> {
    let member = local
        .iter()
        .find(|m| m.addr == to)
        .ok_or_else(|| "no such member runs here".to_string())?;
    if member.tree != qualify(me, tree) {
        return Err("not a member of that tree".into());
    }
    if !member.ingress {
        return Err("this Space takes no connections from its tree".into());
    }
    if !is_member(&unqualify(me, &member.tree), from) {
        return Err("not a member of this tree".into());
    }
    Ok(member.clone())
}

/// The Space id of the member with book key `key` (`qemu:db` is
/// `local:db`).
pub fn space_of_key(key: &str) -> String {
    format!("local:{}", key.split_once(':').map_or(key, |(_, n)| n))
}

/// What the overlay looks like now, for `cua network show` and the apps.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OverlayStatus {
    /// The endpoint is up (the tree has members on other computers to link
    /// and this computer has a mesh).
    pub active: bool,
    /// Why it isn't, or what is wrong, in plain words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The other computers, with the path to each and the members of linked
    /// trees they run.
    pub machines: Vec<OverlayMachine>,
    /// Members that can't be reached, with why.
    pub refused: Vec<String>,
}

/// One other machine in [`OverlayStatus`].
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OverlayMachine {
    /// Its name.
    pub name: String,
    /// `lan`, `direct` or `relay`.
    pub path: String,
    /// Why the mesh chose it, in plain words.
    pub why: String,
    /// The path's round trip, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    /// Members of linked trees it runs.
    pub members: Vec<String>,
}

#[cfg(unix)]
mod run {
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::SocketAddr;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// A remote member's ingress socket, served until dropped.
    pub(crate) struct MirrorRun {
        pub(crate) mirror: Mirror,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for MirrorRun {
        fn drop(&mut self) {
            self.task.abort();
            let _ = cua_vmm::tree::forget_remote(&self.mirror.key);
            let _ = std::fs::remove_file(cua_vmm::tree::sock_path(self.mirror.addr));
        }
    }

    /// What the daemon keeps for the overlay.
    #[derive(Default)]
    pub struct Overlay {
        registration: Option<cua_mesh::gang::Registration>,
        endpoint: Option<tokio::task::JoinHandle<()>>,
        ctl: Option<cua_mesh::gang::ControlClient>,
        /// A mesh forward to each computer's overlay port, by relay machine
        /// id (valid while `ctl` is).
        forwards: BTreeMap<String, SocketAddr>,
        mirrors: BTreeMap<String, MirrorRun>,
        missed: BTreeMap<String, u8>,
        /// Overlay id -> (relay machine id, name).
        machines: BTreeMap<String, (String, String)>,
        status: OverlayStatus,
        limit: Option<std::sync::Arc<tokio::sync::Semaphore>>,
    }

    impl Overlay {
        fn down(&mut self, why: Option<String>) {
            self.mirrors.clear();
            self.missed.clear();
            self.forwards.clear();
            self.ctl = None;
            if let Some(t) = self.endpoint.take() {
                // The listener goes with the task.
                t.abort();
            }
            self.registration = None;
            self.machines.clear();
            self.status = OverlayStatus {
                active: false,
                note: why,
                ..Default::default()
            };
        }
    }

    /// The table in [`crate::spaces`]'s inner state.
    pub(crate) type Table = tokio::sync::Mutex<Overlay>;

    async fn read_line<R: tokio::io::AsyncRead + tokio::io::AsyncBufRead + Unpin>(
        r: &mut R,
    ) -> Option<String> {
        let mut line = String::new();
        let n = tokio::time::timeout(STEP, (&mut *r).take(MAX_LINE).read_line(&mut line))
            .await
            .ok()?
            .ok()?;
        (n > 0 && line.ends_with('\n')).then(|| line.trim().to_string())
    }

    /// A directory request to the computer whose overlay port is forwarded
    /// at `addr`.
    async fn query_dir(addr: SocketAddr) -> Result<Dir, String> {
        tokio::time::timeout(STEP, async {
            let mut s = tokio::net::TcpStream::connect(addr)
                .await
                .map_err(|e| e.to_string())?;
            s.write_all(b"CUAOVL 1 DIR\n")
                .await
                .map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            (&mut s)
                .take(1 << 20)
                .read_to_end(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::from_slice::<Dir>(&buf).map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "no answer in time".to_string())?
    }

    impl crate::Spaces {
        /// Trees (as this computer's book names them) that have, or should
        /// have, members on other computers: the tree a Space here was made
        /// for on another computer, and trees started here with a member
        /// registered as another computer's Space.
        pub fn linked_trees(&self) -> BTreeSet<String> {
            linked_trees_of(&self.inner.registry.all_meta())
        }

        /// Whether the overlay is switched off (`CUA_TREE_OVERLAY=0`).
        fn overlay_off() -> bool {
            matches!(
                std::env::var("CUA_TREE_OVERLAY").as_deref(),
                Ok("0" | "off" | "false" | "no")
            )
        }

        /// Brings the overlay in line: serves the endpoint, asks the other
        /// machines for the members of linked trees and records them. The
        /// daemon calls it with the rest of the network reconcile.
        pub async fn reconcile_overlay(&self) {
            let linked = self.linked_trees();
            if linked.is_empty() || Self::overlay_off() {
                let mut ov = self.inner.overlay.lock().await;
                let why = Self::overlay_off().then(|| "The tree overlay is turned off here".into());
                ov.down(why);
                return;
            }
            let home = self.home_dir().to_path_buf();
            // Mesh, control socket, path table and a forward per online
            // machine, under the lock; the directory requests without it.
            let (targets, paths) = {
                let mut ov = self.inner.overlay.lock().await;
                if ov.ctl.is_none() {
                    match cua_mesh::gang::ControlClient::connect(&crate::paths::control_socket(
                        &home,
                    ))
                    .await
                    {
                        Ok(c) => ov.ctl = Some(c),
                        Err(_) => {
                            ov.down(Some(
                                "No mesh on this computer: members on other computers can't be reached"
                                    .into(),
                            ));
                            return;
                        }
                    }
                }
                let status = match ov.ctl.as_mut().expect("just set").status().await {
                    Ok((_, p)) => p,
                    Err(e) => {
                        ov.down(Some(format!("The mesh didn't answer: {e}")));
                        return;
                    }
                };
                if ov.endpoint.as_ref().is_none_or(|t| t.is_finished())
                    && let Err(e) = self.start_endpoint(&mut ov, &home).await
                {
                    ov.down(Some(e));
                    return;
                }
                let mut targets: Vec<(String, String, SocketAddr)> = Vec::new();
                for m in status.iter().filter(|m| m.online && !m.machine.is_empty()) {
                    let addr = match ov.forwards.get(&m.machine) {
                        Some(a) => *a,
                        None => {
                            let r = ov
                                .ctl
                                .as_mut()
                                .expect("set")
                                .forward(&m.machine, OVERLAY_PORT)
                                .await;
                            match r {
                                Ok(f) => {
                                    ov.forwards.insert(m.machine.clone(), f.local);
                                    f.local
                                }
                                Err(e) => {
                                    tracing::debug!(machine = %m.name, error = %e, "overlay forward");
                                    continue;
                                }
                            }
                        }
                    };
                    targets.push((m.machine.clone(), m.name.clone(), addr));
                }
                (targets, status)
            };
            let answers = futures_util::future::join_all(targets.iter().map(|(_, _, a)| {
                let a = *a;
                async move { query_dir(a).await }
            }))
            .await;
            let me = cua_vmm::tree::overlay_id();
            let mut ov = self.inner.overlay.lock().await;
            let mut want: BTreeMap<String, Mirror> = BTreeMap::new();
            let mut machines: Vec<OverlayMachine> = Vec::new();
            for ((machine, name, _), answer) in targets.iter().zip(answers) {
                let path = paths.iter().find(|p| &p.machine == machine);
                let mut info = OverlayMachine {
                    name: name.clone(),
                    path: path.map(|p| p.kind.clone()).unwrap_or_default(),
                    why: path.map(|p| p.why.clone()).unwrap_or_default(),
                    rtt_ms: path.and_then(|p| p.rtt_ms),
                    members: Vec::new(),
                };
                match answer {
                    Ok(dir) => {
                        ov.missed.remove(machine);
                        ov.machines
                            .insert(dir.machine.clone(), (machine.clone(), name.clone()));
                        for m in plan_mirror(&me, &linked, &dir) {
                            info.members
                                .push(format!("{}.{}", m.name, tree_label(&m.tree)));
                            want.insert(m.key.clone(), m);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(machine = %name, error = %e, "overlay directory");
                        let missed = ov.missed.entry(machine.clone()).or_default();
                        *missed = missed.saturating_add(1);
                        // Keep what it had for a few rounds: one lost
                        // answer isn't a reason to cut a tree in two.
                        if *missed < MISSED_ROUNDS {
                            let keep: Vec<String> = ov
                                .mirrors
                                .iter()
                                .filter(|(_, r)| {
                                    ov.machines
                                        .get(&r.mirror.machine)
                                        .is_some_and(|(id, _)| id == machine)
                                })
                                .map(|(k, r)| {
                                    info.members.push(format!(
                                        "{}.{}",
                                        r.mirror.name,
                                        tree_label(&r.mirror.tree)
                                    ));
                                    k.clone()
                                })
                                .collect();
                            for k in keep {
                                if let Some(r) = ov.mirrors.get(&k) {
                                    want.insert(k, r.mirror.clone());
                                }
                            }
                        }
                    }
                }
                machines.push(info);
            }
            // Stale mirrors go (their sockets close, the book forgets them);
            // new or changed ones are recorded and served.
            ov.mirrors.retain(|k, r| want.get(k) == Some(&r.mirror));
            let mut refused = Vec::new();
            for (key, m) in want {
                if ov.mirrors.contains_key(&key) {
                    continue;
                }
                match self.start_mirror(&m) {
                    Ok(run) => {
                        ov.mirrors.insert(key, run);
                    }
                    Err(e) => {
                        tracing::warn!(member = %m.name, error = %e, "tree overlay member");
                        refused.push(format!("{}.{}: {e}", m.name, tree_label(&m.tree)));
                    }
                }
            }
            ov.status = OverlayStatus {
                active: true,
                note: None,
                machines,
                refused,
            };
        }

        /// Stops the overlay (the daemon is shutting down).
        pub async fn overlay_stop(&self) {
            self.inner.overlay.lock().await.down(None);
        }

        /// What the overlay looks like now.
        pub async fn overlay_status(&self) -> OverlayStatus {
            self.inner.overlay.lock().await.status.clone()
        }

        /// The name of the computer with overlay id `id`, once it answered.
        pub async fn overlay_machine_name(&self, id: &str) -> Option<String> {
            self.inner
                .overlay
                .lock()
                .await
                .machines
                .get(id)
                .map(|(_, n)| n.clone())
        }

        async fn start_endpoint(
            &self,
            ov: &mut Overlay,
            home: &std::path::Path,
        ) -> Result<(), String> {
            let registry = cua_mesh::gang::PortRegistry::new(cua_mesh::gang::ports_dir(home));
            let registration = registry
                .register(OVERLAY_PORT, "tree overlay")
                .map_err(|e| format!("the overlay port {OVERLAY_PORT} is taken ({e})"))?;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", OVERLAY_PORT))
                .await
                .map_err(|e| format!("the overlay port {OVERLAY_PORT} is taken ({e})"))?;
            let limit = ov
                .limit
                .get_or_insert_with(|| {
                    std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_STREAMS))
                })
                .clone();
            let spaces = self.clone();
            ov.registration = Some(registration);
            ov.endpoint = Some(tokio::spawn(async move {
                loop {
                    let Ok((stream, peer)) = listener.accept().await else {
                        return;
                    };
                    // Only this user's own processes may ask.
                    let local = stream.local_addr().ok();
                    if !match local {
                        Some(l) => same_user(peer, l).await,
                        None => false,
                    } {
                        drop(stream);
                        continue;
                    }
                    let Ok(permit) = limit.clone().try_acquire_owned() else {
                        // Over the limit: refused, never queued.
                        drop(stream);
                        continue;
                    };
                    let spaces = spaces.clone();
                    tokio::spawn(async move {
                        serve_peer(&spaces, stream).await;
                        drop(permit);
                    });
                }
            }));
            Ok(())
        }

        /// Records remote member `m` and serves its ingress socket.
        fn start_mirror(&self, m: &Mirror) -> Result<MirrorRun, String> {
            let joined = cua_vmm::tree::join_remote(&m.key, &m.machine, &m.tree, m.addr, m.ingress)
                .map_err(|e| e.to_string())?;
            let sock = std::path::PathBuf::from(&joined.sock);
            let _ = std::fs::remove_file(&sock);
            let listener = match tokio::net::UnixListener::bind(&sock) {
                Ok(l) => l,
                Err(e) => {
                    let _ = cua_vmm::tree::forget_remote(&m.key);
                    return Err(e.to_string());
                }
            };
            let spaces = self.clone();
            let mirror = m.clone();
            let task = {
                let mirror = mirror.clone();
                tokio::spawn(async move {
                    loop {
                        let Ok((stream, _)) = listener.accept().await else {
                            return;
                        };
                        let spaces = spaces.clone();
                        let mirror = mirror.clone();
                        tokio::spawn(async move {
                            serve_remote(&spaces, &mirror, stream).await;
                        });
                    }
                })
            };
            Ok(MirrorRun { mirror, task })
        }

        /// A loopback address that reaches the overlay port of machine
        /// `overlay_id`, over the mesh (a forward the control socket keeps).
        async fn overlay_forward(&self, overlay_id: &str) -> Result<SocketAddr, String> {
            let mut ov = self.inner.overlay.lock().await;
            let machine = ov
                .machines
                .get(overlay_id)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| "that computer isn't on the mesh right now".to_string())?;
            if let Some(a) = ov.forwards.get(&machine) {
                return Ok(*a);
            }
            let f = ov
                .ctl
                .as_mut()
                .ok_or_else(|| "no mesh on this computer".to_string())?
                .forward(&machine, OVERLAY_PORT)
                .await?;
            ov.forwards.insert(machine, f.local);
            Ok(f.local)
        }

        /// Forgets the forward to `overlay_id`'s machine (it stopped
        /// answering), so the next connection makes a new one.
        async fn overlay_forward_failed(&self, overlay_id: &str) {
            let mut ov = self.inner.overlay.lock().await;
            if let Some((machine, _)) = ov.machines.get(overlay_id).cloned() {
                ov.forwards.remove(&machine);
            }
        }
    }

    fn tree_label(tree: &str) -> String {
        cua_vmm::tree::dns_label(tree.rsplit(':').next().unwrap_or(tree))
    }

    /// Opens a connection to port `port` of the member at tree address `to`
    /// that runs on the computer with overlay id `machine`, as tree address
    /// `from` of `tree` (as this computer's book names it), over the mesh. The
    /// other machine checks again and answers `OK` or `NO <why>`.
    pub(crate) async fn connect_remote(
        spaces: &crate::Spaces,
        machine: &str,
        tree: &str,
        from: std::net::Ipv4Addr,
        to: std::net::Ipv4Addr,
        port: u16,
    ) -> Result<tokio::io::BufReader<tokio::net::TcpStream>, String> {
        let me = cua_vmm::tree::overlay_id();
        let line = dial_line(&qualify(&me, tree), from, to, port);
        let mut last = String::new();
        // Twice: the first try may find a forward the mesh dropped.
        for _ in 0..2 {
            let addr = spaces.overlay_forward(machine).await?;
            let attempt = tokio::time::timeout(STEP, async {
                let mut s = tokio::net::TcpStream::connect(addr)
                    .await
                    .map_err(|e| e.to_string())?;
                s.write_all(line.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                let mut r = tokio::io::BufReader::new(s);
                let reply = read_line(&mut r)
                    .await
                    .ok_or_else(|| "the other machine closed the connection".to_string())?;
                if reply == "OK" {
                    Ok(r)
                } else {
                    Err(reply.strip_prefix("NO ").unwrap_or(&reply).to_string())
                }
            })
            .await
            .unwrap_or_else(|_| Err("no answer in time".into()));
            match attempt {
                Ok(r) => return Ok(r),
                Err(e) => {
                    last = e;
                    spaces.overlay_forward_failed(machine).await;
                }
            }
        }
        Err(last)
    }

    /// A member of this computer's tree connects to a member that runs on
    /// another computer (this is the member's ingress socket here).
    async fn serve_remote(spaces: &crate::Spaces, m: &Mirror, stream: tokio::net::UnixStream) {
        let mut stream = tokio::io::BufReader::new(stream);
        let Some(line) = read_line(&mut stream).await else {
            return;
        };
        let Some((from, port)) = crate::tree_hub::parse_request(&line) else {
            let _ = stream.get_mut().write_all(b"NO not a tree request\n").await;
            return;
        };
        if !m.ingress {
            let _ = stream
                .get_mut()
                .write_all(b"NO that Space takes no connections from its tree\n")
                .await;
            return;
        }
        // The same membership check as for a member on this computer: the
        // source must be a running member of the tree.
        if !cua_vmm::tree::is_running_member(&m.tree, from) {
            let _ = stream
                .get_mut()
                .write_all(b"NO not a member of this tree\n")
                .await;
            return;
        }
        let mut remote = match connect_remote(spaces, &m.machine, &m.tree, from, m.addr, port).await
        {
            Ok(r) => r,
            Err(why) => {
                let msg = format!("NO {}\n", why.replace('\n', " "));
                let _ = stream.get_mut().write_all(msg.as_bytes()).await;
                return;
            }
        };
        if stream.get_mut().write_all(b"OK\n").await.is_err() {
            return;
        }
        let _ = tokio::io::copy_bidirectional(&mut stream, &mut remote).await;
    }

    /// Another machine of the account asks this computer's overlay endpoint
    /// (through the mesh) for its members or for a connection into one.
    async fn serve_peer(spaces: &crate::Spaces, stream: tokio::net::TcpStream) {
        let mut stream = tokio::io::BufReader::new(stream);
        let Some(line) = read_line(&mut stream).await else {
            return;
        };
        match parse_request(&line) {
            Some(Request::Dir) => {
                let me = cua_vmm::tree::overlay_id();
                let dir = dir_of(&me, &cua_vmm::tree::local_members());
                let body = serde_json::to_vec(&dir).unwrap_or_default();
                let _ = stream.get_mut().write_all(&body).await;
                let _ = stream.get_mut().shutdown().await;
            }
            Some(Request::Dial {
                tree,
                from,
                to,
                port,
            }) => {
                let me = cua_vmm::tree::overlay_id();
                let member = check_dial(
                    &me,
                    &cua_vmm::tree::local_members(),
                    &tree,
                    from,
                    to,
                    cua_vmm::tree::is_running_member,
                );
                match member {
                    Ok(m) => {
                        let id = space_of_key(&m.key);
                        crate::tree_hub::carry_into_guest(spaces, &id, &mut stream, port).await;
                    }
                    Err(why) => {
                        let _ = stream
                            .get_mut()
                            .write_all(format!("NO {why}\n").as_bytes())
                            .await;
                    }
                }
            }
            None => {
                let _ = stream
                    .get_mut()
                    .write_all(b"NO not an overlay request\n")
                    .await;
            }
        }
    }
}

#[cfg(unix)]
pub(crate) use run::{Table, connect_remote};

/// Without the mesh's control socket (not Unix) there is no overlay.
#[cfg(not(unix))]
pub(crate) type Table = tokio::sync::Mutex<()>;

/// No overlay without Unix: nothing can be reached on another computer.
#[cfg(not(unix))]
pub(crate) async fn connect_remote(
    _spaces: &crate::Spaces,
    _machine: &str,
    _tree: &str,
    _from: Ipv4Addr,
    _to: Ipv4Addr,
    _port: u16,
) -> Result<tokio::io::BufReader<tokio::net::TcpStream>, String> {
    Err("the tree overlay needs macOS or Linux".into())
}

#[cfg(not(unix))]
impl crate::Spaces {
    /// Trees with members on other computers (none without the overlay).
    pub fn linked_trees(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    /// The overlay needs macOS or Linux.
    pub async fn reconcile_overlay(&self) {}

    /// Stops the overlay (nothing to stop).
    pub async fn overlay_stop(&self) {}

    /// What the overlay looks like now.
    pub async fn overlay_status(&self) -> OverlayStatus {
        OverlayStatus {
            note: Some("The tree overlay needs macOS or Linux".into()),
            ..Default::default()
        }
    }

    /// The name of a computer (none without the overlay).
    pub async fn overlay_machine_name(&self, _id: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lm(key: &str, tree: &str, addr: [u8; 4], ingress: bool) -> LocalMember {
        LocalMember {
            key: key.into(),
            tree: tree.into(),
            addr: Ipv4Addr::from(addr),
            ingress,
        }
    }

    #[test]
    fn requests_parse_and_nothing_else_does() {
        assert_eq!(parse_request("CUAOVL 1 DIR"), Some(Request::Dir));
        assert_eq!(
            parse_request("CUAOVL 1 DIAL aa11/local:app 198.19.0.1 198.19.7.2 8080"),
            Some(Request::Dial {
                tree: "aa11/local:app".into(),
                from: Ipv4Addr::new(198, 19, 0, 1),
                to: Ipv4Addr::new(198, 19, 7, 2),
                port: 8080,
            })
        );
        for bad in [
            "CUAOVL 2 DIR",
            "CUAOVL 1 DIR extra",
            "CUAOVL 1 DIAL t 10.0.0.1 198.19.7.2 80",
            "CUAOVL 1 DIAL t 198.19.0.1 10.0.0.2 80",
            "CUAOVL 1 DIAL t 198.19.0.1 198.19.7.2 0",
            "CUAOVL 1 DIAL t 198.19.0.1 198.19.7.2",
            "CUATREE 1 198.19.0.1 80",
            "GET / HTTP/1.1",
            "",
        ] {
            assert_eq!(parse_request(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_dial_line_round_trips_a_tree_id_with_odd_characters() {
        let line = dial_line(
            "aa11/local:my app",
            Ipv4Addr::new(198, 19, 0, 1),
            Ipv4Addr::new(198, 19, 7, 2),
            443,
        );
        assert!(!line.trim_end().contains("my app"), "one word on the wire");
        assert_eq!(
            parse_request(line.trim()),
            Some(Request::Dial {
                tree: "aa11/local:my app".into(),
                from: Ipv4Addr::new(198, 19, 0, 1),
                to: Ipv4Addr::new(198, 19, 7, 2),
                port: 443,
            })
        );
        assert_eq!(unescape("%zz"), None);
        assert_eq!(unescape("%4"), None);
    }

    #[test]
    fn the_directory_lists_local_members_by_name_with_their_qualified_tree() {
        let d = dir_of(
            "bb22",
            &[lm(
                "microvm:worker",
                "aa11/local:app",
                [198, 19, 7, 1],
                true,
            )],
        );
        assert_eq!(d.version, PROTOCOL);
        assert_eq!(d.machine, "bb22");
        assert_eq!(d.members[0].name, "worker");
        assert_eq!(d.members[0].tree, "aa11/local:app");
        let back: Dir = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn only_members_of_linked_trees_are_recorded() {
        let dir = Dir {
            version: PROTOCOL,
            machine: "bb22".into(),
            members: vec![
                DirMember {
                    tree: "aa11/local:app".into(),
                    name: "worker".into(),
                    addr: Ipv4Addr::new(198, 19, 7, 1),
                    ingress: true,
                },
                DirMember {
                    tree: "bb22/local:other".into(),
                    name: "stranger".into(),
                    addr: Ipv4Addr::new(198, 19, 7, 2),
                    ingress: true,
                },
                DirMember {
                    tree: "aa11/local:app".into(),
                    name: "outside".into(),
                    addr: Ipv4Addr::new(10, 0, 0, 5),
                    ingress: true,
                },
            ],
        };
        let linked = BTreeSet::from(["local:app".to_string()]);
        // Machine aa11 (the tree started there): its book says `local:app`.
        let m = plan_mirror("aa11", &linked, &dir);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].tree, "local:app");
        assert_eq!(m[0].key, "remote:bb22:worker");
        // A machine that is not part of the tree sees nothing to record.
        assert!(plan_mirror("cc33", &linked, &dir).is_empty());
        // A hosted machine's book names the tree qualified.
        let hosted = BTreeSet::from(["aa11/local:app".to_string()]);
        assert_eq!(plan_mirror("cc33", &hosted, &dir).len(), 1);
        // Never a computer's own members back, nor a newer protocol.
        assert!(plan_mirror("bb22", &linked, &dir).is_empty());
        let mut newer = dir.clone();
        newer.version = 2;
        assert!(plan_mirror("aa11", &linked, &newer).is_empty());
    }

    #[test]
    fn a_dial_is_checked_on_the_machine_that_runs_the_member() {
        let local = [
            lm("microvm:db", "aa11/local:app", [198, 19, 7, 1], true),
            lm("qemu:closed", "aa11/local:app", [198, 19, 7, 2], false),
        ];
        let from = Ipv4Addr::new(198, 19, 0, 1);
        let to = Ipv4Addr::new(198, 19, 7, 1);
        let member = |_: &str, a: Ipv4Addr| a == from;
        // The member runs here, in that tree, takes tree connections, and the
        // source is a member of the tree here too.
        assert_eq!(
            check_dial("bb22", &local, "aa11/local:app", from, to, member)
                .unwrap()
                .key,
            "microvm:db"
        );
        // Another tree's id.
        assert!(check_dial("bb22", &local, "aa11/local:other", from, to, member).is_err());
        // A member that takes no tree connections.
        assert!(
            check_dial(
                "bb22",
                &local,
                "aa11/local:app",
                from,
                Ipv4Addr::new(198, 19, 7, 2),
                member
            )
            .unwrap_err()
            .contains("takes no connections")
        );
        // A source this computer doesn't know as a member of the tree.
        assert!(
            check_dial(
                "bb22",
                &local,
                "aa11/local:app",
                Ipv4Addr::new(198, 19, 9, 9),
                to,
                member
            )
            .unwrap_err()
            .contains("not a member")
        );
        // An address nobody runs here.
        assert!(
            check_dial(
                "bb22",
                &local,
                "aa11/local:app",
                from,
                Ipv4Addr::new(198, 19, 7, 9),
                member
            )
            .is_err()
        );
        // A tree that started on this computer is told plain on the wire only
        // after it is qualified: the plain id doesn't match a qualified one
        // of another computer.
        assert!(check_dial("aa11", &local, "bb22/local:app", from, to, member).is_err());
        assert!(check_dial("aa11", &local, "local:app", from, to, member).is_ok());
    }

    #[test]
    fn the_owner_of_a_loopback_connection_is_read_from_the_socket_table() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   \
0: 0100007F:B7A4 0100007F:B8FB 01 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0\n   \
1: 0100007F:C001 0100007F:B8FB 01 00000000:00000000 00:00000000 00000000   501        0 22222 1 0\n";
        let server: std::net::SocketAddrV4 = "127.0.0.1:47355".parse().unwrap();
        let a: std::net::SocketAddrV4 = "127.0.0.1:47012".parse().unwrap();
        let b: std::net::SocketAddrV4 = "127.0.0.1:49153".parse().unwrap();
        let none: std::net::SocketAddrV4 = "127.0.0.1:50000".parse().unwrap();
        assert_eq!(proc_net_tcp_owner(table, a, server), Some(1000));
        assert_eq!(proc_net_tcp_owner(table, b, server), Some(501));
        assert_eq!(proc_net_tcp_owner(table, none, server), None);
    }

    #[test]
    fn lsof_output_must_list_the_exact_connection() {
        let c: std::net::SocketAddrV4 = "127.0.0.1:60001".parse().unwrap();
        let s: std::net::SocketAddrV4 = "127.0.0.1:47419".parse().unwrap();
        assert!(lsof_lists(
            "p123\nf9\nn127.0.0.1:60001->127.0.0.1:47419\n",
            c,
            s
        ));
        assert!(!lsof_lists(
            "p123\nn127.0.0.1:60002->127.0.0.1:47419\n",
            c,
            s
        ));
        assert!(!lsof_lists("", c, s));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn only_loopback_ipv4_peers_can_be_the_same_user() {
        let v6: std::net::SocketAddr = "[::1]:60001".parse().unwrap();
        let lan: std::net::SocketAddr = "192.168.1.5:60001".parse().unwrap();
        let s: std::net::SocketAddr = "127.0.0.1:47419".parse().unwrap();
        assert!(!same_user(v6, s).await);
        assert!(!same_user(lan, s).await);
    }

    #[test]
    fn a_tree_whose_top_level_space_runs_on_another_machine_is_linked() {
        use crate::registry::SpaceMeta;
        let meta = |root: Option<&str>, overlay: Option<&str>| SpaceMeta {
            root: root.map(str::to_string),
            overlay_tree: overlay.map(str::to_string),
            ..Default::default()
        };
        let mut all = std::collections::BTreeMap::new();
        // The top-level Space runs on another computer: registered here as a relay Space.
        all.insert("relay:abc123def456".to_string(), meta(None, None));
        assert!(
            linked_trees_of(&all).is_empty(),
            "nothing of that tree here yet"
        );
        // A child made here, and one of its siblings registered here from that computer.
        all.insert(
            "local:worker".to_string(),
            meta(Some("relay:abc123def456"), None),
        );
        all.insert(
            "relay:sibling0001".to_string(),
            meta(Some("relay:abc123def456"), None),
        );
        assert_eq!(
            linked_trees_of(&all),
            BTreeSet::from(["relay:abc123def456".to_string()])
        );
        // The tree a local Space was made for on another computer still links as before.
        all.insert(
            "local:made-for".to_string(),
            meta(None, Some("beef/local:root")),
        );
        assert!(linked_trees_of(&all).contains("beef/local:root"));
        // A member that joined another computer's tree keeps that tree, not its root's id.
        let mut joined = std::collections::BTreeMap::new();
        joined.insert(
            "local:c".to_string(),
            meta(Some("relay:abc123def456"), Some("aaaa/relay:abc123def456")),
        );
        assert_eq!(
            linked_trees_of(&joined),
            BTreeSet::from(["aaaa/relay:abc123def456".to_string()])
        );
        // A tree started here still links once a member runs elsewhere.
        let mut here = std::collections::BTreeMap::new();
        here.insert("local:root".to_string(), meta(None, None));
        assert!(linked_trees_of(&here).is_empty());
        here.insert(
            "relay:child0001".to_string(),
            meta(Some("local:root"), None),
        );
        assert_eq!(
            linked_trees_of(&here),
            BTreeSet::from(["local:root".to_string()])
        );
    }

    #[test]
    fn a_members_space_is_its_local_name() {
        assert_eq!(space_of_key("qemu:db"), "local:db");
        assert_eq!(space_of_key("container:web"), "local:web");
    }
}
