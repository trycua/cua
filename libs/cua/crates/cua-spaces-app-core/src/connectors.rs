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
//! Only official sign-in paths are offered: an app's own token command
//! (`claude setup-token`, which prints a token meant for this use), a key
//! the person pastes, or a key already exported in their shell. Cua never
//! reads another app's private sign-in store for these.
//!
//! Every credential lives where the agent keys do (`agent_keys`: the
//! daemon's Keychain-backed store); this module only decides the words.

use serde::{Deserialize, Serialize};

use crate::agent_keys::{AgentKeyInput, AgentKeyRow, AgentKeysInput};
use crate::model::SpaceOs;

/// How a provider can be connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// An app's own token command, run on This Mac with one browser
    /// approval; the token is saved under `env` and only that app uses it.
    AppToken {
        app: &'static str,
        env: &'static str,
        label: &'static str,
        help: &'static str,
    },
    /// An app's own device-code sign-in, run on This Mac in a folder Cua
    /// owns (Codex: `codex login --device-auth`); the person enters a code
    /// on the provider's page. Saved under `env`; only that app uses it.
    AppDevice {
        app: &'static str,
        env: &'static str,
        label: &'static str,
        help: &'static str,
    },
    /// A key the person pastes.
    Key,
    /// The key already exported in the person's shell, saved once.
    Shell,
}

/// A provider row.
struct Provider {
    id: &'static str,
    label: &'static str,
    /// The variable a pasted key is saved as.
    env: &'static str,
    /// Other variables that also mean this provider is connected (a
    /// sign-in token, a key saved under another name).
    also: &'static [&'static str],
    used_by: &'static str,
    key_help: &'static str,
    placeholder: &'static str,
    ways: &'static [Way],
}

const CLAUDE_TOKEN: Way = Way::AppToken {
    app: "claude-code",
    env: "CLAUDE_CODE_OAUTH_TOKEN",
    label: "Sign in with Claude",
    help: "Opens your browser to approve. Uses your Claude plan. Only Claude Code uses this sign-in.",
};

const CODEX_DEVICE: Way = Way::AppDevice {
    app: "codex",
    env: "CODEX_AUTH_JSON",
    label: "Sign in with ChatGPT",
    help: "Shows a code to enter on openai.com. Uses your ChatGPT plan. Only Codex uses this sign-in.",
};

const PROVIDERS: &[Provider] = &[
    Provider {
        id: "anthropic",
        label: "Anthropic",
        env: "ANTHROPIC_API_KEY",
        also: &["CLAUDE_CODE_OAUTH_TOKEN"],
        used_by: "Claude Code. Hermes and other apps that use Claude need an API key.",
        key_help: "Create a key at console.anthropic.com. Every app that uses Claude can use it.",
        placeholder: "sk-ant-…",
        ways: &[CLAUDE_TOKEN, Way::Key, Way::Shell],
    },
    Provider {
        id: "openai",
        label: "OpenAI",
        env: "OPENAI_API_KEY",
        also: &["CODEX_AUTH_JSON"],
        used_by: "Codex. Hermes and other apps that use OpenAI models need an API key.",
        key_help: "Create a key at platform.openai.com. Every app that uses OpenAI can use it.",
        placeholder: "sk-…",
        ways: &[CODEX_DEVICE, Way::Key, Way::Shell],
    },
    Provider {
        id: "google",
        label: "Google",
        env: "GEMINI_API_KEY",
        also: &["GOOGLE_API_KEY"],
        used_by: "Gemini CLI, Google Antigravity, OpenCode and Pi.",
        key_help: "Create a key at aistudio.google.com/apikey. Every app that uses Gemini can use it.",
        placeholder: "AIza\u{2026}",
        ways: &[Way::Key, Way::Shell],
    },
    Provider {
        id: "openrouter",
        label: "OpenRouter",
        env: "OPENROUTER_API_KEY",
        also: &[],
        used_by: "OpenCode, Goose, Pi, Hermes and OpenClaw.",
        key_help: "Create a key at openrouter.ai/keys. Every app that uses OpenRouter can use it.",
        placeholder: "sk-or-\u{2026}",
        ways: &[Way::Key, Way::Shell],
    },
    Provider {
        id: "github",
        label: "GitHub",
        env: "COPILOT_GITHUB_TOKEN",
        also: &[],
        used_by: "GitHub Copilot CLI.",
        key_help: "Create a fine-grained token with the Copilot Requests permission at github.com/settings/tokens.",
        placeholder: "github_pat_…",
        ways: &[Way::Key, Way::Shell],
    },
];

