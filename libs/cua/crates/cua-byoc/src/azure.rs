// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Azure Virtual Machines (`--on azure`): one small Linux VM per sandbox,
//! over the Azure Resource Manager REST API (api-versions pinned below),
//! signed in through Cua's issuer only: Cua signs a short-lived token, the
//! `cua-spaces` managed identity's federated credential trusts it, and
//! Microsoft Entra exchanges it for an ARM token
//! ([`crate::federation::azure`]). No key is stored anywhere, and the
//! identity's roles reach one resource group only.
//!
//! What a sandbox creates, all in the connection's resource group and
//! tagged with the sandbox's tags (`cua-managed=true`, `cua-owner`, ...):
//!
//! - one Standard public IPv4 address (`<name>-ip`, for egress: the guest
//!   only dials out to the relay) and one network interface (`<name>-nic`);
//! - one VM (`Standard_B2s`, `Standard_B1ms` for the slim image) from
//!   Ubuntu 24.04 LTS, whose cloud-init (custom data) runs the image with
//!   Docker; no SSH key, a random password that is never stored, no managed
//!   identity on the VM (nothing on it can call Azure), boot diagnostics on
//!   (the serial log), its OS disk and interface deleted with it;
//! - once per cua home and resource group, a virtual network and a network
//!   security group `cua-sb-<owner>` with **no rules of its own**, so
//!   Azure's defaults deny every inbound connection from the internet.
//!
//! Azure cannot end a VM by itself at a deadline: the `cua-expires` tag
//! and the sweeper do (`cua cloud sweep --delete`, and the daemon every 15
//! minutes). Stop deallocates the VM, so its compute is not billed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{CloudCredentials, CloudKind};
use serde_json::{Value, json};

use crate::api::{CloudApi, Result, Target, Tested, cloud_err};
use crate::federation::{IssuerClient, Purpose};
use crate::model::{
    self, Auth, Billing, Capabilities, Connection, HostType, ProvisionSpec, Resource, Supplies,
    Tier,
};

/// Azure Resource Manager.
pub const ARM: &str = "https://management.azure.com";

/// `Microsoft.Compute` api-version.
pub const COMPUTE_API: &str = "2024-07-01";
/// `Microsoft.Network` api-version.
pub const NETWORK_API: &str = "2024-05-01";
/// `Microsoft.Resources` api-version.
pub const RESOURCES_API: &str = "2021-04-01";
/// `Microsoft.Authorization` api-version.
pub const AUTHORIZATION_API: &str = "2022-04-01";

/// The default region.
pub const DEFAULT_REGION: &str = "westus2";

/// The address space of the Cua network (private, the VMs' only network).
const ADDRESS_SPACE: &str = "10.42.0.0/16";
/// Its one subnet.
const SUBNET: &str = "cua";
const SUBNET_PREFIX: &str = "10.42.0.0/24";

/// What every machine carries besides compute: a 30 GiB Standard SSD
/// ($2.40 a month) and a Standard public IPv4 address ($0.005 an hour).
const EXTRAS_USD_PER_HOUR: f64 = 2.40 / 730.0 + 0.005;

/// Machine type per image family and its pay-as-you-go Linux price in
/// West US 2 (US dollars per hour) with [`EXTRAS_USD_PER_HOUR`].
const OFFERS: &[(&str, &str, f64)] = &[
    ("linux", "Standard_B2s", 0.0416 + EXTRAS_USD_PER_HOUR),
    ("linux-slim", "Standard_B1ms", 0.0207 + EXTRAS_USD_PER_HOUR),
];

/// What the roles must allow in the resource group (checked by `test`).
pub const ACTIONS: &[&str] = &[
    "Microsoft.Compute/virtualMachines/read",
    "Microsoft.Compute/virtualMachines/write",
    "Microsoft.Compute/virtualMachines/delete",
    "Microsoft.Compute/virtualMachines/start/action",
    "Microsoft.Compute/virtualMachines/deallocate/action",
    "Microsoft.Compute/virtualMachines/retrieveBootDiagnosticsData/action",
    "Microsoft.Network/virtualNetworks/write",
    "Microsoft.Network/virtualNetworks/subnets/join/action",
    "Microsoft.Network/networkSecurityGroups/write",
    "Microsoft.Network/networkSecurityGroups/join/action",
    "Microsoft.Network/publicIPAddresses/write",
    "Microsoft.Network/publicIPAddresses/join/action",
    "Microsoft.Network/networkInterfaces/write",
    "Microsoft.Network/networkInterfaces/join/action",
    "Microsoft.Resources/deployments/validate/action",
];

/// Azure Virtual Machines.
pub struct Azure {
    http: reqwest::Client,
    arm: String,
    login: String,
    issuer: std::sync::Mutex<Option<Arc<dyn IssuerClient>>>,
    /// One token exchange per connection, identity, purpose and agent.
    exchanges: std::sync::Mutex<BTreeMap<String, Arc<crate::federation::azure::Exchange>>>,
    /// Held while the shared network is found or created.
    network_lock: tokio::sync::Mutex<()>,
    /// The az CLI's configuration directory (detection only).
    config_dir: Option<PathBuf>,
    /// Between polls of a long-running operation.
    poll_every: Duration,
}

impl Default for Azure {
    fn default() -> Self {
        Self::new()
    }
}

impl Azure {
    /// The real API, through Cua's issuer.
    pub fn new() -> Self {
        Self::with_endpoints(
            ARM,
            crate::federation::azure::LOGIN_URL,
            None,
            az_config_dir(),
        )
    }

