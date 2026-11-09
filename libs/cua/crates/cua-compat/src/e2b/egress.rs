//! Outbound network rules (`network.allow_out`, `network.deny_out`,
//! `update_network`, `allow_internet_access` on a running sandbox).
//!
//! E2B filters a sandbox's outbound traffic by IP address and CIDR block
//! (and by domain, with an inspecting proxy); an allowed destination wins
//! over a denied one. Cua's Linux guests carry `ip` (iproute2) and no
//! firewall userland, so the rules are enforced with the kernel's routing
//! table, which every guest has: each denied block, minus every allowed
//! block and minus the guest's own network (connected subnets and resolver
//! addresses, so name resolution keeps working), becomes a `blackhole`
//! route tagged `proto 77`. The default route stays as it is. Replacing the
//! policy flushes the tagged routes and adds the new ones.
//!
//! Domain names and the proxy features (`rules`, `egress_proxy`,
//! `mask_request_host`, `https_ports`) need a traffic-inspecting proxy that
//! Cua Spaces do not have; the gateway answers those with E2B's 400 and the
//! reason instead of pretending to filter by name.

use std::net::IpAddr;

/// The parsed rules of one sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// Allowed destinations (CIDR blocks).
    pub allow: Vec<String>,
    /// Denied destinations (CIDR blocks).
    pub deny: Vec<String>,
    /// `allow_internet_access=False`: everything not allowed is denied.
    pub no_internet: bool,
}

/// One address or block as a normalised CIDR (`1.1.1.1` is `1.1.1.1/32`),
/// or why it is not one.
pub fn cidr(s: &str) -> Result<String, String> {
    let s = s.trim();
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    let ip: IpAddr = addr.parse().map_err(|_| {
        format!(
            "'{s}' is not an IP address or CIDR block: filtering outbound traffic by domain name needs an inspecting egress proxy, which Cua Spaces do not have"
        )
    })?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let prefix: u32 = match prefix {
        Some(p) => p
            .parse()
            .map_err(|_| format!("'{s}' has an invalid prefix length"))?,
        None => max,
    };
    if prefix > max {
        return Err(format!("'{s}' has an invalid prefix length"));
    }
    Ok(format!("{ip}/{prefix}"))
}

impl Policy {
    pub fn parse(allow: &[String], deny: &[String], no_internet: bool) -> Result<Policy, String> {
        Ok(Policy {
            allow: allow.iter().map(|a| cidr(a)).collect::<Result<_, _>>()?,
            deny: deny.iter().map(|a| cidr(a)).collect::<Result<_, _>>()?,
            no_internet,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty() && !self.no_internet
    }

    /// The blocks to black-hole: every denied block minus every allowed
    /// block and minus `exempt` (the guest's own networks), per family;
    /// plus each of `resolvers` (host addresses) the policy denies, even
    /// inside an exempt network, because E2B's filter blocks name
    /// resolution too unless the resolver is allowed.
    pub fn blackholes(&self, exempt: &[String], resolvers: &[String]) -> Vec<String> {
        let mut deny = self.deny.clone();
        if self.no_internet {
            deny.push("0.0.0.0/0".into());
            deny.push("::/0".into());
        }
        // E2B's `0.0.0.0/0` means all traffic.
        if deny.iter().any(|d| d == "0.0.0.0/0" || d == "::/0") {
            for all in ["0.0.0.0/0", "::/0"] {
                if !deny.iter().any(|d| d == all) {
                    deny.push(all.into());
                }
            }
        }
        let holes: Vec<Block> = self
            .allow
            .iter()
            .chain(exempt.iter())
            .filter_map(|c| Block::parse(c))
            .collect();
        let mut out: Vec<Block> = Vec::new();
        let denied: Vec<Block> = deny.iter().filter_map(|c| Block::parse(c)).collect();
        let allowed: Vec<Block> = self.allow.iter().filter_map(|c| Block::parse(c)).collect();
        for r in resolvers.iter().filter_map(|c| Block::parse(c)) {
            if denied.iter().any(|d| d.contains(&r)) && !allowed.iter().any(|a| a.contains(&r)) {
                out.push(r);
            }
        }
        for d in denied.iter().copied() {
            let mut pieces = vec![d];
            for h in &holes {
                pieces = pieces.into_iter().flat_map(|p| p.minus(h)).collect();
            }
            out.extend(pieces);
        }
        // A /0 route would replace the default route; two /1 halves do not.
        let mut halves = Vec::new();
        for b in out {
            if b.prefix == 0 {
                halves.extend(b.split());
            } else {
                halves.push(b);
            }
        }
        halves.sort_by_key(|b| (b.v6, b.addr, b.prefix));
        halves.dedup();
        halves.iter().map(Block::to_string).collect()
    }
}

/// One CIDR block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Block {
    v6: bool,
    addr: u128,
    prefix: u32,
}

impl Block {
    fn bits(&self) -> u32 {
        if self.v6 { 128 } else { 32 }
    }

