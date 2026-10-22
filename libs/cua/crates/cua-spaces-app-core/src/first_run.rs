// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The first run every app shows (0.10.0): two screens.
//!
//! 1. **Agents.** Pick any, or none: a checkbox for every agent the setup
//!    knows (the common ones and the ones on this computer first, the rest
//!    under "More agents"), ticked for the ones installed here. Under it,
//!    the agents' own sign-ins and keys. Both are per computer, and the page
//!    says so: Cua's skills and MCP server go into each picked agent's
//!    settings on this computer only, the ChatGPT sign-in Codex uses is
//!    Codex's own file on this computer (`codex login`), and provider keys
//!    stay in this computer's credential vault (`agent_keys`). Nothing here
//!    follows the Cua account to another computer.
//! 2. **Your agent can use this Mac.** Two checked rows, the agent and this
//!    computer, then a first request that works on this computer now. This
//!    computer is the user's first Space and is ready at once; an extra
//!    Space (a fresh macOS or Linux computer inside this one) is an
//!    optional card that never blocks Done. A fix that is left moves to the
//!    to-do list on Home ("Finish later").
//!
//! The words, which row needs what, and when Done is on are decided here, so
//! every app says the same thing. The page gathers the readings (the agent
//! setup, `host.status`, the sign-in, the create progress) and draws what
//! [`view`] returns; it decides nothing.
//!
//! What each reading is, and how real it is:
//!
//! * **"Cua is in the agent's settings"**: the agent setup reports the
//!   agent `configured` (its MCP config and skills are in place). Real.
//! * **"The open agent session reloaded"**: an agent session that was open
//!   when its settings changed only sees Cua after a restart. No process
//!   check exists yet, so [`FirstRunInput::agent_open_before_setup`] is
//!   `None` today and the row only tells the user to restart it if it was
//!   open. Once the agent *uses* Cua (the next point), the session has
//!   reloaded for certain and the advice goes away.
//! * **"The agent used Cua"**: a connection in this computer's access log
//!   since the setup screen opened ([`agent_used`]). Partly real: the
//!   host logs only some calls, and an agent names itself.
//! * **This computer**: `host.status` (`configured`, the service running,
//!   online, every permission granted) and the app's sign-in. Real.

use serde::{Deserialize, Serialize};

use crate::relay_usage::{BUY_PRO_PATH, BUY_TEAMS_PATH, PRO_HOURS, PRO_SESSIONS, buy, plan_kind};

/// The agents the access log tells apart by name (setup ids).
pub const OFFERED: [&str; 4] = ["claude-code", "codex", "hermes", "openclaw"];

/// The agents step 1 always lists first, in order (setup ids). Any other
/// agent the setup knows is listed too: beside these when it is installed
/// or ticked, else under "More agents".
pub const FEATURED: [&str; 5] = ["claude-code", "codex", "hermes", "cursor", "openclaw"];

/// The provider key each agent's runs in a Space read (`agent_keys`'
/// providers), for the keys step 1 lists under the picked agents.
const AGENT_KEYS: [(&str, &str); 2] = [("claude-code", "anthropic"), ("codex", "openai")];

/// The first request: it works on this computer with nothing else set up.
pub const FIRST_PROMPT: &str = "List my computers and tell me what's on this Mac";
const FIRST_PROMPT_OTHER: &str = "List my computers and tell me what's on this computer";
const FIRST_PROMPT_PC: &str = "List my computers and tell me what's on this PC";

/// The two screens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirstRunStep {
    #[default]
    Connect,
    Setup,
}

/// An agent the setup knows (`agents.setup`'s row, trimmed).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunAgent {
    /// Setup id (`claude-code`, `codex`, `hermes`, `openclaw`, ...).
    pub agent: String,
    /// "Claude Code".
    pub name: String,
    pub installed: bool,
    pub configured: bool,
    /// The account it signs in with on its own (`ChatGPT` for Codex).
    pub sign_in: Option<String>,
    /// That sign-in is on this computer.
    pub signed_in: bool,
}

/// A permission this computer still needs (`host.status`'s hint).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunPermission {
    /// `accessibility`, `screen-recording`, ...
    pub id: String,
    pub granted: bool,
    /// The Settings pane, when known.
    pub settings_url: Option<String>,
}

/// This computer as `host.status` reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunHost {
    pub configured: bool,
    /// The background service is running.
    pub running: bool,
    /// The relay sees it (none: not known).
    pub online: Option<bool>,
    pub permissions: Vec<FirstRunPermission>,
    /// The host setup failed with this.
    pub error: Option<String>,
}

/// An extra Space this computer can make in one click (the Spaces page's
/// one-click offer, `firstSpaceOffers`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunOffer {
    /// `macos` or `linux`.
    pub os: String,
    /// "about 2 min", "10 to 30 min the first time".
    pub time: Option<String>,
    /// "22 GB download", "Already downloaded".
    pub size: Option<String>,
    /// Why it can't be made here (the core's words).
    pub blocked: Option<String>,
}

/// The extra Space, as far as it got.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunExtra {
    /// `none` (not chosen yet), `not-now`, `creating`, `ready`, `failed`.
    pub phase: String,
    /// "macOS 1".
    pub name: Option<String>,
    /// `macos` or `linux`.
    pub os: Option<String>,
    /// The create's progress line ("Downloading macOS · 4.2 of 22.3 GB ...").
    pub detail: Option<String>,
    /// 0..1, when known.
    pub fraction: Option<f64>,
    /// Why it failed.
    pub error: Option<String>,
}

/// The plan, for the card after the first success.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunPlan {
    /// The relay meters this account (`relay.usage`'s `metered`).
    pub metered: bool,
    /// `free`, `pro`, `team`.
    pub plan: String,
    /// Free's connections at once (default 2).
    pub sessions_limit: Option<u32>,
    /// Free's remote hours a month (default 20).
    pub hours_limit: Option<u32>,
    /// The user entered teammates' emails (a share with other people).
    pub teammates: bool,
    /// "Not now" was pressed on this card once: it never shows again.
    pub dismissed: bool,
    /// The account app (https).
    pub account_app: String,
}

/// Everything the first run reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunInput {
    pub step: FirstRunStep,
    /// `macos`, `windows`, `linux`.
    pub host_os: String,
    /// What the agent setup found (none yet: still looking).
    pub agents: Option<Vec<FirstRunAgent>>,
    /// The agent the first request names (setup id): the first one
    /// connected. Older saved runs connected only this one.
    pub agent: Option<String>,
    /// Step 1: the ticked agents (none: not changed yet, so the ones
    /// installed here). Step 2: the agents connected (empty: none).
    pub picked: Option<Vec<String>>,
    /// The provider keys saved on this computer (none: not read, or this
    /// app can't read them).
    pub keys: Option<crate::agent_keys::AgentKeysInput>,
    /// The agent whose own sign-in is waiting for the browser.
    pub agent_signing_in: Option<String>,
    /// Starting an agent's own sign-in failed with this.
    pub agent_sign_in_error: Option<String>,
    /// Adding Cua to the agent's settings is running.
    pub connecting: bool,
    /// Adding Cua to the agent's settings failed with this.
    pub connect_error: Option<String>,
    /// The agent was running when its settings changed (none: not known;
    /// no process check exists yet).
    pub agent_open_before_setup: Option<bool>,
    /// When the agent last used Cua, as the page shows times ("18:12").
    pub agent_used_at: Option<String>,
    pub signed_in: bool,
    /// Sign-in is waiting for the browser.
    pub signing_in: bool,
    /// `host.status` (none: not read yet).
    pub host: Option<FirstRunHost>,
    /// The host setup is running (after sign-in).
    pub host_setting_up: bool,
    /// The extra Space offers, macOS first.
    pub offers: Vec<FirstRunOffer>,
    pub extra: FirstRunExtra,
    pub plan: Option<FirstRunPlan>,
}

