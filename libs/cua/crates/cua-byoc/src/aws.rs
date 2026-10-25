// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! AWS EC2 ([`Aws`]): one small VM per sandbox, through the official AWS
//! SDK for Rust. Credentials come from the AWS CLI's own configuration (a
//! profile, SSO, assume-role, the environment); Cua stores only the profile
//! and region names.
//!
//! What a create makes, and nothing else:
//!
//! - one security group per cua home, `cua-sandboxes-<owner>`, in the
//!   account's default VPC: no ingress rule at all, the default egress (the
//!   guest dials out to the relay). Found by its tags and reused; no other
//!   group is ever read for write or changed;
//! - one instance (Canonical Ubuntu 24.04 for the image's architecture)
//!   with its gp3 root volume, tagged in the create call itself; no key
//!   pair, no instance profile, IMDSv2 only with a hop limit of 1 (the
//!   sandbox's containers cannot reach the metadata service), and
//!   `InstanceInitiatedShutdownBehavior=terminate`, so the first boot's
//!   `shutdown -h +<ttl>` and the expiry timer end it for good.
//!
//! No IAM user, role, key or policy is created or used.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use async_trait::async_trait;
use aws_sdk_ec2::error::ProvideErrorMetadata;
use aws_sdk_ec2::types::{
    BlockDeviceMapping, EbsBlockDevice, Filter, HttpTokensState, InstanceMetadataEndpointState,
    InstanceMetadataOptionsRequest, InstanceType, LocationType, ResourceType, ShutdownBehavior,
    Tag, TagSpecification, VolumeType,
};
use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{CloudCredentials, CloudKind};

use crate::api::{CloudApi, Result, Target, Tested, cloud_err};
use crate::model::{
    self, Billing, Capabilities, Connection, HostType, ProvisionSpec, Resource, Supplies, Tier, tag,
};

/// The region when neither the target nor the profile names one.
pub const DEFAULT_REGION: &str = "us-west-2";

/// Canonical's AWS account (the Ubuntu images).
pub const CANONICAL: &str = "099720109477";

/// The `cua-group` tag value of the pool hosts' group.
pub const POOL_GROUP: &str = "pool-hosts";

/// The instance type for Linux sandboxes on arm64 (and its amd64 twin).
pub const LINUX_TYPE: (&str, &str) = ("t4g.medium", "t3.medium");
/// The instance type for the slim Linux image.
pub const SLIM_TYPE: (&str, &str) = ("t4g.small", "t3.small");

/// The public SSM parameter of a pool host's image: Canonical Ubuntu 24.04
/// for CPU types, the AWS Deep Learning Base OSS Nvidia Driver GPU image
/// (Ubuntu 24.04: NVIDIA driver, CUDA, Docker with the NVIDIA container
/// toolkit) for GPU types; `arch` is `amd64` or `arm64`.
pub fn host_image_parameter(gpu: bool, arch: &str) -> String {
    let arm = arch == "arm64";
    if gpu {
        format!(
            "/aws/service/deeplearning/ami/{}/base-oss-nvidia-driver-gpu-ubuntu-24.04/latest/ami-id",
            if arm { "arm64" } else { "x86_64" }
        )
    } else {
        format!(
            "/aws/service/canonical/ubuntu/server/24.04/stable/current/{}/hvm/ebs-gp3/ami-id",
            if arm { "arm64" } else { "amd64" }
        )
    }
}

/// On-demand Linux prices in us-west-2 (US dollars per hour), 2026-09.
fn price(instance_type: &str) -> f64 {
    match instance_type {
        "t4g.small" => 0.0168,
        "t4g.medium" => 0.0336,
        "t3.small" => 0.0208,
        "t3.medium" => 0.0416,
        _ => 0.0,
    }
}

/// Regions elastic hosts may be added in besides the connection's.
pub const HOST_REGIONS: &[&str] = &["us-west-2", "us-east-1", "us-east-2", "eu-west-1"];

/// An elastic host's boot disk (one shared base image, per-Space overlays).
pub const HOST_DISK_GB: u32 = 60;

/// Machine types offered to the elastic pool: name, arch, vCPUs, GiB,
/// on-demand Linux USD per hour in us-west-2 (AWS Price List, 2026-10-21),
/// whether it has `/dev/kvm` (bare metal), and its GPU (empty: none).
pub const HOST_TYPES: &[(&str, &str, u32, u64, f64, bool, &str)] = &[
    ("t3.medium", "amd64", 2, 4, 0.0416, false, ""),
    ("t3.large", "amd64", 2, 8, 0.0832, false, ""),
    ("c7g.large", "arm64", 2, 4, 0.0725, false, ""),
    ("c7g.xlarge", "arm64", 4, 8, 0.145, false, ""),
    ("c7g.2xlarge", "arm64", 8, 16, 0.29, false, ""),
    ("c7g.4xlarge", "arm64", 16, 32, 0.58, false, ""),
    ("m7g.large", "arm64", 2, 8, 0.0816, false, ""),
    ("m7g.xlarge", "arm64", 4, 16, 0.1632, false, ""),
    ("m7g.2xlarge", "arm64", 8, 32, 0.3264, false, ""),
    ("c7i.large", "amd64", 2, 4, 0.08925, false, ""),
    ("c7i.xlarge", "amd64", 4, 8, 0.1785, false, ""),
    ("c7i.2xlarge", "amd64", 8, 16, 0.357, false, ""),
    ("c7i.4xlarge", "amd64", 16, 32, 0.714, false, ""),
    ("m7i.large", "amd64", 2, 8, 0.1008, false, ""),
    ("m7i.xlarge", "amd64", 4, 16, 0.2016, false, ""),
    ("m7i.2xlarge", "amd64", 8, 32, 0.4032, false, ""),
    ("r7i.large", "amd64", 2, 16, 0.1323, false, ""),
    ("c6g.metal", "arm64", 64, 128, 2.176, true, ""),
    ("g4dn.xlarge", "amd64", 4, 16, 0.526, false, "nvidia:T4"),
    ("g5.xlarge", "amd64", 4, 16, 1.006, false, "nvidia:A10G"),
    ("g5.2xlarge", "amd64", 8, 32, 1.212, false, "nvidia:A10G"),
    ("g5.12xlarge", "amd64", 48, 192, 5.672, false, "nvidia:A10G"),
    ("g6.xlarge", "amd64", 4, 16, 0.8048, false, "nvidia:L4"),
    ("g6.2xlarge", "amd64", 8, 32, 0.9776, false, "nvidia:L4"),
    ("g6.4xlarge", "amd64", 16, 64, 1.3232, false, "nvidia:L4"),
    ("g6.12xlarge", "amd64", 48, 192, 4.6016, false, "nvidia:L4"),
    ("g6e.xlarge", "amd64", 4, 32, 1.861, false, "nvidia:L40S"),
    ("g6e.2xlarge", "amd64", 8, 64, 2.24208, false, "nvidia:L40S"),
    (
        "g6e.12xlarge",
        "amd64",
        48,
        384,
        10.49264,
        false,
        "nvidia:L40S",
    ),
    (
        "p4d.24xlarge",
        "amd64",
        96,
        1152,
        21.95764,
        false,
        "nvidia:A100",
    ),
    (
        "p5.48xlarge",
        "amd64",
        192,
        2048,
        55.04,
        false,
        "nvidia:H100",
    ),
];

