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
    /// How it reaches the cloud: `cli` (the cloud CLI's sign-in on this
    /// device), `oidc` (Cua's issuer, no stored key) or `sealed`; empty
    /// when not connected.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth: String,
    /// [`status`]: `connected`, `waiting-for-approval`, `not-connected`
    /// (and, from `cloud_connect`, `needs-trust`, `checking`, `failed`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// The monthly budget (US dollars), when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd_month: Option<f64>,
    /// The largest machine Cua may create there (`4x16`: vCPUs x GiB),
    /// empty: no limit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub max_size: String,
    /// Estimated US dollars Cua's machines there cost this calendar month
    /// (UTC), list price times runtime.
    #[serde(default)]
    pub spent_month: f64,
    /// Machines (and sandboxes) Cua runs there now.
    #[serde(default)]
    pub running: u32,
    /// What waits for a person's approval in Cua Spaces: `connection`,
    /// `limits`, or empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval: String,
    /// The agent whose link or limits wait for approval, when an agent
    /// asked.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_agent: String,
    /// The monthly budget an agent asked for, waiting for approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_budget_usd_month: Option<f64>,
    /// The largest machine an agent asked for (`4x16`), waiting for
    /// approval.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pending_max_size: String,
    /// Whether the caller can approve or deny what waits (`cloud_approve`):
    /// a person's session on a device signed in to the account, never an
    /// agent.
    #[serde(default)]
    pub can_approve: bool,
    /// A better way to link it that this link could switch to (`oidc` for
    /// an AWS link through the CLI sign-in), or empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suggest_auth: String,
    /// What is wrong with the link and what to do, in one plain sentence
    /// (for example, it was disconnected on another device), or empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub issue: String,
    /// `sealed`: the devices that can open the account's API token now
    /// (names, else ids). At least one must be online for machines to be
    /// made there unattended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holders: Vec<String>,
}

/// The words of [`CloudProvider::status`] and [`CloudConnected`]'s outcome.
pub mod status {
    /// Ready: Spaces can be created there.
    pub const CONNECTED: &str = "connected";
    /// Not linked.
    pub const NOT_CONNECTED: &str = "not-connected";
    /// The cloud does not trust Cua yet: a person sets up the trust
    /// (`next.open`).
    pub const NEEDS_TRUST: &str = "needs-trust";
    /// Recorded; the check that proves it works has not run
    /// (`next.run`).
    pub const CHECKING: &str = "checking";
    /// An agent linked it or raised a limit: a person approves it in Cua
    /// Spaces.
    pub const WAITING_FOR_APPROVAL: &str = "waiting-for-approval";
    /// It did not work; `checks` and `next` say why.
    pub const FAILED: &str = "failed";
}

/// The next step of a `cloud_connect`: a command for the agent to run, or
/// a page for a person to open, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudNext {
    /// The exact command the agent runs next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The page a person opens next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    /// What the person pastes there (an AWS CloudShell command), when
    /// there is something to paste.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paste: Option<String>,
    /// Why, in plain words.
    pub why: String,
}

impl CloudNext {
    /// Whether it waits on a person (a page to open).
    pub fn for_person(&self) -> bool {
        self.open.is_some()
    }
}

/// The environment variable a Cua agent run carries with its agent's name
/// (a persistent agent or routine).
pub const AGENT_VAR: &str = "CUA_AGENT";

/// The run id every Cua agent run carries: an agent with no name is still
/// an agent.
pub const AGENT_RUN_VAR: &str = "CUA_AGENT_RUN_ID";

/// Coding agents that mark their shells, and the name each acts as.
pub const CODING_AGENT_VARS: &[(&str, &str)] = &[
    ("CLAUDECODE", "claude-code"),
    ("CURSOR_AGENT", "cursor"),
    ("GEMINI_CLI", "gemini-cli"),
    ("CODEX_SANDBOX", "codex"),
];

