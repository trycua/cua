// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spaces in your own cloud account on [`Spaces`]: connect AWS, Google
//! Cloud or Modal, check it without creating anything, see what Cua
//! created there and sweep leftovers (the Spaces `cloud` tools, in process
//! or in the daemon; `Spaces.stop` / `start` stop and start a Space there). The providers are the sandbox layer's (the `byoc` feature,
//! on by default); a build without them fails these calls with
//! `Unsupported`.

use serde::Deserialize;
use serde_json::{Value, json};

use super::run;
use super::spaces::{Spaces, parse};
use crate::Result;

/// Where a cloud's credentials were found on this machine (never a
/// secret).
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudCredentials {
    pub found: bool,
    /// "~/.aws profile default", "gcloud account ada@x.io".
    #[serde(default)]
    pub source: String,
}

/// What a cloud can run for one image family, and the estimated cost.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudKind {
    /// `linux`, `linux-slim`, `windows`, `omarchy`, `macos`.
    pub image: String,
    /// `container` or `vm`.
    pub kind: String,
    pub supported: bool,
    /// Why not, or how it runs (one line).
    #[serde(default)]
    pub reason: String,
    /// The machine type Cua picks (`t3.medium`, `e2-medium`).
    #[serde(default)]
    pub machine_type: String,
    /// Estimated on-demand US dollars per hour while it runs.
    #[serde(default)]
    pub usd_per_hour: f64,
}

/// One cloud as `cloud_status` lists it.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudProvider {
    /// `aws`, `gcp`, `hetzner`, `modal`.
    pub name: String,
    /// `AWS`, `Google Cloud`, `Hetzner Cloud`, `Modal`.
    pub title: String,
    /// `vm` or `sandbox`.
    pub tier: String,
    pub connected: bool,
    /// Whether it is the default location (`default.on`).
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub credentials: CloudCredentials,
    /// The account, project or workspace (after a test or connect).
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub zone: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub environment: String,
    /// "AWS · us-west-2".
    #[serde(default)]
    pub label: String,
    /// Hours after which a Space there deletes itself (0: never).
    #[serde(default)]
    pub ttl_hours: u32,
    #[serde(default)]
    pub kinds: Vec<CloudKind>,
    /// `cli` (the cloud CLI's sign-in), `oidc` (no stored key) or
    /// `sealed`; empty when not connected.
    #[serde(default)]
    pub auth: String,
    /// `connected`, `waiting-for-approval`, `not-connected`; from
    /// `cloud_connect` also `needs-trust`, `checking`, `failed`.
    #[serde(default)]
    pub status: String,
    /// The monthly budget in US dollars, when set.
    #[serde(default)]
    pub budget_usd_month: Option<f64>,
    /// The largest machine (`4x16`: vCPUs x GiB), empty: no limit.
    #[serde(default)]
    pub max_size: String,
    /// Estimated US dollars this calendar month.
    #[serde(default)]
    pub spent_month: f64,
    /// Machines Cua runs there now.
    #[serde(default)]
    pub running: u32,
    /// `connection` or `limits` while a person's approval is pending.
    #[serde(default)]
    pub approval: String,
    /// The agent whose request waits for approval.
    #[serde(default)]
    pub approval_agent: String,
    /// The monthly budget an agent asked for, waiting for approval.
    #[serde(default)]
    pub pending_budget_usd_month: Option<f64>,
    /// The largest machine an agent asked for (`4x16`), waiting for
    /// approval.
    #[serde(default)]
    pub pending_max_size: String,
    /// Whether this caller (a person) can approve or deny what waits
    /// (`cloud_approve`).
    #[serde(default)]
    pub can_approve: bool,
    /// `oidc` when this link could switch to linking without a stored key
    /// (an AWS link through the CLI sign-in), else empty.
    #[serde(default)]
    pub suggest_auth: String,
    /// What is wrong with the link and what to do, one sentence, or empty.
    #[serde(default)]
    pub issue: String,
    /// `sealed`: the devices that can open the account's API token
    /// ("Can open it").
    #[serde(default)]
    pub holders: Vec<String>,
}

/// The next step of a `cloud_connect`: a command for the agent, or a page
/// for a person (and what to paste there), and why.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudNext {
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub open: Option<String>,
    #[serde(default)]
    pub paste: Option<String>,
    #[serde(default)]
    pub why: String,
}

