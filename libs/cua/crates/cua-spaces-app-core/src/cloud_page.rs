// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The clouds on the Computers page: the "Clouds" group of the list (one row
//! per connected, waiting or refused cloud, then "Connect a cloud") and each
//! cloud's page (status, spend against the budget, what runs there, its
//! settings, what to fix, Disconnect), and the sheet that asks a person to
//! approve what their agent set up.
//!
//! Everything is read from `cloud_status` ([`input_from_status`]): the
//! providers' rows (`auth`, `status`, `budget_usd_month`, `max_size`,
//! `spent_month`, `running`, `approval`) and the machines Cua created
//! there. A fact `cloud_status` does not carry yet (who started a machine,
//! what it cost, who can open a sealed token) is simply not drawn.
//! Nothing here calls a cloud; the shells run the actions.

use serde::{Deserialize, Serialize};

use crate::host::HostConfirm;
use crate::machines_view::MachinesViewFact;

/// The group's title.
pub const CLOUDS_TITLE: &str = "Clouds";

/// The row at the end of the group.
pub const CONNECT_LABEL: &str = "Connect a cloud";

/// One cloud as `cloud_status` lists it (read leniently: [`input_from_status`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudInput {
    /// `aws`, `gcp`, `azure`, `hetzner`, `modal`.
    #[serde(default)]
    pub name: String,
    /// Connected (a link exists).
    #[serde(default)]
    pub connected: bool,
    /// `oidc`, `sealed`, `cli`; empty when not connected.
    #[serde(default)]
    pub auth: String,
    /// `connected`, `waiting-for-approval`, `failed`, `not-connected`.
    #[serde(default)]
    pub status: String,
    /// Region (a Hetzner location).
    #[serde(default)]
    pub region: String,
    /// Google Cloud project.
    #[serde(default)]
    pub project: String,
    /// Modal environment.
    #[serde(default)]
    pub environment: String,
    /// The account the link reaches (AWS account id).
    #[serde(default)]
    pub account: String,
    /// The monthly budget in cents.
    #[serde(default)]
    pub budget_cents: Option<u64>,
    /// The largest machine, `4x16`.
    #[serde(default)]
    pub max_size: String,
    /// Estimated cost this month, in cents.
    #[serde(default)]
    pub spent_cents: u64,
    /// Machines running there.
    #[serde(default)]
    pub running: u32,
    /// What waits for a person: `connection`, `limits` or empty.
    #[serde(default)]
    pub approval: String,
    /// The budget an agent asked for, in cents, while it waits.
    #[serde(default)]
    pub pending_budget_cents: Option<u64>,
    /// The size an agent asked for, while it waits.
    #[serde(default)]
    pub pending_max_size: String,
    /// The agent that set it up ("Claude Code on This Mac"), when known.
    #[serde(default)]
    pub agent: String,
    /// Who can open a sealed token ("This Mac", "Mac mini").
    #[serde(default)]
    pub holders: Vec<String>,
    /// This caller (a person) can approve or deny what waits
    /// (`cloud_approve`).
    #[serde(default)]
    pub can_approve: bool,
    /// `oidc` when the link could switch to linking without a stored key
    /// (an AWS link through the CLI sign-in).
    #[serde(default)]
    pub suggest_auth: String,
    /// What is wrong with the link and what to do, one sentence.
    #[serde(default)]
    pub issue: String,
}

/// One machine Cua created in a cloud.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudMachineInput {
    /// The cloud.
    #[serde(default)]
    pub provider: String,
    /// The cloud's id.
    #[serde(default)]
    pub id: String,
    /// The sandbox it belongs to (`aws:rl-env-0412`).
    #[serde(default)]
    pub sandbox: String,
    /// The cloud's word (`running`, `stopped`, ...).
    #[serde(default)]
    pub state: String,
    /// Who started it ("agent eval-runner", "ada on Mac mini"), when known.
    #[serde(default)]
    pub started_by: String,
    /// What it cost so far, in cents, when known.
    #[serde(default)]
    pub cost_cents: Option<u64>,
}

/// What the clouds are drawn from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudsInput {
    /// The clouds, as `cloud_status` lists them.
    #[serde(default)]
    pub providers: Vec<CloudInput>,
    /// What Cua created in them.
    #[serde(default)]
    pub machines: Vec<CloudMachineInput>,
    /// This app can approve every cloud's waiting link (the demo host;
    /// real hosts say so per cloud, [`CloudInput::can_approve`]).
    #[serde(default)]
    pub can_approve: bool,
    /// Now, Unix seconds (the month, and when it resets).
    #[serde(default)]
    pub now: u64,
}

/// One row of the Clouds group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRow {
    /// `aws`.
    pub id: String,
    /// "AWS".
    pub name: String,
    /// The vendor's logo (`aws`, `gcp`, `azure`, `hetzner`, `modal`).
    pub logo: String,
    /// One short line: "3 running", "Budget reached", "Not trusted".
    pub line: String,
    /// `ok`, `warn` or `fail`.
    pub health: String,
}

/// The Clouds group of the list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudsGroup {
    /// "Clouds".
    pub title: String,
    /// The connected, waiting and refused clouds.
    pub rows: Vec<CloudRow>,
    /// "Connect a cloud".
    pub connect_label: String,
}

/// A button on the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAction {
    /// `raise-budget`, `set-up-again`, `new-token`, `review` or `open`.
    pub id: String,
    /// "Raise budget…".
    pub label: String,
    /// A page to open (`open`).
    pub url: Option<String>,
}

/// What is wrong, in one sentence, with its fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudIssue {
    /// `budget`, `trust`, `token`, `machines`, `approval`.
    pub id: String,
    /// `fail` or `warn`.
    pub level: String,
    /// The sentence.
    pub text: String,
    /// Its one action.
    pub action: Option<CloudAction>,
}

