// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Teleport an app...": pick an app, choose what moves, plan, review the
//! consent list (every install, path and secret) and acknowledge secrets,
//! run, done. A pure reducer; the shell runs the SDK calls (`Teleport.catalog`,
//! `plan`, `run`) and feeds their results back as events.
//!
//! Records mirror the cua SDK's teleport records in the camelCase shape the
//! webview uses; `json` carries the SDK's own JSON for the entry or plan so
//! the shell can hand it back to `plan` / `run` untouched.

use super::review::{self, ReviewChoice, ReviewDomain, ReviewToggle, SendSource, VaultSource};
use crate::keyvault::wire::KvInventory;
use crate::util::{contains_word, to_fixed};
use serde::{Deserialize, Serialize};

/// Most run events kept.
pub const MAX_EVENTS: usize = 200;

/// What teleport can do with an app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// App and signed-in state.
    Full,
    /// App, empty or with files.
    InstallOnly,
    /// Not available.
    Unsupported,
}

impl Capability {
    /// Label.
    pub fn label(self) -> &'static str {
        match self {
            Capability::Full => "App and signed-in state",
            Capability::InstallOnly => "App, empty or with files",
            Capability::Unsupported => "Not available",
        }
    }
}

/// What a teleport moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Move {
    /// Just the app.
    AppOnly,
    /// The app with files or folders.
    AppWithFiles,
    /// The app with its signed-in state.
    AppWithState,
}

impl Move {
    /// Label.
    pub fn label(self) -> &'static str {
        match self {
            Move::AppOnly => "Just the app",
            Move::AppWithFiles => "The app with files or folders",
            Move::AppWithState => "The app with its signed-in state",
        }
    }
}

/// Credential-shaped state the signed-in state move leaves out by default,
/// opted into one group at a time (`cua_teleport::ux::SensitiveGroup`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveGroup {
    /// The session cookies: what keeps the app signed in.
    SignIns,
    /// Saved passwords.
    Passwords,
    /// Browsing history.
    History,
}

impl SensitiveGroup {
    /// The checkbox label.
    pub fn label(self) -> &'static str {
        match self {
            SensitiveGroup::SignIns => "Keep me signed in",
            SensitiveGroup::Passwords => "Saved passwords",
            SensitiveGroup::History => "Browsing history",
        }
    }

    /// The line under it.
    pub fn detail(self) -> &'static str {
        match self {
            SensitiveGroup::SignIns => "Sends the session cookies.",
            SensitiveGroup::Passwords => "Sends the passwords saved in the app.",
            SensitiveGroup::History => {
                "Sends the history of pages visited, and the bookmarks where the browser keeps them together."
            }
        }
    }
}

/// One opt-in checkbox of the options step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SensitiveOption {
    /// The group.
    pub group: SensitiveGroup,
    /// Label.
    pub label: String,
    /// The line under the label.
    pub detail: String,
    /// Checked.
    pub checked: bool,
}

/// One app row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    /// Id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Bundle path.
    #[serde(default)]
    pub host_path: Option<String>,
    /// Bundle id.
    #[serde(default)]
    pub host_app_id: Option<String>,
    /// Version.
    #[serde(default)]
    pub version: Option<String>,
    /// Capability.
    pub capability: Capability,
    /// Why unsupported, or a caveat.
    #[serde(default)]
    pub reason: Option<String>,
    /// Offered moves, UI order.
    pub moves: Vec<Move>,
    /// Provider id.
    #[serde(default)]
    pub provider_id: Option<String>,
    /// The opt-in groups the signed-in state move offers, UI order.
    #[serde(default)]
    pub sensitive_groups: Vec<SensitiveGroup>,
    /// `manifest`, `image`, `space`.
    #[serde(default)]
    pub install_source: Option<String>,
    /// Installable id.
    #[serde(default)]
    pub install_id: Option<String>,
    /// Installable version.
    #[serde(default)]
    pub install_version: Option<String>,
    /// Binary.
    #[serde(default)]
    pub launch_bin: Option<String>,
    /// Last used (recents).
    #[serde(default)]
    pub last_used_ms: Option<i64>,
    /// The SDK's JSON for this entry.
    pub json: String,
}

/// What a consent line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentKind {
    /// An install into the Space.
    Install,
    /// A file that leaves this machine.
    File,
    /// A folder that leaves this machine.
    Folder,
    /// App state that leaves this machine.
    State,
    /// A secret that leaves this machine.
    Secret,
}

/// One line of the consent review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentItem {
    /// Kind.
    pub kind: ConsentKind,
    /// Stable key.
    pub key: String,
    /// Label.
    pub label: String,
    /// Detail.
    pub detail: String,
    /// Bytes that leave.
    pub bytes: u64,
    /// Credentials, cookies, tokens.
    pub sensitive: bool,
}

/// One step of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStepView {
    /// `install`, `files`, `state`, `launch`.
    pub kind: String,
    /// One line.
    pub summary: String,
}

/// Exactly what a teleport will do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// The app.
    pub app: CatalogEntry,
    /// Target Space.
    pub space_id: String,
    /// What moves.
    pub moves: Move,
    /// Steps.
    pub steps: Vec<PlanStepView>,
    /// Consent lines.
    pub consent: Vec<ConsentItem>,
    /// Any line is a secret.
    pub sensitive: bool,
    /// Bytes that leave this machine.
    pub total_bytes: u64,
    /// Caveats.
    pub warnings: Vec<String>,
    /// This Space is reached through a relay connection that predates
    /// end-to-end sealing: a secret this plan sends would cross it in the
    /// clear (S1). Needs `acknowledgeRelayPlaintext` the same way a secret
    /// needs `acknowledgeSensitive`. Defaults to `false`: older fixtures
    /// and a plan from a relay that predates the capability both read as
    /// sealed (never retroactively flagged).
    #[serde(default)]
    pub relay_unsealed: bool,
    /// The SDK's JSON for this plan.
    pub json: String,
}

/// Run phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunPhase {
    /// A step started.
    Started,
    /// Bytes moved.
    Progress,
    /// A step finished.
    Finished,
    /// A step failed.
    Failed,
    /// All done.
    Done,
}

/// One run event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEvent {
    /// Step index.
    pub step: u32,
    /// Steps.
    pub steps: u32,
    /// Step kind.
    pub kind: String,
    /// Phase.
    pub phase: RunPhase,
    /// Detail.
    pub detail: String,
    /// Bytes done.
    pub done_bytes: u64,
    /// Bytes total.
    pub total_bytes: u64,
}

/// What a run did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    /// App id.
    pub app_id: String,
    /// Installed.
    pub installed: Vec<String>,
    /// Sent.
    pub sent: Vec<String>,
    /// Imported.
    pub imported: Vec<String>,
    /// Skipped.
    pub skipped: Vec<String>,
    /// Launched.
    pub launched: bool,
}

/// The consent the run carries (`cua_teleport::ux::Consent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Consent {
    /// Confirmed.
    pub approved: bool,
    /// Secrets acknowledged.
    pub acknowledge_sensitive: bool,
    /// "Save to Keyvault" checked.
    #[serde(default)]
    pub save_to_keyvault: bool,
    /// [`Plan::relay_unsealed`]'s warning acknowledged (S1).
    #[serde(default)]
    pub acknowledge_relay_plaintext: bool,
    /// The sites whose cookies to send (the review's per-site choice);
    /// `None` when the review did not list sites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_domains: Option<Vec<String>>,
    /// Consent items (keys) the user turned off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Send these saved Keyvault items instead of reading the live app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_vault: Option<Vec<String>>,
    /// Also send the saved passwords (ticked in the review).
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_passwords: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The "needs the Cua app" prompt for a Keyvault refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallCuaPrompt {
    /// Cua is installed but not running.
    pub installed: bool,
    /// Title.
    pub title: String,
    /// One line.
    pub message: String,
    /// Button.
    pub action_label: String,
    /// Button target: always one of the constants, never from the error.
    pub url: String,
}

