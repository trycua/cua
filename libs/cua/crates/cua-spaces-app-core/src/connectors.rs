// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Connectors: the coding apps Cua works with and the providers
//! they sign in to, with one Connect for each.
//!
//! - **Coding apps.** One row for every agent harness that can run in a Space
//!   ([`HARNESS_APPS`]: Claude Code, Codex, Hermes, OpenCode, Goose and the
//!   rest), installed on this machine or not, and one for every other app
//!   found here, with Connect (or Connected) and a chevron to the app's own
//!   page. Connect does everything once: Cua's skills and MCP server in the
//!   app where it is installed, then the app's model sign-in (a sign-in
//!   where one works, else a short provider choice and a key), so the
//!   matching provider is connected too ([`connect_plan`]). Connected means
//!   the app can read what is saved (`reads`), never more.
//! - **An app's page** ([`app_detail`]): the separate switches (skills, MCP,
//!   setting the app up again automatically), which provider it signs in
//!   with and how, Sign in and Sign out, and taking Cua out of the app.
//! - **Providers.** Anthropic, OpenAI and GitHub, each with Connect: the
//!   provider's own sign-in where one exists for this use, else a key
//!   ([`provider_sheet`]). Other keys follow by variable name.
//! - **The setup card** ([`setup_card`]): what a task that couldn't start
//!   because its app isn't connected says, with "Connect <app>".
//!
//! The sign-in paths offered: for Claude Code, the sign-in Claude Code
//! already has on this machine, imported once the person agrees
//! ([`import_sheet_on`]; `cua-agent-setup`'s `claude_login` reads it and keeps
//! the access token only), a token from `claude setup-token` the person
//! pastes, a key the person pastes, or a key already exported in their shell.
//! Codex signs in with its own device code, in a folder Cua owns; Cua never
//! reads Codex's own `auth.json` for it.
//!
//! Every credential lives where the agent keys do (`agent_keys`: the
//! daemon's Keychain-backed store); this module only decides the words.

use serde::{Deserialize, Serialize};

use crate::agent_keys::{AgentKeyInput, AgentKeyRow, AgentKeysInput};
use crate::model::SpaceOs;

/// The providers come from the registry (`cua_connections`: one file per
/// provider). This module decides the words and the states.
use cua_connections as reg;

/// How an app signs in on its own, when it can: a method of a provider file
/// with `app` set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// The sign-in the app already has on this machine, imported once the
    /// person agrees; saved under the method's env and only that app uses it.
    AppToken(&'static reg::Method),
    /// The app's own device-code sign-in, run on This Mac in a folder Cua
    /// owns (Codex: `codex login --device-auth`); the person enters a code on
    /// the provider's page. Only that app uses it.
    AppDevice(&'static reg::Method),
}

/// Which providers an app signs in to, and the way its Connect uses.
struct AppSignIn {
    /// Any one of these connected is enough: the providers that deliver a
    /// variable to the app, then those it can be pointed at by wire.
    providers: Vec<&'static reg::Provider>,
    /// The way Connect starts when none is connected (`None`: it asks with
    /// the provider's sheet).
    way: Option<Way>,
    /// The saved credentials the app can read, as the variables a run gets
    /// (the harness's `keys`; a drift test holds them equal). Empty: the app
    /// takes any credential of its providers.
    reads: &'static [&'static str],
}

/// Apps that are not agent harnesses but read a provider's variable.
fn is_provider_app(app: &str) -> bool {
    reg::PROVIDERS.iter().any(|p| p.apps.contains(&app))
}

fn app_way(app: &str) -> Option<Way> {
    reg::visible()
        .flat_map(|p| p.offered())
        .filter(|m| m.app == app)
        .find_map(|m| match (m.kind, m.flow) {
            (reg::MethodKind::ImportLogin, "") => Some(Way::AppToken(m)),
            (reg::MethodKind::SubscriptionLogin, "device_code") => Some(Way::AppDevice(m)),
            _ => None,
        })
}

fn app_sign_in(app: &str) -> Option<AppSignIn> {
    if let Some(h) = HARNESS_APPS.iter().find(|h| h.app == app) {
        return Some(AppSignIn {
            providers: reg::visible()
                .filter(|p| reg::works_with(p).contains(&h.harness))
                .collect(),
            way: app_way(app),
            reads: reg::reads(h.harness),
        });
    }
    if is_provider_app(app) {
        return Some(AppSignIn {
            providers: reg::visible().filter(|p| p.apps.contains(&app)).collect(),
            way: None,
            reads: &[],
        });
    }
    None
}

/// Every agent harness that can run in a Space (`cua_agents::harness`), with
/// the setup id of its Connectors row, in the order the page lists them. A
/// drift test fails when a harness is added without a row here and in
/// [`APPS`], so one can never be missing from Settings.
pub const HARNESS_APPS: &[HarnessApp] = &[
    HarnessApp {
        app: "claude-code",
        harness: "claude-code",
        name: "Claude Code",
    },
    HarnessApp {
        app: "codex",
        harness: "openai-codex",
        name: "OpenAI Codex",
    },
    HarnessApp {
        app: "gemini-cli",
        harness: "gemini-cli",
        name: "Gemini CLI",
    },
    HarnessApp {
        app: "antigravity",
        harness: "google-antigravity",
        name: "Google Antigravity",
    },
    HarnessApp {
        app: "opencode",
        harness: "opencode",
        name: "OpenCode",
    },
    HarnessApp {
        app: "goose",
        harness: "goose",
        name: "Goose",
    },
    HarnessApp {
        app: "pi",
        harness: "pi",
        name: "Pi",
    },
    HarnessApp {
        app: "hermes",
        harness: "hermes",
        name: "Hermes",
    },
    HarnessApp {
        app: "openclaw",
        harness: "openclaw",
        name: "OpenClaw",
    },
];

/// One agent harness and its Connectors row.
#[derive(Debug, Clone, Copy)]
pub struct HarnessApp {
    /// The setup id of the row (`codex`).
    pub app: &'static str,
    /// The harness id a run uses (`openai-codex`).
    pub harness: &'static str,
    /// The row's name when the host did not list the app.
    pub name: &'static str,
}

/// The Connectors row (setup id) of `harness` (`openai-codex` is `codex`).
pub fn app_for_harness(harness: &str) -> Option<&'static str> {
    HARNESS_APPS
        .iter()
        .find(|h| h.harness == harness)
        .map(|h| h.app)
}

/// The harness `app` runs as in a Space, when it is one.
pub fn harness_of_app(app: &str) -> Option<&'static str> {
    HARNESS_APPS
        .iter()
        .find(|h| h.app == app)
        .map(|h| h.harness)
}

/// `names` as "A", "A and B", "A, B and C".
pub fn and_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `names` as "A", "A or B", "A, B or C".
pub fn or_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

fn provider(id: &str) -> Option<&'static reg::Provider> {
    reg::provider(id).filter(|p| p.visible())
}

/// Every variable the providers own (they are not listed under Other keys).
fn provider_env(env: &str) -> bool {
    reg::provider_of_env(env).is_some()
}

/// A provider's saved names in the order a credential is preferred: what the
/// person pasted as a key, then a sign-in.
fn provider_envs(p: &reg::Provider) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = p
        .methods
        .iter()
        .filter(|m| !m.terms_review && matches!(m.kind, reg::MethodKind::ApiKey | reg::MethodKind::CustomEndpoint))
        .flat_map(|m| std::iter::once(m.env).chain(m.aliases.iter().copied()))
        .collect();
    for e in p.envs() {
        if !v.contains(&e) {
            v.push(e);
        }
    }
    v
}

/// One coding app on this computer (`agents.setup`, plus what the page knows).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectorAppInput {
    /// Setup id (`claude-code`, `codex`, ...).
    pub agent: String,
    pub name: String,
    pub installed: bool,
    /// Cua's skills and MCP server are both in place.
    pub configured: bool,
    /// Cua's skills are in the app.
    pub skills: bool,
    /// Cua's MCP server is in the app's settings (`None`: the app has no MCP settings Cua writes).
    pub mcp: Option<bool>,
    /// The app's settings file, for its page.
    pub mcp_config: Option<String>,
}

/// Something running for a row: `app:<id>` or `provider:<id>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectorBusy {
    pub id: String,
    /// `connecting` (tools), `signing-in` (reading the app's sign-in, or a device code), `signing-out`.
    pub step: String,
}

/// Why the last change to a row failed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectorFailure {
    pub id: String,
    pub line: String,
}

/// What the Connectors page reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectorsInput {
    pub apps: Vec<ConnectorAppInput>,
    /// The saved keys and sign-ins (`agentKeys.list`).
    pub keys: AgentKeysInput,
    /// Provider variables exported in the person's shell (names only).
    pub shell_keys: Vec<String>,
    /// "Set up new apps automatically".
    pub auto: bool,
    /// Apps the person turned off: never set up again on their own.
    pub off: Vec<String>,
    pub busy: Vec<ConnectorBusy>,
    pub failures: Vec<ConnectorFailure>,
    /// What is typed in the search of More providers.
    pub query: String,
    /// More providers is open (a search is open too).
    pub more_open: bool,
}

impl ConnectorsInput {
    fn key(&self, env: &str) -> Option<&AgentKeyInput> {
        self.keys.keys.iter().find(|k| k.env == env)
    }
    fn busy(&self, id: &str) -> Option<&str> {
        self.busy
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.step.as_str())
    }
    fn failure(&self, id: &str) -> Option<&str> {
        self.failures
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.line.as_str())
    }
    fn app(&self, id: &str) -> Option<&ConnectorAppInput> {
        self.apps.iter().find(|a| a.agent == id)
    }
    /// The app as a row reads it: what the host listed, or for an agent
    /// harness the host did not list (or that is not installed here), one
    /// that is not installed on this machine.
    fn row_app(&self, id: &str) -> Option<ConnectorAppInput> {
        self.app(id).cloned().or_else(|| {
            HARNESS_APPS
                .iter()
                .find(|h| h.app == id)
                .map(|h| ConnectorAppInput {
                    agent: h.app.into(),
                    name: h.name.into(),
                    ..Default::default()
                })
        })
    }
}

/// The saved credential that connects a provider, first match wins: the
/// pasted key, then a sign-in token.
fn provider_credential<'a>(input: &'a ConnectorsInput, p: &reg::Provider) -> Option<&'a AgentKeyInput> {
    provider_envs(p)
        .into_iter()
        .find_map(|e| input.key(e).filter(|k| !k.expired))
}

/// A sign-in saved for the provider has expired (and none that works is).
fn provider_expired(input: &ConnectorsInput, p: &reg::Provider) -> bool {
    provider_credential(input, p).is_none()
        && provider_envs(p)
            .into_iter()
            .any(|e| input.key(e).is_some_and(|k| k.expired))
}

fn provider_connected(input: &ConnectorsInput, id: &str) -> bool {
    provider(id).is_some_and(|p| provider_credential(input, p).is_some())
}

