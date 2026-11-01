//! Outbound network policy for a Space's guest: block everything, or allow
//! only some IPv4 CIDRs and domains.
//!
//! This is a guest firewall (nftables), applied by running one root script
//! in the guest through cua-spacesd. It limits what code in the Space can
//! reach; it is not a boundary against someone who already has root in the
//! guest. Domains are resolved when the policy is applied and again every
//! 30 seconds by a small guest loop, so the allowed set follows the names'
//! addresses; DNS itself is allowed while a domain list is in force.
//!
//! Linux guests with nftables (installed on demand when the image lacks it
//! and the guest still has network). Other guests refuse with
//! [`Error::FeatureUnsupported`]-style errors from the script's exit code.

use crate::{
    client::SpacesdClient,
    error::{Error, Result},
    process::Command,
};
use std::net::Ipv4Addr;

/// What a guest may reach.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EgressPolicy {
    /// Block all outbound traffic (loopback and replies to inbound
    /// connections stay). Overrides the allow lists.
    pub block_all: bool,
    /// Allowed IPv4 destinations, `a.b.c.d/len`. With any entry here or in
    /// `allow_domains`, everything else is blocked.
    pub allow_cidrs: Vec<String>,
    /// Allowed domains (`example.com`, `*.example.com`).
    pub allow_domains: Vec<String>,
}

impl EgressPolicy {
    /// No restriction (removes a policy).
    pub fn open() -> Self {
        Self::default()
    }

    /// Whether the policy restricts anything.
    pub fn is_open(&self) -> bool {
        !self.block_all && self.allow_cidrs.is_empty() && self.allow_domains.is_empty()
    }

    /// Checks every entry. CIDRs are canonicalized (`1.2.3.4` is `/32`).
    pub fn validate(&self) -> Result<EgressPolicy> {
        let mut out = self.clone();
        out.allow_cidrs = self
            .allow_cidrs
            .iter()
            .map(|c| canonical_cidr(c))
            .collect::<Result<_>>()?;
        for d in &self.allow_domains {
            if !valid_domain(d) {
                return Err(Error::Protocol(format!("invalid domain {d:?}")));
            }
        }
        Ok(out)
    }

    /// The root script that makes the guest enforce this policy.
    pub fn script(&self) -> Result<String> {
        let p = self.validate()?;
        let mut s = String::from(PRELUDE);
        if p.is_open() {
            s.push_str("remove_policy\nexit 0\n");
            return Ok(s);
        }
        s.push_str("ensure_nft\nremove_policy\n");
        let mut rules = String::new();
        rules.push_str("    oif lo accept\n    ct state established,related accept\n");
        if !p.block_all {
            for c in &p.allow_cidrs {
                rules.push_str(&format!("    ip daddr {c} accept\n"));
            }
            if !p.allow_domains.is_empty() {
                rules.push_str("    udp dport 53 accept\n    tcp dport 53 accept\n    ip daddr @cua_allowed accept\n");
            }
        }
        rules.push_str("    counter drop\n");
        s.push_str("nft -f - <<'NFT'\ntable inet cua_egress {\n");
        if !p.block_all && !p.allow_domains.is_empty() {
            s.push_str("  set cua_allowed { type ipv4_addr; flags interval; }\n");
        }
        s.push_str("  chain output {\n    type filter hook output priority 0; policy accept;\n");
        s.push_str(&rules);
        s.push_str("  }\n}\nNFT\n");
        if !p.block_all && !p.allow_domains.is_empty() {
            let domains: Vec<String> = p
                .allow_domains
                .iter()
                .map(|d| d.trim_start_matches("*.").to_string())
                .collect();
            s.push_str(&format!(
                "cat >/run/cua-egress-refresh.sh <<'REF'\n{REFRESH}\nREF\nchmod 700 /run/cua-egress-refresh.sh\nsh /run/cua-egress-refresh.sh once '{}'\n(setsid sh /run/cua-egress-refresh.sh loop '{}' >/dev/null 2>&1 </dev/null & echo $! >/run/cua-egress.pid)\n",
                domains.join(" "),
                domains.join(" ")
            ));
        }
        Ok(s)
    }
}

const PRELUDE: &str = r#"set -e
[ "$(id -u)" = 0 ] || { echo "egress policy needs root" >&2; exit 3; }
[ "$(uname -s)" = Linux ] || { echo "egress policy is supported on Linux guests only" >&2; exit 4; }
ensure_nft() {
  command -v nft >/dev/null 2>&1 && return 0
  if command -v apt-get >/dev/null 2>&1; then DEBIAN_FRONTEND=noninteractive apt-get install -y nftables >/dev/null 2>&1 || true
  elif command -v apk >/dev/null 2>&1; then apk add nftables >/dev/null 2>&1 || true
  elif command -v dnf >/dev/null 2>&1; then dnf install -y nftables >/dev/null 2>&1 || true; fi
  command -v nft >/dev/null 2>&1 || { echo "nftables is not available in this guest" >&2; exit 5; }
}
remove_policy() {
  if [ -f /run/cua-egress.pid ]; then kill "$(cat /run/cua-egress.pid)" 2>/dev/null || true; rm -f /run/cua-egress.pid; fi
  pkill -f 'cua-egress-refresh.sh' 2>/dev/null || true
  command -v nft >/dev/null 2>&1 && nft delete table inet cua_egress 2>/dev/null || true
}
"#;

