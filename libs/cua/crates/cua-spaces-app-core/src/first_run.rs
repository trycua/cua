// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The first run every app shows (0.10.0): two screens.
//!
//! 1. **Connect your agent.** One button for the agent this computer has
//!    (Claude Code first), the others under it.
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

/// The agents step 1 offers by name, in order (setup ids). Any other agent
/// is under "Other".
pub const OFFERED: [&str; 4] = ["claude-code", "codex", "hermes", "openclaw"];

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
    /// The agent the user connected (setup id).
    pub agent: Option<String>,
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

/// An agent step 1 offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunAgentChoice {
    pub agent: String,
    pub name: String,
    pub installed: bool,
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
    /// "Connect Claude Code".
    pub primary: FirstRunButton,
    /// The agent the primary connects.
    pub agent: FirstRunAgentChoice,
    /// "Your agent will use this Mac."
    pub note: String,
    /// "You can add a separate Space later."
    pub note_more: String,
    /// "Or".
    pub or_label: String,
    /// The other offered agents.
    pub others: Vec<FirstRunAgentChoice>,
    /// "Other": every agent the setup knows.
    pub other_label: String,
    /// "Adding Cua to Claude Code…" while it runs.
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
    pub prompt: PromptView,
    pub extra: ExtraView,
    pub upsell: Option<UpsellCard>,
    pub primary: PrimaryView,
}

/// The first run as drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunView {
    pub step: FirstRunStep,
    /// "Set up later" (top right, both screens).
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
        _ => "your agent",
    }
}

fn choice(agents: &[FirstRunAgent], id: &str) -> FirstRunAgentChoice {
    let row = agents.iter().find(|a| a.agent == id);
    FirstRunAgentChoice {
        agent: id.into(),
        name: row
            .map(|a| a.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| default_name(id).into()),
        installed: row.is_some_and(|a| a.installed),
    }
}

/// The agent step 1 leads with: the first offered one installed here,
/// else Claude Code.
pub fn lead_agent(agents: &[FirstRunAgent]) -> String {
    OFFERED
        .iter()
        .find(|id| agents.iter().any(|a| a.agent == **id && a.installed))
        .unwrap_or(&OFFERED[0])
        .to_string()
}

fn agent_name(input: &FirstRunInput) -> String {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let id = input.agent.clone().unwrap_or_else(|| lead_agent(agents));
    choice(agents, &id).name
}

fn connect_view(input: &FirstRunInput) -> ConnectView {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let lead = input.agent.clone().unwrap_or_else(|| lead_agent(agents));
    let agent = choice(agents, &lead);
    let place = this_computer(&input.host_os);
    let others = OFFERED
        .iter()
        .filter(|id| **id != lead)
        .map(|id| choice(agents, id))
        .collect();
    ConnectView {
        title: "A shared Space for every agent.".into(),
        accent: "agent".into(),
        lede: format!(
            "Connect your agents and give them computers to work on: macOS, Linux or Windows, right here on {place}."
        ),
        primary: button("connect", &format!("Connect {}", agent.name)),
        busy: input
            .connecting
            .then(|| format!("Adding Cua to {}…", agent.name)),
        agent,
        note: format!("Your agent will use {place}."),
        note_more: "You can add a separate Space later.".into(),
        or_label: "Or".into(),
        others,
        other_label: "Other".into(),
        error: input
            .connect_error
            .as_ref()
            .map(|e| format!("Couldn't add Cua: {e}")),
    }
}