/// An app with no variable of its own for `p` is pointed at its endpoint, so a
/// connected `p` (with its URL and model, for a custom endpoint) is enough.
fn endpoint_connected(input: &ConnectorsInput, p: &reg::Provider, harness: &str) -> bool {
    if !p.delivery_for(harness).is_empty() {
        return false;
    }
    let Some(k) = provider_credential(input, p) else {
        return false;
    };
    p.kind != reg::Kind::Custom
        || (k.fields.get("base_url").is_some_and(|v| !v.is_empty())
            && k.fields.get("model").is_some_and(|v| !v.is_empty()))
}

/// The app's sign-in is in place: one of its providers is connected, or it
/// signs in on its own.
///
/// Only a credential the app can read counts: a provider being connected is
/// not enough when what is saved is a sign-in that app has no reader for (the
/// ChatGPT sign-in is for Codex; Hermes reads an OpenAI API key, not it).
fn app_signed_in(input: &ConnectorsInput, app: &str) -> bool {
    let Some(a) = app_sign_in(app) else {
        return true;
    };
    if a.reads.is_empty() {
        return a.providers.iter().any(|p| provider_connected(input, p.id));
    }
    let harness = harness_of_app(app).unwrap_or_default();
    a.reads
        .iter()
        .any(|k| input.key(k).is_some_and(|k| !k.expired))
        || a.providers.iter().any(|p| endpoint_connected(input, p, harness))
}

/// An app's sign-in is saved but expired.
///
/// Only a sign-in the app can read counts: Claude Code's sign-in running out
/// is Claude Code's to reconnect, not Hermes' (it reads an API key).
fn app_expired(input: &ConnectorsInput, app: &str) -> bool {
    app_sign_in(app).is_some_and(|a| {
        if !a.reads.is_empty() {
            return a
                .reads
                .iter()
                .any(|k| input.key(k).is_some_and(|k| k.expired));
        }
        a.providers.iter().any(|p| provider_expired(input, p))
    })
}

/// "Signed in as ada@example.com (Claude Max)": the name of the sign-in an app
/// reads, when it has one that works (a copied sign-in, never a typed key).
fn signed_in_as(input: &ConnectorsInput, app: &str) -> Option<String> {
    app_sign_in(app)?.reads.iter().find_map(|e| {
        input
            .key(e)
            .filter(|k| !k.expired)
            .and_then(|k| k.label.clone())
            .filter(|l| !l.is_empty())
    })
}

fn dots(k: &AgentKeyInput) -> String {
    if k.last4.is_empty() {
        "Saved".into()
    } else {
        format!("\u{2022}\u{2022}\u{2022}\u{2022} {}", k.last4)
    }
}

/// How a saved credential reads on its provider's row.
fn credential_text(p: &reg::Provider, k: &AgentKeyInput) -> String {
    let plan = k
        .region
        .as_deref()
        .and_then(|r| p.regions.iter().find(|x| x.id == r))
        .map(|r| format!(", {}", r.name))
        .unwrap_or_default();
    match k.env.as_str() {
        "CLAUDE_CODE_OAUTH_TOKEN" => k
            .label
            .clone()
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| "Signed in to Claude".into()),
        "CODEX_AUTH_JSON" => "Signed in with ChatGPT, for Codex".into(),
        _ if p.kind == reg::Kind::Custom => match k.fields.get("base_url") {
            Some(u) if !u.is_empty() => u.clone(),
            _ => "Saved".into(),
        },
        e if p.key_method().is_some_and(|m| m.env == e) => format!("API key {}{plan}", dots(k)),
        e => format!("{e} {}", dots(k)),
    }
}

/// A button on a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorButton {
    /// `connect`, `sign-out`, `try-again`.
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

fn button(id: &str, label: impl Into<String>, enabled: bool) -> ConnectorButton {
    ConnectorButton {
        id: id.into(),
        label: label.into(),
        enabled,
    }
}

/// One coding app's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAppRow {
    pub agent: String,
    pub name: String,
    /// `connected`, `not-connected`, `connecting`, `failed`.
    pub state: String,
    /// The row's second line: what is missing, what is happening, or why it failed.
    pub line: Option<String>,
    /// Connect, or Try again (absent when connected: the row says so instead).
    pub button: Option<ConnectorButton>,
    /// "Connected", beside the chevron once done.
    pub connected_label: Option<String>,
    /// The page the row opens ("Claude Code settings", for the chevron's label).
    pub open_label: String,
}

/// An app a connection works with (a chip on its row; it opens the app's page).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkChip {
    /// The app's setup id (`claude-code`, `codex`, `hermes`).
    pub agent: String,
    pub name: String,
}

/// A provider's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorProviderRow {
    pub id: String,
    pub label: String,
    /// The icon's key (the app draws it; a mark of the initial when it has none).
    pub icon: String,
    /// `top` (up front) or `more` (the searchable list).
    pub tier: String,
    /// `connected`, `not-connected`, `connecting`, `failed`.
    pub state: String,
    /// "Connected", "Needs reconnect", "Not connected", or what is happening
    /// ("Reading your Claude Code sign-in…") or why it failed.
    pub status: String,
    /// What is saved, when connected: "API key •••• 3f9a", "Signed in to Claude".
    pub credential: Option<String>,
    /// The apps it works with, computed from the provider file.
    pub works_with: Vec<WorkChip>,
    /// "Works with".
    pub works_with_label: String,
    /// Connect, Reconnect, or Sign out.
    pub button: ConnectorButton,
    /// When it was connected (the page writes "Added <date>").
    pub added_ms: Option<u64>,
    /// The saved variables Sign out removes.
    pub sign_out_envs: Vec<String>,
}

/// An app that is not on this machine and cannot run as an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoreApp {
    pub agent: String,
    pub name: String,
}

/// The page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorsView {
    pub apps_title: String,
    pub apps: Vec<ConnectorAppRow>,
    /// "We didn't find a coding app yet…", when there is none.
    pub apps_empty: Option<String>,
    /// Other apps Cua knows that are not on this machine and cannot run as agents
    /// (under Show all, each with where to get it).
    pub more: Vec<MoreApp>,
    pub auto_label: String,
    pub auto_help: String,
    pub auto: bool,
    /// "Connections".
    pub providers_title: String,
    /// Under Connections: where credentials stay.
    pub providers_footer: String,
    /// The providers up front, then any other that is connected or needs
    /// reconnecting (so a connected one never hides in the search).
    pub providers: Vec<ConnectorProviderRow>,
    /// "More providers".
    pub more_title: String,
    /// The search field's label.
    pub more_search_label: String,
    /// The searchable list: every provider not shown above, filtered by the
    /// query (empty while More providers is closed).
    pub more_providers: Vec<ConnectorProviderRow>,
    /// How many providers More providers holds.
    pub more_count: u32,
    /// "No provider matches that.", when a search finds none.
    pub more_empty: Option<String>,
    /// Keys under any other variable (`agent_keys`' rows).
    pub others: Vec<AgentKeyRow>,
    pub add_other_label: String,
    pub other_help: String,
    /// Why credentials can't be read or saved here.
    pub notice: Option<String>,
    pub can_edit: bool,
    pub added_label: String,
}

/// Where credentials stay, in the words of the system the app runs on.
fn vault(os: SpaceOs) -> Option<&'static str> {
    match os {
        SpaceOs::Macos => Some("the Keychain"),
        SpaceOs::Windows => Some("Windows Credential Manager"),
        SpaceOs::Linux | SpaceOs::Unknown => None,
    }
}

fn notice(input: &ConnectorsInput) -> Option<String> {
    input
        .keys
        .error
        .as_ref()
        .map(|e| format!("Couldn't read your sign-ins: {e}"))
        .or_else(|| {
            input
                .keys
                .unavailable
                .as_ref()
                .map(|why| format!("Sign-ins can't be saved here: {why}."))
        })
}

fn app_row(input: &ConnectorsInput, a: &ConnectorAppInput) -> ConnectorAppRow {
    let id = format!("app:{}", a.agent);
    let busy = input.busy(&id);
    let failure = input.failure(&id);
    let signed_in = app_signed_in(input, &a.agent);
    // An agent harness runs in Spaces, so it is connected once it can read a
    // sign-in, installed on this machine or not (installed, Cua's tools must
    // be in it too).
    let harness = harness_of_app(&a.agent).is_some();
    let done = if a.installed || a.configured {
        a.configured && signed_in
    } else {
        harness && signed_in
    };
    let needs = app_sign_in(&a.agent).filter(|_| !signed_in).map(|s| {
        // An app with a sign-in of its own (Claude Code, Codex) names its provider;
        // one that takes several says so, and names them when there are few.
        let names = if s.way.is_some() {
            s.providers.first().map(|p| p.name.to_string())
        } else if s.providers.len() <= 3 {
            Some(or_list(&s.providers.iter().map(|p| p.name).collect::<Vec<_>>()))
        } else {
            None
        };
        match names {
            Some(n) if s.way.is_none() && !s.reads.is_empty() => format!("Needs an API key for {n}."),
            Some(n) => format!("Needs a sign-in to {n}."),
            None => "Needs a connected provider.".to_string(),
        }
    });

    let (state, line, btn) = if let Some(step) = busy {
        let line = match step {
            "signing-in"
                if app_sign_in(&a.agent)
                    .is_some_and(|s| matches!(s.way, Some(Way::AppDevice(_)))) =>
            {
                "Enter the code in your browser…".to_string()
            }
            "signing-in" => "Reading your Claude Code sign-in…".to_string(),
            "signing-out" => "Signing out…".to_string(),
            _ => format!("Adding Cua to {}…", a.name),
        };
        (
            "connecting",
            Some(line),
            Some(button("connect", "Connect", false)),
        )
    } else if let Some(why) = failure {
        (
            "failed",
            Some(why.to_string()),
            Some(button("try-again", "Try again", true)),
        )
    } else if done {
        // Not on this machine: say where it runs, once.
        (
            "connected",
            signed_in_as(input, &a.agent).or_else(|| {
                (!a.installed && !a.configured).then(|| "Runs in your Spaces".to_string())
            }),
            None,
        )
    } else if a.configured || !a.installed {
        // Cua's tools are in it (or it is not on this machine): what it still needs.
        let reconnect = app_expired(input, &a.agent);
        (
            "not-connected",
            if reconnect {
                Some("Needs reconnect".to_string())
            } else {
                needs
            },
            Some(button(
                "connect",
                if reconnect { "Reconnect" } else { "Connect" },
                true,
            )),
        )
    } else {
        (
            "not-connected",
            None,
            Some(button("connect", "Connect", true)),
        )
    };
    ConnectorAppRow {
        agent: a.agent.clone(),
        name: a.name.clone(),
        state: state.into(),
        line,
        button: btn,
        connected_label: done.then(|| "Connected".into()).filter(|_| busy.is_none()),
        open_label: format!("{} settings", a.name),
    }
}

/// The chips of an app: the apps a provider works with, by their setup ids.
fn chips(p: &reg::Provider) -> Vec<WorkChip> {
    let mut v: Vec<WorkChip> = reg::works_with(p)
        .into_iter()
        .filter_map(|h| {
            HARNESS_APPS.iter().find(|x| x.harness == h).map(|x| WorkChip {
                agent: x.app.into(),
                name: x.name.into(),
            })
        })
        .collect();
    for a in p.apps {
        v.push(WorkChip {
            agent: (*a).into(),
            name: provider_app_name(a).into(),
        });
    }
    v
}