/// Spend against the budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSpend {
    /// "$42.18".
    pub amount: String,
    /// "of $200 budget this month".
    pub of_budget: String,
    /// "Resets Nov 1".
    pub resets: String,
    /// The bar, 0 to 100.
    pub percent: u32,
    /// `ok` or `warn` (the budget is used).
    pub tone: String,
    /// One line under the bar.
    pub note: String,
}

/// A machine Cua runs there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudMachine {
    /// The cloud's id.
    pub id: String,
    /// "rl-env-0412".
    pub name: String,
    /// Who started it; empty when not known.
    pub by: String,
    /// "$0.08"; empty when not known.
    pub cost: String,
}

/// The machines list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRunning {
    /// "Running in AWS".
    pub title: String,
    /// The machines.
    pub machines: Vec<CloudMachine>,
}

/// A sealed token's section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudToken {
    /// "API token".
    pub title: String,
    /// "Sealed to your devices" ("Refused by Hetzner").
    pub lock: String,
    /// What sealed means.
    pub note: String,
    /// "Can open it"; empty with no holders.
    pub holders_label: String,
    /// "This Mac, Mac mini and Linux box".
    pub holders: String,
    /// "Revoke".
    pub revoke_label: String,
    /// Where to revoke it.
    pub revoke_help: String,
    /// "Replace token…".
    pub replace: CloudAction,
}

/// What the cloud is set to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSettingRow {
    /// `region`, `budget`, `size`, `access`.
    pub id: String,
    /// "Monthly budget".
    pub label: String,
    /// A line under the label.
    pub help: String,
    /// "$200".
    pub value: String,
}

/// "Approve AWS for your agent?".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudApproval {
    /// "Approve AWS for your agent?".
    pub title: String,
    /// Who set it up, and that nothing runs yet.
    pub intro: String,
    /// Account, budget, size, access.
    pub rows: Vec<MachinesViewFact>,
    /// "Agents can't raise the budget or the size. Only you can, here."
    pub note: String,
    /// "Approve".
    pub approve_label: String,
    /// "Deny".
    pub deny_label: String,
}

/// Disconnect, and what it asks first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudDisconnect {
    /// "Disconnect AWS…".
    pub label: String,
    /// The question.
    pub confirm: HostConfirm,
    /// Delete what Cua created there first (`cloud_disconnect` with `delete`).
    pub delete: bool,
}

/// One cloud's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPage {
    /// `aws`.
    pub id: String,
    /// "AWS".
    pub name: String,
    /// The vendor's logo.
    pub logo: String,
    /// "Connected", "Budget reached", "Not trusted", "Token refused",
    /// "Waiting for approval".
    pub status: String,
    /// `ok`, `warn` or `fail`.
    pub health: String,
    /// "Issues".
    pub issues_title: String,
    /// What is wrong.
    pub issues: Vec<CloudIssue>,
    /// Spend against the budget.
    pub spend: Option<CloudSpend>,
    /// What runs there.
    pub running: Option<CloudRunning>,
    /// A sealed token's section.
    pub token: Option<CloudToken>,
    /// "Settings".
    pub settings_title: String,
    /// Region, budget, size, access.
    pub settings: Vec<CloudSettingRow>,
    /// "Change limits…" (a connected cloud with a budget).
    pub limits: Option<CloudAction>,
    /// "Switch to OIDC": a CLI sign-in link that could link without a
    /// stored key (`switch-oidc`, opens the cloud's sheet).
    pub switch_auth: Option<CloudAction>,
    /// The approval sheet (a host that can approve).
    pub approval: Option<CloudApproval>,
    /// Disconnect.
    pub disconnect: Option<CloudDisconnect>,
}

/// The clouds as the Computers page draws them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudsView {
    /// The list's group.
    pub group: CloudsGroup,
    /// Each row's page.
    pub pages: Vec<CloudPage>,
}

/// `cloud_status` (its JSON) as the page reads it. Lenient: a missing or
/// odd field is left empty.
pub fn input_from_status(status: &serde_json::Value, now: u64, can_approve: bool) -> CloudsInput {
    let text = |v: &serde_json::Value, k: &str| {
        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    let cents = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.as_f64())
            .filter(|d| d.is_finite() && *d >= 0.0)
            .map(|d| (d * 100.0).round() as u64)
    };
    let providers = status
        .get("providers")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .map(|p| CloudInput {
                    name: text(p, "name"),
                    connected: p
                        .get("connected")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false),
                    auth: text(p, "auth"),
                    status: text(p, "status"),
                    region: text(p, "region"),
                    project: text(p, "project"),
                    environment: text(p, "environment"),
                    account: text(p, "account"),
                    budget_cents: cents(p, "budget_usd_month"),
                    max_size: text(p, "max_size"),
                    spent_cents: cents(p, "spent_month").unwrap_or(0),
                    running: p.get("running").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                    approval: text(p, "approval"),
                    pending_budget_cents: cents(p, "pending_budget_usd_month"),
                    pending_max_size: text(p, "pending_max_size"),
                    agent: text(p, "approval_agent"),
                    can_approve: p
                        .get("can_approve")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false),
                    suggest_auth: text(p, "suggest_auth"),
                    issue: text(p, "issue"),
                    holders: p
                        .get("holders")
                        .and_then(|h| h.as_array())
                        .map(|h| {
                            h.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let machines = status
        .get("resources")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter(|r| matches!(text(r, "type").as_str(), "instance" | "sandbox"))
                .map(|r| CloudMachineInput {
                    provider: text(r, "provider"),
                    id: text(r, "id"),
                    sandbox: text(r, "sandbox"),
                    state: text(r, "state"),
                    started_by: text(r, "started_by"),
                    cost_cents: cents(r, "cost_usd"),
                })
                .collect()
        })
        .unwrap_or_default();
    CloudsInput {
        providers,
        machines,
        can_approve,
        now,
    }
}