/// Where people install Cua.
pub const CUA_INSTALL_URL: &str = "https://cua.ai/install";
/// Opens the Keyvault page of an installed Cua app.
pub const CUA_OPEN_URL: &str = "cua://keyvault";

/// The prompt when `texts` carry the Keyvault's `requires_cua_app` refusal.
pub fn requires_cua_app(texts: &[String], installed_flag: bool) -> Option<InstallCuaPrompt> {
    if !texts
        .iter()
        .any(|t| contains_word(t, "requires_cua_app") || contains_word(t, "RequiresCuaApp"))
    {
        return None;
    }
    let installed = installed_flag
        || texts.iter().any(|t| {
            let l = t.to_lowercase();
            l.contains("installed but not running") || l.contains("needs the cua app running")
        });
    Some(if installed {
        InstallCuaPrompt {
            installed: true,
            title: "Open Cua to teleport your session".into(),
            message: "The Cua app keeps your logins in its Keyvault and asks you before sharing them. Open it, then try again.".into(),
            action_label: "Open Cua".into(),
            url: CUA_OPEN_URL.into(),
        }
    } else {
        InstallCuaPrompt {
            installed: false,
            title: "Install Cua to teleport your session".into(),
            message:
                "The Cua app keeps your logins in its Keyvault and asks you before sharing them."
                    .into(),
            action_label: "Install Cua".into(),
            url: CUA_INSTALL_URL.into(),
        }
    })
}

/// The picker's step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    /// Loading the catalog.
    Loading,
    /// Choosing an app.
    Pick,
    /// Choosing what moves.
    Options,
    /// Planning.
    Planning,
    /// Reviewing the consent list.
    Consent,
    /// Running.
    Running,
    /// Done.
    Done,
    /// Failed.
    Error,
}

/// The picker's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerState {
    /// Step.
    pub step: Step,
    /// The target Space's name.
    pub space_name: String,
    /// The catalog, once loaded.
    pub entries: Option<Vec<CatalogEntry>>,
    /// Search.
    pub query: String,
    /// Highlighted app.
    pub selected_id: Option<String>,
    /// The chosen app.
    pub entry: Option<CatalogEntry>,
    /// What moves.
    #[serde(rename = "move")]
    pub moves: Option<Move>,
    /// Chosen files.
    pub files: Vec<String>,
    /// The opt-in groups checked on the options step (only planned with the
    /// signed-in state move). Each group's items are secrets the review
    /// lists and asks to acknowledge.
    #[serde(default)]
    pub sensitive: Vec<SensitiveGroup>,
    /// The plan under review.
    pub plan: Option<Plan>,
    /// Secrets acknowledged.
    pub acknowledged: bool,
    /// "Save to Keyvault" checked: keep the captured session sealed in the
    /// Cua Keyvault after delivery, for reuse without asking again (shown
    /// alongside the acknowledgement, only when the plan is sensitive).
    #[serde(default)]
    pub save_to_keyvault: bool,
    /// [`Plan::relay_unsealed`]'s warning acknowledged (S1).
    #[serde(default)]
    pub acknowledged_relay_plaintext: bool,
    /// What the review sends: which sites, which items, from where.
    #[serde(default)]
    pub choice: ReviewChoice,
    /// Run events (last [`MAX_EVENTS`]).
    pub events: Vec<RunEvent>,
    /// The run's report.
    pub report: Option<RunReport>,
    /// Error text.
    pub error: Option<String>,
    /// The "needs the Cua app" prompt.
    pub install_prompt: Option<InstallCuaPrompt>,
    /// Where Back from an error goes.
    pub error_back: Step,
}

/// An input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PickerEvent {
    /// The catalog arrived.
    Loaded {
        /// Entries.
        entries: Vec<CatalogEntry>,
    },
    /// Something failed.
    #[serde(rename_all = "camelCase")]
    Failed {
        /// Message.
        message: String,
        /// Strings found in the error's cause (code, message, detail).
        #[serde(default)]
        cause_texts: Vec<String>,
        /// The cause said Cua is installed.
        #[serde(default)]
        cause_installed: bool,
    },
    /// Search.
    Query {
        /// Text.
        query: String,
    },
    /// Highlight.
    Select {
        /// Id.
        id: String,
    },
    /// Open the options for the highlighted (or given) app.
    Choose {
        /// Id.
        #[serde(default)]
        id: Option<String>,
    },
    /// Start on an app (a drop or a window drag).
    Preselect {
        /// Entry.
        entry: CatalogEntry,
        /// Files dropped with it.
        #[serde(default)]
        files: Vec<String>,
    },
    /// What moves.
    Move {
        /// Move.
        #[serde(rename = "move")]
        moves: Move,
    },
    /// Add files.
    Files {
        /// Paths.
        files: Vec<String>,
    },
    /// Remove a file.
    RemoveFile {
        /// Path.
        path: String,
    },
    /// An opt-in checkbox ("Keep me signed in", "Saved passwords", ...).
    Sensitive {
        /// The group.
        group: SensitiveGroup,
        /// Checked.
        value: bool,
    },
    /// Plan.
    Plan,
    /// The plan arrived.
    Planned {
        /// Plan.
        plan: Plan,
    },
    /// The secrets checkbox.
    Acknowledge {
        /// Checked.
        value: bool,
    },
    /// The "Save to Keyvault" checkbox.
    SaveToKeyvault {
        /// Checked.
        value: bool,
    },
    /// The relay-plaintext warning's checkbox (S1).
    AcknowledgeRelayPlaintext {
        /// Checked.
        value: bool,
    },
    /// The browser's sites with counts arrived (and what was picked last time
    /// for this app and Space, if anything).
    DomainsLoaded {
        /// The inventory.
        inventory: KvInventory,
        /// The sites sent last time.
        #[serde(default)]
        remembered: Option<Vec<String>>,
    },
    /// The inventory could not be read (the review then sends what the plan
    /// lists, as before).
    DomainsFailed,
    /// Pick or drop one site.
    ToggleDomain {
        /// The site.
        domain: String,
    },
    /// Pick or drop every site the search shows.
    SelectShownDomains {
        /// Pick (true) or drop.
        value: bool,
    },
    /// The sites list's search text.
    DomainQuery {
        /// Text.
        text: String,
    },
    /// Turn a consent line on or off.
    ToggleItem {
        /// The consent item's key.
        key: String,
    },
    /// Where to send from.
    SendFrom {
        /// The source.
        source: SendSource,
    },
    /// The app's saved Keyvault items (what the Keyvault holds for it).
    #[serde(rename_all = "camelCase")]
    VaultItems {
        /// How many can be sent.
        count: u32,
        /// When the newest was saved, Unix ms.
        newest_ms: i64,
        /// Now, Unix ms.
        now_ms: i64,
        /// The ones chosen to send (ids).
        selected: Vec<String>,
        /// The app's saved passwords (ids): sent only when ticked.
        #[serde(default)]
        password_ids: Vec<String>,
    },
    /// Tick or untick "Also send saved passwords".
    TogglePasswords {
        /// Ticked.
        value: bool,
    },
    /// The chosen saved items changed.
    VaultSelection {
        /// The ids.
        selected: Vec<String>,
    },
    /// Confirm the review.
    Confirm,
    /// A run event.
    Progress {
        /// Event.
        event: RunEvent,
    },
    /// The run finished.
    Finished {
        /// Report.
        report: RunReport,
    },
    /// Back.
    Back,
}