/// The name of an app that is not an agent harness but reads a provider's variable.
fn provider_app_name(app: &str) -> &'static str {
    match app {
        "copilot-cli" => "GitHub Copilot CLI",
        "t3code" => "T3 Code",
        _ => "",
    }
}

fn provider_row(input: &ConnectorsInput, p: &reg::Provider, can_edit: bool) -> ConnectorProviderRow {
    let id = format!("provider:{}", p.id);
    let busy = input.busy(&id);
    let cred = provider_credential(input, p);
    let (state, status, btn) = if let Some(step) = busy {
        let status = match step {
            "signing-out" => "Signing out…",
            "signing-in"
                if p.offered().any(|m| m.kind == reg::MethodKind::SubscriptionLogin) =>
            {
                "Enter the code in your browser…"
            }
            "signing-in" => "Reading your Claude Code sign-in…",
            _ => "Connecting…",
        };
        (
            "connecting",
            status.to_string(),
            button("connect", "Connect", false),
        )
    } else if let Some(why) = input.failure(&id) {
        (
            "failed",
            why.to_string(),
            button("connect", "Try again", can_edit),
        )
    } else if cred.is_some() {
        (
            "connected",
            "Connected".to_string(),
            button("sign-out", "Disconnect", can_edit),
        )
    } else if provider_expired(input, p) {
        (
            "not-connected",
            "Needs reconnect".into(),
            button("connect", "Reconnect", can_edit),
        )
    } else {
        (
            "not-connected",
            "Not connected".into(),
            button("connect", "Connect", can_edit),
        )
    };
    ConnectorProviderRow {
        id: p.id.into(),
        label: p.name.into(),
        icon: p.icon.into(),
        tier: if p.tier == reg::Tier::Top { "top" } else { "more" }.into(),
        state: state.into(),
        status,
        credential: cred.filter(|_| busy.is_none()).map(|k| credential_text(p, k)),
        works_with: chips(p),
        works_with_label: "Works with".into(),
        button: btn,
        added_ms: cred.filter(|_| busy.is_none()).map(|k| k.added_ms),
        sign_out_envs: provider_envs(p)
            .into_iter()
            .filter(|e| input.key(e).is_some())
            .map(Into::into)
            .collect(),
    }
}

/// Whether `p` matches what is typed in the search: every word of the query
/// is the start of a word of its name, id or an alias.
pub fn provider_matches(p: &reg::Provider, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let hay: Vec<String> = std::iter::once(p.name)
        .chain(std::iter::once(p.id))
        .chain(p.aliases.iter().copied())
        .flat_map(|t| {
            t.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    q.split_whitespace()
        .all(|w| hay.iter().any(|h| h.starts_with(w)) || p.name.to_lowercase().contains(w) && w.len() > 2)
}

/// The page on a Mac.
pub fn view(input: &ConnectorsInput) -> ConnectorsView {
    view_on(input, SpaceOs::Macos)
}

/// [`view`] in a shell on `os`.
pub fn view_on(input: &ConnectorsInput, os: SpaceOs) -> ConnectorsView {
    let notice = notice(input);
    let can_edit = notice.is_none();
    // Every agent harness (installed here or not), then the other apps found here.
    let apps: Vec<ConnectorAppRow> = HARNESS_APPS
        .iter()
        .filter_map(|h| input.row_app(h.app))
        .chain(
            input
                .apps
                .iter()
                .filter(|a| a.installed && harness_of_app(&a.agent).is_none())
                .cloned(),
        )
        .map(|a| app_row(input, &a))
        .collect();
    let mut others: Vec<&AgentKeyInput> = input
        .keys
        .keys
        .iter()
        .filter(|k| !provider_env(&k.env))
        .collect();
    others.sort_by(|a, b| a.env.cmp(&b.env));
    let this = os.this_machine_lower();
    let place = vault(os).map(|v| format!(" in {v}")).unwrap_or_default();
    // Up front: the first tier, plus any other provider that has something saved
    // or is mid-change; the rest wait in the searchable list.
    let shown = |p: &reg::Provider| {
        p.tier == reg::Tier::Top
            || provider_credential(input, p).is_some()
            || provider_expired(input, p)
            || input.busy(&format!("provider:{}", p.id)).is_some()
            || input.failure(&format!("provider:{}", p.id)).is_some()
    };
    // The first tier in its order, then the others that have something to show.
    let providers: Vec<ConnectorProviderRow> = reg::visible()
        .filter(|p| shown(p) && p.tier == reg::Tier::Top)
        .chain(reg::visible().filter(|p| shown(p) && p.tier != reg::Tier::Top))
        .map(|p| provider_row(input, p, can_edit))
        .collect();
    let rest: Vec<&reg::Provider> = reg::visible().filter(|p| !shown(p)).collect();
    let open = input.more_open || !input.query.trim().is_empty();
    let more_providers: Vec<ConnectorProviderRow> = if open {
        rest.iter()
            .filter(|p| provider_matches(p, &input.query))
            .map(|p| provider_row(input, p, can_edit))
            .collect()
    } else {
        vec![]
    };
    ConnectorsView {
        apps_title: "Coding apps".into(),
        apps_empty: apps
            .is_empty()
            .then(|| "We didn't find a coding app yet. Install one and Cua will notice.".into()),
        more: input
            .apps
            .iter()
            .filter(|a| !a.installed && !a.configured && harness_of_app(&a.agent).is_none())
            .map(|a| MoreApp {
                agent: a.agent.clone(),
                name: a.name.clone(),
            })
            .collect(),
        apps,
        auto_label: "Set up new apps automatically".into(),
        auto_help: "When you install another coding app, Cua adds itself and tells you.".into(),
        auto: input.auto,
        providers_title: "Connections".into(),
        providers_footer: format!(
            "Sign-ins and keys stay on {this}{place}. Each one is only given to the apps that use it."
        ),
        providers,
        more_title: "More providers".into(),
        more_search_label: "Search providers".into(),
        more_empty: (open && more_providers.is_empty())
            .then(|| "No provider matches that.".to_string()),
        more_count: rest.len() as u32,
        more_providers,
        others: others
            .into_iter()
            .map(|k| AgentKeyRow {
                env: k.env.clone(),
                provider: "other".into(),
                title: k.env.clone(),
                detail: format!("For apps that read {}.", k.env),
                set: true,
                status: dots(k),
                added_ms: Some(k.added_ms),
                action_label: "Replace".into(),
                remove_label: Some("Remove".into()),
            })
            .collect(),
        add_other_label: "Add a key by variable name".into(),
        other_help: "For a provider that isn't listed, save its key under the variable its app reads, like \
                     MISTRAL_API_KEY."
            .into(),
        can_edit,
        notice,
        added_label: "Added".into(),
    }
}

/// What one Connect on an app does, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectPlan {
    /// Add Cua's skills and MCP server (`agents.connect`).
    pub tools: bool,
    /// Import the sign-in the app already has on this machine
    /// (`agents.connect` with `signIn`), after the person agrees
    /// ([`import_sheet_on`]).
    pub app_token: bool,
    /// Run the app's own device-code sign-in (`agents.signInStart`, then
    /// `agents.signInWait`): a code to show, entered on the provider's page.
    pub device: bool,
    /// Then open this provider's Connect sheet (the app has no sign-in of
    /// its own): the first provider it can use, with "Other providers" in
    /// the sheet for the rest.
    pub provider: Option<String>,
    /// The provider the token command signs in to (its row shows the approval too).
    pub token_provider: Option<String>,
}

/// What Connect on `agent` does now.
pub fn connect_plan(input: &ConnectorsInput, agent: &str) -> ConnectPlan {
    let app = input.app(agent).filter(|a| a.installed);
    let configured = app.is_some_and(|a| a.configured);
    let signed_in = app_signed_in(input, agent);
    let s = app_sign_in(agent);
    // The app's own sign-in is made on This Mac: only where the app is.
    let app_token = !signed_in
        && app.is_some()
        && s.as_ref().is_some_and(|s| matches!(s.way, Some(Way::AppToken(_))));
    let device = !signed_in
        && app.is_some()
        && s.as_ref().is_some_and(|s| matches!(s.way, Some(Way::AppDevice(_))));
    let provider = if signed_in || app_token || device {
        None
    } else {
        s.as_ref()
            .and_then(|s| s.providers.first())
            .map(|p| p.id.to_string())
    };
    ConnectPlan {
        // Cua's tools go into the app where it is on this computer.
        tools: app.is_some() && !configured,
        app_token,
        device,
        provider,
        token_provider: if app_token || device {
            s.as_ref()
                .and_then(|s| s.providers.first())
                .map(|p| p.id.to_string())
        } else {
            None
        },
    }
}

/// The provider ids `agent` signs in to (any one is enough; empty: it signs in on its own).
///
/// An app with a sign-in of its own (Claude Code, Codex) names its provider
/// only; one that takes many lists them all (the callers say "a connected
/// provider" when there are more than three).
pub fn app_providers(agent: &str) -> Vec<String> {
    app_sign_in(agent)
        .map(|s| {
            let n = if s.way.is_some() { 1 } else { s.providers.len() };
            s.providers.iter().take(n).map(|p| p.id.to_string()).collect()
        })
        .unwrap_or_default()
}

/// Whether `agent` has a sign-in it can use in a Space now.
pub fn signed_in(input: &ConnectorsInput, agent: &str) -> bool {
    app_signed_in(input, agent)
}

/// A provider's name ("Anthropic").
pub fn provider_label(id: &str) -> Option<&'static str> {
    provider(id).map(|p| p.name)
}

/// One field a way asks for besides the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WayField {
    /// `region` (the plan or region), `model`, `base_url`, `wire`: what
    /// `agentKeys.set` takes as `region` and `fields`.
    pub id: String,
    pub label: String,
    /// `text`, `url` or `choice`.
    pub kind: String,
    pub placeholder: String,
    pub required: bool,
    /// For a choice: its options, the first one chosen to start.
    pub options: Vec<FieldOption>,
    /// What it starts with.
    pub value: String,
}

/// One option of a choice field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldOption {
    pub id: String,
    pub label: String,
}

/// One way to connect a provider, as its sheet shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderWay {
    /// `app-token`, `app-device`, `token`, `key`, `endpoint`, `shell`.
    pub id: String,
    /// The method kind from the provider file (`api_key`, `import_login`,
    /// `subscription_login`, `custom_endpoint`; `shell` reads a key from the shell).
    pub kind: String,
    /// The method's id in the provider file.
    pub method: String,
    pub label: String,
    pub help: String,
    pub button: String,
    /// For `app-token`: the app whose sign-in is imported (`agents.connect` with `signIn`).
    /// For `token`: the app the pasted token is for.
    pub app: Option<String>,
    /// For `shell`: the variable read from the shell.
    pub env: Option<String>,
    /// `required` (a key or token is pasted), `optional` (a key, if the server
    /// asks for one) or `none`.
    pub secret: String,
    /// What it asks for besides the key.
    pub fields: Vec<WayField>,
}