/// The Cua agent this process runs for ([`AGENT_VAR`], else
/// [`AGENT_RUN_VAR`]) over `env`; `None` when none.
pub fn cua_agent_with(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    [AGENT_VAR, AGENT_RUN_VAR]
        .into_iter()
        .filter_map(env)
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

/// [`cua_agent_with`] over this process's environment.
pub fn cua_agent() -> Option<String> {
    cua_agent_with(|k| std::env::var(k).ok())
}

/// Who runs this process, over `env`: a Cua agent's name or run id, else a
/// coding agent that marks its shell (Claude Code, Cursor, Gemini CLI,
/// Codex); `None` when a person does.
pub fn caller_agent_with(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    cua_agent_with(&env).or_else(|| {
        CODING_AGENT_VARS
            .iter()
            .find(|(k, _)| env(k).is_some_and(|v| !v.trim().is_empty()))
            .map(|(_, name)| (*name).to_string())
    })
}

/// [`caller_agent_with`] over this process's environment.
pub fn caller_agent() -> Option<String> {
    caller_agent_with(|k| std::env::var(k).ok())
}

/// What `cloud_connect` asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectRequest {
    /// Where in the account.
    pub target: CloudTarget,
    /// Also make it the default location.
    pub make_default: bool,
    /// Hours a Space there lives (default 8, 0: never).
    pub ttl_hours: Option<u32>,
    /// `cli` (default: the cloud CLI's sign-in), `oidc` or `sealed`.
    pub auth: Option<String>,
    /// The monthly budget, US dollars.
    pub budget_usd_month: Option<f64>,
    /// The largest machine, `4x16` (vCPUs x GiB).
    pub max_size: Option<String>,
    /// AWS: the 12-digit account id the trust stack is in.
    pub account_id: Option<String>,
    /// AWS: the role, when it is not `cua-spaces`.
    pub role_arn: Option<String>,
    /// Google Cloud `oidc`: the project number (the trust script prints
    /// it). The project id is `target.project`.
    pub project_number: Option<String>,
    /// Google Cloud `oidc`: a service account to act as (default: none,
    /// the federated identity calls Google directly).
    pub service_account: Option<String>,
    /// Azure: the subscription id.
    pub subscription_id: Option<String>,
    /// Azure: the Microsoft Entra tenant id.
    pub tenant_id: Option<String>,
    /// Azure: the client id of the `cua-spaces` managed identity.
    pub client_id: Option<String>,
    /// Azure: the resource group Cua works in (default `cua-spaces`).
    pub resource_group: Option<String>,
    /// Run the check that proves the link works (a `check` token, the
    /// exchange, a dry run).
    pub check: bool,
    /// The agent that asks, when an agent does (its changes wait for a
    /// person's approval).
    pub agent: Option<String>,
}

impl ConnectRequest {
    /// The way it links, `cli`, `oidc` or `sealed`: the one asked for,
    /// else `oidc` for Azure (it has no CLI sign-in link) and for a Google
    /// Cloud project number or service account (they name OIDC trust),
    /// else `cli`.
    pub fn auth_word(&self) -> String {
        let given = |o: &Option<String>| o.as_deref().is_some_and(|v| !v.trim().is_empty());
        match self
            .auth
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            Some(a) => a.to_ascii_lowercase(),
            None if self.target.provider.eq_ignore_ascii_case("azure") => "oidc".into(),
            None if self.target.provider.eq_ignore_ascii_case("gcp")
                && (given(&self.project_number) || given(&self.service_account)) =>
            {
                "oidc".into()
            }
            None => "cli".into(),
        }
    }

    /// Whether it asks for today's CLI sign-in link ([`Self::auth_word`]
    /// is `cli`).
    pub fn is_cli(&self) -> bool {
        self.auth_word() == "cli"
    }
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
    /// Who started it: the agent's name, or `user`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started_by: String,
    /// Estimated US dollars it cost so far (list price times runtime).
    #[serde(default)]
    pub cost_usd: f64,
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