/// A button: what it says and what it does (`id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunButton {
    pub id: String,
    pub label: String,
}

fn button(id: &str, label: &str) -> FirstRunButton {
    FirstRunButton {
        id: id.into(),
        label: label.into(),
    }
}

/// An agent on step 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPick {
    pub agent: String,
    pub name: String,
    pub checked: bool,
    pub installed: bool,
    /// Cua is in its settings already: it stays ticked.
    pub locked: bool,
    /// "Already uses Cua", "Not found on this Mac".
    pub note: Option<String>,
}

/// A sign-in or key on step 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRow {
    /// `sign-in:codex`, `key:anthropic`, `key:openai`.
    pub id: String,
    /// "ChatGPT sign-in", "Anthropic key".
    pub title: String,
    /// Who uses it: "Codex uses it on this Mac".
    pub note: String,
    /// "Signed in", "Saved", "Finish in your browser…".
    pub status: Option<String>,
    /// The status is a done one ("Signed in", "Saved"), not a wait.
    pub ok: bool,
    /// `agent-sign-in` (with `agent`), or `add-key` (with `provider`).
    pub action: Option<FirstRunButton>,
    pub agent: Option<String>,
    pub provider: Option<String>,
    /// Starting it failed.
    pub error: Option<String>,
}

/// Step 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectView {
    /// "A shared Space for every agent." (the one serif accent word is
    /// `accent`, the only one in the app).
    pub title: String,
    pub accent: String,
    pub lede: String,
    /// "Pick any, or none."
    pub intro: String,
    /// "On this Mac".
    pub agents_label: String,
    /// The common agents, and any installed or ticked here.
    pub agents: Vec<AgentPick>,
    /// "More agents".
    pub more_label: String,
    /// Every other agent the setup knows.
    pub more: Vec<AgentPick>,
    /// What picking does, and that it is this computer only.
    pub agents_note: String,
    /// "Sign-ins and keys".
    pub credentials_label: String,
    /// "They stay on this Mac. Other computers keep their own."
    pub credentials_note: String,
    /// The picked agents' own sign-ins and provider keys (empty: the
    /// section is not drawn).
    pub credentials: Vec<CredentialRow>,
    /// "Continue".
    pub primary: FirstRunButton,
    /// The agents Continue connects (setup ids, ticked and not set up yet).
    pub connects: Vec<String>,
    /// "Adding Cua to Claude Code and Codex…" while it runs.
    pub busy: Option<String>,
    pub error: Option<String>,
}

/// A checked row's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RowState {
    Checking,
    Ok,
    Fix,
}

/// One of the two checked rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRow {
    /// `agent` or `computer`.
    pub id: String,
    pub state: RowState,
    pub title: String,
    pub note: String,
    /// The fix: `restart-check`, `connect`, `sign-in`, `open-settings`,
    /// `set-up`.
    pub action: Option<FirstRunButton>,
    /// The Settings pane `open-settings` opens.
    pub settings_url: Option<String>,
}

/// The first request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptView {
    /// "Try this in Claude Code", then "You asked Claude Code".
    pub heading: String,
    pub text: String,
    /// "This works now, on this Mac."
    pub note: String,
    /// "Copy", until the agent used Cua.
    pub copy: Option<String>,
}

/// A choice on the extra Space card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraChoice {
    pub os: String,
    /// "macOS", "Linux".
    pub label: String,
    /// "Ready in about 2 min", "22 GB download · 10 to 30 min the first time".
    pub detail: String,
    /// Why it can't be made here.
    pub blocked: Option<String>,
}

/// The optional extra Space card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraView {
    /// `choose`, `creating`, `ready`, `failed`, `not-now`.
    pub phase: String,
    pub title: String,
    /// "(optional)" on `choose`.
    pub tag: Option<String>,
    pub note: String,
    pub choices: Vec<ExtraChoice>,
    /// "Not now", "Cancel", "Add a Space", "Try again".
    pub action: Option<FirstRunButton>,
    pub fraction: Option<f64>,
    /// Under the progress: "You can press Done now. ..."
    pub footnote: Option<String>,
}

/// The plan card after the first success (Pro, or Teams for someone who
/// shares with teammates). Only what the plans really include.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsellCard {
    /// `pro` or `teams`.
    pub plan: String,
    pub title: String,
    pub lines: Vec<String>,
    pub price: String,
    pub cta: FirstRunButton,
    /// The checkout on the account app.
    pub url: String,
    /// "Not now": hides it for good.
    pub dismiss: String,
}

/// The main button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryView {
    /// `done` or `finish-later`.
    pub id: String,
    pub label: String,
    pub enabled: bool,
    /// "We'll keep a to-do list on Home." beside Finish later.
    pub note: Option<String>,
}

/// The art beside the page: which keys show. `agent` is the agent key's
/// mark (setup id). `computer`: `on`, `faded`. `extra`: `none`, `outline`,
/// `filling`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtView {
    pub agent: String,
    pub computer: String,
    pub extra: String,
}

/// Step 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupView {
    /// "Checking your setup.", "Your agent can use this Mac.", "One step left."
    pub title: String,
    pub rows: Vec<CheckRow>,
    /// None when no agent was picked.
    pub prompt: Option<PromptView>,
    pub extra: ExtraView,
    pub upsell: Option<UpsellCard>,
    pub primary: PrimaryView,
}

/// The first run as drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunView {
    pub step: FirstRunStep,
    /// "Skip for now" (top right, both screens).
    pub later: String,
    pub connect: Option<ConnectView>,
    pub setup: Option<SetupView>,
    pub art: ArtView,
    /// Two page marks.
    pub dots: u8,
    pub current: u8,
}

/// A line of Home's "Finish setting up" list (frame 9), after Finish later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunTodo {
    /// `agent`, `computer`, `prompt`, `extra`, `sign-in`.
    pub id: String,
    pub title: String,
    pub note: String,
    pub done: bool,
    pub optional: bool,
    pub action: Option<FirstRunButton>,
    pub settings_url: Option<String>,
}

/// Home's "Finish setting up" card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunTodos {
    pub title: String,
    pub lede: String,
    /// "2 of 5 done".
    pub progress: String,
    pub items: Vec<FirstRunTodo>,
    /// Every required line is done (the card can go).
    pub complete: bool,
}

fn this_computer(os: &str) -> &'static str {
    match os {
        "macos" => "this Mac",
        "windows" => "this PC",
        _ => "this computer",
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn os_name(os: &str) -> &'static str {
    match os {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        _ => "a computer",
    }
}

/// The first request's words on this system.
pub fn first_prompt(os: &str) -> &'static str {
    match os {
        "macos" => FIRST_PROMPT,
        "windows" => FIRST_PROMPT_PC,
        _ => FIRST_PROMPT_OTHER,
    }
}

fn default_name(agent: &str) -> &'static str {
    match agent {
        "claude-code" => "Claude Code",
        "codex" => "Codex",
        "hermes" => "Hermes",
        "openclaw" => "OpenClaw",
        "cursor" => "Cursor",
        _ => "your agent",
    }
}

fn name_of(agents: &[FirstRunAgent], id: &str) -> String {
    agents
        .iter()
        .find(|a| a.agent == id)
        .map(|a| a.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| default_name(id).into())
}

