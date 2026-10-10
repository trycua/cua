// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Your own cloud account as a sandbox location (`--on aws`, `gcp`,
//! `modal`): the records every surface shares (`cua cloud`, the SDK, the
//! Spaces `cloud_*` tools, the apps) and [`CloudManager`], the service that
//! connects, checks and sweeps the clouds. The providers themselves are
//! ordinary [`crate::Provider`]s registered like the contrib ones; they live
//! in the `cua-byoc` crate.
//!
//! Every cloud sandbox is a relay machine of the signed-in cua.ai account:
//! the guest's cua-spacesd dials out to the relay (no inbound port), and the
//! SDK reaches it there with the account's credentials
//! ([`crate::Provider::connect_options`]). Its instance details carry
//! [`DETAIL_RELAY_MACHINE`], which is how Spaces shows it as
//! `relay:<machine>`.

use serde::{Deserialize, Serialize};

use crate::Result;

/// The instance detail naming the relay machine a cloud sandbox joined as.
pub const DETAIL_RELAY_MACHINE: &str = "relay_machine";

/// Relay machine metadata (`meta`) a cloud sandbox's machine is registered
/// with, so every device of the account can label it and knows where it
/// can be deleted. No secrets.
pub mod meta {
    /// The provider word (`aws`, `gcp`, `modal`).
    pub const PROVIDER: &str = "cua.cloud.provider";
    /// Where it runs, for people ("AWS · us-west-2").
    pub const PLACE: &str = "cua.cloud.place";
    /// The sandbox's name.
    pub const SANDBOX: &str = "cua.cloud.sandbox";
    /// The cua home that created it (its `cua-owner` tag).
    pub const OWNER: &str = "cua.cloud.owner";
    /// That home's own relay machine, when it is a host other devices can
    /// ask to delete it (`HostSpacesService.DeleteCloudSpace`).
    pub const HOST: &str = "cua.cloud.host";
    /// That home's device name, for "delete it there".
    pub const DEVICE: &str = "cua.cloud.device";
}

/// The instance detail naming where a cloud sandbox runs ("AWS ·
/// us-west-2"), shown next to its Space.
pub const DETAIL_PLACE: &str = "place";

/// Where a cloud's credentials were found on this machine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudCredentials {
    /// Whether the cloud's CLI sign-in (or its config) is on this machine.
    pub found: bool,
    /// Where ("~/.aws profile default", "gcloud account ada@x.io"), never a
    /// secret. Empty when none was found.
    #[serde(default)]
    pub source: String,
}

/// What a cloud can run for one image family, and at what cost.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudKind {
    /// The image family (`linux`, `linux-slim`, `windows`, `omarchy`,
    /// `macos`).
    pub image: String,
    /// `container` or `vm`.
    pub kind: String,
    /// Whether a sandbox of it can be created there.
    pub supported: bool,
    /// Why not (one line), or a note on how it runs.
    #[serde(default)]
    pub reason: String,
    /// The machine type Cua picks for it (`t3.medium`, `e2-medium`, a
    /// Modal CPU and memory size).
    #[serde(default)]
    pub machine_type: String,
    /// The estimated on-demand cost per hour in US dollars while it runs
    /// (compute and disk; not network).
    #[serde(default)]
    pub usd_per_hour: f64,
}

/// One cloud as `cloud_status` lists it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudProvider {
    /// `aws`, `gcp`, `modal`.
    pub name: String,
    /// `AWS`, `Google Cloud`, `Modal`.
    pub title: String,
    /// `vm` (one VM per sandbox) or `sandbox` (one platform sandbox each).
    pub tier: String,
    /// Whether it is connected (`cloud_connect`).
    pub connected: bool,
    /// Whether it is the default location (`default.on`).
    #[serde(default)]
    pub default: bool,
    /// The credentials found on this machine.
    #[serde(default)]
    pub credentials: CloudCredentials,
    /// The account, project or workspace the credentials reach (after a
    /// test or connect).
    #[serde(default)]
    pub account: String,
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
    /// How the apps name it once connected: "AWS · us-west-2".
    #[serde(default)]
    pub label: String,
    /// Hours after which a sandbox there deletes itself (0: never).
    #[serde(default)]
    pub ttl_hours: u32,
    /// What it can run, and the cost.
    #[serde(default)]
    pub kinds: Vec<CloudKind>,
}