/// `cloud_connect`: the cloud as `cloud_status` lists it (its `status`
/// is the outcome), the checks it ran, the next step and how to verify.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudConnected {
    /// The cloud, as `cloud_status` lists it.
    #[serde(flatten)]
    pub provider: CloudProvider,
    /// The checks it ran.
    #[serde(default)]
    pub checks: Vec<CloudCheck>,
    /// What to do next (`None`: nothing, it is connected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<CloudNext>,
    /// The command that confirms it (`cua cloud status aws`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub verify: String,
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
    /// Where it was recorded: `device` (this cua home's log) or `account`
    /// (Cua's issuer, for clouds linked to the account: tokens issued or
    /// refused, approvals, limit changes). Empty from an older cua: this
    /// device.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// How `cloud_cost` groups the spend.
pub mod cost_by {
    /// One row per Space.
    pub const SPACE: &str = "space";
    /// One row per agent (`user` for what a person started).
    pub const AGENT: &str = "agent";
    /// One row per UTC day (`YYYY-MM-DD`).
    pub const DAY: &str = "day";
    /// One row per cloud.
    pub const CLOUD: &str = "cloud";
    /// Every grouping, in the order the docs list them.
    pub const ALL: &[&str] = &[SPACE, AGENT, DAY, CLOUD];
}

/// One row of `cloud_cost`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudCostRow {
    /// The Space, agent, day (`YYYY-MM-DD`) or cloud.
    pub key: String,
    /// Estimated US dollars (list price times runtime), to the cent.
    pub usd: f64,
    /// Machine hours, to the hundredth.
    pub hours: f64,
    /// How many machines ran.
    pub machines: u32,
}

/// `cloud_cost`: what the machines Cua created cost, estimated from list
/// price times runtime (the cloud's bill is the final word).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudCostReport {
    /// `space`, `agent`, `day` or `cloud`.
    pub by: String,
    /// The first day counted (`YYYY-MM-DD`, UTC).
    pub since: String,
    /// Up to when (RFC 3339).
    pub until: String,
    /// The rows: by day oldest first, otherwise the costliest first.
    pub rows: Vec<CloudCostRow>,
    /// The sum of the rows, to the cent.
    pub total_usd: f64,
}

/// A cloud's spend crossed a share of its monthly budget.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudBudgetAlert {
    /// `aws`, `gcp`, `hetzner`, `modal`.
    pub provider: String,
    /// `AWS`, `Google Cloud`, ...
    pub title: String,
    /// 50, 80 or 100.
    pub percent: u32,
    /// `YYYY-MM` (UTC).
    pub month: String,
    /// Estimated US dollars spent this month.
    pub spent_usd: f64,
    /// The monthly budget.
    pub budget_usd: f64,
}

impl CloudBudgetAlert {
    /// "AWS reached 80% of its budget".
    pub fn title_line(&self) -> String {
        if self.percent >= 100 {
            format!("{} reached its monthly budget", self.title)
        } else {
            format!("{} reached {}% of its budget", self.title, self.percent)
        }
    }

    /// One plain sentence with the figures and what happens next.
    pub fn body_line(&self) -> String {
        let figures = format!(
            "About ${:.2} of ${:.2} spent this month.",
            self.spent_usd, self.budget_usd
        );
        if self.percent >= 100 {
            format!(
                "{figures} New machines there are refused until next month; stopping and \
                 deleting still work. Raise it with `cua cloud connect {} --budget <usd>`.",
                self.provider
            )
        } else {
            figures
        }
    }
}

/// Where budget alerts go besides the audit log: the app's notification
/// feed, registered by the Spaces runtime. Takes the cua home and the alert.
pub type BudgetAlertSink =
    std::sync::Arc<dyn Fn(&std::path::Path, &CloudBudgetAlert) + Send + Sync>;

