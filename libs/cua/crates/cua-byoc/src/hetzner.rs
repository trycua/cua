// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Hetzner Cloud (`--on hetzner`): one small server per sandbox, over the
//! Hetzner Cloud REST API (v1) with a project API token. Docker runs the
//! image on the server; the guest dials out to the relay like every other
//! cloud sandbox, so nothing needs an inbound port.
//!
//! The token is the only credential. It lives in the Cua credential store
//! (the OS vault on a Cua-signed Mac, a 0600 file in the cua home elsewhere;
//! never in `config.toml` or the connection record), or is read from
//! `HCLOUD_TOKEN` or the `hcloud` CLI's active context where it already is.
//! `cua cloud connect hetzner` saves a token it was given only after the
//! checks passed. A connection whose token is sealed to the account's
//! devices ([`crate::model::Auth::Sealed`]) gets it from
//! [`crate::api::SecretSource`] instead, opened in memory only when a call
//! needs it.
//!
//! What a sandbox creates, all labelled with the sandbox's tags:
//!
//! - one server: the cheapest shared-vCPU type of the location that fits
//!   (x86 `cx23` where it is offered, `cax11` for an Arm image), from the
//!   Hetzner `docker-ce` app image (Ubuntu 24.04 with Docker), with an
//!   IPv4 address for egress (no IPv6: it would use a second primary IP of the
//!   project's small limit). No SSH key is added;
//! - once per cua home and project, a dedicated firewall `cua-sb-<owner>`
//!   with **no rules**: Hetzner drops all inbound traffic and allows all
//!   outbound (the guest only dials out). The project's other firewalls are
//!   never used or changed.
//!
//! Honest limits, shown by `cua cloud status`:
//!
//! - Hetzner Cloud servers have no nested virtualization (no `/dev/kvm`),
//!   so only container sandboxes run; VM images (Windows, Omarchy) are
//!   refused with that reason.
//! - A stopped server is still billed (Hetzner bills the reserved
//!   resources), so `stop` keeps the disk but does not stop the charge.
//! - Hetzner has no instance-initiated delete. The server shuts itself
//!   down at its time limit, and `cua cloud sweep` (or any later create)
//!   deletes it; until then it is billed.
//! - The API allows about 3600 requests per hour per project; calls back
//!   off when it answers 429.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{CloudCredentials, CloudKind};
use serde_json::{Value, json};

use crate::api::{CloudApi, Result, Secret, SecretSource, Target, Tested, cloud_err};
use crate::model::{
    self, Billing, Capabilities, Connection, HostType, ProvisionSpec, Resource, Supplies, Tier,
};

/// The Hetzner Cloud API root.
pub const API: &str = "https://api.hetzner.cloud/v1";

/// The default location (every shared type, the lowest prices).
pub const DEFAULT_LOCATION: &str = "fsn1";

/// Every Hetzner Cloud location.
pub const LOCATIONS: &[&str] = &["fsn1", "nbg1", "hel1", "ash", "hil", "sin"];

/// The name of the token in the Cua credential store.
pub const SECRET_NAME: &str = "byoc-hetzner-token";

/// The image servers boot: Ubuntu 24.04 with Docker installed.
pub const DOCKER_IMAGE: &str = "docker-ce";

/// What every Hetzner server's IPv4 address costs per hour.
pub const IPV4_USD_PER_HOUR: f64 = 0.001;

/// Typical seconds from the create call to a joined host (the core learns
/// the real value per type and location and overrides this).
pub const BOOT_SECS: u32 = 150;

/// The default most servers a Hetzner Cloud project may have. New projects
/// start at 5 primary IPv4 addresses (each server takes one; an IPv6 address
/// would take another from the same limit, so servers get IPv4 only); Hetzner
/// raises the limit on request, and the create's own limit error is the
/// authority.
pub const DEFAULT_MAX_SERVERS: u32 = 5;

/// What `cua cloud status` shows before a create picks the real cheapest
/// type from the API: `cx23` in `fsn1` (US dollars per hour: server price
/// 0.0104 plus the IPv4 address 0.001).
const ESTIMATE: (&str, f64) = ("cx23", 0.0104 + 0.001);

/// Where the Cua credential store keeps the token (and a fake in tests).
pub trait TokenStore: Send + Sync + 'static {
    /// The saved token, if any.
    fn load(&self) -> Option<String>;
    /// Saves the token (0600 or the OS vault).
    fn save(&self, token: &str) -> std::io::Result<()>;
    /// Removes it; returns whether there was one.
    fn clear(&self) -> bool;
}

/// The Cua credential store ([`cua_auth::Store`]).
pub struct CuaCredentialStore;