/// The agent a single-agent screen leads with: the first offered one
/// installed here, else Claude Code.
pub fn lead_agent(agents: &[FirstRunAgent]) -> String {
    OFFERED
        .iter()
        .find(|id| agents.iter().any(|a| a.agent == **id && a.installed))
        .unwrap_or(&OFFERED[0])
        .to_string()
}

/// "Claude Code", "Claude Code and Codex", "Claude Code, Codex and Hermes"
/// (`or` for the first request: "Claude Code or Codex").
fn names(agents: &[FirstRunAgent], ids: &[String], and: &str) -> String {
    let n: Vec<String> = ids.iter().map(|id| name_of(agents, id)).collect();
    match n.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} {and} {last}", rest.join(", ")),
    }
}

/// Step 1's ticks: the user's, else every agent installed here (one Cua
/// is in already counts as installed).
pub fn picked(input: &FirstRunInput) -> Vec<String> {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let mut out: Vec<String> = match &input.picked {
        Some(p) => p.clone(),
        None => agents
            .iter()
            .filter(|a| a.installed || a.configured)
            .map(|a| a.agent.clone())
            .collect(),
    };
    // One Cua is already in can't be unticked here (Settings removes it).
    for a in agents.iter().filter(|a| a.configured) {
        if !out.contains(&a.agent) {
            out.push(a.agent.clone());
        }
    }
    out
}

/// The agents connected on step 2: what step 1 connected, or the one an
/// older run connected.
fn connected(input: &FirstRunInput) -> Vec<String> {
    match &input.picked {
        Some(p) => p.clone(),
        None => input.agent.clone().into_iter().collect(),
    }
}

/// The agent the first request names: the first connected.
fn lead_of(input: &FirstRunInput) -> Option<String> {
    connected(input).into_iter().next()
}

fn agent_name(input: &FirstRunInput) -> String {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    lead_of(input)
        .map(|id| name_of(agents, &id))
        .unwrap_or_else(|| "your agent".into())
}

/// Every agent step 1 lists, in order: [`FEATURED`], then each other one
/// the setup knows (`more` when it isn't installed or ticked here).
fn agent_picks(input: &FirstRunInput) -> (Vec<AgentPick>, Vec<AgentPick>) {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let ticked = picked(input);
    let place = this_computer(&input.host_os);
    let pick = |id: &str| {
        let row = agents.iter().find(|a| a.agent == id);
        let installed = row.is_some_and(|a| a.installed);
        let locked = row.is_some_and(|a| a.configured);
        AgentPick {
            agent: id.into(),
            name: name_of(agents, id),
            checked: ticked.iter().any(|t| t == id),
            installed,
            locked,
            note: if locked {
                Some("Already uses Cua".into())
            } else if row.is_some() && !installed {
                Some(format!("Not found on {place}"))
            } else {
                None
            },
        }
    };
    let mut main: Vec<AgentPick> = FEATURED.iter().map(|id| pick(id)).collect();
    let mut more = Vec::new();
    for a in agents
        .iter()
        .filter(|a| !FEATURED.contains(&a.agent.as_str()))
    {
        let p = pick(&a.agent);
        if p.installed || p.checked {
            main.push(p);
        } else {
            more.push(p);
        }
    }
    (main, more)
}

/// The picked agents' own sign-ins (Codex's ChatGPT sign-in), then the
/// provider keys their runs in a Space read, when this app can read keys.
fn credentials(input: &FirstRunInput, ticked: &[String]) -> Vec<CredentialRow> {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let place = this_computer(&input.host_os);
    let mut out = Vec::new();
    for id in ticked {
        let Some(a) = agents.iter().find(|a| &a.agent == id) else {
            continue;
        };
        let Some(label) = a.sign_in.as_ref().filter(|l| !l.is_empty()) else {
            continue;
        };
        let waiting = input.agent_signing_in.as_deref() == Some(id.as_str()) && !a.signed_in;
        out.push(CredentialRow {
            id: format!("sign-in:{id}"),
            title: format!("{label} sign-in"),
            note: format!("{} uses it on {place}", default_or(&a.name, id)),
            ok: a.signed_in,
            status: if a.signed_in {
                Some("Signed in".into())
            } else if waiting {
                Some("Finish in your browser…".into())
            } else {
                None
            },
            action: (!a.signed_in && !waiting && a.installed)
                .then(|| button("agent-sign-in", "Sign in")),
            agent: Some(id.clone()),
            provider: None,
            error: (!a.signed_in)
                .then(|| input.agent_sign_in_error.clone())
                .flatten()
                .map(|e| format!("Couldn't start the sign-in: {e}")),
        });
    }
    let Some(keys) = input
        .keys
        .as_ref()
        .filter(|k| k.unavailable.is_none() && k.error.is_none())
    else {
        return out;
    };
    for (agent, provider) in AGENT_KEYS {
        if !ticked.iter().any(|t| t == agent) {
            continue;
        }
        let Some(row) = crate::agent_keys::view(keys)
            .rows
            .into_iter()
            .find(|r| r.provider == provider)
        else {
            continue;
        };
        out.push(CredentialRow {
            id: format!("key:{provider}"),
            title: format!("{} key", row.title),
            note: format!("For {} in your Spaces", name_of(agents, agent)),
            status: row.set.then(|| "Saved".into()),
            ok: row.set,
            action: (!row.set).then(|| button("add-key", "Add key")),
            agent: None,
            provider: Some(provider.into()),
            error: None,
        });
    }
    out
}

fn default_or(name: &str, id: &str) -> String {
    if name.is_empty() {
        default_name(id).into()
    } else {
        name.into()
    }
}

fn connect_view(input: &FirstRunInput) -> ConnectView {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let place = this_computer(&input.host_os);
    let (list, more) = agent_picks(input);
    let ticked = picked(input);
    let connects: Vec<String> = ticked
        .iter()
        .filter(|id| !agents.iter().any(|a| &a.agent == *id && a.configured))
        .cloned()
        .collect();
    ConnectView {
        title: "A shared Space for every agent.".into(),
        accent: "agent".into(),
        lede: format!(
            "Connect your agents and give them computers to work on: macOS, Linux or Windows, right here on {place}."
        ),
        intro: "Pick any, or none.".into(),
        agents_label: format!("On {place}"),
        agents: list,
        more_label: "More agents".into(),
        more,
        agents_note: format!(
            "Cua adds its skills and MCP server to each one you pick, on {place} only. Your other computers set up their own."
        ),
        credentials_label: "Sign-ins and keys".into(),
        credentials_note: format!("They stay on {place}. Other computers keep their own."),
        credentials: credentials(input, &ticked),
        primary: button("continue", "Continue"),
        busy: (input.connecting && !connects.is_empty())
            .then(|| format!("Adding Cua to {}…", names(agents, &connects, "and"))),
        connects,
        error: input
            .connect_error
            .as_ref()
            .map(|e| format!("Couldn't add Cua: {e}")),
    }
}