    /// Other roots for ARM and Entra sign-in, and an issuer (a local fake
    /// in tests; `None`: the environment's on first use), and the az CLI
    /// configuration directory detection reads (`None`: none).
    pub fn with_endpoints(
        arm: &str,
        login: &str,
        issuer: Option<Arc<dyn IssuerClient>>,
        config_dir: Option<PathBuf>,
    ) -> Self {
        let local = arm.starts_with("http://");
        Azure {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("http client"),
            arm: arm.trim_end_matches('/').into(),
            login: login.trim_end_matches('/').into(),
            issuer: std::sync::Mutex::new(issuer),
            exchanges: Default::default(),
            network_lock: tokio::sync::Mutex::new(()),
            config_dir,
            poll_every: if local {
                Duration::from_millis(10)
            } else {
                Duration::from_secs(3)
            },
        }
    }

    fn issuer(&self) -> Result<Arc<dyn IssuerClient>> {
        let mut slot = self.issuer.lock().expect("azure issuer");
        if let Some(i) = slot.as_ref() {
            return Ok(i.clone());
        }
        let i: Arc<dyn IssuerClient> =
            Arc::new(crate::federation::HttpIssuer::from_env().ok_or_else(|| {
                Error::Cloud(
                    "azure: Azure signs in through Cua, and this device has no Cua sign-in yet \
                     (run `cua auth login`)"
                        .into(),
                )
            })?);
        *slot = Some(i.clone());
        Ok(i)
    }

    fn exchange(
        &self,
        conn: &Connection,
        purpose: Purpose,
    ) -> Result<Arc<crate::federation::azure::Exchange>> {
        let Auth::Oidc {
            conn_id,
            tenant_id,
            client_id,
            ..
        } = &conn.auth
        else {
            return Err(Error::InvalidArgument(
                "azure: Azure is linked through Cua's issuer only; run `cua cloud connect azure`"
                    .into(),
            ));
        };
        if tenant_id.is_empty() || client_id.is_empty() {
            return Err(Error::InvalidArgument(
                "azure: this link names no tenant or managed identity; run `cua cloud connect \
                 azure --tenant <id> --client-id <id> --check` again"
                    .into(),
            ));
        }
        let agent = crate::federation::caller_agent();
        let key = format!(
            "{conn_id}|{tenant_id}|{client_id}|{}|{}",
            purpose.as_str(),
            agent.as_deref().unwrap_or_default()
        );
        let mut map = self.exchanges.lock().expect("azure exchanges");
        if let Some(x) = map.get(&key) {
            return Ok(x.clone());
        }
        let x = Arc::new(crate::federation::azure::Exchange::new(
            self.issuer()?,
            &self.login,
            conn_id,
            tenant_id,
            client_id,
            purpose,
            agent,
        ));
        map.insert(key, x.clone());
        Ok(x)
    }

    async fn token(&self, conn: &Connection, purpose: Purpose) -> Result<String> {
        self.exchange(conn, purpose)?
            .access_token()
            .await
            .map_err(|e| cloud_err("azure", "credentials", e))
    }

    /// `/subscriptions/<id>/resourceGroups/<group>`.
    fn group_path(conn: &Connection) -> String {
        format!(
            "/subscriptions/{}/resourceGroups/{}",
            conn.account, conn.project
        )
    }

    /// The ARM path of a resource of `kind` named `name` in the group.
    fn path(conn: &Connection, kind: &str, name: &str) -> String {
        let provider = match kind {
            "instance" => "Microsoft.Compute/virtualMachines",
            "network" => "Microsoft.Network/virtualNetworks",
            "security_group" => "Microsoft.Network/networkSecurityGroups",
            "nic" => "Microsoft.Network/networkInterfaces",
            "public_ip" => "Microsoft.Network/publicIPAddresses",
            other => other,
        };
        format!("{}/providers/{provider}/{name}", Self::group_path(conn))
    }

