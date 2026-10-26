// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The first run every app shows: a few short screens, agent first.
//!
//! 1. **Welcome.** "Your computers, shared by your team and its agents." One button.
//! 2. **Pick your agent.** The agents found on this computer, in the order
//!    [`PREFERENCE`] gives (Hermes, Claude Code, Codex, Cursor, then the
//!    rest), one Connect each. Nothing is connected until its button is
//!    pressed, and each row names the file Cua changes. The connected
//!    agents' own sign-ins and keys follow, in the same order. Nothing
//!    found: install an agent, or use the CLI.
//!    Then **Install the Cua CLI**, only while the app's bundled CLI is not
//!    on this computer (an app installed from the disk image has none on
//!    PATH): what it installs and the exact file, Install or Not now, and
//!    the result. It is the app's install plan ([`crate::installer`]):
//!    nothing is written before Install, and the file is the channel's
//!    (`cua`, or `cua-local` for Cua Spaces Local).
//! 3. **Add your other computers** (optional). Signing in lets your other
//!    computers and your cloud use this computer's Spaces. "Not now" skips
//!    it: this computer works without an account.
//! 4. **Share this Mac's screen** (only a signed-in Mac that shares its
//!    screen). Screen Recording and Accessibility, one row each, with Allow
//!    (the app's guide to the System Settings pane) and a check once
//!    granted; "Not now" leaves them for Settings. macOS's own prompts never
//!    show first: the app asks here, in its own words.
//! 5. **Try it in your agent.** A first request for the lead connected agent
//!    (read-only, with Copy) that makes a Space on this computer, and a line
//!    that turns green when the agent used Cua. No agent: a one-click first
//!    Space instead.
//!
//! Nothing starts or connects by itself, permissions are asked only on a Mac
//! that shares its screen (screen 4), and no plan is shown. The words, the
//! order and which button does what are decided here, so every app says the
//! same thing; the page gathers the readings and draws what [`view`]
//! returns.
//!
//! What each reading is, and how real it is:
//!
//! * **"Cua is in the agent's settings"**: the agent setup reports the
//!   agent `configured` (its MCP config and skills are in place). Real.
//! * **"The agent used Cua"**: a connection in this computer's access log
//!   since the try screen opened ([`agent_used`]). Partly real: the host
//!   logs only some calls, and an agent names itself.
//! * **This computer in your account**: the app's sign-in and `host.status`.
//!   Real.

use serde::{Deserialize, Serialize};

use crate::relay_usage::{BUY_PRO_PATH, BUY_TEAMS_PATH, PRO_HOURS, PRO_SESSIONS, buy, plan_kind};

/// The agents the first run leads with, most preferred first (setup ids).
/// The first one found on this computer names the first request and the
/// art; any other agent the setup knows comes after these, in its order.
pub const PREFERENCE: [&str; 5] = ["hermes", "claude-code", "codex", "cursor", "openclaw"];

/// The agents the access log tells apart by name (setup ids).
pub const OFFERED: [&str; 5] = PREFERENCE;

/// The provider key each agent's runs in a Space read (`agent_keys`'
/// providers), for the keys listed under the connected agents.
const AGENT_KEYS: [(&str, &str); 2] = [("claude-code", "anthropic"), ("codex", "openai")];

/// Home's to-do request: it works on this computer with nothing else set up.
pub const FIRST_PROMPT: &str = "List my computers and tell me what's on this Mac";
const FIRST_PROMPT_OTHER: &str = "List my computers and tell me what's on this computer";
const FIRST_PROMPT_PC: &str = "List my computers and tell me what's on this PC";

/// The try screen's request: the agent makes a Space on this computer.
pub const TRY_PROMPT: &str = "Make a Linux Space on this Mac and show its desktop in a new window.";
const TRY_PROMPT_OTHER: &str =
    "Make a Linux Space on this computer and show its desktop in a new window.";
const TRY_PROMPT_PC: &str = "Make a Linux Space on this PC and show its desktop in a new window.";

/// The CLI's name when the host did not say (the released app's,
/// `CuaChannel::cli_name`). A Local build's host passes its own.
fn default_cli_name() -> &'static str {
    cua_home::channel::CuaChannel::Stable.cli_name()
}

/// The same first Space from a terminal (no agent found), under the CLI
/// name of the app's channel: `cua spaces create linux --on local`, or
/// `cua-local spaces create linux --on local` in Cua Spaces Local.
pub fn cli_first_space(cli: &str) -> String {
    format!("{cli} spaces create linux --on local")
}

/// Where to get an agent (none found).
pub const AGENTS_GUIDE_URL: &str = "https://cua.ai/docs/spaces/guides/connect-your-agent";

/// The screens. `connect` and `setup` keep their old names, so a run
/// saved by an older app resumes on the matching screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirstRunStep {
    #[default]
    Welcome,
    /// Pick your agent.
    Connect,
    /// Install the Cua CLI (only while it is not installed).
    Cli,
    /// Add your other computers.
    Devices,
    /// Share this Mac's screen: Screen Recording and Accessibility. Only on
    /// a signed-in Mac that shares its screen ([`asks_permissions`]).
    Permissions,
    /// Try it in your agent.
    Setup,
}

/// The app's bundled CLI, as its install plan reads it (the Command line
/// screen, and Settings' row).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunCli {
    /// What installing would do (`cli.plan`; none: not read yet, or this
    /// app can't install it).
    pub plan: Option<crate::installer::CliInstallPlan>,
    /// Install is running.
    pub installing: bool,
    /// Installed from this screen: the file written.
    pub installed: Option<String>,
    /// Installing failed with this.
    pub error: Option<String>,
}

impl FirstRunCli {
    /// The CLI is on this computer: installed here, the target is current,
    /// or a working one of this channel was found.
    pub fn present(&self) -> bool {
        self.installed.is_some()
            || self
                .plan
                .as_ref()
                .is_some_and(|p| p.up_to_date || p.found.is_some())
    }

    /// The app carries a CLI it can install, and none is on this computer.
    pub fn missing(&self) -> bool {
        !self.present() && self.plan.as_ref().is_some_and(|p| p.source.is_some())
    }

    /// Where it is (installed here, current at the target, or found).
    fn location(&self) -> Option<String> {
        self.installed.clone().or_else(|| {
            let p = self.plan.as_ref()?;
            if p.up_to_date {
                Some(p.target.clone())
            } else {
                p.found.clone()
            }
        })
    }
}

/// The screens this run shows, in order: Install the Cua CLI only while
/// the app's CLI is missing (and while it is the screen shown, so its
/// result stays until Continue); Share this Mac's screen only where
/// [`asks_permissions`].
pub fn steps(input: &FirstRunInput) -> Vec<FirstRunStep> {
    let cli =
        input.step == FirstRunStep::Cli || input.cli.installed.is_some() || input.cli.missing();
    let mut out = vec![FirstRunStep::Welcome, FirstRunStep::Connect];
    if cli {
        out.push(FirstRunStep::Cli);
    }
    out.push(FirstRunStep::Devices);
    if asks_permissions(input) {
        out.push(FirstRunStep::Permissions);
    }
    out.push(FirstRunStep::Setup);
    out
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
    /// The settings file Cua adds itself to (`~/.claude.json`), when the
    /// agent has one.
    pub mcp_config: Option<String>,
}

/// A permission this computer still needs (`host.status`'s hint: the host
/// lists only the missing ones).
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
    /// People it is shared with see and use its screen (none: not known,
    /// the default for a new host: yes).
    pub share_desktop: Option<bool>,
    /// The name System Settings lists the app under ("Cua Spaces", "Cua
    /// Spaces Local"; none: not known).
    pub app_name: Option<String>,
}

/// A first Space this computer can make in one click (the Spaces page's
/// one-click offer, `firstSpaceOffers`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirstRunOffer {
    /// What the person picks (`linux`, `omarchy`, `macos`; the
    /// wizard's `firstSpaceOffers` ids). Empty: `os`.
    pub id: String,
    /// The guest system, `macos` or `linux`.
    pub os: String,
    /// "about 2 min", "10 to 30 min the first time".
    pub time: Option<String>,
    /// "22 GB download", "Already downloaded".
    pub size: Option<String>,
    /// Why it can't be made here (the core's words).
    pub blocked: Option<String>,
}

/// The one-click first Space, as far as it got.
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

