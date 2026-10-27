// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Connect a cloud" as Computers shows it: a picker of five clouds, then
//! one short sheet for the one picked. Each sheet asks for a region, a
//! required monthly budget and the largest machine, and has one primary
//! button; below it, "Or have your agent do it" holds the prompt that hands
//! the same job to a coding agent (read-only, with Copy prompt).
//!
//! - AWS trusts Cua through OIDC: no key is copied or stored. The button
//!   opens AWS (`cloud_connect` with `auth: "oidc"` answers `needs-trust`
//!   and the page to open), the person comes back with the account id, and
//!   the check proves the link (`check: true`).
//! - Hetzner links with an API token. The token never enters this module:
//!   the shell keeps it in its own secure field and tells the sheet only
//!   whether one is entered, as the older sheet ([`crate::cloud_connect`])
//!   does.
//! - Modal uses the sign-in `modal token new` leaves on this computer.
//! - Google Cloud and Azure show their sheet, marked "Coming soon": Cua's
//!   sign-in for them is not built, so nothing is faked.

use serde::{Deserialize, Serialize};

use crate::cloud_page::{cloud_name, region_label, size_label};

/// What an agent follows.
pub const DOCS_URL: &str = "https://cua.ai/docs/spaces/guides/connect-a-cloud";

/// What Cua touches, and how to take it away.
pub const DETAILS_URL: &str = "https://cua.ai/docs/cua-sdk/guides/your-cloud-safety";

/// The line under every sheet's fields.
pub const SAFETY_NOTE: &str = "Cua can only create, stop and delete machines it tags.";

/// The clouds, in the picker's order.
pub const PICKER: [&str; 5] = ["aws", "gcp", "azure", "hetzner", "modal"];

/// A cloud as `cloud_status` lists it, for what the sheet prefills.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSheetProvider {
    /// `aws`, `gcp`, `hetzner`, `modal`.
    #[serde(default)]
    pub name: String,
    /// The cloud is connected: the sheet starts from what it is set to.
    #[serde(default)]
    pub connected: bool,
    /// This computer has the cloud's sign-in or token.
    #[serde(default)]
    pub found: bool,
    /// The connected region (location).
    #[serde(default)]
    pub region: String,
    /// The connected Modal environment.
    #[serde(default)]
    pub environment: String,
    /// The connected budget, in dollars.
    #[serde(default)]
    pub budget: Option<f64>,
    /// The connected largest machine, `4x16`.
    #[serde(default)]
    pub max_size: String,
}

/// What the sheet reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSheetInput {
    /// The clouds the host lists.
    #[serde(default)]
    pub providers: Vec<CloudSheetProvider>,
}

/// The sheet's input from a `cloud_status` result (its JSON).
pub fn input_from_status(status: &serde_json::Value) -> CloudSheetInput {
    let text = |p: &serde_json::Value, k: &str| {
        p.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    CloudSheetInput {
        providers: status
            .get("providers")
            .and_then(|p| p.as_array())
            .map(|a| {
                a.iter()
                    .map(|p| CloudSheetProvider {
                        name: text(p, "name"),
                        connected: p
                            .get("connected")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false),
                        found: p
                            .pointer("/credentials/found")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false),
                        region: text(p, "region"),
                        environment: text(p, "environment"),
                        budget: p.get("budget_usd_month").and_then(|x| x.as_f64()),
                        max_size: text(p, "max_size"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Where the sheet is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum CloudSheetPhase {
    /// Nothing running.
    #[default]
    Idle,
    /// The first `cloud_connect` runs (AWS: it answers where to open).
    Opening,
    /// The cloud's page is open; waiting for the account id.
    Trust {
        /// The page to open.
        open: String,
        /// What to paste there, when there is something.
        paste: String,
        /// Why the last check did not pass.
        error: String,
    },
    /// The check runs.
    Checking {
        /// The page that was opened.
        open: String,
        /// What was pasted there.
        paste: String,
    },
    /// `cloud_connect` runs (an API token or a sign-in).
    Connecting,
    /// Connected (the shell closes the sheet and refreshes).
    Connected,
    /// A call failed.
    Failed {
        /// The error, as the SDK words it.
        error: String,
    },
}

/// The sheet's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSheetState {
    /// The cloud picked; `None`: the picker.
    pub cloud: Option<String>,
    /// Region, location, project or environment, as typed.
    pub place: String,
    /// The monthly budget in dollars, as typed.
    pub budget: String,
    /// The largest machine, `4x16`.
    pub max_size: String,
    /// AWS: the 12-digit account id, as typed.
    pub account_id: String,
    /// The shell's token field is not empty (the token stays there).
    #[serde(default)]
    pub token_entered: bool,
    /// Changing the limits of a cloud that is connected (no trust step, no
    /// token, no agent prompt).
    #[serde(default)]
    pub limits: bool,
    /// Where it is.
    pub phase: CloudSheetPhase,
}

/// What `cloud_connect` answered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudSheetAnswer {
    /// `connected`, `needs-trust`, `checking`, `waiting-for-approval`, `failed`.
    #[serde(default)]
    pub status: String,
    /// `next.open`.
    #[serde(default)]
    pub open: String,
    /// `next.paste`.
    #[serde(default)]
    pub paste: String,
    /// `next.why`.
    #[serde(default)]
    pub why: String,
    /// The first check that did not pass ("permissions: ...").
    #[serde(default)]
    pub failed_check: String,
}

/// An input to the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum CloudSheetAction {
    /// A picker row (or a page's "Set up again…").
    Choose {
        /// `aws`, `gcp`, `azure`, `hetzner`, `modal`.
        id: String,
    },
    /// Back to the picker.
    Back,
    /// The place field.
    SetPlace {
        /// Text.
        text: String,
    },
    /// The budget field.
    SetBudget {
        /// Text.
        text: String,
    },
    /// The largest machine.
    SetSize {
        /// `4x16`.
        text: String,
    },
    /// The account id field.
    SetAccountId {
        /// Text.
        text: String,
    },
    /// The token field changed between empty and not empty.
    SetTokenEntered {
        /// Not empty.
        entered: bool,
    },
    /// The primary button (the shell runs the view's request).
    Press,
    /// The call answered.
    Answered(CloudSheetAnswer),
    /// The call failed.
    Failed {
        /// The SDK's error.
        error: String,
    },
}

/// The `cloud_connect` call to run now (only names and numbers, never a
/// credential).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudSheetTarget {
    /// `aws`, `hetzner`, `modal`.
    pub provider: String,
    /// `oidc` for AWS; empty: the cloud's own sign-in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth: String,
    /// AWS region or Hetzner location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Modal environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// US dollars a month.
    pub budget_usd_month: f64,
    /// The largest machine, `4x16`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<String>,
    /// AWS: the account id the trust stack is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Run the check that proves the link.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub check: bool,
    /// Hetzner: the shell attaches the token it holds as the secret `token`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uses_token: bool,
}

/// One picker row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPickRow {
    /// `aws`.
    pub id: String,
    /// "AWS".
    pub name: String,
    /// The logo.
    pub logo: String,
    /// "OIDC" or "API token".
    pub auth: String,
    /// "Coming soon" when the sheet cannot connect yet.
    pub note: String,
}

/// One choice of a select.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudChoice {
    /// What the call carries.
    pub value: String,
    /// What the person reads.
    pub label: String,
}