/// How `cloud_connect` links a cloud, beyond where.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct CloudConnectOptions {
    /// `cli` (default), `oidc` (AWS, Google Cloud, Azure: no stored key;
    /// Azure always) or `sealed`.
    #[uniffi(default = None)]
    pub auth: Option<String>,
    /// The monthly budget, US dollars.
    #[uniffi(default = None)]
    pub budget_usd_month: Option<f64>,
    /// The largest machine, `4x16`.
    #[uniffi(default = None)]
    pub max_size: Option<String>,
    /// AWS `oidc`: the 12-digit account id.
    #[uniffi(default = None)]
    pub account_id: Option<String>,
    /// AWS `oidc`: the role ARN when it is not `cua-spaces`.
    #[uniffi(default = None)]
    pub role_arn: Option<String>,
    /// Google Cloud `oidc`: the project number (implies `oidc`).
    #[uniffi(default = None)]
    pub project_number: Option<String>,
    /// Google Cloud `oidc`: a service account to act as.
    #[uniffi(default = None)]
    pub service_account: Option<String>,
    /// Azure: the subscription id.
    #[uniffi(default = None)]
    pub subscription_id: Option<String>,
    /// Azure: the Microsoft Entra tenant id.
    #[uniffi(default = None)]
    pub tenant_id: Option<String>,
    /// Azure: the `cua-spaces` managed identity's client id.
    #[uniffi(default = None)]
    pub client_id: Option<String>,
    /// Azure: the resource group (default `cua-spaces`).
    #[uniffi(default = None)]
    pub resource_group: Option<String>,
    /// `oidc`: prove the link (nothing is created) and save it.
    #[uniffi(default = false)]
    pub check: bool,
    /// The agent making the change; its changes wait for approval.
    #[uniffi(default = None)]
    pub agent: Option<String>,
}

/// One thing Cua created in a cloud.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudResource {
    pub provider: String,
    /// The cloud's id.
    pub id: String,
    /// `instance`, `security_group`, `firewall`, `sandbox`, ...
    #[serde(rename = "type")]
    pub resource_type: String,
    /// The sandbox it belongs to (`aws:<name>`), empty for shared
    /// resources.
    #[serde(default)]
    pub sandbox: String,
    /// The relay machine the sandbox joined as (its Space is
    /// `relay:<machine>`).
    #[serde(default)]
    pub machine: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub state: String,
    /// RFC 3339.
    #[serde(default)]
    pub created: String,
    /// RFC 3339, empty: never.
    #[serde(default)]
    pub expires: String,
    #[serde(default)]
    pub expired: bool,
    /// Who started it: an agent's name, or `user` (empty for records made
    /// before it was kept).
    #[serde(default)]
    pub started_by: String,
    /// Estimated US dollars it cost so far (list price times runtime).
    #[serde(default)]
    pub cost_usd: f64,
}

/// `cloud_status`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudStatus {
    /// `default.on`, for example `aws`.
    #[serde(default)]
    pub default_on: String,
    pub providers: Vec<CloudProvider>,
    #[serde(default)]
    pub resources: Vec<CloudResource>,
}

/// One check `cloud_test` ran.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudCheck {
    pub name: String,
    pub ok: bool,
    #[serde(default)]
    pub detail: String,
}

/// `cloud_test`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudTestReport {
    pub provider: String,
    pub ok: bool,
    #[serde(default)]
    pub account: String,
    pub checks: Vec<CloudCheck>,
}

/// `cloud_connect`: the cloud (its `status` is the outcome), the checks it
/// ran, the next step and the command that verifies it.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct CloudConnected {
    pub provider: CloudProvider,
    pub checks: Vec<CloudCheck>,
    /// `None`: nothing left to do.
    pub next: Option<CloudNext>,
    /// `cua cloud status aws`.
    pub verify: String,
}

/// `cloud_disconnect`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudDisconnected {
    pub provider: String,
    pub disconnected: bool,
    /// Resources Cua still records there.
    #[serde(default)]
    pub left: Vec<CloudResource>,
    /// With `delete`: what was deleted from the account first.
    #[serde(default)]
    pub deleted: Vec<CloudResource>,
}