    fn api_of(kind: &str) -> &'static str {
        match kind {
            "instance" => COMPUTE_API,
            _ => NETWORK_API,
        }
    }

    fn url(&self, path: &str, api: &str) -> String {
        let sep = if path.contains('?') { '&' } else { '?' };
        format!("{}{path}{sep}api-version={api}", self.arm)
    }

    /// An absolute URL ARM handed back (an operation to poll), under this
    /// client's root (they differ only for a fake server).
    fn rebase(&self, link: &str) -> String {
        match link.find("/subscriptions/") {
            Some(i) if !link.starts_with(&self.arm) => format!("{}{}", self.arm, &link[i..]),
            _ => link.to_string(),
        }
    }

    /// One call: `Ok(None)` for a 404, the cloud's message otherwise. A
    /// long-running operation's poll URL comes back with the body.
    async fn call(
        &self,
        conn: &Connection,
        purpose: Purpose,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
        what: &str,
    ) -> Result<Option<Answer>> {
        let mut retried = false;
        let mut throttled = 0;
        let idempotent = matches!(
            method,
            reqwest::Method::GET | reqwest::Method::DELETE | reqwest::Method::PUT
        );
        let resp = loop {
            let token = self.token(conn, purpose).await?;
            let mut req = self.http.request(method.clone(), url).bearer_auth(token);
            match body {
                Some(b) => req = req.json(b),
                None if method == reqwest::Method::POST => {
                    req = req.header(reqwest::header::CONTENT_LENGTH, "0")
                }
                None => {}
            }
            let resp = req
                .send()
                .await
                .map_err(|e| cloud_err("azure", what, e.without_url()))?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && !retried {
                retried = true;
                self.exchange(conn, purpose)?.forget().await;
                continue;
            }
            if crate::api::retryable_status(resp.status().as_u16(), idempotent) {
                throttled += 1;
                if let Some(wait) = crate::api::retry_backoff(throttled) {
                    tokio::time::sleep(wait).await;
                    continue;
                }
            }
            break resp;
        };
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let header = |n: &str| {
            resp.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let poll = header("azure-asyncoperation").or_else(|| {
            (status == reqwest::StatusCode::ACCEPTED)
                .then(|| header("location"))
                .flatten()
        });
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(cloud_err("azure", what, arm_error(&v, status.as_u16())));
        }
        Ok(Some(Answer { body: v, poll }))
    }

    async fn get(
        &self,
        conn: &Connection,
        purpose: Purpose,
        url: &str,
        what: &str,
    ) -> Result<Option<Value>> {
        Ok(self
            .call(conn, purpose, reqwest::Method::GET, url, None, what)
            .await?
            .map(|a| a.body))
    }

    /// Waits for a long-running operation to finish, and fails with its
    /// error.
    async fn wait(
        &self,
        conn: &Connection,
        purpose: Purpose,
        answer: &Answer,
        what: &str,
    ) -> Result<()> {
        let Some(poll) = &answer.poll else {
            return Ok(());
        };
        let url = self.rebase(poll);
        let deadline = Instant::now() + Duration::from_secs(15 * 60);
        loop {
            match self
                .call(conn, purpose, reqwest::Method::GET, &url, None, what)
                .await?
            {
                // Gone: a delete that finished.
                None => return Ok(()),
                Some(a) => match a.body["status"].as_str() {
                    Some("Succeeded") => return Ok(()),
                    Some("Failed") | Some("Canceled") => {
                        return Err(cloud_err("azure", what, arm_error(&a.body, 200)));
                    }
                    Some(_) => {}
                    // A Location poll: 202 while running, then the result.
                    None if a.poll.is_none() => return Ok(()),
                    None => {}
                },
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "azure: {what}: the operation did not finish"
                )));
            }
            tokio::time::sleep(self.poll_every).await;
        }
    }

    /// PUT a resource and wait until it exists.
    async fn put(
        &self,
        conn: &Connection,
        purpose: Purpose,
        kind: &str,
        name: &str,
        body: &Value,
        what: &str,
    ) -> Result<Value> {
        let url = self.url(&Self::path(conn, kind, name), Self::api_of(kind));
        let a = self
            .call(conn, purpose, reqwest::Method::PUT, &url, Some(body), what)
            .await?
            .ok_or_else(|| {
                cloud_err(
                    "azure",
                    what,
                    format!(
                        "resource group {} not found in subscription {}",
                        conn.project, conn.account
                    ),
                )
            })?;
        self.wait(conn, purpose, &a, what).await?;
        Ok(a.body)
    }

    /// A resource of `kind` as the cloud has it now (`None`: gone).
    async fn read(
        &self,
        conn: &Connection,
        purpose: Purpose,
        kind: &str,
        name: &str,
    ) -> Result<Option<Value>> {
        let mut path = Self::path(conn, kind, name);
        if kind == "instance" {
            path.push_str("?$expand=instanceView");
        }
        self.get(
            conn,
            purpose,
            &self.url(&path, Self::api_of(kind)),
            &format!("read {kind} {name}"),
        )
        .await
    }

    /// The name of `owner`'s network and security group.
    fn network_name(owner: &str) -> String {
        format!("cua-sb-{}", model::label_value(owner))
            .chars()
            .take(64)
            .collect()
    }

    /// The shared security group and network: reused when they are this
    /// owner's, created (the group with no rules) when missing, refused
    /// when one of that name is not Cua's.
    async fn ensure_network(&self, conn: &Connection, owner: &str) -> Result<Vec<Resource>> {
        let name = Self::network_name(owner);
        let tags = network_tags(owner);
        let p = Purpose::Create;
        let _one = self.network_lock.lock().await;
        let mut out = Vec::new();
        for kind in ["security_group", "network"] {
            match self.read(conn, p, kind, &name).await? {
                Some(v) if model::is_ours(&tags_of(&v), owner) => {}
                Some(_) => {
                    return Err(cloud_err(
                        "azure",
                        "the Cua network",
                        format!(
                            "{kind} {name} exists and is not tagged as this Cua owner's; it is \
                             left alone"
                        ),
                    ));
                }
                None => {
                    let body = match kind {
                        "security_group" => json!({
                            "location": conn.region,
                            "tags": tags,
                            // No rules of its own: Azure's defaults deny
                            // every inbound connection from the internet.
                            "properties": {"securityRules": []},
                        }),
                        _ => json!({
                            "location": conn.region,
                            "tags": tags,
                            "properties": {
                                "addressSpace": {"addressPrefixes": [ADDRESS_SPACE]},
                                "subnets": [{
                                    "name": SUBNET,
                                    "properties": {
                                        "addressPrefix": SUBNET_PREFIX,
                                        "networkSecurityGroup": {
                                            "id": Self::path(conn, "security_group", &name),
                                        },
                                    },
                                }],
                            },
                        }),
                    };
                    self.put(
                        conn,
                        p,
                        kind,
                        &name,
                        &body,
                        &format!("create the Cua {kind}"),
                    )
                    .await?;
                }
            }
            out.push(Resource {
                provider: "azure".into(),
                id: name.clone(),
                resource_type: kind.into(),
                name: name.clone(),
                region: conn.region.clone(),
                project: conn.project.clone(),
                created: model::now(),
                tags: tags.clone(),
                state: "ready".into(),
                ..Default::default()
            });
        }
        // The network first: it goes before the group it uses.
        out.reverse();
        Ok(out)
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
                "azure",
                verb,
                format!(
                    "{} {} is not tagged as this Cua owner's; it is left alone",
                    r.resource_type, r.id
                ),
            )),
        }
    }

    async fn vm_action(
        &self,
        conn: &Connection,
        r: &Resource,
        owner: &str,
        op: &str,
    ) -> Result<()> {
        if !self.guard(conn, r, owner, op).await? {
            return Err(Error::NotFound(format!("azure vm {}", r.id)));
        }
        let url = self.url(
            &format!("{}/{op}", Self::path(conn, "instance", &r.id)),
            COMPUTE_API,
        );
        let what = format!("{op} vm {}", r.id);
        if let Some(a) = self
            .call(
                conn,
                Purpose::Manage,
                reqwest::Method::POST,
                &url,
                None,
                &what,
            )
            .await?
        {
            self.wait(conn, Purpose::Manage, &a, &what).await?;
        }
        Ok(())
    }

    /// Deletes one resource of `kind` (when it is there and `owner`'s).
    async fn delete_one(
        &self,
        conn: &Connection,
        purpose: Purpose,
        kind: &str,
        name: &str,
        owner: &str,
    ) -> Result<()> {
        let r = Resource {
            id: name.into(),
            resource_type: kind.into(),
            ..Default::default()
        };
        if !self.guard(conn, &r, owner, "delete").await? {
            return Ok(());
        }
        let url = self.url(&Self::path(conn, kind, name), Self::api_of(kind));
        let what = format!("delete {kind} {name}");
        if let Some(a) = self
            .call(conn, purpose, reqwest::Method::DELETE, &url, None, &what)
            .await?
        {
            self.wait(conn, purpose, &a, &what).await?;
        }
        Ok(())
    }

    /// A deployment template with one machine as Cua creates it, for the
    /// check's validate (nothing is created).
    fn check_template(conn: &Connection) -> Value {
        let (name, vm_size) = ("cua-check", OFFERS[0].1);
        json!({
            "$schema": "https://schema.management.azure.com/schemas/2019-04-01/deploymentTemplate.json#",
            "contentVersion": "1.0.0.0",
            "resources": [
                {
                    "type": "Microsoft.Network/networkSecurityGroups",
                    "apiVersion": NETWORK_API,
                    "name": name,
                    "location": conn.region,
                    "tags": {model::tag::MANAGED: "true"},
                    "properties": {"securityRules": []},
                },
                {
                    "type": "Microsoft.Network/virtualNetworks",
                    "apiVersion": NETWORK_API,
                    "name": name,
                    "location": conn.region,
                    "tags": {model::tag::MANAGED: "true"},
                    "dependsOn": [format!("[resourceId('Microsoft.Network/networkSecurityGroups', '{name}')]")],
                    "properties": {
                        "addressSpace": {"addressPrefixes": [ADDRESS_SPACE]},
                        "subnets": [{"name": SUBNET, "properties": {
                            "addressPrefix": SUBNET_PREFIX,
                            "networkSecurityGroup": {"id": format!("[resourceId('Microsoft.Network/networkSecurityGroups', '{name}')]")},
                        }}],
                    },
                },
                {
                    "type": "Microsoft.Network/publicIPAddresses",
                    "apiVersion": NETWORK_API,
                    "name": format!("{name}-ip"),
                    "location": conn.region,
                    "tags": {model::tag::MANAGED: "true"},
                    "sku": {"name": "Standard"},
                    "properties": {"publicIPAllocationMethod": "Static"},
                },
                {
                    "type": "Microsoft.Network/networkInterfaces",
                    "apiVersion": NETWORK_API,
                    "name": format!("{name}-nic"),
                    "location": conn.region,
                    "tags": {model::tag::MANAGED: "true"},
                    "dependsOn": [
                        format!("[resourceId('Microsoft.Network/virtualNetworks', '{name}')]"),
                        format!("[resourceId('Microsoft.Network/publicIPAddresses', '{name}-ip')]"),
                    ],
                    "properties": {"ipConfigurations": [{"name": "ipconfig1", "properties": {
                        "subnet": {"id": format!("[resourceId('Microsoft.Network/virtualNetworks/subnets', '{name}', '{SUBNET}')]")},
                        "publicIPAddress": {"id": format!("[resourceId('Microsoft.Network/publicIPAddresses', '{name}-ip')]")},
                    }}]},
                },
                {
                    "type": "Microsoft.Compute/virtualMachines",
                    "apiVersion": COMPUTE_API,
                    "name": name,
                    "location": conn.region,
                    "tags": {model::tag::MANAGED: "true"},
                    "dependsOn": [format!("[resourceId('Microsoft.Network/networkInterfaces', '{name}-nic')]")],
                    "properties": vm_properties(
                        vm_size,
                        "amd64",
                        30,
                        name,
                        "#cloud-config\n",
                        &format!("[resourceId('Microsoft.Network/networkInterfaces', '{name}-nic')]"),
                    ),
                },
            ],
        })
    }
}