/// The first state: loading the catalog.
pub fn initial(space_name: &str) -> PickerState {
    PickerState {
        step: Step::Loading,
        space_name: space_name.into(),
        entries: None,
        query: String::new(),
        selected_id: None,
        entry: None,
        moves: None,
        files: vec![],
        sensitive: vec![],
        plan: None,
        acknowledged: false,
        save_to_keyvault: false,
        acknowledged_relay_plaintext: false,
        choice: ReviewChoice::default(),
        events: vec![],
        report: None,
        error: None,
        install_prompt: None,
        error_back: Step::Pick,
    }
}

/// The least that moves (files when some were dropped).
pub fn default_move(entry: &CatalogEntry, files: &[String]) -> Option<Move> {
    if !files.is_empty() && entry.moves.contains(&Move::AppWithFiles) {
        return Some(Move::AppWithFiles);
    }
    entry.moves.first().copied()
}

fn to_options(s: &PickerState, entry: &CatalogEntry, files: Vec<String>) -> PickerState {
    PickerState {
        step: Step::Options,
        selected_id: Some(entry.id.clone()),
        moves: default_move(entry, &files),
        entry: Some(entry.clone()),
        files,
        sensitive: vec![],
        plan: None,
        acknowledged: false,
        save_to_keyvault: false,
        acknowledged_relay_plaintext: false,
        choice: ReviewChoice::default(),
        error: None,
        install_prompt: None,
        ..s.clone()
    }
}