/// A provider's Connect sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSheetView {
    pub provider: String,
    pub icon: String,
    pub title: String,
    /// "Works with Claude Code, Hermes and OpenCode."
    pub lede: String,
    pub works_with: Vec<WorkChip>,
    pub ways: Vec<ProviderWay>,
    pub cancel_label: String,
    /// The key field, when the person picks Paste a key.
    pub key_label: String,
    pub key_placeholder: String,
    pub key_help: String,
    /// The field when the person picks Paste a Claude token.
    pub token_label: String,
    pub token_placeholder: String,
    pub save_label: String,
    /// A note to read before saving (a server on this machine), when there is one.
    pub note: Option<String>,
    /// "Other providers": opens the provider choice (for the app, when opened for one).
    pub switch_label: String,
}

/// One provider in a choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChoice {
    pub provider: String,
    pub label: String,
    pub icon: String,
    pub tier: String,
    /// `connected`, `not-connected`.
    pub state: String,
    pub status: String,
}

/// The provider choice: the first tier, then (once More providers is open or
/// something is typed) the rest, filtered by the search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPickerView {
    pub title: String,
    /// "Providers Hermes can use", when opened for an app.
    pub lede: String,
    pub search_label: String,
    pub items: Vec<ProviderChoice>,
    /// "More providers" and how many are hidden, while closed and nothing is typed.
    pub more_label: Option<String>,
    pub more_count: u32,
    pub empty: Option<String>,
    pub cancel_label: String,
}

/// The sheet for `provider` (None: an unknown provider).
pub fn provider_sheet(input: &ConnectorsInput, provider_id: &str) -> Option<ProviderSheetView> {
    provider_sheet_on(input, provider_id, SpaceOs::Macos, None)
}

fn wire_label(w: &str) -> &'static str {
    match w {
        "openai-chat" => "OpenAI chat",
        "openai-responses" => "OpenAI responses",
        "anthropic" => "Anthropic messages",
        "gemini" => "Gemini",
        _ => "Other",
    }
}

fn way_fields(p: &reg::Provider, m: &reg::Method) -> Vec<WayField> {
    let mut out = vec![];
    if !p.regions.is_empty() {
        out.push(WayField {
            id: "region".into(),
            label: if p.id == "zai" { "Plan".into() } else { "Region".into() },
            kind: "choice".into(),
            placeholder: String::new(),
            required: true,
            options: p
                .regions
                .iter()
                .map(|r| FieldOption {
                    id: r.id.into(),
                    label: r.name.into(),
                })
                .collect(),
            value: p.regions[0].id.into(),
        });
    }
    for f in m.fields {
        let options: Vec<FieldOption> = f
            .options
            .iter()
            .map(|o| FieldOption {
                id: (*o).into(),
                label: if f.id == "wire" { wire_label(o).into() } else { (*o).into() },
            })
            .collect();
        out.push(WayField {
            id: f.id.into(),
            label: f.label.into(),
            kind: f.kind.into(),
            placeholder: f.placeholder.into(),
            required: f.required,
            value: options.first().map(|o| o.id.clone()).unwrap_or_default(),
            options,
        });
    }
    // A provider with no default model asks for one when an app is told which to use.
    if p.default_model.is_empty() && p.kind != reg::Kind::Custom && !p.delivery_for("hermes").is_empty() {
        out.push(WayField {
            id: "model".into(),
            label: "Model".into(),
            kind: "text".into(),
            placeholder: "The model id to use".into(),
            required: true,
            options: vec![],
            value: String::new(),
        });
    }
    out
}

/// [`provider_sheet`] in a shell on `os`, opened for `for_app` (an app's own
/// sign-in is offered only in its sheet).
pub fn provider_sheet_on(
    input: &ConnectorsInput,
    provider_id: &str,
    os: SpaceOs,
    for_app: Option<&str>,
) -> Option<ProviderSheetView> {
    let p = provider(provider_id)?;
    let installed = |app: &str| input.app(app).is_some_and(|a| a.installed);
    let mut ways: Vec<ProviderWay> = vec![];
    for m in p.offered() {
        let way = |id: &str, button: &str, secret: &str, app: Option<&str>, env: Option<&str>, fields: Vec<WayField>| ProviderWay {
            id: id.into(),
            kind: m.kind.word().into(),
            method: m.id.into(),
            label: m.label.into(),
            help: m.help.into(),
            button: button.into(),
            app: app.map(Into::into),
            env: env.map(Into::into),
            secret: secret.into(),
            fields,
        };
        match (m.kind, m.flow) {
            // An app's own sign-in is that app's alone: another app's sheet offers a key.
            (reg::MethodKind::ImportLogin, "") => {
                if installed(m.app) && for_app.is_none_or(|a| a == m.app) {
                    ways.push(way("app-token", "Sign in", "none", Some(m.app), None, vec![]));
                }
            }
            (reg::MethodKind::ImportLogin, "paste") => {
                if for_app.is_none_or(|a| a == m.app) {
                    ways.push(way("token", "Paste token", "required", Some(m.app), None, vec![]));
                }
            }
            (reg::MethodKind::SubscriptionLogin, "device_code") => {
                if installed(m.app) && for_app.is_none_or(|a| a == m.app) {
                    ways.push(way("app-device", "Sign in", "none", Some(m.app), None, vec![]));
                }
            }
            (reg::MethodKind::ApiKey, _) => {
                let mut w = way("key", "Paste key", "required", None, None, way_fields(p, m));
                w.label = "Paste an API key".into();
                w.help = p.key_help.into();
                ways.push(w);
                if input.shell_keys.iter().any(|k| k == m.env) {
                    let mut sh = way("shell", "Use it", "none", None, Some(m.env), vec![]);
                    sh.kind = "api_key".into();
                    sh.label = format!("Use {} from your shell", m.env);
                    sh.help = "Cua saves a copy now. It doesn't follow later changes in your shell.".into();
                    sh.fields = way_fields(p, m);
                    ways.push(sh);
                }
            }
            (reg::MethodKind::CustomEndpoint, _) => {
                let mut w = way("endpoint", "Set up", if m.secret_optional { "optional" } else { "required" }, None, None, way_fields(p, m));
                w.label = "Connect a server by its base URL".into();
                w.help = p.key_help.into();
                ways.push(w);
            }
            // Methods Cua does not run yet (browser approval, cloud credentials,
            // a local server probe) are not offered.
            _ => {}
        }
    }
    let works_with = chips(p);
    let lede = if works_with.is_empty() {
        String::new()
    } else {
        format!(
            "Works with {}.",
            and_list(&works_with.iter().map(|c| c.name.as_str()).collect::<Vec<_>>())
        )
    };
    let this = os.this_machine_lower();
    let place = vault(os).map(|v| format!("in {v} ")).unwrap_or_default();
    Some(ProviderSheetView {
        provider: p.id.into(),
        icon: p.icon.into(),
        title: format!("Connect {}", p.name),
        lede,
        works_with,
        ways,
        cancel_label: "Cancel".into(),
        key_label: "API key".into(),
        key_placeholder: p.placeholder.into(),
        key_help: format!("It's saved {place}on {this} and won't be shown again."),
        token_label: "Claude token".into(),
        token_placeholder: "sk-ant-oat01-\u{2026}".into(),
        save_label: "Save".into(),
        note: (p.kind == reg::Kind::Custom).then(|| {
            format!(
                "A server on {this} is only reachable from a Space that is allowed to reach that port. Cua doesn't open it for you."
            )
        }),
        switch_label: "Other providers".into(),
    })
}

/// The provider choice, for `for_app` when given (only providers it can use).
/// Closed and with no search it lists the first tier and counts the rest;
/// `expanded` or a typed `query` lists every match.
pub fn provider_picker(
    input: &ConnectorsInput,
    for_app: Option<&str>,
    query: &str,
    expanded: bool,
) -> ProviderPickerView {
    let usable: Vec<&'static reg::Provider> = match for_app.and_then(app_sign_in) {
        Some(a) => a.providers.clone(),
        None => reg::visible().collect(),
    };
    let choice = |p: &reg::Provider| {
        let row = provider_row(input, p, true);
        ProviderChoice {
            provider: p.id.into(),
            label: p.name.into(),
            icon: p.icon.into(),
            tier: row.tier,
            state: row.state,
            status: row.status,
        }
    };
    let searching = !query.trim().is_empty();
    let open = expanded || searching;
    let up_front = |p: &reg::Provider| p.tier == reg::Tier::Top || provider_credential(input, p).is_some();
    let items: Vec<ProviderChoice> = usable
        .iter()
        .filter(|p| provider_matches(p, query))
        .filter(|p| open || up_front(p))
        .map(|p| choice(p))
        .collect();
    let hidden = usable.iter().filter(|p| !up_front(p)).count() as u32;
    ProviderPickerView {
        title: "Connect a provider".into(),
        lede: match for_app.and_then(|a| input.row_app(a)) {
            Some(a) if for_app.is_some() => format!("Providers {} can use.", a.name),
            _ => String::new(),
        },
        search_label: "Search providers".into(),
        empty: (open && items.is_empty()).then(|| "No provider matches that.".to_string()),
        more_label: (!open && hidden > 0).then(|| "More providers".to_string()),
        more_count: if open { 0 } else { hidden },
        items,
        cancel_label: "Cancel".into(),
    }
}

/// What importing Claude Code's sign-in says before it reads anything, and
/// what it offers when there is nothing to import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSheetView {
    pub title: String,
    pub lede: String,
    /// What is read, what is kept, where it goes: plain sentences.
    pub points: Vec<String>,
    pub confirm_label: String,
    pub cancel_label: String,
    /// While it reads (and macOS may be asking to allow it).
    pub reading_label: String,
    /// When there is no sign-in to import.
    pub not_found_title: String,
    pub not_found_lede: String,
    /// In order: sign in with Claude Code, paste a token, use an API key.
    pub options: Vec<ImportOption>,
}

/// One way forward when there is no sign-in to import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOption {
    /// `login`, `token`, `key`.
    pub id: String,
    pub label: String,
    pub help: String,
    /// Something to copy and run (`claude auth login`), when there is one.
    pub command: Option<String>,
    pub button: String,
}

/// The words around importing Claude Code's sign-in, for a shell on `os`.
pub fn import_sheet_on(os: SpaceOs) -> ImportSheetView {
    let this = os.this_machine_lower();
    let keychain = vault(os);
    let from = match os {
        SpaceOs::Macos => {
            "the Keychain item \u{201c}Claude Code-credentials\u{201d} (macOS asks you to allow it), or the file ~/.claude/.credentials.json"
        }
        SpaceOs::Windows => "the .credentials.json file in your .claude folder",
        SpaceOs::Linux | SpaceOs::Unknown => "the file ~/.claude/.credentials.json",
    };
    let keep = match keychain {
        Some(v) => format!("in Cua's own item in {v}"),
        None => "in Cua's own place for credentials".to_string(),
    };
    ImportSheetView {
        title: "Use your Claude Code sign-in".into(),
        lede: format!(
            "Run Claude Code in your Spaces with the Claude plan you already use on {this}."
        ),
        points: vec![
            format!("Cua reads your Claude Code sign-in from {from}."),
            format!(
                "It keeps the access token {keep}, never your refresh token, and doesn't change Claude Code's sign-in. You stay signed in to Claude Code."
            ),
            "Your Spaces get that access token so Claude Code can run there. Only Claude Code uses it.".into(),
            "When it runs out, Cua reads the newer one from Claude Code. Sign out here to remove the copy.".into(),
        ],
        confirm_label: "Use my sign-in".into(),
        cancel_label: "Cancel".into(),
        reading_label: "Reading your Claude Code sign-in…".into(),
        not_found_title: "No Claude Code sign-in found".into(),
        not_found_lede: format!(
            "Claude Code isn't signed in on {this}, or its sign-in couldn't be read. Pick one:"
        ),
        options: vec![
            ImportOption {
                id: "login".into(),
                label: "Sign in with Claude Code".into(),
                help: "Run this in Terminal, finish signing in, then try again.".into(),
                command: Some("claude auth login".into()),
                button: "Try again".into(),
            },
            ImportOption {
                id: "token".into(),
                label: "Paste a token".into(),
                help: "Advanced. A token made by running claude setup-token.".into(),
                command: None,
                button: "Paste token".into(),
            },
            ImportOption {
                id: "key".into(),
                label: "Use an Anthropic API key".into(),
                help: "Create a key at console.anthropic.com. It's billed to your API account, not your Claude plan.".into(),
                command: None,
                button: "Paste key".into(),
            },
        ],
    }
}

