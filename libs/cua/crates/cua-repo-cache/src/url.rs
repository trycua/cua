//! Which repositories the cache will mirror: public `https` repositories on a
//! public host, named by a normalized URL, and how a command line names
//! them.

use sha2::{Digest, Sha256};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A repository the cache can hold: its normalized URL and the key its
/// mirror is stored under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoUrl {
    /// `https://host/path`: lowercase host, no port, userinfo, query,
    /// fragment, trailing slash or `.git`.
    pub url: String,
    /// 16 hex digits of the SHA-256 of [`RepoUrl::url`].
    pub key: String,
    /// The host.
    pub host: String,
}

/// Why a URL is not one the cache holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported;

/// The most URLs [`urls_in`] takes from one command line.
const MAX_URLS_PER_TEXT: usize = 4;

/// Normalizes `raw`. Only `https` on the default port, with no credentials
/// in it and a host name (not an address), is a repository the cache holds.
pub fn parse(raw: &str) -> Result<RepoUrl, Unsupported> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 512 {
        return Err(Unsupported);
    }
    let u = url::Url::parse(raw).map_err(|_| Unsupported)?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.port().is_some_and(|p| p != 443)
    {
        return Err(Unsupported);
    }
    let host = match u.host() {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        _ => return Err(Unsupported),
    };
    if !host.contains('.') || host == "localhost" || host.ends_with(".local") {
        return Err(Unsupported);
    }
    let mut path = u.path().trim_end_matches('/').to_string();
    if let Some(p) = path.strip_suffix(".git") {
        path = p.trim_end_matches('/').to_string();
    }
    // `https://host/owner/repo` at least; no dot segments or odd bytes.
    if path.matches('/').count() < 2
        || path
            .split('/')
            .any(|s| s == ".." || s == "." || s.contains('\\'))
        || !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-._~%+".contains(&b))
    {
        return Err(Unsupported);
    }
    let url = format!("https://{host}{path}");
    let key = hex::encode(&Sha256::digest(url.as_bytes())[..8]);
    Ok(RepoUrl { url, key, host })
}

/// The repositories a command line clones: every `https://` word of a text
/// that says `git` and `clone` (or `fetch`, `submodule`).
pub fn urls_in(text: &str) -> Vec<RepoUrl> {
    if text.len() > 64 * 1024 || !text.contains("git") {
        return Vec::new();
    }
    if !["clone", "fetch", "pull", "submodule"]
        .iter()
        .any(|w| text.contains(w))
    {
        return Vec::new();
    }
    let mut out: Vec<RepoUrl> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("https://") {
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| c.is_whitespace() || "'\"<>)(;|&`\\".contains(c))
            .unwrap_or(tail.len());
        if let Ok(r) = parse(&tail[..end])
            && !out.iter().any(|o| o.key == r.key)
        {
            out.push(r);
            if out.len() >= MAX_URLS_PER_TEXT {
                break;
            }
        }
        rest = &tail[end.max(1)..];
    }
    out
}

/// Whether `ip` is an address anyone on the internet can reach (not this
/// machine, a private network, link-local, shared or reserved space).
pub fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_is_public(v4),
        IpAddr::V6(v6) => v6_is_public(v6),
    }
}

fn v4_is_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        // Shared address space, benchmarking, reserved and protocol blocks.
        || (o[0] == 100 && (64..128).contains(&o[1]))
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || o[0] == 0
        || o[0] >= 240
        || (o[0] == 192 && o[1] == 0 && o[2] == 0))
}

fn v6_is_public(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return v4_is_public(v4);
    }
    let s = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // Unique local fc00::/7, link-local fe80::/10, documentation 2001:db8::/32.
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        // NAT64 and 6to4 can wrap a private v4 address.
        || (s[0] == 0x0064 && s[1] == 0xff9b)
        || s[0] == 0x2002)
}

/// Whether every address `host` resolves to is public: the daemon never
/// mirrors from a host on this machine or a network behind it (a Space
/// without access to that network would get the repository through the
/// mirror).
pub async fn host_is_public(host: &str) -> bool {
    let host = host.to_string();
    let resolved = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::lookup_host((host, 443)),
    )
    .await;
    let Ok(Ok(addrs)) = resolved else {
        return false;
    };
    let mut any = false;
    for a in addrs {
        any = true;
        if !ip_is_public(a.ip()) {
            return false;
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_normalize_to_one_spelling() {
        let a = parse("https://GitHub.com/vitejs/vite.git").unwrap();
        let b = parse("https://github.com/vitejs/vite/").unwrap();
        let c = parse("https://github.com/vitejs/vite").unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert_eq!(a.url, "https://github.com/vitejs/vite");
        assert_eq!(a.key.len(), 16);
        assert_ne!(
            a.key,
            parse("https://github.com/vitejs/vitest").unwrap().key
        );
    }

    #[test]
    fn only_plain_public_https_repositories_qualify() {
        for bad in [
            "http://github.com/o/r",
            "ssh://git@github.com/o/r",
            "git@github.com:o/r.git",
            "https://user:token@github.com/o/r",
            "https://token@github.com/o/r",
            "https://github.com:8443/o/r",
            "https://github.com/o/r?x=1",
            "https://github.com/o/r#frag",
            "https://127.0.0.1/o/r",
            "https://[::1]/o/r",
            "https://localhost/o/r",
            "https://intranet/o/r",
            "https://github.com/o",
            "https://github.com/o/../r",
            "file:///etc/passwd",
            "",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse("https://gitlab.com/group/sub/project.git").is_ok());
    }

    #[test]
    fn a_command_line_names_the_repositories_it_clones() {
        let found = urls_in(
            "cd /work && git clone --depth 1 https://github.com/vitejs/vite.git app; \
             git clone 'https://gitlab.com/g/p' x",
        );
        assert_eq!(
            found.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
            ["https://github.com/vitejs/vite", "https://gitlab.com/g/p"]
        );
        // A URL with credentials is never taken, and text that does not
        // clone names nothing.
        assert!(urls_in("git clone https://u:p@github.com/o/r").is_empty());
        assert!(urls_in("curl https://github.com/o/r").is_empty());
        assert!(urls_in("echo git https://github.com/o/r").is_empty());
    }

    #[test]
    fn private_and_reserved_addresses_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!ip_is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["140.82.112.3", "8.8.8.8", "2606:50c0:8000::154"] {
            assert!(ip_is_public(ip.parse().unwrap()), "{ip}");
        }
    }
}