/// Step 2's agent row (none when no agent was picked).
fn agent_row(input: &FirstRunInput) -> Option<CheckRow> {
    let ids = connected(input);
    if ids.is_empty() {
        return None;
    }
    let all = input.agents.as_deref().unwrap_or(&[]);
    let name = names(all, &ids, "and");
    let many = ids.len() > 1;
    let row = |state, title: String, note: String, action: Option<FirstRunButton>| CheckRow {
        id: "agent".into(),
        state,
        title,
        note,
        action,
        settings_url: None,
    };
    let Some(agents) = &input.agents else {
        return Some(row(
            RowState::Checking,
            name,
            format!(
                "Checking that {} can use Cua…",
                if many { "they" } else { "it" }
            ),
            None,
        ));
    };
    let missing: Vec<String> = ids
        .iter()
        .filter(|id| !agents.iter().any(|a| &a.agent == *id && a.configured))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let them = names(all, &missing, "and");
        if input.connecting {
            return Some(row(
                RowState::Checking,
                name,
                format!("Adding Cua to {them}…"),
                None,
            ));
        }
        return Some(row(
            RowState::Fix,
            format!("Connect {them}"),
            input.connect_error.clone().map_or_else(
                || {
                    if missing.len() > 1 {
                        "Cua isn't in their settings yet.".into()
                    } else {
                        format!("Cua isn't in {them}'s settings yet.")
                    }
                },
                |e| format!("Couldn't add Cua: {e}"),
            ),
            Some(button("connect", "Connect")),
        ));
    }
    let can = format!("{name} can use Cua");
    if let Some(at) = &input.agent_used_at {
        return Some(row(
            RowState::Ok,
            can,
            format!(
                "{} used Cua at {at}.",
                if many { "Your agent" } else { "It" }
            ),
            None,
        ));
    }
    if input.agent_open_before_setup == Some(true) {
        return Some(row(
            RowState::Fix,
            format!("Restart {name}"),
            format!(
                "{} open when we added Cua. Close {}, then open {} again.",
                if many { "They were" } else { "It was" },
                if many { "them" } else { "it" },
                if many { "them" } else { "it" }
            ),
            Some(button("restart-check", "Check again")),
        ));
    }
    Some(row(
        RowState::Ok,
        can,
        if many {
            "We added Cua to their settings. If any was already open, restart it.".into()
        } else {
            format!("We added Cua to {name}'s settings. If {name} was already open, restart it.")
        },
        None,
    ))
}

fn missing_permission(host: &FirstRunHost, id: &str) -> Option<FirstRunPermission> {
    host.permissions
        .iter()
        .find(|p| p.id == id && !p.granted)
        .cloned()
}

fn computer_row(input: &FirstRunInput) -> CheckRow {
    let place = this_computer(&input.host_os);
    let row = |state, title: String, note: &str, action: Option<FirstRunButton>| CheckRow {
        id: "computer".into(),
        state,
        title,
        note: note.into(),
        action,
        settings_url: None,
    };
    let checking = |note: &str| row(RowState::Checking, cap(place), note, None);
    if !input.signed_in {
        if input.signing_in {
            return checking("Finish signing in to Cua in your browser.");
        }
        return row(
            RowState::Fix,
            "Sign in to Cua".into(),
            &format!("So your agent can find {place} and your other computers."),
            Some(button("sign-in", "Sign in")),
        );
    }
    let Some(host) = &input.host else {
        return checking("Checking that your agent can use it…");
    };
    if input.host_setting_up {
        return checking(&format!("Setting up {place} for your agent…"));
    }
    if !host.configured {
        return row(
            RowState::Fix,
            format!("Set up {place}"),
            &host.error.as_ref().map_or_else(
                || format!("Cua needs to add {place} to your account. It takes a few seconds."),
                |e| format!("Setting it up didn't work: {e}"),
            ),
            Some(button(
                "set-up",
                if host.error.is_some() {
                    "Try again"
                } else {
                    "Set up"
                },
            )),
        );
    }
    if !host.running {
        return row(
            RowState::Fix,
            format!("Start Cua on {place}"),
            "Cua's background service isn't running.",
            Some(button("set-up", "Start")),
        );
    }
    for (id, title, note) in [
        (
            "accessibility",
            format!("Let Cua control {place}"),
            format!(
                "So your agent can click and type. {} asks you once.",
                os_name(&input.host_os)
            ),
        ),
        (
            "screen-recording",
            format!(
                "Let Cua see {}",
                if input.host_os == "macos" {
                    "this screen"
                } else {
                    "the screen"
                }
            ),
            format!(
                "So your agent can see what's on {place}. {} asks you once.",
                os_name(&input.host_os)
            ),
        ),
    ] {
        if let Some(p) = missing_permission(host, id) {
            let mut r = row(
                RowState::Fix,
                title,
                &note,
                Some(button("open-settings", "Open Settings")),
            );
            r.settings_url = p.settings_url;
            return r;
        }
    }
    if let Some(p) = host.permissions.iter().find(|p| !p.granted) {
        let mut r = row(
            RowState::Fix,
            format!("Give Cua one more permission on {place}"),
            "Open Settings and turn Cua on.",
            Some(button("open-settings", "Open Settings")),
        );
        r.settings_url = p.settings_url.clone();
        return r;
    }
    if host.online == Some(false) {
        return row(
            RowState::Fix,
            format!("{} is offline", cap(place)),
            "Cua can't reach the internet from here. Check your connection.",
            Some(button("restart-check", "Check again")),
        );
    }
    row(
        RowState::Ok,
        format!("{} is your first Space", cap(place)),
        "Ready now. Your agent can see the screen, click and type.",
        None,
    )
}

fn os_label(os: &str) -> String {
    os_name(os).into()
}