/// A switch on an app's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorToggle {
    /// `skills`, `mcp`, `auto`.
    pub id: String,
    pub label: String,
    pub help: String,
    pub on: bool,
    pub enabled: bool,
}

/// The sign-in part of an app's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSignIn {
    /// "Signs in with Anthropic".
    pub title: String,
    /// The provider row's status for it ("Claude Code sign-in", "Not connected").
    pub status: String,
    pub connected: bool,
    /// How Connect signs it in ("the sign-in Claude Code already has").
    pub way: String,
    /// The provider it uses (for Sign in and Sign out).
    pub provider: String,
    pub button: ConnectorButton,
}

/// One app's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDetailView {
    pub agent: String,
    pub name: String,
    pub back_label: String,
    /// Under the title: "Connected", "Not connected", or what is missing.
    pub status: String,
    pub connected: bool,
    /// Connect, while not connected.
    pub connect: Option<ConnectorButton>,
    pub tools_title: String,
    pub toggles: Vec<ConnectorToggle>,
    pub sign_in_title: String,
    /// Absent for an app that signs in on its own.
    pub sign_in: Option<ConnectorSignIn>,
    /// Instead of `sign_in`: the app signs in on its own.
    pub sign_in_note: Option<String>,
    pub settings_file_label: String,
    pub settings_file: Option<String>,
    /// "Remove Cua from Claude Code", while Cua is in it.
    pub remove_label: Option<String>,
}

fn way_text(app: &str) -> &'static str {
    match app {
        "claude-code" => {
            "Connect uses the sign-in Claude Code already has on this machine, with your Claude plan. Or connect Anthropic with an API key."
        }
        "codex" => {
            "Connect signs Codex in with ChatGPT: a code to enter on openai.com, using your ChatGPT plan. Or connect OpenAI with an API key."
        }
        "copilot-cli" => "Connect asks for a GitHub token with the Copilot Requests permission.",
        "hermes" | "opencode" | "goose" | "pi" | "openclaw" => {
            "It runs in your Spaces with a key from any provider you connect."
        }
        _ => "It uses whichever of these providers you connect.",
    }
}

/// `agent`'s page (None: not on this computer).
pub fn app_detail(input: &ConnectorsInput, agent: &str) -> Option<AppDetailView> {
    let a = &input.row_app(agent)?;
    let row = app_row(input, a);
    // An agent harness that is not on this machine has no local tools to switch.
    let local = a.installed || a.configured;
    let can_edit = notice(input).is_none();
    let busy = input.busy(&format!("app:{agent}")).is_some();
    let mut toggles = vec![];
    if local {
        toggles.push(ConnectorToggle {
            id: "skills".into(),
            label: "Cua skills".into(),
            help: format!("Teaches {} how to use your Spaces.", a.name),
            on: a.skills,
            enabled: !busy,
        });
    }
    if let Some(on) = a.mcp {
        toggles.push(ConnectorToggle {
            id: "mcp".into(),
            label: "Cua tools (MCP)".into(),
            help: format!("Lets {} open, use and share your Spaces.", a.name),
            on,
            enabled: !busy,
        });
    }
    if local {
        toggles.push(ConnectorToggle {
            id: "auto".into(),
            label: "Keep it set up".into(),
            help: format!("If {} changes its settings, Cua adds itself again.", a.name),
            on: !input.off.contains(&a.agent),
            enabled: !busy,
        });
    }
    let sign_in = app_sign_in(agent).map(|s| {
        // The provider it uses: the first connected one, else the first.
        let p = s
            .providers
            .iter()
            .find(|p| provider_connected(input, p.id))
            .or(s.providers.first())
            .copied();
        let pid = p.map(|p| p.id).unwrap_or_default();
        let prow = p.map(|p| provider_row(input, p, can_edit));
        let connected = prow.as_ref().is_some_and(|r| r.state == "connected");
        let names: Vec<&str> = s.providers.iter().map(|p| p.name).collect();
        let names = if names.len() > 3 && s.way.is_none() {
            "a connected provider".to_string()
        } else {
            or_list(&names)
        };
        ConnectorSignIn {
            title: format!("Signs in with {names}"),
            status: prow
                .as_ref()
                .map(|r| r.credential.clone().unwrap_or_else(|| r.status.clone()))
                .unwrap_or_default(),
            connected,
            way: way_text(agent).into(),
            provider: pid.into(),
            button: if connected {
                button("sign-out", "Sign out", can_edit && !busy)
            } else {
                button("sign-in", "Sign in", can_edit && !busy)
            },
        }
    });
    let sign_in_note = sign_in.is_none().then(|| {
        format!(
            "{} signs in on its own. Cua doesn't need a sign-in for it.",
            a.name
        )
    });
    Some(AppDetailView {
        agent: a.agent.clone(),
        name: a.name.clone(),
        back_label: "Connections".into(),
        status: match row.state.as_str() {
            "connected" => signed_in_as(input, agent).unwrap_or_else(|| "Connected".into()),
            _ => row.line.clone().unwrap_or_else(|| "Not connected".into()),
        },
        connected: row.state == "connected",
        connect: row.button.filter(|_| row.state != "connected"),
        tools_title: if local {
            format!("Cua in {}", a.name)
        } else {
            String::new()
        },
        toggles,
        sign_in_title: "Sign-in".into(),
        sign_in,
        sign_in_note,
        settings_file_label: "Settings file".into(),
        settings_file: a.mcp_config.clone(),
        remove_label: (a.skills || a.mcp == Some(true))
            .then(|| format!("Remove Cua from {}", a.name)),
    })
}

/// The card a task shows when its app isn't connected yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupCardView {
    pub title: String,
    pub body: String,
    /// "Connect Claude Code".
    pub connect_label: String,
    /// While Connect runs.
    pub connecting_label: String,
    /// Once connected (the task starts again on its own).
    pub connected_label: String,
    pub retry_label: String,
    pub change_label: String,
    pub details_label: String,
}

/// The card for a task whose app `agent` (called `name`) can't sign in.
/// `provider` is the provider that refused it, when a run said which.
pub fn setup_card(input: &ConnectorsInput, agent: &str, name: &str) -> SetupCardView {
    setup_card_for(input, agent, name, None)
}