/// The plan, for [`upsell`] (not shown in the first run).
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
    /// An older saved run's one connected agent (setup id).
    pub agent: Option<String>,
    /// The agents connected on this screen (setup ids). An agent the setup
    /// reports `configured` counts as connected either way.
    pub picked: Option<Vec<String>>,
    /// The provider keys saved on this computer (none: not read, or this
    /// app can't read them).
    pub keys: Option<crate::agent_keys::AgentKeysInput>,
    /// The agent whose own sign-in is waiting for the browser.
    pub agent_signing_in: Option<String>,
    /// Starting an agent's own sign-in failed with this.
    pub agent_sign_in_error: Option<String>,
    /// Adding Cua to an agent's settings is running.
    pub connecting: bool,
    /// The agent being connected (none with `connecting`: every one asked).
    pub connecting_agent: Option<String>,
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
    /// The first Space offers, in the order the wizard's
    /// `firstSpaceOffers` gives them (Linux, Omarchy, macOS).
    pub offers: Vec<FirstRunOffer>,
    pub extra: FirstRunExtra,
    pub plan: Option<FirstRunPlan>,
    /// The app's bundled CLI (the Command line screen).
    pub cli: FirstRunCli,
    /// The CLI's name (`SettingsSnapshot.cli`: `CuaChannel::cli_name`);
    /// none: the install plan's file name, else `cua`.
    pub cli_name: Option<String>,
}

/// The CLI's name for `input`: the host's, else the install plan's file
/// name, else the released app's.
pub fn cli_name(input: &FirstRunInput) -> String {
    let given = input
        .cli_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let planned = input.cli.plan.as_ref().and_then(|p| {
        let file = p.target.rsplit(['/', '\\']).next()?;
        let file = file.strip_suffix(".exe").unwrap_or(file);
        (!file.is_empty()).then_some(file)
    });
    given.or(planned).unwrap_or(default_cli_name()).to_string()
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

/// Screen 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WelcomeView {
    /// "Your computers, shared by your team and its agents."
    pub title: String,
    pub lede: String,
    /// "Get started" (`start`).
    pub primary: FirstRunButton,
}

/// An agent found on this computer (screen 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPick {
    pub agent: String,
    pub name: String,
    /// The file Connect changes, as the page shows it (`~/.claude.json`).
    pub file: Option<String>,
    /// Cua is in its settings.
    pub connected: bool,
    /// "Connect" (`connect`, with `agent`), none once connected or while
    /// it runs.
    pub action: Option<FirstRunButton>,
    /// "Connected", "Connecting…".
    pub note: Option<String>,
}

/// Something to do when no agent was found (screen 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAlternative {
    /// `install` or `cli`.
    pub id: String,
    pub title: String,
    pub note: String,
    /// `install` opens this page.
    pub url: Option<String>,
    /// `cli`: the command, shown read-only with Copy.
    pub command: Option<String>,
}

/// A sign-in or key of a connected agent (screen 2).
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

/// Screen 2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectView {
    /// "Pick your agent."
    pub title: String,
    pub lede: String,
    /// The agents found here, most preferred first (none: still looking,
    /// or nothing found: `alternatives`).
    pub agents: Vec<AgentPick>,
    /// Nothing found: install an agent, or use the CLI.
    pub alternatives: Vec<AgentAlternative>,
    /// The connected agents' own sign-ins and provider keys, in the
    /// agents' order (empty: the section is not drawn).
    pub credentials: Vec<CredentialRow>,
    /// "They stay on this Mac."
    pub credentials_note: String,
    /// "Continue".
    pub primary: FirstRunButton,
    /// "Skip" (none once an agent is connected).
    pub secondary: Option<FirstRunButton>,
    pub error: Option<String>,
}

/// A row of screen 3 (what signing in connects).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRow {
    pub title: String,
    pub note: String,
}

/// Screen 3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesView {
    /// "Add your other computers.", "This Mac is in your account."
    pub title: String,
    pub lede: String,
    pub rows: Vec<DeviceRow>,
    /// The sign-in waiting, or this computer being added to the account.
    pub status: Option<CheckRow>,
    /// "Sign in" (`sign-in`), or "Continue" once signed in.
    pub primary: FirstRunButton,
    /// "Not now" (`not-now`) before sign-in.
    pub secondary: Option<FirstRunButton>,
    /// "Not now keeps Cua on this Mac only. You can sign in later."
    pub note: Option<String>,
}

/// A checked row's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RowState {
    Checking,
    Ok,
    Fix,
}

/// A line with a state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRow {
    /// `agent`, `computer`, `account`.
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
    /// "Paste this in Hermes."
    pub heading: String,
    /// Shown read-only, with Copy.
    pub text: String,
    pub note: String,
    /// "Copy".
    pub copy: Option<String>,
    /// The same request for each system a first Space can run (macOS only
    /// on a Mac), shown as tabs over the text; `text` is the selected one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<PromptTab>,
    /// The tab `text` shows (`linux`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
}

/// One system's version of the try request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTab {
    /// `macos`, `linux`, `omarchy`.
    pub id: String,
    /// "macOS", "Linux", "Omarchy".
    pub label: String,
    pub text: String,
}

/// A choice on the first Space card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraChoice {
    /// What picking it sends (`linux`, `omarchy`, `macos`).
    pub id: String,
    /// The guest system, for its icon (`linux` for Omarchy).
    pub os: String,
    /// "Linux", "Omarchy", "macOS".
    pub label: String,
    /// "Ready in about 2 min", "22 GB download · 10 to 30 min the first time".
    pub detail: String,
    /// Why it can't be made here.
    pub blocked: Option<String>,
}

/// The one-click first Space (screen 4 with no agent).
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

/// The plan card [`upsell`] makes (Pro, or Teams for someone who shares
/// with teammates). Only what the plans really include.
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
    /// `done`.
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub note: Option<String>,
}

/// The art beside the page: which keys show. `agent` is the agent key's
/// mark (setup id, `other` for none). `computer`: `on`, `faded`. `extra`:
/// `none`, `outline`, `filling`, `on`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtView {
    pub agent: String,
    pub computer: String,
    pub extra: String,
}

/// Screen 3b: the two permissions a shared screen needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsView {
    /// "Share this Mac's screen."
    pub title: String,
    /// "To let your team and agents see and use this Mac, allow two
    /// permissions."
    pub lede: String,
    /// Screen Recording, then Accessibility (`id` the host's permission
    /// id): `ok` once granted, `fix` with Allow (`allow`; `settings_url`
    /// the pane), `checking` while this computer is still being added.
    pub rows: Vec<CheckRow>,
    /// What Allow does, naming the app as System Settings lists it.
    pub note: String,
    /// "Continue" once both are granted.
    pub primary: Option<FirstRunButton>,
    /// "Not now" (`not-now`) while one is missing: Settings has them later.
    pub secondary: Option<FirstRunButton>,
}

/// Screen 4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupView {
    /// "Try it in Hermes.", "Try a first Space."
    pub title: String,
    pub lede: String,
    /// None with no connected agent.
    pub prompt: Option<PromptView>,
    /// "Waiting for Hermes…", then "Hermes used Cua at 18:12."
    pub status: Option<CheckRow>,
    /// The one-click first Space, with no connected agent.
    pub extra: Option<ExtraView>,
    pub primary: PrimaryView,
}

/// The first run as drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunView {
    pub step: FirstRunStep,
    /// "Skip for now" (top right, every screen).
    pub later: String,
    pub welcome: Option<WelcomeView>,
    pub connect: Option<ConnectView>,
    pub devices: Option<DevicesView>,
    pub permissions: Option<PermissionsView>,
    pub setup: Option<SetupView>,
    /// Install the Cua CLI.
    pub cli: Option<CliView>,
    pub art: ArtView,
    /// The screens this run shows ([`steps`]), for Back and Continue.
    pub steps: Vec<FirstRunStep>,
    /// One page mark per screen.
    pub dots: u8,
    pub current: u8,
}

/// A fact on the Command line screen ("Installs", "~/.local/bin/cua").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliFact {
    pub label: String,
    pub value: String,
}

/// Install the Cua CLI: the first run's screen, and Settings' row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliView {
    pub title: String,
    pub lede: String,
    /// The CLI's name (`cua`, `cua-local`).
    pub name: String,
    /// What Install writes: the file, and the PATH line when one is needed.
    pub facts: Vec<CliFact>,
    /// One plain line on where it stands (Settings' description).
    pub status: String,
    /// It is on this computer.
    pub installed: bool,
    /// Install is running.
    pub busy: bool,
    /// A command to try once it is installed.
    pub try_command: Option<String>,
    pub error: Option<String>,
    /// `install` ("Install", "Try again"), or `continue` once installed.
    pub primary: Option<FirstRunButton>,
    /// `not-now` until it is installed.
    pub secondary: Option<FirstRunButton>,
}

/// A line of Home's "Finish setting up" list.
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

/// Home's "Finish setting up" list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunTodos {
    pub title: String,
    pub lede: String,
    /// "2 of 5 done".
    pub progress: String,
    pub items: Vec<FirstRunTodo>,
    /// Every required line is done (the list can go).
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
        "omarchy" => "Omarchy",
        "windows" => "Windows",
        _ => "a computer",
    }
}

/// Home's to-do request on this system.
pub fn first_prompt(os: &str) -> &'static str {
    match os {
        "macos" => FIRST_PROMPT,
        "windows" => FIRST_PROMPT_PC,
        _ => FIRST_PROMPT_OTHER,
    }
}

