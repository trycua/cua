// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Hosting Spaces over the direct listener (`cua host setup --direct
//! <ip:port> --provide-spaces`), for a machine reached by its Tailscale or
//! LAN address without the cua.ai relay.
//!
//! The direct listener is plaintext HTTP with the host's env token as a
//! bearer. That token is the owner: it creates and deletes Spaces on the
//! host, and it also reaches the host's own shell and files
//! (`ProcessService`, `FilesystemService`) whether or not the desktop is
//! shared. So by default the host accepts host RPCs, and connections to the
//! Spaces it forwards, only from [`is_private_address`] peers: loopback,
//! Tailscale (100.64.0.0/10, fd7a:115c:a1e0::/48) and RFC 1918 / ULA LAN
//! ranges. `--allow-any-address` turns that off.

use std::net::{IpAddr, SocketAddr};

use crate::{Error, Result};

/// Why a public address needs `--allow-any-address`, for people.
pub const PLAINTEXT_WARNING: &str = "the direct listener is plaintext HTTP: anyone on the path \
     can read the env token, and the token is a shell on this machine and creates Spaces on it";

/// Loopback, Tailscale (100.64.0.0/10 and fd7a:115c:a1e0::/48) or a
/// private LAN address (10/8, 172.16/12, 192.168/16, fc00::/7). An
/// IPv4-mapped IPv6 address counts as its IPv4 address.
pub fn is_private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                // Tailscale's CGNAT range.
                || (a == 100 && (b & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_address(IpAddr::V4(v4));
            }
            let s = v6.segments();
            v6.is_loopback()
                // Tailscale's ULA prefix (inside fc00::/7, named for clarity).
                || (s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0)
                // Unique local addresses.
                || (s[0] & 0xfe00) == 0xfc00
        }
    }
}

/// Checks that hosting Spaces on `listen` does not bind a public address
/// silently: an unspecified bind (every interface) or a private address is
/// fine (host RPCs are then accepted only from private peers); a public one
/// needs `allow_any_address`. Returns the warning to show when it is
/// allowed anyway.
pub fn check_listen(listen: SocketAddr, allow_any_address: bool) -> Result<Option<String>> {
    let ip = listen.ip();
    if allow_any_address {
        return Ok(Some(format!(
            "--allow-any-address: host calls are accepted from any address on {listen}; {PLAINTEXT_WARNING}. \
             Prefer a Tailscale address."
        )));
    }
    if ip.is_unspecified() || is_private_address(ip) {
        return Ok(None);
    }
    Err(Error::InvalidArgument(format!(
        "{ip} is a public address and {PLAINTEXT_WARNING}; bind this machine's Tailscale or LAN \
         address (or 0.0.0.0, which accepts host calls only from loopback, Tailscale and LAN \
         addresses), or pass --allow-any-address to accept the risk"
    )))
}

/// This machine's Tailscale IPv4 address, when the `tailscale` CLI is
/// installed and answers (`tailscale ip -4`).
pub fn tailscale_ip() -> Option<IpAddr> {
    let candidates = [
        "tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    ];
    for bin in candidates {
        let Ok(out) = std::process::Command::new(bin)
            .args(["ip", "-4"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(ip) = text
            .lines()
            .filter_map(|l| l.trim().parse::<IpAddr>().ok())
            .find(|ip| matches!(ip, IpAddr::V4(v4) if v4.octets()[0] == 100))
        {
            return Some(ip);
        }
    }
    None
}

/// `word` quoted for a POSIX shell when it needs it.
pub(crate) fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/@%+=,".contains(c))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `host:port`, bracketing an IPv6 address.
pub(crate) fn authority(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        v4 => format!("{v4}:{port}"),
    }
}

/// The command a laptop runs to add this machine: `cua spaces add
/// <addr> [--host] --name <name> --token <token>` (`--host` when it
/// provides Spaces, so `on="host:<name>"` finds it).
pub fn pairing_command(address: &str, name: &str, token: &str, host: bool) -> String {
    format!(
        "cua spaces add {}{} --name {} --token {}",
        shell_quote(address),
        if host { " --host" } else { "" },
        shell_quote(name),
        shell_quote(token)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn private_addresses_are_loopback_tailscale_and_lan() {
        for a in [
            "127.0.0.1",
            "::1",
            "100.64.0.1",
            "100.101.102.103",
            "100.127.255.254",
            "10.0.0.5",
            "172.16.4.1",
            "172.31.255.255",
            "192.168.1.20",
            "fd7a:115c:a1e0::1",
            "fd12:3456::1",
            "::ffff:192.168.1.20",
            "::ffff:100.100.1.1",
        ] {
            assert!(is_private_address(ip(a)), "{a}");
        }
        for a in [
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "172.32.0.1",
            "192.169.0.1",
            "2001:4860::8888",
            "::ffff:8.8.8.8",
            "fe80::1",
            "0.0.0.0",
        ] {
            assert!(!is_private_address(ip(a)), "{a}");
        }
    }

    #[test]
    fn hosting_never_binds_a_public_address_silently() {
        let any: SocketAddr = "0.0.0.0:3211".parse().unwrap();
        let ts: SocketAddr = "100.101.102.103:3211".parse().unwrap();
        let public: SocketAddr = "203.0.113.7:3211".parse().unwrap();
        assert_eq!(check_listen(any, false).unwrap(), None);
        assert_eq!(check_listen(ts, false).unwrap(), None);
        let e = check_listen(public, false).unwrap_err();
        assert!(
            matches!(e, Error::InvalidArgument(ref m) if m.contains("--allow-any-address")),
            "{e}"
        );
        let warning = check_listen(public, true).unwrap().unwrap();
        assert!(warning.contains("plaintext"), "{warning}");
    }

    #[test]
    fn the_pairing_command_quotes_what_needs_it() {
        assert_eq!(
            pairing_command("100.101.102.103:3211", "Mac mini (spare)", "abc123", true),
            "cua spaces add 100.101.102.103:3211 --host --name 'Mac mini (spare)' --token abc123"
        );
        assert_eq!(
            pairing_command("10.0.0.5:3211", "studio", "t", false),
            "cua spaces add 10.0.0.5:3211 --name studio --token t"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(
            authority(ip("fd7a:115c:a1e0::5"), 3211),
            "[fd7a:115c:a1e0::5]:3211"
        );
    }
}
