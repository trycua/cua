// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The records every provider shares: a connection (where Spaces go in an
//! account), a resource (one thing Cua created there, with its tags), what
//! a provision asks for, and the tag rules that make ownership provable.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The record type of a relay machine Cua registered and could not remove
/// (the relay was unreachable): the sweeper removes it.
pub const RELAY_MACHINE: &str = "relay_machine";

/// Tag keys every resource Cua creates carries (labels on Google Cloud,
/// tags on AWS and Modal). The values are provider-safe: lowercase
/// letters, digits and `-` (see [`label_value`]).
pub mod tag {
    /// `true` on everything Cua created.
    pub const MANAGED: &str = "cua-managed";
    /// The owner id of the cua home that created it ([`crate::Store::owner_id`]).
    pub const OWNER: &str = "cua-owner";
    /// The sandbox (and Space) it belongs to, by name; empty for shared
    /// resources.
    pub const SPACE: &str = "cua-space";
    /// The cua version that created it (`0-1-0`).
    pub const CREATED_BY: &str = "cua-created-by";
    /// Unix seconds after which it may be deleted (`0`: never).
    pub const EXPIRES: &str = "cua-expires";
    /// The relay machine the sandbox joined as (`cloud-...`).
    pub const MACHINE: &str = "cua-machine";
    /// On a security group: what it is for (`pool-hosts`; absent: the
    /// outbound-only group of sandboxes).
    pub const GROUP: &str = "cua-group";
    /// What a machine is for: `host` (an elastic pool host that provides
    /// Spaces) or `sandbox` (one sandbox's machine). Absent: `sandbox`.
    pub const ROLE: &str = "cua-role";
}

/// The `cua-role` of an elastic pool host.
pub const ROLE_HOST: &str = "host";
/// The `cua-role` of a one-sandbox machine.
pub const ROLE_SANDBOX: &str = "sandbox";

/// What a cloud supplies to the elastic pool (`provider-contract.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Supplies {
    /// Nothing: the cloud only runs one sandbox per machine (no elastic
    /// pool there).
    #[default]
    Nothing,
    /// Machines the pool controls: each runs cua-spacesd as a host and many
    /// Spaces run on it (AWS, GCP, Hetzner).
    Hosts,
    /// Spaces directly, one platform sandbox per Space; no host the pool
    /// controls (Modal). The solver prices each Space as a one-Space host.
    Spaces,
}

impl Supplies {
    /// The wire word: `hosts`, `spaces` (empty for nothing).
    pub fn as_str(self) -> &'static str {
        match self {
            Supplies::Nothing => "",
            Supplies::Hosts => "hosts",
            Supplies::Spaces => "spaces",
        }
    }
}

/// How a cloud bills a machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Billing {
    /// Billed in steps of this many seconds (Hetzner 3600, AWS 1).
    pub granularity_secs: u64,
    /// Minimum billed per machine (Hetzner 3600, AWS 60).
    pub minimum_secs: u64,
    /// A stopped machine is still billed (Hetzner).
    pub stopped_bills: bool,
}

/// What a cloud supplies, how it bills, where, and its limits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    /// What it supplies.
    pub supplies: Supplies,
    /// How it bills.
    pub billing: Billing,
    /// Regions a host may be added in (the connection's first).
    pub regions: Vec<String>,
    /// Most machines Cua may run there at once.
    pub max_machines: u32,
    /// The cloud deletes a machine on its own when it shuts down (AWS).
    pub self_deletes: bool,
    /// vCPU limits per quota pool, when the cloud says them (`shared` ->
    /// 10). A pool not listed is learned from a refused create.
    #[serde(default)]
    pub vcpu_quotas: std::collections::BTreeMap<String, u32>,
}

