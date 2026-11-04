// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The tree proxy: how a container reaches members of its tree that aren't
//! containers on this computer (VMs, and members on other computers).
//!
//! A container has no route to the tree range, and it can't be given one
//! the way a VM can (its network is a bridge in the engine host, and
//! gVisor has a stack of its own), so a plain connect to `198.19.x.y`
//! fails closed. Instead, a container whose tree has such members gets a
//! SOCKS5 proxy on its own loopback, `127.0.0.1:41920`, published the way
//! host ports are ([`crate::host_ports`]): the guest's daemon listens, and
//! this process serves each connection.
//!
//! ```text
//! curl --proxy socks5h://127.0.0.1:41920 http://db.app:5432/
//! ```
//!
//! The proxy takes a tree address (`198.19.0.7`) or a member's name
//! (`db.app`, resolved here, so `socks5h` works) and carries the connection
//! to that member's port, the same way the tree hub carries a VM's
//! connection into a container. It is not a general proxy: it connects
//! only to running members of the container's own tree, never to the
//! internet, the host or the LAN.
//!
//! **Rules first.** Each connection is judged by the container's own network
//! settings, compiled the way a VM's are for its tree (platform denies, the
//! user's lists, the tree's allow, then the defaults), before anything is
//! dialed: a Space set to no network reaches no member, and an explicit deny
//! of a member wins. Then membership: the target must be a running member
//! of the container's tree, here or on another computer (checked again by
//! the computer that runs it), and must take connections from its tree.

use std::net::Ipv4Addr;

use cua_vmm::egress::{Net, Proto};
use cua_vmm::network::{Mode, NetworkContext, NetworkSettings};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::hotspot::ExitStream;

/// The proxy's port on the container's loopback.
pub const PORT: u16 = crate::topology::TREE_PROXY_PORT;

/// SOCKS5 reply codes the proxy uses.
const GENERAL: u8 = 1;
const NOT_ALLOWED: u8 = 2;
const HOST_UNREACHABLE: u8 = 4;
const COMMAND_UNSUPPORTED: u8 = 7;
const ADDRESS_UNSUPPORTED: u8 = 8;

/// What a client asks to connect to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A tree address.
    Addr(Ipv4Addr),
    /// A name (`<space>.<tree>`).
    Name(String),
}

/// Whether a Space with `settings` may connect to tree address `ip` on
/// `port`: its rules as compiled for its tree. An explicit deny (platform,
/// the user's lists, `none`'s default) refuses; nothing decided, or an
/// allow, passes.
pub fn tree_allows(settings: &NetworkSettings, ip: Ipv4Addr, port: u16) -> bool {
    let ctx = NetworkContext {
        tree: cua_vmm::tree::context_tree(),
        ..Default::default()
    };
    // Names in the lists are the gateway's business; tree addresses are
    // addresses, so the address rules decide.
    let Ok((rules, _)) = cua_vmm::network::compile_named(settings, &ctx, Mode::from_env()) else {
        return false;
    };
    !matches!(
        rules.decide(ip.into(), Some(Proto::Tcp), Some(port)),
        Some(r) if !r.accept
    )
}

/// The member a target names, among `members` (name, address): an address
/// that belongs to one, or a name (any case, with or without the root dot).
pub fn resolve_target(target: &Target, members: &[(String, Ipv4Addr)]) -> Option<Ipv4Addr> {
    match target {
        Target::Addr(a) => members.iter().find(|(_, m)| m == a).map(|(_, a)| *a),
        Target::Name(n) => {
            let n = n.trim().trim_end_matches('.').to_ascii_lowercase();
            members.iter().find(|(m, _)| *m == n).map(|(_, a)| *a)
        }
    }
}

/// A SOCKS5 reply.
fn reply(code: u8) -> [u8; 10] {
    [5, code, 0, 1, 0, 0, 0, 0, 0, 0]
}

/// The proxy's end of a connection the container opened, as an in-memory
/// stream (the host ports session serves it for port [`PORT`]).
pub(crate) fn proxy_stream(spaces: crate::Spaces, space: String) -> ExitStream {
    let (client, server) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Err(e) = serve(&spaces, &space, server).await {
            tracing::debug!(space = %space, error = %e, "tree proxy");
        }
    });
    Box::new(client)
}