    fn parse(c: &str) -> Option<Block> {
        let (a, p) = c.split_once('/')?;
        let ip: IpAddr = a.parse().ok()?;
        let prefix: u32 = p.parse().ok()?;
        let (v6, raw) = match ip {
            IpAddr::V4(v) => (false, u128::from(u32::from(v))),
            IpAddr::V6(v) => (true, u128::from(v)),
        };
        let bits = if v6 { 128 } else { 32 };
        if prefix > bits {
            return None;
        }
        let mask = Self::mask(bits, prefix);
        Some(Block {
            v6,
            addr: raw & mask,
            prefix,
        })
    }

    fn mask(bits: u32, prefix: u32) -> u128 {
        let full: u128 = if bits == 128 {
            u128::MAX
        } else {
            (1u128 << bits) - 1
        };
        if prefix == 0 {
            0
        } else {
            full & !((1u128 << (bits - prefix)) - 1)
        }
    }

    fn contains(&self, o: &Block) -> bool {
        self.v6 == o.v6
            && self.prefix <= o.prefix
            && (o.addr & Self::mask(self.bits(), self.prefix)) == self.addr
    }

    fn split(&self) -> [Block; 2] {
        let p = self.prefix + 1;
        let bit = 1u128 << (self.bits() - p);
        [
            Block { prefix: p, ..*self },
            Block {
                prefix: p,
                addr: self.addr | bit,
                ..*self
            },
        ]
    }

    /// `self` without `hole`: the blocks that remain.
    fn minus(self, hole: &Block) -> Vec<Block> {
        if self.v6 != hole.v6 {
            return vec![self];
        }
        if hole.contains(&self) {
            return vec![];
        }
        if !self.contains(hole) {
            return vec![self];
        }
        // The hole is inside: split until the pieces either are the hole
        // (dropped) or do not touch it.
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(b) = stack.pop() {
            if hole.contains(&b) {
                continue;
            }
            if !b.contains(hole) {
                out.push(b);
                continue;
            }
            stack.extend(b.split());
        }
        out
    }
}

impl std::fmt::Display for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.v6 {
            write!(f, "{}/{}", std::net::Ipv6Addr::from(self.addr), self.prefix)
        } else {
            write!(
                f,
                "{}/{}",
                std::net::Ipv4Addr::from(self.addr as u32),
                self.prefix
            )
        }
    }
}