/// The guest helper that keeps the allowed-domain set current.
const REFRESH: &str = r#"mode="$1"; DOMAINS="$2"
refresh_domains() {
  for d in $DOMAINS; do
    for a in $(getent ahostsv4 "$d" 2>/dev/null | awk '{print $1}' | sort -u); do
      nft add element inet cua_egress cua_allowed "{ $a }" 2>/dev/null || true
    done
  done
}
refresh_domains
[ "$mode" = loop ] || exit 0
while sleep 30; do refresh_domains; done"#;

/// `a.b.c.d` or `a.b.c.d/len` as `a.b.c.d/len`.
pub fn canonical_cidr(c: &str) -> Result<String> {
    let c = c.trim();
    let (ip, bits) = match c.split_once('/') {
        Some((ip, b)) => (ip, b.parse::<u8>().ok()),
        None => (c, Some(32)),
    };
    let ip: Ipv4Addr = ip
        .parse()
        .map_err(|_| Error::Protocol(format!("invalid IPv4 CIDR {c:?}")))?;
    match bits {
        Some(b) if b <= 32 => Ok(format!("{ip}/{b}")),
        _ => Err(Error::Protocol(format!("invalid IPv4 CIDR {c:?}"))),
    }
}

/// A DNS name, optionally `*.`-prefixed: labels of letters, digits and `-`.
pub fn valid_domain(d: &str) -> bool {
    let body = d.strip_prefix("*.").unwrap_or(d);
    !body.is_empty()
        && body.len() <= 253
        && body.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

impl SpacesdClient {
    /// Makes the guest enforce `policy` (an open policy removes any).
    /// Needs a Linux guest; runs as root (directly, or through `sudo`).
    pub async fn set_egress(&self, policy: &EgressPolicy) -> Result<()> {
        let script = policy.script()?;
        let id = self.run(Command::new("id").arg("-u")).await?;
        let root = id.stdout_str().trim() == "0";
        let cmd = if root {
            Command::new("/bin/sh").args(["-c", script.as_str()])
        } else {
            Command::new("sudo").args(["-n", "/bin/sh", "-c", script.as_str()])
        };
        let out = self.run(cmd).await?;
        if out.success() {
            Ok(())
        } else {
            Err(Error::Protocol(format!(
                "egress policy: {}",
                out.stderr_str().trim()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn open_policy_removes() {
        let s = EgressPolicy::open().script().unwrap();
        assert!(s.contains("remove_policy\nexit 0"));
        assert!(!s.contains("nft -f"));
    }

    #[test]
    fn block_all_only_keeps_loopback_and_replies() {
        let s = EgressPolicy {
            block_all: true,
            allow_cidrs: vec!["10.0.0.0/8".into()],
            ..Default::default()
        }
        .script()
        .unwrap();
        assert!(s.contains("oif lo accept") && s.contains("ct state established,related accept"));
        assert!(
            !s.contains("10.0.0.0/8"),
            "block-all ignores the allow list"
        );
        assert!(s.contains("counter drop"));
    }

    #[test]
    fn allow_lists_open_dns_for_domains_only() {
        let cidrs = EgressPolicy {
            allow_cidrs: vec!["1.2.3.4".into()],
            ..Default::default()
        }
        .script()
        .unwrap();
        assert!(cidrs.contains("ip daddr 1.2.3.4/32 accept") && !cidrs.contains("dport 53"));
        let doms = EgressPolicy {
            allow_domains: vec!["*.example.com".into(), "pypi.org".into()],
            ..Default::default()
        }
        .script()
        .unwrap();
        assert!(
            doms.contains("udp dport 53 accept") && doms.contains("once 'example.com pypi.org'")
        );
    }

    #[test]
    fn bad_entries_are_refused() {
        for bad in [
            "1.2.3.4/33",
            "1.2.3",
            "a.b.c.d",
            "1.2.3.4/x",
            "1.2.3.4; reboot",
        ] {
            assert!(canonical_cidr(bad).is_err(), "{bad}");
        }
        for bad in ["a b", "x;rm", "-a.com", "a..com", "a.com/x", ""] {
            assert!(!valid_domain(bad), "{bad}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn scripts_never_carry_unvalidated_text(
            cidrs in proptest::collection::vec("\\PC{0,20}", 0..4),
            doms in proptest::collection::vec("\\PC{0,20}", 0..4),
        ) {
            let p = EgressPolicy { block_all: false, allow_cidrs: cidrs.clone(), allow_domains: doms.clone() };
            if let Ok(s) = p.script() {
                // only validated tokens reach the script
                for c in &cidrs { if c.trim().contains(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '/')) { prop_assert!(!s.contains(c.trim()) || c.trim().is_empty()); } }
                for d in &doms { if !valid_domain(d) { prop_assert!(!s.contains(d.as_str()) || d.is_empty() || PRELUDE.contains(d.as_str())); } }
            }
        }

        #[test]
        fn canonical_cidr_roundtrips(a in any::<[u8;4]>(), b in 0u8..=32) {
            let c = format!("{}.{}.{}.{}/{b}", a[0], a[1], a[2], a[3]);
            prop_assert_eq!(canonical_cidr(&c).unwrap(), c);
        }
    }
}