/// Reads the SOCKS5 greeting and request; returns the target and port.
async fn read_request<S>(s: &mut S) -> std::io::Result<Result<(Target, u16), u8>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello).await?;
    if hello[0] != 5 {
        return Err(bad("not SOCKS5"));
    }
    let mut methods = vec![0u8; usize::from(hello[1])];
    s.read_exact(&mut methods).await?;
    // No authentication: the loopback is the container's own.
    if !methods.contains(&0) {
        s.write_all(&[5, 0xff]).await?;
        return Err(bad("no usable method"));
    }
    s.write_all(&[5, 0]).await?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(bad("not SOCKS5"));
    }
    if head[1] != 1 {
        return Ok(Err(COMMAND_UNSUPPORTED));
    }
    let target = match head[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await?;
            Target::Addr(Ipv4Addr::from(a))
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            s.read_exact(&mut name).await?;
            match String::from_utf8(name) {
                Ok(n) => Target::Name(n),
                Err(_) => return Ok(Err(ADDRESS_UNSUPPORTED)),
            }
        }
        _ => return Ok(Err(ADDRESS_UNSUPPORTED)),
    };
    let mut port = [0u8; 2];
    s.read_exact(&mut port).await?;
    Ok(Ok((target, u16::from_be_bytes(port))))
}

async fn serve<S>(spaces: &crate::Spaces, space: &str, mut s: S) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (target, port) = match read_request(&mut s).await? {
        Ok(t) => t,
        Err(code) => {
            s.write_all(&reply(code)).await?;
            return Ok(());
        }
    };
    match dial(spaces, space, &target, port).await {
        Ok(mut upstream) => {
            s.write_all(&reply(0)).await?;
            let _ = tokio::io::copy_bidirectional(&mut s, &mut upstream).await;
        }
        Err((code, why)) => {
            tracing::debug!(space = %space, ?target, port, %why, "tree proxy refused");
            s.write_all(&reply(code)).await?;
        }
    }
    Ok(())
}

/// Connects Space `space` to `target`:`port`: its rules first, then the
/// member (here, or on another computer).
async fn dial(
    spaces: &crate::Spaces,
    space: &str,
    target: &Target,
    port: u16,
) -> Result<ExitStream, (u8, String)> {
    let members: Vec<(String, Ipv4Addr)> = spaces
        .topology_members(space)
        .into_iter()
        .filter(|m| m.relation != "self")
        .filter_map(|m| Some((m.name, m.address?.parse().ok()?)))
        .collect();
    let to = resolve_target(target, &members).ok_or_else(|| {
        (
            HOST_UNREACHABLE,
            "not a member of this Space's tree".to_string(),
        )
    })?;
    let settings = crate::network::settings_of(spaces.inner.registry.meta(space).as_ref());
    if !tree_allows(&settings, to, port) {
        return Err((
            NOT_ALLOWED,
            "this Space's network rules don't allow it".into(),
        ));
    }
    let name = crate::topology::short_name(space);
    let from = cua_vmm::tree::addresses()
        .get(name)
        .copied()
        .ok_or_else(|| (GENERAL, "this Space has no tree address yet".to_string()))?;
    let my_tree = spaces.book_tree_of(space);
    let me = cua_vmm::tree::overlay_id();
    // A member this computer runs.
    if let Some(m) = cua_vmm::tree::local_members()
        .into_iter()
        .find(|m| m.addr == to && cua_vmm::tree::unqualify(&me, &m.tree) == my_tree)
    {
        if !m.ingress {
            return Err((
                NOT_ALLOWED,
                "that Space takes no connections from its tree".into(),
            ));
        }
        let id = crate::overlay::space_of_key(&m.key);
        let s = spaces
            .space(&id)
            .await
            .map_err(|e| (GENERAL, e.to_string()))?;
        return s
            .open_loopback(port)
            .await
            .map_err(|e| (GENERAL, e.to_string()));
    }
    // A member of another computer.
    if let Some(r) = cua_vmm::tree::remote_members()
        .into_iter()
        .find(|r| r.addr == to && r.tree == my_tree)
    {
        if !r.ingress {
            return Err((
                NOT_ALLOWED,
                "that Space takes no connections from its tree".into(),
            ));
        }
        let s = crate::overlay::connect_remote(spaces, &r.machine, &r.tree, from, to, port)
            .await
            .map_err(|e| (GENERAL, e))?;
        return Ok(Box::new(s));
    }
    Err((HOST_UNREACHABLE, "that member isn't running".into()))
}

/// Whether the tree proxy is worth publishing for a container Space with
/// `settings` whose tree has `running_members` (book keys): one that isn't a
/// container, or `linked` (members on other computers), and its settings
/// reach its tree.
pub fn wanted(settings: &NetworkSettings, running_members: &[String], linked: bool) -> bool {
    cua_vmm::tree::ingress_for(Some(settings), false)
        && (linked || running_members.iter().any(|k| !k.starts_with("container:")))
}

