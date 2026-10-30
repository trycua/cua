//! The provider registry behind Settings, Connections and `cua connections`.
//!
//! One TOML file per provider lives in `providers/`: its id, name, wire, base
//! URL, regions, the ways to connect it (methods, from a closed set of kinds),
//! the environment variables each app receives, and a terms flag that keeps a
//! subscription login out of every UI until it has been reviewed. The
//! generator (`cargo run -p cua-connections --bin cua-connections-gen`)
//! turns the files into [`generated`] (the tables everything links), the
//! parity data the web and SwiftUI apps test against, the CLI help and the
//! provider table in the docs; `--check` fails when any of them is stale.
//!
//! Nothing here touches a credential. [`deliver`] decides which saved
//! variables reach a run of an app (one place, tested for every provider and
//! every app), [`works_with`] computes the "works with" chips, and
//! [`hermes::config_yaml`] writes the config Hermes reads so that the
//! connected provider is the one it uses.

pub mod delivery;
pub mod generate;
pub mod hermes;
pub mod schema;

#[rustfmt::skip]
mod generated;

use std::collections::BTreeMap;

pub use delivery::{deliver, endpoint_for, works_with};
pub use generated::*;

/// Every agent harness a run can use, in the order the Connections page lists
/// them. `cua-agents` holds a test that this equals its own table.
pub const HARNESS_IDS: &[&str] = &[
    "claude-code",
    "openai-codex",
    "gemini-cli",
    "google-antigravity",
    "opencode",
    "goose",
    "pi",
    "hermes",
    "openclaw",
];

/// Harnesses that can be pointed at any endpoint of one wire with a base URL
/// and a model (their launcher writes the config), with that wire. A provider
/// that serves the wire and names a default model works with these without a
/// native key of its own.
pub const ENDPOINT_HARNESSES: &[(&str, &str)] = &[
    ("opencode", "openai-chat"),
    ("goose", "openai-chat"),
    ("pi", "openai-chat"),
    ("openclaw", "openai-chat"),
    ("openai-codex", "openai-responses"),
];

/// The wires a provider can serve (closed set).
pub const WIRES: &[&str] = &[
    "anthropic",
    "openai-responses",
    "openai-chat",
    "gemini",
    "bedrock",
];

/// Where a provider shows up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Up front on the Connections page.
    Top,
    /// In the searchable "More providers" list.
    More,
}

/// What kind of service a provider is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hosted,
    Aggregator,
    Local,
    Custom,
    Cloud,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Hosted => "hosted",
            Kind::Aggregator => "aggregator",
            Kind::Local => "local",
            Kind::Custom => "custom",
            Kind::Cloud => "cloud",
        }
    }
}

/// The ways to connect a provider: a closed set, each implemented once in
/// core. The UIs branch on the kind, never on a provider id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    /// A key the person pastes (or reads from their shell).
    ApiKey,
    /// The sign-in another app made on this machine, copied with consent
    /// (access token only, never refreshed by Cua).
    ImportLogin,
    /// A code entered on the provider's page.
    DeviceCode,
    /// A browser approval that returns to this machine.
    BrowserOauth,
    /// A plan sign-in whose token belongs to one app.
    SubscriptionLogin,
    /// The cloud credential chain on this machine (Google, AWS, Entra).
    Adc,
    /// A server on a machine of the person's, by URL.
    LocalServer,
    /// Any OpenAI-compatible base URL with an optional key.
    CustomEndpoint,
}

impl MethodKind {
    /// The word used in files, telemetry and the view model.
    pub fn word(self) -> &'static str {
        match self {
            MethodKind::ApiKey => "api_key",
            MethodKind::ImportLogin => "import_login",
            MethodKind::DeviceCode => "device_code",
            MethodKind::BrowserOauth => "browser_oauth",
            MethodKind::SubscriptionLogin => "subscription_login",
            MethodKind::Adc => "adc",
            MethodKind::LocalServer => "local_server",
            MethodKind::CustomEndpoint => "custom_endpoint",
        }
    }

    /// Every kind, in the order of [`MethodKind::word`]'s list in the docs.
    pub const ALL: [MethodKind; 8] = [
        MethodKind::ApiKey,
        MethodKind::ImportLogin,
        MethodKind::DeviceCode,
        MethodKind::BrowserOauth,
        MethodKind::SubscriptionLogin,
        MethodKind::Adc,
        MethodKind::LocalServer,
        MethodKind::CustomEndpoint,
    ];

    pub fn parse(word: &str) -> Option<MethodKind> {
        Self::ALL.into_iter().find(|k| k.word() == word)
    }
}

/// How a saved key is checked, without blocking: one read-only GET on the
/// provider's own host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    /// Appended to the provider's base URL (or the region's).
    pub path: &'static str,
    /// `bearer`, `x-api-key` (with the Anthropic version header) or
    /// `x-goog-api-key`.
    pub auth: &'static str,
}

/// A non-secret value a method asks for besides the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub id: &'static str,
    pub label: &'static str,
    /// `text`, `url` or `choice`.
    pub kind: &'static str,
    pub placeholder: &'static str,
    pub required: bool,
    pub options: &'static [&'static str],
}