fn alert_sink() -> &'static std::sync::RwLock<Option<BudgetAlertSink>> {
    static SINK: std::sync::OnceLock<std::sync::RwLock<Option<BudgetAlertSink>>> =
        std::sync::OnceLock::new();
    SINK.get_or_init(Default::default)
}

/// Sets where budget alerts are posted (the last call wins).
pub fn set_budget_alert_sink(sink: BudgetAlertSink) {
    *alert_sink().write().expect("alert sink") = Some(sink);
}

/// Posts `alert` for the cua home `home` to the registered sink, if any;
/// whether one was registered.
pub fn post_budget_alert(home: &std::path::Path, alert: &CloudBudgetAlert) -> bool {
    let sink = alert_sink().read().expect("alert sink").clone();
    match sink {
        Some(f) => {
            f(home, alert);
            true
        }
        None => false,
    }
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

    /// `cloud_connect` with every option: federation (`auth`), limits,
    /// the check. Without `auth` (or `cli`) it is [`CloudManager::connect`]
    /// plus the limits.
    async fn connect_with(&self, req: &ConnectRequest) -> Result<CloudConnected> {
        if !req.is_cli() {
            return Err(not_built());
        }
        let mut c = self
            .connect(&req.target, req.make_default, req.ttl_hours)
            .await?;
        c.provider.status = status::CONNECTED.into();
        Ok(c)
    }

    /// `cloud_disconnect`: refuses while Spaces Cua created still run
    /// there (it lists them). With `delete`, deletes everything Cua created
    /// there first and forgets the connection only once the cloud confirms
    /// nothing is left.
    async fn disconnect(&self, provider: &str, delete: bool) -> Result<CloudDisconnected>;

    /// `cloud_approve`: a person approves (or, with `deny`, declines) what
    /// an agent asked for on `provider`: its link, or its raised limits.
    /// Refused when an agent calls.
    async fn approve(&self, provider: &str, deny: bool) -> Result<CloudProvider> {
        let _ = (provider, deny);
        Err(not_built())
    }

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

    /// `cloud_audit` with every option: `provider` narrows it to one cloud;
    /// clouds linked to the account also bring the account's entries
    /// (`source: account`). Oldest first, newest last.
    async fn audit_for(
        &self,
        provider: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CloudAuditEntry>> {
        let mut v = self.audit(limit).await?;
        if let Some(p) = provider {
            v.retain(|e| e.provider.eq_ignore_ascii_case(p));
        }
        Ok(v)
    }

    /// `cloud_cost`: what the machines Cua created cost since `since`
    /// (`YYYY-MM-DD`; default the first of this month), grouped `by`
    /// ([`cost_by`]).
    async fn cost(
        &self,
        _provider: Option<&str>,
        _by: &str,
        _since: Option<&str>,
    ) -> Result<CloudCostReport> {
        Err(not_built())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_and_named_google_trust_link_through_the_issuer() {
        let req = |provider: &str, f: &dyn Fn(&mut ConnectRequest)| {
            let mut r = ConnectRequest {
                target: CloudTarget {
                    provider: provider.into(),
                    ..Default::default()
                },
                ..Default::default()
            };
            f(&mut r);
            r
        };
        assert_eq!(req("aws", &|_| {}).auth_word(), "cli");
        assert!(req("gcp", &|_| {}).is_cli());
        assert_eq!(req("azure", &|_| {}).auth_word(), "oidc");
        assert_eq!(
            req("gcp", &|r| r.project_number = Some("123".into())).auth_word(),
            "oidc"
        );
        assert_eq!(
            req("gcp", &|r| r.service_account = Some(" ".into())).auth_word(),
            "cli"
        );
        assert_eq!(
            req("aws", &|r| r.auth = Some(" OIDC ".into())).auth_word(),
            "oidc"
        );
        assert_eq!(
            req("azure", &|r| r.auth = Some("sealed".into())).auth_word(),
            "sealed"
        );
    }
}