/// The try screen's request on this system.
pub fn try_prompt(os: &str) -> &'static str {
    match os {
        "macos" => TRY_PROMPT,
        "windows" => TRY_PROMPT_PC,
        _ => TRY_PROMPT_OTHER,
    }
}

/// The try request for each system a first Space can run here: macOS (on a
/// Mac only), Linux and Omarchy. Linux comes selected: it is ready fastest.
pub fn try_prompt_tabs(os: &str) -> Vec<PromptTab> {
    let place = this_computer(os);
    let mut tabs = Vec::new();
    if os == "macos" {
        tabs.push(("macos", "macOS", "a macOS"));
    }
    tabs.push(("linux", "Linux", "a Linux"));
    tabs.push(("omarchy", "Omarchy", "an Omarchy"));
    tabs.into_iter()
        .map(|(id, label, kind)| PromptTab {
            id: id.into(),
            label: label.into(),
            text: format!("Make {kind} Space on {place} and show its desktop in a new window."),
        })
        .collect()
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

/// Where `id` sorts: its place in [`PREFERENCE`], else after all of them
/// in the setup's own order.
fn rank(agents: &[FirstRunAgent], id: &str) -> usize {
    PREFERENCE.iter().position(|p| *p == id).unwrap_or_else(|| {
        PREFERENCE.len()
            + agents
                .iter()
                .position(|a| a.agent == id)
                .unwrap_or(agents.len())
    })
}

/// `ids` in preference order (each once).
fn ordered(agents: &[FirstRunAgent], ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out.sort_by_key(|id| rank(agents, id));
    out
}

/// The agents found on this computer (installed, or with Cua in their
/// settings), most preferred first.
pub fn detected(agents: &[FirstRunAgent]) -> Vec<String> {
    ordered(
        agents,
        agents
            .iter()
            .filter(|a| a.installed || a.configured)
            .map(|a| a.agent.clone()),
    )
}

/// The agent a single-agent line leads with: the most preferred one found
/// on this computer, else `other` (never a default agent).
pub fn lead_agent(agents: &[FirstRunAgent]) -> String {
    detected(agents)
        .into_iter()
        .next()
        .unwrap_or_else(|| "other".into())
}

/// "Hermes", "Hermes and Codex", "Hermes, Claude Code and Codex"
/// (`or` for a request: "Hermes or Codex").
fn names(agents: &[FirstRunAgent], ids: &[String], and: &str) -> String {
    let n: Vec<String> = ids.iter().map(|id| name_of(agents, id)).collect();
    match n.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} {and} {last}", rest.join(", ")),
    }
}

/// The connected agents, most preferred first: each one the setup reports
/// with Cua in its settings, and each one connected on screen 2 (or an
/// older run's one agent).
pub fn picked(input: &FirstRunInput) -> Vec<String> {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let mut ids: Vec<String> = match &input.picked {
        Some(p) => p.clone(),
        None => input.agent.clone().into_iter().collect(),
    };
    ids.extend(
        agents
            .iter()
            .filter(|a| a.configured)
            .map(|a| a.agent.clone()),
    );
    ordered(agents, ids)
}

/// The agent the first request names: the most preferred connected one.
fn lead_of(input: &FirstRunInput) -> Option<String> {
    picked(input).into_iter().next()
}

/// `/Users/ada/.claude.json` as `~/.claude.json` (a path already under
/// `~` stays).
fn tilde(path: &str) -> String {
    for home in ["/Users/", "/home/"] {
        if let Some(rest) = path.strip_prefix(home)
            && let Some((_, after)) = rest.split_once('/')
        {
            return format!("~/{after}");
        }
    }
    if let Some(rest) = path.strip_prefix("C:\\Users\\")
        && let Some((_, after)) = rest.split_once('\\')
    {
        return format!("~\\{after}");
    }
    path.into()
}

fn agent_picks(input: &FirstRunInput) -> Vec<AgentPick> {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let connected = picked(input);
    detected(agents)
        .into_iter()
        .map(|id| {
            let row = agents.iter().find(|a| a.agent == id);
            // Connected here: Cua is in its settings. A run that pressed
            // Connect and is waiting for the setup to say so counts too.
            let is_connected = row.is_some_and(|a| a.configured)
                || (connected.contains(&id) && input.agents.is_some() && !input.connecting);
            let busy = input.connecting
                && input.connecting_agent.as_deref().is_none_or(|a| a == id)
                && !row.is_some_and(|a| a.configured);
            AgentPick {
                name: name_of(agents, &id),
                file: row.and_then(|a| a.mcp_config.as_deref()).map(tilde),
                connected: is_connected,
                action: (!is_connected && !busy).then(|| button("connect", "Connect")),
                note: if busy {
                    Some("Connecting…".into())
                } else if is_connected {
                    Some("Connected".into())
                } else {
                    None
                },
                agent: id,
            }
        })
        .collect()
}

