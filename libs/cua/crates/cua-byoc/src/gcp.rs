// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Google Compute Engine (`--on gcp`): one small VM per sandbox, over the
//! Compute Engine REST API (v1) with the gcloud CLI's sign-in (an access
//! token from `gcloud auth print-access-token`, for the connection's
//! account when it names one). Credentials stay in gcloud; Cua stores the
//! project, region, zone and account names only.
//!
//! What a sandbox creates, all labelled with the sandbox's tags:
//!
//! - one VM (`e2-medium`, `e2-small` for the slim image) from Ubuntu 24.04
//!   LTS, whose cloud-init runs the image with Docker; no service account
//!   (nothing on the VM can call Google APIs), an ephemeral external IP for
//!   egress, `scheduling.maxRunDuration` = the time limit with
//!   `instanceTerminationAction=DELETE`, a boot disk deleted with it;
//! - once per cua home and project, a dedicated VPC network
//!   `cua-sb-<owner>` (auto subnets) with **no firewall rules**, so nothing
//!   reaches the VMs from outside (the implied rules deny ingress and allow
//!   egress; the guest only dials out to the relay). The project's default
//!   network, whose `default-allow-ssh` and `default-allow-rdp` accept the
//!   whole internet, is never used or changed. Networks carry no labels:
//!   its description holds the tags (`cua-managed=true cua-owner=...`) and
//!   is checked the same way before Cua reuses or deletes it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{CloudCredentials, CloudKind};
use serde_json::{Value, json};

use crate::api::{CloudApi, Result, Target, Tested, cloud_err};
use crate::model::{self, Connection, ProvisionSpec, Resource, Tier};

/// The Compute Engine API root.
pub const COMPUTE: &str = "https://compute.googleapis.com/compute/v1";
/// The Resource Manager API root (permission checks).
pub const RESOURCE_MANAGER: &str = "https://cloudresourcemanager.googleapis.com/v1";

/// The default region (cheapest e2 prices, every e2 type).
pub const DEFAULT_REGION: &str = "us-central1";

/// Machine type per image family, and its us-central1 on-demand price
/// (US dollars per hour) with a 30 GiB balanced disk ($0.0041/h) and an
/// ephemeral external IPv4 address ($0.005/h).
const OFFERS: &[(&str, &str, f64)] = &[
    ("linux", "e2-medium", 0.03351 + 0.0041 + 0.005),
    ("linux-slim", "e2-small", 0.01675 + 0.0041 + 0.005),
];

/// The permissions a create, stop, start and delete use.
pub const PERMISSIONS: &[&str] = &[
    "compute.instances.create",
    "compute.instances.delete",
    "compute.instances.get",
    "compute.instances.list",
    "compute.instances.start",
    "compute.instances.stop",
    "compute.instances.setMetadata",
    "compute.instances.setLabels",
    "compute.disks.create",
    "compute.networks.create",
    "compute.networks.get",
    "compute.networks.delete",
    "compute.subnetworks.use",
    "compute.subnetworks.useExternalIp",
];

/// Where an access token comes from.
#[async_trait]
pub trait TokenSource: Send + Sync + 'static {
    /// A fresh access token for `account` (`None`: the active account).
    async fn token(&self, account: Option<&str>) -> Result<String>;
}

/// The gcloud CLI's sign-in.
pub struct Gcloud;

#[async_trait]
impl TokenSource for Gcloud {
    async fn token(&self, account: Option<&str>) -> Result<String> {
        let mut cmd = tokio::process::Command::new("gcloud");
        cmd.args(["auth", "print-access-token", "--quiet"]);
        if let Some(a) = account.filter(|a| !a.is_empty()) {
            cmd.arg(format!("--account={a}"));
        }
        cmd.stdin(std::process::Stdio::null());
        let out = tokio::time::timeout(Duration::from_secs(60), cmd.output())
            .await
            .map_err(|_| cloud_err("gcp", "gcloud auth print-access-token", "timed out"))?
            .map_err(|e| {
                cloud_err(
                    "gcp",
                    "gcloud",
                    format!("{e} (install the Google Cloud CLI and run `gcloud auth login`)"),
                )
            })?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(cloud_err(
                "gcp",
                "gcloud sign-in",
                format!("{} (run `gcloud auth login`)", err.trim()),
            ));
        }
        let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if t.is_empty() {
            return Err(cloud_err("gcp", "gcloud sign-in", "no access token"));
        }
        Ok(t)
    }
}