/// Every word of `query` in the name, id or bundle id (case-insensitive).
pub fn search_entries(entries: &[CatalogEntry], query: &str) -> Vec<CatalogEntry> {
    let q = query.to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    entries
        .iter()
        .filter(|e| {
            let hay = format!(
                "{} {} {}",
                e.name,
                e.id,
                e.host_app_id.as_deref().unwrap_or("")
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .cloned()
        .collect()
}

/// The entries the search shows.
pub fn visible_entries(s: &PickerState) -> Vec<CatalogEntry> {
    search_entries(s.entries.as_deref().unwrap_or(&[]), &s.query)
}

/// A titled list of apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntrySection {
    /// "Recent", "Apps", "Not available".
    pub title: String,
    /// Entries.
    pub entries: Vec<CatalogEntry>,
}

/// Recents, then available, then unavailable.
pub fn sections(s: &PickerState) -> Vec<EntrySection> {
    let visible = visible_entries(s);
    let supported = |e: &CatalogEntry| e.capability != Capability::Unsupported;
    let (recents, rest): (Vec<_>, Vec<_>) = visible
        .into_iter()
        .partition(|e| e.last_used_ms.is_some() && supported(e));
    let (apps, unavailable): (Vec<_>, Vec<_>) = rest.into_iter().partition(|e| supported(e));
    [
        ("Recent", recents),
        ("Apps", apps),
        ("Not available", unavailable),
    ]
    .into_iter()
    .filter(|(_, e)| !e.is_empty())
    .map(|(t, entries)| EntrySection {
        title: t.into(),
        entries,
    })
    .collect()
}

/// The options step's opt-in checkboxes: the entry's groups, with the
/// signed-in state move chosen; none otherwise. Every group starts
/// unchecked.
pub fn sensitive_options(s: &PickerState) -> Vec<SensitiveOption> {
    if s.step != Step::Options || s.moves != Some(Move::AppWithState) {
        return vec![];
    }
    s.entry
        .iter()
        .flat_map(|e| e.sensitive_groups.iter())
        .map(|&group| SensitiveOption {
            group,
            label: group.label().into(),
            detail: group.detail().into(),
            checked: s.sensitive.contains(&group),
        })
        .collect()
}

/// The groups the plan asks for (`TeleportPlanOptions.sensitive_groups`),
/// in the entry's order: the checked ones, only with the signed-in state
/// move.
pub fn plan_sensitive(s: &PickerState) -> Vec<SensitiveGroup> {
    if s.moves != Some(Move::AppWithState) {
        return vec![];
    }
    s.entry
        .iter()
        .flat_map(|e| e.sensitive_groups.iter())
        .filter(|g| s.sensitive.contains(g))
        .copied()
        .collect()
}

/// Planning is possible.
pub fn can_plan(s: &PickerState) -> bool {
    match (&s.entry, s.moves) {
        (Some(e), Some(m)) if e.moves.contains(&m) => {
            m != Move::AppWithFiles || !s.files.is_empty()
        }
        _ => false,
    }
}

/// The saved passwords can be offered: the Keyvault holds some for the app
/// (sending from it), or a chosen site holds some in the browser (reading it
/// now).
pub fn passwords_offered(s: &PickerState) -> bool {
    match s.choice.source {
        SendSource::Vault => !s.choice.vault.password_ids.is_empty(),
        SendSource::Live => password_count(s) > 0,
    }
}

/// How many saved passwords ticking the box would send.
pub fn password_count(s: &PickerState) -> u32 {
    match s.choice.source {
        SendSource::Vault => s.choice.vault.password_ids.len() as u32,
        SendSource::Live => s
            .choice
            .domains
            .iter()
            .filter(|d| s.choice.selected_domains.contains(&d.domain))
            .map(|d| d.passwords)
            .sum(),
    }
}

/// The review can be confirmed: a plan, secrets acknowledged when any, and
/// the relay-plaintext warning acknowledged when it applies (S1).
pub fn can_confirm(s: &PickerState) -> bool {
    let from_vault_ok = s.choice.source != SendSource::Vault || !s.choice.vault.selected.is_empty();
    from_vault_ok
        && s.plan.as_ref().is_some_and(|p| {
            (!(p.sensitive || s.choice.include_passwords) || s.acknowledged)
                && (!p.relay_unsealed || s.acknowledged_relay_plaintext)
        })
}

/// Whether a consent key is the browser's cookie store.
fn is_cookie_key(key: &str) -> bool {
    key.rsplit('/')
        .next()
        .unwrap_or(key)
        .eq_ignore_ascii_case("cookies")
}

/// The plan sends a browser's cookies.
fn sends_cookies(plan: &Plan) -> bool {
    plan.consent.iter().any(|c| is_cookie_key(&c.key))
}

/// Consent lines the user may turn off: not installs, and not the cookie
/// store while its sites are listed (the site choice is that line).
fn toggleable(c: &ConsentItem, choice: &ReviewChoice) -> bool {
    c.kind != ConsentKind::Install && !(is_cookie_key(&c.key) && !choice.domains.is_empty())
}

/// The consent the confirmed review carries.
pub fn consent(s: &PickerState) -> Consent {
    let vault = s.choice.source == SendSource::Vault;
    Consent {
        approved: can_confirm(s),
        acknowledge_sensitive: s.acknowledged,
        // Items sent from the vault are already saved.
        save_to_keyvault: s.save_to_keyvault && !vault,
        acknowledge_relay_plaintext: s.acknowledged_relay_plaintext,
        cookie_domains: (!vault && !s.choice.domains.is_empty())
            .then(|| s.choice.selected_domains.clone()),
        exclude: s.choice.excluded.clone(),
        from_vault: vault.then(|| {
            let mut ids = s.choice.vault.selected.clone();
            if s.choice.include_passwords {
                ids.extend(s.choice.vault.password_ids.iter().cloned());
            }
            ids
        }),
        include_passwords: passwords_offered(s) && s.choice.include_passwords,
    }
}

/// Run progress in `[0, 1]`.
pub fn progress(s: &PickerState) -> f64 {
    let Some(last) = s.events.last() else {
        return 0.0;
    };
    if last.phase == RunPhase::Done {
        return 1.0;
    }
    let within = if last.total_bytes > 0 {
        (last.done_bytes as f64 / last.total_bytes as f64).min(1.0)
    } else if last.phase == RunPhase::Finished {
        1.0
    } else {
        0.0
    };
    ((last.step as f64 + within) / (last.steps.max(1) as f64)).min(1.0)
}

/// What the run is doing, in words, for the line under its progress bar:
/// the latest step or stage the pipeline reported ("Reading Chrome cookies
/// (macOS will ask for Keychain access)…", "Packing profile"), with the
/// bytes while they move ("Uploading 12 / 80 MB"). None before the first
/// event and once the run is done or failed.
pub fn run_status(events: &[RunEvent]) -> Option<String> {
    let last = events.last()?;
    if matches!(last.phase, RunPhase::Done | RunPhase::Failed) {
        return None;
    }
    // The newest event with words: a stage, else the step as it started.
    let worded = events
        .iter()
        .rev()
        .take_while(|e| e.step == last.step)
        .find(|e| {
            matches!(e.phase, RunPhase::Started | RunPhase::Progress) && !e.detail.trim().is_empty()
        })?;
    let mut text = worded.detail.trim().to_string();
    // A file path or an installer line reads as the step it belongs to.
    if worded.phase == RunPhase::Progress && worded.kind != "state" {
        text = events
            .iter()
            .rev()
            .find(|e| e.step == last.step && e.phase == RunPhase::Started)
            .map(|e| e.detail.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or(text);
    }
    if last.phase == RunPhase::Progress
        && last.total_bytes > 0
        && last.done_bytes < last.total_bytes
    {
        text = format!(
            "{text} {}",
            format_bytes_pair(last.done_bytes, last.total_bytes)
        );
    }
    Some(text)
}

/// [`run_status`] of the picker's run (none unless it is running).
pub fn status(s: &PickerState) -> Option<String> {
    (s.step == Step::Running)
        .then(|| run_status(&s.events))
        .flatten()
}

/// "12 / 80 MB" (one unit when both read in it), else "512 KB / 80 MB".
pub fn format_bytes_pair(done: u64, total: u64) -> String {
    let (d, t) = (format_bytes(done.min(total)), format_bytes(total));
    match (d.rsplit_once(' '), t.rsplit_once(' ')) {
        (Some((dn, du)), Some((_, tu))) if du == tu => format!("{dn} / {t}"),
        _ => format!("{d} / {t}"),
    }
}

/// "512 B", "1.5 KB", "12 MB".
pub fn format_bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let num = if v >= 10.0 {
        format!("{}", v.round() as u64)
    } else {
        to_fixed(v, 1)
    };
    format!("{num} {}", units[i])
}

/// Advances the picker. Events that do not apply leave it unchanged.
pub fn reduce(s: &PickerState, e: &PickerEvent) -> PickerState {
    let mut n = s.clone();
    match e {
        PickerEvent::Loaded { entries } => {
            if s.step != Step::Loading {
                n.entries = Some(entries.clone());
                return n;
            }
            let enabled = entries
                .iter()
                .find(|x| x.capability != Capability::Unsupported)
                .map(|x| x.id.clone());
            n.step = Step::Pick;
            n.entries = Some(entries.clone());
            n.selected_id = s.selected_id.clone().or(enabled);
            n.error = None;
            n.install_prompt = None;
        }
        PickerEvent::Failed {
            message,
            cause_texts,
            cause_installed,
        } => {
            n.error_back = match s.step {
                Step::Planning => Step::Options,
                Step::Running => Step::Consent,
                _ if s.entries.is_some() => Step::Pick,
                _ => Step::Loading,
            };
            n.step = Step::Error;
            n.error = Some(message.clone());
            n.install_prompt = requires_cua_app(cause_texts, *cause_installed)
                .or_else(|| requires_cua_app(std::slice::from_ref(message), false));
        }
        PickerEvent::Query { query } => {
            if s.step != Step::Pick {
                return n;
            }
            n.query = query.clone();
            let visible = visible_entries(&n);
            if !visible
                .iter()
                .any(|x| Some(&x.id) == s.selected_id.as_ref())
            {
                n.selected_id = visible
                    .iter()
                    .find(|x| x.capability != Capability::Unsupported)
                    .map(|x| x.id.clone());
            }
        }
        PickerEvent::Select { id } => {
            if s.step == Step::Pick {
                n.selected_id = Some(id.clone());
            }
        }
        PickerEvent::Choose { id } => {
            if s.step != Step::Pick {
                return n;
            }
            let id = id.clone().or_else(|| s.selected_id.clone());
            let entry = s
                .entries
                .as_ref()
                .and_then(|es| es.iter().find(|x| Some(&x.id) == id.as_ref()));
            match entry {
                Some(entry) if entry.capability != Capability::Unsupported => {
                    return to_options(s, entry, vec![]);
                }
                _ => return n,
            }
        }
        PickerEvent::Preselect { entry, files } => {
            if entry.capability == Capability::Unsupported {
                n.step = Step::Error;
                n.entry = Some(entry.clone());
                n.error = Some(format!(
                    "{} cannot be teleported: {}",
                    entry.name,
                    entry.reason.as_deref().unwrap_or("unsupported")
                ));
                n.install_prompt = None;
                n.error_back = Step::Pick;
                return n;
            }
            return to_options(s, entry, files.clone());
        }
        PickerEvent::Move { moves } => {
            if s.step == Step::Options && s.entry.as_ref().is_some_and(|e| e.moves.contains(moves))
            {
                n.moves = Some(*moves);
            }
        }
        PickerEvent::Files { files } => {
            if s.step == Step::Options {
                for f in files {
                    if !n.files.contains(f) {
                        n.files.push(f.clone());
                    }
                }
            }
        }
        PickerEvent::RemoveFile { path } => {
            if s.step == Step::Options {
                n.files.retain(|f| f != path);
            }
        }
        PickerEvent::Sensitive { group, value } => {
            let offered = s
                .entry
                .as_ref()
                .is_some_and(|e| e.sensitive_groups.contains(group));
            // Only while the checkboxes show: options, signed-in state move.
            if s.step == Step::Options && s.moves == Some(Move::AppWithState) && offered {
                n.sensitive.retain(|g| g != group);
                if *value {
                    n.sensitive.push(*group);
                }
            }
        }
        PickerEvent::Plan => {
            if s.step == Step::Options && can_plan(s) {
                n.step = Step::Planning;
                n.error = None;
                n.install_prompt = None;
            }
        }
        PickerEvent::Planned { plan } => {
            if s.step == Step::Planning {
                n.step = Step::Consent;
                n.plan = Some(plan.clone());
                n.acknowledged = false;
                n.save_to_keyvault = false;
                n.acknowledged_relay_plaintext = false;
                n.choice = ReviewChoice::default();
            }
        }
        PickerEvent::Acknowledge { value } => {
            if s.step == Step::Consent {
                n.acknowledged = *value;
            }
        }
        PickerEvent::SaveToKeyvault { value } => {
            // Only meaningful (and only shown) for a plan with something
            // sensitive to save; otherwise there is nothing the Keyvault
            // would keep.
            if s.step == Step::Consent && s.plan.as_ref().is_some_and(|p| p.sensitive) {
                n.save_to_keyvault = *value;
            }
        }
        PickerEvent::AcknowledgeRelayPlaintext { value } => {
            if s.step == Step::Consent {
                n.acknowledged_relay_plaintext = *value;
            }
        }
        PickerEvent::DomainsLoaded {
            inventory,
            remembered,
        } => {
            if s.step == Step::Consent {
                let keep = n.choice.clone();
                n.choice = review::with_inventory(keep, inventory, remembered.as_deref());
                n.choice.loaded = true;
            }
        }
        PickerEvent::DomainsFailed => {
            if s.step == Step::Consent {
                n.choice.loaded = true;
            }
        }
        PickerEvent::ToggleDomain { domain } => {
            if s.step == Step::Consent {
                review::toggle_domain(&mut n.choice, domain);
            }
        }
        PickerEvent::SelectShownDomains { value } => {
            if s.step == Step::Consent {
                review::set_shown_domains(&mut n.choice, *value);
            }
        }
        PickerEvent::DomainQuery { text } => {
            if s.step == Step::Consent {
                n.choice.query = text.clone();
            }
        }
        PickerEvent::ToggleItem { key } => {
            let ok = s.step == Step::Consent
                && s.plan.as_ref().is_some_and(|p| {
                    p.consent
                        .iter()
                        .any(|c| &c.key == key && toggleable(c, &s.choice))
                });
            if ok {
                match n.choice.excluded.iter().position(|k| k == key) {
                    Some(i) => {
                        n.choice.excluded.remove(i);
                    }
                    None => n.choice.excluded.push(key.clone()),
                }
            }
        }
        PickerEvent::SendFrom { source } => {
            if s.step == Step::Consent && (*source == SendSource::Live || s.choice.vault.available)
            {
                n.choice.source = *source;
            }
        }
        PickerEvent::VaultItems {
            count,
            newest_ms,
            now_ms,
            selected,
            password_ids,
        } => {
            if s.step == Step::Consent {
                n.choice.vault = VaultSource {
                    password_ids: password_ids.clone(),
                    available: *count > 0,
                    items: *count,
                    saved: if *newest_ms > 0 {
                        format!("saved {}", crate::keyvault::view::ago(now_ms - newest_ms))
                    } else {
                        String::new()
                    },
                    selected: selected.clone(),
                };
                if *count == 0 {
                    n.choice.source = SendSource::Live;
                }
            }
        }
        PickerEvent::TogglePasswords { value } => {
            if s.step == Step::Consent && passwords_offered(s) {
                n.choice.include_passwords = *value;
            }
        }
        PickerEvent::VaultSelection { selected } => {
            if s.step == Step::Consent && s.choice.vault.available {
                n.choice.vault.selected = selected.clone();
            }
        }
        PickerEvent::Confirm => {
            if s.step == Step::Consent && can_confirm(s) {
                n.step = Step::Running;
                n.events = vec![];
            }
        }
        PickerEvent::Progress { event } => {
            if s.step == Step::Running {
                n.events.push(event.clone());
                let excess = n.events.len().saturating_sub(MAX_EVENTS);
                n.events.drain(..excess);
            }
        }
        PickerEvent::Finished { report } => {
            if s.step == Step::Running {
                n.step = Step::Done;
                n.report = Some(report.clone());
            }
        }
        PickerEvent::Back => match s.step {
            Step::Options => {
                if s.entries.is_some() {
                    n.step = Step::Pick;
                    n.plan = None;
                }
            }
            Step::Consent => {
                n.step = Step::Options;
                n.plan = None;
                n.acknowledged = false;
                n.save_to_keyvault = false;
                n.acknowledged_relay_plaintext = false;
                n.choice = ReviewChoice::default();
            }
            Step::Error => {
                n.step = s.error_back;
                n.error = None;
                n.install_prompt = None;
            }
            _ => {}
        },
    }
    n
}

/// The review screen as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewView {
    /// "Teleport Slack to Aurora".
    pub title: String,
    /// The consent lines.
    pub items: Vec<ConsentItem>,
    /// Plan steps.
    pub steps: Vec<PlanStepView>,
    /// The secrets checkbox shows.
    pub needs_acknowledgement: bool,
    /// Checked.
    pub acknowledged: bool,
    /// The "Save to Keyvault" checkbox shows (a sensitive plan only: there
    /// is nothing to keep sealed otherwise).
    pub offers_save_to_keyvault: bool,
    /// Checked.
    pub save_to_keyvault: bool,
    /// The relay-plaintext warning's checkbox shows (S1): this Space
    /// predates end-to-end sealing, and the Cua relay could read this
    /// secret in transit.
    pub needs_relay_plaintext_acknowledgement: bool,
    /// Checked.
    pub acknowledged_relay_plaintext: bool,
    /// "Teleport" is enabled.
    pub can_confirm: bool,
    /// "12 MB leaves this Mac" (none when nothing leaves).
    pub leaves_text: Option<String>,
    /// Caveats.
    pub warnings: Vec<String>,
    /// Lines the user can turn off (files, folders, state, secrets), each
    /// with whether it is sent. The cookie store is the site list below.
    pub toggles: Vec<ReviewToggle>,
    /// The plan sends a browser's cookies, so the sites can be chosen.
    pub offers_domains: bool,
    /// The sites have not been read yet (the shell asks the Keyvault for
    /// them, which may ask for Touch ID).
    pub needs_domains: bool,
    /// The sites with counts, filtered by the search.
    pub domains: Vec<ReviewDomain>,
    /// "3 of 12 sites".
    pub domain_summary: String,
    /// The sites search text.
    pub domain_query: String,
    /// The sites chosen (what is remembered for next time).
    pub selected_domains: Vec<String>,
    /// Where it sends from.
    pub source: SendSource,
    /// The app has saved Keyvault items to send instead of reading the live
    /// app.
    pub offers_vault: bool,
    /// The saved items.
    pub vault: VaultSource,
    /// "Send from the Keyvault (212 items, saved 2 days ago)".
    pub vault_label: String,
    /// "Read Chrome now (macOS asks for Keychain access)".
    pub live_label: String,
    /// "Also send saved passwords" shows (there are some to send).
    pub offers_passwords: bool,
    /// Ticked.
    pub include_passwords: bool,
    /// "Also send 14 saved passwords".
    pub passwords_label: String,
    /// What the Keychain will do for this source, in a line.
    pub source_note: String,
}

/// The review, when a plan is under review.
/// This Space predates end-to-end sealing (S1): the same warning the SDK's
/// own relay gate reports, worded for the consent screen as a question.
pub const RELAY_PLAINTEXT_WARNING: &str =
    "this Space predates end-to-end sealing; the Cua relay could read these secrets. Send anyway?";

pub fn review(s: &PickerState) -> Option<ReviewView> {
    let plan = s.plan.as_ref()?;
    let mut warnings = plan.warnings.clone();
    if plan.relay_unsealed {
        warnings.push(RELAY_PLAINTEXT_WARNING.into());
    }
    let choice = &s.choice;
    let vault = choice.source == SendSource::Vault;
    let cookies = sends_cookies(plan);
    // What leaves, less what was turned off (and the cookie store when no
    // site is chosen).
    let no_sites = !choice.domains.is_empty() && choice.selected_domains.is_empty();
    let bytes: u64 = plan
        .consent
        .iter()
        .filter(|c| !choice.excluded.contains(&c.key) && !(no_sites && is_cookie_key(&c.key)))
        .map(|c| c.bytes)
        .sum();
    let leaves_text = if vault {
        let n = choice.vault.selected.len();
        Some(format!(
            "{n} saved item{} from the Keyvault",
            if n == 1 { "" } else { "s" }
        ))
    } else {
        (bytes > 0).then(|| format!("{} leaves this Mac", format_bytes(bytes)))
    };
    let app = &plan.app.name;
    Some(ReviewView {
        title: format!("Teleport {} to {}", plan.app.name, s.space_name),
        items: plan.consent.clone(),
        steps: plan.steps.clone(),
        toggles: plan
            .consent
            .iter()
            .filter(|c| toggleable(c, choice))
            .map(|c| ReviewToggle {
                key: c.key.clone(),
                label: c.label.clone(),
                detail: c.detail.clone(),
                bytes: c.bytes,
                sensitive: c.sensitive,
                selected: !choice.excluded.contains(&c.key),
            })
            .collect(),
        offers_domains: cookies && !vault,
        needs_domains: cookies && !vault && !choice.loaded,
        domains: review::domain_rows(choice),
        domain_summary: review::domain_summary(choice),
        domain_query: choice.query.clone(),
        selected_domains: choice.selected_domains.clone(),
        source: choice.source,
        offers_vault: choice.vault.available,
        vault: choice.vault.clone(),
        vault_label: if choice.vault.available {
            let n = choice.vault.items;
            let saved = if choice.vault.saved.is_empty() {
                String::new()
            } else {
                format!(", {}", choice.vault.saved)
            };
            format!(
                "Send from the Keyvault ({n} item{}{saved})",
                if n == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        },
        live_label: if cookies {
            format!("Read {app} now")
        } else {
            format!("Read {app} on this Mac")
        },
        source_note: if vault {
            "Nothing is read from this Mac, so macOS does not ask for Keychain access.".into()
        } else if cookies {
            format!("Reading {app}'s cookies makes macOS ask for Keychain access.")
        } else {
            String::new()
        },
        offers_passwords: passwords_offered(s),
        include_passwords: s.choice.include_passwords && passwords_offered(s),
        passwords_label: {
            let n = password_count(s);
            if n == 0 {
                String::new()
            } else {
                format!(
                    "Also send {n} saved password{}",
                    if n == 1 { "" } else { "s" }
                )
            }
        },
        needs_acknowledgement: plan.sensitive
            || (s.choice.include_passwords && passwords_offered(s)),
        acknowledged: s.acknowledged,
        offers_save_to_keyvault: plan.sensitive,
        save_to_keyvault: s.save_to_keyvault,
        needs_relay_plaintext_acknowledgement: plan.relay_unsealed,
        acknowledged_relay_plaintext: s.acknowledged_relay_plaintext,
        can_confirm: can_confirm(s),
        leaves_text,
        warnings,
    })
}

#[cfg(test)]
mod tests {

    fn ev(step: u32, kind: &str, phase: RunPhase, detail: &str, done: u64, total: u64) -> RunEvent {
        RunEvent {
            step,
            steps: 2,
            kind: kind.into(),
            phase,
            detail: detail.into(),
            done_bytes: done,
            total_bytes: total,
        }
    }

    #[test]
    fn the_run_says_what_it_is_doing_step_by_step() {
        use RunPhase::*;
        const MB: u64 = 1024 * 1024;
        let reading = "Reading Chrome cookies (macOS will ask for Keychain access)\u{2026}";
        let mut events = vec![];
        assert_eq!(run_status(&events), None, "nothing yet");
        events.push(ev(0, "state", Started, "Preparing the sign-in", 0, 0));
        assert_eq!(
            run_status(&events).as_deref(),
            Some("Preparing the sign-in")
        );
        events.push(ev(0, "state", Progress, reading, 0, 0));
        assert_eq!(run_status(&events).as_deref(), Some(reading));
        events.push(ev(0, "state", Progress, "Packing profile", 0, 0));
        assert_eq!(run_status(&events).as_deref(), Some("Packing profile"));
        events.push(ev(0, "state", Progress, "Uploading", 12 * MB, 80 * MB));
        assert_eq!(run_status(&events).as_deref(), Some("Uploading 12 / 80 MB"));
        events.push(ev(0, "state", Progress, "Importing into the Space", 0, 0));
        assert_eq!(
            run_status(&events).as_deref(),
            Some("Importing into the Space")
        );
        // Between steps the finished step's words stay.
        events.push(ev(0, "state", Finished, "", 0, 0));
        assert_eq!(
            run_status(&events).as_deref(),
            Some("Importing into the Space")
        );
        // A file path reads as its step, with the bytes.
        events.push(ev(1, "files", Started, "Sending 2 items", 0, 0));
        events.push(ev(
            1,
            "files",
            Progress,
            "/Users/me/a.txt",
            512 * 1024,
            2 * MB,
        ));
        assert_eq!(
            run_status(&events).as_deref(),
            Some("Sending 2 items 512 KB / 2.0 MB")
        );
        events.push(ev(2, "done", Done, "Chrome", 0, 0));
        assert_eq!(run_status(&events), None, "the done step says the rest");
        assert_eq!(format_bytes_pair(5 * MB, 80 * MB), "5.0 / 80 MB");
        assert_eq!(format_bytes_pair(90 * MB, 80 * MB), "80 / 80 MB");
    }

    use super::*;

    #[test]
    fn bytes_format_like_the_webview() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1280), "1.3 KB");
        assert_eq!(format_bytes(15 * 1024 * 1024), "15 MB");
    }

    #[test]
    fn sensitive_groups_are_separate_unchecked_opt_ins_for_the_state_move() {
        let entry: CatalogEntry = serde_json::from_value(serde_json::json!({
            "id": "com.google.Chrome", "name": "Google Chrome", "capability": "full",
            "moves": ["app_only", "app_with_state"], "providerId": "chrome",
            "sensitiveGroups": ["sign_ins", "passwords", "history"], "json": "{}"
        }))
        .unwrap();
        let s = reduce(
            &initial("Space B"),
            &PickerEvent::Preselect {
                entry,
                files: vec![],
            },
        );
        assert_eq!(s.step, Step::Options);
        // The first move (app only) offers nothing and plans nothing.
        assert!(sensitive_options(&s).is_empty());
        let s = reduce(
            &s,
            &PickerEvent::Sensitive {
                group: SensitiveGroup::SignIns,
                value: true,
            },
        );
        // Not shown, so not taken.
        assert!(s.sensitive.is_empty());
        let s = reduce(
            &s,
            &PickerEvent::Move {
                moves: Move::AppWithState,
            },
        );
        let s = reduce(
            &s,
            &PickerEvent::Sensitive {
                group: SensitiveGroup::SignIns,
                value: true,
            },
        );
        let opts = sensitive_options(&s);
        let labels: Vec<(&str, bool)> =
            opts.iter().map(|o| (o.label.as_str(), o.checked)).collect();
        assert_eq!(
            labels,
            [
                ("Keep me signed in", true),
                ("Saved passwords", false),
                ("Browsing history", false)
            ]
        );
        assert_eq!(plan_sensitive(&s), [SensitiveGroup::SignIns]);
        let s = reduce(
            &s,
            &PickerEvent::Sensitive {
                group: SensitiveGroup::History,
                value: true,
            },
        );
        assert_eq!(
            plan_sensitive(&s),
            [SensitiveGroup::SignIns, SensitiveGroup::History]
        );
        let s = reduce(
            &s,
            &PickerEvent::Sensitive {
                group: SensitiveGroup::SignIns,
                value: false,
            },
        );
        assert_eq!(plan_sensitive(&s), [SensitiveGroup::History]);
        // Only on the options step.
        let planning = reduce(&s, &PickerEvent::Plan);
        assert_eq!(planning.step, Step::Planning);
        let unchanged = reduce(
            &planning,
            &PickerEvent::Sensitive {
                group: SensitiveGroup::History,
                value: false,
            },
        );
        assert_eq!(unchanged.sensitive, [SensitiveGroup::History]);
    }

    #[test]
    fn a_group_the_app_does_not_offer_cannot_be_checked() {
        let entry: CatalogEntry = serde_json::from_value(serde_json::json!({
            "id": "slack", "name": "Slack", "capability": "full",
            "moves": ["app_only", "app_with_state"], "providerId": "slack", "json": "{}"
        }))
        .unwrap();
        let s = reduce(
            &initial("B"),
            &PickerEvent::Preselect {
                entry,
                files: vec![],
            },
        );
        let s = reduce(
            &s,
            &PickerEvent::Move {
                moves: Move::AppWithState,
            },
        );
        assert!(sensitive_options(&s).is_empty());
        let s = reduce(
            &s,
            &PickerEvent::Sensitive {
                group: SensitiveGroup::Passwords,
                value: true,
            },
        );
        assert!(s.sensitive.is_empty());
        assert!(plan_sensitive(&s).is_empty());
    }

    #[test]
    fn cua_app_refusal_becomes_a_prompt() {
        let p = requires_cua_app(
            &["requires_cua_app: installed but not running".into()],
            false,
        )
        .unwrap();
        assert!(p.installed);
        assert_eq!(p.url, CUA_OPEN_URL);
        assert!(requires_cua_app(&["xrequires_cua_app".into()], false).is_none());
    }

    fn chrome_entry() -> CatalogEntry {
        serde_json::from_value(serde_json::json!({
            "id": "com.google.Chrome", "name": "Google Chrome", "capability": "full",
            "moves": ["app_only", "app_with_state"], "providerId": "chrome",
            "sensitiveGroups": ["sign_ins"], "json": "{}"
        }))
        .unwrap()
    }

    fn plan(sensitive: bool) -> Plan {
        Plan {
            app: chrome_entry(),
            space_id: "local:dev".into(),
            moves: Move::AppWithState,
            steps: vec![],
            consent: vec![],
            sensitive,
            total_bytes: 1024,
            warnings: vec![],
            relay_unsealed: false,
            json: "{}".into(),
        }
    }

    /// Walks `initial` to `Step::Consent` the way the picker really does
    /// (preselect, choose the signed-in state move, plan).
    fn consent_state() -> PickerState {
        let s = reduce(
            &initial("dev"),
            &PickerEvent::Preselect {
                entry: chrome_entry(),
                files: vec![],
            },
        );
        let s = reduce(
            &s,
            &PickerEvent::Move {
                moves: Move::AppWithState,
            },
        );
        reduce(&s, &PickerEvent::Plan)
    }

    /// "Save to Keyvault" only shows and only takes for a sensitive plan (a
    /// non-sensitive teleport has nothing a Keyvault would seal), resets
    /// going Back, and reaches `consent()`.
    #[test]
    fn save_to_keyvault_only_applies_to_a_sensitive_plan_and_reaches_consent() {
        let s = reduce(&consent_state(), &PickerEvent::Planned { plan: plan(true) });
        assert_eq!(s.step, Step::Consent);
        let rv = review(&s).unwrap();
        assert!(rv.offers_save_to_keyvault);
        assert!(!rv.save_to_keyvault);

        let s = reduce(&s, &PickerEvent::SaveToKeyvault { value: true });
        assert!(s.save_to_keyvault);
        assert!(review(&s).unwrap().save_to_keyvault);
        assert!(consent(&s).save_to_keyvault);

        // Back resets it, matching `acknowledged`.
        let back = reduce(&s, &PickerEvent::Back);
        assert!(!back.save_to_keyvault);

        // A non-sensitive plan offers nothing to save and the event is a
        // no-op even if sent anyway.
        let ns = reduce(
            &consent_state(),
            &PickerEvent::Planned { plan: plan(false) },
        );
        let review_ns = review(&ns).unwrap();
        assert!(!review_ns.offers_save_to_keyvault);
        let ns = reduce(&ns, &PickerEvent::SaveToKeyvault { value: true });
        assert!(!ns.save_to_keyvault);
    }

    /// A chrome plan that sends its cookie store, a bookmarks file and tabs.
    fn browser_plan() -> Plan {
        let mut p = plan(true);
        p.consent = vec![
            ConsentItem {
                kind: ConsentKind::Install,
                key: "chrome".into(),
                label: "Google Chrome".into(),
                detail: "pinned".into(),
                bytes: 0,
                sensitive: false,
            },
            ConsentItem {
                kind: ConsentKind::State,
                key: "Default/Bookmarks".into(),
                label: "Bookmarks".into(),
                detail: "".into(),
                bytes: 200,
                sensitive: false,
            },
            ConsentItem {
                kind: ConsentKind::Secret,
                key: ".config/google-chrome/Default/Cookies".into(),
                label: "Cookies".into(),
                detail: "".into(),
                bytes: 800,
                sensitive: true,
            },
        ];
        p.total_bytes = 1000;
        p
    }

    fn inventory() -> KvInventory {
        use crate::keyvault::wire::KvDomainCount;
        let d = |domain: &str, cookies: u32, signin: bool, idp: bool| KvDomainCount {
            domain: domain.into(),
            cookies,
            signin,
            identity_provider: idp,
            ..Default::default()
        };
        KvInventory {
            provider_id: "chrome".into(),
            app_display: "Google Chrome".into(),
            domains: vec![
                d("github.com", 12, true, false),
                d("google.com", 30, true, true),
                d("notion.so", 4, true, false),
                d("doubleclick.net", 9, false, false),
            ],
            notes: vec![],
        }
    }

    fn review_state() -> PickerState {
        let s = reduce(
            &consent_state(),
            &PickerEvent::Planned {
                plan: browser_plan(),
            },
        );
        reduce(&s, &PickerEvent::Acknowledge { value: true })
    }

    /// The browser's sites are asked for once, start minimal (or from what
    /// was picked last time), and the consent carries exactly the chosen ones.
    #[test]
    fn the_review_lists_sites_with_counts_and_sends_only_the_chosen_ones() {
        let s = review_state();
        let rv = review(&s).unwrap();
        assert!(rv.offers_domains && rv.needs_domains && rv.domains.is_empty());
        assert_eq!(
            rv.toggles
                .iter()
                .map(|t| t.key.as_str())
                .collect::<Vec<_>>(),
            ["Default/Bookmarks", ".config/google-chrome/Default/Cookies"],
            "until the sites are listed the cookie line is a plain line"
        );
        assert!(consent(&s).cookie_domains.is_none());

        let s = reduce(
            &s,
            &PickerEvent::DomainsLoaded {
                inventory: inventory(),
                remembered: None,
            },
        );
        let rv = review(&s).unwrap();
        assert!(!rv.needs_domains);
        assert_eq!(
            rv.domain_summary, "2 of 4 sites",
            "minimal: sign-in sites, never an identity provider"
        );
        assert_eq!(rv.selected_domains, ["github.com", "notion.so"]);
        assert_eq!(rv.domains[0].counts, "9 cookies");
        assert_eq!(
            rv.toggles
                .iter()
                .map(|t| t.key.as_str())
                .collect::<Vec<_>>(),
            ["Default/Bookmarks"],
            "the cookie line is now the site list"
        );
        assert_eq!(
            consent(&s).cookie_domains,
            Some(vec!["github.com".to_string(), "notion.so".to_string()])
        );

        // Pick one more, drop one, search, turn the bookmarks off.
        let s = reduce(
            &s,
            &PickerEvent::ToggleDomain {
                domain: "google.com".into(),
            },
        );
        let s = reduce(
            &s,
            &PickerEvent::ToggleDomain {
                domain: "github.com".into(),
            },
        );
        let s = reduce(
            &s,
            &PickerEvent::ToggleItem {
                key: "Default/Bookmarks".into(),
            },
        );
        let s = reduce(
            &s,
            &PickerEvent::ToggleItem {
                key: "chrome".into(),
            },
        );
        let s = reduce(
            &s,
            &PickerEvent::ToggleItem {
                key: "unknown".into(),
            },
        );
        let c = consent(&s);
        assert_eq!(
            c.cookie_domains,
            Some(vec!["google.com".to_string(), "notion.so".to_string()])
        );
        assert_eq!(
            c.exclude,
            ["Default/Bookmarks"],
            "installs and unknown lines cannot be turned off"
        );
        assert!(c.approved && c.from_vault.is_none());
        let rv = review(&s).unwrap();
        assert_eq!(rv.leaves_text.as_deref(), Some("800 B leaves this Mac"));
        let s2 = reduce(&s, &PickerEvent::DomainQuery { text: "not".into() });
        assert_eq!(review(&s2).unwrap().domains.len(), 1);
        let s3 = reduce(&s2, &PickerEvent::SelectShownDomains { value: false });
        assert_eq!(
            consent(&s3).cookie_domains,
            Some(vec!["google.com".to_string()])
        );

        // No sites chosen: no cookies leave, and the bytes follow.
        let none = reduce(&s, &PickerEvent::SelectShownDomains { value: false });
        let none = reduce(
            &none,
            &PickerEvent::DomainQuery {
                text: String::new(),
            },
        );
        let none = reduce(&none, &PickerEvent::SelectShownDomains { value: false });
        assert_eq!(consent(&none).cookie_domains, Some(vec![]));
        assert_eq!(review(&none).unwrap().leaves_text, None);

        // What was picked last time wins over the minimal default.
        let r = reduce(
            &review_state(),
            &PickerEvent::DomainsLoaded {
                inventory: inventory(),
                remembered: Some(vec!["doubleclick.net".into()]),
            },
        );
        assert_eq!(review(&r).unwrap().selected_domains, ["doubleclick.net"]);

        // A failed read leaves the plan as it was.
        let f = reduce(&review_state(), &PickerEvent::DomainsFailed);
        assert!(!review(&f).unwrap().needs_domains);
        assert!(consent(&f).cookie_domains.is_none());
        // Going Back forgets the choices.
        assert_eq!(
            reduce(&s, &PickerEvent::Back).choice,
            ReviewChoice::default()
        );
    }

    /// Sending from the Keyvault: offered only when the app has saved items,
    /// reads nothing from the host, needs a selection, and carries the ids.
    #[test]
    fn the_review_can_send_the_apps_saved_keyvault_items_instead_of_reading_it() {
        let s = review_state();
        assert!(!review(&s).unwrap().offers_vault);
        // Not offered: choosing it does nothing.
        let nope = reduce(
            &s,
            &PickerEvent::SendFrom {
                source: SendSource::Vault,
            },
        );
        assert_eq!(review(&nope).unwrap().source, SendSource::Live);

        let now = 1_800_000_000_000_i64;
        let s = reduce(
            &s,
            &PickerEvent::VaultItems {
                count: 212,
                newest_ms: now - 2 * 24 * 3_600_000,
                now_ms: now,
                selected: vec!["a".into(), "b".into()],
                password_ids: vec![],
            },
        );
        let rv = review(&s).unwrap();
        assert!(rv.offers_vault);
        assert_eq!(
            rv.vault_label,
            "Send from the Keyvault (212 items, saved 2 days ago)"
        );
        assert_eq!(
            rv.source,
            SendSource::Live,
            "reading the live app stays the default"
        );
        assert!(rv.source_note.contains("Keychain access"));

        let s = reduce(
            &s,
            &PickerEvent::SendFrom {
                source: SendSource::Vault,
            },
        );
        let rv = review(&s).unwrap();
        assert_eq!(rv.source, SendSource::Vault);
        assert!(
            !rv.offers_domains,
            "no sites to pick: the saved items are the list"
        );
        assert!(rv.source_note.contains("Nothing is read from this Mac"));
        assert_eq!(
            rv.leaves_text.as_deref(),
            Some("2 saved items from the Keyvault")
        );
        let c = consent(&s);
        assert_eq!(c.from_vault, Some(vec!["a".to_string(), "b".to_string()]));
        assert!(c.cookie_domains.is_none() && !c.save_to_keyvault);
        assert!(c.approved);

        // Nothing selected: nothing to send.
        let empty = reduce(&s, &PickerEvent::VaultSelection { selected: vec![] });
        assert!(!review(&empty).unwrap().can_confirm);
        assert!(!consent(&empty).approved);
        let back = reduce(
            &empty,
            &PickerEvent::VaultSelection {
                selected: vec!["c".into()],
            },
        );
        assert_eq!(consent(&back).from_vault, Some(vec!["c".to_string()]));

        // The saved items vanished (deleted): back to the live app.
        let gone = reduce(
            &s,
            &PickerEvent::VaultItems {
                count: 0,
                newest_ms: 0,
                now_ms: now,
                selected: vec![],
                password_ids: vec![],
            },
        );
        assert_eq!(review(&gone).unwrap().source, SendSource::Live);
        assert!(!review(&gone).unwrap().offers_vault);
    }

    /// Saved passwords are offered, off by default, never remembered, and
    /// ticking them adds exactly their ids (from the Keyvault) or the chosen
    /// sites' passwords (from the app), needs the secrets acknowledged, and
    /// is refused where there are none.
    #[test]
    fn saved_passwords_are_an_explicit_choice_in_the_review() {
        let now = 1_800_000_000_000_i64;
        let s = review_state();
        // Nothing to send: the toggle does nothing and is not offered.
        let nope = reduce(&s, &PickerEvent::TogglePasswords { value: true });
        assert!(!review(&nope).unwrap().offers_passwords);
        assert!(!consent(&nope).include_passwords);

        // From the Keyvault: its password items are offered, off.
        let s = reduce(
            &s,
            &PickerEvent::VaultItems {
                count: 2,
                newest_ms: now,
                now_ms: now,
                selected: vec!["a".into(), "b".into()],
                password_ids: vec!["p1".into(), "p2".into(), "p3".into()],
            },
        );
        let s = reduce(
            &s,
            &PickerEvent::SendFrom {
                source: SendSource::Vault,
            },
        );
        let rv = review(&s).unwrap();
        assert!(rv.offers_passwords && !rv.include_passwords);
        assert_eq!(rv.passwords_label, "Also send 3 saved passwords");
        let c = consent(&s);
        assert!(!c.include_passwords);
        assert_eq!(c.from_vault, Some(vec!["a".to_string(), "b".to_string()]));

        let on = reduce(&s, &PickerEvent::TogglePasswords { value: true });
        let c = consent(&on);
        assert!(c.include_passwords);
        assert_eq!(
            c.from_vault,
            Some(
                ["a", "b", "p1", "p2", "p3"]
                    .iter()
                    .map(|x| x.to_string())
                    .collect()
            )
        );
        assert!(
            review(&on).unwrap().needs_acknowledgement,
            "a password is a secret that must be acknowledged"
        );
        // Off again, and a different source forgets nothing but also sends none.
        let off = reduce(&on, &PickerEvent::TogglePasswords { value: false });
        assert!(!consent(&off).include_passwords);
    }
}