/// A field of a cloud's sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSheetField {
    /// `place`, `budget`, `size`, `account`, `token`.
    pub id: String,
    /// "Region".
    pub label: String,
    /// `select`, `text`, `money` or `secret`.
    pub kind: String,
    /// As typed (empty for the secret: the shell holds it).
    pub value: String,
    /// Shown while empty.
    pub placeholder: String,
    /// The choices of a select.
    pub options: Vec<CloudChoice>,
}

/// The step after the cloud's page opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudTrustStep {
    /// What to do there, and what to bring back.
    pub text: String,
    /// The page (open it again).
    pub open_url: String,
    /// "Open AWS console".
    pub open_label: String,
    /// What to paste there; empty when nothing.
    pub paste: String,
    /// "Copy command"; empty with nothing to paste.
    pub copy_label: String,
}

/// The sheet as drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSheetView {
    /// `picker` or `cloud`.
    pub stage: String,
    /// "Connect a cloud", "Connect AWS".
    pub title: String,
    /// The cloud's logo; empty on the picker.
    pub logo: String,
    /// One line under the title.
    pub intro: String,
    /// The picker's rows (the picker only).
    pub rows: Vec<CloudPickRow>,
    /// The sheet's fields.
    pub fields: Vec<CloudSheetField>,
    /// After the cloud's page opened.
    pub trust: Option<CloudTrustStep>,
    /// "Cua can only create, stop and delete machines it tags."
    pub note: String,
    /// "Details".
    pub details_label: String,
    /// Where Details goes.
    pub details_url: String,
    /// "Or have your agent do it"; empty when an agent can't do it yet.
    pub prompt_title: String,
    /// The prompt, read-only.
    pub prompt: String,
    /// "Copy prompt".
    pub copy_label: String,
    /// The primary button.
    pub primary_label: String,
    /// It can be pressed.
    pub primary_enabled: bool,
    /// "Back" on a cloud's sheet; empty on the picker.
    pub back_label: String,
    /// "Cancel".
    pub cancel_label: String,
    /// Under the buttons.
    pub footnote: String,
    /// The last error.
    pub error: Option<String>,
    /// Connected: close the sheet and refresh.
    pub done: bool,
    /// The call to run now.
    pub request: Option<CloudSheetTarget>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// AWS: Cua's own sign-in (OIDC).
    Oidc,
    /// An API token (Hetzner).
    Token,
    /// The sign-in on this computer (Modal).
    Local,
    /// Not built yet.
    Soon,
}

fn kind_of(id: &str) -> Option<Kind> {
    Some(match id {
        "aws" => Kind::Oidc,
        "hetzner" => Kind::Token,
        "modal" => Kind::Local,
        "gcp" | "azure" => Kind::Soon,
        _ => return None,
    })
}

fn auth_word(id: &str) -> &'static str {
    match id {
        "aws" | "gcp" | "azure" => "OIDC",
        _ => "API token",
    }
}

fn default_place(id: &str) -> &'static str {
    match id {
        "aws" => "us-west-2",
        "gcp" => "us-central1",
        "azure" => "westus2",
        "hetzner" => "fsn1",
        _ => "",
    }
}

fn default_budget(id: &str) -> &'static str {
    match id {
        "aws" | "gcp" | "azure" => "200",
        _ => "50",
    }
}

