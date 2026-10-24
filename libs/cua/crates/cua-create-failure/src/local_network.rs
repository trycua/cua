//! macOS Local Network access for a macOS Space on this Mac: whether it is
//! missing (the app is not allowed to use the local network) or something
//! else blocks the VMs' network (a VPN or firewall), decided from what a
//! host found. Pure: the platform probes (the app's own connect, the
//! routing table, the daemon's connect to the VM) run in the shells and
//! `cua-vmm`, and pass their results here, so every app and the daemon say
//! the same thing.
//!
//! A macOS VM lives on Apple's shared VM network (`192.168.64.0/24`, the
//! `bridge100` interface). macOS blocks a process from it until the app
//! responsible for that process is allowed under System Settings › Privacy
//! & Security › Local Network; a blocked connect fails at once with "No
//! route to host". A VPN can block it too, while access is on: a route for
//! that network through the VPN's tunnel, or a firewall rule that drops it.

use serde::{Deserialize, Serialize};

use crate::preflight::IssueKind;

/// The telemetry subclass for missing Local Network access.
pub const DENIED: &str = "local_network_denied";
/// The telemetry subclass for a VPN or firewall that blocks the VM network.
pub const VPN: &str = "local_network_vpn";

/// The network macOS VMs get on this Mac (`192.168.64.0/24`).
pub const VMNET: ([u8; 4], u8) = ([192, 168, 64, 0], 24);

/// What one probe of the local network found, from one process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// The connect went out (accepted, refused, or answered): allowed.
    Allowed,
    /// macOS blocked it ("No route to host" at once, or Network
    /// framework's `localNetworkDenied`).
    Blocked,
    /// Nothing to probe (no local network) or no clear answer.
    #[default]
    Unknown,
}

/// Whether `iface` is a VPN's tunnel (`utun3`, `ipsec0`, `ppp0`, `tun0`,
/// `tap0`, `wg0`), not a real network or the VMs' bridge.
pub fn is_tunnel(iface: &str) -> bool {
    let name = iface.trim().to_ascii_lowercase();
    ["utun", "ipsec", "ppp", "tun", "tap", "wg"]
        .iter()
        .any(|p| name.starts_with(p) && name[p.len()..].chars().all(|c| c.is_ascii_digit()))
}

/// The tunnel interface a VPN routes the VM network through, from `netstat
/// -rn -f inet` output, when it does: a route through a tunnel that is at
/// least as specific as the VM network's own (`/24` or longer) and overlaps
/// it. Wider VPN routes (a default route, `0/1` + `128.0/1`, a `/16`) lose
/// to the VM bridge's own `/24` once a VM runs, so they do not count.
pub fn vmnet_tunnel_route(netstat: &str) -> Option<String> {
    let (net, len) = VMNET;
    let vm = u32::from_be_bytes(net);
    netstat.lines().find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Destination Gateway Flags Netif [Expire]
        let (dest, iface) = (cols.first()?, cols.get(3)?);
        let (addr, prefix) = destination(dest)?;
        if prefix < len || !is_tunnel(iface) {
            return None;
        }
        // The route's network overlaps the VM network (it is inside it,
        // being at least as long).
        let mask = |p: u8| {
            if p == 0 {
                0
            } else {
                u32::MAX << (32 - p as u32)
            }
        };
        (addr & mask(len) == vm & mask(len)).then(|| iface.to_string())
    })
}

/// A `netstat -rn` destination as an address and prefix length:
/// `192.168.64` (abbreviated: one `/8` per octet), `192.168.64.7` (a host,
/// `/32`), `192.168.64.128/25`. `default` and anything else: `None`.
fn destination(dest: &str) -> Option<(u32, u8)> {
    let (addr, explicit) = match dest.split_once('/') {
        Some((a, p)) => (a, Some(p.parse::<u8>().ok().filter(|p| *p <= 32)?)),
        None => (dest, None),
    };
    let octets: Vec<u8> = addr
        .split('.')
        .map(|o| o.parse::<u8>().ok())
        .collect::<Option<_>>()?;
    if octets.is_empty() || octets.len() > 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    bytes[..octets.len()].copy_from_slice(&octets);
    let prefix = explicit.unwrap_or(8 * octets.len() as u8);
    Some((u32::from_be_bytes(bytes), prefix))
}

/// The interface `route -n get <address>` names (`interface: bridge100`).
pub fn route_interface(route_get: &str) -> Option<&str> {
    route_get.lines().find_map(|l| {
        l.trim()
            .strip_prefix("interface:")
            .map(str::trim)
            .filter(|i| !i.is_empty())
    })
}

/// Why this Mac cannot reach its own macOS VM, when a connect to it failed
/// with "No route to host":
///
/// - the route to it goes through a VPN's tunnel (`route_iface`), or this
///   process reaches the rest of the local network (`lan` is allowed): a
///   VPN or firewall ([`IssueKind::LocalNetworkVpn`]); nothing to wait for;
/// - this process is blocked from the local network too: Local Network
///   access is off ([`IssueKind::LocalNetwork`]);
/// - neither is known: `None` (the caller treats it as missing access, as
///   before).
pub fn unreachable_cause(route_iface: Option<&str>, lan: Access) -> Option<IssueKind> {
    if route_iface.is_some_and(is_tunnel) || lan == Access::Allowed {
        return Some(IssueKind::LocalNetworkVpn);
    }
    (lan == Access::Blocked).then_some(IssueKind::LocalNetwork)
}