/// One machine type a cloud offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostType {
    /// The cloud's type name.
    pub name: String,
    /// `amd64` or `arm64`.
    pub arch: String,
    /// vCPUs.
    pub vcpus: u32,
    /// Memory, MiB.
    pub memory_mb: u64,
    /// Boot disk it comes with, GiB (0: Cua picks).
    pub disk_gb: u32,
    /// `/dev/kvm` on the machine.
    pub kvm: bool,
    /// Accelerator or empty.
    pub gpu: String,
    /// On-demand USD per hour with what every machine of it carries.
    pub usd_per_hour: f64,
    /// Where it is offered now.
    pub regions: Vec<String>,
    /// Typical seconds from create to ready.
    pub boot_secs: u32,
    /// The quota pool its vCPUs count against (`shared`, `dedicated`,
    /// `standard`, `g`); empty: none known.
    #[serde(default)]
    pub quota_pool: String,
}

/// How a cloud runs a Space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// One VM per sandbox: Docker runs the image on it.
    Vm,
    /// One platform sandbox per sandbox (Modal).
    Sandbox,
}

impl Tier {
    /// `vm` or `sandbox`.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Vm => "vm",
            Tier::Sandbox => "sandbox",
        }
    }
}

/// Where Spaces go in one cloud account: names only, never a secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    /// `aws`, `gcp`, `modal`.
    pub provider: String,
    /// AWS or Modal profile.
    #[serde(default)]
    pub profile: String,
    /// Region.
    #[serde(default)]
    pub region: String,
    /// GCP zone.
    #[serde(default)]
    pub zone: String,
    /// GCP project.
    #[serde(default)]
    pub project: String,
    /// Modal environment.
    #[serde(default)]
    pub environment: String,
    /// The account the credentials reached at connect (AWS account id,
    /// GCP project number, Modal workspace).
    #[serde(default)]
    pub account: String,
    /// Hours after which a Space deletes itself (0: never).
    #[serde(default = "default_ttl")]
    pub ttl_hours: u32,
    /// When it was connected (RFC 3339).
    #[serde(default)]
    pub connected_at: String,
}

fn default_ttl() -> u32 {
    DEFAULT_TTL_HOURS
}

/// A Space's lifetime unless the connection says otherwise.
pub const DEFAULT_TTL_HOURS: u32 = 8;

impl Connection {
    /// "AWS · us-west-2", "Google Cloud · my-project", "Modal · main".
    pub fn label(&self, title: &str) -> String {
        let place = [&self.region, &self.project, &self.environment]
            .into_iter()
            .find(|v| !v.is_empty())
            .cloned()
            .unwrap_or_default();
        if place.is_empty() {
            title.to_string()
        } else {
            format!("{title} \u{b7} {place}")
        }
    }
}

/// One thing Cua created in a cloud, as this cua home records it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    /// `aws`, `gcp`, `modal`.
    pub provider: String,
    /// The cloud's id (`i-...`, the instance name, `sb-...`). Empty while
    /// the create is under way ([`Resource::pending`]).
    pub id: String,
    /// `instance`, `security_group`, `firewall`, `sandbox`.
    #[serde(rename = "type")]
    pub resource_type: String,
    /// A name Cua chose before creating it (the instance name, the client
    /// token): how a create that died before recording its id is found.
    #[serde(default)]
    pub name: String,
    /// Region.
    #[serde(default)]
    pub region: String,
    /// Zone (GCP).
    #[serde(default)]
    pub zone: String,
    /// Project (GCP) or environment (Modal).
    #[serde(default)]
    pub project: String,
    /// The sandbox it belongs to (`aws:<name>`), empty for a shared
    /// resource.
    #[serde(default)]
    pub sandbox: String,
    /// The relay machine the sandbox joined as.
    #[serde(default)]
    pub machine: String,
    /// Unix seconds.
    #[serde(default)]
    pub created: u64,
    /// Unix seconds (0: never).
    #[serde(default)]
    pub expires: u64,
    /// The tags it was created with (what the cloud must show before Cua
    /// touches it again).
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    /// The last state seen (`pending`, `running`, `stopped`, `gone`).
    #[serde(default)]
    pub state: String,
    /// Provider details (the security group it uses, the machine type).
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl Resource {
    /// Whether the create is under way (no cloud id recorded yet).
    pub fn pending(&self) -> bool {
        self.id.is_empty()
    }

    /// Whether it is past its expiry.
    pub fn expired(&self, now: u64) -> bool {
        self.expires > 0 && now >= self.expires
    }

    /// The wire row.
    pub fn wire(&self, now: u64) -> cua_sandbox_core::byoc::CloudResource {
        cua_sandbox_core::byoc::CloudResource {
            provider: self.provider.clone(),
            id: if self.id.is_empty() {
                self.name.clone()
            } else {
                self.id.clone()
            },
            resource_type: self.resource_type.clone(),
            sandbox: self.sandbox.clone(),
            machine: self.machine.clone(),
            region: if self.zone.is_empty() {
                self.region.clone()
            } else {
                self.zone.clone()
            },
            state: self.state.clone(),
            created: rfc3339(self.created),
            expires: if self.expires == 0 {
                String::new()
            } else {
                rfc3339(self.expires)
            },
            expired: self.expired(now),
        }
    }
}