/// Whether `instance_type` has a GPU: a GPU row of [`HOST_TYPES`], or
/// a type of the g or p families (`g6.xlarge`, `p5.48xlarge`; `gr6`).
pub fn is_gpu_type(instance_type: &str) -> bool {
    if HOST_TYPES
        .iter()
        .any(|t| t.0 == instance_type && !t.6.is_empty())
    {
        return true;
    }
    let family = instance_type.split('.').next().unwrap_or_default();
    let letters: String = family
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    matches!(letters.as_str(), "g" | "p" | "gr") && family.len() > letters.len()
}

/// A gp3 volume's cost per hour ($0.08 per GiB-month) plus the public IPv4
/// address ($0.005 per hour): what runs besides the instance.
fn extras_per_hour(disk_gb: u32) -> f64 {
    f64::from(disk_gb) * 0.08 / 730.0 + 0.005
}

enum Mode {
    /// The AWS CLI's configuration (profiles, SSO, the environment).
    Cli,
    /// A fixed endpoint and credentials (tests, LocalStack-style mirrors).
    Endpoint {
        url: String,
        key: String,
        secret: String,
    },
}

/// EC2 as a [`CloudApi`].
pub struct Aws {
    mode: Mode,
    clients: Mutex<HashMap<(String, String), Clients>>,
}

#[derive(Clone)]
struct Clients {
    ec2: aws_sdk_ec2::Client,
    sts: aws_sdk_sts::Client,
}

impl Default for Aws {
    fn default() -> Self {
        Self::new()
    }
}

fn some(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn home() -> Option<std::path::PathBuf> {
    env("HOME").map(std::path::PathBuf::from)
}

fn config_file() -> Option<std::path::PathBuf> {
    env("AWS_CONFIG_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| home().map(|h| h.join(".aws/config")))
}

fn credentials_file() -> Option<std::path::PathBuf> {
    env("AWS_SHARED_CREDENTIALS_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| home().map(|h| h.join(".aws/credentials")))
}

/// The `region` of `profile` in the AWS config file (a plain INI read of
/// names only; no secret is read).
fn profile_region(profile: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_file()?).ok()?;
    profile_keys(&text, profile)
        .remove("region")
        .filter(|v| !v.is_empty())
}

/// The keys of `profile`'s section in AWS config `text` (names and values
/// of the config file; the credentials file is never read).
fn profile_keys(text: &str, profile: &str) -> BTreeMap<String, String> {
    let want = if profile == "default" || profile.is_empty() {
        "default".to_string()
    } else {
        format!("profile {profile}")
    };
    let mut out = BTreeMap::new();
    let mut inside = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            inside = l[1..l.len() - 1].trim() == want;
            continue;
        }
        if inside && let Some((k, v)) = l.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// How `profile` signs in, from AWS config `text` (and the error, when the
/// config does not say): the command that signs it in again.
pub fn sign_in_command(text: &str, profile: &str, error: &str) -> String {
    let keys = profile_keys(text, profile);
    let flag = if profile.is_empty() || profile == "default" {
        String::new()
    } else {
        format!(" --profile {profile}")
    };
    let lower = error.to_ascii_lowercase();
    if keys.contains_key("sso_session") || keys.contains_key("sso_start_url") {
        format!("aws sso login{flag}")
    } else if keys.contains_key("login_session") || lower.contains("login token") {
        format!("aws login{flag}")
    } else if lower.contains("sso") {
        format!("aws sso login{flag}")
    } else if keys.contains_key("aws_access_key_id") || lower.contains("invalidclienttokenid") {
        format!("aws configure{flag}")
    } else {
        format!("aws login{flag}")
    }
}

/// An AWS error as a sign-in problem, when it is one.
pub fn sign_in_of(profile: &str, config: &str, error: &str) -> Option<crate::signin::SignIn> {
    use crate::signin::{SignIn, mentions};
    let fix = || sign_in_command(config, profile, error);
    // A clock off by minutes reads "RequestExpired"/"Signature expired":
    // not a sign-in problem.
    if mentions(error, &["requestexpired", "signature expired"]) {
        return None;
    }
    if mentions(
        error,
        &[
            "invalidclienttokenid",
            "unrecognizedclientexception",
            "signaturedoesnotmatch",
            "authfailure",
            "the security token included in the request is invalid",
        ],
    ) {
        let whose = if profile.is_empty() || profile == "default" {
            "the default profile's keys".to_string()
        } else {
            format!("the keys of profile {profile}")
        };
        return Some(SignIn::rejected("AWS", &whose, &fix()));
    }
    if mentions(
        error,
        &[
            "expiredtoken",
            "token has expired",
            "session has expired",
            "refresh token has expired",
            "sso session has expired",
            "token is expired",
            "credentials have expired",
            "expired token",
        ],
    ) {
        return Some(SignIn::expired("AWS", &fix()));
    }
    if mentions(
        error,
        &[
            "no credentials",
            "credentialsnotloaded",
            "could not load credentials",
            "no providers in chain provided credentials",
            "an error occurred while loading credentials",
            "failed to load credentials",
            "could not find profile",
            "profile file",
            "the credential provider was not enabled",
            "unable to locate credentials",
        ],
    ) {
        return Some(SignIn::missing("AWS", &fix()));
    }
    None
}

/// Reads the `Date` header of the response it watches (the cloud's clock).
#[derive(Debug, Clone, Default)]
struct ClockTap(std::sync::Arc<Mutex<Option<u64>>>);

impl aws_sdk_sts::config::Intercept for ClockTap {
    fn name(&self) -> &'static str {
        "cua-clock-tap"
    }

    fn read_after_transmit(
        &self,
        context: &aws_sdk_sts::config::interceptors::BeforeDeserializationInterceptorContextRef<'_>,
        _rc: &aws_sdk_sts::config::RuntimeComponents,
        _cfg: &mut aws_sdk_sts::config::ConfigBag,
    ) -> std::result::Result<(), aws_sdk_sts::error::BoxError> {
        if let Some(t) = context
            .response()
            .headers()
            .get("date")
            .and_then(model::parse_http_date)
        {
            *self.0.lock().expect("clock tap") = Some(t);
        }
        Ok(())
    }
}

fn tags_of(tags: &[Tag]) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|t| {
            Some((
                t.key()?.to_string(),
                t.value().unwrap_or_default().to_string(),
            ))
        })
        .collect()
}

