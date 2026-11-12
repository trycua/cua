// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The share edge (CUA-2602): one small server in the user's own cloud that
//! carries the public TCP shares of Spaces on their own computers, so Cua
//! pays nothing for the bandwidth.
//!
//! It runs cua-relay in edge mode (`CUA_RELAY_TCP_EDGE=on`): the user's cua
//! daemon connects *out* to it with a token only the daemon and the edge
//! hold, exactly as a computer joins Cua's relay; players connect to its
//! share ports; each connection goes down that tunnel to the daemon's share
//! gateway, which reaches the one Space port the share names.
//!
//! [`EdgePlan`] is everything the provider creates, decided before anything
//! exists, so the consent card can show it and tests can check it:
//!
//! - one computer of the smallest type, no SSH key, no inbound rule but the
//!   tunnel port (443) and the share port range, IMDSv2 only, its OS
//!   updating itself (unattended upgrades, reboot when needed);
//! - one stable public address (an Elastic IP on AWS);
//! - every part tagged as Cua's (`cua-managed`, `cua-owner`,
//!   `cua-role=share-edge`, `cua-expires`), so the role can only touch these
//!   and the sweep finds every one of them.
//!
//! TLS for the tunnel: a Caddy front on 443 gets a certificate for the
//! edge's *own* host name from Let's Encrypt (never Cua's `*.m.cua.sh`
//! certificate or key, which never leave Cua's cluster).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::tag;

/// The `cua-role` of a share edge's parts.
pub const ROLE_EDGE: &str = "share-edge";

/// The `cua-role` of a firewall rule opened for one share.
pub const ROLE_SHARE_RULE: &str = "share-rule";

/// The tag naming the share a rule was opened for.
pub const TAG_SHARE: &str = "cua-share";

/// The port the daemon's tunnel reaches the edge on.
pub const TUNNEL_PORT: u16 = 443;

/// The edge's share port pool.
pub const EDGE_PORTS: (u16, u16) = (27000, 27063);

/// How long an edge lives without the daemon renewing it (its
/// `cua-expires` tag): a daemon that is gone for good loses its edge.
pub const EDGE_LEASE_SECS: u64 = 14 * 24 * 3600;

/// An edge nobody shared through for this long is removed.
pub const EDGE_IDLE_SECS: u64 = 3 * 24 * 3600;

/// The cua-relay image the edge runs (a public image).
pub const EDGE_IMAGE: &str = "ghcr.io/trycua/cua-relay";

/// One inbound rule of the edge's firewall.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ingress {
    /// First port.
    pub from: u16,
    /// Last port.
    pub to: u16,
    /// Who may connect.
    pub cidrs: Vec<String>,
    /// Why (shown on the consent card and in the rule's description).
    pub why: String,
}

/// Everything a share edge is, before it exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgePlan {
    /// The cloud word.
    pub provider: String,
    /// Region.
    pub region: String,
    /// The name of every part (`cua-share-edge-<owner>`).
    pub name: String,
    /// Machine type (`t4g.nano`).
    pub machine_type: String,
    /// `arm64` or `x86_64`.
    pub arch: String,
    /// Boot disk, GiB.
    pub disk_gb: u32,
    /// The firewall: these rules and nothing else.
    pub ingress: Vec<Ingress>,
    /// The share port pool.
    pub ports: (u16, u16),
    /// The tags of every part.
    pub tags: BTreeMap<String, String>,
    /// Unix seconds the edge expires unless renewed.
    pub expires: u64,
    /// The cua-relay image and tag.
    pub image: String,
    /// The computer's first-boot script (holds the tunnel token: never logged).
    #[serde(skip)]
    pub user_data: String,
}

/// What a deployed edge is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDeployed {
    /// The cloud word.
    pub provider: String,
    /// Region.
    pub region: String,
    /// The computer's id.
    pub instance: String,
    /// The firewall's id.
    pub group: String,
    /// The stable address's id (an allocation id on AWS).
    pub address_id: String,
    /// The stable public address.
    pub ip: String,
    /// The host name players and the tunnel use (the cloud's own DNS name
    /// for the address, or the address).
    pub host: String,
    /// The share port pool.
    pub ports: (u16, u16),
    /// Unix seconds it expires unless renewed.
    pub expires: u64,
}

impl EdgeDeployed {
    /// The tunnel URL the daemon joins.
    pub fn tunnel_url(&self) -> String {
        format!("wss://{}", self.host)
    }
}

/// The inputs of a plan.
#[derive(Clone, Debug)]
pub struct EdgeInputs<'a> {
    /// The cloud word.
    pub provider: &'a str,
    /// Region.
    pub region: &'a str,
    /// The cua home's owner id (tags).
    pub owner: &'a str,
    /// The tunnel token the edge accepts (40+ hex, made by the daemon).
    pub token: &'a str,
    /// The cua-relay version to run.
    pub version: &'a str,
    /// Unix seconds now.
    pub now: u64,
}