/// "AWS", "Google Cloud", ...; empty for a cloud this build does not know.
pub fn cloud_name(id: &str) -> &'static str {
    match id {
        "aws" => "AWS",
        "gcp" => "Google Cloud",
        "azure" => "Microsoft Azure",
        "hetzner" => "Hetzner",
        "modal" => "Modal",
        _ => "",
    }
}

/// What a page shows the person for a cloud.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Connected,
    BudgetReached,
    NotTrusted,
    TokenRefused,
    /// The link was revoked elsewhere (the host's `issue` says what is left).
    Disconnected,
    Waiting,
}

fn state_of(c: &CloudInput) -> State {
    // A raise of a working link's limits waits without stopping the link.
    if c.approval == "connection" || c.status == "waiting-for-approval" && c.approval != "limits" {
        State::Waiting
    } else if c.status == "failed" {
        if !c.issue.is_empty() {
            State::Disconnected
        } else if c.auth == "oidc" {
            State::NotTrusted
        } else {
            State::TokenRefused
        }
    } else if c.budget_cents.is_some_and(|b| b > 0 && c.spent_cents >= b) {
        State::BudgetReached
    } else {
        State::Connected
    }
}

fn status_text(s: State) -> &'static str {
    match s {
        State::Connected => "Connected",
        State::BudgetReached => "Budget reached",
        State::NotTrusted => "Not trusted",
        State::TokenRefused => "Token refused",
        State::Disconnected => "Disconnected",
        State::Waiting => "Waiting for approval",
    }
}

fn health_of(s: State) -> &'static str {
    match s {
        State::Connected => "ok",
        State::BudgetReached | State::Waiting => "warn",
        State::NotTrusted | State::TokenRefused | State::Disconnected => "fail",
    }
}

/// "$42.18", or "$200" for whole dollars.
fn money(cents: u64) -> String {
    if cents.is_multiple_of(100) {
        format!("${}", with_commas(cents / 100))
    } else {
        format!("${}.{:02}", with_commas(cents / 100), cents % 100)
    }
}

fn with_commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `4x16` as "4 vCPU, 16 GB"; the text itself when it is not that shape.
pub fn size_label(size: &str) -> String {
    match size.split_once('x') {
        Some((c, m)) if c.parse::<u32>().is_ok() && m.parse::<u32>().is_ok() => {
            format!("{c} vCPU, {m} GB")
        }
        _ => size.to_string(),
    }
}