/// Compute Engine.
pub struct Gcp {
    http: reqwest::Client,
    compute: String,
    resource_manager: String,
    tokens: Arc<dyn TokenSource>,
    cache: tokio::sync::Mutex<BTreeMap<String, (String, Instant)>>,
    config_dir: Option<PathBuf>,
}

impl Default for Gcp {
    fn default() -> Self {
        Self::new()
    }
}

impl Gcp {
    /// The real API with the gcloud CLI's sign-in.
    pub fn new() -> Self {
        Self::with_endpoints(
            COMPUTE,
            RESOURCE_MANAGER,
            Arc::new(Gcloud),
            gcloud_config_dir(),
        )
    }

    /// Other API roots and token source (a local fake in tests), and the
    /// gcloud configuration directory detection reads (`None`: none).
    pub fn with_endpoints(
        compute: &str,
        resource_manager: &str,
        tokens: Arc<dyn TokenSource>,
        config_dir: Option<PathBuf>,
    ) -> Self {
        Gcp {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("http client"),
            compute: compute.trim_end_matches('/').into(),
            resource_manager: resource_manager.trim_end_matches('/').into(),
            tokens,
            cache: Default::default(),
            config_dir,
        }
    }

    async fn token(&self, conn: &Connection) -> Result<String> {
        let key = conn.profile.clone();
        let mut cache = self.cache.lock().await;
        // gcloud hands out its own cached token, which may be close to
        // expiry: reuse one for a few minutes only (a 401 drops it too).
        if let Some((t, at)) = cache.get(&key)
            && at.elapsed() < Duration::from_secs(5 * 60)
        {
            return Ok(t.clone());
        }
        let t = self
            .tokens
            .token(
                Some(&conn.profile)
                    .filter(|p| !p.is_empty())
                    .map(String::as_str),
            )
            .await?;
        cache.insert(key, (t.clone(), Instant::now()));
        Ok(t)
    }

    /// One call: `Ok(None)` for a 404, the cloud's message otherwise.
    async fn call(
        &self,
        conn: &Connection,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
        what: &str,
    ) -> Result<Option<Value>> {
        let mut retried = false;
        let resp = loop {
            let token = self.token(conn).await?;
            let mut req = self.http.request(method.clone(), url).bearer_auth(token);
            match body {
                Some(b) => req = req.json(b),
                // Google answers a POST without a length with 411.
                None if method == reqwest::Method::POST => {
                    req = req.header(reqwest::header::CONTENT_LENGTH, "0")
                }
                None => {}
            }
            let resp = req
                .send()
                .await
                .map_err(|e| cloud_err("gcp", what, e.without_url()))?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && !retried {
                // An expired token: drop it and ask gcloud again, once.
                retried = true;
                self.cache.lock().await.remove(&conn.profile);
                continue;
            }
            break resp;
        };
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = v["error"]["message"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(cloud_err("gcp", what, msg));
        }
        Ok(Some(v))
    }

    async fn get(&self, conn: &Connection, url: &str, what: &str) -> Result<Option<Value>> {
        self.call(conn, reqwest::Method::GET, url, None, what).await
    }

    fn project_url(&self, conn: &Connection) -> String {
        format!("{}/projects/{}", self.compute, conn.project)
    }

    fn zone_url(&self, conn: &Connection) -> String {
        format!("{}/zones/{}", self.project_url(conn), conn.zone)
    }

    fn instance_url(&self, conn: &Connection, name: &str) -> String {
        format!("{}/instances/{name}", self.zone_url(conn))
    }