impl TokenStore for CuaCredentialStore {
    fn load(&self) -> Option<String> {
        let bytes = cua_auth::Store::from_env()
            .load_secret(SECRET_NAME)
            .ok()??;
        String::from_utf8(bytes)
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    fn save(&self, token: &str) -> std::io::Result<()> {
        cua_auth::Store::from_env()
            .save_secret(SECRET_NAME, token.as_bytes())
            .map_err(std::io::Error::other)
    }

    fn clear(&self) -> bool {
        cua_auth::Store::from_env()
            .clear_secret(SECRET_NAME)
            .unwrap_or(false)
    }
}

/// Where a token came from (never the token itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Given,
    Saved,
    Env,
    Hcloud,
    Sealed,
}

impl Source {
    fn describe(self) -> &'static str {
        match self {
            Source::Given => "the token you gave",
            Source::Saved => "the token saved by `cua cloud connect hetzner`",
            Source::Env => "HCLOUD_TOKEN",
            Source::Hcloud => "the hcloud CLI's active context",
            Source::Sealed => "the account's token, sealed to your devices",
        }
    }
}

/// Hetzner Cloud.
pub struct Hetzner {
    http: reqwest::Client,
    api: String,
    store: Arc<dyn TokenStore>,
    /// A token from a connect or test in flight (not saved yet).
    staged: Mutex<Option<Secret>>,
    hcloud_config: Option<std::path::PathBuf>,
    poll: Duration,
    /// Held while the shared firewall is found or created.
    firewall_lock: tokio::sync::Mutex<()>,
    /// Sealed connections' tokens.
    secrets: Mutex<Option<Arc<dyn SecretSource>>>,
}

impl Default for Hetzner {
    fn default() -> Self {
        Self::new()
    }
}

impl Hetzner {
    /// The real API with the Cua credential store.
    pub fn new() -> Self {
        Self::with_endpoint(API, Arc::new(CuaCredentialStore), hcloud_config_path())
    }

    /// Another API root and token store (a local fake in tests), and the
    /// `hcloud` CLI config file detection reads (`None`: none).
    pub fn with_endpoint(
        api: &str,
        store: Arc<dyn TokenStore>,
        hcloud_config: Option<std::path::PathBuf>,
    ) -> Self {
        Hetzner {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("http client"),
            api: api.trim_end_matches('/').into(),
            store,
            staged: Mutex::new(None),
            hcloud_config,
            poll: Duration::from_secs(2),
            firewall_lock: tokio::sync::Mutex::new(()),
            secrets: Mutex::new(None),
        }
    }

    /// How often waits poll (tests make it short).
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// The token on this device: the one given now, the saved one, the
    /// environment's or the hcloud CLI's (not a sealed one).
    fn token_and_source(&self) -> Option<(String, Source)> {
        if let Some(t) = self.staged.lock().unwrap().clone() {
            return Some((t.expose().to_string(), Source::Given));
        }
        self.saved_token()
    }

    fn saved_token(&self) -> Option<(String, Source)> {
        if let Some(t) = self.store.load() {
            return Some((t, Source::Saved));
        }
        if let Some(t) = std::env::var("HCLOUD_TOKEN")
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
        {
            return Some((t, Source::Env));
        }
        self.hcloud_config
            .as_deref()
            .and_then(hcloud_active_token)
            .map(|t| (t, Source::Hcloud))
    }

    fn sealed_source(&self) -> Option<Arc<dyn SecretSource>> {
        self.secrets.lock().unwrap().clone()
    }

    /// The token a call uses: the one given now (a connect's checks), the
    /// sealed one when the connection is sealed (and nothing else then),
    /// else this device's.
    async fn token_with_source(&self) -> Result<Option<(Secret, Source)>> {
        if let Some(t) = self.staged.lock().unwrap().clone() {
            return Ok(Some((t, Source::Given)));
        }
        if let Some(src) = self.sealed_source()
            && let Some(t) = src.secret("hetzner").await
        {
            return t.map(|t| Some((t, Source::Sealed)));
        }
        Ok(self.saved_token().map(|(t, s)| (Secret::new(t), s)))
    }

    async fn token(&self) -> Result<Secret> {
        self.token_with_source()
            .await?
            .map(|(t, _)| t)
            .ok_or_else(|| {
                Error::ContribNotConfigured(
                    "hetzner: no API token: run `cua cloud connect hetzner` with a Read & Write \
                 project token (HCLOUD_TOKEN, or --token-stdin)"
                        .into(),
                )
            })
    }