fn offer_detail(o: &FirstRunOffer) -> String {
    let time = o.time.as_deref().map(|t| format!("Ready in {t}"));
    match o.os.as_str() {
        "macos" => [o.size.clone(), o.time.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
        _ => time.unwrap_or_default(),
    }
}

fn extra_view(input: &FirstRunInput) -> ExtraView {
    let place = this_computer(&input.host_os);
    let e = &input.extra;
    let name = e
        .name
        .clone()
        .unwrap_or_else(|| os_label(e.os.as_deref().unwrap_or("")));
    let base = |phase: &str, title: String, note: String| ExtraView {
        phase: phase.into(),
        title,
        tag: None,
        note,
        choices: vec![],
        action: None,
        fraction: None,
        footnote: None,
    };
    match e.phase.as_str() {
        "creating" => ExtraView {
            action: Some(button("cancel", "Cancel")),
            fraction: e.fraction.map(|f| f.clamp(0.0, 1.0)),
            footnote: Some("You can press Done now. We'll let you know when it's ready.".into()),
            ..base(
                "creating",
                format!("Making {name} in the background"),
                e.detail.clone().unwrap_or_else(|| "Starting…".into()),
            )
        },
        "ready" => base(
            "ready",
            format!("{name} is ready"),
            format!(
                "A fresh {} computer inside {place}. Your agent can use it now.",
                os_label(e.os.as_deref().unwrap_or(""))
            ),
        ),
        "failed" => ExtraView {
            action: Some(button("retry", "Try again")),
            ..base(
                "failed",
                format!("Couldn't make {name}"),
                e.error
                    .clone()
                    .unwrap_or_else(|| "Something went wrong.".into()),
            )
        },
        "not-now" => ExtraView {
            action: Some(button("add", "Add a Space")),
            ..base(
                "not-now",
                "No extra Space for now".into(),
                "Add one from Home whenever you like.".into(),
            )
        },
        _ => {
            // macOS first, then Linux; nothing else.
            let mut choices: Vec<ExtraChoice> = ["macos", "linux"]
                .iter()
                .filter_map(|os| input.offers.iter().find(|o| o.os == *os))
                .map(|o| ExtraChoice {
                    os: o.os.clone(),
                    label: os_label(&o.os),
                    detail: offer_detail(o),
                    blocked: o.blocked.clone(),
                })
                .collect();
            // A system this computer can't run at all isn't a choice.
            if choices.iter().any(|c| c.blocked.is_none()) {
                choices.retain(|c| !(c.os == "macos" && c.blocked.is_some()));
            }
            ExtraView {
                tag: Some("(optional)".into()),
                choices,
                action: Some(button("not-now", "Not now")),
                ..base(
                    "choose",
                    "Give your agent its own Space".into(),
                    format!(
                        "A fresh computer inside {place}, so your agent can't touch your files or your mouse."
                    ),
                )
            }
        }
    }
}

/// The plan card after the first success: Teams for someone who entered
/// teammates' emails and isn't on Teams, else Pro for Free. Never after
/// "Not now", never before the agent used Cua, never on an unmetered
/// account. Only what the plans include: Pro is more remote hours and more
/// connections at once; Teams is computers a team shares, roles and device
/// approval.
pub fn upsell(plan: &FirstRunPlan, agent_used: bool) -> Option<UpsellCard> {
    if !agent_used || plan.dismissed || !plan.metered {
        return None;
    }
    let kind = plan_kind(&plan.plan);
    let app = if plan.account_app.starts_with("https://") {
        plan.account_app.clone()
    } else {
        "https://app.cua.ai".into()
    };
    if plan.teammates && kind != "teams" {
        return Some(UpsellCard {
            plan: "teams".into(),
            title: "Working with others?".into(),
            lines: vec![
                "Teams gives your team computers you share, with roles and device approval.".into(),
            ],
            price: "$30 per person a month, starting at 1".into(),
            cta: button("upsell", "Start Teams"),
            url: buy(&app, BUY_TEAMS_PATH, "app_onboarding"),
            dismiss: "Not now".into(),
        });
    }
    if kind != "free" {
        return None;
    }
    let sessions = plan.sessions_limit.unwrap_or(2);
    let hours = plan.hours_limit.unwrap_or(20);
    Some(UpsellCard {
        plan: "pro".into(),
        title: "More remote hours with Pro".into(),
        lines: vec![
            format!("Free: {hours} remote hours a month · {sessions} connections at once"),
            format!("Pro: {PRO_HOURS} remote hours a month · {PRO_SESSIONS} connections at once"),
        ],
        price: "$18 a month".into(),
        cta: button("upsell", "Try Pro"),
        url: buy(&app, BUY_PRO_PATH, "app_onboarding"),
        dismiss: "Not now".into(),
    })
}

/// The art for step 2: the computer key fades while its row is checked or
/// needs a fix; an extra Space is a dashed outline, filled faintly while
/// it is made.
fn art(input: &FirstRunInput, computer: &CheckRow) -> ArtView {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let agent = match input.step {
        FirstRunStep::Connect => picked(input).into_iter().next(),
        FirstRunStep::Setup => lead_of(input),
    };
    ArtView {
        agent: agent.unwrap_or_else(|| {
            if input.step == FirstRunStep::Connect && input.agents.is_none() {
                lead_agent(agents)
            } else {
                "other".into()
            }
        }),
        computer: if input.step == FirstRunStep::Setup && computer.state != RowState::Ok {
            "faded"
        } else {
            "on"
        }
        .into(),
        extra: match (input.step, input.extra.phase.as_str()) {
            (FirstRunStep::Connect, _) => "none",
            (_, "creating") => "filling",
            (_, "ready") => "on",
            (_, "not-now") => "none",
            _ => "outline",
        }
        .into(),
    }
}

/// The first run as drawn.
pub fn view(input: &FirstRunInput) -> FirstRunView {
    let agent = agent_row(input);
    let computer = computer_row(input);
    let art = art(input, &computer);
    let later = "Skip for now".to_string();
    if input.step == FirstRunStep::Connect {
        return FirstRunView {
            step: FirstRunStep::Connect,
            later,
            connect: Some(connect_view(input)),
            setup: None,
            art,
            dots: 2,
            current: 0,
        };
    }
    let place = this_computer(&input.host_os);
    let ids = connected(input);
    let all = input.agents.as_deref().unwrap_or(&[]);
    let rows: Vec<CheckRow> = agent.into_iter().chain([computer]).collect();
    let checking = rows.iter().any(|r| r.state == RowState::Checking);
    let fixes = rows.iter().filter(|r| r.state == RowState::Fix).count();
    let ready = rows.iter().all(|r| r.state == RowState::Ok);
    let title = if ready {
        match ids.len() {
            0 => format!("{} is ready.", cap(place)),
            1 => format!("Your agent can use {place}."),
            _ => format!("Your agents can use {place}."),
        }
    } else if fixes == 1 && !checking {
        "One step left.".into()
    } else if fixes > 1 && !checking {
        format!("{fixes} steps left.")
    } else {
        "Checking your setup.".into()
    };
    let used = input.agent_used_at.is_some();
    let prompt = (!ids.is_empty()).then(|| {
        if used {
            let name = agent_name(input);
            PromptView {
                heading: format!("You asked {name}"),
                text: first_prompt(&input.host_os).into(),
                note: format!("It answered in {name}. Ask it anything else."),
                copy: None,
            }
        } else {
            PromptView {
                heading: format!("Try this in {}", names(all, &ids, "or")),
                text: first_prompt(&input.host_os).into(),
                note: format!("This works now, on {place}."),
                copy: Some("Copy".into()),
            }
        }
    });
    let finish_later = fixes > 0 && !checking;
    FirstRunView {
        step: FirstRunStep::Setup,
        later,
        connect: None,
        setup: Some(SetupView {
            title,
            rows,
            prompt,
            extra: extra_view(input),
            upsell: input.plan.as_ref().and_then(|p| upsell(p, used)),
            primary: if finish_later {
                PrimaryView {
                    id: "finish-later".into(),
                    label: "Finish later".into(),
                    enabled: true,
                    note: Some("We'll keep a to-do list on Home.".into()),
                }
            } else {
                PrimaryView {
                    id: "done".into(),
                    label: "Done".into(),
                    enabled: ready,
                    note: None,
                }
            },
        }),
        art,
        dots: 2,
        current: 1,
    }
}

/// Home's "Finish setting up" list after Finish later: the same checks,
/// kept (frame 9). Nothing left that is required: `complete`.
pub fn todos(input: &FirstRunInput) -> FirstRunTodos {
    let place = this_computer(&input.host_os);
    let ids = connected(input);
    let all = input.agents.as_deref().unwrap_or(&[]);
    let name = names(all, &ids, "and");
    let agent = agent_row(input);
    let computer = computer_row(input);
    let used = input.agent_used_at.is_some();
    // No agent picked: no agent line and no first request.
    let mut items: Vec<FirstRunTodo> = agent
        .iter()
        .map(|agent| FirstRunTodo {
            id: "agent".into(),
            done: agent.state == RowState::Ok,
            title: if agent.state == RowState::Fix {
                agent.title.clone()
            } else {
                format!("Connect {name}")
            },
            note: if agent.state == RowState::Ok {
                format!("{name} can use Cua.")
            } else {
                agent.note.clone()
            },
            optional: false,
            action: agent.action.clone(),
            settings_url: None,
        })
        .collect();
    // Sign-in is its own line; the computer line is what is left after it.
    items.push(FirstRunTodo {
        id: "computer".into(),
        done: computer.state == RowState::Ok,
        title: if computer.state == RowState::Ok || !input.signed_in {
            format!("Make {place} your first Space")
        } else {
            computer.title.clone()
        },
        note: if computer.state == RowState::Ok {
            "Ready now. Your agent can see the screen, click and type.".into()
        } else if !input.signed_in {
            "After you sign in.".into()
        } else {
            computer.note.clone()
        },
        optional: false,
        action: if input.signed_in {
            computer.action.clone()
        } else {
            None
        },
        settings_url: computer.settings_url.clone(),
    });
    if !ids.is_empty() {
        items.push(FirstRunTodo {
            id: "prompt".into(),
            title: "Try your first request".into(),
            note: format!(
                "Ask {}: \u{201c}{}\u{201d}",
                names(all, &ids, "or"),
                first_prompt(&input.host_os)
            ),
            done: used,
            optional: false,
            action: (!used).then(|| button("copy", "Copy")),
            settings_url: None,
        });
    }
    let extra_done = matches!(input.extra.phase.as_str(), "creating" | "ready");
    items.push(FirstRunTodo {
        id: "extra".into(),
        title: "Give your agent its own Space (optional)".into(),
        note: format!("A fresh computer inside {place}, away from your files and your mouse."),
        done: extra_done,
        optional: true,
        action: (!extra_done).then(|| button("add", "Add a Space")),
        settings_url: None,
    });
    items.push(FirstRunTodo {
        id: "sign-in".into(),
        title: "Sign in".into(),
        note: if input.signed_in {
            "Signed in to Cua.".into()
        } else {
            format!("So your agent can find {place} and your other computers.")
        },
        done: input.signed_in,
        optional: false,
        action: (!input.signed_in).then(|| button("sign-in", "Sign in")),
        settings_url: None,
    });
    let done = items.iter().filter(|i| i.done).count();
    let complete = items.iter().all(|i| i.done || i.optional);
    let left = items.iter().filter(|i| !i.done && !i.optional).count();
    let rest = match left {
        0 => "You're all set.",
        1 => "One thing is left.",
        _ => "A couple of things are left.",
    };
    let lede = match &agent {
        Some(a) if a.state == RowState::Ok => format!("{name} is connected. {rest}"),
        Some(_) => "A couple of things are left.".into(),
        None => rest.into(),
    };
    FirstRunTodos {
        title: "Finish setting up".into(),
        lede,
        progress: format!("{done} of {} done", items.len()),
        items,
        complete,
    }
}

/// One access in this computer's log (`host.status`'s `recentAccess`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessLine {
    pub at_ms: u64,
    pub via: String,
    pub who: String,
    pub what: String,
}