/// [`setup_card`] for a failure that named `provider`.
pub fn setup_card_for(
    input: &ConnectorsInput,
    agent: &str,
    name: &str,
    provider_id: Option<&str>,
) -> SetupCardView {
    let plan = connect_plan(input, agent);
    let s = app_sign_in(agent);
    let provider = provider_id
        .and_then(provider)
        .or_else(|| {
            s.as_ref()
                .filter(|s| s.providers.len() == 1 || s.way.is_some())
                .and_then(|s| s.providers.first().copied())
        })
        .map(|p| p.name);
    let body = match provider {
        Some(p) if plan.app_token => format!(
            "{name} needs a sign-in to {p} to work here. Connect it once: Cua uses the sign-in {name} already has on this machine, and every Space can use it."
        ),
        Some(p) => format!(
            "{name} needs a sign-in to {p} to work here. Connect it once and every Space can use it."
        ),
        None if s.as_ref().is_some_and(|s| s.providers.len() > 1) => format!(
            "{name} needs a connected provider to work here. Connect one once and every Space can use it."
        ),
        None => format!("Connect {name} once and every Space can use it."),
    };
    SetupCardView {
        title: format!("Connect {name} to start"),
        body,
        connect_label: format!("Connect {name}"),
        connecting_label: if plan.app_token {
            "Reading your Claude Code sign-in…".into()
        } else if plan.device {
            "Enter the code in your browser…".into()
        } else {
            "Connecting…".into()
        },
        connected_label: format!("{name} is connected. Starting again…"),
        retry_label: "Try again".into(),
        change_label: "Change computer".into(),
        details_label: "Details".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(agent: &str, name: &str, configured: bool) -> ConnectorAppInput {
        ConnectorAppInput {
            agent: agent.into(),
            name: name.into(),
            installed: true,
            configured,
            skills: configured,
            mcp: Some(configured),
            mcp_config: Some(format!("~/.{agent}/settings.json")),
        }
    }

    fn key(env: &str, last4: &str) -> AgentKeyInput {
        AgentKeyInput {
            provider: if env == "ANTHROPIC_API_KEY" {
                "anthropic".into()
            } else if env == "OPENAI_API_KEY" {
                "openai".into()
            } else {
                "other".into()
            },
            env: env.into(),
            last4: last4.into(),
            added_ms: 1_000,
            expired: false,
            label: None,
            region: None,
            fields: Default::default(),
        }
    }

    fn input() -> ConnectorsInput {
        ConnectorsInput {
            apps: vec![
                app("claude-code", "Claude Code", false),
                app("codex", "OpenAI Codex", true),
                app("vscode", "VS Code (Copilot agent mode)", false),
            ],
            ..Default::default()
        }
    }

    /// The credentials an app reads here are the ones its harness reads
    /// (`cua_agents::harness`), so "connected" never outruns what a run gets.
    #[test]
    fn what_an_app_reads_is_what_its_harness_reads() {
        for h in HARNESS_APPS {
            let a = app_sign_in(h.app).unwrap_or_else(|| panic!("{} has no sign-in", h.app));
            let real = cua_agents::harness::harness(h.harness).unwrap();
            assert_eq!(a.reads, real.keys, "{}", h.app);
        }
    }

    /// Drift guard: every agent harness has a row on the Connectors page. A
    /// harness added to `cua_agents::harness::HARNESSES` without an entry in
    /// `HARNESS_APPS` (and a sign-in in `APPS`) fails here.
    #[test]
    fn every_agent_harness_has_a_connectors_row() {
        for h in cua_agents::harness::HARNESSES {
            let row = HARNESS_APPS.iter().find(|x| x.harness == h.id);
            let row = row.unwrap_or_else(|| {
                panic!(
                    "harness {:?} has no Connectors row: add it to HARNESS_APPS and APPS in connectors.rs",
                    h.id
                )
            });
            assert!(app_sign_in(row.app).is_some(), "{}: no sign-in", row.app);
            // The row is the setup registry's app of that name.
            let spec = cua_agent_setup::registry::find(row.app)
                .unwrap_or_else(|| panic!("{} is not in the setup registry", row.app));
            assert_eq!(spec.name, row.name, "{}", row.app);
            // It shows with nothing installed and nothing saved, with Connect.
            let v = view(&ConnectorsInput::default());
            let shown = v.apps.iter().find(|a| a.agent == row.app);
            let shown = shown.unwrap_or_else(|| panic!("{} is not listed", row.app));
            assert_eq!(shown.state, "not-connected", "{}", row.app);
            assert_eq!(
                shown.button.as_ref().map(|b| b.label.as_str()),
                Some("Connect")
            );
            assert!(shown.button.as_ref().is_some_and(|b| b.enabled));
        }
        assert_eq!(HARNESS_APPS.len(), cua_agents::harness::HARNESSES.len());
        // Each provider an app offers connects it: it delivers a variable the app
        // reads, or the app is pointed at the provider's endpoint.
        for h in HARNESS_APPS {
            let a = app_sign_in(h.app).unwrap();
            assert!(!a.providers.is_empty(), "{}", h.app);
            for p in &a.providers {
                let native = p.delivery_for(h.harness);
                if native.is_empty() {
                    assert!(
                        reg::ENDPOINT_HARNESSES.iter().any(|(x, _)| *x == h.harness),
                        "{}: {} saves nothing it reads",
                        h.app,
                        p.id
                    );
                } else {
                    assert!(native.iter().all(|e| a.reads.contains(&e.dest)), "{}: {}", h.app, p.id);
                }
            }
        }
    }

    #[test]
    fn every_harness_is_listed_without_show_all_installed_or_not() {
        // Nothing installed on this machine: still every harness, in order.
        let v = view(&ConnectorsInput::default());
        let ids: Vec<_> = v.apps.iter().map(|a| a.agent.as_str()).collect();
        assert_eq!(
            ids,
            [
                "claude-code",
                "codex",
                "gemini-cli",
                "antigravity",
                "opencode",
                "goose",
                "pi",
                "hermes",
                "openclaw"
            ]
        );
        let hermes = v.apps.iter().find(|a| a.agent == "hermes").unwrap();
        assert_eq!(hermes.name, "Hermes");
        assert_eq!(
            hermes.line.as_deref(),
            Some("Needs a connected provider.")
        );
        // The other apps Cua knows (not agents) wait under Show all with where to get them.
        let mut i = input();
        i.apps.push(ConnectorAppInput {
            agent: "windsurf".into(),
            name: "Windsurf".into(),
            ..Default::default()
        });
        let v = view(&i);
        let more: Vec<_> = v.more.iter().map(|m| m.agent.as_str()).collect();
        assert_eq!(more, ["windsurf"]);
        // An installed app that is not an agent is listed after the harnesses.
        assert_eq!(v.apps.last().unwrap().agent, "vscode");
    }

    #[test]
    fn a_harness_off_this_machine_is_connected_by_what_it_can_read() {
        let mut i = ConnectorsInput::default();
        // Goose reads an OpenAI key; the Claude Code sign-in is Claude Code's alone.
        i.keys.keys.push(key("OPENAI_API_KEY", "3f9a"));
        let v = view(&i);
        let row = |id: &str| v.apps.iter().find(|a| a.agent == id).unwrap().clone();
        for id in ["goose", "opencode", "pi", "hermes", "openclaw"] {
            let r = row(id);
            assert_eq!(r.state, "connected", "{id}");
            assert_eq!(r.line.as_deref(), Some("Runs in your Spaces"), "{id}");
            assert!(r.button.is_none());
        }
        // Not what Gemini CLI reads.
        assert_eq!(row("gemini-cli").state, "not-connected");
        let mut t = ConnectorsInput::default();
        t.keys.keys.push(key("CLAUDE_CODE_OAUTH_TOKEN", "AAAA"));
        let v = view(&t);
        let state = |id: &str| v.apps.iter().find(|a| a.agent == id).unwrap().state.clone();
        // No harness but Claude Code reads the Claude Code sign-in.
        for id in ["hermes", "goose", "opencode", "pi", "openclaw"] {
            assert_eq!(state(id), "not-connected", "{id}");
        }
        // An OpenRouter key connects the ones that read it.
        let mut r = ConnectorsInput::default();
        r.keys.keys.push(key("OPENROUTER_API_KEY", "7777"));
        let v = view(&r);
        let state = |id: &str| v.apps.iter().find(|a| a.agent == id).unwrap().state.clone();
        for id in ["opencode", "goose", "pi", "hermes", "openclaw"] {
            assert_eq!(state(id), "connected", "{id}");
        }
        assert_eq!(state("claude-code"), "not-connected");
    }

    #[test]
    fn connect_on_a_harness_off_this_machine_asks_for_a_provider_and_offers_the_rest() {
        let i = ConnectorsInput::default();
        let plan = connect_plan(&i, "opencode");
        assert_eq!(
            plan,
            ConnectPlan {
                tools: false,
                app_token: false,
                device: false,
                provider: Some("anthropic".into()),
                token_provider: None
            }
        );
        // The choice lists the first tier for the app, then the rest on request.
        let pick = provider_picker(&i, Some("opencode"), "", false);
        let up_front: Vec<_> = pick.items.iter().map(|c| c.provider.as_str()).collect();
        assert_eq!(up_front, ["anthropic", "openai", "openrouter", "google", "xai", "deepseek", "kimi", "custom"]);
        assert_eq!(pick.more_label.as_deref(), Some("More providers"));
        assert!(pick.more_count > 10);
        let found = provider_picker(&i, Some("opencode"), "fire", false);
        assert_eq!(found.items.iter().map(|c| c.provider.as_str()).collect::<Vec<_>>(), ["fireworks"]);
        assert!(found.more_label.is_none());
        assert!(provider_picker(&i, Some("opencode"), "zzzz", false).empty.is_some());
        // Claude Code takes Anthropic only, so there is nothing to choose.
        let cc = provider_picker(&i, Some("claude-code"), "", true);
        assert_eq!(cc.items.iter().map(|c| c.provider.as_str()).collect::<Vec<_>>(), ["anthropic"]);
        let s = provider_sheet_on(&i, "anthropic", SpaceOs::Macos, Some("opencode")).unwrap();
        assert_eq!(s.switch_label, "Other providers");
        // OpenRouter is an API key.
        let o = provider_sheet_on(&i, "openrouter", SpaceOs::Macos, Some("hermes")).unwrap();
        let ids: Vec<_> = o.ways.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["key"]);
        assert_eq!(o.ways[0].kind, "api_key");
        assert_eq!(o.ways[0].secret, "required");
        assert!(o.lede.starts_with("Works with ") && o.lede.contains("Hermes"), "{}", o.lede);
    }

    #[test]
    fn a_harness_off_this_machine_has_a_page_with_its_sign_in_and_no_local_tools() {
        let d = app_detail(&ConnectorsInput::default(), "goose").unwrap();
        assert_eq!(d.name, "Goose");
        assert!(d.toggles.is_empty() && d.tools_title.is_empty());
        assert_eq!(
            d.sign_in.as_ref().unwrap().title,
            "Signs in with a connected provider"
        );
        assert_eq!(d.connect.as_ref().unwrap().label, "Connect");
        assert!(d.remove_label.is_none() && d.settings_file.is_none());
        assert!(app_detail(&ConnectorsInput::default(), "vscode").is_none());
    }

    #[test]
    fn hermes_is_connected_only_by_a_credential_it_reads() {
        let mut i = input();
        // The ChatGPT sign-in is for Codex; Hermes can't read it.
        i.keys.keys.push(key("CODEX_AUTH_JSON", "AAAA"));
        assert!(app_signed_in(&i, "codex"));
        assert!(!app_signed_in(&i, "hermes"));
        // An OpenAI API key, or the Claude sign-in token, it reads.
        let mut k = input();
        k.keys.keys.push(key("OPENAI_API_KEY", "AAAA"));
        assert!(app_signed_in(&k, "hermes"));
        let mut t = input();
        t.keys.keys.push(key("CLAUDE_CODE_OAUTH_TOKEN", "AAAA"));
        // The Claude Code sign-in is Claude Code's alone.
        assert!(app_signed_in(&t, "claude-code"));
        assert!(!app_signed_in(&t, "hermes"));
        assert!(!app_signed_in(&t, "codex"));
        let mut a = input();
        a.keys.keys.push(key("ANTHROPIC_API_KEY", "AAAA"));
        assert!(app_signed_in(&a, "hermes") && app_signed_in(&a, "claude-code"));
    }

    #[test]
    fn a_sign_in_that_no_longer_works_is_not_connected_and_says_reconnect() {
        let mut i = input();
        i.apps[0].configured = true;
        let mut k = key("CLAUDE_CODE_OAUTH_TOKEN", "AAAA");
        k.expired = true;
        i.keys.keys.push(k);
        assert!(!app_signed_in(&i, "claude-code"));
        let v = view(&i);
        let p = v.providers.iter().find(|p| p.id == "anthropic").unwrap();
        assert_eq!(p.state, "not-connected");
        assert_eq!(p.status, "Needs reconnect");
        assert_eq!(p.button.label, "Reconnect");
        let a = v.apps.iter().find(|a| a.agent == "claude-code").unwrap();
        assert_eq!(a.line.as_deref(), Some("Needs reconnect"));
        // The app's row offers Reconnect, not a first Connect.
        assert_eq!(
            a.button.as_ref().map(|b| b.label.as_str()),
            Some("Reconnect")
        );
    }

    #[test]
    fn apps_start_with_connect_and_providers_not_connected() {
        let v = view(&input());
        assert_eq!(v.apps_title, "Coding apps");
        assert_eq!(v.providers_title, "Connections");
        let claude = &v.apps[0];
        assert_eq!(claude.state, "not-connected");
        assert_eq!(claude.button.as_ref().unwrap().label, "Connect");
        assert_eq!(claude.connected_label, None);
        assert_eq!(claude.open_label, "Claude Code settings");
        // Codex has Cua but no OpenAI sign-in: still Connect, with what's missing.
        let codex = &v.apps[1];
        assert_eq!(codex.state, "not-connected");
        assert_eq!(codex.line.as_deref(), Some("Needs a sign-in to OpenAI."));
        let ids: Vec<_> = v.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            ["anthropic", "openai", "openrouter", "google", "xai", "deepseek", "kimi", "custom"]
        );
        assert!(
            v.providers
                .iter()
                .all(|p| p.status == "Not connected" && p.button.label == "Connect" && p.credential.is_none())
        );
        // Everything else waits in the searchable list, closed until opened.
        assert!(v.more_providers.is_empty() && v.more_count > 10);
    }

    #[test]
    fn an_app_that_signs_in_on_its_own_is_connected_with_cua_in_it() {
        let mut i = input();
        i.apps[2].configured = true;
        let v = view(&i);
        let vscode = v.apps.iter().find(|a| a.agent == "vscode").unwrap();
        assert_eq!(vscode.state, "connected");
        assert_eq!(vscode.connected_label.as_deref(), Some("Connected"));
        assert!(vscode.button.is_none());
    }

    #[test]
    fn connecting_claude_code_connects_anthropic_too() {
        let mut i = input();
        i.apps[0].configured = true;
        i.keys.keys.push(key("CLAUDE_CODE_OAUTH_TOKEN", "AAAA"));
        let v = view(&i);
        assert_eq!(v.apps[0].state, "connected");
        let a = &v.providers[0];
        assert_eq!(a.state, "connected");
        assert_eq!(a.status, "Connected");
        assert_eq!(a.credential.as_deref(), Some("Signed in to Claude"));
        assert_eq!(a.button.label, "Disconnect");
        assert_eq!(a.sign_out_envs, ["CLAUDE_CODE_OAUTH_TOKEN"]);
        // The sign-in token is the provider's, not an Other key.
        assert!(v.others.is_empty());
    }

    #[test]
    fn a_pasted_key_reads_with_its_last_four_and_others_follow() {
        let mut i = input();
        i.keys.keys.push(key("OPENAI_API_KEY", "3f9a"));
        i.keys.keys.push(key("MISTRAL_API_KEY", "9999"));
        let v = view(&i);
        assert_eq!(v.providers[1].status, "Connected");
        assert_eq!(
            v.providers[1].credential.as_deref(),
            Some("API key \u{2022}\u{2022}\u{2022}\u{2022} 3f9a")
        );
        assert_eq!(v.apps[1].state, "connected");
        assert_eq!(v.others.len(), 1);
        assert_eq!(v.others[0].env, "MISTRAL_API_KEY");
    }

    #[test]
    fn busy_and_failed_rows_say_so() {
        let mut i = input();
        i.busy.push(ConnectorBusy {
            id: "app:claude-code".into(),
            step: "signing-in".into(),
        });
        i.busy.push(ConnectorBusy {
            id: "provider:anthropic".into(),
            step: "signing-in".into(),
        });
        i.failures.push(ConnectorFailure {
            id: "app:codex".into(),
            line: "Couldn't add Cua to ~/.codex/config.toml.".into(),
        });
        let v = view(&i);
        assert_eq!(v.apps[0].state, "connecting");
        assert_eq!(
            v.apps[0].line.as_deref(),
            Some("Reading your Claude Code sign-in…")
        );
        assert!(!v.apps[0].button.as_ref().unwrap().enabled);
        assert_eq!(v.providers[0].state, "connecting");
        assert_eq!(v.providers[0].status, "Reading your Claude Code sign-in…");
        assert_eq!(v.apps[1].state, "failed");
        assert_eq!(v.apps[1].button.as_ref().unwrap().label, "Try again");
    }

    #[test]
    fn connect_uses_only_official_sign_ins() {
        let i = input();
        // Claude Code: its own token command, once.
        assert_eq!(
            connect_plan(&i, "claude-code"),
            ConnectPlan {
                tools: true,
                app_token: true,
                device: false,
                provider: None,
                token_provider: Some("anthropic".into())
            }
        );
        // Codex: its own device-code sign-in, in a folder Cua owns.
        assert_eq!(
            connect_plan(&i, "codex"),
            ConnectPlan {
                tools: false,
                app_token: false,
                device: true,
                provider: None,
                token_provider: Some("openai".into())
            }
        );
        // Not on This Mac: the OpenAI sheet asks for a key.
        let mut no_codex = i.clone();
        no_codex.apps.retain(|a| a.agent != "codex");
        assert_eq!(
            connect_plan(&no_codex, "codex").provider.as_deref(),
            Some("openai")
        );
        // Not on This Mac: no token command to run here; the provider's sheet asks.
        let mut away = i.clone();
        away.apps.retain(|a| a.agent != "claude-code");
        assert_eq!(
            connect_plan(&away, "claude-code"),
            ConnectPlan {
                tools: false,
                app_token: false,
                device: false,
                provider: Some("anthropic".into()),
                token_provider: None
            }
        );
        // An app that signs in on its own: only Cua's tools.
        assert_eq!(
            connect_plan(&i, "vscode"),
            ConnectPlan {
                tools: true,
                app_token: false,
                device: false,
                provider: None,
                token_provider: None
            }
        );
    }

    #[test]
    fn provider_sheet_offers_sign_in_key_and_shell() {
        let mut i = input();
        i.shell_keys.push("ANTHROPIC_API_KEY".into());
        let s = provider_sheet(&i, "anthropic").unwrap();
        assert_eq!(s.title, "Connect Anthropic");
        let ids: Vec<_> = s.ways.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["app-token", "token", "key", "shell"]);
        assert_eq!(s.ways[0].app.as_deref(), Some("claude-code"));
        assert_eq!(s.ways[3].label, "Use ANTHROPIC_API_KEY from your shell");
        // Without Claude Code there is no Claude sign-in to offer; without the
        // variable, no shell row.
        i.apps.retain(|a| a.agent != "claude-code");
        i.shell_keys.clear();
        let s = provider_sheet(&i, "anthropic").unwrap();
        let ids: Vec<_> = s.ways.iter().map(|w| w.id.as_str()).collect();
        // A token from `claude setup-token` can be pasted without Claude Code here.
        assert_eq!(ids, ["token", "key"]);
        assert_eq!(s.provider, "anthropic");
        let g = provider_sheet(&i, "github").unwrap();
        assert_eq!(g.ways.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["key"]);
        assert!(provider_sheet(&i, "nope").is_none());
    }

    #[test]
    fn app_detail_has_switches_and_its_sign_in() {
        let mut i = input();
        i.apps[0].configured = true;
        i.apps[0].skills = true;
        i.apps[0].mcp = Some(true);
        let d = app_detail(&i, "claude-code").unwrap();
        assert_eq!(d.tools_title, "Cua in Claude Code");
        let ids: Vec<_> = d.toggles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["skills", "mcp", "auto"]);
        assert!(d.toggles.iter().all(|t| t.on));
        let s = d.sign_in.as_ref().unwrap();
        assert_eq!(s.title, "Signs in with Anthropic");
        assert_eq!(s.button.label, "Sign in");
        assert!(!d.connected);
        assert_eq!(d.status, "Needs a sign-in to Anthropic.");
        assert_eq!(
            d.remove_label.as_deref(),
            Some("Remove Cua from Claude Code")
        );
        // Signed in: Sign out, and Connected.
        i.keys.keys.push(key("CLAUDE_CODE_OAUTH_TOKEN", "AAAA"));
        let d = app_detail(&i, "claude-code").unwrap();
        assert!(d.connected);
        assert_eq!(d.sign_in.unwrap().button.label, "Sign out");
        // Turned off: Keep it set up is off.
        i.off.push("claude-code".into());
        let d = app_detail(&i, "claude-code").unwrap();
        assert!(!d.toggles.iter().find(|t| t.id == "auto").unwrap().on);
        // An app that signs in on its own says so.
        let d = app_detail(&i, "vscode").unwrap();
        assert!(d.sign_in.is_none());
        assert!(d.sign_in_note.unwrap().contains("signs in on its own"));
    }

    fn imported(label: &str) -> AgentKeyInput {
        let mut k = key("CLAUDE_CODE_OAUTH_TOKEN", "AAAA");
        k.label = Some(label.into());
        k
    }

    /// One sign-in, one wording, on every surface: the Claude Code app row, the
    /// Anthropic provider row, the app's page and the sign-in block.
    #[test]
    fn an_imported_sign_in_reads_signed_in_as_everywhere() {
        let who = "Signed in as ada@example.com (Claude Max)";
        let mut i = input();
        i.apps[0].configured = true;
        i.keys.keys.push(imported(who));
        let v = view(&i);
        assert_eq!(v.apps[0].state, "connected");
        assert_eq!(v.apps[0].line.as_deref(), Some(who));
        assert_eq!(v.providers[0].status, "Connected");
        assert_eq!(v.providers[0].credential.as_deref(), Some(who));
        let d = app_detail(&i, "claude-code").unwrap();
        assert_eq!(d.status, who);
        assert_eq!(d.sign_in.as_ref().unwrap().status, who);
        assert_eq!(d.sign_in.as_ref().unwrap().button.label, "Sign out");
        // Without a name (an older copy) it still says what it is.
        let mut j = input();
        j.apps[0].configured = true;
        j.keys.keys.push(key("CLAUDE_CODE_OAUTH_TOKEN", "AAAA"));
        assert_eq!(view(&j).providers[0].credential.as_deref(), Some("Signed in to Claude"));
    }

    #[test]
    fn needs_reconnect_reads_the_same_on_every_surface() {
        let mut i = input();
        i.apps[0].configured = true;
        let mut k = imported("Signed in as ada@example.com (Claude Max)");
        k.expired = true;
        i.keys.keys.push(k);
        let v = view(&i);
        assert_eq!(v.apps[0].line.as_deref(), Some("Needs reconnect"));
        assert_eq!(v.providers[0].status, "Needs reconnect");
        assert_eq!(v.apps[0].button.as_ref().unwrap().label, "Reconnect");
        assert_eq!(v.providers[0].button.label, "Reconnect");
        let d = app_detail(&i, "claude-code").unwrap();
        assert_eq!(d.status, "Needs reconnect");
        assert_eq!(d.connect.as_ref().unwrap().label, "Reconnect");
        // An expired copy never shows the name as if it worked.
        assert!(!format!("{v:?}").contains("ada@example.com"));
    }

    #[test]
    fn claude_codes_sign_in_running_out_is_not_hermes_to_reconnect() {
        let mut i = input();
        i.apps[0].configured = true;
        let mut k = imported("Signed in as ada@example.com (Claude Max)");
        k.expired = true;
        i.keys.keys.push(k);
        let v = view(&i);
        let hermes = v.apps.iter().find(|a| a.agent == "hermes").unwrap();
        assert_eq!(
            hermes.line.as_deref(),
            Some("Needs a connected provider.")
        );
        assert_eq!(hermes.button.as_ref().unwrap().label, "Connect");
        let claude = v.apps.iter().find(|a| a.agent == "claude-code").unwrap();
        assert_eq!(claude.button.as_ref().unwrap().label, "Reconnect");
    }

    #[test]
    fn the_import_sheet_says_what_is_read_kept_and_sent_in_plain_words() {
        let mac = import_sheet_on(SpaceOs::Macos);
        let text = format!("{}\n{}", mac.lede, mac.points.join("\n"));
        assert!(
            text.contains("Claude Code-credentials")
                && text.contains("~/.claude/.credentials.json")
        );
        assert!(text.contains("Keychain") && text.contains("never your refresh token"));
        assert!(text.contains("stay signed in to Claude Code"));
        assert!(text.contains("Your Spaces get that access token"));
        assert_eq!(mac.confirm_label, "Use my sign-in");
        // Nothing found: sign in with Claude Code, then a token, then a key, in that order.
        let ids: Vec<_> = mac.options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["login", "token", "key"]);
        assert_eq!(mac.options[0].command.as_deref(), Some("claude auth login"));
        // Other systems name their own place, never the macOS Keychain.
        let linux = import_sheet_on(SpaceOs::Linux);
        assert!(!linux.points.join(" ").contains("Keychain"));
        let win = import_sheet_on(SpaceOs::Windows);
        assert!(win.points[0].contains(".credentials.json"));
    }

    /// The words of this feature: no middle dot, no "devices", no promise of
    /// `claude setup-token` as the way Connect works.
    #[test]
    fn the_claude_wording_has_no_middle_dot_devices_or_setup_token_promise() {
        let mut i = input();
        i.apps[0].configured = true;
        i.keys
            .keys
            .push(imported("Signed in as ada@example.com (Claude Max)"));
        let mut all = format!("{:?}", view(&i));
        for os in [SpaceOs::Macos, SpaceOs::Windows, SpaceOs::Linux] {
            all += &format!("{:?}", import_sheet_on(os));
        }
        all += &format!("{:?}", app_detail(&i, "claude-code"));
        all += &format!("{:?}", provider_sheet(&input(), "anthropic"));
        all += &format!("{:?}", setup_card(&input(), "claude-code", "Claude Code"));
        assert!(!all.contains('\u{b7}'), "no middle dot");
        assert!(
            !all.to_lowercase().contains("device"),
            "machines, not devices"
        );
        assert!(!all.contains("Connect runs claude setup-token"));
        assert!(!all.contains("one browser approval"));
    }

    #[test]
    fn setup_card_names_the_app() {
        let i = input();
        let c = setup_card(&i, "claude-code", "Claude Code");
        assert_eq!(c.title, "Connect Claude Code to start");
        assert_eq!(c.connect_label, "Connect Claude Code");
        assert_eq!(c.connecting_label, "Reading your Claude Code sign-in…");
        assert!(c.body.contains("sign-in to Anthropic"));
        let c = setup_card(&i, "codex", "OpenAI Codex");
        assert_eq!(c.connect_label, "Connect OpenAI Codex");
        assert_eq!(c.connecting_label, "Enter the code in your browser…");
    }

    #[test]
    fn notice_turns_buttons_off() {
        let mut i = input();
        i.keys.error = Some("the daemon isn't running".into());
        let v = view(&i);
        assert!(!v.can_edit);
        assert_eq!(
            v.notice.as_deref(),
            Some("Couldn't read your sign-ins: the daemon isn't running")
        );
        assert!(v.providers.iter().all(|p| !p.button.enabled));
    }

    fn keyed(env: &str, provider: &str) -> AgentKeyInput {
        let mut k = key(env, "abcd");
        k.provider = provider.into();
        k
    }

    #[test]
    fn a_row_says_which_apps_it_works_with() {
        let v = view(&input());
        let names = |id: &str| -> Vec<String> {
            v.providers
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .works_with
                .iter()
                .map(|c| c.name.clone())
                .collect()
        };
        assert_eq!(names("anthropic"), ["Claude Code", "OpenCode", "Goose", "Pi", "Hermes", "OpenClaw", "T3 Code"]);
        assert_eq!(names("openai"), ["OpenAI Codex", "OpenCode", "Goose", "Pi", "Hermes", "OpenClaw", "T3 Code"]);
        assert_eq!(names("deepseek"), ["OpenCode", "Goose", "Pi", "Hermes", "OpenClaw"]);
        // Only Claude Code takes the Anthropic sign-in, and the chips never claim it elsewhere.
        for row in v.providers.iter().chain(v.more_providers.iter()) {
            assert_eq!(row.works_with_label, "Works with");
            assert_eq!(row.works_with.iter().any(|c| c.agent == "claude-code"), row.id == "anthropic", "{}", row.id);
        }
        // A chip opens its app's page: it is the app's setup id.
        let c = &v.providers[0].works_with[0];
        assert_eq!((c.agent.as_str(), c.name.as_str()), ("claude-code", "Claude Code"));
    }

    #[test]
    fn more_providers_is_a_closed_searchable_list_and_a_connected_one_never_hides_in_it() {
        let mut i = input();
        let v = view(&i);
        assert!(v.more_providers.is_empty());
        assert!(v.more_count as usize >= 20, "{}", v.more_count);
        i.more_open = true;
        let v = view(&i);
        let ids: Vec<_> = v.more_providers.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"huggingface") && ids.contains(&"zai") && ids.contains(&"fireworks") && ids.contains(&"nvidia") && ids.contains(&"ollama-cloud"));
        // Hidden providers (unreviewed logins, unverified ones) never show.
        for hidden in ["nous", "qwen-oauth", "minimax-oauth", "xai-oauth", "vertex", "bedrock", "azure-foundry", "meta-ai"] {
            assert!(!ids.contains(&hidden), "{hidden} is hidden");
            assert!(!v.providers.iter().any(|p| p.id == hidden));
        }
        // Search by name, id or alias; each word starts a word.
        for (q, want) in [("hugg", "huggingface"), ("hf", "huggingface"), ("glm", "zai"), ("z.ai", "zai"), ("nim", "nvidia"), ("fire", "fireworks"), ("together", "")] {
            i.query = q.into();
            let v = view(&i);
            let got: Vec<_> = v.more_providers.iter().map(|p| p.id.as_str()).collect();
            if want.is_empty() {
                assert!(got.is_empty() && v.more_empty.is_some(), "{q}: {got:?}");
            } else {
                assert!(got.contains(&want), "{q}: {got:?}");
            }
        }
        // A search opens the list by itself, and a match for nothing says so.
        i.more_open = false;
        i.query = "fireworks".into();
        assert_eq!(view(&i).more_providers.len(), 1);
        i.query = "zzz".into();
        assert_eq!(view(&i).more_empty.as_deref(), Some("No provider matches that."));
        // One that is connected moves up front, and one that needs reconnecting too.
        let mut c = input();
        c.keys.keys.push(keyed("FIREWORKS_API_KEY", "fireworks"));
        let mut r = keyed("HF_TOKEN", "huggingface");
        r.expired = true;
        c.keys.keys.push(r);
        let v = view(&c);
        let up: Vec<_> = v.providers.iter().map(|p| (p.id.as_str(), p.state.as_str(), p.status.as_str())).collect();
        assert!(up.contains(&("fireworks", "connected", "Connected")), "{up:?}");
        assert!(up.contains(&("huggingface", "not-connected", "Needs reconnect")), "{up:?}");
        assert!(v.providers.len() == 10);
    }

    #[test]
    fn an_app_is_connected_by_any_provider_it_is_pointed_at_and_a_custom_endpoint_needs_its_url() {
        // DeepSeek is no variable OpenCode reads, but OpenCode is pointed at its endpoint.
        let mut i = ConnectorsInput::default();
        i.keys.keys.push(keyed("DEEPSEEK_API_KEY", "deepseek"));
        assert!(app_signed_in(&i, "opencode"));
        assert!(app_signed_in(&i, "hermes"));
        // Claude Code, Gemini and Codex (it speaks the Responses wire) are not.
        for app in ["claude-code", "gemini-cli", "codex"] {
            assert!(!app_signed_in(&i, app), "{app}");
        }
        // A custom endpoint counts only with a URL and a model.
        let mut c = ConnectorsInput::default();
        c.keys.keys.push(keyed("CUSTOM_ENDPOINT_API_KEY", "custom"));
        assert!(!app_signed_in(&c, "opencode"));
        c.keys.keys[0].fields.insert("base_url".into(), "http://127.0.0.1:1234/v1".into());
        c.keys.keys[0].fields.insert("model".into(), "qwen".into());
        assert!(app_signed_in(&c, "opencode") && app_signed_in(&c, "hermes"));
        let v = view(&c);
        let row = v.providers.iter().find(|p| p.id == "custom").unwrap();
        assert_eq!((row.state.as_str(), row.credential.as_deref()), ("connected", Some("http://127.0.0.1:1234/v1")));
        assert_eq!(row.button.label, "Disconnect");
        assert_eq!(row.sign_out_envs, ["CUSTOM_ENDPOINT_API_KEY"]);
    }

    #[test]
    fn the_sheet_asks_only_for_what_the_method_needs() {
        let i = input();
        // A plan chooser for Z.AI, which bills its plans separately.
        let z = provider_sheet(&i, "zai").unwrap();
        let key = &z.ways[0];
        assert_eq!((key.id.as_str(), key.kind.as_str()), ("key", "api_key"));
        let plan = key.fields.iter().find(|f| f.id == "region").unwrap();
        assert_eq!((plan.label.as_str(), plan.kind.as_str(), plan.value.as_str()), ("Plan", "choice", "global"));
        assert_eq!(plan.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Global", "China", "Global, Coding Plan", "China, Coding Plan"]);
        // A provider with no default model asks for one.
        let d = provider_sheet(&i, "deepinfra").unwrap();
        assert!(d.ways[0].fields.iter().any(|f| f.id == "model" && f.required));
        let o = provider_sheet(&i, "openrouter").unwrap();
        assert!(o.ways[0].fields.is_empty());
        // A custom endpoint: URL, model and API style, a key only if the server asks.
        let c = provider_sheet(&i, "custom").unwrap();
        let w = &c.ways[0];
        assert_eq!((w.id.as_str(), w.kind.as_str(), w.secret.as_str()), ("endpoint", "custom_endpoint", "optional"));
        let ids: Vec<_> = w.fields.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["base_url", "model", "wire"]);
        assert!(w.fields.iter().all(|f| f.required));
        assert_eq!(w.fields[2].value, "openai-chat");
        let note = c.note.as_deref().unwrap();
        assert!(note.contains("only reachable from a Space that is allowed to reach that port"), "{note}");
        // The Anthropic sheet offers the Claude sign-in only where Claude Code is, and says who uses it.
        let a = provider_sheet(&i, "anthropic").unwrap();
        assert_eq!(a.ways.iter().map(|w| w.kind.as_str()).collect::<Vec<_>>(), ["import_login", "import_login", "api_key"]);
        // Hidden providers have no sheet.
        for hidden in ["nous", "xai-oauth", "qwen-oauth", "minimax-oauth", "vertex"] {
            assert!(provider_sheet(&i, hidden).is_none(), "{hidden}");
        }
        // OpenAI offers the ChatGPT sign-in to Codex only.
        let mut j = input();
        j.apps.push(app("codex", "OpenAI Codex", true));
        let for_hermes = provider_sheet_on(&j, "openai", SpaceOs::Macos, Some("hermes")).unwrap();
        assert_eq!(for_hermes.ways.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["key"]);
        let for_codex = provider_sheet_on(&j, "openai", SpaceOs::Macos, Some("codex")).unwrap();
        assert_eq!(for_codex.ways.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["app-device", "key"]);
    }

    #[test]
    fn the_connect_card_for_a_refused_provider_names_it() {
        let i = ConnectorsInput::default();
        let c = setup_card_for(&i, "hermes", "Hermes", Some("openrouter"));
        assert!(c.body.contains("OpenRouter"), "{}", c.body);
        let c = setup_card(&i, "hermes", "Hermes");
        assert!(c.body.contains("a connected provider"), "{}", c.body);
        assert_eq!(c.connect_label, "Connect Hermes");
        // The plan for Hermes asks which provider.
        let plan = connect_plan(&i, "hermes");
        assert_eq!(plan.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn nothing_the_connections_say_has_a_dot_separator_or_says_devices() {
        let mut i = input();
        i.more_open = true;
        let mut all = format!("{:?}", view(&i));
        for p in reg::visible() {
            all += &format!("{:?}", provider_sheet(&i, p.id));
        }
        all += &format!("{:?}", provider_picker(&i, None, "", true));
        assert!(!all.contains('\u{b7}'), "no middle dot");
        // `app-device` is the way's id, not a word anyone reads.
        assert!(!all.replace("app-device", "").to_lowercase().contains("device"), "machines, not devices");
    }
}