/// The smallest machine type the edge runs on, per cloud.
pub fn machine_type(provider: &str) -> (&'static str, &'static str) {
    match provider {
        "aws" => ("t4g.nano", "arm64"),
        "gcp" => ("e2-micro", "x86_64"),
        "hetzner" => ("cax11", "arm64"),
        _ => ("small", "x86_64"),
    }
}

/// Plans the edge of `inp.owner` in `inp.region`.
pub fn plan(inp: &EdgeInputs<'_>) -> EdgePlan {
    let (machine_type, arch) = machine_type(inp.provider);
    let name = format!("cua-share-edge-{}", crate::model::label_value(inp.owner));
    let expires = inp.now + EDGE_LEASE_SECS;
    let mut tags = BTreeMap::new();
    tags.insert(tag::MANAGED.to_owned(), "true".to_owned());
    tags.insert(tag::OWNER.to_owned(), inp.owner.to_owned());
    tags.insert(tag::ROLE.to_owned(), ROLE_EDGE.to_owned());
    tags.insert(tag::EXPIRES.to_owned(), expires.to_string());
    tags.insert(tag::CREATED_BY.to_owned(), "cua-share-edge".to_owned());
    tags.insert("Name".to_owned(), name.clone());
    let image = format!("{EDGE_IMAGE}:{}", inp.version);
    let ingress = vec![
        Ingress {
            from: TUNNEL_PORT,
            to: TUNNEL_PORT,
            cidrs: vec!["0.0.0.0/0".into()],
            why: "your computer's tunnel to the edge (TLS, token)".into(),
        },
        // Let's Encrypt's HTTP-01 check of the edge's own certificate;
        // Caddy answers only the challenge and redirects to 443.
        Ingress {
            from: 80,
            to: 80,
            cidrs: vec!["0.0.0.0/0".into()],
            why: "the edge's own certificate (Let's Encrypt)".into(),
        },
        Ingress {
            from: EDGE_PORTS.0,
            to: EDGE_PORTS.1,
            cidrs: vec!["0.0.0.0/0".into()],
            why: "public TCP shares (one port each, closed in the edge unless shared)".into(),
        },
    ];
    let user_data = user_data(&image, inp.token, EDGE_PORTS);
    EdgePlan {
        provider: inp.provider.to_owned(),
        region: inp.region.to_owned(),
        name,
        machine_type: machine_type.to_owned(),
        arch: arch.to_owned(),
        disk_gb: 8,
        ingress,
        ports: EDGE_PORTS,
        tags,
        expires,
        image,
        user_data,
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The edge's first-boot script: unattended upgrades, Docker, Caddy in
/// front of cua-relay in edge mode, structured logs to the journal, and no
/// way in but the tunnel (no SSH key, the SSH server disabled).
pub fn user_data(image: &str, token: &str, ports: (u16, u16)) -> String {
    let mut s = String::from("#!/bin/bash\nset -euo pipefail\n");
    s.push_str(
        "# Cua share edge. No SSH: the only way in is the tunnel on 443.\n\
         systemctl disable --now ssh.service ssh.socket 2>/dev/null || true\n\
         export DEBIAN_FRONTEND=noninteractive\n\
         apt-get update -q\n\
         apt-get install -yq unattended-upgrades docker.io caddy\n\
         # The OS updates itself and reboots at night when an update needs it.\n\
         printf 'APT::Periodic::Update-Package-Lists \"1\";\\nAPT::Periodic::Unattended-Upgrade \"1\";\\n' > /etc/apt/apt.conf.d/20auto-upgrades\n\
         printf 'Unattended-Upgrade::Automatic-Reboot \"true\";\\nUnattended-Upgrade::Automatic-Reboot-Time \"04:00\";\\n' > /etc/apt/apt.conf.d/52cua-reboot\n\
         systemctl enable --now unattended-upgrades docker\n\
         # The edge's own host name, for its own certificate.\n\
         TOKEN_IMDS=$(curl -sX PUT http://169.254.169.254/latest/api/token -H 'X-aws-ec2-metadata-token-ttl-seconds: 60' || true)\n\
         HOST=$(curl -s -H \"X-aws-ec2-metadata-token: $TOKEN_IMDS\" http://169.254.169.254/latest/meta-data/public-hostname || hostname -f)\n\
         install -d -m 0700 /etc/cua\n",
    );
    s.push_str(&format!(
        "printf '%s\\n' {token} > /etc/cua/edge-token\n\
         chmod 0600 /etc/cua/edge-token\n\
         cat > /etc/caddy/Caddyfile <<CADDY\n\
         $HOST {{\n\
         \x20 reverse_proxy 127.0.0.1:8080\n\
         \x20 log {{\n\
         \x20   output stderr\n\
         \x20   format json\n\
         \x20 }}\n\
         }}\n\
         CADDY\n\
         systemctl restart caddy\n\
         docker pull {image}\n\
         docker rm -f cua-share-edge >/dev/null 2>&1 || true\n\
         docker run -d --name cua-share-edge --restart unless-stopped --network host \\\n\
         \x20 --read-only --cap-drop ALL --security-opt no-new-privileges \\\n\
         \x20 --log-driver journald \\\n\
         \x20 -e CUA_RELAY_LISTEN=127.0.0.1:8080 \\\n\
         \x20 -e CUA_RELAY_TOKEN_FILE=/run/cua/edge-token \\\n\
         \x20 -e CUA_RELAY_TCP_EDGE=on \\\n\
         \x20 -e CUA_RELAY_TCP_PROXY_PROTOCOL=off \\\n\
         \x20 -e CUA_RELAY_TCP_PORTS={first}-{last} \\\n\
         \x20 -e CUA_RELAY_TCP_HOST=\"$HOST\" \\\n\
         \x20 -e RUST_LOG=info \\\n\
         \x20 -v /etc/cua/edge-token:/run/cua/edge-token:ro \\\n\
         \x20 {image}\n",
        token = sh_quote(token),
        image = sh_quote(image),
        first = ports.0,
        last = ports.1,
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> EdgeInputs<'static> {
        EdgeInputs {
            provider: "aws",
            region: "us-west-2",
            owner: "home-1a2b",
            token: "edge-test-token-not-a-secret",
            version: "1.2.3",
            now: 1_000,
        }
    }

    #[test]
    fn the_plan_is_small_tagged_and_expires() {
        let p = plan(&inputs());
        assert_eq!(
            (p.machine_type.as_str(), p.arch.as_str()),
            ("t4g.nano", "arm64")
        );
        assert_eq!(p.disk_gb, 8);
        assert_eq!(p.tags[tag::MANAGED], "true");
        assert_eq!(p.tags[tag::OWNER], "home-1a2b");
        assert_eq!(p.tags[tag::ROLE], ROLE_EDGE);
        assert_eq!(p.tags[tag::EXPIRES], (1_000 + EDGE_LEASE_SECS).to_string());
        assert_eq!(p.expires, 1_000 + EDGE_LEASE_SECS);
        assert!(p.name.starts_with("cua-share-edge-"));
        assert_eq!(p.image, format!("{EDGE_IMAGE}:1.2.3"));
    }

    #[test]
    fn the_firewall_denies_everything_but_the_tunnel_and_the_share_ports() {
        let p = plan(&inputs());
        let ports: Vec<(u16, u16)> = p.ingress.iter().map(|i| (i.from, i.to)).collect();
        assert_eq!(ports, vec![(443, 443), (80, 80), EDGE_PORTS]);
        // No SSH, no RDP, nothing else.
        for i in &p.ingress {
            assert!(!(i.from..=i.to).contains(&22), "{i:?}");
            assert!(!(i.from..=i.to).contains(&3389), "{i:?}");
        }
        assert_eq!(EDGE_PORTS.1 - EDGE_PORTS.0 + 1, 64);
    }

    #[test]
    fn the_boot_script_updates_itself_has_no_ssh_and_keeps_the_token_out_of_logs() {
        let p = plan(&inputs());
        let u = &p.user_data;
        assert!(u.contains("systemctl disable --now ssh.service"));
        assert!(u.contains("unattended-upgrades"));
        assert!(u.contains("Automatic-Reboot \"true\""));
        assert!(u.contains("CUA_RELAY_TCP_EDGE=on"));
        assert!(
            u.contains("CUA_RELAY_TCP_PROXY_PROTOCOL=off"),
            "client addresses come from the listener only"
        );
        assert!(u.contains("CUA_RELAY_TCP_PORTS=27000-27063"));
        assert!(u.contains("--cap-drop ALL") && u.contains("--read-only"));
        assert!(u.contains("chmod 0600 /etc/cua/edge-token"));
        // The token is written to a file, never echoed into a log line.
        let line = u
            .lines()
            .find(|l| l.contains("edge-test-token-not-a-secret"))
            .unwrap();
        assert!(line.starts_with("printf '%s\\n' "), "{line}");
        assert_eq!(u.matches("edge-test-token-not-a-secret").count(), 1);
        // The plan's serialized form (records, the consent card) never holds it.
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("edge-test-token-not-a-secret"));
        // Never Cua's wildcard certificate.
        assert!(!u.contains("m.cua.sh"));
    }

    #[test]
    fn quoting_keeps_the_script_safe() {
        let u = user_data("img'; rm -rf /", "t'x", (1, 2));
        assert!(u.contains(r"'img'\''; rm -rf /'"));
        assert!(u.contains(r"'t'\''x'"));
    }
}