fn default_size(id: &str) -> &'static str {
    match id {
        "hetzner" => "4x8",
        "modal" => "",
        _ => "4x16",
    }
}

const SIZES: [&str; 6] = ["2x4", "4x8", "4x16", "8x16", "8x32", "16x32"];

fn regions(id: &str) -> &'static [&'static str] {
    match id {
        "aws" => &[
            "us-east-1",
            "us-east-2",
            "us-west-1",
            "us-west-2",
            "eu-west-1",
            "eu-central-1",
            "ap-southeast-1",
            "ap-northeast-1",
        ],
        "hetzner" => &["fsn1", "nbg1", "hel1", "ash", "hil"],
        _ => &[],
    }
}

fn find<'a>(input: &'a CloudSheetInput, id: &str) -> Option<&'a CloudSheetProvider> {
    input.providers.iter().find(|p| p.name == id)
}

/// The sheet's first state: the picker, or one cloud's sheet (with
/// `limits`, the sheet that changes a connected cloud's limits).
pub fn cloud_sheet_initial(
    input: &CloudSheetInput,
    cloud: Option<&str>,
    limits: bool,
) -> CloudSheetState {
    let mut s = CloudSheetState::default();
    if let Some(id) = cloud.filter(|c| kind_of(c).is_some_and(|k| k != Kind::Soon || !limits)) {
        choose(&mut s, input, id);
        s.limits = limits;
    }
    s
}

fn choose(s: &mut CloudSheetState, input: &CloudSheetInput, id: &str) {
    // A cloud that is not connected starts from the sheet's own defaults.
    let have = find(input, id).filter(|p| p.connected);
    let place = have
        .map(|p| {
            if id == "modal" {
                &p.environment
            } else {
                &p.region
            }
        })
        .filter(|p| !p.is_empty())
        .map(String::as_str)
        .unwrap_or(default_place(id));
    s.cloud = Some(id.to_string());
    s.place = place.to_string();
    s.budget = have
        .and_then(|p| p.budget)
        .map(trim_money)
        .unwrap_or_else(|| default_budget(id).to_string());
    s.max_size = have
        .map(|p| p.max_size.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| default_size(id).to_string());
    s.account_id.clear();
    s.token_entered = false;
    s.phase = CloudSheetPhase::Idle;
}

fn trim_money(d: f64) -> String {
    if d.fract() == 0.0 {
        format!("{}", d as u64)
    } else {
        format!("{d:.2}")
    }
}

/// The budget as typed ("200", "$200", "1,200.50"); `None` when it is not
/// a positive amount.
pub fn parse_budget(text: &str) -> Option<f64> {
    let t: String = text
        .trim()
        .trim_start_matches('$')
        .chars()
        .filter(|c| *c != ',')
        .collect();
    let d: f64 = t.parse().ok()?;
    (d.is_finite() && d > 0.0 && d <= 1_000_000.0).then_some(d)
}

fn account_ok(text: &str) -> bool {
    let t = text.trim();
    t.len() == 12 && t.chars().all(|c| c.is_ascii_digit())
}

fn busy(s: &CloudSheetState) -> bool {
    matches!(
        s.phase,
        CloudSheetPhase::Opening | CloudSheetPhase::Checking { .. } | CloudSheetPhase::Connecting
    )
}

fn found(input: &CloudSheetInput, id: &str) -> bool {
    find(input, id).is_some_and(|p| p.found)
}

/// Whether Press can start the call now.
fn ready(input: &CloudSheetInput, s: &CloudSheetState) -> bool {
    let Some(id) = s.cloud.as_deref() else {
        return false;
    };
    let Some(kind) = kind_of(id) else {
        return false;
    };
    if busy(s) || parse_budget(&s.budget).is_none() {
        return false;
    }
    match kind {
        Kind::Soon => false,
        Kind::Oidc => match s.phase {
            CloudSheetPhase::Trust { .. } => account_ok(&s.account_id),
            _ => true,
        },
        Kind::Token => s.token_entered || found(input, id),
        Kind::Local => found(input, id),
    }
}