/// The tree range as a [`Net`] (for tests and callers that filter by it).
pub fn range() -> Net {
    Net::parse(cua_vmm::tree::RANGE).expect("a valid range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_vmm::network::NetworkPreset;

    fn members() -> Vec<(String, Ipv4Addr)> {
        vec![
            ("db.app".into(), Ipv4Addr::new(198, 19, 0, 7)),
            ("worker.app".into(), Ipv4Addr::new(198, 19, 7, 1)),
        ]
    }

    #[test]
    fn a_target_names_a_member_by_address_or_by_name() {
        let m = members();
        assert_eq!(
            resolve_target(&Target::Addr(Ipv4Addr::new(198, 19, 0, 7)), &m),
            Some(Ipv4Addr::new(198, 19, 0, 7))
        );
        assert_eq!(
            resolve_target(&Target::Name("DB.app.".into()), &m),
            Some(Ipv4Addr::new(198, 19, 0, 7)),
            "any case, with the root dot"
        );
        assert_eq!(
            resolve_target(&Target::Addr(Ipv4Addr::new(198, 19, 0, 9)), &m),
            None,
            "an address nobody holds"
        );
        assert_eq!(
            resolve_target(&Target::Name("example.com".into()), &m),
            None
        );
        assert_eq!(
            resolve_target(&Target::Addr(Ipv4Addr::new(10, 0, 0, 1)), &m),
            None,
            "never an address outside the tree"
        );
    }

    #[test]
    fn the_spaces_own_rules_judge_a_member_first() {
        let internet = NetworkSettings::internet();
        let member = Ipv4Addr::new(198, 19, 0, 7);
        assert!(tree_allows(&internet, member, 5432));
        // No network: no member either.
        assert!(!tree_allows(&NetworkSettings::none(), member, 5432));
        // An explicit deny of a member wins over the tree's allow.
        let mut custom = NetworkSettings::preset(NetworkPreset::Internet);
        custom.custom.deny = vec!["198.19.0.7".into()];
        assert!(!tree_allows(&custom, member, 5432));
        assert!(tree_allows(&custom, Ipv4Addr::new(198, 19, 7, 1), 5432));
    }

    #[test]
    fn the_proxy_is_for_containers_with_members_a_bridge_cant_reach() {
        let s = NetworkSettings::internet();
        let only_containers = vec!["container:a".to_string(), "container:b".to_string()];
        assert!(!wanted(&s, &only_containers, false));
        assert!(wanted(&s, &only_containers, true), "members elsewhere");
        assert!(wanted(&s, &["qemu:vm".to_string()], false), "a VM here");
        assert!(
            !wanted(&NetworkSettings::none(), &["qemu:vm".to_string()], true),
            "no network, no proxy"
        );
        assert_eq!(PORT, 41_920);
        assert!(range().contains(Ipv4Addr::new(198, 19, 3, 3).into()));
    }

    #[tokio::test]
    async fn socks5_requests_parse_addresses_names_and_refuse_the_rest() {
        async fn ask(bytes: Vec<u8>) -> std::io::Result<Result<(Target, u16), u8>> {
            let (mut client, mut server) = tokio::io::duplex(4096);
            client.write_all(&bytes).await.unwrap();
            read_request(&mut server).await
        }
        // Greeting (one method: none), CONNECT 198.19.0.7:5432.
        let mut req = vec![5, 1, 0, 5, 1, 0, 1, 198, 19, 0, 7];
        req.extend_from_slice(&5432u16.to_be_bytes());
        assert_eq!(
            ask(req).await.unwrap().unwrap(),
            (Target::Addr(Ipv4Addr::new(198, 19, 0, 7)), 5432)
        );
        // By name.
        let mut req = vec![5, 1, 0, 5, 1, 0, 3, 6];
        req.extend_from_slice(b"db.app");
        req.extend_from_slice(&80u16.to_be_bytes());
        assert_eq!(
            ask(req).await.unwrap().unwrap(),
            (Target::Name("db.app".into()), 80)
        );
        // BIND is refused, IPv6 is refused.
        assert_eq!(
            ask(vec![5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 80])
                .await
                .unwrap()
                .unwrap_err(),
            COMMAND_UNSUPPORTED
        );
        assert_eq!(
            ask(vec![5, 1, 0, 5, 1, 0, 4]).await.unwrap().unwrap_err(),
            ADDRESS_UNSUPPORTED
        );
        // Not SOCKS5 at all.
        assert!(ask(vec![4, 1, 0]).await.is_err());
        assert_eq!(reply(0), [5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    }
}
