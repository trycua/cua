// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Your own cloud account as a sandbox location (`--on aws`, `gcp`,
//! `modal`, `hetzner`): the records every surface shares (`cua cloud`, the SDK, the
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
    /// The provider word (`aws`, `gcp`, `hetzner`, `modal`).
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
    /// `aws`, `gcp`, `hetzner`, `modal`.
    pub name: String,
    /// `AWS`, `Google Cloud`, `Hetzner Cloud`, `Modal`.
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
    /// Region (a Hetzner location such as `fsn1`).
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
    /// `aws`, `gcp`, `hetzner`, `modal`.
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
    /// With `delete`: what was deleted from the account first (the
    /// receipt), each resource as `cloud_status` lists it.
    #[serde(default)]
    pub deleted: Vec<CloudResource>,
}

/// One line of the cloud audit log (`cloud_audit`): something Cua did in a
/// cloud account. Never carries a secret or a token.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudAuditEntry {
    /// When (RFC 3339, UTC).
    pub ts: String,
    /// `connect`, `disconnect`, `resource_recorded`, `resource_created`,
    /// `resource_confirmed` or `resource_deleted`.
    pub action: String,
    /// `aws`, `gcp`, `modal`.
    pub provider: String,
    /// The resource type (`instance`, `sandbox`, `security_group`, ...);
    /// empty for `connect` and `disconnect`.
    #[serde(default)]
    pub kind: String,
    /// The cloud id (the name Cua chose while a create is under way).
    #[serde(default)]
    pub id: String,
    /// The sandbox (Space) it belongs to; empty for shared resources.
    #[serde(default)]
    pub space: String,
    /// Who did it: `user`, `sweep`, `auto-sweep`, `disconnect` or
    /// `rollback`.
    #[serde(default)]
    pub actor: String,
    /// `ok`, `failed` or `refused`.
    #[serde(default)]
    pub outcome: String,
    /// A short note (the reason, or the error).
    #[serde(default)]
    pub detail: String,
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
/// Clouds with a CLI sign-in (AWS, Google Cloud, Hetzner, Modal) use it, and Cua
/// stores only these names. A cloud that has only an API token (Hetzner)
/// takes it in [`CloudTarget::token`]: used for the checks, saved in the
/// Cua credential store only when they pass, and never in `Debug` output,
/// logs or the connection record.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudTarget {
    /// `aws`, `gcp`, `hetzner`, `modal`.
    pub provider: String,
    /// AWS or Modal profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Region (a Hetzner location such as `fsn1`).
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
    /// The API token of a cloud that has no CLI sign-in (Hetzner). Secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// What a linked cloud supplies to the elastic pool.
pub mod supplies {
    /// Machines the pool controls: each runs cua-spacesd as a host and many
    /// Spaces run on it (AWS, GCP, Hetzner).
    pub const HOSTS: &str = "hosts";
    /// Spaces directly, one platform sandbox each (Modal).
    pub const SPACES: &str = "spaces";
}

/// One machine type a linked cloud offers to the elastic pool, in one
/// region: what the solver may add (`cua cloud plan` lists them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostTypeOffer {
    /// `aws`, `gcp`, `hetzner`, `modal`.
    pub provider: String,
    /// The cloud's type name (`cx33`, `c7g.xlarge`; for a Spaces supplier
    /// a size word such as `2x4`).
    pub type_name: String,
    /// Region or location.
    pub region: String,
    /// `amd64` or `arm64`.
    pub arch: String,
    /// vCPUs.
    pub vcpus: u32,
    /// Memory, MiB.
    pub memory_mb: u64,
    /// Boot disk, GiB.
    #[serde(default)]
    pub disk_gb: u32,
    /// It has `/dev/kvm` (runs `vm` and `microvm` Spaces).
    #[serde(default)]
    pub kvm: bool,
    /// Kinds its hosts create.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Accelerator, or empty.
    #[serde(default)]
    pub gpu: String,
    /// On-demand USD per hour, with what every machine of it carries.
    pub usd_per_hour: f64,
    /// Billed in steps of this many seconds.
    pub billing_step_secs: u64,
    /// Minimum billed seconds.
    pub billing_min_secs: u64,
    /// Typical seconds from add to ready (learned when measured).
    pub boot_secs: u32,
    /// [`supplies::HOSTS`] or [`supplies::SPACES`].
    pub supplies: String,
    /// How many more machines this provider allows now.
    #[serde(default)]
    pub max_new: u32,
    /// The quota pool its vCPUs count against (`hetzner/shared`); empty:
    /// none.
    #[serde(default)]
    pub quota: String,
    /// That pool's vCPU limit when the cloud says it (0: unknown).
    #[serde(default)]
    pub quota_vcpus: u32,
    /// 0 in the region you linked; n > 0 in the provider's n-th nearby
    /// region, which the pool uses only when every type it could use in the
    /// linked region has just failed (the cloud had no capacity there).
    #[serde(default)]
    pub fallback: u8,
}