/// Applies `action`.
pub fn cloud_sheet_reduce(
    input: &CloudSheetInput,
    state: &CloudSheetState,
    action: &CloudSheetAction,
) -> CloudSheetState {
    let mut s = state.clone();
    // Editing a field starts over from the fields (the page stays open).
    let edit = |s: &mut CloudSheetState| {
        if !matches!(s.phase, CloudSheetPhase::Trust { .. }) {
            s.phase = CloudSheetPhase::Idle;
        }
    };
    match action {
        CloudSheetAction::Choose { id } if !busy(&s) && kind_of(id).is_some() => {
            choose(&mut s, input, id)
        }
        CloudSheetAction::Back if !busy(&s) => s = CloudSheetState::default(),
        CloudSheetAction::SetPlace { text } if !busy(&s) && s.cloud.is_some() => {
            s.place = text.clone();
            edit(&mut s);
        }
        CloudSheetAction::SetBudget { text } if !busy(&s) && s.cloud.is_some() => {
            s.budget = text.clone();
            edit(&mut s);
        }
        CloudSheetAction::SetSize { text } if !busy(&s) && s.cloud.is_some() => {
            s.max_size = text.clone();
            edit(&mut s);
        }
        CloudSheetAction::SetAccountId { text } if !busy(&s) => {
            s.account_id = text.clone();
            if let CloudSheetPhase::Trust { open, paste, .. } = &s.phase {
                s.phase = CloudSheetPhase::Trust {
                    open: open.clone(),
                    paste: paste.clone(),
                    error: String::new(),
                };
            }
        }
        CloudSheetAction::SetTokenEntered { entered } if !busy(&s) => {
            s.token_entered = *entered;
            edit(&mut s);
        }
        CloudSheetAction::Press if ready(input, &s) => {
            let kind = s.cloud.as_deref().and_then(kind_of);
            s.phase = match (kind, &s.phase) {
                (Some(Kind::Oidc), CloudSheetPhase::Trust { open, paste, .. }) => {
                    CloudSheetPhase::Checking {
                        open: open.clone(),
                        paste: paste.clone(),
                    }
                }
                (Some(Kind::Oidc), _) if !s.limits => CloudSheetPhase::Opening,
                _ => CloudSheetPhase::Connecting,
            };
        }
        CloudSheetAction::Answered(a) if busy(&s) => {
            s.phase = match a.status.as_str() {
                "connected" | "waiting-for-approval" => {
                    s.token_entered = false;
                    CloudSheetPhase::Connected
                }
                "needs-trust" if !a.open.is_empty() => CloudSheetPhase::Trust {
                    open: a.open.clone(),
                    paste: a.paste.clone(),
                    error: String::new(),
                },
                // The link is recorded and the check is next (the sheet
                // runs it with the account id it has, or asks for it).
                "checking" if s.phase == CloudSheetPhase::Opening => CloudSheetPhase::Trust {
                    open: String::new(),
                    paste: String::new(),
                    error: String::new(),
                },
                _ => {
                    let why = if !a.failed_check.is_empty() {
                        a.failed_check.clone()
                    } else if !a.why.is_empty() {
                        a.why.clone()
                    } else {
                        "Cua could not connect it.".into()
                    };
                    if let CloudSheetPhase::Checking { open, paste } = &state.phase {
                        CloudSheetPhase::Trust {
                            open: open.clone(),
                            paste: paste.clone(),
                            error: why,
                        }
                    } else {
                        s.token_entered = false;
                        CloudSheetPhase::Failed { error: why }
                    }
                }
            };
        }
        CloudSheetAction::Failed { error } if busy(&s) => {
            s.token_entered = false;
            s.phase = match &state.phase {
                // The page stays open for another try.
                CloudSheetPhase::Checking { open, paste } => {
                    s.token_entered = state.token_entered;
                    CloudSheetPhase::Trust {
                        open: open.clone(),
                        paste: paste.clone(),
                        error: error.clone(),
                    }
                }
                _ => CloudSheetPhase::Failed {
                    error: error.clone(),
                },
            };
        }
        _ => {}
    }
    s
}

fn field_place(id: &str, s: &CloudSheetState) -> CloudSheetField {
    let (label, kind) = match id {
        "modal" => ("Environment", "text"),
        "hetzner" => ("Location", "select"),
        "azure" => ("Region", "text"),
        _ => ("Region", "select"),
    };
    let mut options: Vec<CloudChoice> = regions(id)
        .iter()
        .map(|r| CloudChoice {
            value: (*r).into(),
            label: region_label(r),
        })
        .collect();
    if kind == "select" && !s.place.is_empty() && !options.iter().any(|o| o.value == s.place) {
        options.insert(
            0,
            CloudChoice {
                value: s.place.clone(),
                label: region_label(&s.place),
            },
        );
    }
    CloudSheetField {
        id: "place".into(),
        label: label.into(),
        kind: kind.into(),
        value: s.place.clone(),
        placeholder: if id == "modal" {
            "main".into()
        } else {
            String::new()
        },
        options,
    }
}

fn field_budget(s: &CloudSheetState) -> CloudSheetField {
    CloudSheetField {
        id: "budget".into(),
        label: "Monthly budget".into(),
        kind: "money".into(),
        value: s.budget.clone(),
        placeholder: "Required".into(),
        options: vec![],
    }
}

fn field_size(s: &CloudSheetState) -> CloudSheetField {
    let mut sizes: Vec<String> = SIZES.iter().map(|x| (*x).to_string()).collect();
    if !s.max_size.is_empty() && !sizes.contains(&s.max_size) {
        sizes.push(s.max_size.clone());
    }
    CloudSheetField {
        id: "size".into(),
        label: "Largest machine".into(),
        kind: "select".into(),
        value: s.max_size.clone(),
        placeholder: String::new(),
        options: sizes
            .into_iter()
            .map(|x| CloudChoice {
                label: size_label(&x),
                value: x,
            })
            .collect(),
    }
}

/// "4 vCPU and 16 GB".
fn size_words(size: &str) -> String {
    match size.split_once('x') {
        Some((c, m)) => format!("{c} vCPU and {m} GB"),
        None => size.to_string(),
    }
}

