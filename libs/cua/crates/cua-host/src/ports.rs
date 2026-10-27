//! Which TCP and QUIC (UDP) ports the host service listens on.
//!
//! One host service runs per macOS user (per channel). The first one on a
//! machine keeps its channel's ports (3211 TCP and 3212 QUIC for Stable,
//! 3221 and 3222 for Local), as it always has. When another program, most
//! often another user's host service, already holds them, setup moves this
//! user's service to a pair of its own: `3211 + 10k` and `3212 + 10k` for
//! `k` in `2..=78` (3231/3232 up to 3991/3992; `k = 1` is Local's pair),
//! starting at a slot derived from the user id so two users rarely try the
//! same pair, and taking the first free one from there. The pair is kept in
//! the host config, so a later setup keeps it while it stays free.
//!
//! The mesh media port (3212) is not one of them: it is a port of the
//! mesh's own in-process network, not a socket of the operating system, so
//! two users' services never compete for it.

use std::net::{SocketAddr, TcpListener, UdpSocket};

/// A spacesd port pair: `(tcp, quic)`.
pub type PortPair = (u16, u16);

/// A socket kind the service binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// The HTTP/WebSocket listener.
    Tcp,
    /// The QUIC media listener.
    Udp,
}

/// Whether `addr` can be bound for `transport` right now, the way
/// cua-spacesd binds it (a std listener or UDP socket, closed again at
/// once).
pub fn os_port_free(addr: SocketAddr, transport: Transport) -> bool {
    match transport {
        Transport::Tcp => TcpListener::bind(addr).is_ok(),
        Transport::Udp => UdpSocket::bind(addr).is_ok(),
    }
}

/// The lowest and highest per-user slot (`3211 + 10k`).
const FIRST_SLOT: u16 = 2;
const LAST_SLOT: u16 = 78;

/// The per-user pair of `slot`.
fn slot_pair(slot: u16) -> PortPair {
    (3211 + 10 * slot, 3212 + 10 * slot)
}

/// The per-user port range, for messages (`3231-3992`).
pub(crate) fn per_user_range() -> (u16, u16) {
    (slot_pair(FIRST_SLOT).0, slot_pair(LAST_SLOT).1)
}

/// A number that differs between the users of one machine and stays the
/// same for each: the user id on Unix, a hash of the user name elsewhere.
pub(crate) fn per_user_seed() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: getuid has no preconditions and cannot fail.
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        let name = std::env::var("USERNAME").unwrap_or_default();
        name.bytes().fold(2_166_136_261u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(16_777_619)
        })
    }
}

/// The pairs to try, in order: `preferred` (the pair this user's service
/// already has, then the channel's), then every per-user pair, starting at
/// the one `seed` picks. No pair twice.
pub(crate) fn candidates(preferred: &[PortPair], seed: u32) -> Vec<PortPair> {
    let slots = LAST_SLOT - FIRST_SLOT + 1;
    let start = (seed % u32::from(slots)) as u16;
    let mut out: Vec<PortPair> = Vec::new();
    for pair in preferred
        .iter()
        .copied()
        .chain((0..slots).map(|i| slot_pair(FIRST_SLOT + (start + i) % slots)))
    {
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out
}

/// The first of `candidates` that `free` accepts.
pub(crate) fn first_free(
    candidates: &[PortPair],
    mut free: impl FnMut(PortPair) -> bool,
) -> Option<PortPair> {
    candidates.iter().copied().find(|&pair| free(pair))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    /// A port nothing holds right now (the OS hands out a free one, and
    /// the socket is closed again).
    fn free_tcp_port() -> u16 {
        TcpListener::bind((LOOPBACK, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn free_udp_port() -> u16 {
        UdpSocket::bind((LOOPBACK, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn pair_free(pair: PortPair) -> bool {
        os_port_free(SocketAddr::new(LOOPBACK, pair.0), Transport::Tcp)
            && os_port_free(SocketAddr::new(LOOPBACK, pair.1), Transport::Udp)
    }

    /// A pair whose TCP or UDP port another socket holds is skipped; a free
    /// first choice is kept.
    #[test]
    fn a_pair_held_by_another_socket_is_skipped() {
        let held_tcp = TcpListener::bind((LOOPBACK, 0)).unwrap();
        let held_udp = UdpSocket::bind((LOOPBACK, 0)).unwrap();
        let tcp_busy = (held_tcp.local_addr().unwrap().port(), free_udp_port());
        let udp_busy = (free_tcp_port(), held_udp.local_addr().unwrap().port());
        let free = (free_tcp_port(), free_udp_port());

        assert!(!os_port_free(
            SocketAddr::new(LOOPBACK, tcp_busy.0),
            Transport::Tcp
        ));
        assert!(!os_port_free(
            SocketAddr::new(LOOPBACK, udp_busy.1),
            Transport::Udp
        ));
        assert_eq!(
            first_free(&[tcp_busy, udp_busy, free], pair_free),
            Some(free)
        );
        assert_eq!(first_free(&[free, tcp_busy], pair_free), Some(free));
        assert_eq!(first_free(&[tcp_busy, udp_busy], pair_free), None);

        // Once the other socket lets go, its pair is free again.
        drop(held_tcp);
        assert!(os_port_free(
            SocketAddr::new(LOOPBACK, tcp_busy.0),
            Transport::Tcp
        ));
    }

    /// The preferred pairs come first, then each per-user pair once,
    /// starting at the seed's slot and never Stable's or Local's pair.
    #[test]
    fn candidates_start_with_the_preferred_then_the_users_own_slot() {
        let c = candidates(&[(3211, 3212), (3211, 3212)], 0);
        assert_eq!(c[0], (3211, 3212));
        assert_eq!(c[1], (3231, 3232));
        assert_eq!(c.len(), 1 + 77);
        assert!(
            !c[1..].contains(&(3221, 3222)),
            "Local's pair is not a slot"
        );
        assert_eq!(*c.last().unwrap(), (3991, 3992));
        assert_eq!(per_user_range(), (3231, 3992));

        // Another user starts elsewhere, and wraps around.
        let c = candidates(&[(3211, 3212)], 501);
        assert_eq!(c[1], slot_pair(FIRST_SLOT + (501 % 77) as u16));
        assert_eq!(c.len(), 78);
        let mut sorted = c[1..].to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 77, "every slot once");

        // The same user always gets the same order.
        assert_eq!(
            candidates(&[(3211, 3212)], 502),
            candidates(&[(3211, 3212)], 502)
        );
        // A pair the user already has comes first, before the channel's.
        let c = candidates(&[(3301, 3302), (3211, 3212)], 502);
        assert_eq!(&c[..2], &[(3301, 3302), (3211, 3212)]);
        assert_eq!(c.iter().filter(|p| **p == (3301, 3302)).count(), 1);
    }
}