    fn network_url(&self, conn: &Connection, name: &str) -> String {
        format!("{}/global/networks/{name}", self.project_url(conn))
    }

    /// Waits for a long-running operation (zonal or global) to finish, and
    /// fails with its error.
    async fn wait(&self, conn: &Connection, op: &Value, what: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10 * 60);
        let mut op = op.clone();
        loop {
            if op["status"] == "DONE" {
                if let Some(errs) = op["error"]["errors"].as_array()
                    && !errs.is_empty()
                {
                    let msg: Vec<String> = errs
                        .iter()
                        .map(|e| e["message"].as_str().unwrap_or("error").to_string())
                        .collect();
                    return Err(cloud_err("gcp", what, msg.join("; ")));
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "gcp: {what}: the operation did not finish"
                )));
            }
            let link = op["selfLink"]
                .as_str()
                .ok_or_else(|| cloud_err("gcp", what, "the operation has no selfLink"))?;
            // The real API's links are absolute; a fake's may be too.
            let url = format!("{}/wait", self.rebase(link));
            op = self
                .call(conn, reqwest::Method::POST, &url, None, what)
                .await?
                .ok_or_else(|| cloud_err("gcp", what, "the operation is gone"))?;
        }
    }

    /// A `selfLink` under this client's API root (they differ only for a
    /// fake server).
    fn rebase(&self, link: &str) -> String {
        match link.find("/projects/") {
            Some(i) => format!("{}{}", self.compute, &link[i..]),
            None => link.to_string(),
        }
    }

    /// The network of `owner` in this project.
    fn network_name(owner: &str) -> String {
        format!("cua-sb-{}", model::label_value(owner))
            .chars()
            .take(63)
            .collect()
    }

    /// The dedicated network: reused when it is this owner's, created (with
    /// no firewall rules) when missing, refused when a network of that name
    /// is not Cua's.
    async fn ensure_network(&self, conn: &Connection, owner: &str) -> Result<Resource> {
        let name = Self::network_name(owner);
        let url = self.network_url(conn, &name);
        let tags = network_tags(owner);
        match self.get(conn, &url, "get the Cua network").await? {
            Some(n) => {
                let seen = description_tags(n["description"].as_str().unwrap_or(""));
                if !model::is_ours(&seen, owner) {
                    return Err(cloud_err(
                        "gcp",
                        "the Cua network",
                        format!(
                            "network {name} exists and is not tagged as this Cua owner's; it is left alone"
                        ),
                    ));
                }
            }
            None => {
                let body = json!({
                    "name": name,
                    "description": describe_tags(&tags),
                    "autoCreateSubnetworks": true,
                    "routingConfig": {"routingMode": "REGIONAL"},
                    // The common Ethernet MTU, so containers on the VM
                    // need no MTU of their own.
                    "mtu": 1500,
                });
                let op = self
                    .call(
                        conn,
                        reqwest::Method::POST,
                        &format!("{}/global/networks", self.project_url(conn)),
                        Some(&body),
                        "create the Cua network",
                    )
                    .await?
                    .unwrap_or(Value::Null);
                self.wait(conn, &op, "create the Cua network").await?;
            }
        }
        Ok(Resource {
            provider: "gcp".into(),
            id: name.clone(),
            resource_type: "network".into(),
            name,
            project: conn.project.clone(),
            created: model::now(),
            tags,
            state: "ready".into(),
            ..Default::default()
        })
    }

    async fn guard(
        &self,
        conn: &Connection,
        r: &Resource,
        owner: &str,
        verb: &str,
    ) -> Result<bool> {
        match self.describe(conn, r).await? {
            None => Ok(false),
            Some((_, tags)) if model::is_ours(&tags, owner) => Ok(true),
            Some(_) => Err(cloud_err(
                "gcp",
                verb,
                format!(
                    "{} {} is not tagged as this Cua owner's; it is left alone",
                    r.resource_type, r.id
                ),
            )),
        }
    }

    async fn instance_op(
        &self,
        conn: &Connection,
        r: &Resource,
        owner: &str,
        op: &str,
    ) -> Result<()> {
        if !self.guard(conn, r, owner, op).await? {
            return Err(Error::NotFound(format!("gcp instance {}", r.id)));
        }
        let url = format!("{}/{op}", self.instance_url(conn, &r.id));
        let v = self
            .call(
                conn,
                reqwest::Method::POST,
                &url,
                None,
                &format!("{op} instance {}", r.id),
            )
            .await?
            .unwrap_or(Value::Null);
        self.wait(conn, &v, &format!("{op} instance {}", r.id))
            .await
    }
}