    /// One call: `Ok(None)` for a 404, Hetzner's own message otherwise.
    /// Backs off on 429 (the project's rate limit).
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        what: &str,
    ) -> Result<Option<Value>> {
        let (status, v) = self.raw(method, path, body, what).await?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(cloud_err("hetzner", what, error_message(status, &v)));
        }
        Ok(Some(v))
    }

    async fn raw(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        what: &str,
    ) -> Result<(reqwest::StatusCode, Value)> {
        let token = self.token().await?;
        let url = format!("{}{path}", self.api);
        let mut tries = 0;
        loop {
            let mut req = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(token.expose());
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| cloud_err("hetzner", what, e.without_url()))?;
            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS && tries < 6 {
                tries += 1;
                let wait = resp
                    .headers()
                    .get("ratelimit-reset")
                    .and_then(|h| h.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|reset| {
                        // Unix seconds, or seconds from now.
                        let now = model::now();
                        if reset > now { reset - now } else { reset }
                    })
                    .unwrap_or(2 * tries)
                    .clamp(1, 30);
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            return Ok((status, v));
        }
    }

    async fn get(&self, path: &str, what: &str) -> Result<Option<Value>> {
        self.call(reqwest::Method::GET, path, None, what).await
    }

    /// The dedicated firewall's name for `owner`.
    pub fn firewall_name(owner: &str) -> String {
        format!("cua-sb-{}", model::label_value(owner))
            .chars()
            .take(63)
            .collect()
    }

    async fn find_firewall(&self, name: &str) -> Result<Option<Value>> {
        Ok(self
            .get(
                &format!("/firewalls?name={name}"),
                "look up the Cua firewall",
            )
            .await?
            .and_then(|v| v["firewalls"].as_array().and_then(|a| a.first().cloned())))
    }

    /// The dedicated firewall: reused when it is this owner's, created
    /// (with no rules) when missing, refused when a firewall of that name
    /// is not Cua's.
    async fn ensure_firewall(&self, owner: &str) -> Result<Resource> {
        let name = Self::firewall_name(owner);
        let tags = firewall_tags(owner);
        // One caller at a time: concurrent first launches make it once.
        let _one = self.firewall_lock.lock().await;
        let existing = self.find_firewall(&name).await?;
        let id = match existing {
            Some(f) => {
                if !model::is_ours(&labels_of(&f), owner) {
                    return Err(cloud_err(
                        "hetzner",
                        "the Cua firewall",
                        format!(
                            "firewall {name} exists and is not labelled as this Cua owner's; \
                             it is left alone"
                        ),
                    ));
                }
                id_of(&f)
            }
            None => {
                let body = json!({"name": name, "labels": hlabels(&tags), "rules": []});
                let (status, v) = self
                    .raw(
                        reqwest::Method::POST,
                        "/firewalls",
                        Some(&body),
                        "create the Cua firewall",
                    )
                    .await?;
                if status.is_success() {
                    id_of(&v["firewall"])
                } else if status == reqwest::StatusCode::CONFLICT
                    || v["error"]["code"] == "uniqueness_error"
                {
                    // Made by another process between the look and the
                    // create: the same firewall, when it is Cua's.
                    match self.find_firewall(&name).await? {
                        Some(f) if model::is_ours(&labels_of(&f), owner) => id_of(&f),
                        _ => {
                            return Err(cloud_err(
                                "hetzner",
                                "create the Cua firewall",
                                error_message(status, &v),
                            ));
                        }
                    }
                } else {
                    return Err(cloud_err(
                        "hetzner",
                        "create the Cua firewall",
                        error_message(status, &v),
                    ));
                }
            }
        };
        Ok(Resource {
            provider: "hetzner".into(),
            id,
            resource_type: "firewall".into(),
            name,
            created: model::now(),
            tags,
            state: "ready".into(),
            ..Default::default()
        })
    }

    async fn guard(&self, r: &Resource, owner: &str, verb: &str) -> Result<bool> {
        match self.describe_any(r).await? {
            None => Ok(false),
            Some((_, tags)) if model::is_ours(&tags, owner) => Ok(true),
            Some(_) => Err(cloud_err(
                "hetzner",
                verb,
                format!(
                    "{} {} is not labelled as this Cua owner's; it is left alone",
                    r.resource_type, r.id
                ),
            )),
        }
    }

    async fn describe_any(
        &self,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        if r.id.is_empty() {
            return Ok(None);
        }
        match r.resource_type.as_str() {
            "firewall" => Ok(self
                .get(&format!("/firewalls/{}", r.id), "read the firewall")
                .await?
                .map(|v| ("ready".to_string(), labels_of(&v["firewall"])))),
            _ => Ok(self
                .get(&format!("/servers/{}", r.id), "read the server")
                .await?
                .map(|v| {
                    (
                        state_of(v["server"]["status"].as_str().unwrap_or("")),
                        labels_of(&v["server"]),
                    )
                })),
        }
    }

    /// Waits until the server is in `want` (or gone, for `None`).
    async fn wait_state(&self, id: &str, want: Option<&str>, secs: u64) -> Result<bool> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let now = self
                .get(&format!("/servers/{id}"), "read the server")
                .await?
                .map(|v| v["server"]["status"].as_str().unwrap_or("").to_string());
            if now.as_deref() == want {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Every server type Hetzner offers now with its live price and where it
    /// can be created right now (`available`), cheapest first. Deprecated
    /// types and types with no location left are not listed.
    async fn list_types(&self) -> Result<Vec<HostType>> {
        let v = self
            .get("/server_types?per_page=50", "list the server types")
            .await?
            .unwrap_or(Value::Null);
        Ok(host_types_of(&v))
    }
}