/// "us-west-2 (Oregon)" for the regions people pick; the code otherwise.
pub fn region_label(region: &str) -> String {
    let place = match region {
        "us-east-1" => "N. Virginia",
        "us-east-2" => "Ohio",
        "us-west-1" => "N. California",
        "us-west-2" => "Oregon",
        "eu-west-1" => "Ireland",
        "eu-central-1" => "Frankfurt",
        "ap-southeast-1" => "Singapore",
        "ap-northeast-1" => "Tokyo",
        "fsn1" => "Falkenstein",
        "nbg1" => "Nuremberg",
        "hel1" => "Helsinki",
        "ash" => "Ashburn",
        "hil" => "Hillsboro",
        _ => "",
    };
    if place.is_empty() {
        region.to_string()
    } else {
        format!("{region} ({place})")
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// (year, month 1-12, day 1-31, seconds into the day) of Unix seconds, UTC.
fn civil(now: u64) -> (i64, u32, u32, u64) {
    let days = (now / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d, now % 86_400)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// "Nov 1": the first of next month.
fn resets_on(now: u64) -> String {
    let (_, m, _, _) = civil(now);
    format!("{} 1", MONTHS[(m % 12) as usize])
}

/// What the month adds up to at this pace, in cents; `None` before the
/// first day is over or with nothing spent.
fn projected_cents(spent: u64, now: u64) -> Option<u64> {
    let (y, m, d, secs) = civil(now);
    let elapsed = (d - 1) as f64 + secs as f64 / 86_400.0;
    if spent == 0 || elapsed < 1.0 {
        return None;
    }
    Some((spent as f64 / elapsed * days_in_month(y, m) as f64).round() as u64)
}

/// "Oct 31".
fn month_end(now: u64) -> String {
    let (y, m, _, _) = civil(now);
    format!("{} {}", MONTHS[(m - 1) as usize], days_in_month(y, m))
}

fn live(m: &CloudMachineInput) -> bool {
    !matches!(m.state.as_str(), "gone" | "terminated" | "stopped")
}

fn machine_name(m: &CloudMachineInput) -> String {
    match m.sandbox.split_once(':') {
        Some((_, name)) if !name.is_empty() => name.to_string(),
        _ if !m.sandbox.is_empty() => m.sandbox.clone(),
        _ => m.id.clone(),
    }
}

fn shown(c: &CloudInput) -> bool {
    !cloud_name(&c.name).is_empty()
        && (c.connected
            || c.approval == "connection"
            || matches!(c.status.as_str(), "waiting-for-approval" | "failed"))
}

fn servers(n: usize, cloud: &str) -> String {
    let what = if cloud == "modal" {
        "sandbox"
    } else if cloud == "hetzner" {
        "server"
    } else {
        "machine"
    };
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn console_url(c: &CloudInput) -> Option<(String, String)> {
    match c.name.as_str() {
        "aws" => {
            let region = if c.region.is_empty() {
                "us-east-1"
            } else {
                &c.region
            };
            Some((
                "Open EC2".into(),
                format!(
                    "https://{region}.console.aws.amazon.com/ec2/home?region={region}#Instances:tag:cua-managed=true"
                ),
            ))
        }
        "hetzner" => Some((
            "Open Hetzner Console".into(),
            "https://console.hetzner.cloud".into(),
        )),
        "gcp" => Some((
            "Open Compute Engine".into(),
            "https://console.cloud.google.com/compute/instances".into(),
        )),
        "modal" => Some(("Open Modal".into(), "https://modal.com/apps".into())),
        _ => None,
    }
}

fn settings_of(c: &CloudInput) -> Vec<CloudSettingRow> {
    let mut rows = vec![];
    let (id, label, value) = match c.name.as_str() {
        "hetzner" => ("region", "Location", region_label(&c.region)),
        "gcp" => ("region", "Project", c.project.clone()),
        "modal" => ("region", "Environment", c.environment.clone()),
        _ => ("region", "Region", region_label(&c.region)),
    };
    if !value.is_empty() {
        rows.push(CloudSettingRow {
            id: id.into(),
            label: label.into(),
            help: String::new(),
            value,
        });
    }
    if let Some(b) = c.budget_cents {
        rows.push(CloudSettingRow {
            id: "budget".into(),
            label: "Monthly budget".into(),
            help: "Cua stops creating machines at the budget.".into(),
            value: money(b),
        });
    }
    if !c.max_size.is_empty() {
        rows.push(CloudSettingRow {
            id: "size".into(),
            label: "Largest machine".into(),
            help: String::new(),
            value: size_label(&c.max_size),
        });
    }
    if c.connected && c.auth == "cli" && c.suggest_auth == "oidc" {
        rows.push(CloudSettingRow {
            id: "access".into(),
            label: "Access".into(),
            help: "The AWS CLI's sign-in on this machine".into(),
            value: String::new(),
        });
    }
    if c.auth == "oidc" {
        rows.push(CloudSettingRow {
            id: "access".into(),
            label: "Access".into(),
            help: match c.name.as_str() {
                "aws" => "IAM role cua-spaces, trusted through Cua's OIDC issuer".into(),
                _ => "Trusted through Cua's OIDC issuer".into(),
            },
            value: String::new(),
        });
    }
    rows
}

/// Whether this app may approve `c` (the cloud says so, or the host does).
fn approves(c: &CloudInput, input: &CloudsInput) -> bool {
    c.can_approve || input.can_approve
}

fn approval_of(c: &CloudInput, input: &CloudsInput) -> Option<CloudApproval> {
    if !approves(c, input) || c.approval.is_empty() && c.status != "waiting-for-approval" {
        return None;
    }
    let name = cloud_name(&c.name);
    let who = if c.agent.is_empty() {
        "Your agent".to_string()
    } else {
        c.agent.clone()
    };
    let limits = c.approval == "limits";
    let budget = if limits {
        c.pending_budget_cents.or(c.budget_cents)
    } else {
        c.budget_cents
    };
    let size = if limits && !c.pending_max_size.is_empty() {
        &c.pending_max_size
    } else {
        &c.max_size
    };
    let mut rows = vec![];
    if !c.account.is_empty() {
        rows.push(MachinesViewFact {
            label: "Account".into(),
            value: mask_account(&c.account),
        });
    }
    if let Some(b) = budget {
        rows.push(MachinesViewFact {
            label: "Monthly budget".into(),
            value: money(b),
        });
    }
    if !size.is_empty() {
        rows.push(MachinesViewFact {
            label: "Largest machine".into(),
            value: size_label(size),
        });
    }
    rows.push(MachinesViewFact {
        label: "Access".into(),
        value: "Only machines Cua tags".into(),
    });
    Some(CloudApproval {
        title: if limits {
            format!("Approve new {name} limits?")
        } else {
            format!("Approve {name} for your agent?")
        },
        intro: if limits {
            format!(
                "{who} asked to raise the limits on {name}. They stay as they are until you approve."
            )
        } else {
            format!("{who} set up {name}. Cua checked it; nothing runs until you approve.")
        },
        rows,
        note: "Agents can't raise the budget or the size. Only you can, here.".into(),
        approve_label: "Approve".into(),
        deny_label: "Deny".into(),
    })
}

/// "2109…4321".
fn mask_account(a: &str) -> String {
    let chars: Vec<char> = a.chars().collect();
    if chars.len() > 8 {
        format!(
            "{}…{}",
            chars[..4].iter().collect::<String>(),
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    } else {
        a.to_string()
    }
}

fn page(c: &CloudInput, input: &CloudsInput) -> CloudPage {
    let name = cloud_name(&c.name);
    let state = state_of(c);
    let machines: Vec<CloudMachine> = input
        .machines
        .iter()
        .filter(|m| m.provider == c.name && live(m))
        .map(|m| CloudMachine {
            id: m.id.clone(),
            name: machine_name(m),
            by: m.started_by.clone(),
            cost: m.cost_cents.map(money).unwrap_or_default(),
        })
        .collect();
    let n = machines.len().max(c.running as usize);

    let mut issues = vec![];
    match state {
        State::BudgetReached => issues.push(CloudIssue {
            id: "budget".into(),
            level: "warn".into(),
            text: format!(
                "This month's {} budget is used. New Spaces go to your machines, or wait until {}.",
                money(c.budget_cents.unwrap_or(0)),
                resets_on(input.now)
            ),
            action: Some(CloudAction {
                id: "raise-budget".into(),
                label: "Raise budget…".into(),
                url: None,
            }),
        }),
        // The host's sentence (a failed link with an `issue`) says it all.
        State::Disconnected => {
            issues.push(CloudIssue {
                id: "link".into(),
                level: "fail".into(),
                text: c.issue.clone(),
                action: Some(CloudAction {
                    id: "reconnect".into(),
                    label: "Reconnect…".into(),
                    url: None,
                }),
            })
        }
        State::NotTrusted => issues.push(CloudIssue {
            id: "trust".into(),
            level: "fail".into(),
            text: match c.name.as_str() {
                "aws" => "AWS refused Cua's sign-in: the role cua-spaces or its OIDC provider was deleted.".into(),
                _ => format!("{name} refused Cua's sign-in: the trust was deleted."),
            },
            action: Some(CloudAction {
                id: "set-up-again".into(),
                label: "Set up again…".into(),
                url: None,
            }),
        }),
        State::TokenRefused => issues.push(CloudIssue {
            id: "token".into(),
            level: "fail".into(),
            text: format!(
                "The API token was revoked or deleted in {name}. Cua can't create, stop or delete servers."
            ),
            action: Some(CloudAction {
                id: "new-token".into(),
                label: "Paste a new token…".into(),
                url: None,
            }),
        }),
        State::Waiting => issues.push(CloudIssue {
            id: "approval".into(),
            level: "warn".into(),
            text: if approves(c, input) {
                format!("Your agent set up {name}. It waits for your approval.")
            } else {
                format!("Your agent set up {name}. Nothing runs there until it is approved.")
            },
            action: approves(c, input).then(|| CloudAction {
                id: "review".into(),
                label: "Review…".into(),
                url: None,
            }),
        }),
        State::Connected if !c.issue.is_empty() => issues.push(CloudIssue {
            id: "link".into(),
            level: "warn".into(),
            text: c.issue.clone(),
            action: Some(CloudAction {
                id: "reconnect".into(),
                label: "Reconnect…".into(),
                url: None,
            }),
        }),
        State::Connected => {}
    }
    if matches!(state, State::Connected | State::BudgetReached) && c.approval == "limits" {
        issues.push(CloudIssue {
            id: "approval".into(),
            level: "warn".into(),
            text: format!(
                "{} asked to raise the limits on {name}. They stay as they are until {}.",
                if c.agent.is_empty() {
                    "Your agent"
                } else {
                    &c.agent
                },
                if approves(c, input) {
                    "you approve"
                } else {
                    "a person approves"
                }
            ),
            action: approves(c, input).then(|| CloudAction {
                id: "review".into(),
                label: "Review…".into(),
                url: None,
            }),
        });
    }
    // The daemon's own sentence already counts what may still run.
    if matches!(state, State::NotTrusted | State::TokenRefused) && n > 0 && c.issue.is_empty() {
        let open = console_url(c);
        issues.push(CloudIssue {
            id: "machines".into(),
            level: "warn".into(),
            text: if state == State::NotTrusted {
                format!(
                    "{} Cua created {} running and billing. Cua can't stop {} until it's trusted again.",
                    servers(n, &c.name),
                    if n == 1 { "keeps" } else { "keep" },
                    if n == 1 { "it" } else { "them" }
                )
            } else {
                format!(
                    "{} Cua created {} running and billing. Stop {} in {name}, or paste a new token and Cua stops {} at {} time limit.",
                    servers(n, &c.name),
                    if n == 1 { "keeps" } else { "keep" },
                    if n == 1 { "it" } else { "them" },
                    if n == 1 { "it" } else { "them" },
                    if n == 1 { "its" } else { "their" }
                )
            },
            action: open.map(|(label, url)| CloudAction {
                id: "open".into(),
                label,
                url: Some(url),
            }),
        });
    }

    let failed = matches!(
        state,
        State::NotTrusted | State::TokenRefused | State::Disconnected
    );
    let spend = match (c.budget_cents, failed || state == State::Waiting) {
        (Some(b), false) if b > 0 => {
            let used = state == State::BudgetReached;
            let percent = ((c.spent_cents as f64 / b as f64) * 100.0)
                .round()
                .clamp(0.0, 100.0) as u32;
            let note = if used {
                format!(
                    "New machines wait until {} or a higher budget.",
                    resets_on(input.now)
                )
            } else {
                match projected_cents(c.spent_cents, input.now) {
                    Some(p) if p > c.spent_cents => format!(
                        "About {} by {} at this pace. An estimate from list prices.",
                        money(p),
                        month_end(input.now)
                    ),
                    _ => "An estimate from list prices.".into(),
                }
            };
            Some(CloudSpend {
                amount: money(c.spent_cents),
                of_budget: format!("of {} budget this month", money(b)),
                resets: format!("Resets {}", resets_on(input.now)),
                percent,
                tone: if used { "warn" } else { "ok" }.into(),
                note,
            })
        }
        _ => None,
    };

    let running = (!machines.is_empty()).then(|| CloudRunning {
        title: match state {
            State::BudgetReached => "Still running (they end at their time limit)".into(),
            State::NotTrusted | State::TokenRefused | State::Disconnected => {
                "Last seen running".into()
            }
            _ => format!("Running in {name}"),
        },
        machines,
    });

    let token = (c.auth == "sealed").then(|| CloudToken {
        title: "API token".into(),
        lock: if state == State::TokenRefused {
            format!("Refused by {name}")
        } else {
            "Sealed to your machines".into()
        },
        note: "Only your own devices can open it. Cua's servers keep a sealed copy they can't read. Agents use it through Cua; they never see it.".into(),
        holders_label: if c.holders.is_empty() { String::new() } else { "Can open it".into() },
        holders: join_and(&c.holders),
        revoke_label: "Revoke".into(),
        revoke_help: format!("Delete it in {name} Console, Security, API tokens. Disconnect also removes every sealed copy."),
        replace: CloudAction {
            id: "new-token".into(),
            label: "Replace token…".into(),
            url: None,
        },
    });

    CloudPage {
        id: c.name.clone(),
        name: name.into(),
        logo: c.name.clone(),
        status: status_text(state).into(),
        health: health_of(state).into(),
        issues_title: "Issues".into(),
        issues,
        spend,
        running,
        token,
        settings_title: "Settings".into(),
        limits: (matches!(state, State::Connected | State::BudgetReached)
            && c.budget_cents.is_some()
            && c.name != "gcp"
            && c.name != "azure")
            .then(|| CloudAction {
                id: "change-limits".into(),
                label: "Change limits…".into(),
                url: None,
            }),
        switch_auth: (c.connected && c.auth != "oidc" && c.suggest_auth == "oidc" && !failed).then(
            || CloudAction {
                id: "switch-oidc".into(),
                label: "Switch to OIDC".into(),
                url: None,
            },
        ),
        settings: if state == State::Waiting && !approves(c, input) {
            vec![]
        } else {
            settings_of(c)
        },
        approval: approval_of(c, input),
        disconnect: Some(CloudDisconnect {
            label: format!("Disconnect {name}…"),
            confirm: HostConfirm {
                title: format!("Disconnect {name}?"),
                message: disconnect_message(c, n),
                confirm_label: if n > 0 {
                    format!("Delete {n} and disconnect")
                } else {
                    "Disconnect".into()
                },
                cancel_label: "Cancel".into(),
            },
            delete: n > 0 && !failed,
        }),
    }
}

fn disconnect_message(c: &CloudInput, n: usize) -> String {
    let name = cloud_name(&c.name);
    let what = servers(n, &c.name);
    let failed = c.status == "failed";
    let removing = match c.auth.as_str() {
        "oidc" if c.name == "aws" => {
            " To remove the trust too, delete the stack cua-spaces in AWS.".to_string()
        }
        "oidc" => format!(" To remove the trust too, delete it in {name}."),
        "sealed" => format!(" To revoke the token too, delete it in {name} Console."),
        _ => String::new(),
    };
    if n == 0 {
        format!("Cua stops using {name}.{removing}")
    } else if failed {
        format!(
            "Cua can't reach {name}, so the {what} it runs there stay until you delete them. Cua stops using {name}.{removing}"
        )
    } else {
        format!("Cua deletes the {what} it runs there, then stops using {name}.{removing}")
    }
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn line_of(c: &CloudInput, state: State) -> String {
    match state {
        State::Connected if c.running == 0 => "Nothing running".into(),
        State::Connected => format!("{} running", c.running),
        s => status_text(s).into(),
    }
}

/// The clouds as the Computers page draws them: the group and each row's page.
pub fn view(input: &CloudsInput) -> CloudsView {
    let shown: Vec<&CloudInput> = input.providers.iter().filter(|c| shown(c)).collect();
    CloudsView {
        group: CloudsGroup {
            title: CLOUDS_TITLE.into(),
            rows: shown
                .iter()
                .map(|c| {
                    let state = state_of(c);
                    CloudRow {
                        id: c.name.clone(),
                        name: cloud_name(&c.name).into(),
                        logo: c.name.clone(),
                        line: line_of(c, state),
                        health: health_of(state).into(),
                    }
                })
                .collect(),
            connect_label: CONNECT_LABEL.into(),
        },
        pages: shown.iter().map(|c| page(c, input)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oct 24, 2026, 12:00 UTC.
    const NOW: u64 = 1_792_843_200;

    fn aws() -> CloudInput {
        CloudInput {
            name: "aws".into(),
            connected: true,
            auth: "oidc".into(),
            status: "connected".into(),
            region: "us-west-2".into(),
            account: "210987654321".into(),
            budget_cents: Some(20_000),
            max_size: "4x16".into(),
            spent_cents: 4_218,
            running: 3,
            ..Default::default()
        }
    }

    fn input(c: CloudInput) -> CloudsInput {
        CloudsInput {
            providers: vec![c],
            machines: vec![CloudMachineInput {
                provider: "aws".into(),
                id: "i-1".into(),
                sandbox: "aws:rl-env-0412".into(),
                state: "running".into(),
                started_by: "agent eval-runner".into(),
                cost_cents: Some(8),
            }],
            can_approve: false,
            now: NOW,
        }
    }

    #[test]
    fn dates_come_from_unix_seconds() {
        assert_eq!(civil(NOW).0, 2026);
        assert_eq!((civil(NOW).1, civil(NOW).2), (10, 24));
        assert_eq!(resets_on(NOW), "Nov 1");
        assert_eq!(resets_on(NOW + 40 * 86_400), "Jan 1");
        assert_eq!(month_end(NOW), "Oct 31");
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
    }

    #[test]
    fn a_connected_cloud_shows_spend_machines_and_settings() {
        let v = view(&input(aws()));
        assert_eq!(v.group.title, "Clouds");
        assert_eq!(v.group.connect_label, "Connect a cloud");
        assert_eq!(v.group.rows.len(), 1);
        let row = &v.group.rows[0];
        assert_eq!(
            (row.name.as_str(), row.line.as_str(), row.health.as_str()),
            ("AWS", "3 running", "ok")
        );
        let p = &v.pages[0];
        assert_eq!(p.status, "Connected");
        assert!(p.issues.is_empty());
        let s = p.spend.as_ref().unwrap();
        assert_eq!(s.amount, "$42.18");
        assert_eq!(s.of_budget, "of $200 budget this month");
        assert_eq!(s.resets, "Resets Nov 1");
        assert_eq!(s.percent, 21);
        assert!(s.note.starts_with("About $"), "{}", s.note);
        let r = p.running.as_ref().unwrap();
        assert_eq!(r.title, "Running in AWS");
        assert_eq!(r.machines[0].name, "rl-env-0412");
        assert_eq!(r.machines[0].by, "agent eval-runner");
        assert_eq!(r.machines[0].cost, "$0.08");
        let labels: Vec<_> = p
            .settings
            .iter()
            .map(|x| (x.label.as_str(), x.value.as_str()))
            .collect();
        assert_eq!(
            labels,
            [
                ("Region", "us-west-2 (Oregon)"),
                ("Monthly budget", "$200"),
                ("Largest machine", "4 vCPU, 16 GB"),
                ("Access", "")
            ]
        );
        assert!(p.token.is_none() && p.approval.is_none());
        assert_eq!(p.limits.as_ref().unwrap().label, "Change limits…");
        let d = p.disconnect.as_ref().unwrap();
        assert_eq!(d.label, "Disconnect AWS…");
        assert_eq!(d.confirm.confirm_label, "Delete 3 and disconnect");
        assert!(d.delete);
    }

    #[test]
    fn a_used_budget_says_so_and_offers_to_raise_it() {
        let mut c = aws();
        c.spent_cents = 20_000;
        let v = view(&input(c));
        let p = &v.pages[0];
        assert_eq!(
            (p.status.as_str(), p.health.as_str()),
            ("Budget reached", "warn")
        );
        assert_eq!(v.group.rows[0].line, "Budget reached");
        assert_eq!(p.issues[0].action.as_ref().unwrap().id, "raise-budget");
        assert!(p.issues[0].text.contains("until Nov 1"));
        assert_eq!(p.spend.as_ref().unwrap().tone, "warn");
        assert_eq!(p.spend.as_ref().unwrap().percent, 100);
        assert!(
            p.running
                .as_ref()
                .unwrap()
                .title
                .starts_with("Still running")
        );
    }

    #[test]
    fn lost_trust_and_a_refused_token_name_what_still_bills() {
        let mut c = aws();
        c.status = "failed".into();
        let v = view(&input(c));
        let p = &v.pages[0];
        assert_eq!(
            (p.status.as_str(), p.health.as_str()),
            ("Not trusted", "fail")
        );
        assert_eq!(p.issues[0].action.as_ref().unwrap().id, "set-up-again");
        assert!(
            p.issues[1]
                .text
                .starts_with("3 machines Cua created keep running and billing")
        );
        assert!(
            p.issues[1]
                .action
                .as_ref()
                .unwrap()
                .url
                .as_deref()
                .unwrap()
                .contains("ec2/home?region=us-west-2")
        );
        assert!(p.spend.is_none());
        assert_eq!(p.running.as_ref().unwrap().title, "Last seen running");
        assert!(!p.disconnect.as_ref().unwrap().delete);

        let h = CloudInput {
            name: "hetzner".into(),
            connected: true,
            auth: "sealed".into(),
            status: "failed".into(),
            region: "fsn1".into(),
            running: 2,
            holders: vec!["This Mac".into(), "Mac mini".into(), "Linux box".into()],
            ..Default::default()
        };
        let v = view(&CloudsInput {
            providers: vec![h],
            now: NOW,
            ..Default::default()
        });
        let p = &v.pages[0];
        assert_eq!(
            (p.status.as_str(), v.group.rows[0].line.as_str()),
            ("Token refused", "Token refused")
        );
        assert_eq!(
            p.issues[0].action.as_ref().unwrap().label,
            "Paste a new token…"
        );
        assert!(p.issues[1].text.starts_with("2 servers Cua created"));
        let t = p.token.as_ref().unwrap();
        assert_eq!(t.lock, "Refused by Hetzner");
        assert_eq!(t.holders, "This Mac, Mac mini and Linux box");
    }

    #[test]
    fn an_agents_link_waits_and_the_sheet_needs_a_host_that_can_approve() {
        let mut c = aws();
        c.status = "waiting-for-approval".into();
        c.approval = "connection".into();
        c.agent = "Claude Code on This Mac".into();
        c.spent_cents = 0;
        c.running = 0;
        let mut i = input(c);
        i.machines.clear();
        let v = view(&i);
        let p = &v.pages[0];
        assert_eq!(p.status, "Waiting for approval");
        assert!(p.approval.is_none());
        assert!(p.issues[0].action.is_none());
        i.can_approve = true;
        let v = view(&i);
        let p = &v.pages[0];
        assert_eq!(p.issues[0].action.as_ref().unwrap().id, "review");
        let a = p.approval.as_ref().unwrap();
        assert_eq!(a.title, "Approve AWS for your agent?");
        assert!(a.intro.starts_with("Claude Code on This Mac set up AWS"));
        assert_eq!(a.rows[0].value, "2109…4321");
        assert_eq!(
            (a.approve_label.as_str(), a.deny_label.as_str()),
            ("Approve", "Deny")
        );
    }

    #[test]
    fn clouds_that_are_not_connected_are_not_listed() {
        let off = CloudInput {
            name: "gcp".into(),
            status: "not-connected".into(),
            ..Default::default()
        };
        let v = view(&CloudsInput {
            providers: vec![off],
            now: NOW,
            ..Default::default()
        });
        assert!(v.group.rows.is_empty() && v.pages.is_empty());
        assert_eq!(v.group.connect_label, "Connect a cloud");
    }

    #[test]
    fn money_has_cents_only_when_it_needs_them() {
        assert_eq!(money(20_000), "$200");
        assert_eq!(money(4_218), "$42.18");
        assert_eq!(money(123_456_700), "$1,234,567");
    }

    #[test]
    fn cloud_status_json_is_read_leniently() {
        let status = serde_json::json!({
            "providers": [
                {"name": "aws", "connected": true, "auth": "oidc", "status": "connected", "region": "us-west-2",
                 "budget_usd_month": 200.0, "max_size": "4x16", "spent_month": 42.18, "running": 3, "approval": ""},
                {"name": "modal"}
            ],
            "resources": [
                {"provider": "aws", "id": "i-1", "type": "instance", "sandbox": "aws:a", "state": "running", "cost_usd": 0.08},
                {"provider": "aws", "id": "sg-1", "type": "security_group"}
            ]
        });
        let i = input_from_status(&status, NOW, false);
        assert_eq!(i.providers.len(), 2);
        assert_eq!(i.providers[0].budget_cents, Some(20_000));
        assert_eq!(i.providers[0].spent_cents, 4_218);
        assert_eq!(i.machines.len(), 1);
        assert_eq!(i.machines[0].cost_cents, Some(8));
        assert!(
            input_from_status(&serde_json::Value::Null, NOW, false)
                .providers
                .is_empty()
        );
    }

    #[test]
    fn a_cli_aws_link_offers_the_switch_to_oidc_and_nothing_else_changes() {
        let mut c = aws();
        c.auth = "cli".into();
        c.suggest_auth = "oidc".into();
        let v = view(&input(c.clone()));
        let p = &v.pages[0];
        let s = p.switch_auth.as_ref().expect("switch offered");
        assert_eq!(
            (s.id.as_str(), s.label.as_str()),
            ("switch-oidc", "Switch to OIDC")
        );
        assert!(p.issues.is_empty());
        assert_eq!(p.status, "Connected");
        let access = p.settings.iter().find(|r| r.id == "access").unwrap();
        assert_eq!(access.help, "The AWS CLI's sign-in on this machine");
        // An OIDC link, or a CLI link with no suggestion, offers nothing.
        assert!(view(&input(aws())).pages[0].switch_auth.is_none());
        c.suggest_auth.clear();
        let p = view(&input(c)).pages[0].clone();
        assert!(p.switch_auth.is_none());
        assert!(p.settings.iter().all(|r| r.id != "access"));
    }

    #[test]
    fn the_hosts_issue_sentence_replaces_the_generic_one_with_reconnect() {
        let mut c = aws();
        c.status = "failed".into();
        c.issue =
            "AWS was disconnected on another device; 2 machines this machine made may still run."
                .into();
        let v = view(&input(c));
        let p = v.pages[0].clone();
        // Disconnected, not "Not trusted": the trust is still there.
        assert_eq!(
            (p.status.as_str(), p.health.as_str()),
            ("Disconnected", "fail")
        );
        assert_eq!(v.group.rows[0].line, "Disconnected");
        let i = &p.issues[0];
        assert_eq!(i.id, "link");
        assert_eq!(i.level, "fail");
        assert!(i.text.starts_with("AWS was disconnected on another device"));
        let a = i.action.as_ref().unwrap();
        assert_eq!(
            (a.id.as_str(), a.label.as_str()),
            ("reconnect", "Reconnect…")
        );
        assert!(
            p.issues
                .iter()
                .all(|i| i.id != "trust" && i.id != "machines")
        );
        assert!(p.switch_auth.is_none());
        // A connected cloud with a sentence shows it as a warning.
        let mut c = aws();
        c.issue = "Check the link.".into();
        let p = view(&input(c)).pages[0].clone();
        assert_eq!(
            (p.issues[0].id.as_str(), p.issues[0].level.as_str()),
            ("link", "warn")
        );
    }

    #[test]
    fn a_cloud_that_says_this_person_can_approve_shows_the_sheet_on_any_host() {
        let mut c = aws();
        c.status = "waiting-for-approval".into();
        c.approval = "limits".into();
        c.agent = "Claude Code".into();
        c.pending_budget_cents = Some(50_000);
        c.pending_max_size = "8x32".into();
        // The host flag is off (a real host); the row says this caller can.
        assert!(view(&input(c.clone())).pages[0].approval.is_none());
        c.can_approve = true;
        let p = view(&input(c)).pages[0].clone();
        let a = p.approval.expect("approval sheet");
        assert!(
            a.intro
                .starts_with("Claude Code asked to raise the limits on AWS")
        );
        assert!(
            a.rows
                .iter()
                .any(|r| r.label == "Monthly budget" && r.value == "$500")
        );
        assert!(
            a.rows
                .iter()
                .any(|r| r.label == "Largest machine" && r.value.contains('8'))
        );
        assert_eq!(p.issues[0].action.as_ref().unwrap().id, "review");
    }

    #[test]
    fn status_rows_carry_the_new_fields() {
        let status = serde_json::json!({
            "providers": [{"name": "aws", "connected": true, "auth": "cli", "status": "connected",
                "can_approve": true, "suggest_auth": "oidc", "issue": "x",
                "approval_agent": "eval-runner", "pending_budget_usd_month": 12.5, "pending_max_size": "8x32"}],
            "resources": [{"provider": "aws", "id": "i-1", "type": "instance", "state": "running",
                "started_by": "agent eval-runner", "cost_usd": 1.234}]
        });
        let i = input_from_status(&status, NOW, false);
        let c = &i.providers[0];
        assert!(c.can_approve);
        assert_eq!(
            (c.suggest_auth.as_str(), c.issue.as_str(), c.agent.as_str()),
            ("oidc", "x", "eval-runner")
        );
        assert_eq!(
            (c.pending_budget_cents, c.pending_max_size.as_str()),
            (Some(1250), "8x32")
        );
        assert_eq!(i.machines[0].started_by, "agent eval-runner");
        assert_eq!(i.machines[0].cost_cents, Some(123));
        // An older daemon's rows (none of these fields) read as before.
        let old = input_from_status(
            &serde_json::json!({"providers": [{"name": "aws", "connected": true}]}),
            NOW,
            false,
        );
        assert!(!old.providers[0].can_approve && old.providers[0].suggest_auth.is_empty());
    }
}
