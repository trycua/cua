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
    /// `aws`, `gcp`, `modal`.
    pub name: String,
    /// `AWS`, `Google Cloud`, `Modal`.
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

/// Where in a cloud account Spaces go. Credentials are never here: each
/// cloud uses its own CLI sign-in.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct CloudTarget {
    /// `aws`, `gcp` or `modal`.
    pub provider: String,
    /// AWS or Modal profile.
    #[uniffi(default = None)]
    pub profile: Option<String>,
    /// AWS or GCP region.
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

    /// Forgets a connected cloud (nothing in it is deleted).
    pub async fn cloud_disconnect(&self, provider: String) -> Result<CloudDisconnected> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value("cloud_disconnect", json!({"provider": provider}))
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