/// A cloud host the elastic pool added (or `cua cloud hosts add`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudHost {
    /// Its pool name (`hetzner-3`).
    pub name: String,
    /// Provider.
    pub provider: String,
    /// Machine type.
    pub type_name: String,
    /// Region.
    pub region: String,
    /// The cloud's id.
    #[serde(default)]
    pub id: String,
    /// Its relay machine once joined (empty before).
    #[serde(default)]
    pub machine: String,
    /// `creating`, `booting`, `joined`, `cordoned`, `removing`, `gone`.
    pub state: String,
    /// Unix seconds it was created (billing starts).
    pub started: u64,
    /// Unix seconds it ends on its own at the latest (its TTL).
    #[serde(default)]
    pub expires: u64,
    /// USD per hour.
    pub usd_per_hour: f64,
    /// Billing step, seconds.
    pub billing_step_secs: u64,
    /// Minimum billed, seconds.
    pub billing_min_secs: u64,
    /// Added by hand (`cua cloud hosts add`): the scaler never removes it.
    #[serde(default)]
    pub pinned: bool,
}

/// Adds one host.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AddHost {
    /// Provider.
    pub provider: String,
    /// Machine type.
    pub type_name: String,
    /// Region (empty: the connection's).
    #[serde(default)]
    pub region: String,
    /// Pool name (empty: `<provider>-<n>`).
    #[serde(default)]
    pub name: String,
    /// Added by hand: never removed by the scaler.
    #[serde(default)]
    pub pinned: bool,
    /// Seconds until it ends on its own (0: the default, 24 h).
    #[serde(default)]
    pub ttl_secs: u64,
    /// Images it pulls as soon as it joined (the ones it is added for), so
    /// its first Spaces start without a pull.
    #[serde(default)]
    pub prepull: Vec<String>,
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

    /// `cloud_disconnect`: refuses while Spaces Cua created still run
    /// there (it lists them). With `delete`, deletes everything Cua created
    /// there first and forgets the connection only once the cloud confirms
    /// nothing is left.
    async fn disconnect(&self, provider: &str, delete: bool) -> Result<CloudDisconnected>;

    /// `cloud_sweep`: dry run unless `dry_run` is false.
    async fn sweep(
        &self,
        provider: Option<&str>,
        dry_run: bool,
        all: bool,
    ) -> Result<CloudSweepReport>;

    /// The machine types every linked cloud offers to the elastic pool,
    /// one row per type and region (empty: no cloud linked, or none can
    /// add machines now).
    async fn host_offers(&self) -> Result<Vec<HostTypeOffer>> {
        Ok(vec![])
    }

    /// The hosts this home added to the pool, in every linked cloud.
    async fn hosts(&self) -> Result<Vec<CloudHost>> {
        Ok(vec![])
    }

    /// Adds a host: it is tagged and recorded first, then created; it
    /// installs cua, joins the account's pool as a host that provides
    /// Spaces, and ends on its own at its TTL. Returns once the cloud
    /// accepted it (state `booting`); [`CloudManager::hosts`] shows it
    /// `joined` when it is in the pool.
    async fn add_host(&self, _req: &AddHost) -> Result<CloudHost> {
        Err(not_built())
    }

    /// Removes a host this home added (refused for anything else).
    async fn remove_host(&self, _name: &str) -> Result<()> {
        Err(not_built())
    }

    /// Cordons a host this home added (it takes no new Spaces and drains),
    /// or opens it again.
    async fn cordon_host(&self, _name: &str, _cordoned: bool) -> Result<()> {
        Err(not_built())
    }
    /// The automatic sweep the daemon schedules: deletes what
    /// `cloud_sweep` with `dry_run: false` (and not `all`) deletes, in
    /// every connected cloud; one failing cloud does not stop the others.
    async fn auto_sweep(&self) -> Result<CloudSweepReport>;

    /// `cloud_audit`: the newest `limit` lines of the audit log, newest
    /// last.
    async fn audit(&self, limit: usize) -> Result<Vec<CloudAuditEntry>>;
}

/// The error a cloud word without a manager in this build gets.
pub fn not_built() -> crate::Error {
    crate::Error::Unsupported {
        provider: crate::ProviderKind::Contrib,
        op: "your own cloud (aws, gcp, hetzner, modal): this build has no cloud providers (build the \
             cua CLI or SDK with the `byoc` feature, which the release builds include)"
            .into(),
    }
}