/// What one Space asks a provider for.
#[derive(Clone, Debug, Default)]
pub struct ProvisionSpec {
    /// The resource name Cua chose (`cua-space-<hex>`), unique per create.
    pub name: String,
    /// The image family (`linux`, `linux-slim`, `windows`, `omarchy`).
    pub family: String,
    /// The image reference (pinned by digest when it could be resolved).
    pub image: String,
    /// `container` or `vm` (what the Space is).
    pub kind: String,
    /// The machine type Cua picked ([`crate::Provider::kinds`]).
    pub machine_type: String,
    /// Boot disk in GiB.
    pub disk_gb: u32,
    /// VM tier: the cloud-init user data (runs the image with Docker and
    /// joins the relay).
    pub user_data: String,
    /// Sandbox tier: the guest environment (secret: the machine token and
    /// the spacesd token).
    pub env: BTreeMap<String, String>,
    /// Sandbox tier: the command (`None`: the image's).
    pub command: Option<Vec<String>>,
    /// The CPU architecture of the image variant (`amd64`, `arm64`).
    pub arch: String,
    /// The engine asked for (`--runtime`), one of
    /// [`crate::CloudApi::runtimes`]; empty: the cloud's default.
    pub runtime: String,
    /// vCPUs and memory, when the caller asked.
    pub cpus: Option<u32>,
    /// MiB.
    pub memory_mb: Option<u64>,
    /// The tags every resource of this create carries.
    pub tags: BTreeMap<String, String>,
    /// Seconds until it expires (0: never).
    pub ttl_secs: u64,
    /// VM tier: boot this image, named by a public SSM parameter that
    /// holds its id (`/aws/service/...`), instead of the provider's
    /// default image (pool hosts: Ubuntu 24.04 or the AWS Deep Learning
    /// Base GPU image). Empty: the default.
    pub image_parameter: String,
    /// VM tier: a TCP port range the instances of this kind open to each
    /// other only (pool hosts: every port, since a framework's ranks open
    /// ports of their own choosing, as PyTorch's NCCL and Gloo do), and
    /// every UDP port (the mesh's direct path). Such instances get their own
    /// group, whose ingress rules name the group itself; nothing is opened
    /// to any address. `None`: the outbound-only group.
    pub peer_ports: Option<(u16, u16)>,
    /// Expose hardware virtualization extensions when the host type supports it.
    pub nested_virtualization: bool,
    /// [`ROLE_HOST`] (an elastic pool host: exactly `machine_type` in
    /// `region`, user data passed through) or [`ROLE_SANDBOX`] / empty.
    pub role: String,
    /// The region for this machine (empty: the connection's).
    pub region: String,
}