/// Applies `p` to the sandbox (replacing any earlier policy).
pub async fn apply(
    e: &super::E2b,
    r: &crate::state::Record,
    p: &Policy,
) -> Result<(), crate::backend::BackendError> {
    use crate::backend::BackendError;
    let c = e.spacesd(r).await?;
    let driver = e.drivers.of(&r.id, &c).await;
    let run = |script: String, args: Vec<String>| {
        let (c, driver) = (c.clone(), driver.clone());
        async move {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            crate::guest::run_as(&c, &driver, "root", &script, &a)
                .await
                .map_err(BackendError::Other)
        }
    };
    // Start from no tagged routes; the guest's own networks come next.
    let probe = run(
        "command -v ip >/dev/null 2>&1 || exit 127; \
         ip -4 route flush proto 77 2>/dev/null; ip -6 route flush proto 77 2>/dev/null; \
         ip -4 route show scope link | awk '{print $1}' | sed 's|^\\([0-9.]*\\)$|\\1/32|'; \
         ip -6 route show | grep -v '^default' | awk '{print $1}' | grep -v '^fe80' | sed 's|^\\([0-9a-f:]*\\)$|\\1/128|'; \
         for ns in $(sed -n 's/^nameserver[[:space:]]*//p' /run/systemd/resolve/resolv.conf /etc/resolv.conf 2>/dev/null); do \
           case $ns in 127.*|::1) ;; *:*) echo \"ns $ns/128\";; *) echo \"ns $ns/32\";; esac; done"
            .into(),
        vec![],
    )
    .await?;
    if probe.status.code == Some(127) {
        return Err(BackendError::Unsupported(
            "outbound network rules need the `ip` tool, which this sandbox's image does not have"
                .into(),
        ));
    }
    if p.is_empty() {
        return Ok(());
    }
    let (mut exempt, mut resolvers) = (Vec::new(), Vec::new());
    for l in String::from_utf8_lossy(&probe.stdout)
        .lines()
        .map(str::trim)
    {
        if let Some(r) = l.strip_prefix("ns ") {
            resolvers.push(r.to_string());
        } else if l.contains('/') && Block::parse(l).is_some() {
            exempt.push(l.to_string());
        }
    }
    let mut script = String::new();
    for b in p.blackholes(&exempt, &resolvers) {
        let fam = if b.contains(':') { "-6" } else { "-4" };
        // Metric 1 beats the guest's own routes of the same prefix.
        script.push_str(&format!(
            "ip {fam} route replace blackhole {b} metric 1 proto 77 || exit 1\n"
        ));
    }
    if script.is_empty() {
        return Ok(());
    }
    let out = run(script, vec![]).await?;
    if out.success() {
        Ok(())
    } else {
        Err(BackendError::Other(format!(
            "applying the outbound rules: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(allow: &[&str], deny: &[&str], no_internet: bool) -> Policy {
        Policy::parse(
            &allow.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &deny.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            no_internet,
        )
        .unwrap()
    }

    #[test]
    fn addresses_become_blocks_and_names_are_refused() {
        assert_eq!(cidr("1.1.1.1").unwrap(), "1.1.1.1/32");
        assert_eq!(cidr("10.0.0.0/8").unwrap(), "10.0.0.0/8");
        assert_eq!(cidr("2001:db8::1").unwrap(), "2001:db8::1/128");
        assert!(cidr("example.com").unwrap_err().contains("domain"));
        assert!(cidr("*.example.com").is_err());
        assert!(cidr("1.1.1.1/33").is_err());
    }

    #[test]
    fn deny_all_keeps_the_default_route_and_covers_both_families() {
        let b = p(&[], &["0.0.0.0/0"], false).blackholes(&[], &[]);
        assert_eq!(b, ["0.0.0.0/1", "128.0.0.0/1", "::/1", "8000::/1"]);
    }

    #[test]
    fn allowed_blocks_are_carved_out_of_denied_ones() {
        let b = p(&["1.1.1.1"], &["0.0.0.0/0"], false).blackholes(&[], &[]);
        // Everything except 1.1.1.1/32, as the minimal set of blocks.
        assert!(b.iter().all(|x| x != "0.0.0.0/1"));
        assert!(
            b.contains(&"1.1.1.0/32".to_string()),
            "the neighbour stays denied"
        );
        let covered = |ip: u32| {
            b.iter().filter_map(|x| Block::parse(x)).any(|bl| {
                !bl.v6
                    && bl.contains(&Block {
                        v6: false,
                        addr: ip as u128,
                        prefix: 32,
                    })
            })
        };
        assert!(!covered(u32::from(std::net::Ipv4Addr::new(1, 1, 1, 1))));
        assert!(covered(u32::from(std::net::Ipv4Addr::new(1, 1, 1, 0))));
        assert!(covered(u32::from(std::net::Ipv4Addr::new(1, 1, 1, 2))));
        assert!(covered(u32::from(std::net::Ipv4Addr::new(8, 8, 8, 8))));
        assert!(covered(u32::from(std::net::Ipv4Addr::new(
            255, 255, 255, 255
        ))));
    }

    #[test]
    fn an_allowed_superset_removes_the_deny_and_the_guest_network_is_exempt() {
        assert!(
            p(&["10.0.0.0/8"], &["10.1.0.0/16"], false)
                .blackholes(&[], &[])
                .is_empty()
        );
        let b = p(&[], &[], true).blackholes(&["10.0.2.0/24".into()], &[]);
        let covered = |ip: [u8; 4]| {
            b.iter().filter_map(|x| Block::parse(x)).any(|bl| {
                !bl.v6
                    && bl.contains(&Block {
                        v6: false,
                        addr: u32::from(std::net::Ipv4Addr::from(ip)) as u128,
                        prefix: 32,
                    })
            })
        };
        assert!(!covered([10, 0, 2, 3]));
        assert!(covered([10, 0, 3, 3]));
        assert!(covered([1, 1, 1, 1]));
    }

    #[test]
    fn a_denied_resolver_inside_the_guest_network_is_black_holed_unless_allowed() {
        let exempt = vec!["10.0.2.0/24".to_string()];
        let ns = vec!["10.0.2.3/32".to_string()];
        let b = p(&[], &["0.0.0.0/0"], false).blackholes(&exempt, &ns);
        assert!(b.contains(&"10.0.2.3/32".to_string()));
        let b = p(&["10.0.2.3"], &["0.0.0.0/0"], false).blackholes(&exempt, &ns);
        assert!(!b.contains(&"10.0.2.3/32".to_string()));
        let b = p(&[], &["8.8.8.8"], false).blackholes(&exempt, &ns);
        assert!(!b.contains(&"10.0.2.3/32".to_string()), "not denied");
    }

    #[test]
    fn single_denies_stay_single() {
        assert_eq!(
            p(&[], &["8.8.8.8/32"], false).blackholes(&[], &[]),
            ["8.8.8.8/32"]
        );
        assert!(Policy::default().is_empty());
    }
}