/// The host types in a `/server_types` answer: live prices (plus the IPv4
/// address every server carries), only where a type is available now, not
/// deprecated, cheapest first. No Hetzner Cloud server has `/dev/kvm`.
pub fn host_types_of(v: &Value) -> Vec<HostType> {
    let mut out: Vec<HostType> = v["server_types"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| !t["deprecated"].as_bool().unwrap_or(false))
        .filter_map(|t| {
            let name = t["name"].as_str()?.to_string();
            let regions: Vec<String> = t["locations"]
                .as_array()?
                .iter()
                .filter(|l| l["available"].as_bool().unwrap_or(false))
                .filter(|l| l["deprecation"].is_null())
                .filter_map(|l| l["name"].as_str().map(str::to_string))
                .collect();
            if regions.is_empty() {
                return None;
            }
            // The price is the lowest across the regions it is offered in.
            let price = t["prices"]
                .as_array()?
                .iter()
                .filter(|p| regions.iter().any(|r| p["location"] == r.as_str()))
                .filter_map(|p| p["price_hourly"]["gross"].as_str()?.parse::<f64>().ok())
                .fold(f64::INFINITY, f64::min);
            if !price.is_finite() {
                return None;
            }
            Some(HostType {
                name,
                arch: if t["architecture"] == "arm" {
                    "arm64"
                } else {
                    "amd64"
                }
                .into(),
                vcpus: t["cores"].as_u64()? as u32,
                memory_mb: (t["memory"].as_f64()? * 1024.0) as u64,
                disk_gb: t["disk"].as_u64().unwrap_or(0) as u32,
                kvm: false,
                gpu: String::new(),
                usd_per_hour: price + IPV4_USD_PER_HOUR,
                regions,
                boot_secs: BOOT_SECS,
                // Hetzner limits shared and dedicated cores separately.
                quota_pool: if t["cpu_type"] == "dedicated" {
                    "dedicated"
                } else {
                    "shared"
                }
                .into(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.usd_per_hour.total_cmp(&b.usd_per_hour));
    out
}

/// The tags of the dedicated firewall (it belongs to no one sandbox).
fn firewall_tags(owner: &str) -> BTreeMap<String, String> {
    model::tags(owner, "", "", 0)
        .into_iter()
        .filter(|(k, _)| k != model::tag::SPACE && k != model::tag::EXPIRES)
        .collect()
}

/// Hetzner label values start and end with a letter or digit and are at
/// most 63 characters; Cua's tag values are already lowercase.
pub fn hlabel_value(v: &str) -> String {
    v.trim_matches(|c: char| !c.is_ascii_alphanumeric()).into()
}

fn hlabels(tags: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(k, v)| (k.clone(), hlabel_value(v)))
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

fn id_of(v: &Value) -> String {
    match &v["id"] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// A Hetzner server status as a provider state.
fn state_of(status: &str) -> String {
    match status {
        "running" | "migrating" => "running",
        "initializing" | "starting" | "rebuilding" => "pending",
        "stopping" | "deleting" => "stopping",
        "off" => "stopped",
        _ => "unknown",
    }
    .to_string()
}

/// Whether Hetzner's answer to a create says the type is out of stock or
/// not offered in that location (nothing was created).
fn is_unavailable(m: &str) -> bool {
    let m = m.to_ascii_lowercase();
    [
        "unavailable",
        "not available",
        "placement",
        "unsupported location",
        "sold out",
        "resource_unavailable",
    ]
    .iter()
    .any(|k| m.contains(k))
}

fn error_message(status: reqwest::StatusCode, v: &Value) -> String {
    let code = v["error"]["code"].as_str().unwrap_or_default();
    let msg = v["error"]["message"].as_str().unwrap_or_default();
    let hint = match code {
        "unauthorized" => {
            " (the token was not accepted: create a Read & Write token in the \
                          Hetzner Cloud Console, Security, API tokens)"
        }
        "resource_limit_exceeded" => {
            " (your project's server limit: ask Hetzner to raise it, \
                                     or delete servers)"
        }
        "forbidden" => " (the token is read-only: create a Read & Write token)",
        _ => "",
    };
    if msg.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("{msg}{hint}")
    }
}

/// The `hcloud` CLI's config file (`$HCLOUD_CONFIG`, else
/// `~/.config/hcloud/cli.toml`).
fn hcloud_config_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("HCLOUD_CONFIG").filter(|p| !p.is_empty()) {
        return Some(p.into());
    }
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config/hcloud/cli.toml"))
}

/// The token of the `hcloud` CLI's active context, read where it is (a
/// minimal reader of the two keys; never logged).
fn hcloud_active_token(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let unq = |s: &str| s.trim().trim_matches('"').trim_matches('\'').to_string();
    let mut active = String::new();
    let (mut name, mut token) = (String::new(), String::new());
    let mut contexts: Vec<(String, String)> = Vec::new();
    let mut in_ctx = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("[[") {
            if in_ctx {
                contexts.push((std::mem::take(&mut name), std::mem::take(&mut token)));
            }
            in_ctx = l == "[[contexts]]";
        } else if let Some((k, v)) = l.split_once('=') {
            match (in_ctx, k.trim()) {
                (false, "active_context") => active = unq(v),
                (true, "name") => name = unq(v),
                (true, "token") => token = unq(v),
                _ => {}
            }
        }
    }
    if in_ctx {
        contexts.push((name, token));
    }
    contexts
        .into_iter()
        .find(|(n, t)| *n == active && !t.is_empty())
        .map(|(_, t)| t)
}

#[async_trait]
impl CloudApi for Hetzner {
    fn name(&self) -> &'static str {
        "hetzner"
    }

    fn title(&self) -> &'static str {
        "Hetzner"
    }

    fn tier(&self) -> Tier {
        Tier::Vm
    }

    fn arches(&self) -> Vec<&'static str> {
        // x86 cost-optimized (cx) is cheaper than Arm (cax) at the same size.
        vec!["amd64", "arm64"]
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        // The connection's location first, then the rest.
        let mut regions = vec![conn.region.clone()];
        regions.extend(
            LOCATIONS
                .iter()
                .filter(|l| **l != conn.region)
                .map(|l| l.to_string()),
        );
        Capabilities {
            supplies: Supplies::Hosts,
            // Hetzner bills each started hour, and a powered-off server still.
            billing: Billing {
                granularity_secs: 3600,
                minimum_secs: 3600,
                stopped_bills: true,
            },
            regions,
            max_machines: DEFAULT_MAX_SERVERS,
            // Learned from a refused create (`shared core limit exceeded`).
            vcpu_quotas: Default::default(),
            // It cannot delete itself: the scaler or the sweeper does.
            self_deletes: false,
        }
    }

    async fn host_types(&self, _conn: &Connection) -> Result<Vec<HostType>> {
        self.list_types().await
    }

    fn detect(&self) -> CloudCredentials {
        if self.staged.lock().unwrap().is_none()
            && self.sealed_source().is_some_and(|s| s.sealed("hetzner"))
        {
            return CloudCredentials {
                found: true,
                source: Source::Sealed.describe().into(),
            };
        }
        match self.token_and_source() {
            Some((_, s)) => CloudCredentials {
                found: true,
                source: s.describe().into(),
            },
            None => CloudCredentials::default(),
        }
    }

    fn resolve(&self, t: &Target) -> Result<Connection> {
        let region = t
            .region
            .clone()
            .unwrap_or_else(|| DEFAULT_LOCATION.into())
            .to_ascii_lowercase();
        if !LOCATIONS.contains(&region.as_str()) {
            return Err(Error::InvalidArgument(format!(
                "hetzner: unknown location {region:?} (one of: {})",
                LOCATIONS.join(", ")
            )));
        }
        Ok(Connection {
            provider: "hetzner".into(),
            region,
            ttl_hours: model::DEFAULT_TTL_HOURS,
            ..Default::default()
        })
    }

    fn stage_secret(&self, t: &Target) {
        // Held for the checks; saved only when they pass.
        if let Some(tok) = t.token.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            *self.staged.lock().unwrap() = Some(Secret::new(tok));
        }
    }

    fn persist_secret(&self) -> Result<()> {
        // A token given now, or one that only the environment has, is
        // saved so the daemon (another process) finds it later.
        let staged = self.staged.lock().unwrap().take();
        let token = match (staged, self.token_and_source()) {
            (Some(t), _) => Some(t),
            (None, Some((t, Source::Env))) => Some(Secret::new(t)),
            _ => None,
        };
        if let Some(t) = token {
            self.store.save(t.expose()).map_err(Error::Io)?;
        }
        Ok(())
    }

    fn forget_secret(&self) {
        *self.staged.lock().unwrap() = None;
        self.store.clear();
    }

    fn seals(&self) -> bool {
        true
    }

    fn unstage_secret(&self) {
        *self.staged.lock().unwrap() = None;
    }

    fn local_secret(&self, t: &Target) -> Option<Secret> {
        t.token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Secret::new)
            .or_else(|| self.token_and_source().map(|(t, _)| Secret::new(t)))
    }

    fn attach_secrets(&self, source: Arc<dyn SecretSource>) {
        *self.secrets.lock().unwrap() = Some(source);
    }

    fn kinds(&self, _: &Connection) -> Vec<CloudKind> {
        let mut v: Vec<CloudKind> = ["linux", "linux-slim"]
            .iter()
            .map(|image| CloudKind {
                image: (*image).into(),
                kind: "container".into(),
                supported: true,
                reason: "a server runs the image with Docker".into(),
                machine_type: ESTIMATE.0.into(),
                usd_per_hour: ESTIMATE.1,
            })
            .collect();
        for image in ["windows", "omarchy"] {
            v.push(CloudKind {
                image: image.into(),
                kind: "vm".into(),
                supported: false,
                reason: "Hetzner Cloud servers have no nested virtualization (/dev/kvm), so VM \
                         images cannot run there"
                    .into(),
                ..Default::default()
            });
        }
        v.push(CloudKind {
            image: "macos".into(),
            kind: "vm".into(),
            supported: false,
            reason: "macOS runs only on Apple hardware; Hetzner Cloud has none".into(),
            ..Default::default()
        });
        v
    }

    async fn test(&self, conn: &Connection) -> Tested {
        let mut t = Tested::default();
        let source = match self.token_with_source().await {
            Ok(s) => s.map(|(_, s)| s),
            Err(e) => {
                t.check("credentials", false, e.to_string());
                return t;
            }
        };
        let Some(source) = source else {
            t.check(
                "credentials",
                false,
                "no Hetzner API token (pass one with --token-stdin or set HCLOUD_TOKEN; create it \
                 in the Hetzner Cloud Console, Security, API tokens, Read & Write)",
            );
            return t;
        };
        // Read: lists servers (and says how many the project already has).
        let existing = match self.get("/servers?per_page=1", "read the project").await {
            Ok(Some(v)) => v["meta"]["pagination"]["total_entries"]
                .as_u64()
                .unwrap_or(0),
            Ok(None) => 0,
            Err(e) => {
                t.check("credentials", false, e.to_string());
                return t;
            }
        };
        t.account = "Hetzner Cloud project".into();
        t.check(
            "credentials",
            true,
            format!(
                "{}; project reachable, {existing} servers already there",
                source.describe()
            ),
        );
        // Write: an empty create is rejected after the permission check,
        // so 422 means a Read & Write token and 403 a read-only one, and
        // nothing is created either way.
        match self
            .raw(
                reqwest::Method::POST,
                "/servers",
                Some(&json!({})),
                "check permissions",
            )
            .await
        {
            Ok((s, _)) if s == reqwest::StatusCode::FORBIDDEN => t.check(
                "permissions",
                false,
                "the token is read-only: create a Read & Write token",
            ),
            Ok((s, _))
                if s == reqwest::StatusCode::UNPROCESSABLE_ENTITY
                    || s == reqwest::StatusCode::BAD_REQUEST =>
            {
                t.check("permissions", true, "Read & Write (servers and firewalls)")
            }
            Ok((s, v)) => t.check("permissions", false, error_message(s, &v)),
            Err(e) => t.check("permissions", false, e.to_string()),
        }
        match self.get("/locations", "list the locations").await {
            Ok(Some(v))
                if v["locations"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|l| l["name"] == conn.region.as_str()) =>
            {
                t.check("region", true, format!("location {}", conn.region))
            }
            Ok(_) => {
                t.check(
                    "region",
                    false,
                    format!("location {} not found", conn.region),
                );
                return t;
            }
            Err(e) => {
                t.check("region", false, e.to_string());
                return t;
            }
        }
        match self.list_types().await {
            Ok(types) => {
                let here: Vec<&HostType> = types
                    .iter()
                    .filter(|t| t.regions.contains(&conn.region))
                    .collect();
                let cheapest = |arch: &str| {
                    here.iter()
                        .find(|t| t.arch == arch)
                        .map(|t| format!("{arch}: {} ${:.4}/h", t.name, t.usd_per_hour))
                };
                let picked: Vec<String> = ["amd64", "arm64"]
                    .iter()
                    .filter_map(|a| cheapest(a))
                    .collect();
                if picked.is_empty() {
                    t.check(
                        "machine",
                        false,
                        format!("no server type is available in {} right now", conn.region),
                    );
                } else {
                    t.check(
                        "machine",
                        true,
                        format!(
                            "{} types available in {} (cheapest {})",
                            here.len(),
                            conn.region,
                            picked.join(", ")
                        ),
                    );
                }
            }
            Err(e) => {
                t.check("machine", false, e.to_string());
                return t;
            }
        }
        match self
            .get(
                &format!("/images?name={DOCKER_IMAGE}&type=app&per_page=1"),
                "find the Docker image",
            )
            .await
        {
            Ok(Some(v)) if v["images"].as_array().is_some_and(|a| !a.is_empty()) => t.check(
                "image",
                true,
                format!("{DOCKER_IMAGE} (Ubuntu 24.04 with Docker)"),
            ),
            Ok(_) => t.check("image", false, format!("image {DOCKER_IMAGE} not found")),
            Err(e) => t.check("image", false, e.to_string()),
        }
        t
    }

    async fn provision(&self, conn: &Connection, spec: &ProvisionSpec) -> Result<Vec<Resource>> {
        let owner =
            spec.tags.get(model::tag::OWNER).cloned().ok_or_else(|| {
                Error::InvalidArgument("hetzner: the create has no owner tag".into())
            })?;
        if spec.machine_type.is_empty() {
            return Err(Error::InvalidArgument(
                "hetzner: the create names no server type".into(),
            ));
        }
        let region = if spec.region.is_empty() {
            conn.region.clone()
        } else {
            spec.region.clone()
        };
        let firewall = self.ensure_firewall(&owner).await?;
        let want_arch = if spec.arch == "arm64" {
            "arm64"
        } else {
            "amd64"
        };
        // An elastic host gets exactly the type asked for. The older
        // one-server-per-Space path, which only names a rough size, may
        // take the next cheapest type of its architecture that is at least
        // that big when the first is sold out.
        let mut tries = vec![spec.machine_type.clone()];
        let exact = spec.role == "host";
        if !exact && let Ok(all) = self.list_types().await {
            let asked = all.iter().find(|t| t.name == spec.machine_type);
            let (vcpus, mem) = asked.map_or((2, 4096), |t| (t.vcpus, t.memory_mb));
            let fits = |t: &&HostType| {
                t.arch == want_arch
                    && t.vcpus >= vcpus
                    && t.memory_mb >= mem
                    && t.regions.contains(&region)
                    && t.name != spec.machine_type
            };
            let mut more: Vec<String> = all.iter().filter(fits).map(|t| t.name.clone()).collect();
            more.truncate(3);
            // The asked type first, but not when it is another architecture.
            if asked.is_some_and(|t| t.arch != want_arch) {
                tries.clear();
            }
            tries.extend(more);
        }
        let mut last: Option<Error> = None;
        let mut made = None;
        for ty in &tries {
            // Every label in this call, the rule-less firewall, no SSH key,
            // the user data as given.
            let body = json!({
                "name": spec.name,
                "server_type": ty,
                "image": DOCKER_IMAGE,
                "location": region,
                "labels": hlabels(&spec.tags),
                "user_data": spec.user_data,
                "start_after_create": true,
                "public_net": {"enable_ipv4": true, "enable_ipv6": false},
                "firewalls": [{"firewall": firewall.id.parse::<u64>().unwrap_or(0)}],
            });
            match self
                .call(
                    reqwest::Method::POST,
                    "/servers",
                    Some(&body),
                    "create the server",
                )
                .await
            {
                Ok(v) => {
                    made = Some((ty.clone(), v.unwrap_or(Value::Null)));
                    break;
                }
                Err(e) => {
                    let m = e.to_string();
                    // Out of stock, or not offered there: say which, so the
                    // caller can pick again. Nothing was created.
                    if is_unavailable(&m) {
                        last = Some(Error::Cloud(format!(
                            "hetzner: {ty} not available in {region}: {m}"
                        )));
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        let Some((ty, v)) = made else {
            return Err(last.unwrap_or_else(|| {
                Error::Cloud(format!(
                    "hetzner: {} not available in {region}",
                    spec.machine_type
                ))
            }));
        };
        let id = id_of(&v["server"]);
        if id.is_empty() {
            return Err(cloud_err("hetzner", "create the server", "no server id"));
        }
        let usd = self
            .list_types()
            .await
            .ok()
            .and_then(|t| t.into_iter().find(|h| h.name == ty))
            .map(|h| h.usd_per_hour);
        let mut extra = BTreeMap::new();
        extra.insert("machine_type".to_string(), ty.clone());
        if let Some(u) = usd {
            extra.insert("usd_per_hour".to_string(), format!("{u:.4}"));
        }
        extra.insert("firewall".to_string(), firewall.id.clone());
        if let Some(ip) = v["server"]["public_net"]["ipv4"]["ip"].as_str() {
            extra.insert("public_ipv4".to_string(), ip.to_string());
        }
        Ok(vec![
            Resource {
                provider: "hetzner".into(),
                id,
                resource_type: "instance".into(),
                name: spec.name.clone(),
                region,
                created: model::now(),
                expires: model::tag_expires(&spec.tags),
                tags: spec.tags.clone(),
                state: "pending".into(),
                extra,
                ..Default::default()
            },
            firewall,
        ])
    }

    async fn describe(
        &self,
        _conn: &Connection,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        self.describe_any(r).await
    }

    async fn stop(&self, _conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        if !self.guard(r, owner, "stop").await? {
            return Err(Error::NotFound(format!("hetzner server {}", r.id)));
        }
        let path = format!("/servers/{}/actions", r.id);
        self.call(
            reqwest::Method::POST,
            &format!("{path}/shutdown"),
            Some(&json!({})),
            "shut the server down",
        )
        .await?;
        if !self.wait_state(&r.id, Some("off"), 60).await? {
            self.call(
                reqwest::Method::POST,
                &format!("{path}/poweroff"),
                Some(&json!({})),
                "power the server off",
            )
            .await?;
            self.wait_state(&r.id, Some("off"), 60).await?;
        }
        Ok(())
    }

    async fn start(&self, _conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        if !self.guard(r, owner, "start").await? {
            return Err(Error::NotFound(format!("hetzner server {}", r.id)));
        }
        self.call(
            reqwest::Method::POST,
            &format!("/servers/{}/actions/poweron", r.id),
            Some(&json!({})),
            "start the server",
        )
        .await?;
        Ok(())
    }

    async fn delete(&self, _conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        if !self.guard(r, owner, "delete").await? {
            return Ok(());
        }
        let what = format!("delete {} {}", r.resource_type, r.id);
        match r.resource_type.as_str() {
            "firewall" => {
                // A firewall frees up a moment after the last server using
                // it is gone.
                let deadline = Instant::now() + Duration::from_secs(120);
                loop {
                    match self
                        .call(
                            reqwest::Method::DELETE,
                            &format!("/firewalls/{}", r.id),
                            None,
                            &what,
                        )
                        .await
                    {
                        Err(e) if e.to_string().contains("in use") && Instant::now() < deadline => {
                            tokio::time::sleep(self.poll.max(Duration::from_millis(500))).await;
                        }
                        other => {
                            other?;
                            break;
                        }
                    }
                }
            }
            _ => {
                self.call(
                    reqwest::Method::DELETE,
                    &format!("/servers/{}", r.id),
                    None,
                    &what,
                )
                .await?;
                // Deleting is asynchronous; a firewall can go only after.
                self.wait_state(&r.id, None, 60).await?;
            }
        }
        Ok(())
    }

    async fn list_owned(&self, _conn: &Connection, owner: &str) -> Result<Vec<Resource>> {
        let selector = format!(
            "{}=true,{}={}",
            model::tag::MANAGED,
            model::tag::OWNER,
            hlabel_value(&model::label_value(owner))
        );
        let mut out = Vec::new();
        for (kind, key) in [("servers", "servers"), ("firewalls", "firewalls")] {
            let mut page = 1u32;
            loop {
                let mut url = reqwest::Url::parse("http://x/")
                    .map_err(|e| cloud_err("hetzner", "list", e))?;
                url.query_pairs_mut()
                    .append_pair("label_selector", &selector)
                    .append_pair("per_page", "50")
                    .append_pair("page", &page.to_string());
                let v = self
                    .get(
                        &format!("/{kind}?{}", url.query().unwrap_or_default()),
                        &format!("list {kind}"),
                    )
                    .await?
                    .unwrap_or(Value::Null);
                for i in v[key].as_array().into_iter().flatten() {
                    let tags = labels_of(i);
                    // Checked again here: the server filter is not trusted alone.
                    if !model::is_ours(&tags, owner) {
                        continue;
                    }
                    let is_server = kind == "servers";
                    out.push(Resource {
                        provider: "hetzner".into(),
                        id: id_of(i),
                        resource_type: if is_server { "instance" } else { "firewall" }.into(),
                        name: i["name"].as_str().unwrap_or_default().to_string(),
                        region: i["location"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        created: i["created"]
                            .as_str()
                            .and_then(|t| humantime::parse_rfc3339_weak(&t[..19.min(t.len())]).ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                        expires: if is_server {
                            model::tag_expires(&tags)
                        } else {
                            0
                        },
                        tags,
                        state: if is_server {
                            state_of(i["status"].as_str().unwrap_or(""))
                        } else {
                            "ready".into()
                        },
                        ..Default::default()
                    });
                }
                match v["meta"]["pagination"]["next_page"].as_u64() {
                    Some(n) => page = n as u32,
                    None => break,
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_labels_and_names() {
        assert_eq!(state_of("off"), "stopped");
        assert_eq!(state_of("initializing"), "pending");
        assert_eq!(state_of("running"), "running");
        assert_eq!(hlabel_value("-abc_"), "abc");
        assert_eq!(Hetzner::firewall_name("0123abcd"), "cua-sb-0123abcd");
        let t = model::tags("test-owner", "space-1", "cloud-1", 100);
        let l = hlabels(&t);
        assert_eq!(l["cua-owner"], "test-owner");
        assert_eq!(l["cua-managed"], "true");
    }

    #[test]
    fn the_hcloud_context_is_read_without_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cli.toml");
        std::fs::write(
            &p,
            "active_context = \"work\"\n\n[[contexts]]\nname = \"home\"\ntoken = \"aaa\"\n\n\
             [[contexts]]\nname = \"work\"\ntoken = \"bbb\"\n",
        )
        .unwrap();
        assert_eq!(hcloud_active_token(&p).as_deref(), Some("bbb"));
        std::fs::write(&p, "active_context = \"none\"\n").unwrap();
        assert_eq!(hcloud_active_token(&p), None);
    }
}