impl ProvisionSpec {
    /// Whether this is an elastic pool host.
    pub fn is_host(&self) -> bool {
        self.role == ROLE_HOST
    }

    /// The region to create it in.
    pub fn region_or<'a>(&'a self, conn: &'a Connection) -> &'a str {
        if self.region.is_empty() {
            &conn.region
        } else {
            &self.region
        }
    }
}

/// Unix seconds now.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Unix seconds as RFC 3339 (UTC, seconds).
pub fn rfc3339(secs: u64) -> String {
    humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(secs)).to_string()
}

/// A tag value every provider accepts: lowercase ASCII letters, digits and
/// `-` (Google Cloud labels are the strictest: `[a-z0-9_-]{0,63}`), so
/// `0.1.0` is `0-1-0`.
pub fn label_value(v: &str) -> String {
    let mut out: String = v
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    out.truncate(63);
    out
}

/// The tags of one create: managed, this home's owner, the sandbox, its
/// relay machine, the cua version and the expiry.
pub fn tags(owner: &str, sandbox: &str, machine: &str, expires: u64) -> BTreeMap<String, String> {
    let mut t = BTreeMap::from([
        (tag::MANAGED.to_string(), "true".to_string()),
        (tag::OWNER.to_string(), label_value(owner)),
        (tag::SPACE.to_string(), label_value(sandbox)),
        (
            tag::CREATED_BY.to_string(),
            label_value(env!("CARGO_PKG_VERSION")),
        ),
        (tag::EXPIRES.to_string(), expires.to_string()),
    ]);
    if !machine.is_empty() {
        t.insert(tag::MACHINE.to_string(), label_value(machine));
    }
    t
}

/// Whether `tags` say Cua created it for `owner`: `cua-managed=true` and
/// `cua-owner=<owner>`. Nothing without both is ever touched.
pub fn is_ours(tags: &BTreeMap<String, String>, owner: &str) -> bool {
    tags.get(tag::MANAGED).map(String::as_str) == Some("true")
        && !owner.is_empty()
        && tags.get(tag::OWNER).map(String::as_str) == Some(label_value(owner).as_str())
}

/// The expiry a resource's tags carry (0: none or unreadable).
pub fn tag_expires(tags: &BTreeMap<String, String>) -> u64 {
    tags.get(tag::EXPIRES)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_needs_both_tags_and_the_same_owner() {
        let t = tags("abc123", "space-1", "cloud-1", 100);
        assert!(is_ours(&t, "abc123"));
        assert!(!is_ours(&t, "someone-else"));
        assert!(!is_ours(&t, ""));
        let mut unmanaged = t.clone();
        unmanaged.remove(tag::MANAGED);
        assert!(!is_ours(&unmanaged, "abc123"));
        let mut other = t.clone();
        other.insert(tag::MANAGED.into(), "false".into());
        assert!(!is_ours(&other, "abc123"));
        assert!(!is_ours(&BTreeMap::new(), "abc123"));
        assert_eq!(tag_expires(&t), 100);
    }

    #[test]
    fn tag_values_fit_every_provider() {
        assert_eq!(label_value("0.1.0"), "0-1-0");
        assert_eq!(label_value("Space:ABC"), "space-abc");
        assert!(label_value(&"x".repeat(100)).len() <= 63);
        let t = tags("o", "space-1", "", 0);
        for (k, v) in &t {
            assert!(
                v.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
                "{k}={v}"
            );
        }
        assert!(!t.contains_key(tag::MACHINE));
    }

    #[test]
    fn labels_name_the_place() {
        let c = Connection {
            region: "us-west-2".into(),
            ..Default::default()
        };
        assert_eq!(c.label("AWS"), "AWS \u{b7} us-west-2");
        let m = Connection {
            environment: "cua-byoc".into(),
            ..Default::default()
        };
        assert_eq!(m.label("Modal"), "Modal \u{b7} cua-byoc");
    }
}