/// The tags of the dedicated network (it belongs to no one sandbox).
fn network_tags(owner: &str) -> BTreeMap<String, String> {
    model::tags(owner, "", "", 0)
        .into_iter()
        .filter(|(k, _)| k != model::tag::SPACE && k != model::tag::EXPIRES)
        .collect()
}

/// `Cua sandboxes, no ingress. cua-managed=true cua-owner=...`.
fn describe_tags(tags: &BTreeMap<String, String>) -> String {
    let kv: Vec<String> = tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!(
        "Cua sandboxes (no firewall rules: no ingress). {}",
        kv.join(" ")
    )
}

/// The `k=v` tags a description carries.
fn description_tags(d: &str) -> BTreeMap<String, String> {
    d.split_whitespace()
        .filter_map(|w| w.split_once('='))
        .filter(|(k, _)| k.starts_with("cua-"))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn labels_of(v: &Value) -> BTreeMap<String, String> {
    v["labels"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// A Compute Engine status as a provider state.
fn state_of(status: &str) -> String {
    match status {
        "RUNNING" => "running",
        "PROVISIONING" | "STAGING" | "REPAIRING" => "pending",
        "STOPPING" | "SUSPENDING" => "stopping",
        // TERMINATED is a stopped VM on Compute Engine (it can start again).
        "TERMINATED" | "SUSPENDED" => "stopped",
        _ => "unknown",
    }
    .to_string()
}

/// `$CLOUDSDK_CONFIG`, else `~/.config/gcloud`.
fn gcloud_config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CLOUDSDK_CONFIG").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("gcloud"))
}

/// The active gcloud configuration's `account` and `project`.
fn gcloud_active(dir: &std::path::Path) -> (Option<String>, Option<String>) {
    let name = std::fs::read_to_string(dir.join("active_config"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "default".into());
    let text = std::fs::read_to_string(dir.join("configurations").join(format!("config_{name}")))
        .unwrap_or_default();
    let mut section = String::new();
    let (mut account, mut project) = (None, None);
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            section = l.trim_matches(['[', ']']).to_string();
        } else if section == "core"
            && let Some((k, v)) = l.split_once('=')
        {
            match k.trim() {
                "account" => account = Some(v.trim().to_string()),
                "project" => project = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    (
        account.filter(|a| !a.is_empty()),
        project.filter(|p| !p.is_empty()),
    )
}

#[async_trait]
impl CloudApi for Gcp {
    fn name(&self) -> &'static str {
        "gcp"
    }

    fn title(&self) -> &'static str {
        "Google Cloud"
    }

    fn tier(&self) -> Tier {
        Tier::Vm
    }

    fn arches(&self) -> Vec<&'static str> {
        // e2 (amd64) is cheaper than the Arm t2a types and in every region.
        vec!["amd64"]
    }

    fn detect(&self) -> CloudCredentials {
        let Some(dir) = &self.config_dir else {
            return CloudCredentials::default();
        };
        match gcloud_active(dir) {
            (Some(account), project) => CloudCredentials {
                found: true,
                source: match project {
                    Some(p) => format!("gcloud account {account}, project {p}"),
                    None => format!("gcloud account {account}"),
                },
            },
            _ => CloudCredentials::default(),
        }
    }

    fn resolve(&self, t: &Target) -> Result<Connection> {
        let (account, project) = self
            .config_dir
            .as_deref()
            .map(gcloud_active)
            .unwrap_or((None, None));
        let project = t.project.clone().or(project).ok_or_else(|| {
            Error::InvalidArgument(
                "gcp: no project: pass --project (or `gcloud config set project <id>`)".into(),
            )
        })?;
        let zone = t.zone.clone();
        let region = t
            .region
            .clone()
            .or_else(|| {
                zone.as_deref()
                    .and_then(|z| z.rsplit_once('-'))
                    .map(|(r, _)| r.to_string())
            })
            .unwrap_or_else(|| DEFAULT_REGION.into());
        let zone = zone.unwrap_or_else(|| format!("{region}-a"));
        if !zone.starts_with(&format!("{region}-")) {
            return Err(Error::InvalidArgument(format!(
                "gcp: zone {zone} is not in region {region}"
            )));
        }
        Ok(Connection {
            provider: "gcp".into(),
            profile: t.profile.clone().or(account).unwrap_or_default(),
            region,
            zone,
            project,
            ttl_hours: model::DEFAULT_TTL_HOURS,
            ..Default::default()
        })
    }

    fn kinds(&self, _: &Connection) -> Vec<CloudKind> {
        let mut v: Vec<CloudKind> = OFFERS
            .iter()
            .map(|(image, mt, usd)| CloudKind {
                image: (*image).into(),
                kind: "container".into(),
                supported: true,
                reason: "a VM runs the image with Docker".into(),
                machine_type: (*mt).into(),
                usd_per_hour: *usd,
            })
            .collect();
        for image in ["windows", "omarchy"] {
            v.push(CloudKind {
                image: image.into(),
                kind: "vm".into(),
                supported: false,
                reason: "VM images are not offered in your own cloud yet (they need nested \
                         virtualization, N2, and QEMU on the VM)"
                    .into(),
                ..Default::default()
            });
        }
        v.push(CloudKind {
            image: "macos".into(),
            kind: "vm".into(),
            supported: false,
            reason: "macOS runs only on Apple hardware; Google Cloud has none".into(),
            ..Default::default()
        });
        v
    }

    async fn test(&self, conn: &Connection) -> Tested {
        let mut t = Tested::default();
        let who = if conn.profile.is_empty() {
            "the active gcloud account".to_string()
        } else {
            format!("gcloud account {}", conn.profile)
        };
        if let Err(e) = self.token(conn).await {
            t.check("credentials", false, e.to_string());
            return t;
        }
        t.check("credentials", true, who);
        match self
            .get(conn, &self.project_url(conn), "read the project")
            .await
        {
            Ok(Some(_)) => {
                // The project id people know (Compute's own numeric id for
                // the project is not the project number they see).
                t.account = conn.project.clone();
                t.check(
                    "project",
                    true,
                    format!("{}, Compute Engine enabled", conn.project),
                );
            }
            Ok(None) => {
                t.check(
                    "project",
                    false,
                    format!("project {} not found", conn.project),
                );
                return t;
            }
            Err(e) => {
                t.check(
                    "project",
                    false,
                    format!("{e} (enable Compute Engine: `gcloud services enable compute.googleapis.com --project {}`)", conn.project),
                );
                return t;
            }
        }
        let url = format!(
            "{}/projects/{}:testIamPermissions",
            self.resource_manager, conn.project
        );
        let body = json!({"permissions": PERMISSIONS});
        match self
            .call(
                conn,
                reqwest::Method::POST,
                &url,
                Some(&body),
                "check permissions",
            )
            .await
        {
            Ok(Some(v)) => {
                let have: Vec<&str> = v["permissions"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let missing: Vec<&str> = PERMISSIONS
                    .iter()
                    .copied()
                    .filter(|p| !have.contains(p))
                    .collect();
                if missing.is_empty() {
                    t.check(
                        "permissions",
                        true,
                        format!("{} Compute Engine permissions", PERMISSIONS.len()),
                    );
                } else {
                    t.check(
                        "permissions",
                        false,
                        format!("missing: {}", missing.join(", ")),
                    );
                }
            }
            Ok(None) => t.check("permissions", false, "project not found"),
            Err(e) => t.check("permissions", false, e.to_string()),
        }
        let family = format!(
            "{}/projects/ubuntu-os-cloud/global/images/family/ubuntu-2404-lts-amd64",
            self.compute
        );
        match self.get(conn, &family, "find the Ubuntu 24.04 image").await {
            Ok(Some(i)) => t.check(
                "image",
                true,
                i["name"].as_str().unwrap_or("ubuntu-2404-lts").to_string(),
            ),
            Ok(None) => t.check(
                "image",
                false,
                "Ubuntu 24.04 LTS (ubuntu-2404-lts-amd64) not found",
            ),
            Err(e) => t.check("image", false, e.to_string()),
        }
        let mut types = Vec::new();
        for (_, mt, _) in OFFERS {
            let url = format!("{}/machineTypes/{mt}", self.zone_url(conn));
            match self.get(conn, &url, "look up the machine type").await {
                Ok(Some(_)) => types.push(*mt),
                Ok(None) => {
                    t.check(
                        "region",
                        false,
                        format!("{mt} is not offered in {}", conn.zone),
                    );
                    return t;
                }
                Err(e) => {
                    t.check("region", false, e.to_string());
                    return t;
                }
            }
        }
        t.check(
            "region",
            true,
            format!("{} in {}", types.join(", "), conn.zone),
        );
        let region = format!("{}/regions/{}", self.project_url(conn), conn.region);
        match self.get(conn, &region, "read the region's quotas").await {
            Ok(Some(r)) => {
                let quota = |m: &str| {
                    r["quotas"].as_array().and_then(|q| {
                        q.iter().find(|x| x["metric"] == m).map(|x| {
                            (
                                x["limit"].as_f64().unwrap_or(0.0),
                                x["usage"].as_f64().unwrap_or(0.0),
                            )
                        })
                    })
                };
                let cpus = quota("CPUS");
                let ips = quota("IN_USE_ADDRESSES");
                let ok = cpus.is_some_and(|(l, u)| l - u >= 2.0)
                    && ips.is_some_and(|(l, u)| l - u >= 1.0);
                let fmt = |q: Option<(f64, f64)>| {
                    q.map_or("unknown".to_string(), |(l, u)| format!("{u}/{l}"))
                };
                t.check(
                    "quota",
                    ok,
                    format!(
                        "vCPUs {} and external IPs {} in use in {}",
                        fmt(cpus),
                        fmt(ips),
                        conn.region
                    ),
                );
            }
            Ok(None) => t.check("quota", false, format!("region {} not found", conn.region)),
            Err(e) => t.check("quota", false, e.to_string()),
        }
        t
    }

    async fn provision(&self, conn: &Connection, spec: &ProvisionSpec) -> Result<Vec<Resource>> {
        let owner = spec
            .tags
            .get(model::tag::OWNER)
            .cloned()
            .ok_or_else(|| Error::InvalidArgument("gcp: the create has no owner tag".into()))?;
        let network = self.ensure_network(conn, &owner).await?;
        let arch = if spec.arch == "arm64" {
            "arm64"
        } else {
            "amd64"
        };
        let mut scheduling = json!({"automaticRestart": true, "onHostMaintenance": "MIGRATE"});
        if spec.ttl_secs > 0 {
            scheduling = json!({
                "automaticRestart": true,
                "onHostMaintenance": "MIGRATE",
                "maxRunDuration": {"seconds": spec.ttl_secs.to_string()},
                "instanceTerminationAction": "DELETE",
            });
        }
        let body = json!({
            "name": spec.name,
            "description": "Cua sandbox",
            "machineType": format!("zones/{}/machineTypes/{}", conn.zone, spec.machine_type),
            "labels": spec.tags,
            "disks": [{
                "boot": true,
                "autoDelete": true,
                "initializeParams": {
                    "sourceImage": format!("projects/ubuntu-os-cloud/global/images/family/ubuntu-2404-lts-{arch}"),
                    "diskSizeGb": spec.disk_gb.max(10).to_string(),
                    "diskType": format!("zones/{}/diskTypes/pd-balanced", conn.zone),
                    "labels": spec.tags,
                },
            }],
            "networkInterfaces": [{
                "network": format!("global/networks/{}", network.id),
                "accessConfigs": [{"type": "ONE_TO_ONE_NAT", "name": "External NAT"}],
            }],
            "metadata": {"items": [{"key": "user-data", "value": spec.user_data}]},
            // No service account: nothing on the VM can call Google APIs.
            "serviceAccounts": [],
            "scheduling": scheduling,
            "shieldedInstanceConfig": {"enableSecureBoot": true, "enableVtpm": true, "enableIntegrityMonitoring": true},
        });
        let op = self
            .call(
                conn,
                reqwest::Method::POST,
                &format!("{}/instances", self.zone_url(conn)),
                Some(&body),
                "create the instance",
            )
            .await?
            .unwrap_or(Value::Null);
        if let Err(e) = self.wait(conn, &op, "create the instance").await {
            // A half-made instance is deleted (it carries our labels).
            let probe = Resource {
                id: spec.name.clone(),
                resource_type: "instance".into(),
                ..Default::default()
            };
            if let Ok(Some((_, tags))) = self.describe(conn, &probe).await
                && model::is_ours(&tags, &owner)
            {
                let _ = self
                    .call(
                        conn,
                        reqwest::Method::DELETE,
                        &self.instance_url(conn, &spec.name),
                        None,
                        "delete the failed instance",
                    )
                    .await;
            }
            return Err(e);
        }
        let mut extra = BTreeMap::new();
        extra.insert("network".to_string(), network.id.clone());
        Ok(vec![
            Resource {
                provider: "gcp".into(),
                id: spec.name.clone(),
                resource_type: "instance".into(),
                name: spec.name.clone(),
                region: conn.region.clone(),
                zone: conn.zone.clone(),
                project: conn.project.clone(),
                created: model::now(),
                expires: model::tag_expires(&spec.tags),
                tags: spec.tags.clone(),
                state: "pending".into(),
                extra,
                ..Default::default()
            },
            network,
        ])
    }

    async fn describe(
        &self,
        conn: &Connection,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        if r.id.is_empty() {
            return Ok(None);
        }
        match r.resource_type.as_str() {
            "network" => Ok(self
                .get(conn, &self.network_url(conn, &r.id), "read the network")
                .await?
                .map(|n| {
                    (
                        "ready".to_string(),
                        description_tags(n["description"].as_str().unwrap_or("")),
                    )
                })),
            _ => Ok(self
                .get(conn, &self.instance_url(conn, &r.id), "read the instance")
                .await?
                .map(|i| (state_of(i["status"].as_str().unwrap_or("")), labels_of(&i)))),
        }
    }

    async fn stop(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.instance_op(conn, r, owner, "stop").await
    }

    async fn start(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.instance_op(conn, r, owner, "start").await
    }

    async fn delete(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        if !self.guard(conn, r, owner, "delete").await? {
            return Ok(());
        }
        let url = match r.resource_type.as_str() {
            "network" => self.network_url(conn, &r.id),
            _ => self.instance_url(conn, &r.id),
        };
        let what = format!("delete {} {}", r.resource_type, r.id);
        match self
            .call(conn, reqwest::Method::DELETE, &url, None, &what)
            .await?
        {
            Some(op) => self.wait(conn, &op, &what).await,
            None => Ok(()),
        }
    }

    async fn list_owned(&self, conn: &Connection, owner: &str) -> Result<Vec<Resource>> {
        let filter = format!(
            "labels.{}=\"true\" AND labels.{}=\"{}\"",
            model::tag::MANAGED,
            model::tag::OWNER,
            model::label_value(owner)
        );
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut url = reqwest::Url::parse(&format!("{}/instances", self.zone_url(conn)))
                .map_err(|e| cloud_err("gcp", "list instances", e))?;
            url.query_pairs_mut()
                .append_pair("filter", &filter)
                .append_pair("maxResults", "500");
            if let Some(p) = &page {
                url.query_pairs_mut().append_pair("pageToken", p);
            }
            let v = self
                .get(conn, url.as_str(), "list instances")
                .await?
                .unwrap_or(Value::Null);
            for i in v["items"].as_array().into_iter().flatten() {
                let tags = labels_of(i);
                // Checked again here: the server filter is not trusted alone.
                if !model::is_ours(&tags, owner) {
                    continue;
                }
                let name = i["name"].as_str().unwrap_or_default().to_string();
                out.push(Resource {
                    provider: "gcp".into(),
                    id: name.clone(),
                    resource_type: "instance".into(),
                    name,
                    region: conn.region.clone(),
                    zone: conn.zone.clone(),
                    project: conn.project.clone(),
                    created: i["creationTimestamp"]
                        .as_str()
                        .and_then(|t| humantime::parse_rfc3339_weak(&t[..19.min(t.len())]).ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    expires: model::tag_expires(&tags),
                    tags,
                    state: state_of(i["status"].as_str().unwrap_or("")),
                    ..Default::default()
                });
            }
            page = v["nextPageToken"].as_str().map(str::to_string);
            if page.is_none() {
                break;
            }
        }
        let name = Self::network_name(owner);
        if let Some(n) = self
            .get(conn, &self.network_url(conn, &name), "read the network")
            .await?
        {
            let tags = description_tags(n["description"].as_str().unwrap_or(""));
            if model::is_ours(&tags, owner) {
                out.push(Resource {
                    provider: "gcp".into(),
                    id: name.clone(),
                    resource_type: "network".into(),
                    name,
                    project: conn.project.clone(),
                    tags,
                    state: "ready".into(),
                    ..Default::default()
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_tags_round_trip_and_states() {
        let tags = network_tags("0123abcd");
        let d = describe_tags(&tags);
        assert!(d.starts_with("Cua sandboxes"));
        assert_eq!(description_tags(&d), tags);
        assert!(model::is_ours(&description_tags(&d), "0123abcd"));
        assert!(!model::is_ours(&description_tags("a network"), "0123abcd"));
        assert_eq!(state_of("TERMINATED"), "stopped");
        assert_eq!(state_of("STAGING"), "pending");
        assert_eq!(Gcp::network_name("0123abcd"), "cua-sb-0123abcd");
    }

    #[test]
    fn gcloud_config_is_read_without_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("configurations")).unwrap();
        std::fs::write(dir.path().join("active_config"), "work\n").unwrap();
        std::fs::write(
            dir.path().join("configurations/config_work"),
            "[core]\naccount = ada@example.com\nproject = p-1\n",
        )
        .unwrap();
        let g = Gcp::with_endpoints(
            "http://x",
            "http://y",
            Arc::new(Gcloud),
            Some(dir.path().into()),
        );
        let c = g.detect();
        assert!(c.found);
        assert_eq!(c.source, "gcloud account ada@example.com, project p-1");
        let conn = g
            .resolve(&Target {
                provider: "gcp".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            (
                conn.project.as_str(),
                conn.region.as_str(),
                conn.zone.as_str()
            ),
            ("p-1", "us-central1", "us-central1-a")
        );
        assert_eq!(conn.profile, "ada@example.com");
        let z = g
            .resolve(&Target {
                provider: "gcp".into(),
                zone: Some("europe-west4-b".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(z.region, "europe-west4");
        let none = Gcp::with_endpoints("http://x", "http://y", Arc::new(Gcloud), None);
        assert!(!none.detect().found);
        assert!(
            none.resolve(&Target {
                provider: "gcp".into(),
                ..Default::default()
            })
            .is_err()
        );
    }
}