/// The pool group's ingress: members of group `id` reach each other on
/// TCP `ports` and on every UDP port (the mesh's direct path), nothing
/// else. Rules already there are left as they are.
async fn open_to_group(ec2: &aws_sdk_ec2::Client, id: &str, ports: (u16, u16)) -> Result<()> {
    let from_group = || {
        aws_sdk_ec2::types::UserIdGroupPair::builder()
            .group_id(id)
            .build()
    };
    let rules = [
        aws_sdk_ec2::types::IpPermission::builder()
            .ip_protocol("tcp")
            .from_port(i32::from(ports.0))
            .to_port(i32::from(ports.1))
            .user_id_group_pairs(from_group())
            .build(),
        aws_sdk_ec2::types::IpPermission::builder()
            .ip_protocol("udp")
            .from_port(0)
            .to_port(65535)
            .user_id_group_pairs(from_group())
            .build(),
    ];
    for rule in rules {
        match ec2
            .authorize_security_group_ingress()
            .group_id(id)
            .ip_permissions(rule)
            .send()
            .await
        {
            Ok(_) => {}
            Err(e)
                if e.as_service_error()
                    .and_then(aws_sdk_ec2::error::ProvideErrorMetadata::code)
                    == Some("InvalidPermission.Duplicate") => {}
            Err(e) => return Err(aws_err("open the pool hosts to each other", e)),
        }
    }
    Ok(())
}

fn filter(name: &str, values: &[&str]) -> Filter {
    Filter::builder()
        .name(name)
        .set_values(Some(values.iter().map(|v| v.to_string()).collect()))
        .build()
}

fn owner_filters(owner: &str) -> Vec<Filter> {
    vec![
        filter(&format!("tag:{}", tag::MANAGED), &["true"]),
        filter(
            &format!("tag:{}", tag::OWNER),
            &[&model::label_value(owner)],
        ),
    ]
}

/// The instance type for `family` on `arch`.
pub fn instance_type(family: &str, arch: &str) -> &'static str {
    let (arm, x86) = if family == "linux-slim" {
        SLIM_TYPE
    } else {
        LINUX_TYPE
    };
    if arch == "amd64" { x86 } else { arm }
}

/// The `amd64` twin of an arm64 type Cua picked (the image may only have
/// an amd64 build).
fn for_arch(machine_type: &str, arch: &str) -> String {
    if arch != "amd64" {
        return machine_type.to_string();
    }
    match machine_type {
        "t4g.small" => "t3.small".into(),
        "t4g.medium" => "t3.medium".into(),
        other => other.to_string(),
    }
}

fn ec2_arch(arch: &str) -> (&'static str, &'static str) {
    // (the Ubuntu image name's arch, EC2's architecture)
    if arch == "amd64" {
        ("amd64", "x86_64")
    } else {
        ("arm64", "arm64")
    }
}

fn state_name(i: &aws_sdk_ec2::types::Instance) -> String {
    i.state()
        .and_then(|s| s.name())
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| "unknown".into())
}