/// When the agent first used Cua on this computer since `since_ms`: the
/// newest connection in the access log (not a background probe, a
/// thumbnail or a refusal) at or after it. An agent names itself, and the
/// host logs only some calls, so a line naming another agent doesn't
/// count, but an unnamed one does. None: no such line yet.
pub fn agent_used(lines: &[AccessLine], since_ms: u64, agent_name: &str) -> Option<u64> {
    let want = agent_name.to_ascii_lowercase();
    let others: Vec<String> = OFFERED
        .iter()
        .map(|id| default_name(id).to_ascii_lowercase())
        .filter(|n| *n != want)
        .collect();
    lines
        .iter()
        .filter(|l| l.at_ms >= since_ms)
        .filter(|l| cua_host_access_is_connection(&l.who, &l.what))
        .filter(|l| {
            let who = format!("{} {}", l.who, l.what).to_ascii_lowercase();
            who.contains(&want) || !others.iter().any(|o| who.contains(o.as_str()))
        })
        .map(|l| l.at_ms)
        .max()
}

/// `cua_host::access::classify(..).is_connection()`, without the host
/// crate (the core builds for wasm).
fn cua_host_access_is_connection(who: &str, what: &str) -> bool {
    !(who == "refused"
        || what.starts_with("refused ")
        || what.ends_with("(background)")
        || what.ends_with("(thumbnail)")
        || what.ends_with("(thumbnail, not owner)")
        || matches!(what, "SystemService" | "PresenceService"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(configured: bool) -> FirstRunAgent {
        FirstRunAgent {
            agent: "claude-code".into(),
            name: "Claude Code".into(),
            installed: true,
            configured,
            ..Default::default()
        }
    }

    fn agent(id: &str, name: &str, installed: bool) -> FirstRunAgent {
        FirstRunAgent {
            agent: id.into(),
            name: name.into(),
            installed,
            ..Default::default()
        }
    }

    fn codex(installed: bool, signed_in: bool) -> FirstRunAgent {
        FirstRunAgent {
            sign_in: Some("ChatGPT".into()),
            signed_in,
            ..agent("codex", "OpenAI Codex", installed)
        }
    }

    fn keys(saved: &[&str]) -> crate::agent_keys::AgentKeysInput {
        crate::agent_keys::AgentKeysInput {
            keys: saved
                .iter()
                .map(|env| crate::agent_keys::AgentKeyInput {
                    provider: crate::agent_keys::provider_of(env).into(),
                    env: (*env).into(),
                    last4: "1234".into(),
                    added_ms: 1,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn connect(agents: Vec<FirstRunAgent>, picked: Option<&[&str]>) -> ConnectView {
        view(&FirstRunInput {
            host_os: "macos".into(),
            agents: Some(agents),
            picked: picked.map(|p| p.iter().map(|x| x.to_string()).collect()),
            keys: Some(keys(&["ANTHROPIC_API_KEY"])),
            ..Default::default()
        })
        .connect
        .unwrap()
    }

    fn ids(p: &[AgentPick], f: impl Fn(&AgentPick) -> bool) -> Vec<String> {
        p.iter().filter(|x| f(x)).map(|x| x.agent.clone()).collect()
    }

    fn ready_host() -> FirstRunHost {
        FirstRunHost {
            configured: true,
            running: true,
            online: Some(true),
            permissions: vec![FirstRunPermission {
                id: "accessibility".into(),
                granted: true,
                settings_url: None,
            }],
            error: None,
        }
    }

    fn setup() -> FirstRunInput {
        FirstRunInput {
            step: FirstRunStep::Setup,
            host_os: "macos".into(),
            agents: Some(vec![claude(true)]),
            agent: Some("claude-code".into()),
            signed_in: true,
            host: Some(ready_host()),
            offers: vec![
                FirstRunOffer {
                    os: "linux".into(),
                    time: Some("about 2 min".into()),
                    ..Default::default()
                },
                FirstRunOffer {
                    os: "macos".into(),
                    time: Some("10 to 30 min the first time".into()),
                    size: Some("22 GB download".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    fn s(v: &FirstRunView) -> &SetupView {
        v.setup.as_ref().unwrap()
    }

    #[test]
    fn every_agent_is_listed_once_and_installed_ones_are_ticked() {
        let all = vec![
            claude(false),
            codex(true, false),
            agent("cursor", "Cursor", false),
            agent("goose", "Goose", true),
            agent("zed", "Zed", false),
        ];
        let c = connect(all.clone(), None);
        assert_eq!(c.intro, "Pick any, or none.");
        assert_eq!(c.agents_label, "On this Mac");
        // The common ones first, then an installed other one; the rest under More.
        assert_eq!(
            ids(&c.agents, |_| true),
            [
                "claude-code",
                "codex",
                "hermes",
                "cursor",
                "openclaw",
                "goose"
            ]
        );
        assert_eq!(ids(&c.more, |_| true), ["zed"]);
        assert_eq!(c.more_label, "More agents");
        // Ticked by default: what is installed here.
        assert_eq!(
            ids(&c.agents, |p| p.checked),
            ["claude-code", "codex", "goose"]
        );
        assert_eq!(c.connects, ["claude-code", "codex", "goose"]);
        assert_eq!(c.primary.id, "continue");
        let cursor = c.agents.iter().find(|p| p.agent == "cursor").unwrap();
        assert_eq!(cursor.note.as_deref(), Some("Not found on this Mac"));
        // Nothing read yet: the common ones, none ticked.
        let none_yet = view(&FirstRunInput::default()).connect.unwrap();
        assert_eq!(ids(&none_yet.agents, |_| true), FEATURED);
        assert!(none_yet.agents.iter().all(|p| !p.checked) && none_yet.more.is_empty());
    }

    /// Drift guard: every agent the setup's registry knows is on step 1
    /// exactly once, so a newly supported agent shows up without an edit
    /// here; and every agent this file names is one the registry knows.
    #[test]
    fn every_registry_agent_is_on_step_one() {
        let registry: Vec<FirstRunAgent> = cua_agent_setup::AGENTS
            .iter()
            .map(|a| agent(a.id, a.name, false))
            .collect();
        let c = connect(registry.clone(), None);
        let mut listed = ids(&c.agents, |_| true);
        listed.extend(ids(&c.more, |_| true));
        let mut want: Vec<String> = registry.iter().map(|a| a.agent.clone()).collect();
        listed.sort();
        want.sort();
        assert_eq!(listed, want);
        let known = |id: &str| cua_agent_setup::registry::find(id).is_some_and(|a| a.id == id);
        for id in FEATURED
            .iter()
            .chain(OFFERED.iter())
            .chain(AGENT_KEYS.iter().map(|(a, _)| a))
        {
            assert!(known(id), "{id} is not a registry id");
        }
        // The agents with a sign-in the setup can start get a row for it.
        for a in cua_agent_setup::AGENTS.iter().filter(|a| a.login.is_some()) {
            let mut row = agent(a.id, a.name, true);
            row.sign_in = a.login.map(|l| l.label.to_string());
            let c = connect(vec![row], Some(&[a.id]));
            assert!(
                c.credentials
                    .iter()
                    .any(|r| r.id == format!("sign-in:{}", a.id)),
                "{}",
                a.id
            );
        }
    }

    #[test]
    fn none_is_a_choice() {
        let c = connect(vec![claude(false), codex(true, false)], Some(&[]));
        assert!(c.agents.iter().all(|p| !p.checked));
        assert!(c.connects.is_empty() && c.credentials.is_empty());
        assert_eq!(c.primary.label, "Continue");
        // Step 2 with none: no agent row and no first request; done once this Mac is.
        let mut i = setup();
        i.picked = Some(vec![]);
        let v = view(&i);
        let st = s(&v);
        assert_eq!(st.rows.len(), 1);
        assert_eq!(st.rows[0].id, "computer");
        assert!(st.prompt.is_none());
        assert_eq!(st.title, "This Mac is ready.");
        assert!(st.primary.enabled);
        assert_eq!(v.art.agent, "other");
        let t = todos(&i);
        assert!(t.items.iter().all(|x| x.id != "agent" && x.id != "prompt"));
        assert!(t.complete);
        assert_eq!(t.lede, "You're all set.");
    }

    #[test]
    fn one_or_several_and_set_up_ones_stay_ticked() {
        // Codex only.
        let c = connect(vec![claude(false), codex(true, false)], Some(&["codex"]));
        assert_eq!(ids(&c.agents, |p| p.checked), ["codex"]);
        assert_eq!(c.connects, ["codex"]);
        // Several, with one that already uses Cua: it can't be unticked here.
        let c = connect(
            vec![
                claude(true),
                codex(true, false),
                agent("hermes", "Hermes", true),
            ],
            Some(&["codex", "hermes"]),
        );
        assert_eq!(
            ids(&c.agents, |p| p.checked),
            ["claude-code", "codex", "hermes"]
        );
        assert_eq!(ids(&c.agents, |p| p.locked), ["claude-code"]);
        assert_eq!(c.connects, ["codex", "hermes"]);
        let mut i = FirstRunInput {
            agents: Some(vec![
                claude(true),
                codex(true, false),
                agent("hermes", "Hermes", true),
            ]),
            picked: Some(vec!["codex".into(), "hermes".into()]),
            connecting: true,
            ..Default::default()
        };
        assert_eq!(
            view(&i).connect.unwrap().busy.as_deref(),
            Some("Adding Cua to OpenAI Codex and Hermes…")
        );
        // Step 2 names them all.
        i.step = FirstRunStep::Setup;
        i.connecting = false;
        i.picked = Some(vec!["claude-code".into(), "codex".into()]);
        i.agents = Some(vec![
            claude(true),
            FirstRunAgent {
                configured: true,
                ..codex(true, false)
            },
        ]);
        i.signed_in = true;
        i.host = Some(ready_host());
        i.host_os = "macos".into();
        let v = view(&i);
        let st = s(&v);
        assert_eq!(st.title, "Your agents can use this Mac.");
        assert_eq!(st.rows[0].title, "Claude Code and OpenAI Codex can use Cua");
        assert_eq!(
            st.prompt.as_ref().unwrap().heading,
            "Try this in Claude Code or OpenAI Codex"
        );
        // One not set up yet: the fix names only it.
        i.agents = Some(vec![claude(true), codex(true, false)]);
        let v = view(&i);
        assert_eq!(s(&v).rows[0].title, "Connect OpenAI Codex");
        assert_eq!(
            s(&v).rows[0].note,
            "Cua isn't in OpenAI Codex's settings yet."
        );
    }

    #[test]
    fn codex_signs_in_on_this_computer_and_keys_stay_here() {
        // Codex only: its ChatGPT sign-in and the OpenAI key, never the Anthropic one.
        let c = connect(vec![claude(false), codex(true, false)], Some(&["codex"]));
        let rows: Vec<_> = c.credentials.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(rows, ["sign-in:codex", "key:openai"]);
        let chatgpt = &c.credentials[0];
        assert_eq!(chatgpt.title, "ChatGPT sign-in");
        assert_eq!(chatgpt.note, "OpenAI Codex uses it on this Mac");
        assert_eq!(chatgpt.action.as_ref().unwrap().id, "agent-sign-in");
        assert_eq!(chatgpt.agent.as_deref(), Some("codex"));
        let openai = &c.credentials[1];
        assert_eq!(
            (openai.title.as_str(), openai.status.as_deref()),
            ("OpenAI key", None)
        );
        assert_eq!(openai.action.as_ref().unwrap().id, "add-key");
        // Both: the Anthropic key is saved, so it says so and asks nothing.
        let c = connect(
            vec![claude(false), codex(true, true)],
            Some(&["claude-code", "codex"]),
        );
        let rows: Vec<_> = c
            .credentials
            .iter()
            .map(|r| (r.id.as_str(), r.status.as_deref(), r.action.is_some()))
            .collect();
        assert_eq!(
            rows,
            [
                ("sign-in:codex", Some("Signed in"), false),
                ("key:anthropic", Some("Saved"), false),
                ("key:openai", None, true),
            ]
        );
        assert!(c.credentials.iter().all(|r| r.ok == r.status.is_some()));
        // Waiting for the browser, then a failure to start.
        let mut i = FirstRunInput {
            agents: Some(vec![codex(true, false)]),
            picked: Some(vec!["codex".into()]),
            agent_signing_in: Some("codex".into()),
            ..Default::default()
        };
        let row = view(&i).connect.unwrap().credentials[0].clone();
        assert_eq!(row.status.as_deref(), Some("Finish in your browser…"));
        assert!(!row.ok, "a wait is not a done status");
        assert!(row.action.is_none());
        i.agent_signing_in = None;
        i.agent_sign_in_error = Some("codex: not found".into());
        let row = view(&i).connect.unwrap().credentials[0].clone();
        assert_eq!(
            row.error.as_deref(),
            Some("Couldn't start the sign-in: codex: not found")
        );
        // Codex not installed here: nothing to start yet.
        i.agents = Some(vec![codex(false, false)]);
        assert!(view(&i).connect.unwrap().credentials[0].action.is_none());
        // Keys this app can't read: only the sign-in.
        i.keys = Some(crate::agent_keys::AgentKeysInput {
            unavailable: Some("no vault".into()),
            ..Default::default()
        });
        assert_eq!(view(&i).connect.unwrap().credentials.len(), 1);
    }

    #[test]
    fn step_one_says_what_stays_on_this_computer() {
        let c = connect(
            vec![claude(false), codex(true, false)],
            Some(&["claude-code", "codex"]),
        );
        assert_eq!(
            c.agents_note,
            "Cua adds its skills and MCP server to each one you pick, on this Mac only. Your other computers set up their own."
        );
        assert_eq!(c.credentials_label, "Sign-ins and keys");
        assert_eq!(
            c.credentials_note,
            "They stay on this Mac. Other computers keep their own."
        );
        // Nothing follows the Cua account: the page never says it does.
        let all = serde_json::to_string(&c).unwrap().to_lowercase();
        for claim in ["account", "everywhere", "any computer", "sync"] {
            assert!(!all.contains(claim), "{claim}");
        }
        let pc = view(&FirstRunInput {
            host_os: "windows".into(),
            ..Default::default()
        })
        .connect
        .unwrap();
        assert_eq!(pc.agents_label, "On this PC");
        assert!(pc.agents_note.contains("on this PC only"));
        // Linux (and anything else): "this computer"; never "Mac" off a Mac.
        let mut linux_in = FirstRunInput {
            host_os: "linux".into(),
            agents: Some(vec![codex(true, false)]),
            picked: Some(vec!["codex".into()]),
            ..Default::default()
        };
        let linux = view(&linux_in).connect.unwrap();
        assert_eq!(linux.agents_label, "On this computer");
        assert_eq!(
            linux.credentials_note,
            "They stay on this computer. Other computers keep their own."
        );
        assert_eq!(
            linux.credentials[0].note,
            "OpenAI Codex uses it on this computer"
        );
        for os in ["linux", "windows"] {
            linux_in.host_os = os.into();
            let all = serde_json::to_string(&view(&linux_in)).unwrap();
            assert!(!all.contains("Mac"), "{os}: {all}");
        }
    }

    #[test]
    fn ready_when_both_rows_pass() {
        let v = view(&setup());
        let s = s(&v);
        assert_eq!(s.title, "Your agent can use this Mac.");
        assert_eq!(s.rows[1].title, "This Mac is your first Space");
        assert!(s.primary.enabled);
        assert_eq!(s.primary.label, "Done");
        // macOS first, then Linux.
        assert_eq!(s.extra.choices[0].os, "macos");
        assert_eq!(s.extra.choices[1].detail, "Ready in about 2 min");
        assert!(s.upsell.is_none());
    }

    #[test]
    fn checking_keeps_done_off() {
        let mut i = setup();
        i.host = None;
        let v = view(&i);
        assert_eq!(s(&v).title, "Checking your setup.");
        assert!(!s(&v).primary.enabled);
        assert_eq!(v.art.computer, "faded");
    }

    #[test]
    fn fixes_turn_done_into_finish_later() {
        let mut i = setup();
        i.host.as_mut().unwrap().permissions[0].granted = false;
        i.host.as_mut().unwrap().permissions[0].settings_url =
            Some("x-apple.systempreferences:a".into());
        let v = view(&i);
        let s = s(&v);
        assert_eq!(s.title, "One step left.");
        assert_eq!(s.rows[1].title, "Let Cua control this Mac");
        assert_eq!(
            s.rows[1].settings_url.as_deref(),
            Some("x-apple.systempreferences:a")
        );
        assert_eq!(s.primary.id, "finish-later");

        let mut i = setup();
        i.signed_in = false;
        assert_eq!(s_title(&i), "One step left.");
        assert_eq!(
            view(&i).setup.unwrap().rows[1].action.as_ref().unwrap().id,
            "sign-in"
        );

        let mut i = setup();
        i.agent_open_before_setup = Some(true);
        assert_eq!(view(&i).setup.unwrap().rows[0].title, "Restart Claude Code");
    }

    fn s_title(i: &FirstRunInput) -> String {
        view(i).setup.unwrap().title
    }

    #[test]
    fn creating_never_blocks_done() {
        let mut i = setup();
        i.extra = FirstRunExtra {
            phase: "creating".into(),
            name: Some("macOS 1".into()),
            os: Some("macos".into()),
            detail: Some("Downloading macOS · 4.2 of 22.3 GB".into()),
            fraction: Some(0.19),
            error: None,
        };
        let v = view(&i);
        assert_eq!(s(&v).extra.title, "Making macOS 1 in the background");
        assert!(s(&v).primary.enabled);
        assert_eq!(v.art.extra, "filling");
    }

    #[test]
    fn the_card_shows_only_after_the_first_success() {
        let mut i = setup();
        i.plan = Some(FirstRunPlan {
            metered: true,
            plan: "free".into(),
            ..Default::default()
        });
        assert!(s(&view(&i)).upsell.is_none());
        i.agent_used_at = Some("18:12".into());
        let v = view(&i);
        let u = s(&v).upsell.clone().unwrap();
        assert_eq!(u.plan, "pro");
        assert_eq!(
            u.lines[1],
            "Pro: 200 remote hours a month · 5 connections at once"
        );
        assert!(!u.title.to_lowercase().contains("sleep"));
        assert_eq!(s(&v).rows[0].note, "It used Cua at 18:12.");
        i.plan.as_mut().unwrap().teammates = true;
        assert_eq!(s(&view(&i)).upsell.clone().unwrap().plan, "teams");
        i.plan.as_mut().unwrap().dismissed = true;
        assert!(s(&view(&i)).upsell.is_none());
    }

    #[test]
    fn access_log_finds_the_agent() {
        let line = |at, who: &str, what: &str| AccessLine {
            at_ms: at,
            via: "token".into(),
            who: who.into(),
            what: what.into(),
        };
        let lines = vec![
            line(5, "Claude Code", "MCP"),
            line(9, "Codex", "MCP"),
            line(12, "owner", "SystemService"),
            line(1, "Claude Code", "MCP"),
        ];
        assert_eq!(agent_used(&lines, 2, "Claude Code"), Some(5));
        assert_eq!(agent_used(&lines, 6, "Claude Code"), None);
        assert_eq!(
            agent_used(&[line(7, "local", "DesktopService")], 2, "Claude Code"),
            Some(7)
        );
    }

    #[test]
    fn todos_after_finish_later() {
        let mut i = setup();
        i.host.as_mut().unwrap().permissions[0].granted = false;
        let t = todos(&i);
        assert_eq!(t.progress, "2 of 5 done");
        assert!(!t.complete);
        assert_eq!(t.items[1].title, "Let Cua control this Mac");
        i.host = Some(ready_host());
        i.agent_used_at = Some("18:12".into());
        assert!(todos(&i).complete);
    }

    #[test]
    fn plain_words() {
        let mut i = setup();
        i.extra.phase = "creating".into();
        let all = serde_json::to_string(&view(&i)).unwrap().to_lowercase();
        for word in [
            "vm",
            "virtual machine",
            "container",
            "mcp",
            "daemon",
            "host",
            "your space is ready",
        ] {
            assert!(!all.contains(&format!(" {word}")), "{word}");
        }
    }
}