fn agent_row(input: &FirstRunInput) -> CheckRow {
    let name = agent_name(input);
    let id = input.agent.clone().unwrap_or_default();
    let found = input
        .agents
        .as_ref()
        .map(|a| a.iter().find(|r| r.agent == id).cloned());
    let row = |state, title: String, note: String, action: Option<FirstRunButton>| CheckRow {
        id: "agent".into(),
        state,
        title,
        note,
        action,
        settings_url: None,
    };
    match found {
        None => row(
            RowState::Checking,
            name.clone(),
            "Checking that it can use Cua…".into(),
            None,
        ),
        Some(None) | Some(Some(FirstRunAgent { configured: false, .. })) => {
            if input.connecting {
                return row(
                    RowState::Checking,
                    name.clone(),
                    format!("Adding Cua to {name}'s settings…"),
                    None,
                );
            }
            row(
                RowState::Fix,
                format!("Connect {name}"),
                input.connect_error.clone().map_or_else(
                    || format!("Cua isn't in {name}'s settings yet."),
                    |e| format!("Couldn't add Cua: {e}"),
                ),
                Some(button("connect", "Connect")),
            )
        }
        Some(Some(_)) => {
            if let Some(at) = &input.agent_used_at {
                return row(
                    RowState::Ok,
                    format!("{name} can use Cua"),
                    format!("It used Cua at {at}."),
                    None,
                );
            }
            if input.agent_open_before_setup == Some(true) {
                return row(
                    RowState::Fix,
                    format!("Restart {name}"),
                    "It was open when we added Cua. Close it, then open it again.".into(),
                    Some(button("restart-check", "Check again")),
                );
            }
            row(
                RowState::Ok,
                format!("{name} can use Cua"),
                format!(
                    "We added Cua to {name}'s settings. If {name} was already open, restart it."
                ),
                None,
            )
        }
    }
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
            Some(button("set-up", if host.error.is_some() { "Try again" } else { "Set up" })),
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
            format!("So your agent can click and type. {} asks you once.", os_name(&input.host_os)),
        ),
        (
            "screen-recording",
            format!("Let Cua see {}", if input.host_os == "macos" { "this screen" } else { "the screen" }),
            format!("So your agent can see what's on {place}. {} asks you once.", os_name(&input.host_os)),
        ),
    ] {
        if let Some(p) = missing_permission(host, id) {
            let mut r = row(RowState::Fix, title, &note, Some(button("open-settings", "Open Settings")));
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
        title: "Reach your computers from anywhere for longer".into(),
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
    ArtView {
        agent: input.agent.clone().unwrap_or_else(|| lead_agent(agents)),
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
    let later = "Set up later".to_string();
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
    let name = agent_name(input);
    let rows = vec![agent, computer];
    let checking = rows.iter().any(|r| r.state == RowState::Checking);
    let fixes = rows.iter().filter(|r| r.state == RowState::Fix).count();
    let ready = rows.iter().all(|r| r.state == RowState::Ok);
    let title = if ready {
        format!("Your agent can use {place}.")
    } else if fixes == 1 && !checking {
        "One step left.".into()
    } else if fixes > 1 && !checking {
        format!("{fixes} steps left.")
    } else {
        "Checking your setup.".into()
    };
    let used = input.agent_used_at.is_some();
    let prompt = if used {
        PromptView {
            heading: format!("You asked {name}"),
            text: first_prompt(&input.host_os).into(),
            note: format!("It answered in {name}. Ask it anything else."),
            copy: None,
        }
    } else {
        PromptView {
            heading: format!("Try this in {name}"),
            text: first_prompt(&input.host_os).into(),
            note: format!("This works now, on {place}."),
            copy: Some("Copy".into()),
        }
    };
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
    let name = agent_name(input);
    let agent = agent_row(input);
    let computer = computer_row(input);
    let used = input.agent_used_at.is_some();
    let mut items = vec![FirstRunTodo {
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
    }];
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
    items.push(FirstRunTodo {
        id: "prompt".into(),
        title: "Try your first request".into(),
        note: format!("Ask {name}: \u{201c}{}\u{201d}", first_prompt(&input.host_os)),
        done: used,
        optional: false,
        action: (!used).then(|| button("copy", "Copy")),
        settings_url: None,
    });
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
    let lede = if agent.state == RowState::Ok {
        let left = items.iter().filter(|i| !i.done && !i.optional).count();
        match left {
            0 => format!("{name} is connected. You're all set."),
            1 => format!("{name} is connected. One thing is left."),
            _ => format!("{name} is connected. A couple of things are left."),
        }
    } else {
        "A couple of things are left.".into()
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
        }
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
    fn connect_leads_with_an_installed_agent() {
        let mut i = FirstRunInput {
            host_os: "macos".into(),
            ..Default::default()
        };
        let v = view(&i);
        let c = v.connect.unwrap();
        assert_eq!(c.primary.label, "Connect Claude Code");
        assert_eq!(c.note, "Your agent will use this Mac.");
        assert_eq!(c.others.len(), 3);
        i.agents = Some(vec![FirstRunAgent {
            agent: "codex".into(),
            name: "Codex".into(),
            installed: true,
            configured: false,
        }]);
        assert_eq!(view(&i).connect.unwrap().primary.label, "Connect Codex");
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
        i.host.as_mut().unwrap().permissions[0].settings_url = Some("x-apple.systempreferences:a".into());
        let v = view(&i);
        let s = s(&v);
        assert_eq!(s.title, "One step left.");
        assert_eq!(s.rows[1].title, "Let Cua control this Mac");
        assert_eq!(s.rows[1].settings_url.as_deref(), Some("x-apple.systempreferences:a"));
        assert_eq!(s.primary.id, "finish-later");

        let mut i = setup();
        i.signed_in = false;
        assert_eq!(s_title(&i), "One step left.");
        assert_eq!(view(&i).setup.unwrap().rows[1].action.as_ref().unwrap().id, "sign-in");

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
        assert_eq!(u.lines[1], "Pro: 200 remote hours a month · 5 connections at once");
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
        assert_eq!(agent_used(&[line(7, "local", "DesktopService")], 2, "Claude Code"), Some(7));
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
        for word in ["vm", "virtual machine", "container", "mcp", "daemon", "host", "your space is ready"] {
            assert!(!all.contains(&format!(" {word}")), "{word}");
        }
    }
}