impl Aws {
    /// Through the AWS CLI's configuration.
    pub fn new() -> Self {
        Aws {
            mode: Mode::Cli,
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// Against `url` with these static credentials (tests and mirrors that
    /// speak the EC2 and STS Query APIs); never reads the AWS CLI config.
    pub fn with_endpoint(url: impl Into<String>, key: &str, secret: &str) -> Self {
        Aws {
            mode: Mode::Endpoint {
                url: url.into(),
                key: key.into(),
                secret: secret.into(),
            },
            clients: Mutex::new(HashMap::new()),
        }
    }

    async fn clients(&self, conn: &Connection) -> Clients {
        let key = (conn.profile.clone(), conn.region.clone());
        if let Some(c) = self.clients.lock().expect("aws clients").get(&key) {
            return c.clone();
        }
        let region = aws_sdk_ec2::config::Region::new(conn.region.clone());
        let clients = match &self.mode {
            Mode::Cli => {
                let mut loader =
                    aws_config::defaults(aws_config::BehaviorVersion::latest()).region(region);
                if !conn.profile.is_empty() {
                    loader = loader.profile_name(&conn.profile);
                }
                let sdk = loader.load().await;
                Clients {
                    ec2: aws_sdk_ec2::Client::new(&sdk),
                    sts: aws_sdk_sts::Client::new(&sdk),
                }
            }
            Mode::Endpoint { url, key, secret } => {
                let creds = aws_sdk_ec2::config::Credentials::new(
                    key.clone(),
                    secret.clone(),
                    None,
                    None,
                    "cua-byoc-endpoint",
                );
                let ec2 = aws_sdk_ec2::Config::builder()
                    .behavior_version_latest()
                    .region(region.clone())
                    .credentials_provider(creds.clone())
                    .endpoint_url(url.clone())
                    .build();
                let sts = aws_sdk_sts::Config::builder()
                    .behavior_version_latest()
                    .region(aws_sdk_sts::config::Region::new(conn.region.clone()))
                    .credentials_provider(creds)
                    .endpoint_url(url.clone())
                    .build();
                Clients {
                    ec2: aws_sdk_ec2::Client::from_conf(ec2),
                    sts: aws_sdk_sts::Client::from_conf(sts),
                }
            }
        };
        self.clients
            .lock()
            .expect("aws clients")
            .insert(key, clients.clone());
        clients
    }

    /// The newest Canonical Ubuntu 24.04 image for `arch`, and its root
    /// device name.
    async fn ubuntu(&self, ec2: &aws_sdk_ec2::Client, arch: &str) -> Result<(String, String)> {
        let (name_arch, ec2_arch) = ec2_arch(arch);
        let out = ec2
            .describe_images()
            .owners(CANONICAL)
            .filters(filter(
                "name",
                &[&format!(
                    "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-{name_arch}-server-*"
                )],
            ))
            .filters(filter("architecture", &[ec2_arch]))
            .filters(filter("state", &["available"]))
            .send()
            .await
            .map_err(|e| aws_err("find the Ubuntu 24.04 image", e))?;
        let best = out
            .images()
            .iter()
            .filter(|i| i.image_id().is_some())
            .max_by(|a, b| a.creation_date().cmp(&b.creation_date()))
            .ok_or_else(|| {
                Error::Cloud(format!(
                    "aws: no Canonical Ubuntu 24.04 {name_arch} image in this region"
                ))
            })?;
        Ok((
            best.image_id().unwrap_or_default().to_string(),
            best.root_device_name().unwrap_or("/dev/sda1").to_string(),
        ))
    }

    async fn default_vpc(&self, ec2: &aws_sdk_ec2::Client) -> Result<String> {
        let out = ec2
            .describe_vpcs()
            .filters(filter("is-default", &["true"]))
            .send()
            .await
            .map_err(|e| aws_err("find the default VPC", e))?;
        out.vpcs()
            .first()
            .and_then(|v| v.vpc_id())
            .map(str::to_string)
            .ok_or_else(|| {
                Error::Cloud(
                    "aws: this region has no default VPC; Cua creates sandboxes in it (create \
                     one with `aws ec2 create-default-vpc`, or connect another region)"
                        .into(),
                )
            })
    }

    /// The default subnets (one per zone) of the default VPC, as (zone,
    /// subnet id) in zone order.
    async fn default_subnets(&self, ec2: &aws_sdk_ec2::Client) -> Result<Vec<(String, String)>> {
        let vpc = self.default_vpc(ec2).await?;
        let out = ec2
            .describe_subnets()
            .filters(filter("vpc-id", &[&vpc]))
            .filters(filter("default-for-az", &["true"]))
            .send()
            .await
            .map_err(|e| aws_err("list the default subnets", e))?;
        let mut v: Vec<(String, String)> = out
            .subnets()
            .iter()
            .filter_map(|s| {
                Some((
                    s.availability_zone()?.to_string(),
                    s.subnet_id()?.to_string(),
                ))
            })
            .collect();
        v.sort();
        Ok(v)
    }

    /// This home's security group in `vpc` for `peer_ports` (the
    /// outbound-only one, or the pool hosts' one), if it exists.
    async fn find_group(
        &self,
        ec2: &aws_sdk_ec2::Client,
        vpc: &str,
        owner: &str,
        pool: bool,
    ) -> Result<Option<aws_sdk_ec2::types::SecurityGroup>> {
        let mut req = ec2
            .describe_security_groups()
            .filters(filter("vpc-id", &[vpc]));
        for f in owner_filters(owner) {
            req = req.filters(f);
        }
        let out = req
            .send()
            .await
            .map_err(|e| aws_err("find the Cua security group", e))?;
        Ok(out
            .security_groups()
            .iter()
            .find(|g| {
                let t = tags_of(g.tags());
                model::is_ours(&t, owner)
                    && (t.get(tag::GROUP).map(String::as_str) == Some(POOL_GROUP)) == pool
            })
            .cloned())
    }

    /// Finds or creates this home's group: outbound only, or (with
    /// `peer_ports`) for pool hosts, which may reach each other on those
    /// TCP ports (and UDP, the mesh's direct path between them) and
    /// nothing else: no address outside the group gets in. An existing pool
    /// group gets the rules it lacks (an older one opened fewer ports). Returns its id and, when it was created
    /// now, its record.
    async fn ensure_group(
        &self,
        ec2: &aws_sdk_ec2::Client,
        conn: &Connection,
        owner: &str,
        peer_ports: Option<(u16, u16)>,
    ) -> Result<(String, Option<Resource>)> {
        let vpc = self.default_vpc(ec2).await?;
        let pool = peer_ports.is_some();
        if let Some(g) = self.find_group(ec2, &vpc, owner, pool).await? {
            let id = g.group_id().unwrap_or_default().to_string();
            if let Some(ports) = peer_ports {
                open_to_group(ec2, &id, ports).await?;
            }
            return Ok((id, None));
        }
        let name = if pool {
            format!("cua-pool-hosts-{}", model::label_value(owner))
        } else {
            format!("cua-sandboxes-{}", model::label_value(owner))
        };
        let mut tags = model::tags(owner, "", "", 0);
        if pool {
            tags.insert(tag::GROUP.into(), POOL_GROUP.into());
        }
        let mut spec = TagSpecification::builder().resource_type(ResourceType::SecurityGroup);
        for (k, v) in tags.iter().chain([(&"Name".to_string(), &name)]) {
            spec = spec.tags(Tag::builder().key(k).value(v).build());
        }
        let out = ec2
            .create_security_group()
            .group_name(&name)
            .description(if pool {
                "Cua pool hosts: no inbound from anywhere but each other (gang ports); they dial out to the cua.ai relay"
            } else {
                "Cua sandboxes: no inbound; the guest dials out to the cua.ai relay"
            })
            .vpc_id(&vpc)
            .tag_specifications(spec.build())
            .send()
            .await
            .map_err(|e| aws_err("create the Cua security group", e))?;
        let id = out.group_id().unwrap_or_default().to_string();
        let record = Resource {
            provider: "aws".into(),
            id: id.clone(),
            resource_type: "security_group".into(),
            name,
            region: conn.region.clone(),
            created: model::now(),
            tags,
            state: "available".into(),
            ..Default::default()
        };
        if let Some(ports) = peer_ports
            && let Err(e) = open_to_group(ec2, &id, ports).await
        {
            let _ = ec2.delete_security_group().group_id(&id).send().await;
            return Err(e);
        }
        Ok((id, Some(record)))
    }

    /// The RunInstances call of `spec` (without sending it), for a create
    /// and for the test's dry run.
    #[allow(clippy::too_many_arguments)]
    fn run_request(
        &self,
        ec2: &aws_sdk_ec2::Client,
        spec: &ProvisionSpec,
        image: &str,
        root: &str,
        group: Option<&str>,
        instance_type: &str,
    ) -> aws_sdk_ec2::operation::run_instances::builders::RunInstancesFluentBuilder {
        let mut tags: Vec<Tag> = spec
            .tags
            .iter()
            .map(|(k, v)| Tag::builder().key(k).value(v).build())
            .collect();
        tags.push(Tag::builder().key("Name").value(&spec.name).build());
        let spec_for = |t: ResourceType| {
            TagSpecification::builder()
                .resource_type(t)
                .set_tags(Some(tags.clone()))
                .build()
        };
        use base64::Engine as _;
        let mut req = ec2
            .run_instances()
            .image_id(image)
            .instance_type(InstanceType::from(instance_type))
            .min_count(1)
            .max_count(1)
            .instance_initiated_shutdown_behavior(ShutdownBehavior::Terminate)
            .metadata_options(
                InstanceMetadataOptionsRequest::builder()
                    .http_tokens(HttpTokensState::Required)
                    .http_put_response_hop_limit(1)
                    .http_endpoint(InstanceMetadataEndpointState::Enabled)
                    .build(),
            )
            .block_device_mappings(
                BlockDeviceMapping::builder()
                    .device_name(root)
                    .ebs(
                        EbsBlockDevice::builder()
                            .volume_size(spec.disk_gb.max(8) as i32)
                            .volume_type(VolumeType::Gp3)
                            .delete_on_termination(true)
                            .build(),
                    )
                    .build(),
            )
            .tag_specifications(spec_for(ResourceType::Instance))
            .tag_specifications(spec_for(ResourceType::Volume))
            .user_data(base64::engine::general_purpose::STANDARD.encode(&spec.user_data));
        if let Some(g) = group {
            req = req.security_group_ids(g);
        }
        req
    }

    async fn find_instance(
        &self,
        ec2: &aws_sdk_ec2::Client,
        r: &Resource,
    ) -> Result<Option<aws_sdk_ec2::types::Instance>> {
        let req = if r.id.is_empty() {
            if r.name.is_empty() {
                return Ok(None);
            }
            ec2.describe_instances()
                .filters(filter("tag:Name", &[&r.name]))
        } else {
            ec2.describe_instances().instance_ids(&r.id)
        };
        match req.send().await {
            Ok(out) => Ok(out
                .reservations()
                .iter()
                .flat_map(|x| x.instances())
                .next()
                .cloned()),
            Err(e) if e.code() == Some("InvalidInstanceID.NotFound") => Ok(None),
            Err(e) => Err(aws_err("describe the instance", e)),
        }
    }

    /// The tags the cloud shows for `r` now, refusing unless they are
    /// `owner`'s (`None`: gone).
    async fn guard(
        &self,
        conn: &Connection,
        r: &Resource,
        owner: &str,
        verb: &str,
    ) -> Result<Option<String>> {
        match self.describe(conn, r).await? {
            None => Ok(None),
            Some((state, tags)) => {
                if !model::is_ours(&tags, owner) {
                    return Err(Error::Cloud(format!(
                        "aws: refusing to {verb} {} {}: it is not tagged as this Cua home's \
                         (cua-managed=true, cua-owner={})",
                        r.resource_type,
                        r.id,
                        model::label_value(owner)
                    )));
                }
                Ok(Some(state))
            }
        }
    }
}

/// Whether a RunInstances error is about the zone it landed in (no
/// capacity there, or the type is not offered there), so another zone may
/// work.
fn zone_bound(code: Option<&str>) -> bool {
    matches!(code, Some("InsufficientInstanceCapacity" | "Unsupported"))
}

/// An EC2 error as `Code: message`.
fn err_detail<E, R>(e: &aws_sdk_ec2::error::SdkError<E, R>) -> String
where
    aws_sdk_ec2::error::SdkError<E, R>: ProvideErrorMetadata,
{
    match (e.code(), e.message()) {
        (Some(c), Some(m)) if m != c => format!("{c}: {m}"),
        (Some(c), _) => c.to_string(),
        _ => "no answer from EC2".to_string(),
    }
}

fn aws_err<E, R>(what: &str, e: aws_sdk_ec2::error::SdkError<E, R>) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug,
    aws_sdk_ec2::error::SdkError<E, R>: ProvideErrorMetadata,
{
    let detail = match (e.code(), e.message()) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        (Some(c), None) => c.to_string(),
        _ => aws_sdk_ec2::error::DisplayErrorContext(&e).to_string(),
    };
    cloud_err("aws", what, detail)
}

fn sts_err<E, R>(what: &str, e: aws_sdk_sts::error::SdkError<E, R>) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug,
    aws_sdk_sts::error::SdkError<E, R>: aws_sdk_sts::error::ProvideErrorMetadata,
{
    use aws_sdk_sts::error::ProvideErrorMetadata as _;
    let detail = match (e.code(), e.message()) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        _ => aws_sdk_sts::error::DisplayErrorContext(&e).to_string(),
    };
    cloud_err("aws", what, detail)
}