/// What the app should do before creating a macOS Space on this Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    /// Nothing in the way (or nothing to tell): create.
    Proceed,
    /// Access is not allowed yet and the app has not explained it: show
    /// the explanation, then make macOS ask, then check again.
    Explain,
    /// Stop with this issue, at once.
    Fail(IssueKind),
}

/// The step for the app's own probe of the local network (`access`),
/// whether it already explained and asked (`asked`), and the tunnel a VPN
/// routes the VM network through, if any (`vpn_route`,
/// [`vmnet_tunnel_route`]).
pub fn gate(access: Access, asked: bool, vpn_route: Option<&str>) -> Gate {
    match access {
        Access::Blocked if !asked => Gate::Explain,
        Access::Blocked => Gate::Fail(IssueKind::LocalNetwork),
        _ if vpn_route.is_some() => Gate::Fail(IssueKind::LocalNetworkVpn),
        // Unknown: the daemon's own checks decide during the create.
        Access::Allowed | Access::Unknown => Gate::Proceed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTES: &str = "Routing tables

Internet:
Destination        Gateway            Flags               Netif Expire
default            192.168.1.1        UGScg                 en0
0/1                10.8.0.1           UGScg               utun4
127                127.0.0.1          UCS                   lo0
128.0/1            10.8.0.1           UGSc                utun4
192.168               10.8.0.1        UGSc                utun4
192.168.1          link#11            UCS                   en0      !
192.168.64         link#25            UC               bridge100      !
192.168.64.1/32    link#25            UCS              bridge100      !
";

    #[test]
    fn tunnels_are_told_from_real_networks() {
        for t in ["utun0", "utun12", "ipsec0", "ppp0", "tun3", "tap1", "wg0"] {
            assert!(is_tunnel(t), "{t}");
        }
        for n in ["en0", "bridge100", "lo0", "utunx", "awdl0", "anpi0", ""] {
            assert!(!is_tunnel(n), "{n}");
        }
    }

    #[test]
    fn wide_vpn_routes_do_not_block_the_vm_network() {
        // A full-tunnel VPN (0/1, 128.0/1) and a /16 lose to the bridge's /24.
        assert_eq!(vmnet_tunnel_route(ROUTES), None);
        assert_eq!(vmnet_tunnel_route(""), None);
    }

    #[test]
    fn a_vpn_route_as_specific_as_the_vm_network_blocks_it() {
        let exact =
            format!("{ROUTES}192.168.64/24      10.8.0.1           UGSc                utun4\n");
        assert_eq!(vmnet_tunnel_route(&exact).as_deref(), Some("utun4"));
        let half =
            format!("{ROUTES}192.168.64.128/25  10.8.0.1           UGSc                utun7\n");
        assert_eq!(vmnet_tunnel_route(&half).as_deref(), Some("utun7"));
        let host =
            format!("{ROUTES}192.168.64.5       10.8.0.1           UGHS                ipsec0\n");
        assert_eq!(vmnet_tunnel_route(&host).as_deref(), Some("ipsec0"));
        // Another /24 through a tunnel is not the VM network.
        let other =
            format!("{ROUTES}192.168.65         10.8.0.1           UGSc                utun4\n");
        assert_eq!(vmnet_tunnel_route(&other), None);
    }

    #[test]
    fn route_get_names_its_interface() {
        let out = "   route to: 192.168.64.5\ndestination: 192.168.64.0\n       mask: 255.255.255.0\n  interface: bridge100\n      flags: <UP,DONE,CLONING>\n";
        assert_eq!(route_interface(out), Some("bridge100"));
        assert_eq!(
            route_interface("route: writing to routing socket: not in table"),
            None
        );
    }

    #[test]
    fn an_unreachable_vm_is_blamed_on_the_right_thing() {
        assert_eq!(
            unreachable_cause(Some("utun4"), Access::Blocked),
            Some(IssueKind::LocalNetworkVpn)
        );
        assert_eq!(
            unreachable_cause(Some("bridge100"), Access::Allowed),
            Some(IssueKind::LocalNetworkVpn)
        );
        assert_eq!(
            unreachable_cause(Some("bridge100"), Access::Blocked),
            Some(IssueKind::LocalNetwork)
        );
        assert_eq!(
            unreachable_cause(None, Access::Blocked),
            Some(IssueKind::LocalNetwork)
        );
        assert_eq!(unreachable_cause(Some("bridge100"), Access::Unknown), None);
        assert_eq!(unreachable_cause(None, Access::Unknown), None);
    }

    #[test]
    fn the_gate_explains_once_then_fails_fast() {
        assert_eq!(gate(Access::Blocked, false, None), Gate::Explain);
        assert_eq!(
            gate(Access::Blocked, true, None),
            Gate::Fail(IssueKind::LocalNetwork)
        );
        // Missing access is what to fix first, even behind a VPN.
        assert_eq!(gate(Access::Blocked, false, Some("utun4")), Gate::Explain);
        assert_eq!(
            gate(Access::Allowed, false, Some("utun4")),
            Gate::Fail(IssueKind::LocalNetworkVpn)
        );
        assert_eq!(
            gate(Access::Unknown, true, Some("utun4")),
            Gate::Fail(IssueKind::LocalNetworkVpn)
        );
        assert_eq!(gate(Access::Allowed, false, None), Gate::Proceed);
        assert_eq!(gate(Access::Unknown, false, None), Gate::Proceed);
    }
}