/// One way to connect a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method {
    pub id: &'static str,
    pub kind: MethodKind,
    /// The variable the credential is saved as (empty: none).
    pub env: &'static str,
    /// Other saved names that also mean this provider is connected.
    pub aliases: &'static [&'static str],
    /// The one app that may receive this credential (a harness id), or empty.
    pub scope: &'static str,
    /// The setup id of the app the sign-in is made through, or empty.
    pub app: &'static str,
    /// `device_code` for a code sign-in, or empty.
    pub flow: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    /// Subscription logins whose terms are unchecked: never offered.
    pub terms_review: bool,
    /// The key may be empty (a server that asks for none).
    pub secret_optional: bool,
    pub fields: &'static [Field],
}

/// One model a provider serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
    /// A quick, cheap model (the picker's default is the first of these).
    pub fast: bool,
}

/// A region (or plan) with its own base URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
}

/// One name delivered to a run: `dest` is the variable the app reads, `src`
/// the saved name it is filled from (usually the same).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub dest: &'static str,
    pub src: &'static str,
}

/// What one app receives from a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub harness: &'static str,
    pub entries: &'static [Entry],
}

/// One provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: &'static str,
    pub tier: Tier,
    /// Preference order: Hermes uses the first connected provider.
    pub rank: u32,
    pub kind: Kind,
    /// Kept out of every UI (`hidden_reason` says why).
    pub hidden: bool,
    pub hidden_reason: &'static str,
    /// The provider's subscription terms are unchecked: out of every UI.
    pub terms_review: bool,
    pub aliases: &'static [&'static str],
    pub wire: &'static str,
    pub also_wires: &'static [&'static str],
    pub base_url: &'static str,
    /// The variable Hermes reads to override the base URL.
    pub base_url_env: &'static str,
    pub default_model: &'static str,
    /// Hermes's provider id, when it has one.
    pub hermes_id: &'static str,
    pub key_help: &'static str,
    pub placeholder: &'static str,
    pub check: Option<Check>,
    /// Apps that are not agent harnesses but read this provider's variable.
    pub apps: &'static [&'static str],
    pub methods: &'static [Method],
    pub regions: &'static [Region],
    /// The models a person can pick, best known first (may be empty: then
    /// `default_model` is the only one).
    pub models: &'static [Model],
    pub delivery: &'static [Delivery],
}

impl Provider {
    /// Whether any UI may show it.
    pub fn visible(&self) -> bool {
        !self.hidden && !self.terms_review
    }

    /// The methods a UI may offer (terms-reviewed only).
    pub fn offered(&self) -> impl Iterator<Item = &'static Method> + '_ {
        self.methods.iter().filter(|m| !m.terms_review)
    }

    /// The API-key method, when there is one.
    pub fn key_method(&self) -> Option<&'static Method> {
        self.methods
            .iter()
            .find(|m| matches!(m.kind, MethodKind::ApiKey | MethodKind::CustomEndpoint))
    }

    /// Every saved name that connects this provider.
    pub fn envs(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.methods
            .iter()
            .filter(|m| !m.terms_review)
            .flat_map(|m| std::iter::once(m.env).chain(m.aliases.iter().copied()))
            .filter(|e| !e.is_empty())
    }

    /// What `harness` receives from this provider.
    pub fn delivery_for(&self, harness: &str) -> &'static [Entry] {
        self.delivery
            .iter()
            .find(|d| d.harness == harness)
            .map(|d| d.entries)
            .unwrap_or(&[])
    }

    /// The models a person can pick: the listed ones, or the default alone.
    pub fn model_list(&self) -> Vec<Model> {
        if !self.models.is_empty() {
            return self.models.to_vec();
        }
        if self.default_model.is_empty() {
            return vec![];
        }
        vec![Model {
            id: self.default_model,
            name: self.default_model,
            fast: false,
        }]
    }

    /// The base URL for `region` (the provider's own when empty or unknown).
    pub fn base_url_for(&self, region: Option<&str>) -> &'static str {
        region
            .and_then(|r| self.regions.iter().find(|x| x.id == r))
            .map(|r| r.base_url)
            .unwrap_or(self.base_url)
    }

    /// Whether `env` is a name only a scoped method saves: the app that may
    /// receive it, when so.
    pub fn scope_of(&self, env: &str) -> Option<&'static str> {
        self.methods
            .iter()
            .find(|m| !m.scope.is_empty() && (m.env == env || m.aliases.contains(&env)))
            .map(|m| m.scope)
    }
}

/// A provider by id (hidden ones too).
pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// The providers a UI may show, in rank order.
pub fn visible() -> impl Iterator<Item = &'static Provider> {
    PROVIDERS.iter().filter(|p| p.visible())
}

/// The visible provider that saves a credential under `env`.
pub fn provider_of_env(env: &str) -> Option<&'static Provider> {
    visible().find(|p| p.envs().any(|e| e == env))
}

/// The names `harness` reads, in preference order (the generated table).
pub fn reads(harness: &str) -> &'static [&'static str] {
    READS
        .iter()
        .find(|(h, _)| *h == harness)
        .map(|(_, r)| *r)
        .unwrap_or(&[])
}

/// Every name any visible provider may be saved or delivered as.
pub fn all_env_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = visible().flat_map(|p| p.envs()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Names that may be read from the person's login shell: the API-key names
/// of visible providers, nothing else.
pub fn shell_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = visible()
        .flat_map(|p| p.methods.iter())
        .filter(|m| m.kind == MethodKind::ApiKey && !m.terms_review)
        .map(|m| m.env)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// What a connected provider contributes to a run's selection.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Connected {
    pub provider: String,
    /// The region or plan chosen, when the provider has them.
    pub region: Option<String>,
    /// Non-secret fields saved with it (`base_url`, `model`, `wire`).
    pub fields: BTreeMap<String, String>,
}