/// Whether a dry-run error says the call would have succeeded.
fn dry_run_ok(code: Option<&str>) -> bool {
    code == Some("DryRunOperation")
}

#[async_trait]
impl CloudApi for Aws {
    fn name(&self) -> &'static str {
        "aws"
    }

    fn title(&self) -> &'static str {
        "AWS"
    }

    fn tier(&self) -> Tier {
        Tier::Vm
    }

    fn arches(&self) -> Vec<&'static str> {
        vec!["arm64", "amd64"]
    }

    fn detect(&self) -> CloudCredentials {
        let profile = env("AWS_PROFILE").unwrap_or_else(|| "default".into());
        if env("AWS_ACCESS_KEY_ID").is_some() {
            return CloudCredentials {
                found: true,
                source: "AWS_ACCESS_KEY_ID in the environment".into(),
            };
        }
        let files: Vec<_> = [config_file(), credentials_file()]
            .into_iter()
            .flatten()
            .filter(|p| p.is_file())
            .collect();
        if files.is_empty() {
            return CloudCredentials::default();
        }
        CloudCredentials {
            found: true,
            source: format!("~/.aws profile {profile}"),
        }
    }

    async fn identity(&self, conn: &Connection) -> Result<crate::api::Identity> {
        let c = self.clients(conn).await;
        let tap = ClockTap::default();
        let me = c
            .sts
            .get_caller_identity()
            .customize()
            .interceptor(tap.clone())
            .send()
            .await
            .map_err(|e| sts_err("identify the account", e))?;
        let clock = *tap.0.lock().expect("clock tap");
        Ok(crate::api::Identity {
            id: me.account().map(str::to_string).filter(|a| !a.is_empty()),
            clock,
        })
    }

    fn expected_identity(&self, conn: &Connection) -> String {
        if conn.identity.is_empty() {
            conn.account.clone()
        } else {
            conn.identity.clone()
        }
    }

    fn sign_in_help(&self, conn: &Connection, error: &str) -> Option<crate::signin::SignIn> {
        let config = config_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        sign_in_of(&conn.profile, &config, error)
    }

    fn resolve(&self, target: &Target) -> Result<Connection> {
        let profile = some(&target.profile)
            .or_else(|| env("AWS_PROFILE"))
            .unwrap_or_else(|| "default".into());
        let region = some(&target.region)
            .or_else(|| env("AWS_REGION"))
            .or_else(|| env("AWS_DEFAULT_REGION"))
            .or_else(|| profile_region(&profile))
            .unwrap_or_else(|| DEFAULT_REGION.into());
        Ok(Connection {
            provider: "aws".into(),
            profile,
            region,
            ..Default::default()
        })
    }

    fn kinds(&self, _conn: &Connection) -> Vec<CloudKind> {
        let offer = |image: &str, family: &str| {
            let t = instance_type(family, "arm64");
            CloudKind {
                image: image.into(),
                kind: "container".into(),
                supported: true,
                reason: format!("{t} (arm64), Docker on Ubuntu 24.04"),
                machine_type: t.into(),
                usd_per_hour: ((price(t) + extras_per_hour(crate::provider::disk_for(family)))
                    * 10_000.0)
                    .round()
                    / 10_000.0,
            }
        };
        let no = |image: &str, kind: &str, reason: &str| CloudKind {
            image: image.into(),
            kind: kind.into(),
            supported: false,
            reason: reason.into(),
            ..Default::default()
        };
        vec![
            offer("linux", "linux"),
            offer("linux-slim", "linux-slim"),
            no(
                "windows",
                "vm",
                "needs a nested-virtualization instance; not offered on AWS yet",
            ),
            no(
                "omarchy",
                "vm",
                "needs a nested-virtualization instance; not offered on AWS yet",
            ),
            no(
                "macos",
                "vm",
                "EC2 Mac: 24 h minimum, about $30; not offered",
            ),
        ]
    }

    async fn test(&self, conn: &Connection) -> Tested {
        let mut t = Tested::default();
        let c = self.clients(conn).await;
        match c.sts.get_caller_identity().send().await {
            Ok(me) => {
                t.account = me.account().unwrap_or_default().to_string();
                t.check(
                    "credentials",
                    true,
                    format!(
                        "account {} ({})",
                        t.account,
                        me.arn()
                            .unwrap_or_default()
                            .rsplit('/')
                            .next()
                            .unwrap_or("")
                    ),
                );
            }
            Err(e) => {
                t.check(
                    "credentials",
                    false,
                    format!(
                        "{} (sign in with `aws sso login --profile {}` or `aws configure`)",
                        sts_err("identify the account", e),
                        conn.profile
                    ),
                );
                return t;
            }
        }
        let arch = "arm64";
        let itype = instance_type("linux", arch);
        match c
            .ec2
            .describe_instance_type_offerings()
            .location_type(LocationType::Region)
            .filters(filter("instance-type", &[itype]))
            .send()
            .await
        {
            Ok(o) if !o.instance_type_offerings().is_empty() => {
                t.check("region", true, format!("{} offers {itype}", conn.region))
            }
            Ok(_) => t.check(
                "region",
                false,
                format!(
                    "{} does not offer {itype}; connect another region",
                    conn.region
                ),
            ),
            Err(e) => t.check(
                "region",
                false,
                aws_err("list instance types", e).to_string(),
            ),
        }
        let (image, root) = match self.ubuntu(&c.ec2, arch).await {
            Ok(i) => {
                t.check("image", true, format!("Ubuntu 24.04 {arch}: {}", i.0));
                i
            }
            Err(e) => {
                t.check("image", false, e.to_string());
                return t;
            }
        };
        let vpc = match self.default_vpc(&c.ec2).await {
            Ok(v) => {
                t.check("network", true, format!("default VPC {v}"));
                v
            }
            Err(e) => {
                t.check("network", false, e.to_string());
                return t;
            }
        };
        // The exact create, as a dry run: nothing is made.
        let owner = "cua-test-dry-run";
        let spec = ProvisionSpec {
            name: "cua-aws-dry-run".into(),
            disk_gb: 30,
            user_data: "#cloud-config\n".into(),
            tags: model::tags(owner, "dry-run", "", 0),
            ..Default::default()
        };
        let group = match self.find_group(&c.ec2, &vpc, owner, false).await {
            Ok(Some(g)) => g.group_id().map(str::to_string),
            _ => None,
        };
        // Tagged like the real create: the least-privilege policy only
        // allows a security group created with the managed tag.
        let mut sg_tags = TagSpecification::builder().resource_type(ResourceType::SecurityGroup);
        for (k, v) in &spec.tags {
            sg_tags = sg_tags.tags(Tag::builder().key(k).value(v).build());
        }
        let sg_dry = c
            .ec2
            .create_security_group()
            .group_name("cua-sandboxes-dry-run")
            .description("dry run")
            .vpc_id(&vpc)
            .tag_specifications(sg_tags.build())
            .dry_run(true)
            .send()
            .await;
        let run_dry = self
            .run_request(&c.ec2, &spec, &image, &root, group.as_deref(), itype)
            .dry_run(true)
            .send()
            .await;
        let (sg_ok, sg_detail) = match &sg_dry {
            Err(e) if dry_run_ok(e.code()) => (true, String::new()),
            Err(e) => (
                false,
                format!("CreateSecurityGroup: {}", e.code().unwrap_or("error")),
            ),
            Ok(_) => (true, String::new()),
        };
        let (run_ok, run_detail) = match &run_dry {
            Err(e) if dry_run_ok(e.code()) => (true, String::new()),
            Err(e) => (
                false,
                format!(
                    "RunInstances: {}: {}",
                    e.code().unwrap_or("error"),
                    e.message().unwrap_or_default()
                ),
            ),
            Ok(_) => (true, String::new()),
        };
        t.check(
            "permissions",
            sg_ok && run_ok,
            if sg_ok && run_ok {
                "RunInstances and CreateSecurityGroup would succeed (dry run)".to_string()
            } else {
                let mut why = [sg_detail, run_detail]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("; ");
                let denied = |code: Option<&str>| code.is_some_and(|c| c.contains("Unauthorized"));
                if sg_dry.as_ref().err().is_some_and(|e| denied(e.code()))
                    || run_dry.as_ref().err().is_some_and(|e| denied(e.code()))
                {
                    why.push_str(&format!(
                        "; attach Cua's least-privilege policy to this identity: {}",
                        crate::policy::DOCS_URL
                    ));
                }
                why
            },
        );
        t
    }

    async fn provision(&self, conn: &Connection, spec: &ProvisionSpec) -> Result<Vec<Resource>> {
        // A host may go to another region than the connection's.
        let in_region;
        let conn = if spec.region.is_empty() || spec.region == conn.region {
            conn
        } else {
            in_region = Connection {
                region: spec.region.clone(),
                ..conn.clone()
            };
            &in_region
        };
        let c = self.clients(conn).await;
        let owner = spec
            .tags
            .get(tag::OWNER)
            .cloned()
            .ok_or_else(|| Error::InvalidArgument("aws: the create has no owner tag".into()))?;
        let arch = if spec.arch.is_empty() {
            "arm64"
        } else {
            spec.arch.as_str()
        };
        let itype = for_arch(&spec.machine_type, arch);
        // A public SSM parameter names the image: EC2 resolves it in the
        // create itself. Canonical's and the Deep Learning images both
        // boot from /dev/sda1.
        // A GPU host boots the Deep Learning image (driver, CUDA, Docker
        // with the NVIDIA toolkit) unless asked otherwise.
        let image_parameter =
            if spec.image_parameter.is_empty() && spec.is_host() && is_gpu_type(&itype) {
                host_image_parameter(true, arch)
            } else {
                spec.image_parameter.clone()
            };
        let (image, root) = if image_parameter.is_empty() {
            self.ubuntu(&c.ec2, arch).await?
        } else {
            if !image_parameter.starts_with("/aws/service/") {
                return Err(Error::InvalidArgument(format!(
                    "aws: {image_parameter} is not a public /aws/service/ image parameter"
                )));
            }
            (
                format!("resolve:ssm:{image_parameter}"),
                "/dev/sda1".to_string(),
            )
        };
        let (group, made_group) = self
            .ensure_group(&c.ec2, conn, &owner, spec.peer_ports)
            .await?;
        let mut sent = self
            .run_request(&c.ec2, spec, &image, &root, Some(&group), &itype)
            .client_token(&spec.name)
            .send()
            .await;
        // No capacity for this type where EC2 put it (or the type is not
        // offered there): try each zone of the default VPC (GPU types often
        // have room in another one). Every attempt and why it failed goes
        // into the error, so nobody has to guess which zones were tried.
        let mut tried = Vec::new();
        if let Err(e) = &sent
            && zone_bound(e.code())
        {
            tried.push(format!("EC2's pick of zone: {}", err_detail(e)));
            for (i, (zone, subnet)) in self.default_subnets(&c.ec2).await?.iter().enumerate() {
                sent = self
                    .run_request(&c.ec2, spec, &image, &root, Some(&group), &itype)
                    .subnet_id(subnet)
                    .client_token(format!("{}-z{i}", spec.name))
                    .send()
                    .await;
                match &sent {
                    Err(e) => {
                        tried.push(format!("{zone} ({subnet}): {}", err_detail(e)));
                        if !zone_bound(e.code()) {
                            break;
                        }
                    }
                    Ok(_) => break,
                }
            }
        }
        let out = match sent {
            Ok(out) => out,
            Err(e) if tried.is_empty() => return Err(aws_err("start the instance", e)),
            Err(_) => {
                return Err(cloud_err(
                    "aws",
                    &format!("start a {itype} in {}", conn.region),
                    format!("tried {} placements: {}", tried.len(), tried.join("; ")),
                ));
            }
        };
        let inst = out
            .instances()
            .first()
            .ok_or_else(|| Error::Cloud("aws: RunInstances returned no instance".into()))?;
        let mut r = Resource {
            provider: "aws".into(),
            id: inst.instance_id().unwrap_or_default().to_string(),
            resource_type: "instance".into(),
            name: spec.name.clone(),
            region: conn.region.clone(),
            created: model::now(),
            expires: model::tag_expires(&spec.tags),
            tags: spec.tags.clone(),
            state: state_name(inst),
            ..Default::default()
        };
        r.extra.insert("instance_type".into(), itype);
        r.extra.insert(
            "image".into(),
            inst.image_id().map(str::to_string).unwrap_or(image),
        );
        r.extra.insert("security_group".into(), group);
        let mut all = vec![r];
        all.extend(made_group);
        Ok(all)
    }

    async fn describe(
        &self,
        conn: &Connection,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        let c = self.clients(conn).await;
        match r.resource_type.as_str() {
            "security_group" => {
                match c
                    .ec2
                    .describe_security_groups()
                    .group_ids(&r.id)
                    .send()
                    .await
                {
                    Ok(out) => Ok(out
                        .security_groups()
                        .first()
                        .map(|g| ("available".to_string(), tags_of(g.tags())))),
                    Err(e) if e.code() == Some("InvalidGroup.NotFound") => Ok(None),
                    Err(e) => Err(aws_err("describe the security group", e)),
                }
            }
            _ => Ok(self
                .find_instance(&c.ec2, r)
                .await?
                .map(|i| (state_name(&i), tags_of(i.tags())))),
        }
    }

    async fn stop(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        if self.guard(conn, r, owner, "stop").await?.is_none() {
            return Err(Error::NotFound(format!("aws instance {}", r.id)));
        }
        let c = self.clients(conn).await;
        c.ec2
            .stop_instances()
            .instance_ids(&r.id)
            .send()
            .await
            .map_err(|e| aws_err("stop the instance", e))?;
        Ok(())
    }

    async fn start(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        if self.guard(conn, r, owner, "start").await?.is_none() {
            return Err(Error::NotFound(format!("aws instance {}", r.id)));
        }
        let c = self.clients(conn).await;
        // A stop still under way refuses a start: wait for it (bounded).
        for _ in 0..60 {
            match self.describe(conn, r).await?.map(|(s, _)| s).as_deref() {
                Some("stopping") => tokio::time::sleep(std::time::Duration::from_secs(5)).await,
                _ => break,
            }
        }
        c.ec2
            .start_instances()
            .instance_ids(&r.id)
            .send()
            .await
            .map_err(|e| aws_err("start the instance", e))?;
        Ok(())
    }

    async fn delete(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        let Some(state) = self.guard(conn, r, owner, "delete").await? else {
            return Ok(());
        };
        let c = self.clients(conn).await;
        match r.resource_type.as_str() {
            "security_group" => {
                // An instance terminated a moment ago still holds the group
                // through its network interface: wait for it (bounded).
                let mut tries = 0;
                loop {
                    match c.ec2.delete_security_group().group_id(&r.id).send().await {
                        Ok(_) => break Ok(()),
                        Err(e) if e.code() == Some("InvalidGroup.NotFound") => break Ok(()),
                        Err(e) if e.code() == Some("DependencyViolation") && tries < 24 => {
                            tries += 1;
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                        Err(e) => break Err(aws_err("delete the Cua security group", e)),
                    }
                }
            }
            _ => {
                if state == "terminated" {
                    return Ok(());
                }
                match c.ec2.terminate_instances().instance_ids(&r.id).send().await {
                    Ok(_) => Ok(()),
                    Err(e) if e.code() == Some("InvalidInstanceID.NotFound") => Ok(()),
                    Err(e) => Err(aws_err("terminate the instance", e)),
                }
            }
        }
    }

    async fn boot_log(&self, conn: &Connection, r: &Resource) -> Result<Option<String>> {
        if r.id.is_empty() || r.resource_type == "security_group" {
            return Ok(None);
        }
        let c = self.clients(conn).await;
        // `latest` returns the newest output (Nitro instances only); the
        // plain call returns the last 64 KiB and works everywhere.
        let out = match c
            .ec2
            .get_console_output()
            .instance_id(&r.id)
            .latest(true)
            .send()
            .await
        {
            Ok(o) => o,
            Err(_) => c
                .ec2
                .get_console_output()
                .instance_id(&r.id)
                .send()
                .await
                .map_err(|e| aws_err("read the console output", e))?,
        };
        Ok(out.output().filter(|o| !o.trim().is_empty()).map(|o| {
            use base64::Engine as _;
            let compact: String = o.split_whitespace().collect();
            base64::engine::general_purpose::STANDARD
                .decode(compact.as_bytes())
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_else(|| o.to_string())
        }))
    }

    async fn list_owned(&self, conn: &Connection, owner: &str) -> Result<Vec<Resource>> {
        let c = self.clients(conn).await;
        let mut out = Vec::new();
        let mut req = c.ec2.describe_instances().filters(filter(
            "instance-state-name",
            &["pending", "running", "stopping", "stopped", "shutting-down"],
        ));
        for f in owner_filters(owner) {
            req = req.filters(f);
        }
        // Every page, or an error: a listing that stopped half way must never
        // read as "the rest is gone".
        let mut reservations = Vec::new();
        let mut pages = req.into_paginator().send();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|e| aws_err("list Cua instances", e))?;
            reservations.extend(page.reservations().iter().cloned());
        }
        for i in reservations.iter().flat_map(|x| x.instances()) {
            let tags = tags_of(i.tags());
            if !model::is_ours(&tags, owner) {
                continue;
            }
            out.push(Resource {
                provider: "aws".into(),
                id: i.instance_id().unwrap_or_default().to_string(),
                resource_type: "instance".into(),
                name: tags.get("Name").cloned().unwrap_or_default(),
                region: conn.region.clone(),
                created: i.launch_time().map(|t| t.secs().max(0) as u64).unwrap_or(0),
                expires: model::tag_expires(&tags),
                tags,
                state: state_name(i),
                ..Default::default()
            });
        }
        let mut req = c.ec2.describe_security_groups();
        for f in owner_filters(owner) {
            req = req.filters(f);
        }
        let mut groups = Vec::new();
        let mut pages = req.into_paginator().send();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|e| aws_err("list Cua security groups", e))?;
            groups.extend(page.security_groups().iter().cloned());
        }
        for g in &groups {
            let tags = tags_of(g.tags());
            if !model::is_ours(&tags, owner) {
                continue;
            }
            out.push(Resource {
                provider: "aws".into(),
                id: g.group_id().unwrap_or_default().to_string(),
                resource_type: "security_group".into(),
                name: g.group_name().unwrap_or_default().to_string(),
                region: conn.region.clone(),
                tags,
                state: "available".into(),
                ..Default::default()
            });
        }
        Ok(out)
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        let mut regions = vec![conn.region.clone()];
        for r in HOST_REGIONS {
            if !regions.iter().any(|x| x == r) {
                regions.push((*r).to_string());
            }
        }
        regions.retain(|r| !r.is_empty());
        Capabilities {
            supplies: Supplies::Hosts,
            billing: Billing {
                granularity_secs: 1,
                minimum_secs: 60,
                stopped_bills: false,
            },
            regions,
            max_machines: 20,
            // Instances shut down from inside are terminated.
            vcpu_quotas: Default::default(),
            self_deletes: true,
        }
    }

    async fn host_types(&self, conn: &Connection) -> Result<Vec<HostType>> {
        let regions = self.capabilities(conn).regions;
        let mut v: Vec<HostType> = HOST_TYPES
            .iter()
            .map(|(name, arch, cpus, gb, usd, kvm, gpu)| HostType {
                name: (*name).into(),
                arch: (*arch).into(),
                vcpus: *cpus,
                memory_mb: gb * 1024,
                disk_gb: if gpu.is_empty() { HOST_DISK_GB } else { 100 },
                kvm: *kvm,
                gpu: (*gpu).into(),
                usd_per_hour: ((usd + extras_per_hour(HOST_DISK_GB)) * 10_000.0).round() / 10_000.0,
                regions: regions.clone(),
                boot_secs: if *kvm { 300 } else { 90 },
                // EC2's on-demand vCPU quotas: standard families, G, P.
                quota_pool: match name.chars().next() {
                    Some('g') if !gpu.is_empty() => "g",
                    Some('p') if !gpu.is_empty() => "p",
                    _ => "standard",
                }
                .into(),
            })
            .collect();
        v.sort_by(|a, b| {
            a.usd_per_hour
                .total_cmp(&b.usd_per_hour)
                .then(a.name.cmp(&b.name))
        });
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The error a user hit with an expired `aws login` session.
    const EXPIRED_LOGIN: &str = "your cloud: aws: list Cua instances: dispatch failure: other: an error occurred while loading credentials: failed to refresh cached Login token: Your session has expired. Please reauthenticate.: AccessDeniedException: The refresh token has expired";

    #[test]
    fn credential_errors_read_as_one_line_with_the_fix() {
        let login = "[profile sandbox]\nregion = us-west-2\nlogin_session = arn:aws:iam::1:user/x\n";
        let sso = "[profile work]\nsso_session = corp\n[sso-session corp]\nsso_start_url = https://x\n";
        let s = sign_in_of("default", "", EXPIRED_LOGIN).unwrap();
        assert_eq!(s.kind, crate::signin::Kind::Expired);
        assert_eq!(
            s.line,
            "Your AWS sign-in expired. Sign in with `aws login`, then run this again."
        );
        assert_eq!(
            sign_in_of("sandbox", login, EXPIRED_LOGIN).unwrap().line,
            "Your AWS sign-in expired. Sign in with `aws login --profile sandbox`, then run this again."
        );
        assert_eq!(
            sign_in_of("work", sso, "ExpiredTokenException: The security token included in the request is expired").unwrap().line,
            "Your AWS sign-in expired. Sign in with `aws sso login --profile work`, then run this again."
        );
        let missing = sign_in_of("gone", "", "dispatch failure: other: no providers in chain provided credentials").unwrap();
        assert_eq!(missing.kind, crate::signin::Kind::Missing);
        assert!(missing.line.contains("`aws login --profile gone`"), "{}", missing.line);
        let rejected = sign_in_of("default", "[default]\naws_access_key_id = x\n", "InvalidClientTokenId: The security token included in the request is invalid").unwrap();
        assert_eq!(rejected.kind, crate::signin::Kind::Rejected);
        assert!(rejected.line.contains("`aws configure`"), "{}", rejected.line);
        // Not sign-in problems.
        assert_eq!(sign_in_of("default", "", "RequestExpired: Request has expired"), None);
        assert_eq!(sign_in_of("default", "", "UnauthorizedOperation: not allowed to RunInstances"), None);
        assert_eq!(sign_in_of("default", "", "InsufficientInstanceCapacity"), None);
    }

    #[test]
    fn types_prices_and_arches() {
        assert_eq!(instance_type("linux", "arm64"), "t4g.medium");
        assert_eq!(instance_type("linux-slim", "amd64"), "t3.small");
        assert_eq!(for_arch("t4g.medium", "amd64"), "t3.medium");
        assert_eq!(for_arch("t4g.medium", "arm64"), "t4g.medium");
        let k = Aws::new().kinds(&Connection::default());
        let linux = k.iter().find(|k| k.image == "linux").unwrap();
        assert!(linux.supported && linux.usd_per_hour > 0.03 && linux.usd_per_hour < 0.05);
        assert!(!k.iter().find(|k| k.image == "macos").unwrap().supported);
    }

    #[test]
    fn host_images_are_public_ssm_parameters() {
        assert_eq!(
            host_image_parameter(false, "amd64"),
            "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id"
        );
        assert_eq!(
            host_image_parameter(true, "amd64"),
            "/aws/service/deeplearning/ami/x86_64/base-oss-nvidia-driver-gpu-ubuntu-24.04/latest/ami-id"
        );
        assert!(host_image_parameter(true, "arm64").contains("/ami/arm64/"));
    }
}