/// One thing Cua created in a cloud.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudResource {
    /// `aws`, `gcp`, `modal`.
    pub provider: String,
    /// The cloud's id (`i-0abc...`, the instance name, `sb-...`).
    pub id: String,
    /// `instance`, `security_group`, `firewall`, `sandbox`, ...
    #[serde(rename = "type")]
    pub resource_type: String,
    /// The sandbox it belongs to (`aws:<name>`), empty for shared
    /// resources (a security group).
    #[serde(default)]
    pub sandbox: String,
    /// The relay machine the sandbox joined as (its Space is
    /// `relay:<machine>`), when it has one.
    #[serde(default)]
    pub machine: String,
    /// Region (or zone).
    #[serde(default)]
    pub region: String,
    /// The cloud's state (`running`, `stopped`, `terminated`, `gone`).
    #[serde(default)]
    pub state: String,
    /// When Cua created it (RFC 3339).
    #[serde(default)]
    pub created: String,
    /// When it expires (RFC 3339; empty: never).
    #[serde(default)]
    pub expires: String,
    /// Whether it is past `expires`.
    #[serde(default)]
    pub expired: bool,
}

/// `cloud_status`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudStatusReport {
    /// The default location (`default.on`), for example `aws`.
    #[serde(default)]
    pub default_on: String,
    /// Every cloud this build supports.
    pub providers: Vec<CloudProvider>,
    /// What Cua created in them.
    #[serde(default)]
    pub resources: Vec<CloudResource>,
}

/// One check of `cloud_test`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudCheck {
    /// `credentials`, `permissions`, `quota`, `region`, ...
    pub name: String,
    /// Whether it passed.
    pub ok: bool,
    /// What was found, or what to fix.
    #[serde(default)]
    pub detail: String,
}

/// `cloud_test`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudTestReport {
    /// The cloud.
    pub provider: String,
    /// Whether every check passed.
    pub ok: bool,
    /// The account, project or workspace the credentials reach.
    #[serde(default)]
    pub account: String,
    /// The checks, in order.
    pub checks: Vec<CloudCheck>,
}

/// `cloud_connect`: the connected cloud plus the checks it passed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudConnected {
    /// The cloud, as `cloud_status` lists it.
    #[serde(flatten)]
    pub provider: CloudProvider,
    /// The checks it passed.
    #[serde(default)]
    pub checks: Vec<CloudCheck>,
}

/// `cloud_disconnect`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudDisconnected {
    /// The cloud.
    pub provider: String,
    /// Whether a connection was forgotten.
    pub disconnected: bool,
    /// Resources Cua still records there.
    #[serde(default)]
    pub left: Vec<CloudResource>,
}

/// One row of `cloud_sweep`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudSweepItem {
    /// The resource.
    #[serde(flatten)]
    pub resource: CloudResource,
    /// `delete` (dry run), `deleted`, `keep`, `forget`, `forgotten` or
    /// `failed`.
    pub action: String,
    /// Why.
    #[serde(default)]
    pub reason: String,
}

/// `cloud_sweep`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudSweepReport {
    /// Whether nothing was deleted.
    pub dry_run: bool,
    /// What was found.
    pub resources: Vec<CloudSweepItem>,
}

/// Where in a cloud account sandboxes go (`cloud_connect`, `cloud_test`).
/// Credentials are never here: each cloud uses its own CLI's sign-in, and
/// Cua stores only these names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudTarget {
    /// `aws`, `gcp`, `modal`.
    pub provider: String,
    /// AWS or Modal profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Region.
    #[serde(default)]
    pub region: Option<String>,
    /// GCP zone.
    #[serde(default)]
    pub zone: Option<String>,
    /// GCP project.
    #[serde(default)]
    pub project: Option<String>,
    /// Modal environment.
    #[serde(default)]
    pub environment: Option<String>,
}

/// Connects, checks and sweeps your clouds (implemented in `cua-byoc`).
#[async_trait::async_trait]
pub trait CloudManager: Send + Sync + 'static {
    /// `cloud_status`: `provider` narrows it to one cloud.
    async fn status(&self, provider: Option<&str>) -> Result<CloudStatusReport>;

    /// `cloud_test`: creates nothing.
    async fn test(&self, target: &CloudTarget) -> Result<CloudTestReport>;

    /// `cloud_connect`: runs the test first; with `make_default`, sets
    /// `default.on`. `ttl_hours` (default 8, 0: never) deletes every
    /// sandbox there after that long.
    async fn connect(
        &self,
        target: &CloudTarget,
        make_default: bool,
        ttl_hours: Option<u32>,
    ) -> Result<CloudConnected>;

    /// `cloud_disconnect`: nothing in the cloud is deleted.
    async fn disconnect(&self, provider: &str) -> Result<CloudDisconnected>;

    /// `cloud_sweep`: dry run unless `dry_run` is false.
    async fn sweep(
        &self,
        provider: Option<&str>,
        dry_run: bool,
        all: bool,
    ) -> Result<CloudSweepReport>;
}

/// The error a cloud word without a manager in this build gets.
pub fn not_built() -> crate::Error {
    crate::Error::Unsupported {
        provider: crate::ProviderKind::Contrib,
        op: "your own cloud (aws, gcp, modal): this build has no cloud providers (build the \
             cua CLI or SDK with the `byoc` feature, which the release builds include)"
            .into(),
    }
}