/// The prompt that hands the job to a coding agent, with the sheet's values.
fn prompt_of(id: &str, s: &CloudSheetState) -> String {
    let name = cloud_name(id);
    let budget = parse_budget(&s.budget)
        .map(trim_money)
        .unwrap_or_else(|| s.budget.trim().to_string());
    let budget_words = if budget.is_empty() {
        "a monthly budget".to_string()
    } else {
        format!("${budget} monthly budget")
    };
    let size = if s.max_size.is_empty() {
        String::new()
    } else {
        format!(", machines up to {}", size_words(&s.max_size))
    };
    let follow = format!(
        "Follow `cua cloud connect {id} --help` and {DOCS_URL} until `cua cloud status {id}` says connected."
    );
    match id {
        "aws" => format!(
            "Connect my AWS account to Cua Spaces: region {}, {budget_words}{size}. {follow}",
            s.place
        ),
        "hetzner" => format!(
            "Connect my Hetzner project to Cua Spaces: location {}, {budget_words}{size}. Ask me for the API token and pass it with --token-stdin; never print or save it. {follow}",
            s.place
        ),
        "modal" => format!(
            "Connect my Modal workspace to Cua Spaces: environment {}, {budget_words}. {follow}",
            if s.place.is_empty() { "main" } else { &s.place }
        ),
        _ => format!("Connect my {name} account to Cua Spaces. {follow}"),
    }
}

/// The prompt for the picker: the agent asks the three questions.
pub fn picker_prompt() -> String {
    format!(
        "Connect a cloud account to Cua Spaces for me. Ask me which cloud, region and monthly budget, then follow `cua cloud connect --help` and {DOCS_URL} until `cua cloud status` says connected."
    )
}