/// An ARM answer: its body, and the URL that tells when a long-running
/// operation is done.
struct Answer {
    body: Value,
    poll: Option<String>,
}

/// ARM's error message (`{"error": {"code", "message"}}`).
fn arm_error(v: &Value, status: u16) -> String {
    let e = &v["error"];
    match (e["code"].as_str(), e["message"].as_str()) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        (None, Some(m)) => m.to_string(),
        (Some(c), None) => c.to_string(),
        _ => format!("HTTP {status}"),
    }
}

/// The tags of the shared network (it belongs to no one sandbox).
fn network_tags(owner: &str) -> BTreeMap<String, String> {
    model::tags(owner, "", "", 0)
        .into_iter()
        .filter(|(k, _)| k != model::tag::SPACE && k != model::tag::EXPIRES)
        .collect()
}

fn tags_of(v: &Value) -> BTreeMap<String, String> {
    v["tags"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// A VM's state from its instance view and provisioning state.
fn state_of(vm: &Value) -> String {
    let power = vm["properties"]["instanceView"]["statuses"]
        .as_array()
        .and_then(|s| {
            s.iter()
                .filter_map(|x| x["code"].as_str())
                .find_map(|c| c.strip_prefix("PowerState/"))
        });
    match power {
        Some("running") => "running",
        Some("starting") => "pending",
        Some("stopping") | Some("deallocating") => "stopping",
        Some("stopped") | Some("deallocated") => "stopped",
        _ => match vm["properties"]["provisioningState"].as_str() {
            Some("Creating") | Some("Updating") => "pending",
            Some("Deleting") => "stopping",
            Some("Succeeded") => "running",
            _ => "unknown",
        },
    }
    .to_string()
}

/// A random password that is never stored or shown: Azure needs one (or
/// an SSH key) for a Linux VM, and nothing can reach the VM to use it.
fn throwaway_password() -> String {
    use rand::Rng as _;
    const SET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::rng();
    let tail: String = (0..40)
        .map(|_| SET[rng.random_range(0..SET.len())] as char)
        .collect();
    // Upper, lower, digit and symbol, as Azure requires.
    format!("Cu4!{tail}")
}

/// The VM's properties: the size, Ubuntu 24.04 LTS, the custom data, the
/// interface, its disk and interface deleted with it.
fn vm_properties(
    size: &str,
    arch: &str,
    disk_gb: u32,
    computer: &str,
    user_data: &str,
    nic_id: &str,
) -> Value {
    json!({
        "hardwareProfile": {"vmSize": size},
        "storageProfile": {
            "imageReference": {
                "publisher": "Canonical",
                "offer": "ubuntu-24_04-lts",
                "sku": if arch == "arm64" { "server-arm64" } else { "server" },
                "version": "latest",
            },
            "osDisk": {
                "createOption": "FromImage",
                "deleteOption": "Delete",
                "diskSizeGB": disk_gb.max(30),
                "managedDisk": {"storageAccountType": "StandardSSD_LRS"},
            },
        },
        "osProfile": {
            "computerName": computer.chars().take(64).collect::<String>(),
            "adminUsername": "cua",
            "adminPassword": throwaway_password(),
            "customData": base64::engine::general_purpose::STANDARD.encode(user_data),
            "linuxConfiguration": {"disablePasswordAuthentication": false, "provisionVMAgent": true},
        },
        "networkProfile": {
            "networkInterfaces": [{"id": nic_id, "properties": {"primary": true, "deleteOption": "Delete"}}],
        },
        "diagnosticsProfile": {"bootDiagnostics": {"enabled": true}},
    })
}

/// Whether `action` is allowed by `pattern` (ARM's `*` wildcards, case
/// insensitive).
fn action_matches(pattern: &str, action: &str) -> bool {
    fn go(p: &[u8], a: &[u8]) -> bool {
        match p.split_first() {
            None => a.is_empty(),
            Some((b'*', rest)) => (0..=a.len()).any(|i| go(rest, &a[i..])),
            Some((c, rest)) => a
                .split_first()
                .is_some_and(|(x, ar)| x.eq_ignore_ascii_case(c) && go(rest, ar)),
        }
    }
    go(pattern.as_bytes(), action.as_bytes())
}

/// The actions of [`ACTIONS`] that `permissions` (an ARM permissions list)
/// does not allow.
fn missing_actions(permissions: &Value) -> Vec<&'static str> {
    let rows = permissions["value"].as_array().cloned().unwrap_or_default();
    ACTIONS
        .iter()
        .copied()
        .filter(|want| {
            !rows.iter().any(|r| {
                let has = |k: &str| {
                    r[k].as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .any(|p| action_matches(p, want))
                };
                has("actions") && !has("notActions")
            })
        })
        .collect()
}

/// `$AZURE_CONFIG_DIR`, else `~/.azure`.
fn az_config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("AZURE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".azure"))
}

/// The az CLI's default subscription (name and id), from its profile.
fn az_default(dir: &std::path::Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(dir.join("azureProfile.json")).ok()?;
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let subs = v["subscriptions"].as_array()?;
    let s = subs
        .iter()
        .find(|s| s["isDefault"].as_bool() == Some(true))
        .or_else(|| subs.first())?;
    Some((
        s["name"].as_str().unwrap_or_default().to_string(),
        s["id"].as_str().unwrap_or_default().to_string(),
    ))
}

#[async_trait]
impl CloudApi for Azure {
    fn name(&self) -> &'static str {
        "azure"
    }

    fn title(&self) -> &'static str {
        "Azure"
    }

    fn tier(&self) -> Tier {
        Tier::Vm
    }

    fn arches(&self) -> Vec<&'static str> {
        vec!["amd64"]
    }

    fn detect(&self) -> CloudCredentials {
        match self.config_dir.as_deref().and_then(az_default) {
            Some((name, id)) => CloudCredentials {
                found: true,
                source: format!("az CLI subscription {name} ({id})"),
            },
            None => CloudCredentials::default(),
        }
    }

    fn resolve(&self, t: &Target) -> Result<Connection> {
        Ok(Connection {
            provider: "azure".into(),
            region: t.region.clone().unwrap_or_else(|| DEFAULT_REGION.into()),
            project: t
                .project
                .clone()
                .unwrap_or_else(|| crate::trust::azure::DEFAULT_RESOURCE_GROUP.into()),
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
                         virtualization and QEMU on the VM)"
                    .into(),
                ..Default::default()
            });
        }
        v.push(CloudKind {
            image: "macos".into(),
            kind: "vm".into(),
            supported: false,
            reason: "macOS runs only on Apple hardware; Azure has none".into(),
            ..Default::default()
        });
        v
    }

    async fn test(&self, conn: &Connection) -> Tested {
        let mut t = Tested::default();
        let ck = Purpose::Check;
        if let Err(e) = self.token(conn, ck).await {
            t.check("credentials", false, e.to_string());
            return t;
        }
        let who = match &conn.auth {
            Auth::Oidc { client_id, .. } => format!("Cua's token, as managed identity {client_id}"),
            _ => "Cua's token".into(),
        };
        t.check("credentials", true, who);
        let group = self.url(&Self::group_path(conn), RESOURCES_API);
        match self.get(conn, ck, &group, "read the resource group").await {
            Ok(Some(g)) => {
                t.account = conn.account.clone();
                t.check(
                    "resource group",
                    true,
                    format!(
                        "{} in {}",
                        conn.project,
                        g["location"].as_str().unwrap_or("?")
                    ),
                );
            }
            Ok(None) => {
                t.check(
                    "resource group",
                    false,
                    format!(
                        "resource group {} not found in subscription {}",
                        conn.project, conn.account
                    ),
                );
                return t;
            }
            Err(e) => {
                t.check("resource group", false, e.to_string());
                return t;
            }
        }
        let perms = self.url(
            &format!(
                "{}/providers/Microsoft.Authorization/permissions",
                Self::group_path(conn)
            ),
            AUTHORIZATION_API,
        );
        match self.get(conn, ck, &perms, "check permissions").await {
            Ok(Some(v)) => {
                let missing = missing_actions(&v);
                if missing.is_empty() {
                    t.check(
                        "permissions",
                        true,
                        format!("{} actions in the resource group", ACTIONS.len()),
                    );
                } else {
                    t.check(
                        "permissions",
                        false,
                        format!("missing: {}", missing.join(", ")),
                    );
                }
            }
            Ok(None) => t.check("permissions", false, "resource group not found"),
            Err(e) => t.check("permissions", false, e.to_string()),
        }
        // A machine as Cua creates it, validated (nothing is created).
        let url = self.url(
            &format!(
                "{}/providers/Microsoft.Resources/deployments/cua-check/validate",
                Self::group_path(conn)
            ),
            RESOURCES_API,
        );
        let body =
            json!({"properties": {"mode": "Incremental", "template": Self::check_template(conn)}});
        match self
            .call(
                conn,
                ck,
                reqwest::Method::POST,
                &url,
                Some(&body),
                "validate a machine",
            )
            .await
        {
            Ok(Some(a)) if a.body["error"].is_null() => t.check(
                "validate",
                true,
                format!("{} with Ubuntu 24.04 in {}", OFFERS[0].1, conn.region),
            ),
            Ok(Some(a)) => t.check("validate", false, arm_error(&a.body, 400)),
            Ok(None) => t.check("validate", false, "resource group not found"),
            Err(e) => t.check("validate", false, e.to_string()),
        }
        t
    }

    async fn provision(&self, conn: &Connection, spec: &ProvisionSpec) -> Result<Vec<Resource>> {
        let owner =
            spec.tags.get(model::tag::OWNER).cloned().ok_or_else(|| {
                Error::InvalidArgument("azure: the create has no owner tag".into())
            })?;
        let shared = self.ensure_network(conn, &owner).await?;
        let net = Self::network_name(&owner);
        let region = if spec.region.is_empty() {
            conn.region.clone()
        } else {
            spec.region.clone()
        };
        let (ip, nic) = (format!("{}-ip", spec.name), format!("{}-nic", spec.name));
        let p = Purpose::Create;
        let made = async {
            self.put(
                conn,
                p,
                "public_ip",
                &ip,
                &json!({
                    "location": region,
                    "tags": spec.tags,
                    "sku": {"name": "Standard"},
                    "properties": {"publicIPAllocationMethod": "Static", "publicIPAddressVersion": "IPv4"},
                }),
                "create the public address",
            )
            .await?;
            self.put(
                conn,
                p,
                "nic",
                &nic,
                &json!({
                    "location": region,
                    "tags": spec.tags,
                    "properties": {"ipConfigurations": [{"name": "ipconfig1", "properties": {
                        "privateIPAllocationMethod": "Dynamic",
                        "subnet": {"id": format!("{}/subnets/{SUBNET}", Self::path(conn, "network", &net))},
                        "publicIPAddress": {"id": Self::path(conn, "public_ip", &ip)},
                    }}]},
                }),
                "create the network interface",
            )
            .await?;
            self.put(
                conn,
                p,
                "instance",
                &spec.name,
                &json!({
                    "location": region,
                    "tags": spec.tags,
                    "properties": vm_properties(
                        &spec.machine_type,
                        &spec.arch,
                        spec.disk_gb,
                        &spec.name,
                        &spec.user_data,
                        &Self::path(conn, "nic", &nic),
                    ),
                }),
                "create the VM",
            )
            .await
        }
        .await;
        if let Err(e) = made {
            // What was made is deleted (it carries our tags).
            for (kind, name) in [("instance", &spec.name), ("nic", &nic), ("public_ip", &ip)] {
                let _ = self
                    .delete_one(conn, Purpose::Manage, kind, name, &owner)
                    .await;
            }
            return Err(e);
        }
        let mut extra = BTreeMap::new();
        extra.insert("network".to_string(), net);
        extra.insert("nic".to_string(), nic);
        extra.insert("public_ip".to_string(), ip);
        let mut out = vec![Resource {
            provider: "azure".into(),
            id: spec.name.clone(),
            resource_type: "instance".into(),
            name: spec.name.clone(),
            region,
            project: conn.project.clone(),
            created: model::now(),
            expires: model::tag_expires(&spec.tags),
            tags: spec.tags.clone(),
            state: "pending".into(),
            extra,
            ..Default::default()
        }];
        out.extend(shared);
        Ok(out)
    }

    async fn describe(
        &self,
        conn: &Connection,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        if r.id.is_empty() {
            return Ok(None);
        }
        let kind = r.resource_type.as_str();
        Ok(self
            .read(conn, Purpose::Manage, kind, &r.id)
            .await?
            .map(|v| {
                let state = if kind == "instance" {
                    state_of(&v)
                } else {
                    "ready".into()
                };
                (state, tags_of(&v))
            }))
    }

    async fn boot_log(&self, conn: &Connection, r: &Resource) -> Result<Option<String>> {
        if r.id.is_empty() || r.resource_type != "instance" {
            return Ok(None);
        }
        let url = self.url(
            &format!(
                "{}/retrieveBootDiagnosticsData",
                Self::path(conn, "instance", &r.id)
            ),
            COMPUTE_API,
        );
        let Some(a) = self
            .call(
                conn,
                Purpose::Manage,
                reqwest::Method::POST,
                &url,
                None,
                "read the boot diagnostics",
            )
            .await?
        else {
            return Ok(None);
        };
        let Some(blob) = a.body["serialConsoleLogBlobUri"].as_str() else {
            return Ok(None);
        };
        // A short-lived signed link to the log: no token of ours with it.
        let resp = self
            .http
            .get(blob)
            .send()
            .await
            .map_err(|e| cloud_err("azure", "read the serial log", e.without_url()))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        Ok(resp.text().await.ok().filter(|t| !t.trim().is_empty()))
    }

    async fn stop(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        // Deallocated, so its compute is not billed (a guest shutdown
        // alone keeps it allocated and billed).
        self.vm_action(conn, r, owner, "deallocate").await
    }

    async fn start(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.vm_action(conn, r, owner, "start").await
    }

    async fn delete(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        if !self.guard(conn, r, owner, "delete").await? {
            return Ok(());
        }
        let p = Purpose::Manage;
        self.delete_one(conn, p, &r.resource_type, &r.id, owner)
            .await?;
        if r.resource_type == "instance" {
            // Its interface goes with it; the address too, and both again
            // here in case Azure kept them.
            let nic = r
                .extra
                .get("nic")
                .cloned()
                .unwrap_or_else(|| format!("{}-nic", r.id));
            let ip = r
                .extra
                .get("public_ip")
                .cloned()
                .unwrap_or_else(|| format!("{}-ip", r.id));
            self.delete_one(conn, p, "nic", &nic, owner).await?;
            self.delete_one(conn, p, "public_ip", &ip, owner).await?;
        }
        Ok(())
    }

    async fn list_owned(&self, conn: &Connection, owner: &str) -> Result<Vec<Resource>> {
        let mut url =
            reqwest::Url::parse(&format!("{}{}/resources", self.arm, Self::group_path(conn)))
                .map_err(|e| cloud_err("azure", "list resources", e))?;
        url.query_pairs_mut()
            .append_pair(
                "$filter",
                &format!(
                    "tagName eq '{}' and tagValue eq '{}'",
                    model::tag::OWNER,
                    model::label_value(owner)
                ),
            )
            .append_pair("$expand", "createdTime")
            .append_pair("api-version", RESOURCES_API);
        let mut next = Some(url.to_string());
        let mut out = Vec::new();
        while let Some(u) = next.take() {
            let v = self
                .get(conn, Purpose::Manage, &self.rebase(&u), "list resources")
                .await?
                .unwrap_or(Value::Null);
            for row in v["value"].as_array().into_iter().flatten() {
                let tags = tags_of(row);
                // Checked again here: the server filter is not trusted alone.
                if !model::is_ours(&tags, owner) {
                    continue;
                }
                let kind = match row["type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                {
                    t if t == "microsoft.compute/virtualmachines" => "instance",
                    t if t == "microsoft.network/virtualnetworks" => "network",
                    t if t == "microsoft.network/networksecuritygroups" => "security_group",
                    t if t == "microsoft.network/networkinterfaces" => "nic",
                    t if t == "microsoft.network/publicipaddresses" => "public_ip",
                    _ => continue,
                };
                let name = row["name"].as_str().unwrap_or_default().to_string();
                let state = if kind == "instance" {
                    match self.read(conn, Purpose::Manage, kind, &name).await? {
                        Some(vm) => state_of(&vm),
                        None => continue,
                    }
                } else {
                    "ready".into()
                };
                out.push(Resource {
                    provider: "azure".into(),
                    id: name.clone(),
                    resource_type: kind.into(),
                    name,
                    region: row["location"].as_str().unwrap_or(&conn.region).to_string(),
                    project: conn.project.clone(),
                    created: row["createdTime"]
                        .as_str()
                        .and_then(|t| humantime::parse_rfc3339_weak(t.get(..19).unwrap_or(t)).ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    expires: model::tag_expires(&tags),
                    tags,
                    state,
                    ..Default::default()
                });
            }
            next = v["nextLink"].as_str().map(str::to_string);
        }
        // Machines first, then what they use, the network before its group.
        let rank = |t: &str| match t {
            "instance" => 0,
            "nic" => 1,
            "public_ip" => 2,
            "network" => 3,
            _ => 4,
        };
        out.sort_by_key(|r| rank(&r.resource_type));
        Ok(out)
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        Capabilities {
            supplies: Supplies::Hosts,
            billing: Billing {
                granularity_secs: 60,
                minimum_secs: 60,
                stopped_bills: false,
            },
            regions: vec![conn.region.clone()],
            max_machines: 20,
            vcpu_quotas: Default::default(),
            self_deletes: false,
        }
    }

    async fn host_types(&self, conn: &Connection) -> Result<Vec<HostType>> {
        let regions = self.capabilities(conn).regions;
        // Pay-as-you-go Linux prices in West US 2 (azure.microsoft.com
        // pricing, 2026-10) plus a 60 GiB Standard SSD ($4.80 a month) and
        // a Standard public IPv4 address.
        let extras = 4.80 / 730.0 + 0.005;
        let table: &[(&str, u32, u64, f64)] = &[
            ("Standard_B2s", 2, 4, 0.0416),
            ("Standard_B2ms", 2, 8, 0.0832),
            ("Standard_B4ms", 4, 16, 0.166),
            ("Standard_B8ms", 8, 32, 0.333),
            ("Standard_D2s_v5", 2, 8, 0.096),
            ("Standard_D4s_v5", 4, 16, 0.192),
        ];
        let mut v: Vec<HostType> = table
            .iter()
            .map(|(name, cpus, gb, usd)| HostType {
                name: (*name).into(),
                arch: "amd64".into(),
                vcpus: *cpus,
                memory_mb: gb * 1024,
                disk_gb: 60,
                kvm: false,
                gpu: String::new(),
                usd_per_hour: ((usd + extras) * 10_000.0).round() / 10_000.0,
                regions: regions.clone(),
                boot_secs: 120,
                quota_pool: "cores".into(),
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

    #[test]
    fn wildcards_and_not_actions_decide_permissions() {
        assert!(action_matches(
            "*",
            "Microsoft.Compute/virtualMachines/write"
        ));
        assert!(action_matches(
            "Microsoft.Compute/virtualMachines/*",
            "microsoft.compute/virtualmachines/start/action"
        ));
        assert!(!action_matches(
            "Microsoft.Compute/disks/*",
            "Microsoft.Compute/virtualMachines/write"
        ));
        let all = json!({"value": [{"actions": ["Microsoft.Compute/*", "Microsoft.Network/*", "Microsoft.Resources/deployments/*"], "notActions": []}]});
        assert!(missing_actions(&all).is_empty());
        let no_delete = json!({"value": [{"actions": ["*"], "notActions": ["Microsoft.Compute/virtualMachines/delete"]}]});
        assert_eq!(
            missing_actions(&no_delete),
            vec!["Microsoft.Compute/virtualMachines/delete"]
        );
        assert_eq!(missing_actions(&json!({})).len(), ACTIONS.len());
    }

    #[test]
    fn states_and_passwords() {
        let vm = |code: &str| json!({"properties": {"instanceView": {"statuses": [{"code": "ProvisioningState/succeeded"}, {"code": code}]}}});
        assert_eq!(state_of(&vm("PowerState/running")), "running");
        assert_eq!(state_of(&vm("PowerState/deallocated")), "stopped");
        assert_eq!(state_of(&vm("PowerState/deallocating")), "stopping");
        assert_eq!(
            state_of(&json!({"properties": {"provisioningState": "Creating"}})),
            "pending"
        );
        let (a, b) = (throwaway_password(), throwaway_password());
        assert_ne!(a, b);
        assert!(a.len() >= 40 && a.chars().any(|c| c.is_ascii_digit()));
        assert_eq!(Azure::network_name("0123abcd"), "cua-sb-0123abcd");
    }

    #[test]
    fn the_az_profile_names_the_subscription_without_reading_secrets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("azureProfile.json"),
            "\u{feff}{\"subscriptions\": [{\"id\": \"s-1\", \"name\": \"Dev\", \"isDefault\": true}]}",
        )
        .unwrap();
        let a = Azure::with_endpoints("http://x", "http://y", None, Some(dir.path().into()));
        let c = a.detect();
        assert!(c.found);
        assert_eq!(c.source, "az CLI subscription Dev (s-1)");
        let none = Azure::with_endpoints("http://x", "http://y", None, None);
        assert!(!none.detect().found);
    }
}