/// One line of the cloud audit log (`cloud_audit`). Never a secret.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudAuditEntry {
    /// When (RFC 3339, UTC).
    pub ts: String,
    /// `connect`, `disconnect`, `resource_recorded`, `resource_created`,
    /// `resource_confirmed` or `resource_deleted`.
    pub action: String,
    pub provider: String,
    /// The resource type; empty for `connect` and `disconnect`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: String,
    /// The Space it belongs to.
    #[serde(default)]
    pub space: String,
    /// `user`, `sweep`, `auto-sweep`, `disconnect` or `rollback`.
    #[serde(default)]
    pub actor: String,
    /// `ok`, `failed` or `refused`.
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub detail: String,
    /// `device` (this cua home's log) or `account` (Cua's issuer, for clouds
    /// linked to the account); empty from an older daemon (this device).
    #[serde(default)]
    pub source: String,
}

/// One row of `cloud_cost`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudCostRow {
    /// The Space, agent, day (`YYYY-MM-DD`) or cloud.
    pub key: String,
    /// Estimated US dollars, to the cent.
    #[serde(default)]
    pub usd: f64,
    /// Machine hours.
    #[serde(default)]
    pub hours: f64,
    /// How many machines ran.
    #[serde(default)]
    pub machines: u32,
}

/// `cloud_cost`: an estimate from list price times runtime.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudCostReport {
    /// `space`, `agent`, `day` or `cloud`.
    pub by: String,
    /// The first day counted (`YYYY-MM-DD`).
    #[serde(default)]
    pub since: String,
    /// Up to when (RFC 3339).
    #[serde(default)]
    pub until: String,
    #[serde(default)]
    pub rows: Vec<CloudCostRow>,
    #[serde(default)]
    pub total_usd: f64,
}

/// One row of `cloud_sweep`.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct CloudSweepItem {
    pub resource: CloudResource,
    /// `delete` (dry run), `deleted`, `keep` or `failed`.
    pub action: String,
    pub reason: String,
}

/// `cloud_sweep`.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct CloudSweepReport {
    pub dry_run: bool,
    pub resources: Vec<CloudSweepItem>,
}

/// Where in a cloud account Spaces go. AWS, Google Cloud and Modal use
/// their own CLI sign-in; Hetzner takes its API token in `token` (or
/// `HCLOUD_TOKEN`).
#[derive(Clone, PartialEq, Default, uniffi::Record)]
pub struct CloudTarget {
    /// `aws`, `gcp`, `hetzner` or `modal`.
    pub provider: String,
    /// AWS or Modal profile.
    #[uniffi(default = None)]
    pub profile: Option<String>,
    /// AWS or GCP region, or Hetzner location (`fsn1`).
    #[uniffi(default = None)]
    pub region: Option<String>,
    /// GCP zone.
    #[uniffi(default = None)]
    pub zone: Option<String>,
    /// GCP project.
    #[uniffi(default = None)]
    pub project: Option<String>,
    /// Modal environment.
    #[uniffi(default = None)]
    pub environment: Option<String>,
    /// Hetzner: the project's Read & Write API token. Secret: used for the
    /// checks, saved in the Cua credential store when they pass, never
    /// printed.
    #[uniffi(default = None)]
    pub token: Option<String>,
}

impl std::fmt::Debug for CloudTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudTarget")
            .field("provider", &self.provider)
            .field("profile", &self.profile)
            .field("region", &self.region)
            .field("zone", &self.zone)
            .field("project", &self.project)
            .field("environment", &self.environment)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl CloudTarget {
    fn args(&self) -> Value {
        json!({
            "provider": self.provider,
            "profile": self.profile,
            "region": self.region,
            "zone": self.zone,
            "project": self.project,
            "environment": self.environment,
            "token": self.token,
        })
    }
}

/// The flattened wire rows (`cloud_connect`, `cloud_sweep`) split into the
/// nested records the bindings get.
fn connected(v: Value) -> Result<CloudConnected> {
    let checks = parse(v.get("checks").cloned().unwrap_or_else(|| json!([])))?;
    let next = match v.get("next") {
        Some(n) if !n.is_null() => Some(parse(n.clone())?),
        _ => None,
    };
    let verify = v
        .get("verify")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(CloudConnected {
        provider: parse(v)?,
        checks,
        next,
        verify,
    })
}