/// Which providers an app signs in to, and the way its Connect uses.
struct AppSignIn {
    app: &'static str,
    /// Any one of these connected is enough.
    providers: &'static [&'static str],
    /// The way Connect starts when none is connected (`None`: it asks with
    /// the provider's sheet).
    way: Option<Way>,
    /// The saved credentials the app can read, as the variables a run gets
    /// (the harness's `keys`; a drift test holds them equal). Empty: the app
    /// takes any credential of its providers.
    reads: &'static [&'static str],
}

const APPS: &[AppSignIn] = &[
    AppSignIn {
        app: "claude-code",
        providers: &["anthropic"],
        way: Some(CLAUDE_TOKEN),
        reads: &[
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_AUTH_TOKEN",
        ],
    },
    AppSignIn {
        app: "codex",
        providers: &["openai"],
        way: Some(CODEX_DEVICE),
        reads: &["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_AUTH_JSON"],
    },
    AppSignIn {
        app: "copilot-cli",
        providers: &["github"],
        way: None,
        reads: &[],
    },
    AppSignIn {
        app: "gemini-cli",
        providers: &["google"],
        way: None,
        reads: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
    },
    AppSignIn {
        app: "antigravity",
        providers: &["google"],
        way: None,
        reads: &["GEMINI_API_KEY"],
    },
    AppSignIn {
        app: "opencode",
        providers: &["anthropic", "openai", "google", "openrouter"],
        way: None,
        reads: &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENROUTER_API_KEY",
        ],
    },
    AppSignIn {
        app: "goose",
        // Not Google: it reads GOOGLE_API_KEY, which the Google row does not save.
        providers: &["openai", "anthropic", "openrouter"],
        way: None,
        reads: &[
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GOOGLE_API_KEY",
            "OPENROUTER_API_KEY",
        ],
    },
    AppSignIn {
        app: "pi",
        providers: &["anthropic", "openai", "google", "openrouter"],
        way: None,
        reads: &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENROUTER_API_KEY",
        ],
    },
    AppSignIn {
        app: "openclaw",
        providers: &["anthropic", "openai", "openrouter"],
        way: None,
        reads: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"],
    },
    AppSignIn {
        app: "hermes",
        providers: &["anthropic", "openai"],
        way: None,
        // API keys only: the Claude Code and ChatGPT sign-ins are not Hermes'.
        reads: &["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
    },
    AppSignIn {
        app: "t3code",
        providers: &["anthropic", "openai"],
        way: None,
        reads: &[],
    },
];

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

/// `names` as "A", "A or B", "A, B or C".
pub fn or_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

fn app_sign_in(app: &str) -> Option<&'static AppSignIn> {
    APPS.iter().find(|a| a.app == app)
}

/// Every variable the providers own (they are not listed under Other keys).
fn provider_env(env: &str) -> bool {
    PROVIDERS
        .iter()
        .any(|p| p.env == env || p.also.contains(&env))
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
    /// `connecting` (tools), `signing-in` (a browser approval), `signing-out`.
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
fn provider_credential<'a>(input: &'a ConnectorsInput, p: &Provider) -> Option<&'a AgentKeyInput> {
    input.key(p.env).filter(|k| !k.expired).or_else(|| {
        p.also
            .iter()
            .find_map(|e| input.key(e).filter(|k| !k.expired))
    })
}

/// A sign-in saved for the provider has expired (and none that works is).
fn provider_expired(input: &ConnectorsInput, p: &Provider) -> bool {
    provider_credential(input, p).is_none()
        && std::iter::once(p.env)
            .chain(p.also.iter().copied())
            .any(|e| input.key(e).is_some_and(|k| k.expired))
}

fn provider_connected(input: &ConnectorsInput, id: &str) -> bool {
    provider(id).is_some_and(|p| provider_credential(input, p).is_some())
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
        return a.providers.iter().any(|p| provider_connected(input, p));
    }
    a.reads
        .iter()
        .any(|k| input.key(k).is_some_and(|k| !k.expired))
}