/// The connected agents' own sign-ins (Codex's ChatGPT sign-in), then the
/// provider keys their runs in a Space read, when this app can read keys;
/// both in the agents' order.
fn credentials(input: &FirstRunInput, connected: &[String]) -> Vec<CredentialRow> {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let place = this_computer(&input.host_os);
    let mut out = Vec::new();
    for id in connected {
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
                .map(|e| format!("Couldn't start sign-in: {e}")),
        });
    }
    let Some(keys) = input
        .keys
        .as_ref()
        .filter(|k| k.unavailable.is_none() && k.error.is_none())
    else {
        return out;
    };
    let rows = crate::agent_keys::view(keys).rows;
    for id in connected {
        let Some((_, provider)) = AGENT_KEYS.iter().find(|(a, _)| a == id) else {
            continue;
        };
        let Some(row) = rows.iter().find(|r| r.provider == *provider) else {
            continue;
        };
        out.push(CredentialRow {
            id: format!("key:{provider}"),
            title: format!("{} key", row.title),
            note: format!("{} uses it in your Spaces", name_of(agents, id)),
            status: row.set.then(|| "Saved".into()),
            ok: row.set,
            action: (!row.set).then(|| button("add-key", "Add key")),
            agent: None,
            provider: Some((*provider).into()),
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

fn welcome_view(input: &FirstRunInput) -> WelcomeView {
    let place = this_computer(&input.host_os);
    WelcomeView {
        title: "Your computers, shared by your team and its agents.".into(),
        lede: format!("Run agents in Spaces on {place}, or on any computer your team connects."),
        primary: button("start", "Get started"),
    }
}

fn connect_view(input: &FirstRunInput) -> ConnectView {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    let place = this_computer(&input.host_os);
    let list = agent_picks(input);
    let connected: Vec<String> = list
        .iter()
        .filter(|p| p.connected)
        .map(|p| p.agent.clone())
        .collect();
    let none = input.agents.is_some() && list.is_empty();
    // Nothing found: the agents Cua sets up, in the same order.
    let names_all = names(
        agents,
        &PREFERENCE[..4]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "or",
    );
    ConnectView {
        title: if none {
            "Connect an agent.".into()
        } else {
            "Pick your agent.".into()
        },
        lede: if input.agents.is_none() {
            format!("Looking for agents on {place}…")
        } else if none {
            format!("No agents found on {place}. Install one, or use Cua from a terminal.")
        } else {
            format!("Cua found these on {place}. Connect the ones you want to use with Cua.")
        },
        agents: list,
        alternatives: if none {
            vec![
                AgentAlternative {
                    id: "install".into(),
                    title: "Install an agent".into(),
                    note: format!("{names_all}, then come back here."),
                    url: Some(AGENTS_GUIDE_URL.into()),
                    command: None,
                },
                AgentAlternative {
                    id: "cli".into(),
                    title: "Use the Terminal".into(),
                    note: format!("Make a Space on {place} from the command line."),
                    url: None,
                    command: Some(cli_first_space(&cli_name(input))),
                },
            ]
        } else {
            vec![]
        },
        credentials: credentials(input, &connected),
        credentials_note: format!("Sign-ins and keys stay on {place}."),
        primary: button("continue", "Continue"),
        secondary: connected.is_empty().then(|| button("skip", "Skip")),
        error: input
            .connect_error
            .as_ref()
            .map(|e| format!("Couldn't connect Cua: {e}")),
    }
}

/// This computer in the account, once signed in (screen 3): added, being
/// added, or the reason it wasn't. Permissions are not asked here.
fn account_row(input: &FirstRunInput) -> Option<CheckRow> {
    let place = this_computer(&input.host_os);
    let row = |state, title: String, note: String, action: Option<FirstRunButton>| CheckRow {
        id: "account".into(),
        state,
        title,
        note,
        action,
        settings_url: None,
    };
    if !input.signed_in {
        return input.signing_in.then(|| {
            row(
                RowState::Checking,
                "Signing in".into(),
                "Finish signing in to Cua in your browser.".into(),
                None,
            )
        });
    }
    let Some(host) = &input.host else {
        return Some(row(
            RowState::Checking,
            format!("Adding {place}"),
            "Checking your account…".into(),
            None,
        ));
    };
    if input.host_setting_up {
        return Some(row(
            RowState::Checking,
            format!("Adding {place}"),
            "It takes a few seconds.".into(),
            None,
        ));
    }
    if !host.configured {
        return Some(row(
            RowState::Fix,
            format!("Add {place} to your account"),
            host.error.as_ref().map_or_else(
                || "It takes a few seconds.".into(),
                |e| format!("Couldn't add it: {e}"),
            ),
            Some(button(
                "set-up",
                if host.error.is_some() {
                    "Try again"
                } else {
                    "Add"
                },
            )),
        ));
    }
    Some(row(
        RowState::Ok,
        "Ready to share".into(),
        "Its Spaces appear on every computer signed in to your account.".into(),
        None,
    ))
}

fn devices_view(input: &FirstRunInput) -> DevicesView {
    let place = this_computer(&input.host_os);
    let rows = vec![
        DeviceRow {
            title: "Your other computers".into(),
            note: "Sign in with the same account".into(),
        },
        DeviceRow {
            title: "Your cloud".into(),
            note: "Hetzner, AWS, Google Cloud or Modal".into(),
        },
    ];
    if input.signed_in {
        let status = account_row(input);
        let added = status.as_ref().is_some_and(|r| r.state == RowState::Ok);
        return DevicesView {
            title: if added {
                format!("{} is in your account.", cap(place))
            } else {
                format!("Adding {place} to your account.")
            },
            lede: "Sign in on your other computers to use its Spaces.".into(),
            rows,
            status,
            primary: button("continue", "Continue"),
            secondary: None,
            note: None,
        };
    }
    DevicesView {
        title: "Add your other computers.".into(),
        lede: format!(
            "Sign in to Cua here and on your other computers. Then they can all use {place}'s Spaces."
        ),
        rows,
        status: account_row(input),
        primary: button("sign-in", "Sign in"),
        secondary: Some(button("not-now", "Not now")),
        note: Some(format!(
            "Not now keeps Cua on {place} only. You can sign in later."
        )),
    }
}

/// The first run asks for Screen Recording and Accessibility: on a Mac,
/// signed in (so it is added to the account and shared), when it shares
/// its screen. A Mac kept for Spaces only, or one not signed in, never
/// sees the screen.
pub fn asks_permissions(input: &FirstRunInput) -> bool {
    input.host_os == "macos"
        && input.signed_in
        && input
            .host
            .as_ref()
            .is_none_or(|h| h.share_desktop != Some(false))
}

/// The host's permission id for macOS Local Network access.
pub const LOCAL_NETWORK: &str = "local-network";

/// The permissions a shared screen needs (two, and Local Network where macOS
/// Spaces run), as rows: also what Settings
/// shows later (the same words and buttons).
pub fn permissions_view(input: &FirstRunInput) -> PermissionsView {
    let place = this_computer(&input.host_os);
    let app = input
        .host
        .as_ref()
        .and_then(|h| h.app_name.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Cua Spaces".into());
    // The host lists only what is missing, and only once it is set up.
    let known = input
        .host
        .as_ref()
        .filter(|h| h.configured && !input.host_setting_up);
    // The host lists a "local-network" entry (granted or not) when macOS Spaces
    // can run on this Mac, and the page then asks for it with the other two.
    let local_network = input
        .host
        .as_ref()
        .is_some_and(|h| h.permissions.iter().any(|p| p.id == LOCAL_NETWORK));
    let mut wanted = vec![
        // Plain words, not the panes' names: macOS renames them (Accessibility
        // is "Device Control and Data Access" from macOS 27), and Allow opens
        // the right one anyway.
        (
            "screen-recording",
            format!("See {place}'s screen"),
            "So your team and agents can see what's on it.".to_string(),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        ),
        (
            "accessibility",
            "Use the mouse and keyboard".to_string(),
            format!("So they can click and type on {place}."),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        ),
    ];
    if local_network {
        wanted.push((
            LOCAL_NETWORK,
            "Run Spaces on this Mac".to_string(),
            "So the macOS Spaces you create here can start and connect.".to_string(),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork",
        ));
    }
    let count = wanted.len();
    let rows: Vec<CheckRow> = wanted
        .into_iter()
        .map(|(id, title, note, pane)| {
            let (state, note, action, settings_url) = match known {
                None => (
                    RowState::Checking,
                    format!("Adding {place} to your account first…"),
                    None,
                    None,
                ),
                Some(host) => match missing_permission(host, id) {
                    Some(p) => (
                        RowState::Fix,
                        note,
                        Some(button("allow", "Allow")),
                        Some(p.settings_url.unwrap_or_else(|| pane.into())),
                    ),
                    None => (RowState::Ok, "Allowed".into(), None, None),
                },
            };
            CheckRow {
                id: id.into(),
                state,
                title,
                note,
                action,
                settings_url,
            }
        })
        .collect();
    let all = rows.iter().all(|r| r.state == RowState::Ok);
    PermissionsView {
        title: format!("Share {place}'s screen."),
        lede: if count == 2 {
            format!("Allow two permissions so your team and agents can see and use {place}.")
        } else {
            format!(
                "Allow three permissions so your team and agents can see and use {place}, and \
                 so macOS Spaces can run on it."
            )
        },
        rows,
        note: format!("Allow opens System Settings, where you turn on {app}."),
        primary: all.then(|| button("continue", "Continue")),
        secondary: (!all).then(|| button("not-now", "Not now")),
    }
}

fn missing_permission(host: &FirstRunHost, id: &str) -> Option<FirstRunPermission> {
    host.permissions
        .iter()
        .find(|p| p.id == id && !p.granted)
        .cloned()
}

/// This computer for Home's to-do list: signed in, set up, running, the
/// permissions an agent needs to use it, online.
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
            &format!("So your other computers can reach {place}."),
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
                |e| format!("Couldn't set it up: {e}"),
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
            "Cua isn't running in the background.",
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

fn offer_id(o: &FirstRunOffer) -> &str {
    if o.id.is_empty() { &o.os } else { &o.id }
}

fn offer_detail(o: &FirstRunOffer) -> String {
    let time = o.time.as_deref().map(|t| format!("Ready in {t}"));
    match offer_id(o) {
        // A download worth saying: its size, then the time.
        "macos" | "omarchy" => [o.size.clone(), o.time.clone()]
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
            footnote: Some("You can press Done. We'll tell you when it's ready.".into()),
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
                "A separate {} computer on {place}.",
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
                "No Space for now".into(),
                "Add one from Home whenever you like.".into(),
            )
        },
        _ => {
            // Linux first, then Omarchy and macOS where they run here;
            // nothing else. Only Linux shows when it can't be made (with
            // why): Omarchy or macOS this computer can't run, or has no
            // room for, isn't a choice at all.
            let choices: Vec<ExtraChoice> = ["linux", "omarchy", "macos"]
                .iter()
                .filter_map(|id| input.offers.iter().find(|o| offer_id(o) == *id))
                .filter(|o| offer_id(o) == "linux" || o.blocked.is_none())
                .map(|o| ExtraChoice {
                    id: offer_id(o).to_string(),
                    os: o.os.clone(),
                    label: os_label(offer_id(o)),
                    detail: offer_detail(o),
                    blocked: o.blocked.clone(),
                })
                .collect();
            ExtraView {
                tag: None,
                choices,
                action: Some(button("not-now", "Not now")),
                ..base(
                    "choose",
                    "Make a first Space".into(),
                    format!("A separate computer on {place}. Pick a system to start it."),
                )
            }
        }
    }
}

fn setup_view(input: &FirstRunInput) -> SetupView {
    let place = this_computer(&input.host_os);
    let all = input.agents.as_deref().unwrap_or(&[]);
    let done = PrimaryView {
        id: "done".into(),
        label: "Done".into(),
        enabled: true,
        note: None,
    };
    let Some(lead) = lead_of(input) else {
        return SetupView {
            title: "Try a first Space.".into(),
            lede: format!("Spaces run on {place}, away from your files and your mouse."),
            prompt: None,
            status: None,
            extra: Some(extra_view(input)),
            primary: done,
        };
    };
    let name = name_of(all, &lead);
    let status = match &input.agent_used_at {
        Some(at) => CheckRow {
            id: "agent".into(),
            state: RowState::Ok,
            title: format!("{name} used Cua at {at}."),
            note: "Ask it anything else.".into(),
            action: None,
            settings_url: None,
        },
        None => CheckRow {
            id: "agent".into(),
            state: RowState::Checking,
            title: format!("Waiting for {name}…"),
            note: format!("If {name} was already open, restart it so it can find Cua."),
            action: None,
            settings_url: None,
        },
    };
    SetupView {
        title: format!("Try it in {name}."),
        lede: format!(
            "Paste this request into {name}. It makes a Space and opens its desktop in a window."
        ),
        prompt: Some(PromptView {
            heading: format!("Paste this in {name}"),
            text: try_prompt(&input.host_os).into(),
            note: format!("The Space runs on {place}."),
            copy: Some("Copy".into()),
            tabs: try_prompt_tabs(&input.host_os),
            selected: Some("linux".into()),
        }),
        status: Some(status),
        extra: None,
        primary: done,
    }
}

/// The plan card for a moment after the first success: Teams for someone
/// who entered teammates' emails and isn't on Teams, else Pro for Free.
/// Never after "Not now", never before the agent used Cua, never on an
/// unmetered account. The first run itself shows no plan.
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

/// The keys beside the page: the lead agent's key (the most preferred
/// connected one, else found one, else `other`), this computer's key, and
/// on the try screen the Space being made.
fn art(input: &FirstRunInput) -> ArtView {
    let agents = input.agents.as_deref().unwrap_or(&[]);
    ArtView {
        agent: lead_of(input).unwrap_or_else(|| lead_agent(agents)),
        computer: "on".into(),
        extra: match (input.step, input.extra.phase.as_str()) {
            (FirstRunStep::Setup, "creating") => "filling",
            (FirstRunStep::Setup, "ready") => "on",
            (FirstRunStep::Setup, "not-now") => "none",
            (FirstRunStep::Setup, _) if input.agent_used_at.is_some() => "on",
            (FirstRunStep::Welcome, _) => "none",
            _ => "outline",
        }
        .into(),
    }
}

/// The first run as drawn.
pub fn view(input: &FirstRunInput) -> FirstRunView {
    let all = steps(input);
    // The permissions screen of a Mac that no longer asks (signed out, or
    // its screen is not shared) shows the try screen instead.
    let step = if all.contains(&input.step) {
        input.step
    } else {
        FirstRunStep::Setup
    };
    let steps = all;
    let current = steps.iter().position(|s| *s == step).unwrap_or(0);
    FirstRunView {
        step,
        later: "Skip for now".into(),
        welcome: (step == FirstRunStep::Welcome).then(|| welcome_view(input)),
        connect: (step == FirstRunStep::Connect).then(|| connect_view(input)),
        devices: (step == FirstRunStep::Devices).then(|| devices_view(input)),
        permissions: (step == FirstRunStep::Permissions).then(|| permissions_view(input)),
        setup: (step == FirstRunStep::Setup).then(|| setup_view(input)),
        cli: (step == FirstRunStep::Cli).then(|| cli_view(input)),
        art: art(input),
        dots: steps.len() as u8,
        current: current as u8,
        steps,
    }
}

/// Install the Cua CLI, as the first run and Settings show it.
pub fn cli_view(input: &FirstRunInput) -> CliView {
    let name = cli_name(input);
    let cli = &input.cli;
    let installed = cli.present();
    let mut facts = Vec::new();
    if let Some(at) = cli.location().filter(|_| installed) {
        facts.push(CliFact {
            label: "Installed at".into(),
            value: tilde(&at),
        });
    } else if let Some(plan) = &cli.plan {
        facts.push(CliFact {
            label: "Installs".into(),
            value: tilde(&plan.target),
        });
        if !plan.on_path
            && let Some(profile) = &plan.path_profile
        {
            facts.push(CliFact {
                label: "Adds to PATH".into(),
                value: format!("{}, in {}", tilde(&plan.bin_dir), tilde(profile)),
            });
        }
    }
    let status = if cli.installing {
        "Installing…".to_string()
    } else if let Some(at) = cli.location().filter(|_| installed) {
        format!("Installed at {}.", tilde(&at))
    } else if let Some(plan) = cli.plan.as_ref().filter(|p| p.source.is_some()) {
        format!("Not installed. Install puts it at {}.", tilde(&plan.target))
    } else {
        "This app can't install the command here.".to_string()
    };
    let can_install = !installed && cli.plan.as_ref().is_some_and(|p| p.source.is_some());
    CliView {
        title: if installed {
            "Cua is ready in Terminal.".into()
        } else {
            "Use Cua from the Terminal.".into()
        },
        lede: format!(
            "Type {name} in Terminal to make and use Spaces. Your agents can use it too."
        ),
        facts,
        status,
        installed,
        busy: cli.installing,
        try_command: installed.then(|| format!("{name} spaces ls")),
        error: cli
            .error
            .as_ref()
            .map(|e| format!("Couldn't install it: {e}")),
        primary: if installed {
            Some(button("continue", "Continue"))
        } else if can_install {
            Some(button(
                "install",
                if cli.installing {
                    "Installing…"
                } else if cli.error.is_some() {
                    "Try again"
                } else {
                    "Install"
                },
            ))
        } else {
            Some(button("continue", "Continue"))
        },
        secondary: (!installed && can_install).then(|| button("not-now", "Not now")),
        name,
    }
}

/// Home's "Finish setting up" list after Skip for now: the connected
/// agents, this computer, the first request, a first Space and the
/// sign-in. Signing in is optional, so is everything that needs it.
pub fn todos(input: &FirstRunInput) -> FirstRunTodos {
    let place = this_computer(&input.host_os);
    let ids = picked(input);
    let all = input.agents.as_deref().unwrap_or(&[]);
    let name = names(all, &ids, "and");
    let computer = computer_row(input);
    let used = input.agent_used_at.is_some();
    let agent_ok = !ids.is_empty()
        && ids
            .iter()
            .all(|id| all.iter().any(|a| &a.agent == id && a.configured));
    let mut items: Vec<FirstRunTodo> = Vec::new();
    if !ids.is_empty() {
        items.push(FirstRunTodo {
            id: "agent".into(),
            title: format!("Connect {name}"),
            note: if agent_ok {
                format!("{name} can use Cua.")
            } else {
                format!("Cua isn't in {name}'s settings yet.")
            },
            done: agent_ok,
            optional: false,
            action: (!agent_ok).then(|| button("connect", "Connect")),
            settings_url: None,
        });
    }
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
        optional: !input.signed_in,
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
                try_prompt(&input.host_os)
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
        title: "Make a first Space (optional)".into(),
        note: format!("A separate computer on {place}, away from your files and your mouse."),
        done: extra_done,
        optional: true,
        action: (!extra_done).then(|| button("add", "Add a Space")),
        settings_url: None,
    });
    items.push(FirstRunTodo {
        id: "sign-in".into(),
        title: "Add your other computers (optional)".into(),
        note: if input.signed_in {
            "Signed in to Cua.".into()
        } else {
            format!("Sign in to use {place}'s Spaces from your other computers.")
        },
        done: input.signed_in,
        optional: true,
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
    let lede = if agent_ok {
        format!("{name} is connected. {rest}")
    } else {
        rest.into()
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

    fn agent(id: &str, name: &str, installed: bool) -> FirstRunAgent {
        FirstRunAgent {
            agent: id.into(),
            name: name.into(),
            installed,
            ..Default::default()
        }
    }

    fn claude(configured: bool) -> FirstRunAgent {
        FirstRunAgent {
            configured,
            mcp_config: Some("/Users/ada/.claude.json".into()),
            ..agent("claude-code", "Claude Code", true)
        }
    }

    fn codex(installed: bool, signed_in: bool) -> FirstRunAgent {
        FirstRunAgent {
            sign_in: Some("ChatGPT".into()),
            signed_in,
            mcp_config: Some("/Users/ada/.codex/config.toml".into()),
            ..agent("codex", "OpenAI Codex", installed)
        }
    }

    fn hermes() -> FirstRunAgent {
        FirstRunAgent {
            mcp_config: Some("/Users/ada/.hermes/config.yaml".into()),
            ..agent("hermes", "Hermes", true)
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

    fn input(step: FirstRunStep, agents: Vec<FirstRunAgent>) -> FirstRunInput {
        FirstRunInput {
            step,
            host_os: "macos".into(),
            agents: Some(agents),
            keys: Some(keys(&[])),
            ..Default::default()
        }
    }

    fn connect(i: &FirstRunInput) -> ConnectView {
        view(i).connect.unwrap()
    }

    fn agents_of(c: &ConnectView) -> Vec<String> {
        c.agents.iter().map(|a| a.agent.clone()).collect()
    }

    #[test]
    fn four_screens_in_order_and_a_fresh_run_starts_on_welcome() {
        assert_eq!(FirstRunInput::default().step, FirstRunStep::Welcome);
        let mut i = input(FirstRunStep::Welcome, vec![claude(false)]);
        for (step, n) in [
            (FirstRunStep::Welcome, 0),
            (FirstRunStep::Connect, 1),
            (FirstRunStep::Devices, 2),
            (FirstRunStep::Setup, 3),
        ] {
            i.step = step;
            let v = view(&i);
            assert_eq!((v.dots, v.current), (4, n));
            let drawn = [
                v.welcome.is_some(),
                v.connect.is_some(),
                v.devices.is_some(),
                v.setup.is_some(),
            ];
            assert_eq!(drawn.iter().filter(|d| **d).count(), 1, "{step:?}");
            assert!(drawn[n as usize]);
        }
        let w = view(&input(FirstRunStep::Welcome, vec![])).welcome.unwrap();
        assert_eq!(
            w.title,
            "Your computers, shared by your team and its agents."
        );
        assert_eq!(w.primary.id, "start");
    }

    #[test]
    fn a_run_saved_by_an_older_app_resumes() {
        for (word, step) in [
            ("connect", FirstRunStep::Connect),
            ("setup", FirstRunStep::Setup),
        ] {
            let i: FirstRunInput =
                serde_json::from_value(serde_json::json!({ "step": word })).unwrap();
            assert_eq!(i.step, step);
        }
    }

    #[test]
    fn agents_are_ordered_by_preference_and_nothing_is_connected_by_itself() {
        let i = input(
            FirstRunStep::Connect,
            vec![
                agent("openclaw", "OpenClaw", true),
                codex(true, false),
                claude(false),
                agent("goose", "Goose", true),
                agent("cursor", "Cursor", false),
                hermes(),
            ],
        );
        let c = connect(&i);
        // Only what's on this computer, most preferred first, the rest after.
        assert_eq!(
            agents_of(&c),
            ["hermes", "claude-code", "codex", "openclaw", "goose"]
        );
        for a in &c.agents {
            assert!(!a.connected, "{}", a.agent);
            assert_eq!(a.action.as_ref().unwrap().id, "connect");
        }
        let files: Vec<Option<String>> = c.agents.iter().map(|a| a.file.clone()).collect();
        assert_eq!(files[0].as_deref(), Some("~/.hermes/config.yaml"));
        assert_eq!(files[1].as_deref(), Some("~/.claude.json"));
        assert_eq!(files[2].as_deref(), Some("~/.codex/config.toml"));
        assert_eq!(c.secondary.as_ref().unwrap().id, "skip");
        assert_eq!(lead_agent(i.agents.as_deref().unwrap()), "hermes");
        assert_eq!(view(&i).art.agent, "hermes");
    }

    #[test]
    fn a_codex_user_never_sees_claude_first() {
        let i = FirstRunInput {
            picked: Some(vec!["codex".into()]),
            keys: Some(keys(&[])),
            ..input(
                FirstRunStep::Connect,
                vec![
                    codex(true, false),
                    agent("claude-code", "Claude Code", false),
                ],
            )
        };
        let c = connect(&i);
        assert_eq!(agents_of(&c), ["codex"]);
        let creds: Vec<&str> = c.credentials.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(creds, ["sign-in:codex", "key:openai"]);
        let mut s = i.clone();
        s.step = FirstRunStep::Setup;
        let v = view(&s).setup.unwrap();
        assert_eq!(v.title, "Try it in OpenAI Codex.");
        let all = serde_json::to_string(&view(&s)).unwrap();
        assert!(!all.contains("Claude"), "{all}");
        assert!(!all.contains("Anthropic"), "{all}");
        assert_eq!(view(&s).art.agent, "codex");
    }

    #[test]
    fn connected_agents_lead_in_preference_order_with_their_sign_ins_and_keys() {
        let i = FirstRunInput {
            picked: Some(vec!["codex".into(), "claude-code".into()]),
            keys: Some(keys(&["ANTHROPIC_API_KEY"])),
            ..input(
                FirstRunStep::Connect,
                vec![codex(true, true), claude(true), hermes()],
            )
        };
        let c = connect(&i);
        let connected: Vec<&str> = c
            .agents
            .iter()
            .filter(|a| a.connected)
            .map(|a| a.agent.as_str())
            .collect();
        assert_eq!(connected, ["claude-code", "codex"]);
        assert!(c.secondary.is_none());
        let creds: Vec<(&str, Option<&str>)> = c
            .credentials
            .iter()
            .map(|r| (r.id.as_str(), r.status.as_deref()))
            .collect();
        assert_eq!(
            creds,
            [
                ("sign-in:codex", Some("Signed in")),
                ("key:anthropic", Some("Saved")),
                ("key:openai", None),
            ]
        );
        assert_eq!(c.credentials[2].action.as_ref().unwrap().id, "add-key");
        let hermes = c.agents.iter().find(|a| a.agent == "hermes").unwrap();
        assert_eq!(hermes.action.as_ref().unwrap().label, "Connect");
        assert_eq!(picked(&i), ["claude-code", "codex"]);
    }

    #[test]
    fn connecting_one_agent_shows_on_its_row_only() {
        let i = FirstRunInput {
            connecting: true,
            connecting_agent: Some("codex".into()),
            ..input(FirstRunStep::Connect, vec![codex(true, false), hermes()])
        };
        let c = connect(&i);
        let codex = c.agents.iter().find(|a| a.agent == "codex").unwrap();
        assert_eq!(codex.note.as_deref(), Some("Connecting…"));
        assert!(codex.action.is_none());
        let hermes = c.agents.iter().find(|a| a.agent == "hermes").unwrap();
        assert!(hermes.action.is_some());
        let failed = FirstRunInput {
            connect_error: Some("no write access".into()),
            ..input(FirstRunStep::Connect, vec![hermes_row()])
        };
        assert_eq!(
            connect(&failed).error.as_deref(),
            Some("Couldn't connect Cua: no write access")
        );
    }

    fn hermes_row() -> FirstRunAgent {
        hermes()
    }

    #[test]
    fn no_agent_found_offers_an_install_or_the_cli() {
        let c = connect(&input(
            FirstRunStep::Connect,
            vec![agent("claude-code", "Claude Code", false)],
        ));
        assert!(c.agents.is_empty());
        assert_eq!(c.title, "Connect an agent.");
        let alts: Vec<&str> = c.alternatives.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(alts, ["install", "cli"]);
        assert_eq!(
            c.alternatives[0].note,
            "Hermes, Claude Code, Codex or Cursor, then come back here."
        );
        assert_eq!(
            c.alternatives[1].command.as_deref(),
            Some("cua spaces create linux --on local")
        );
        // Still looking: no alternatives yet.
        let looking = connect(&FirstRunInput {
            agents: None,
            ..input(FirstRunStep::Connect, vec![])
        });
        assert!(looking.alternatives.is_empty());
        assert_eq!(looking.lede, "Looking for agents on this Mac…");
        assert_eq!(lead_agent(&[]), "other");
    }

    #[test]
    fn configured_claude_is_ready_without_binary_evidence_but_folders_are_not() {
        use crate::agents::{AgentSetupStatus, agent_settings_row};
        for configured in [true, false] {
            let row = agent_settings_row(
                &AgentSetupStatus {
                    id: "claude-code".into(),
                    name: "Claude Code".into(),
                    cua_configured: configured,
                    mcp_config: Some(".claude.json".into()),
                    skills_dir: Some(".claude/skills".into()),
                    skills_installed: vec!["cua".into()],
                    ..Default::default()
                },
                1,
            );
            let a = FirstRunAgent {
                agent: row.agent,
                name: row.name,
                installed: row.installed,
                configured: row.configured,
                ..Default::default()
            };
            let c = connect(&input(FirstRunStep::Connect, vec![a]));
            let found = c.agents.iter().find(|a| a.agent == "claude-code");
            assert_eq!(found.map(|a| a.connected), configured.then_some(true));
        }
    }

    #[test]
    fn signing_in_is_optional_and_says_what_it_connects() {
        let mut i = input(FirstRunStep::Devices, vec![]);
        let d = view(&i).devices.unwrap();
        assert_eq!(d.title, "Add your other computers.");
        assert_eq!(
            d.lede,
            "Sign in to Cua here and on your other computers. Then they can all use this Mac's Spaces."
        );
        let rows: Vec<&str> = d.rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(rows, ["Your other computers", "Your cloud"]);
        assert_eq!(
            (d.primary.id.as_str(), d.primary.label.as_str()),
            ("sign-in", "Sign in")
        );
        assert_eq!(d.secondary.as_ref().unwrap().id, "not-now");
        assert_eq!(
            d.note.as_deref(),
            Some("Not now keeps Cua on this Mac only. You can sign in later.")
        );
        assert!(d.status.is_none());

        i.signing_in = true;
        assert_eq!(
            view(&i).devices.unwrap().status.unwrap().state,
            RowState::Checking
        );

        i.signing_in = false;
        i.signed_in = true;
        i.host_setting_up = true;
        i.host = Some(FirstRunHost::default());
        let d = view(&i).devices.unwrap();
        assert_eq!(d.title, "Adding this Mac to your account.");
        assert_eq!(d.primary.id, "continue");
        assert!(d.secondary.is_none());
        assert_eq!(d.status.as_ref().unwrap().state, RowState::Checking);

        i.host_setting_up = false;
        i.host = Some(FirstRunHost {
            configured: true,
            running: true,
            ..Default::default()
        });
        let d = view(&i).devices.unwrap();
        assert_eq!(d.title, "This Mac is in your account.");
        let ok = d.status.unwrap();
        assert_eq!(
            (ok.state, ok.title.as_str()),
            (RowState::Ok, "Ready to share")
        );

        // No permission is asked here, even when one is missing.
        i.host.as_mut().unwrap().permissions = vec![FirstRunPermission {
            id: "accessibility".into(),
            granted: false,
            settings_url: None,
        }];
        assert_eq!(
            view(&i).devices.unwrap().status.unwrap().state,
            RowState::Ok
        );
    }

    /// Where Back and Continue go from `v`.
    fn nav(v: &FirstRunView) -> (FirstRunStep, FirstRunStep) {
        let at = v.current as usize;
        (
            v.steps[at.saturating_sub(1)],
            v.steps[(at + 1).min(v.steps.len() - 1)],
        )
    }

    fn missing(id: &str) -> FirstRunPermission {
        FirstRunPermission {
            id: id.into(),
            granted: false,
            settings_url: None,
        }
    }

    #[test]
    fn a_shared_mac_asks_for_two_permissions_after_the_account_and_never_before() {
        let mut i = FirstRunInput {
            step: FirstRunStep::Devices,
            host_os: "macos".into(),
            ..Default::default()
        };
        // Not signed in: Not now goes straight to the try screen.
        let v = view(&i);
        assert_eq!((nav(&v).1, v.dots), (FirstRunStep::Setup, 4));

        i.signed_in = true;
        i.host = Some(FirstRunHost {
            configured: true,
            running: true,
            permissions: vec![missing("screen-recording"), missing("accessibility")],
            ..Default::default()
        });
        let v = view(&i);
        assert_eq!((nav(&v).1, v.dots), (FirstRunStep::Permissions, 5));

        i.step = FirstRunStep::Permissions;
        let v = view(&i);
        assert_eq!(
            (nav(&v).0, nav(&v).1, v.current),
            (FirstRunStep::Devices, FirstRunStep::Setup, 3)
        );
        let p = v.permissions.unwrap();
        assert_eq!(p.title, "Share this Mac's screen.");
        assert_eq!(
            p.lede,
            "Allow two permissions so your team and agents can see and use this Mac."
        );
        assert_eq!(
            p.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["screen-recording", "accessibility"]
        );
        assert!(p.rows.iter().all(|r| r.state == RowState::Fix
            && r.action.as_ref().is_some_and(|a| a.id == "allow")
            && r.settings_url.is_some()));
        assert!(p.primary.is_none());
        assert_eq!(p.secondary.unwrap().id, "not-now");
        assert!(p.note.contains("turn on Cua Spaces."));

        // Granted one by one: a check each, then Continue.
        let host = i.host.as_mut().unwrap();
        host.permissions = vec![missing("accessibility")];
        host.app_name = Some("Cua Spaces Local".into());
        let p = view(&i).permissions.unwrap();
        assert_eq!(p.rows[0].state, RowState::Ok);
        assert_eq!(p.rows[1].state, RowState::Fix);
        assert!(p.note.contains("turn on Cua Spaces Local."));
        i.host.as_mut().unwrap().permissions.clear();
        let p = view(&i).permissions.unwrap();
        assert!(
            p.rows
                .iter()
                .all(|r| r.state == RowState::Ok && r.action.is_none())
        );
        assert_eq!(p.primary.unwrap().id, "continue");
        assert!(p.secondary.is_none());

        // Still being added: nothing is known yet, nothing is granted.
        i.host_setting_up = true;
        let p = view(&i).permissions.unwrap();
        assert!(p.rows.iter().all(|r| r.state == RowState::Checking));
        assert!(p.primary.is_none());
    }

    #[test]
    fn a_mac_that_runs_macos_spaces_also_asks_for_local_network() {
        let mut i = FirstRunInput {
            step: FirstRunStep::Permissions,
            host_os: "macos".into(),
            signed_in: true,
            host: Some(FirstRunHost {
                configured: true,
                running: true,
                permissions: vec![FirstRunPermission {
                    id: LOCAL_NETWORK.into(),
                    granted: false,
                    settings_url: None,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let p = view(&i).permissions.unwrap();
        assert_eq!(
            p.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["screen-recording", "accessibility", "local-network"]
        );
        // The two the host does not list are granted; Local Network is not.
        assert_eq!(p.rows[0].state, RowState::Ok);
        assert_eq!(p.rows[2].state, RowState::Fix);
        assert!(
            p.rows[2]
                .settings_url
                .as_deref()
                .is_some_and(|u| u.contains("Privacy_LocalNetwork"))
        );
        assert!(p.lede.starts_with("Allow three permissions"));
        assert!(p.primary.is_none());
        // Granted: listed, and no longer in the way.
        i.host.as_mut().unwrap().permissions[0].granted = true;
        let p = view(&i).permissions.unwrap();
        assert_eq!(p.rows[2].state, RowState::Ok);
        assert_eq!(p.primary.unwrap().id, "continue");
    }

    #[test]
    fn no_permissions_screen_without_a_shared_mac_screen() {
        let mut i = FirstRunInput {
            step: FirstRunStep::Permissions,
            host_os: "macos".into(),
            signed_in: true,
            host: Some(FirstRunHost {
                configured: true,
                share_desktop: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        // Spaces only: the saved step falls through to the try screen.
        let v = view(&i);
        assert_eq!(v.step, FirstRunStep::Setup);
        assert!(v.permissions.is_none());
        assert_eq!((nav(&v).0, v.dots), (FirstRunStep::Devices, 4));
        // Not a Mac.
        i.host_os = "windows".into();
        i.host.as_mut().unwrap().share_desktop = None;
        assert!(!asks_permissions(&i));
        // Signed out.
        i.host_os = "macos".into();
        i.signed_in = false;
        assert!(!asks_permissions(&i));
    }

    #[test]
    fn the_try_screen_waits_for_the_lead_agent_and_done_is_never_blocked() {
        let mut i = FirstRunInput {
            picked: Some(vec!["codex".into(), "hermes".into()]),
            ..input(FirstRunStep::Setup, vec![codex(true, false), hermes()])
        };
        let s = view(&i).setup.unwrap();
        assert_eq!(s.title, "Try it in Hermes.");
        assert_eq!(
            s.lede,
            "Paste this request into Hermes. It makes a Space and opens its desktop in a window."
        );
        let p = s.prompt.unwrap();
        assert_eq!(p.text, TRY_PROMPT);
        assert_eq!(p.copy.as_deref(), Some("Copy"));
        assert_eq!(s.status.as_ref().unwrap().title, "Waiting for Hermes…");
        assert!(s.primary.enabled);
        i.agent_used_at = Some("18:12".into());
        let s = view(&i).setup.unwrap();
        let st = s.status.unwrap();
        assert_eq!(
            (st.state, st.title.as_str()),
            (RowState::Ok, "Hermes used Cua at 18:12.")
        );
        assert_eq!(view(&i).art.extra, "on");
        i.host_os = "windows".into();
        assert_eq!(view(&i).setup.unwrap().prompt.unwrap().text, TRY_PROMPT_PC);
    }

    #[test]
    fn the_try_request_has_a_tab_per_system_with_linux_selected() {
        let i = FirstRunInput {
            picked: Some(vec!["hermes".into()]),
            ..input(FirstRunStep::Setup, vec![hermes()])
        };
        let p = view(&i).setup.unwrap().prompt.unwrap();
        let tabs: Vec<(&str, &str)> = p
            .tabs
            .iter()
            .map(|t| (t.id.as_str(), t.text.as_str()))
            .collect();
        assert_eq!(
            tabs,
            [
                (
                    "macos",
                    "Make a macOS Space on this Mac and show its desktop in a new window."
                ),
                (
                    "linux",
                    "Make a Linux Space on this Mac and show its desktop in a new window."
                ),
                (
                    "omarchy",
                    "Make an Omarchy Space on this Mac and show its desktop in a new window."
                ),
            ]
        );
        assert_eq!(p.selected.as_deref(), Some("linux"));
        assert_eq!(p.text, p.tabs[1].text);
        // macOS Spaces need a Mac: no macOS tab elsewhere.
        let i = FirstRunInput {
            host_os: "windows".into(),
            ..i
        };
        let p = view(&i).setup.unwrap().prompt.unwrap();
        assert_eq!(
            p.tabs.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(),
            ["Linux", "Omarchy"]
        );
        assert_eq!(p.text, TRY_PROMPT_PC);
    }

    #[test]
    fn with_no_agent_the_try_screen_offers_a_first_space() {
        let mut i = FirstRunInput {
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
                    blocked: Some("Needs Apple silicon".into()),
                    ..Default::default()
                },
            ],
            ..input(FirstRunStep::Setup, vec![])
        };
        let s = view(&i).setup.unwrap();
        assert_eq!(s.title, "Try a first Space.");
        assert!(s.prompt.is_none());
        let x = s.extra.unwrap();
        let ids: Vec<&str> = x.choices.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["linux"]);
        assert_eq!(x.choices[0].detail, "Ready in about 2 min");
        i.extra.phase = "creating".into();
        let x = view(&i).setup.unwrap().extra.unwrap();
        assert_eq!(x.action.unwrap().id, "cancel");
        assert!(view(&i).setup.unwrap().primary.enabled);
        assert_eq!(view(&i).art.extra, "filling");
    }

    #[test]
    fn no_plan_in_the_first_run() {
        let mut i = input(FirstRunStep::Setup, vec![claude(true)]);
        i.agent_used_at = Some("18:12".into());
        i.plan = Some(FirstRunPlan {
            metered: true,
            plan: "free".into(),
            account_app: "https://app.cua.ai".into(),
            ..Default::default()
        });
        for step in [
            FirstRunStep::Welcome,
            FirstRunStep::Connect,
            FirstRunStep::Devices,
            FirstRunStep::Setup,
        ] {
            i.step = step;
            let all = serde_json::to_string(&view(&i)).unwrap();
            assert!(!all.contains("Pro"), "{step:?}: {all}");
            assert!(!all.contains("$"), "{step:?}: {all}");
        }
        // The card itself still answers for the moments after.
        assert_eq!(upsell(i.plan.as_ref().unwrap(), true).unwrap().plan, "pro");
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
    fn todos_treat_sign_in_as_optional() {
        let mut i = FirstRunInput {
            picked: Some(vec!["claude-code".into()]),
            ..input(FirstRunStep::Setup, vec![claude(true)])
        };
        let t = todos(&i);
        let sign_in = t.items.iter().find(|x| x.id == "sign-in").unwrap();
        assert!(sign_in.optional && !sign_in.done);
        let computer = t.items.iter().find(|x| x.id == "computer").unwrap();
        assert!(computer.optional);
        assert!(!t.complete, "the first request is still to do");
        i.agent_used_at = Some("18:12".into());
        assert!(todos(&i).complete, "complete without signing in");
        i.signed_in = true;
        i.host = Some(FirstRunHost {
            configured: true,
            running: true,
            permissions: vec![FirstRunPermission {
                id: "accessibility".into(),
                granted: false,
                settings_url: None,
            }],
            ..Default::default()
        });
        let t = todos(&i);
        assert!(!t.complete, "signed in, a permission is still missing");
        assert_eq!(
            t.items.iter().find(|x| x.id == "computer").unwrap().title,
            "Let Cua control this Mac"
        );
    }

    #[test]
    fn plain_words() {
        let mut i = FirstRunInput {
            picked: Some(vec!["codex".into()]),
            ..input(FirstRunStep::Welcome, vec![codex(true, false), hermes()])
        };
        for step in [
            FirstRunStep::Welcome,
            FirstRunStep::Connect,
            FirstRunStep::Devices,
            FirstRunStep::Setup,
        ] {
            i.step = step;
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
                assert!(!all.contains(&format!(" {word}")), "{step:?}: {word}");
            }
        }
    }

    fn plan(found: Option<&str>, on_path: bool, target: &str) -> crate::installer::CliInstallPlan {
        crate::installer::CliInstallPlan {
            source: Some("/Applications/Cua Spaces.app/Contents/MacOS/cua".into()),
            target: target.into(),
            bin_dir: "/Users/ada/.local/bin".into(),
            on_path,
            path_profile: (!on_path).then(|| "/Users/ada/.zshrc".into()),
            path_line: (!on_path).then(|| "export PATH=\"/Users/ada/.local/bin:$PATH\"".into()),
            found: found.map(Into::into),
            ..Default::default()
        }
    }

    const STABLE_TARGET: &str = "/Users/ada/.local/bin/cua";
    const LOCAL_TARGET: &str = "/Users/ada/.local/bin/cua-local";

    #[test]
    fn the_cli_screen_shows_only_while_the_bundled_cli_is_missing() {
        let base = input(FirstRunStep::Connect, vec![claude(true)]);
        // Not read yet, or no plan here: no screen.
        assert_eq!(steps(&base).len(), 4);
        // A disk-image install: the bundled CLI, nothing on PATH.
        let dmg = FirstRunInput {
            cli: FirstRunCli {
                plan: Some(plan(None, false, STABLE_TARGET)),
                ..Default::default()
            },
            ..base.clone()
        };
        assert_eq!(
            steps(&dmg),
            [
                FirstRunStep::Welcome,
                FirstRunStep::Connect,
                FirstRunStep::Cli,
                FirstRunStep::Devices,
                FirstRunStep::Setup
            ]
        );
        let v = view(&FirstRunInput {
            step: FirstRunStep::Cli,
            ..dmg.clone()
        });
        assert_eq!((v.dots, v.current), (5, 2));
        // Already installed (or a working one found): no screen.
        let found = FirstRunInput {
            cli: FirstRunCli {
                plan: Some(plan(Some(STABLE_TARGET), true, STABLE_TARGET)),
                ..Default::default()
            },
            ..base.clone()
        };
        assert_eq!(steps(&found).len(), 4);
        // A build without a bundled CLI can't install one: no screen.
        let unbundled = FirstRunInput {
            cli: FirstRunCli {
                plan: Some(crate::installer::CliInstallPlan {
                    source: None,
                    ..plan(None, true, STABLE_TARGET)
                }),
                ..Default::default()
            },
            ..base
        };
        assert_eq!(steps(&unbundled).len(), 4);
    }

    #[test]
    fn the_cli_screen_names_the_file_asks_first_and_shows_the_result() {
        let mut i = FirstRunInput {
            step: FirstRunStep::Cli,
            cli: FirstRunCli {
                plan: Some(plan(None, false, STABLE_TARGET)),
                ..Default::default()
            },
            ..input(FirstRunStep::Cli, vec![claude(true)])
        };
        let c = view(&i).cli.unwrap();
        assert_eq!(c.title, "Use Cua from the Terminal.");
        assert_eq!(c.name, "cua");
        assert_eq!(
            c.facts,
            [
                CliFact {
                    label: "Installs".into(),
                    value: "~/.local/bin/cua".into()
                },
                CliFact {
                    label: "Adds to PATH".into(),
                    value: "~/.local/bin, in ~/.zshrc".into()
                },
            ]
        );
        assert_eq!(c.primary, Some(button("install", "Install")));
        assert_eq!(c.secondary, Some(button("not-now", "Not now")));
        assert!(!c.installed && c.try_command.is_none());
        assert_eq!(
            c.status,
            "Not installed. Install puts it at ~/.local/bin/cua."
        );

        i.cli.installing = true;
        let c = cli_view(&i);
        assert!(c.busy);
        assert_eq!(c.status, "Installing…");

        i.cli.installing = false;
        i.cli.error = Some("permission denied".into());
        let c = cli_view(&i);
        assert_eq!(
            c.error.as_deref(),
            Some("Couldn't install it: permission denied")
        );
        assert_eq!(c.primary, Some(button("install", "Try again")));

        i.cli.error = None;
        i.cli.installed = Some(STABLE_TARGET.into());
        let v = view(&i);
        // The result stays on its screen until Continue.
        assert!(v.steps.contains(&FirstRunStep::Cli));
        let c = v.cli.unwrap();
        assert!(c.installed);
        assert_eq!(c.title, "Cua is ready in Terminal.");
        assert_eq!(c.status, "Installed at ~/.local/bin/cua.");
        assert_eq!(c.try_command.as_deref(), Some("cua spaces ls"));
        assert_eq!(c.primary, Some(button("continue", "Continue")));
        assert_eq!(c.secondary, None);
    }

    #[test]
    fn local_builds_name_their_own_cli() {
        // The host names it (`SettingsSnapshot.cli`)...
        let named = FirstRunInput {
            cli_name: Some("cua-local".into()),
            ..input(FirstRunStep::Connect, vec![])
        };
        let c = connect(&named);
        assert_eq!(
            c.alternatives[1].command.as_deref(),
            Some("cua-local spaces create linux --on local")
        );
        // ...or its install plan's file does.
        let planned = FirstRunInput {
            step: FirstRunStep::Cli,
            cli: FirstRunCli {
                plan: Some(plan(None, true, LOCAL_TARGET)),
                ..Default::default()
            },
            ..input(FirstRunStep::Cli, vec![])
        };
        assert_eq!(cli_name(&planned), "cua-local");
        let c = cli_view(&planned);
        assert_eq!(c.facts[0].value, "~/.local/bin/cua-local");
        assert_eq!(c.facts.len(), 1, "on PATH already: no PATH line");
        assert_eq!(
            cli_name(&FirstRunInput {
                cli: FirstRunCli {
                    plan: Some(plan(None, true, "C:\\Users\\ada\\bin\\cua-local.exe")),
                    ..Default::default()
                },
                ..Default::default()
            }),
            "cua-local"
        );
        assert_eq!(cli_name(&FirstRunInput::default()), "cua");
        assert_eq!(
            cli_first_space("cua-local"),
            "cua-local spaces create linux --on local"
        );
    }
}
