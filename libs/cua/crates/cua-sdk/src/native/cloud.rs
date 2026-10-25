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
    /// Clouds disconnected while they could not be checked (signed out)
    /// that may still run Spaces Cua started, each until a sweep after
    /// sign-in confirms it clean.
    #[serde(default)]
    pub cleanup: Vec<CloudCleanup>,
}

/// A cloud disconnected while Cua could not check it: what Cua's records
/// say may still run there.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, uniffi::Record)]
pub struct CloudCleanup {
    pub provider: String,
    #[serde(default)]
    pub title: String,
    /// The account the records belong to.
    #[serde(default)]
    pub account: String,
    /// The profile it was connected with: the only one a sweep of it uses.
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub region: String,
    /// Spaces (instances and sandboxes) that may still run.
    #[serde(default)]
    pub spaces: u32,
    /// Every recorded resource, Spaces included.
    #[serde(default)]
    pub resources: u32,
    /// "instance t4g.medium ×2".
    #[serde(default)]
    pub types: String,
    /// RFC 3339; empty: none or never.
    #[serde(default)]
    pub latest_expires: String,
    /// One of them never expires on its own.
    #[serde(default)]
    pub never_expires: bool,
    /// Their recorded cost per hour while they run, US dollars.
    #[serde(default)]
    pub usd_per_hour: f64,
    /// RFC 3339.
    #[serde(default)]
    pub since: String,
    /// Why Cua could not check it.
    #[serde(default)]
    pub reason: String,
    /// "AWS may still have 2 Spaces Cua started. Sign in and clean up."
    #[serde(default)]
    pub message: String,
}

/// A `Cloud` error's message without the cloud's raw error: the plain
/// line(s) to show people ("Your AWS sign-in expired. Sign in with `aws
/// login`, then run this again."). The raw part follows `"\n  details: "`
/// in the message; show it only when asked (`--verbose`).
#[uniffi::export]
pub fn cloud_signin_plain(message: &str) -> String {
    cua_sandbox_core::byoc::signin::split(message).0.to_string()
}

/// The raw cloud error a `Cloud` error message carries after its plain
/// line(s), if any.
#[uniffi::export]
pub fn cloud_signin_details(message: &str) -> Option<String> {
    cua_sandbox_core::byoc::signin::split(message)
        .1
        .map(str::to_string)
}

/// Whether a `Cloud` error is a `cloud_disconnect` refused because the
/// cloud could not be checked (signed out): ask the person "Disconnect
/// anyway?" and call again with `force` if they agree.
#[uniffi::export]
pub fn cloud_signin_unchecked(message: &str) -> bool {
    cua_sandbox_core::byoc::signin::is_unchecked(message)
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

/// `cloud_connect`: the connected cloud and the checks it passed.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct CloudConnected {
    pub provider: CloudProvider,
    pub checks: Vec<CloudCheck>,
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
    /// With `force`, when the cloud could not be checked: what may still
    /// run there (also in `cloud_status().cleanup` until a sweep confirms
    /// it clean).
    #[serde(default)]
    pub cleanup: Option<CloudCleanup>,
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
    Ok(CloudConnected {
        provider: parse(v)?,
        checks,
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
    /// that long.
    pub async fn cloud_connect(
        &self,
        target: CloudTarget,
        make_default: bool,
        ttl_hours: Option<u32>,
    ) -> Result<CloudConnected> {
        let host = self.tool_caller();
        run(async move {
            let mut args = target.args();
            args["make_default"] = json!(make_default);
            args["ttl_hours"] = json!(ttl_hours);
            connected(host.tool_value("cloud_connect", args).await?)
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
    /// created there first and disconnects once the cloud confirms. When the
    /// cloud cannot be checked (signed out), it is refused with a `Cloud`
    /// error starting "Cua couldn't check"; ask the person, then `force`
    /// disconnects anyway and keeps Cua's records for a sweep after sign-in.
    #[uniffi::method(default(delete_resources = false, force = false))]
    pub async fn cloud_disconnect(
        &self,
        provider: String,
        delete_resources: bool,
        force: bool,
    ) -> Result<CloudDisconnected> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value(
                    "cloud_disconnect",
                    json!({"provider": provider, "delete": delete_resources, "force": force}),
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
                ..Default::default()
            },
            checks: vec![cua_spaces::cloud::CloudCheck {
                name: "credentials".into(),
                ok: true,
                detail: "account 1".into(),
            }],
        })
        .unwrap();
        let c = connected(wire).unwrap();
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