/// The sheet as drawn.
pub fn cloud_sheet_view(input: &CloudSheetInput, state: &CloudSheetState) -> CloudSheetView {
    let base = CloudSheetView {
        stage: "picker".into(),
        title: "Connect a cloud".into(),
        logo: String::new(),
        intro: "Run Spaces in your own cloud account, billed by your cloud, within a budget you set. Cua adds no charge.".into(),
        rows: PICKER
            .iter()
            .map(|id| CloudPickRow {
                id: (*id).into(),
                name: cloud_name(id).into(),
                logo: (*id).into(),
                auth: auth_word(id).into(),
                note: if kind_of(id) == Some(Kind::Soon) {
                    "Coming soon".into()
                } else {
                    String::new()
                },
            })
            .collect(),
        fields: vec![],
        trust: None,
        note: String::new(),
        details_label: String::new(),
        details_url: String::new(),
        prompt_title: "Or have your agent do it".into(),
        prompt: picker_prompt(),
        copy_label: "Copy prompt".into(),
        primary_label: String::new(),
        primary_enabled: false,
        back_label: String::new(),
        cancel_label: "Cancel".into(),
        footnote: String::new(),
        error: None,
        done: false,
        request: None,
    };
    let Some(id) = state.cloud.as_deref() else {
        return base;
    };
    let Some(kind) = kind_of(id) else { return base };
    let name = cloud_name(id);
    let soon = kind == Kind::Soon;
    // An agent can't raise limits, so the limits sheet has no prompt.
    let no_prompt = soon || state.limits;
    let no_error = String::new();
    let trust_phase = match &state.phase {
        CloudSheetPhase::Trust { open, paste, error } => Some((open, paste, error)),
        CloudSheetPhase::Checking { open, paste } => Some((open, paste, &no_error)),
        _ => None,
    };
    let in_trust = kind == Kind::Oidc && trust_phase.is_some();

    let mut fields = vec![];
    if !in_trust {
        fields.push(field_place(id, state));
        fields.push(field_budget(state));
        if id != "modal" {
            fields.push(field_size(state));
        }
        if kind == Kind::Token && !state.limits {
            fields.push(CloudSheetField {
                id: "token".into(),
                label: "API token".into(),
                kind: "secret".into(),
                value: String::new(),
                placeholder: if found(input, id) {
                    "Using the token on this computer".into()
                } else {
                    "Project token (Read & Write)".into()
                },
                options: vec![],
            });
        }
    } else {
        fields.push(CloudSheetField {
            id: "account".into(),
            label: "AWS account ID".into(),
            kind: "text".into(),
            value: state.account_id.clone(),
            placeholder: "12 digits".into(),
            options: vec![],
        });
    }

    let (open_url, paste, error_in_trust) = match trust_phase {
        Some((o, p, e)) => (o.clone(), p.clone(), e.clone()),
        None => (String::new(), String::new(), String::new()),
    };
    let trust = in_trust.then(|| CloudTrustStep {
        text: if open_url.is_empty() {
            "The link is recorded. Enter the 12-digit account ID (top right of the AWS console), and Cua checks it.".into()
        } else {
            "AWS opens signed in to your account. Create the stack there. When it shows CREATE_COMPLETE, enter the 12-digit account ID (top right of the AWS console), and Cua checks it.".into()
        },
        open_label: "Open AWS console".into(),
        copy_label: if paste.is_empty() { String::new() } else { "Copy command".into() },
        open_url,
        paste,
    });

    let intro = match kind {
        _ if state.limits => {
            "Cua stops creating machines at the budget. Machines that already run are left alone."
                .to_string()
        }
        Kind::Oidc => "Cua signs in to AWS on its own each time; there's no key to copy or store.".to_string(),
        Kind::Token => "Hetzner has no OIDC, so it takes an API token. Cua keeps it on this computer and uses it only for your Spaces.".to_string(),
        Kind::Local => "Modal uses the sign-in on this computer. Cua keeps no copy of it.".to_string(),
        Kind::Soon => format!("Cua signs in to {name} on its own each time. This arrives in a later update."),
    };
    let note = match kind {
        Kind::Token => "Revoke it any time in the Hetzner console.".to_string(),
        Kind::Local if !found(input, id) => {
            "No Modal sign-in on this computer. Run `modal token new`, then come back.".to_string()
        }
        Kind::Soon => String::new(),
        _ => SAFETY_NOTE.to_string(),
    };

    let primary_label = match (kind, &state.phase) {
        (_, CloudSheetPhase::Connecting) if state.limits => "Saving…",
        (Kind::Oidc, CloudSheetPhase::Trust { .. }) if state.limits => "Check connection",
        (_, _) if state.limits && !matches!(state.phase, CloudSheetPhase::Checking { .. }) => {
            "Save"
        }
        (Kind::Oidc, CloudSheetPhase::Opening) => "Opening…",
        (Kind::Oidc, CloudSheetPhase::Checking { .. }) => "Checking…",
        (Kind::Oidc, CloudSheetPhase::Trust { .. }) => "Check connection",
        (Kind::Oidc, _) => "Open AWS console",
        (Kind::Soon, _) => "Coming soon",
        (_, CloudSheetPhase::Connecting) => "Connecting…",
        _ => "Connect",
    };

    let request = {
        let budget = parse_budget(&state.budget);
        let place = Some(state.place.trim().to_string()).filter(|p| !p.is_empty());
        let size = Some(state.max_size.trim().to_string()).filter(|m| !m.is_empty());
        budget.and_then(|b| match (&state.phase, kind) {
            (CloudSheetPhase::Opening, Kind::Oidc) => Some(CloudSheetTarget {
                provider: id.into(),
                auth: "oidc".into(),
                region: place.clone(),
                budget_usd_month: b,
                max_size: size.clone(),
                ..Default::default()
            }),
            (CloudSheetPhase::Checking { .. }, Kind::Oidc) => Some(CloudSheetTarget {
                provider: id.into(),
                auth: "oidc".into(),
                region: place.clone(),
                budget_usd_month: b,
                max_size: size.clone(),
                account_id: Some(state.account_id.trim().to_string()),
                check: true,
                ..Default::default()
            }),
            (CloudSheetPhase::Connecting, Kind::Oidc) => Some(CloudSheetTarget {
                provider: id.into(),
                auth: "oidc".into(),
                region: place.clone(),
                budget_usd_month: b,
                max_size: size.clone(),
                check: true,
                ..Default::default()
            }),
            (CloudSheetPhase::Connecting, Kind::Token) => Some(CloudSheetTarget {
                provider: id.into(),
                region: place.clone(),
                budget_usd_month: b,
                max_size: size.clone(),
                uses_token: state.token_entered,
                ..Default::default()
            }),
            (CloudSheetPhase::Connecting, Kind::Local) => Some(CloudSheetTarget {
                provider: id.into(),
                environment: place.clone(),
                budget_usd_month: b,
                ..Default::default()
            }),
            _ => None,
        })
    };

    let error = match &state.phase {
        CloudSheetPhase::Failed { error } => Some(error.clone()),
        CloudSheetPhase::Trust { error, .. } if !error.is_empty() => Some(error.clone()),
        _ => None,
    }
    .or_else(|| Some(error_in_trust).filter(|e| !e.is_empty()));

    CloudSheetView {
        stage: "cloud".into(),
        title: if state.limits {
            format!("{name} limits")
        } else {
            format!("Connect {name}")
        },
        logo: id.into(),
        intro,
        rows: vec![],
        fields,
        trust,
        note,
        details_label: if soon {
            String::new()
        } else {
            "Details".into()
        },
        details_url: if soon {
            String::new()
        } else {
            DETAILS_URL.into()
        },
        prompt_title: if no_prompt {
            String::new()
        } else {
            "Or have your agent do it".into()
        },
        prompt: if no_prompt {
            String::new()
        } else {
            prompt_of(id, state)
        },
        copy_label: if no_prompt {
            String::new()
        } else {
            "Copy prompt".into()
        },
        primary_label: primary_label.into(),
        primary_enabled: ready(input, state),
        back_label: if state.limits {
            String::new()
        } else {
            "Back".into()
        },
        cancel_label: "Cancel".into(),
        footnote: String::new(),
        error,
        done: state.phase == CloudSheetPhase::Connected,
        request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> CloudSheetInput {
        CloudSheetInput {
            providers: vec![
                CloudSheetProvider {
                    name: "modal".into(),
                    found: true,
                    ..Default::default()
                },
                CloudSheetProvider {
                    name: "hetzner".into(),
                    ..Default::default()
                },
            ],
        }
    }

    fn run(s: &CloudSheetState, a: CloudSheetAction) -> CloudSheetState {
        cloud_sheet_reduce(&input(), s, &a)
    }

    #[test]
    fn the_picker_lists_five_clouds_with_how_each_signs_in() {
        let v = cloud_sheet_view(&input(), &cloud_sheet_initial(&input(), None, false));
        assert_eq!(v.stage, "picker");
        let rows: Vec<_> = v
            .rows
            .iter()
            .map(|r| (r.name.as_str(), r.auth.as_str(), r.note.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("AWS", "OIDC", ""),
                ("Google Cloud", "OIDC", "Coming soon"),
                ("Microsoft Azure", "OIDC", "Coming soon"),
                ("Hetzner", "API token", ""),
                ("Modal", "API token", ""),
            ]
        );
        assert_eq!(
            v.prompt,
            "Connect a cloud account to Cua Spaces for me. Ask me which cloud, region and monthly budget, then follow `cua cloud connect --help` and https://cua.ai/docs/spaces/guides/connect-a-cloud until `cua cloud status` says connected."
        );
    }

    #[test]
    fn the_aws_prompt_follows_the_sheet() {
        let s = run(
            &cloud_sheet_initial(&input(), None, false),
            CloudSheetAction::Choose { id: "aws".into() },
        );
        let v = cloud_sheet_view(&input(), &s);
        assert_eq!(v.title, "Connect AWS");
        assert_eq!(
            v.prompt,
            "Connect my AWS account to Cua Spaces: region us-west-2, $200 monthly budget, machines up to 4 vCPU and 16 GB. Follow `cua cloud connect aws --help` and https://cua.ai/docs/spaces/guides/connect-a-cloud until `cua cloud status aws` says connected."
        );
        let s = run(
            &s,
            CloudSheetAction::SetBudget {
                text: "$350".into(),
            },
        );
        let s = run(
            &s,
            CloudSheetAction::SetPlace {
                text: "eu-west-1".into(),
            },
        );
        let s = run(
            &s,
            CloudSheetAction::SetSize {
                text: "8x32".into(),
            },
        );
        assert!(cloud_sheet_view(&input(), &s).prompt.starts_with(
            "Connect my AWS account to Cua Spaces: region eu-west-1, $350 monthly budget, machines up to 8 vCPU and 32 GB."
        ));
        assert_eq!(v.note, SAFETY_NOTE);
        assert_eq!(v.primary_label, "Open AWS console");
    }

    #[test]
    fn aws_walks_open_account_check_connected() {
        let s = run(
            &cloud_sheet_initial(&input(), None, false),
            CloudSheetAction::Choose { id: "aws".into() },
        );
        assert!(cloud_sheet_view(&input(), &s).primary_enabled);
        let s = run(&s, CloudSheetAction::Press);
        let v = cloud_sheet_view(&input(), &s);
        let t = v.request.unwrap();
        assert_eq!(
            (
                t.provider.as_str(),
                t.auth.as_str(),
                t.budget_usd_month,
                t.check
            ),
            ("aws", "oidc", 200.0, false)
        );
        assert_eq!(t.max_size.as_deref(), Some("4x16"));
        assert_eq!(t.account_id, None);
        let s = run(
            &s,
            CloudSheetAction::Answered(CloudSheetAnswer {
                status: "needs-trust".into(),
                open: "https://console.aws.amazon.com/cloudformation/quickcreate".into(),
                ..Default::default()
            }),
        );
        let v = cloud_sheet_view(&input(), &s);
        assert!(v.trust.is_some() && v.request.is_none());
        assert_eq!(v.fields[0].id, "account");
        assert!(!v.primary_enabled);
        let s = run(
            &s,
            CloudSheetAction::SetAccountId {
                text: "210987654321".into(),
            },
        );
        assert!(cloud_sheet_view(&input(), &s).primary_enabled);
        let s = run(&s, CloudSheetAction::Press);
        let t = cloud_sheet_view(&input(), &s).request.unwrap();
        assert_eq!(
            (t.account_id.as_deref(), t.check),
            (Some("210987654321"), true)
        );
        // A failed check keeps the page and says why.
        let failed = run(
            &s,
            CloudSheetAction::Answered(CloudSheetAnswer {
                status: "failed".into(),
                failed_check: "permissions: ec2:RunInstances was denied".into(),
                ..Default::default()
            }),
        );
        let v = cloud_sheet_view(&input(), &failed);
        assert_eq!(
            v.error.as_deref(),
            Some("permissions: ec2:RunInstances was denied")
        );
        assert!(v.trust.is_some() && v.primary_enabled);
        let done = run(
            &s,
            CloudSheetAction::Answered(CloudSheetAnswer {
                status: "connected".into(),
                ..Default::default()
            }),
        );
        assert!(cloud_sheet_view(&input(), &done).done);
    }

    #[test]
    fn a_budget_is_required_before_the_button_works() {
        let s = run(
            &cloud_sheet_initial(&input(), None, false),
            CloudSheetAction::Choose { id: "aws".into() },
        );
        for bad in ["", "0", "abc", "-5"] {
            let s = run(&s, CloudSheetAction::SetBudget { text: bad.into() });
            assert!(!cloud_sheet_view(&input(), &s).primary_enabled, "{bad}");
            assert_eq!(
                run(&s, CloudSheetAction::Press).phase,
                CloudSheetPhase::Idle
            );
        }
        assert_eq!(parse_budget("1,200.50"), Some(1200.5));
    }

    #[test]
    fn hetzner_needs_its_token_and_never_holds_it() {
        let s = run(
            &cloud_sheet_initial(&input(), None, false),
            CloudSheetAction::Choose {
                id: "hetzner".into(),
            },
        );
        let v = cloud_sheet_view(&input(), &s);
        assert_eq!(
            v.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["place", "budget", "size", "token"]
        );
        assert_eq!(
            (v.fields[0].label.as_str(), v.fields[0].value.as_str()),
            ("Location", "fsn1")
        );
        assert_eq!(v.fields[1].value, "50");
        assert!(!v.primary_enabled);
        assert!(v.prompt.contains("--token-stdin; never print or save it"));
        let s = run(&s, CloudSheetAction::SetTokenEntered { entered: true });
        let s = run(&s, CloudSheetAction::Press);
        let t = cloud_sheet_view(&input(), &s).request.unwrap();
        assert!(t.uses_token && t.auth.is_empty() && t.region.as_deref() == Some("fsn1"));
        assert!(!serde_json::to_string(&t).unwrap().contains("token\":\""));
        let s = run(
            &s,
            CloudSheetAction::Failed {
                error: "Hetzner refused the token".into(),
            },
        );
        let v = cloud_sheet_view(&input(), &s);
        assert_eq!(v.error.as_deref(), Some("Hetzner refused the token"));
        assert!(!s.token_entered && !v.primary_enabled);
    }

    #[test]
    fn modal_uses_the_sign_in_on_this_computer() {
        let s = run(
            &cloud_sheet_initial(&input(), None, false),
            CloudSheetAction::Choose { id: "modal".into() },
        );
        let v = cloud_sheet_view(&input(), &s);
        assert_eq!(
            v.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["place", "budget"]
        );
        assert!(v.primary_enabled);
        let t = cloud_sheet_view(&input(), &run(&s, CloudSheetAction::Press))
            .request
            .unwrap();
        assert_eq!(t.provider, "modal");
        assert!(t.region.is_none() && t.environment.is_none());
    }

    #[test]
    fn google_cloud_and_azure_say_coming_soon_and_do_nothing() {
        for id in ["gcp", "azure"] {
            let s = run(
                &cloud_sheet_initial(&input(), None, false),
                CloudSheetAction::Choose { id: id.into() },
            );
            let v = cloud_sheet_view(&input(), &s);
            assert_eq!(v.primary_label, "Coming soon");
            assert!(!v.primary_enabled && v.prompt.is_empty() && v.request.is_none());
            let pressed = run(&s, CloudSheetAction::Press);
            assert_eq!(pressed.phase, CloudSheetPhase::Idle);
        }
    }

    #[test]
    fn a_page_can_open_a_sheet_prefilled_from_the_connected_cloud() {
        let i = CloudSheetInput {
            providers: vec![CloudSheetProvider {
                name: "aws".into(),
                connected: true,
                region: "eu-west-1".into(),
                budget: Some(75.0),
                max_size: "2x4".into(),
                ..Default::default()
            }],
        };
        let s = cloud_sheet_initial(&i, Some("aws"), false);
        assert_eq!(
            (s.place.as_str(), s.budget.as_str(), s.max_size.as_str()),
            ("eu-west-1", "75", "2x4")
        );
        assert_eq!(cloud_sheet_initial(&i, Some("nope"), false).cloud, None);
    }

    #[test]
    fn changing_the_limits_of_a_connected_cloud_saves_without_a_trust_step() {
        let i = CloudSheetInput {
            providers: vec![CloudSheetProvider {
                name: "aws".into(),
                connected: true,
                region: "us-west-2".into(),
                budget: Some(200.0),
                max_size: "4x16".into(),
                found: true,
                ..Default::default()
            }],
        };
        let s = cloud_sheet_initial(&i, Some("aws"), true);
        let v = cloud_sheet_view(&i, &s);
        assert_eq!(v.title, "AWS limits");
        assert_eq!(v.primary_label, "Save");
        assert!(v.prompt.is_empty() && v.back_label.is_empty());
        assert!(v.primary_enabled);
        let s = cloud_sheet_reduce(&i, &s, &CloudSheetAction::SetBudget { text: "400".into() });
        let s = cloud_sheet_reduce(&i, &s, &CloudSheetAction::Press);
        let t = cloud_sheet_view(&i, &s).request.unwrap();
        assert_eq!(
            (t.auth.as_str(), t.budget_usd_month, t.check),
            ("oidc", 400.0, true)
        );
        assert_eq!(t.account_id, None);
        let done = cloud_sheet_reduce(
            &i,
            &s,
            &CloudSheetAction::Answered(CloudSheetAnswer {
                status: "connected".into(),
                ..Default::default()
            }),
        );
        assert!(cloud_sheet_view(&i, &done).done);
        // Hetzner keeps its stored token: no token field, no token sent.
        let h = CloudSheetInput {
            providers: vec![CloudSheetProvider {
                name: "hetzner".into(),
                found: true,
                budget: Some(50.0),
                ..Default::default()
            }],
        };
        let s = cloud_sheet_initial(&h, Some("hetzner"), true);
        assert!(
            cloud_sheet_view(&h, &s)
                .fields
                .iter()
                .all(|f| f.id != "token")
        );
        let s = cloud_sheet_reduce(&h, &s, &CloudSheetAction::Press);
        assert!(!cloud_sheet_view(&h, &s).request.unwrap().uses_token);
        // Google Cloud has nothing to change.
        assert_eq!(cloud_sheet_initial(&h, Some("gcp"), true).cloud, None);
    }

    #[test]
    fn back_returns_to_the_picker() {
        let s = run(
            &cloud_sheet_initial(&input(), None, false),
            CloudSheetAction::Choose { id: "aws".into() },
        );
        assert_eq!(run(&s, CloudSheetAction::Back).cloud, None);
    }
}