/// An app's sign-in is saved but expired.
fn app_expired(input: &ConnectorsInput, app: &str) -> bool {
    app_sign_in(app).is_some_and(|a| {
        a.providers
            .iter()
            .filter_map(|p| provider(p))
            .any(|p| provider_expired(input, p))
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
fn credential_text(p: &Provider, k: &AgentKeyInput) -> String {
    match k.env.as_str() {
        "CLAUDE_CODE_OAUTH_TOKEN" => "Claude Code sign-in".into(),
        "CODEX_AUTH_JSON" => "ChatGPT sign-in, for Codex".into(),
        e if e == p.env => format!("API key {}", dots(k)),
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

/// A provider's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorProviderRow {
    pub id: String,
    pub label: String,
    /// `connected`, `not-connected`, `connecting`, `failed`.
    pub state: String,
    /// "Not connected", "API key •••• 3f9a", "Claude Code sign-in",
    /// "Approve in your browser…".
    pub status: String,
    /// Who uses it.
    pub detail: String,
    /// Connect, or Sign out.
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
    pub providers_title: String,
    /// Under Providers: where credentials stay.
    pub providers_footer: String,
    pub providers: Vec<ConnectorProviderRow>,
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
        let names = or_list(
            &s.providers
                .iter()
                .filter_map(|p| provider(p))
                .map(|p| p.label)
                .collect::<Vec<_>>(),
        );
        // An app with no sign-in of its own takes API keys: say so.
        if s.way.is_none() && !s.reads.is_empty() {
            format!("Needs an API key for {names}.")
        } else {
            format!("Needs a sign-in to {names}.")
        }
    });

    let (state, line, btn) = if let Some(step) = busy {
        let line = match step {
            "signing-in"
                if app_sign_in(&a.agent)
                    .is_some_and(|s| matches!(s.way, Some(Way::AppDevice { .. }))) =>
            {
                "Enter the code in your browser…".to_string()
            }
            "signing-in" => "Approve in your browser…".to_string(),
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
            (!a.installed && !a.configured).then(|| "Runs in your Spaces".to_string()),
            None,
        )
    } else if !a.installed && !a.configured {
        (
            "not-connected",
            if app_expired(input, &a.agent) {
                Some("The sign-in expired. Reconnect.".to_string())
            } else {
                needs
            },
            Some(button("connect", "Connect", true)),
        )
    } else if a.configured {
        (
            "not-connected",
            if app_expired(input, &a.agent) {
                Some("The sign-in expired. Reconnect.".to_string())
            } else {
                needs
            },
            Some(button("connect", "Connect", true)),
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

fn provider_row(input: &ConnectorsInput, p: &Provider, can_edit: bool) -> ConnectorProviderRow {
    let id = format!("provider:{}", p.id);
    let busy = input.busy(&id);
    let cred = provider_credential(input, p);
    let (state, status, btn) = if let Some(step) = busy {
        let status = match step {
            "signing-out" => "Signing out…",
            "signing-in" if p.ways.iter().any(|w| matches!(w, Way::AppDevice { .. })) => {
                "Enter the code in your browser…"
            }
            "signing-in" => "Approve in your browser…",
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
    } else if let Some(k) = cred {
        (
            "connected",
            credential_text(p, k),
            button("sign-out", "Sign out", can_edit),
        )
    } else if provider_expired(input, p) {
        (
            "not-connected",
            "Expired, reconnect".into(),
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
        label: p.label.into(),
        state: state.into(),
        status,
        detail: format!("For {}", p.used_by),
        button: btn,
        added_ms: cred.filter(|_| busy.is_none()).map(|k| k.added_ms),
        sign_out_envs: std::iter::once(p.env)
            .chain(p.also.iter().copied())
            .filter(|e| input.key(e).is_some())
            .map(Into::into)
            .collect(),
    }
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
        providers_title: "Providers".into(),
        providers_footer: format!(
            "Sign-ins and keys stay on {this}{place}. Each one is only given to the apps that use it."
        ),
        providers: PROVIDERS
            .iter()
            .map(|p| provider_row(input, p, can_edit))
            .collect(),
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
        add_other_label: "Add another key".into(),
        other_help: "For another provider, save its key under the variable its app reads, like \
                     GEMINI_API_KEY."
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
    /// Run the app's own token command (`agents.connect` with `signIn`): one
    /// browser approval.
    pub app_token: bool,
    /// Run the app's own device-code sign-in (`agents.signInStart`, then
    /// `agents.signInWait`): a code to show, entered on the provider's page.
    pub device: bool,
    /// Then open this provider's Connect sheet (the app has no token
    /// command of its own).
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
    // The app's own token command runs on This Mac: only where the app is.
    let app_token = !signed_in
        && app.is_some()
        && s.is_some_and(|s| matches!(s.way, Some(Way::AppToken { .. })));
    let device = !signed_in
        && app.is_some()
        && s.is_some_and(|s| matches!(s.way, Some(Way::AppDevice { .. })));
    let provider = if signed_in || app_token || device {
        None
    } else {
        s.and_then(|s| s.providers.first())
            .map(|p| (*p).to_string())
    };
    ConnectPlan {
        // Cua's tools go into the app where it is on this computer.
        tools: app.is_some() && !configured,
        app_token,
        device,
        provider,
        token_provider: if app_token || device {
            s.and_then(|s| s.providers.first())
                .map(|p| (*p).to_string())
        } else {
            None
        },
    }
}

/// The provider ids `agent` signs in to (any one is enough; empty: it signs in on its own).
pub fn app_providers(agent: &str) -> Vec<String> {
    app_sign_in(agent)
        .map(|s| s.providers.iter().map(|p| (*p).to_string()).collect())
        .unwrap_or_default()
}

/// Whether `agent` has a sign-in it can use in a Space now.
pub fn signed_in(input: &ConnectorsInput, agent: &str) -> bool {
    app_signed_in(input, agent)
}

/// A provider's name ("Anthropic").
pub fn provider_label(id: &str) -> Option<&'static str> {
    provider(id).map(|p| p.label)
}

/// One way to connect a provider, as its sheet shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderWay {
    /// `app-token`, `key`, `shell`.
    pub id: String,
    pub label: String,
    pub help: String,
    pub button: String,
    /// For `app-token`: the app whose command runs (`agents.connect` with `signIn`).
    pub app: Option<String>,
    /// For `shell`: the variable read from the shell.
    pub env: Option<String>,
}

/// A provider's Connect sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSheetView {
    pub provider: String,
    pub title: String,
    pub lede: String,
    pub ways: Vec<ProviderWay>,
    pub cancel_label: String,
    /// The key field, when the person picks Paste a key.
    pub key_label: String,
    pub key_placeholder: String,
    pub key_help: String,
    pub save_label: String,
    /// What `agentKeys.set` gets with a pasted key: `anthropic`, `openai`, or `other` with `key_env`.
    pub key_provider: String,
    pub key_env: Option<String>,
    /// "Or choose another provider", when the sheet was opened for an app
    /// that signs in with more than one.
    pub others_label: Option<String>,
    /// The app's other providers, each one a choice that swaps this sheet.
    pub others: Vec<ProviderChoice>,
}

/// One provider an app can sign in with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChoice {
    pub provider: String,
    pub label: String,
}

/// The sheet for `provider` (None: an unknown provider).
pub fn provider_sheet(input: &ConnectorsInput, provider_id: &str) -> Option<ProviderSheetView> {
    provider_sheet_on(input, provider_id, SpaceOs::Macos, None)
}

/// [`provider_sheet`] in a shell on `os`, opened for `for_app` (its other
/// providers are offered as a choice).
pub fn provider_sheet_on(
    input: &ConnectorsInput,
    provider_id: &str,
    os: SpaceOs,
    for_app: Option<&str>,
) -> Option<ProviderSheetView> {
    let p = provider(provider_id)?;
    let installed = |app: &str| input.app(app).is_some_and(|a| a.installed);
    let ways = p
        .ways
        .iter()
        .filter_map(|w| match *w {
            // An app's own sign-in is that app's alone: another app's sheet offers a key.
            Way::AppToken {
                app, label, help, ..
            } => (installed(app) && for_app.is_none_or(|a| a == app)).then(|| ProviderWay {
                id: "app-token".into(),
                label: label.into(),
                help: help.into(),
                button: "Sign in".into(),
                app: Some(app.into()),
                env: None,
            }),
            Way::AppDevice {
                app, label, help, ..
            } => (installed(app) && for_app.is_none_or(|a| a == app)).then(|| ProviderWay {
                id: "app-device".into(),
                label: label.into(),
                help: help.into(),
                button: "Sign in".into(),
                app: Some(app.into()),
                env: None,
            }),
            Way::Key => Some(ProviderWay {
                id: "key".into(),
                label: "Paste an API key".into(),
                help: p.key_help.into(),
                button: "Paste key".into(),
                app: None,
                env: None,
            }),
            Way::Shell => input
                .shell_keys
                .iter()
                .any(|k| k == p.env)
                .then(|| ProviderWay {
                    id: "shell".into(),
                    label: format!("Use {} from your shell", p.env),
                    help: "Cua saves a copy now. It doesn't follow later changes in your shell."
                        .into(),
                    button: "Use it".into(),
                    app: None,
                    env: Some(p.env.into()),
                }),
        })
        .collect();
    let others: Vec<ProviderChoice> = for_app
        .and_then(app_sign_in)
        .map(|a| {
            a.providers
                .iter()
                .filter(|id| **id != p.id)
                .filter_map(|id| provider(id))
                .map(|o| ProviderChoice {
                    provider: o.id.into(),
                    label: o.label.into(),
                })
                .collect()
        })
        .unwrap_or_default();
    let this = os.this_machine_lower();
    let place = vault(os).map(|v| format!("in {v} ")).unwrap_or_default();
    Some(ProviderSheetView {
        provider: p.id.into(),
        title: format!("Connect {}", p.label),
        lede: format!("For {}", p.used_by),
        ways,
        cancel_label: "Cancel".into(),
        key_label: "API key".into(),
        key_placeholder: p.placeholder.into(),
        key_help: format!("It's saved {place}on {this} and won't be shown again."),
        save_label: "Save".into(),
        key_provider: if matches!(p.id, "anthropic" | "openai") {
            p.id
        } else {
            "other"
        }
        .into(),
        key_env: (!matches!(p.id, "anthropic" | "openai")).then(|| p.env.into()),
        others_label: (!others.is_empty()).then(|| "Or choose another provider".into()),
        others,
    })
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
    /// How Connect signs it in ("Claude sign-in, through claude setup-token").
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
            "Connect runs claude setup-token: one browser approval, using your Claude plan. Or connect Anthropic with an API key."
        }
        "codex" => {
            "Connect signs Codex in with ChatGPT: a code to enter on openai.com, using your ChatGPT plan. Or connect OpenAI with an API key."
        }
        "copilot-cli" => "Connect asks for a GitHub token with the Copilot Requests permission.",
        "hermes" | "opencode" | "goose" | "pi" | "openclaw" => {
            "It runs in your Spaces with a key from any of these providers."
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
        let pid = s
            .providers
            .iter()
            .find(|p| provider_connected(input, p))
            .or(s.providers.first())
            .copied()
            .unwrap_or_default();
        let p = provider(pid);
        let prow = p.map(|p| provider_row(input, p, can_edit));
        let connected = prow.as_ref().is_some_and(|r| r.state == "connected");
        let names: Vec<&str> = s
            .providers
            .iter()
            .filter_map(|p| provider(p))
            .map(|p| p.label)
            .collect();
        ConnectorSignIn {
            title: format!("Signs in with {}", or_list(&names)),
            status: prow.as_ref().map(|r| r.status.clone()).unwrap_or_default(),
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
        back_label: "Connectors".into(),
        status: match row.state.as_str() {
            "connected" => "Connected".into(),
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
pub fn setup_card(input: &ConnectorsInput, agent: &str, name: &str) -> SetupCardView {
    let plan = connect_plan(input, agent);
    let provider = app_sign_in(agent)
        .and_then(|s| s.providers.first())
        .and_then(|p| provider(p))
        .map(|p| p.label);
    let body = match provider {
        Some(p) if plan.app_token => format!(
            "{name} needs a sign-in to {p} to work here. Connect it once: approve in your browser, and every Space can use it."
        ),
        Some(p) => format!(
            "{name} needs a sign-in to {p} to work here. Connect it once and every Space can use it."
        ),
        None => format!("Connect {name} once and every Space can use it."),
    };
    SetupCardView {
        title: format!("Connect {name} to start"),
        body,
        connect_label: format!("Connect {name}"),
        connecting_label: if plan.app_token {
            "Approve in your browser…".into()
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
        // Each provider an app offers saves a credential the app reads, so
        // connecting it connects the app.
        for h in HARNESS_APPS {
            let a = app_sign_in(h.app).unwrap();
            for id in a.providers {
                let p = provider(id).unwrap();
                let saves_a_read = a.reads.contains(&p.env)
                    || p.ways.iter().any(|w| match w {
                        Way::AppToken { env, .. } | Way::AppDevice { env, .. } => {
                            a.reads.contains(env)
                        }
                        _ => false,
                    });
                assert!(saves_a_read, "{}: {id} saves nothing it reads", h.app);
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
            Some("Needs an API key for Anthropic or OpenAI.")
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
        let s = provider_sheet_on(&i, "anthropic", SpaceOs::Macos, Some("opencode")).unwrap();
        let others: Vec<_> = s.others.iter().map(|o| o.provider.as_str()).collect();
        assert_eq!(others, ["openai", "google", "openrouter"]);
        assert_eq!(
            s.others_label.as_deref(),
            Some("Or choose another provider")
        );
        // OpenRouter is a key, saved under the variable its apps read.
        let o = provider_sheet_on(&i, "openrouter", SpaceOs::Macos, Some("hermes")).unwrap();
        assert_eq!(
            (o.key_provider.as_str(), o.key_env.as_deref()),
            ("other", Some("OPENROUTER_API_KEY"))
        );
        let ids: Vec<_> = o.ways.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["key"]);
        // Without an app there is no choice to offer.
        assert!(provider_sheet(&i, "anthropic").unwrap().others.is_empty());
    }

    #[test]
    fn a_harness_off_this_machine_has_a_page_with_its_sign_in_and_no_local_tools() {
        let d = app_detail(&ConnectorsInput::default(), "goose").unwrap();
        assert_eq!(d.name, "Goose");
        assert!(d.toggles.is_empty() && d.tools_title.is_empty());
        assert_eq!(
            d.sign_in.as_ref().unwrap().title,
            "Signs in with OpenAI, Anthropic or OpenRouter"
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
    fn an_expired_sign_in_is_not_connected_and_says_reconnect() {
        let mut i = input();
        i.apps[0].configured = true;
        let mut k = key("CLAUDE_CODE_OAUTH_TOKEN", "AAAA");
        k.expired = true;
        i.keys.keys.push(k);
        assert!(!app_signed_in(&i, "claude-code"));
        let v = view(&i);
        let p = v.providers.iter().find(|p| p.id == "anthropic").unwrap();
        assert_eq!(p.state, "not-connected");
        assert_eq!(p.status, "Expired, reconnect");
        assert_eq!(p.button.label, "Reconnect");
        let a = v.apps.iter().find(|a| a.agent == "claude-code").unwrap();
        assert_eq!(a.line.as_deref(), Some("The sign-in expired. Reconnect."));
    }

    #[test]
    fn apps_start_with_connect_and_providers_not_connected() {
        let v = view(&input());
        assert_eq!(v.apps_title, "Coding apps");
        assert_eq!(v.providers_title, "Providers");
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
            ["anthropic", "openai", "google", "openrouter", "github"]
        );
        assert!(
            v.providers
                .iter()
                .all(|p| p.status == "Not connected" && p.button.label == "Connect")
        );
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
        assert_eq!(a.status, "Claude Code sign-in");
        assert_eq!(a.button.label, "Sign out");
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
        assert_eq!(
            v.providers[1].status,
            "API key \u{2022}\u{2022}\u{2022}\u{2022} 3f9a"
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
        assert_eq!(v.apps[0].line.as_deref(), Some("Approve in your browser…"));
        assert!(!v.apps[0].button.as_ref().unwrap().enabled);
        assert_eq!(v.providers[0].state, "connecting");
        assert_eq!(v.providers[0].status, "Approve in your browser…");
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
        assert_eq!(ids, ["app-token", "key", "shell"]);
        assert_eq!(s.ways[0].app.as_deref(), Some("claude-code"));
        assert_eq!(s.ways[2].label, "Use ANTHROPIC_API_KEY from your shell");
        // Without Claude Code there is no Claude sign-in to offer; without the
        // variable, no shell row.
        i.apps.retain(|a| a.agent != "claude-code");
        i.shell_keys.clear();
        let s = provider_sheet(&i, "anthropic").unwrap();
        let ids: Vec<_> = s.ways.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["key"]);
        assert_eq!((s.key_provider.as_str(), s.key_env), ("anthropic", None));
        let g = provider_sheet(&i, "github").unwrap();
        assert_eq!(
            (g.key_provider.as_str(), g.key_env.as_deref()),
            ("other", Some("COPILOT_GITHUB_TOKEN"))
        );
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

    #[test]
    fn setup_card_names_the_app() {
        let i = input();
        let c = setup_card(&i, "claude-code", "Claude Code");
        assert_eq!(c.title, "Connect Claude Code to start");
        assert_eq!(c.connect_label, "Connect Claude Code");
        assert_eq!(c.connecting_label, "Approve in your browser…");
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
}