fn sweep(v: Value) -> Result<CloudSweepReport> {
    let dry_run = v.get("dry_run").and_then(Value::as_bool).unwrap_or(true);
    let rows = v
        .get("resources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut resources = Vec::with_capacity(rows.len());
    for row in rows {
        let action = row
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let reason = row
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        resources.push(CloudSweepItem {
            resource: parse(row)?,
            action,
            reason,
        });
    }
    Ok(CloudSweepReport { dry_run, resources })
}

#[uniffi::export]
impl Spaces {
    /// Your clouds: found credentials, the connected account and region,
    /// what each can run at what hourly cost, and what Cua created there.
    /// `provider` narrows it to one cloud.
    pub async fn cloud_status(&self, provider: Option<String>) -> Result<CloudStatus> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value("cloud_status", json!({"provider": provider}))
                    .await?,
            )
        })
        .await
    }

    /// Connects a cloud account (after the checks `cloud_test` runs). With
    /// `make_default`, Spaces are created there when no location is given.
    /// `ttl_hours` (default 8, 0: never) deletes each Space there after
    /// that long. `options` links without a stored key (`auth: oidc`) and
    /// sets the cloud's own budget and largest machine; the result's
    /// `provider.status`, `next` and `verify` say where it stands.
    #[uniffi::method(default(options = None))]
    pub async fn cloud_connect(
        &self,
        target: CloudTarget,
        make_default: bool,
        ttl_hours: Option<u32>,
        options: Option<CloudConnectOptions>,
    ) -> Result<CloudConnected> {
        let host = self.tool_caller();
        run(async move {
            let mut args = target.args();
            args["make_default"] = json!(make_default);
            args["ttl_hours"] = json!(ttl_hours);
            if let Some(o) = options {
                // An older daemon ignores fields it does not know and would
                // link through the CLI sign-in instead: refuse first.
                let asked: Vec<&str> = [
                    ("auth", o.auth.is_some()),
                    ("budget_usd_month", o.budget_usd_month.is_some()),
                    ("max_size", o.max_size.is_some()),
                    ("account_id", o.account_id.is_some()),
                    ("role_arn", o.role_arn.is_some()),
                    ("check", o.check),
                ]
                .into_iter()
                .filter_map(|(k, set)| set.then_some(k))
                .collect();
                host.require_fields(
                    "cloud_connect",
                    &asked,
                    "link a cloud with OIDC, a budget or a largest machine",
                )
                .await?;
                args["auth"] = json!(o.auth);
                args["budget_usd_month"] = json!(o.budget_usd_month);
                args["max_size"] = json!(o.max_size);
                args["account_id"] = json!(o.account_id);
                args["role_arn"] = json!(o.role_arn);
                args["project_number"] = json!(o.project_number);
                args["service_account"] = json!(o.service_account);
                args["subscription_id"] = json!(o.subscription_id);
                args["tenant_id"] = json!(o.tenant_id);
                args["client_id"] = json!(o.client_id);
                args["resource_group"] = json!(o.resource_group);
                args["check"] = json!(o.check);
                args["agent"] = json!(o.agent);
            }
            connected(host.tool_value("cloud_connect", args).await?)
        })
        .await
    }

    /// Approves what an agent asked for on `provider` (its link, or a
    /// raise of its limits), or with `deny` declines it. Only for a person:
    /// refused when an agent calls, and never offered to agents over MCP.
    #[uniffi::method(default(deny = false))]
    pub async fn cloud_approve(&self, provider: String, deny: bool) -> Result<CloudProvider> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value("cloud_approve", json!({"provider": provider, "deny": deny}))
                    .await?,
            )
        })
        .await
    }

    /// Checks a cloud account without creating anything.
    pub async fn cloud_test(&self, target: CloudTarget) -> Result<CloudTestReport> {
        let host = self.tool_caller();
        run(async move { parse(host.tool_value("cloud_test", target.args()).await?) }).await
    }

    /// Forgets a connected cloud. Refused (listing them) while Cua
    /// resources still run there; with `delete_resources`, deletes everything Cua
    /// created there first and disconnects once the cloud confirms.
    #[uniffi::method(default(delete_resources = false))]
    pub async fn cloud_disconnect(
        &self,
        provider: String,
        delete_resources: bool,
    ) -> Result<CloudDisconnected> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value(
                    "cloud_disconnect",
                    json!({"provider": provider, "delete": delete_resources}),
                )
                .await?,
            )
        })
        .await
    }

    /// What Cua did in your clouds, newest last (default the newest 50).
    #[uniffi::method(default(limit = None))]
    pub async fn cloud_audit(&self, limit: Option<u32>) -> Result<Vec<CloudAuditEntry>> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value("cloud_audit", json!({"limit": limit}))
                    .await?,
            )
        })
        .await
    }

    /// What the machines Cua created cost (an estimate: list price times
    /// runtime), grouped `by` `space`, `agent`, `day` or `cloud`, since
    /// `since` (`YYYY-MM-DD`; default the first of this month), for one
    /// `provider` or every cloud.
    #[uniffi::method(default(by = "space", since = None, provider = None))]
    pub async fn cloud_cost(
        &self,
        by: String,
        since: Option<String>,
        provider: Option<String>,
    ) -> Result<CloudCostReport> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value(
                    "cloud_cost",
                    json!({"by": by, "since": since, "provider": provider}),
                )
                .await?,
            )
        })
        .await
    }

    /// What Cua left in your clouds (expired Spaces, resources whose Space
    /// is gone; with `all`, everything Cua created there). Deletes it
    /// unless `dry_run`. Only resources tagged and recorded as Cua's are
    /// ever touched.
    pub async fn cloud_sweep(
        &self,
        provider: Option<String>,
        dry_run: bool,
        all: bool,
    ) -> Result<CloudSweepReport> {
        let host = self.tool_caller();
        run(async move {
            sweep(
                host.tool_value(
                    "cloud_sweep",
                    json!({"provider": provider, "dry_run": dry_run, "all": all}),
                )
                .await?,
            )
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_rows_decode_into_the_binding_records() {
        let wire = serde_json::to_value(cua_spaces::cloud::CloudConnected {
            provider: cua_spaces::cloud::CloudProvider {
                name: "aws".into(),
                title: "AWS".into(),
                tier: "vm".into(),
                connected: true,
                label: "AWS · us-west-2".into(),
                kinds: vec![cua_spaces::cloud::CloudKind {
                    image: "linux".into(),
                    kind: "container".into(),
                    supported: true,
                    machine_type: "t3.medium".into(),
                    usd_per_hour: 0.0416,
                    ..Default::default()
                }],
                auth: "oidc".into(),
                status: "checking".into(),
                budget_usd_month: Some(200.0),
                max_size: "4x16".into(),
                ..Default::default()
            },
            checks: vec![cua_spaces::cloud::CloudCheck {
                name: "credentials".into(),
                ok: true,
                detail: "account 1".into(),
            }],
            next: Some(cua_sandbox_core::byoc::CloudNext {
                run: Some("cua cloud connect aws --oidc --check".into()),
                why: "prove it".into(),
                ..Default::default()
            }),
            verify: "cua cloud status aws".into(),
        })
        .unwrap();
        let c = connected(wire).unwrap();
        assert_eq!(c.provider.status, "checking");
        assert_eq!(c.provider.auth, "oidc");
        assert_eq!(c.provider.budget_usd_month, Some(200.0));
        assert_eq!(c.provider.max_size, "4x16");
        let next = c.next.unwrap();
        assert_eq!(
            next.run.as_deref(),
            Some("cua cloud connect aws --oidc --check")
        );
        assert_eq!((next.open, next.why.as_str()), (None, "prove it"));
        assert_eq!(c.verify, "cua cloud status aws");
        assert_eq!(c.provider.label, "AWS · us-west-2");
        assert_eq!(c.provider.kinds[0].machine_type, "t3.medium");
        assert_eq!(c.checks[0].name, "credentials");

        let wire = serde_json::to_value(cua_spaces::cloud::CloudSweepReport {
            dry_run: true,
            resources: vec![cua_spaces::cloud::CloudSweepItem {
                resource: cua_spaces::cloud::CloudResource {
                    provider: "aws".into(),
                    id: "i-1".into(),
                    resource_type: "instance".into(),
                    expired: true,
                    ..Default::default()
                },
                action: "delete".into(),
                reason: "expired".into(),
            }],
        })
        .unwrap();
        let r = sweep(wire).unwrap();
        assert!(r.dry_run);
        assert_eq!(r.resources[0].resource.resource_type, "instance");
        assert_eq!(r.resources[0].action, "delete");
    }
}
