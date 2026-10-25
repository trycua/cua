// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! App settings: the global hotkey (and its recorder's formatting), the
//! switcher entry point (notch or menu bar), the switcher theme and the
//! default location for new Spaces. The Tauri webview keeps them in
//! localStorage; native shells use [`load`] / [`save`] on a JSON file.
//!
//! [`page`] is the Settings page both shells draw: Account, General,
//! Privacy and AI agents, each row one line.

use crate::CoreError;
use crate::about::UpdateChannel;
use crate::experiments::{Experiment, Experiments};
use crate::model::Location;
use crate::sign_in::SignInFailure;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The default hotkey.
pub const DEFAULT_HOTKEY: &str = "\u{2318}\u{21e7}Space";

/// Notch island or glass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwitcherTheme {
    /// Opaque black, flush with the notch (default).
    Island,
    /// Translucent, below the notch.
    Glass,
}

/// Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// Global hotkey label.
    pub hotkey: String,
    /// The switcher lives in the menu bar instead of the notch.
    pub menu_bar: bool,
    /// Switcher theme.
    pub theme: SwitcherTheme,
    /// Where New Space starts.
    pub default_location: Location,
    /// Hide the notch over full-screen apps.
    pub hide_in_full_screen: bool,
    /// The newest notification already posted (Unix ms; 0: never polled).
    pub notifications_seen_ms: u64,
    /// The version and build the app last launched as
    /// ([`crate::about::identity`]; `None`: never recorded). A launch as
    /// another one follows an update ([`crate::about::after_launch`]).
    pub last_seen_version: Option<String>,
    /// Which releases the updater offers (Settings, About).
    pub update_channel: UpdateChannel,
    /// Launch at login, as the user chose it (the first run's Done page or
    /// Settings); `None`: never chosen ([`crate::login_item::launch_plan`]).
    pub launch_at_login: Option<bool>,
    /// Settings, Experiments: every switch off unless turned on
    /// ([`crate::experiments`]). Its `webUi` is the New UI as this launch
    /// shows it ([`resolve_new_ui`]); the file keeps only [`Self::new_ui`]'s
    /// "on" there, the value older builds read.
    pub experiments: Experiments,
    /// The New UI as the user chose it (Settings, Experiments, "Use classic
    /// UI"); `None`: never chosen, so
    /// [`crate::experiments::new_ui_default`] decides. An older build's
    /// saved `webUi: true` reads as `Some(true)`; its `false` was only the
    /// saved default, so it reads as never chosen ([`load`]).
    pub new_ui: Option<bool>,
    /// Keyvault copies (import ids) the user dismissed from the notch: it no
    /// longer shows them, nothing is revoked or wiped
    /// ([`crate::keyvault::view::prune_dismissed`] forgets the gone ones).
    pub dismissed_access: Vec<String>,
    /// Keyvault rows may load a site's icon from Google's favicon service
    /// when the source browser had none (Settings, Keyvault; on by default).
    /// Off, only icons read locally are shown.
    pub keyvault_site_icons: bool,
    /// The sites the user sent to each Space last time, per app
    /// ([`crate::teleport::review::remember`]): the review starts from them.
    pub teleport_choices: Vec<crate::teleport::review::RememberedChoice>,
    /// Settings, General: a running Space's desktop streams as soon as it
    /// is opened (on by default); off, it waits for Connect
    /// ([`crate::spaces::cover`]).
    pub auto_connect: bool,
    /// Settings, Teams: the team (organization id) the account works in
    /// when it is in several ([`crate::teams::current`]); `None` or an id
    /// the account left: its first owned team.
    pub current_team_id: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.into(),
            menu_bar: false,
            theme: SwitcherTheme::Island,
            default_location: Location::Local,
            hide_in_full_screen: false,
            notifications_seen_ms: 0,
            last_seen_version: None,
            update_channel: UpdateChannel::Stable,
            launch_at_login: None,
            experiments: Experiments::default(),
            new_ui: None,
            dismissed_access: vec![],
            keyvault_site_icons: true,
            teleport_choices: vec![],
            auto_connect: true,
            current_team_id: None,
        }
    }
}

/// Reads settings; a missing or damaged file gives the defaults.
///
/// The New UI: a file with no choice yet ([`AppSettings::new_ui`]) takes an
/// older build's `experiments.webUi: true` as the choice and its `false`
/// as no choice (every older install saved `false` without anyone choosing
/// it). Until [`resolve_new_ui`] runs with the app's version, the New UI
/// is on only by choice.
pub fn load(path: &Path) -> AppSettings {
    let mut s: AppSettings = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    migrate_defaults(path, &mut s);
    if s.new_ui.is_none() && s.experiments.web_ui {
        s.new_ui = Some(true);
    }
    s.experiments.web_ui = s.new_ui == Some(true);
    s
}

/// The migration marker next to the settings file. The shells round-trip
/// [`AppSettings`] through their own mirrors, so the marker lives in a file of
/// its own that no shell rewrites.
fn migrations_path(path: &Path) -> std::path::PathBuf {
    path.with_file_name("settings-migrations.json")
}

/// Defaults that turned on after releases saved them as off, and switches
/// that were retired (Settings, Advanced no longer has a Sharing or a Your
/// cloud switch). Each one has a key in the migration marker.
const MIGRATIONS: [&str; 4] = [
    "sharingDefaultOn",
    "yourCloudDefaultOn",
    "sharingSwitchRetired",
    "yourCloudSwitchRetired",
];

/// Sharing became on by default in 0.9.1 (CUA-1668) and Your cloud later
/// (a linked cloud showed under Run on only after digging into Settings).
/// Earlier builds saved `false` for anyone who toggled any experiment,
/// without them choosing it, and left no record of an explicit choice. The
/// first load without a switch's marker turns a stored `false` on once and
/// writes the marker (also on a fresh install, so a later "off" is never
/// overridden); from then on the stored value is the user's. A missing key
/// already reads as on. Best effort: a failed write only repeats the
/// migration on the next load.
fn migrate_defaults(path: &Path, s: &mut AppSettings) {
    let marker = migrations_path(path);
    let mut done: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|m| serde_json::from_str(&m).ok())
        .unwrap_or_default();
    let pending = |k: &str| done.get(k) != Some(&serde_json::Value::Bool(true));
    if !MIGRATIONS.iter().any(|k| pending(k)) {
        return;
    }
    // The retired switches: with no switch to turn them back on, a stored
    // `false` (a choice nobody can make or undo any more) reads as on, once.
    // The plan decides who can share or use a cloud, on the server.
    let turn_sharing_on =
        (pending(MIGRATIONS[0]) || pending(MIGRATIONS[2])) && !s.experiments.sharing;
    let turn_cloud_on =
        (pending(MIGRATIONS[1]) || pending(MIGRATIONS[3])) && !s.experiments.your_cloud;
    if turn_sharing_on || turn_cloud_on {
        s.experiments.sharing |= turn_sharing_on;
        s.experiments.your_cloud |= turn_cloud_on;
        if path.exists() && save(path, s).is_err() {
            return;
        }
    }
    for k in MIGRATIONS {
        done.insert(k.into(), true.into());
    }
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&marker, serde_json::Value::Object(done).to_string());
}

/// `settings` with the New UI as this launch shows it: the user's choice,
/// else [`crate::experiments::new_ui_default`] for the update channel and
/// the running app's `version`. Again after the channel changes.
pub fn resolve_new_ui(settings: &AppSettings, version: &str) -> AppSettings {
    let mut s = settings.clone();
    s.experiments.web_ui = s
        .new_ui
        .unwrap_or_else(|| crate::experiments::new_ui_default(s.update_channel, version));
    s
}

/// The update channel of an install's first launch, from the one in the
/// settings file (`channel`): Beta for a prerelease (someone who installed
/// a beta keeps getting betas, and the New UI default that comes with
/// them), else unchanged.
pub fn first_launch_channel(channel: UpdateChannel, version: &str) -> UpdateChannel {
    if version.split('+').next().unwrap_or("").contains('-') {
        UpdateChannel::Beta
    } else {
        channel
    }
}

/// The settings after a Settings, Experiments row's choice: the switches
/// ([`crate::experiments::choose`]) and, for the New UI's row ("Use classic
/// UI"), the user's choice ([`AppSettings::new_ui`]), which later launches
/// keep whatever the default becomes.
pub fn choose_experiment(settings: &AppSettings, row: &str, option: &str) -> AppSettings {
    let mut s = settings.clone();
    s.experiments = crate::experiments::choose(&s.experiments, row, option);
    if let Some(on) = crate::experiments::new_ui_choice(row, option) {
        s.new_ui = Some(on);
    }
    s
}

/// Writes settings atomically (temp file, then rename). The New UI is
/// saved as chosen ([`AppSettings::new_ui`]), never as its default: the
/// default follows the version and channel of each launch.
pub fn save(path: &Path, settings: &AppSettings) -> Result<(), CoreError> {
    let mut stored = settings.clone();
    stored.experiments.web_ui = stored.new_ui == Some(true);
    let json = serde_json::to_string_pretty(&stored).map_err(|e| CoreError::Io(e.to_string()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CoreError::Io(e.to_string()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| CoreError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| CoreError::Io(e.to_string()))
}

/// A pressed key combination.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyCombo {
    /// The key (`" "`, `"k"`, `"ArrowUp"`, ...).
    pub key: String,
    /// Command.
    pub meta_key: bool,
    /// Control.
    pub ctrl_key: bool,
    /// Option.
    pub alt_key: bool,
    /// Shift.
    pub shift_key: bool,
}

fn label_for_key(key: &str) -> String {
    match key {
        " " | "Spacebar" => "Space".into(),
        "ArrowUp" => "\u{2191}".into(),
        "ArrowDown" => "\u{2193}".into(),
        "ArrowLeft" => "\u{2190}".into(),
        "ArrowRight" => "\u{2192}".into(),
        "Escape" => "Esc".into(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.into(),
    }
}

/// `⌘⇧Space`-style label, or none while only modifiers are held or no
/// modifier accompanies the key (a global hotkey needs one).
pub fn format_hotkey(c: &KeyCombo) -> Option<String> {
    if matches!(c.key.as_str(), "Meta" | "Control" | "Alt" | "Shift") {
        return None;
    }
    if !(c.meta_key || c.ctrl_key || c.alt_key || c.shift_key) {
        return None;
    }
    let mut out = String::new();
    if c.meta_key {
        out.push('\u{2318}');
    }
    if c.ctrl_key {
        out.push('\u{2303}');
    }
    if c.alt_key {
        out.push('\u{2325}');
    }
    if c.shift_key {
        out.push('\u{21e7}');
    }
    out.push_str(&label_for_key(&c.key));
    Some(out)
}

/// What telemetry's docs say is collected.
/// Where the Sign in page's Teams line links: the account app, which
/// administers teams (Settings shows the real Teams section, [`crate::teams`]).
pub const TEAMS_URL: &str = "https://app.cua.ai/welcome";

/// The Sign in page's Teams line's label.
pub const TEAMS_LABEL: &str = "Teams";

/// Its value.
pub const TEAMS_VALUE: &str = "manage them at app.cua.ai";

/// Its link.
pub const TEAMS_BUTTON: &str = "Open the account app";

pub const TELEMETRY_DOCS_URL: &str = "https://cua.ai/docs/cua-sdk/concepts/telemetry";

/// Where the Account section's sign-in stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum SignInPhase {
    /// Nothing started.
    #[default]
    Idle,
    /// Asking for a code.
    Starting,
    /// The browser is open.
    Waiting {
        /// The code to confirm there, when the flow has one.
        #[serde(default)]
        user_code: Option<String>,
        /// A browser opened on this computer (false: the page is to be
        /// opened by hand, here or on another device).
        #[serde(default = "yes")]
        opened: bool,
    },
    /// It failed ([`crate::sign_in::failed`] makes it).
    Failed {
        /// Why ([`crate::sign_in::SignInFailure`]).
        #[serde(default)]
        reason: SignInFailure,
        /// The line shown: "Sign-in didn't finish." and the reason.
        message: String,
        /// The error's own words, behind Details (never shown by default).
        #[serde(default)]
        detail: Option<String>,
    },
}

fn yes() -> bool {
    true
}

/// The usage-telemetry switch's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TelemetryInput {
    /// Usage data is shared.
    pub enabled: bool,
    /// The environment decides (`DO_NOT_TRACK`, `CUA_TELEMETRY`, CI): who.
    pub locked_by: Option<String>,
    /// The first-run usage notice was already shown on this machine (by
    /// `cua`, or an earlier run of the app): events may be sent before
    /// Welcome is left.
    pub notice_shown: bool,
}

/// What the Settings page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsInput {
    /// The signed-in account.
    pub identity: Option<String>,
    /// Cua Cloud through client credentials (an API key): its client id.
    pub api_key_client: Option<String>,
    /// The sign-in in progress.
    pub sign_in: SignInPhase,
    /// Signing out is possible (a user session).
    pub can_sign_out: bool,
    /// The Spaces tab is hidden from the notch (the menu bar item is the
    /// entry point).
    pub menu_bar: bool,
    /// Where New Space starts.
    pub default_location: Location,
    /// The environment sets the default location: which variable.
    pub location_locked_by: Option<String>,
    /// The telemetry switch (none until read).
    pub telemetry: Option<TelemetryInput>,
    /// Coding agents (none while detecting).
    pub agents: Option<Vec<crate::agents::AgentSettingsRow>>,
    /// "Configure all" is running.
    pub agents_busy: bool,
    /// Agents with an action running.
    pub agents_pending: Vec<String>,
    /// Teams from the Spaces API (the host reads them), with the account
    /// app's URL. None: the Teams section shows the production account
    /// app's links only.
    #[serde(default)]
    pub teams: Option<crate::teams::TeamsInput>,
    /// The account's Cua Cloud billing, once read (signed in).
    pub billing: Option<crate::billing::BillingStatus>,
    /// Launch at login, once the system was asked (none: no rows).
    pub login_item: Option<crate::login_item::LoginItemInput>,
    /// Settings, Experiments (what the page mentions follows them).
    pub experiments: Experiments,
    /// The Keyvault's auto-wipe, once the broker told it (none: no
    /// Keyvault section).
    pub keyvault_auto_wipe: Option<bool>,
    /// The unlock prompt shows (false once the user chose "Never ask
    /// again"); none until the broker told it.
    pub keyvault_unlock_prompt: Option<bool>,
    /// Load site icons from Google when the browser had none
    /// ([`AppSettings::keyvault_site_icons`]).
    #[serde(default = "default_true")]
    pub keyvault_site_icons: bool,
    /// The Keyvault's protection facts (Touch ID, the daemon's signature),
    /// shown in the section.
    #[serde(default)]
    pub keyvault_protection: Vec<crate::spaces::sidebar::Fact>,
    /// "Connect to the desktop automatically" (none: no row; a shell
    /// without the preview cover leaves it out).
    pub auto_connect: Option<bool>,
    /// Which Lume macOS Spaces run on (`runtime.lume`: `auto`, `builtin`,
    /// `system`); none: no Runtimes section.
    #[serde(default)]
    pub lume_source: Option<String>,
    /// Which engine local Linux Spaces run on (`runtime.linux`: `auto`,
    /// `builtin`, `system`); none: no Linux row.
    #[serde(default)]
    pub linux_source: Option<String>,
    /// Overflow into your clouds when this machine is full
    /// (`placement.overflow`); none: no row.
    #[serde(default)]
    pub overflow: Option<bool>,
}

impl Default for SettingsInput {
    fn default() -> Self {
        Self {
            identity: None,
            api_key_client: None,
            sign_in: SignInPhase::Idle,
            can_sign_out: false,
            menu_bar: false,
            default_location: Location::Local,
            location_locked_by: None,
            telemetry: None,
            agents: None,
            agents_busy: false,
            agents_pending: vec![],
            teams: None,
            billing: None,
            login_item: None,
            experiments: Experiments::default(),
            keyvault_auto_wipe: None,
            keyvault_unlock_prompt: None,
            keyvault_site_icons: true,
            keyvault_protection: vec![],
            auto_connect: None,
            lume_source: None,
            linux_source: None,
            overflow: None,
        }
    }
}

fn default_true() -> bool {
    true
}

/// The site icons switch's label.
pub const SITE_ICONS_LABEL: &str = "Load site icons from Google";

/// The line under it.
pub const SITE_ICONS_NOTE: &str =
    "Sends only the site's domain. Off uses your browser's own icons.";

/// The auto-connect switch's label.
pub const AUTO_CONNECT_LABEL: &str = "Connect to the desktop automatically";

/// The auto-connect switch: row id `auto-connect`, options `on` and `off`.
pub fn auto_connect_row(on: bool) -> SettingsRow {
    let mut r = row("auto-connect", SettingsRowKind::Toggle, AUTO_CONNECT_LABEL);
    r.options = vec![opt("on", "On", on), opt("off", "Off", !on)];
    r.help = Some(if on {
        "Opening a running Space shows its live desktop".into()
    } else {
        "Opening a running Space shows a preview and a Connect button".into()
    });
    r
}

/// The overflow switch's label.
pub const OVERFLOW_LABEL: &str = "Overflow into your clouds when this Mac is full";

/// Settings, General: whether Spaces created without a location go to your
/// linked clouds once this machine is full (`placement.overflow`; only with
/// a budget set on the Machines page).
pub fn overflow_row(on: bool) -> SettingsRow {
    let mut r = row(
        "placement-overflow",
        SettingsRowKind::Toggle,
        OVERFLOW_LABEL,
    );
    r.options = vec![opt("on", "On", on), opt("off", "Off", !on)];
    r.help = Some(if on {
        "With a linked cloud and a budget, new Spaces go there when this Mac is full".into()
    } else {
        "New Spaces stay on this Mac".into()
    });
    r
}

/// The Keyvault auto-wipe switch's label.
pub const AUTO_WIPE_LABEL: &str = "Wipe access from Spaces automatically";

/// The line under it.
pub const AUTO_WIPE_NOTE: &str = "When off, sign-ins stay in a Space until you wipe them.";

/// The unlock prompt switch's label.
pub const UNLOCK_PROMPT_LABEL: &str = "Explain unattended access before allowing it";

/// The line under it.
pub const UNLOCK_PROMPT_NOTE: &str = "Unlocking an item always asks for Touch ID. Turn this on to see the explanation again after choosing Never ask again.";

/// The Keyvault section: the auto-wipe switch (off by default) and its
/// line, the unlock prompt switch (on by default; "Never ask again" turns it
/// off and this turns it back on), and the protection facts. Row ids
/// `keyvault-auto-wipe`, `keyvault-unlock-prompt` and `keyvault-protection:<label>`;
/// the switches' options are `on` and `off`.
pub fn keyvault_section(
    auto_wipe: bool,
    unlock_prompt: Option<bool>,
    site_icons: bool,
    protection: &[crate::spaces::sidebar::Fact],
) -> SettingsSection {
    let mut toggle = row(
        "keyvault-auto-wipe",
        SettingsRowKind::Toggle,
        AUTO_WIPE_LABEL,
    );
    toggle.options = vec![opt("on", "On", auto_wipe), opt("off", "Off", !auto_wipe)];
    toggle.help = Some(if auto_wipe {
        "Turning it off asks for Touch ID".into()
    } else {
        "Wipes sign-ins after an hour (15 minutes for identity providers)".into()
    });
    let mut rows = vec![
        toggle,
        row(
            "keyvault-auto-wipe-note",
            SettingsRowKind::Note,
            AUTO_WIPE_NOTE,
        ),
    ];
    if let Some(shows) = unlock_prompt {
        let mut t = row(
            "keyvault-unlock-prompt",
            SettingsRowKind::Toggle,
            UNLOCK_PROMPT_LABEL,
        );
        t.options = vec![opt("on", "On", shows), opt("off", "Off", !shows)];
        t.help = Some(if shows {
            "Choosing Never ask again on the prompt turns this off".into()
        } else {
            "You chose Never ask again. Turn this on to see the prompt again".into()
        });
        rows.push(t);
        rows.push(row(
            "keyvault-unlock-prompt-note",
            SettingsRowKind::Note,
            UNLOCK_PROMPT_NOTE,
        ));
    }
    let mut icons = row(
        "keyvault-site-icons",
        SettingsRowKind::Toggle,
        SITE_ICONS_LABEL,
    );
    icons.options = vec![opt("on", "On", site_icons), opt("off", "Off", !site_icons)];
    rows.push(icons);
    rows.push(row(
        "keyvault-site-icons-note",
        SettingsRowKind::Note,
        SITE_ICONS_NOTE,
    ));
    for f in protection {
        let mut r = row(
            &format!("keyvault-protection:{}", f.label),
            SettingsRowKind::Text,
            &f.label,
        );
        r.value = Some(f.value.clone());
        rows.push(r);
    }
    SettingsSection {
        id: "keyvault".into(),
        title: "Keyvault".into(),
        button: None,
        button_enabled: false,
        button_help: None,
        rows,
    }
}

/// How a Settings row draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsRowKind {
    /// Label, value and an optional button.
    Text,
    /// Label and a segmented choice.
    Choice,
    /// A muted line (with an optional link).
    Note,
    /// An error line.
    Error,
    /// Label and a text field (`value` is its text).
    Field,
    /// Label and a secure text field (`value` is its text).
    Secret,
    /// A label over read-only text to copy (`value`), with a Copy button.
    Prompt,
    /// A small link-style button (the label).
    Link,
    /// Label and a switch: `options` are `on` and `off`, the active one is
    /// its state (a choice drawn as a switch).
    Toggle,
}

/// One option of a choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsOption {
    /// `show`, `hide`, `local`, `cloud`, `on`, `off`.
    pub id: String,
    /// Label.
    pub label: String,
    /// Chosen.
    pub active: bool,
}

/// One Settings row (one line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsRow {
    /// Stable id (`account`, `sign-in`, `notch`, `agent:codex`, ...).
    pub id: String,
    /// How it draws.
    pub kind: SettingsRowKind,
    /// Label (or the note's text).
    pub label: String,
    /// Muted value.
    pub value: Option<String>,
    /// A button's label.
    pub button: Option<String>,
    /// A choice's options.
    pub options: Vec<SettingsOption>,
    /// The button or choice can be used.
    pub enabled: bool,
    /// Tooltip.
    pub help: Option<String>,
    /// A note's link: label.
    pub link_label: Option<String>,
    /// A note's link: URL (or what a Text row's button opens).
    pub link_url: Option<String>,
    /// A field's placeholder.
    pub placeholder: Option<String>,
}

/// A titled group of rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSection {
    /// `account`, `general`, `privacy`, `agents`.
    pub id: String,
    /// "Account", "General", "Privacy", "AI agents".
    pub title: String,
    /// A button in the header.
    pub button: Option<String>,
    /// That button can be used.
    pub button_enabled: bool,
    /// Its tooltip.
    pub button_help: Option<String>,
    /// Rows.
    pub rows: Vec<SettingsRow>,
}

/// The Settings page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsPage {
    /// "Settings".
    pub title: String,
    /// Account, General, Privacy (once read), AI agents.
    pub sections: Vec<SettingsSection>,
}

pub(crate) fn row(id: &str, kind: SettingsRowKind, label: &str) -> SettingsRow {
    SettingsRow {
        id: id.into(),
        kind,
        label: label.into(),
        value: None,
        button: None,
        options: vec![],
        enabled: true,
        help: None,
        link_label: None,
        link_url: None,
        placeholder: None,
    }
}

fn opt(id: &str, label: &str, active: bool) -> SettingsOption {
    SettingsOption {
        id: id.into(),
        label: label.into(),
        active,
    }
}

/// The Settings page for `input`.
pub fn page(input: &SettingsInput) -> SettingsPage {
    use SettingsRowKind::*;
    let mut account = Vec::new();
    let identity = input.identity.clone().filter(|i| !i.is_empty());
    if let Some(id) = identity {
        let mut r = row("account", Text, &id);
        if input.can_sign_out {
            r.button = Some("Sign out".into());
        }
        account.push(r);
        // Billing: the credit left, and the website's billing page (the
        // apps take no payment details). Row id `billing`; the button
        // opens `link_url`. Not shown while the apps do not offer Cua Cloud.
        if let Some(b) = input
            .billing
            .as_ref()
            .filter(|b| b.billing_enabled && crate::billing::BILLING_SHOWN)
        {
            let mut r = row("billing", Text, "Billing");
            r.value = Some(crate::billing::billing_line(b));
            r.button = Some(crate::billing::MANAGE_BILLING.into());
            r.link_url = Some(crate::billing::BILLING_URL.into());
            r.enabled = true;
            account.push(r);
        }
    } else if let SignInPhase::Waiting { user_code, .. } = &input.sign_in {
        let mut r = row(
            "sign-in-code",
            Text,
            if user_code.is_some() {
                "Enter this code in your browser"
            } else {
                "Finish signing in in your browser."
            },
        );
        r.value = user_code.clone();
        r.button = Some("Cancel".into());
        account.push(r);
    } else {
        if let Some(client) = input.api_key_client.clone().filter(|c| !c.is_empty()) {
            let mut r = row("api-key", Text, "Signed in via API key");
            r.value = Some(client);
            account.push(r);
        }
        let mut r = row("sign-in", Text, "Cua account");
        r.button = Some(
            match &input.sign_in {
                SignInPhase::Failed { .. } => "Try again",
                SignInPhase::Starting => "Starting\u{2026}",
                _ => "Sign in to Cua",
            }
            .into(),
        );
        r.enabled = input.sign_in != SignInPhase::Starting;
        account.push(r);
        if let SignInPhase::Failed { message, .. } = &input.sign_in {
            account.push(row("sign-in-error", Error, message));
        }
    }
    let mut notch = row("notch", Choice, "Spaces tab in the notch");
    notch.options = vec![
        opt("show", "Show", !input.menu_bar),
        opt("hide", "Hide", input.menu_bar),
    ];
    // Launch at login first, as in Tailscale's General settings.
    let mut general = input
        .login_item
        .as_ref()
        .map(|l| crate::login_item::rows_with(l, input.experiments.cua_volume))
        .unwrap_or_default();
    general.push(notch);
    if let Some(on) = input.auto_connect {
        general.push(auto_connect_row(on));
    }
    if let Some(on) = input.overflow {
        general.push(overflow_row(on));
    }
    // Where New Space starts: only a choice while the apps offer Cua Cloud.
    if crate::model::CLOUD_SPACES_OFFERED {
        let mut location = row("default-location", Choice, "New Spaces run on");
        location.options = vec![
            opt(
                "local",
                Location::Local.label(),
                input.default_location == Location::Local,
            ),
            opt(
                "cloud",
                Location::Cloud.label(),
                input.default_location == Location::Cloud,
            ),
        ];
        if let Some(var) = input.location_locked_by.clone().filter(|v| !v.is_empty()) {
            location.enabled = false;
            location.help = Some(format!("Set by {var}"));
        }
        general.push(location);
    }
    let mut welcome = row("welcome", Text, "Welcome");
    welcome.button = Some("Show again".into());
    general.push(welcome);

    let mut sections = vec![
        SettingsSection {
            id: "account".into(),
            title: "Account".into(),
            button: None,
            button_enabled: false,
            button_help: None,
            rows: account,
        },
        SettingsSection {
            id: "teams".into(),
            title: crate::teams::TEAMS_TITLE.into(),
            button: None,
            button_enabled: false,
            button_help: None,
            // Each team with its members and pending invites, then the
            // account app's links (invites, roles, billing).
            rows: crate::teams::rows(&input.teams.clone().unwrap_or_else(|| {
                crate::teams::TeamsInput {
                    account_app_url: crate::teams::ACCOUNT_APP_URL.into(),
                    state: if input.identity.is_some() {
                        crate::teams::TeamsState::Unread
                    } else {
                        crate::teams::TeamsState::SignedOut
                    },
                    current_team_id: None,
                }
            })),
        },
        SettingsSection {
            id: "general".into(),
            title: "General".into(),
            button: None,
            button_enabled: false,
            button_help: None,
            rows: general,
        },
    ];

    if input.lume_source.is_some() || input.linux_source.is_some() {
        sections.push(runtimes_section(
            input.lume_source.as_deref(),
            input.linux_source.as_deref(),
        ));
    }

    if let Some(t) = &input.telemetry {
        let mut share = row("telemetry", Choice, "Share usage data to help improve Cua");
        share.options = vec![opt("on", "On", t.enabled), opt("off", "Off", !t.enabled)];
        if let Some(by) = t.locked_by.clone().filter(|b| !b.is_empty()) {
            share.enabled = false;
            share.help = Some(format!("Set by {by}"));
        }
        let mut note = row(
            "telemetry-note",
            Note,
            "Which features you use and when something goes wrong. Linked to your account through a random ID while you're signed in. Never your files, what you type or your screen.",
        );
        note.link_label = Some("What is collected".into());
        note.link_url = Some(TELEMETRY_DOCS_URL.into());
        sections.push(SettingsSection {
            id: "privacy".into(),
            title: "Privacy".into(),
            button: None,
            button_enabled: false,
            button_help: None,
            rows: vec![share, note],
        });
    }

    if let Some(on) = input.keyvault_auto_wipe {
        sections.push(keyvault_section(
            on,
            input.keyvault_unlock_prompt,
            input.keyvault_site_icons,
            &input.keyvault_protection,
        ));
    }

    let agents = input.agents.clone().unwrap_or_default();
    let any_installed = agents.iter().any(|a| a.installed);
    let rows = agents
        .iter()
        .map(|a| {
            let pending = input.agents_pending.iter().any(|p| p == &a.agent);
            let mut r = row(&format!("agent:{}", a.agent), Text, &a.name);
            if a.installed {
                r.value = Some(if pending {
                    "working\u{2026}".into()
                } else {
                    a.detail.clone()
                });
                if !pending {
                    r.button = Some(if a.configured { "Remove" } else { "Configure" }.into());
                }
                r.enabled = !input.agents_busy;
            } else {
                r.enabled = false;
            }
            r
        })
        .collect();
    sections.push(SettingsSection {
        id: "agents".into(),
        title: "AI agents".into(),
        button: Some(
            if input.agents_busy {
                "Configuring\u{2026}"
            } else {
                "Configure all detected agents"
            }
            .into(),
        ),
        button_enabled: !input.agents_busy && any_installed,
        button_help: Some("Add the cua skills and MCP server to every detected agent".into()),
        rows,
    });

    SettingsPage {
        title: "Settings".into(),
        sections,
    }
}

/// The help under Settings, Runtimes, macOS VMs.
pub const MACOS_RUNTIME_HELP: &str =
    "Built-in is Cua\u{2019}s signed Lume, downloaded the first time you need it (6 MB).";

/// The help under Settings, Runtimes, Linux.
pub const LINUX_RUNTIME_HELP: &str =
    "Built-in is a gVisor Linux VM, downloaded the first time you need it (480 MB).";

/// Settings, Runtimes: one row per local runtime Cua can provide itself,
/// each Automatic, Built-in or System.
/// * macOS VMs (`macos-runtime`, `runtime.lume`'s values): Built-in is
///   Cua's signed Lume, set up on first use; System this Mac's own Lume.
/// * Linux (`linux-runtime`, `runtime.linux`'s values): Built-in is Cua's
///   own VM with Docker and gVisor, set up on first use; System this Mac's
///   own container engine; Automatic Docker when it runs, else built-in.
pub fn runtimes_section(lume_source: Option<&str>, linux_source: Option<&str>) -> SettingsSection {
    use SettingsRowKind::*;
    let choice = |id: &str, label: &str, source: &str| {
        let source = match source {
            "builtin" | "system" => source,
            _ => "auto",
        };
        let mut r = row(id, Choice, label);
        r.options = vec![
            opt("auto", "Automatic", source == "auto"),
            opt("builtin", "Built-in", source == "builtin"),
            opt("system", "System", source == "system"),
        ];
        r
    };
    let mut rows = vec![];
    if let Some(source) = lume_source {
        rows.push(choice("macos-runtime", "macOS VMs", source));
        rows.push(row("macos-runtime-note", Note, MACOS_RUNTIME_HELP));
    }
    if let Some(source) = linux_source {
        rows.push(choice("linux-runtime", "Linux", source));
        rows.push(row("linux-runtime-note", Note, LINUX_RUNTIME_HELP));
    }
    SettingsSection {
        id: "runtimes".into(),
        title: "Runtimes".into(),
        button: None,
        button_enabled: false,
        button_help: None,
        rows,
    }
}

/// The Settings page with Settings, Storage (the Cua Volume's store,
/// Finder volume and cache, [`crate::drive_settings::storage_section`])
/// after General, while the Cua Volume experiment is on. Off, Storage is
/// left out (and nothing it set up is touched: a mounted volume stays
/// mounted).
pub fn with_storage(
    page: &SettingsPage,
    storage: &SettingsSection,
    experiments: &Experiments,
) -> SettingsPage {
    let shown = experiments.is_on(Experiment::CuaVolume);
    let mut sections = Vec::with_capacity(page.sections.len() + 1);
    for s in &page.sections {
        sections.push(s.clone());
        if shown && s.id == "general" {
            sections.push(storage.clone());
        }
    }
    if shown && !sections.iter().any(|s| s.id == storage.id) {
        sections.push(storage.clone());
    }
    SettingsPage {
        title: page.title.clone(),
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtimes_section_shows_the_lume_choice() {
        let p = page(&SettingsInput::default());
        assert!(
            !p.sections.iter().any(|s| s.id == "runtimes"),
            "not until read"
        );
        let page = page(&SettingsInput {
            lume_source: Some("system".into()),
            ..Default::default()
        });
        let r = &page
            .sections
            .iter()
            .find(|s| s.id == "runtimes")
            .unwrap()
            .rows[0];
        assert_eq!(r.id, "macos-runtime");
        let active: Vec<_> = r
            .options
            .iter()
            .filter(|o| o.active)
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(active, ["system"]);
        let ids: Vec<_> = r.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(ids, ["Automatic", "Built-in", "System"]);
    }

    #[test]
    fn the_runtimes_section_shows_the_linux_choice() {
        let page = page(&SettingsInput {
            lume_source: Some("auto".into()),
            linux_source: Some("builtin".into()),
            ..Default::default()
        });
        let s = page.sections.iter().find(|s| s.id == "runtimes").unwrap();
        let ids: Vec<_> = s.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "macos-runtime",
                "macos-runtime-note",
                "linux-runtime",
                "linux-runtime-note"
            ]
        );
        let linux = &s.rows[2];
        assert_eq!(linux.label, "Linux");
        let active: Vec<_> = linux
            .options
            .iter()
            .filter(|o| o.active)
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(active, ["builtin"]);
        // An unknown word reads as Automatic.
        let r = runtimes_section(None, Some("docker"));
        assert_eq!(r.rows[0].id, "linux-runtime");
        assert!(r.rows[0].options[0].active);
    }

    #[test]
    fn hotkeys_need_a_modifier() {
        let c = KeyCombo {
            key: " ".into(),
            meta_key: true,
            shift_key: true,
            ..Default::default()
        };
        assert_eq!(format_hotkey(&c).as_deref(), Some(DEFAULT_HOTKEY));
        assert_eq!(
            format_hotkey(&KeyCombo {
                key: "k".into(),
                ..Default::default()
            }),
            None
        );
    }

    #[test]
    fn settings_round_trip_and_default_on_damage() {
        let dir =
            std::env::temp_dir().join(format!("cua-app-core-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let s = AppSettings {
            menu_bar: true,
            default_location: Location::Cloud,
            last_seen_version: Some("0.2.0 (0.2.0.1)".into()),
            update_channel: UpdateChannel::Beta,
            launch_at_login: Some(false),
            ..Default::default()
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
        // A file from a build before the updater: the new fields default.
        std::fs::write(&path, r#"{"menuBar":true,"notificationsSeenMs":5}"#).unwrap();
        let old = load(&path);
        assert!(old.menu_bar);
        assert_eq!(old.last_seen_version, None);
        assert_eq!(old.update_channel, UpdateChannel::Stable);
        assert_eq!(old.launch_at_login, None, "never chosen");
        std::fs::write(&path, "{nope").unwrap();
        assert_eq!(load(&path), AppSettings::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_page_lists_account_general_privacy_agents() {
        let p = page(&SettingsInput::default());
        let ids: Vec<_> = p.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["account", "teams", "general", "agents"]);
        let p = page(&SettingsInput {
            identity: Some("ada@example.com".into()),
            can_sign_out: true,
            telemetry: Some(TelemetryInput::default()),
            ..Default::default()
        });
        let ids: Vec<_> = p.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["account", "teams", "general", "privacy", "agents"]);
        assert_eq!(p.sections[0].rows[0].button.as_deref(), Some("Sign out"));
        let sign_in: SignInPhase =
            serde_json::from_str(r#"{"kind":"waiting","userCode":"AB-CD"}"#).unwrap();
        assert_eq!(
            sign_in,
            SignInPhase::Waiting {
                user_code: Some("AB-CD".into()),
                opened: true,
            }
        );
    }

    #[test]
    fn the_keyvault_section_offers_auto_wipe_off_by_default() {
        let ids =
            |p: &SettingsPage| -> Vec<String> { p.sections.iter().map(|s| s.id.clone()).collect() };
        // Not told yet (no broker): no section.
        assert!(!ids(&page(&SettingsInput::default())).contains(&"keyvault".to_string()));
        let p = page(&SettingsInput {
            telemetry: Some(TelemetryInput::default()),
            keyvault_auto_wipe: Some(false),
            ..Default::default()
        });
        assert_eq!(
            ids(&p),
            [
                "account", "teams", "general", "privacy", "keyvault", "agents"
            ]
        );
        let kv = &p.sections[4];
        assert_eq!(kv.title, "Keyvault");
        let toggle = &kv.rows[0];
        assert_eq!(
            (toggle.id.as_str(), toggle.kind, toggle.label.as_str()),
            (
                "keyvault-auto-wipe",
                SettingsRowKind::Toggle,
                AUTO_WIPE_LABEL
            )
        );
        let on = |r: &SettingsRow| r.options.iter().find(|o| o.id == "on").unwrap().active;
        assert!(!on(toggle), "off by default");
        assert!(on(&keyvault_section(true, None, true, &[]).rows[0]));
        assert_eq!(kv.rows[1].kind, SettingsRowKind::Note);
        assert!(!AUTO_WIPE_NOTE.contains('\u{2014}'), "no em dashes in copy");
        assert!(
            !UNLOCK_PROMPT_NOTE.contains('\u{2014}') && !UNLOCK_PROMPT_LABEL.contains('\u{2014}')
        );
        // Without the setting told, no prompt row; told, it is a switch that
        // is on until "Never ask again" turns it off, with the protection
        // facts under it.
        assert!(!kv.rows.iter().any(|r| r.id == "keyvault-unlock-prompt"));
        let fact = |l: &str, v: &str| crate::spaces::sidebar::Fact {
            label: l.into(),
            value: v.into(),
            copy: None,
            help: None,
            warning: None,
        };
        let with = keyvault_section(
            false,
            Some(false),
            true,
            &[fact("Touch ID", "Asked by the Cua daemon")],
        );
        let prompt = with
            .rows
            .iter()
            .find(|r| r.id == "keyvault-unlock-prompt")
            .unwrap();
        assert!(!on(prompt), "Never ask again turned it off");
        assert!(on(&keyvault_section(false, Some(true), true, &[]).rows[2]));
        let last = with.rows.last().unwrap();
        assert_eq!(last.id, "keyvault-protection:Touch ID");
        assert_eq!(last.value.as_deref(), Some("Asked by the Cua daemon"));
        // The site icons switch is on by default and named for Google.
        let icons = with
            .rows
            .iter()
            .find(|r| r.id == "keyvault-site-icons")
            .unwrap();
        assert!(on(icons));
        assert_eq!(icons.label, "Load site icons from Google");
        assert!(!on(keyvault_section(false, None, false, &[])
            .rows
            .iter()
            .find(|r| r.id == "keyvault-site-icons")
            .unwrap()));
        assert!(AppSettings::default().keyvault_site_icons);
        let old: AppSettings = serde_json::from_str(r#"{"menuBar":true}"#).unwrap();
        assert!(old.keyvault_site_icons, "a missing field is on");
        // A settings file from before dismissals: none dismissed.
        let old: AppSettings = serde_json::from_str(r#"{"menuBar":true}"#).unwrap();
        assert!(old.dismissed_access.is_empty());
    }

    #[test]
    fn launch_at_login_leads_general_once_read() {
        use crate::login_item::{LABEL, LoginItemInput, LoginItemStatus};
        let general = |input: &SettingsInput| {
            page(input)
                .sections
                .into_iter()
                .find(|s| s.id == "general")
                .unwrap()
                .rows
        };
        assert_eq!(general(&SettingsInput::default())[0].id, "notch");
        let rows = general(&SettingsInput {
            login_item: Some(LoginItemInput {
                status: LoginItemStatus::Enabled,
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(
            (rows[0].id.as_str(), rows[0].kind, rows[0].label.as_str()),
            ("launch-at-login", SettingsRowKind::Toggle, LABEL)
        );
        assert_eq!(rows[1].id, "launch-at-login-note");
        assert_eq!(rows[2].id, "notch");
    }

    #[test]
    fn auto_connect_is_on_by_default_and_follows_the_notch_row() {
        assert!(AppSettings::default().auto_connect);
        // A settings file from before the switch: on.
        let old: AppSettings = serde_json::from_str(r#"{"menuBar":true}"#).unwrap();
        assert!(old.auto_connect);
        let off: AppSettings = serde_json::from_str(r#"{"autoConnect":false}"#).unwrap();
        assert!(!off.auto_connect);
        let general = |input: &SettingsInput| {
            page(input)
                .sections
                .into_iter()
                .find(|s| s.id == "general")
                .unwrap()
                .rows
        };
        // Not passed (a shell without the cover): no row.
        assert!(
            !general(&SettingsInput::default())
                .iter()
                .any(|r| r.id == "auto-connect")
        );
        // The overflow switch shows when the app passes it, with its state.
        assert!(
            !general(&SettingsInput::default())
                .iter()
                .any(|r| r.id == "placement-overflow")
        );
        let rows = general(&SettingsInput {
            overflow: Some(false),
            ..Default::default()
        });
        let o = rows.iter().find(|r| r.id == "placement-overflow").unwrap();
        assert!(o.options.iter().any(|x| x.id == "off" && x.active));
        let rows = general(&SettingsInput {
            auto_connect: Some(true),
            ..Default::default()
        });
        let i = rows.iter().position(|r| r.id == "auto-connect").unwrap();
        assert_eq!(rows[i - 1].id, "notch");
        let r = &rows[i];
        assert_eq!(
            (r.kind, r.label.as_str()),
            (SettingsRowKind::Toggle, AUTO_CONNECT_LABEL)
        );
        let on = |r: &SettingsRow| r.options.iter().find(|o| o.id == "on").unwrap().active;
        assert!(on(r));
        assert!(!on(&auto_connect_row(false)));
        assert!(!AUTO_CONNECT_LABEL.contains('\u{2014}'));
    }

    /// Cua Volume off: no Storage section, and the launch-at-login line
    /// does not promise a Volume; on: Storage after General.
    #[test]
    fn the_volume_experiment_decides_the_storage_section() {
        use crate::login_item::{LoginItemInput, LoginItemStatus, NOTE, NOTE_WITHOUT_VOLUME};
        let storage = SettingsSection {
            id: "storage".into(),
            title: "Storage".into(),
            button: None,
            button_enabled: false,
            button_help: None,
            rows: vec![],
        };
        let input = SettingsInput {
            login_item: Some(LoginItemInput {
                status: LoginItemStatus::Enabled,
                ..Default::default()
            }),
            ..Default::default()
        };
        let ids =
            |p: &SettingsPage| -> Vec<String> { p.sections.iter().map(|s| s.id.clone()).collect() };
        let note = |p: &SettingsPage| {
            p.sections[2]
                .rows
                .iter()
                .find(|r| r.id == "launch-at-login-note")
                .map(|r| r.label.clone())
        };
        let off = with_storage(&page(&input), &storage, &input.experiments);
        assert_eq!(ids(&off), ["account", "teams", "general", "agents"]);
        assert_eq!(note(&off).as_deref(), Some(NOTE_WITHOUT_VOLUME));
        let on = SettingsInput {
            experiments: Experiments {
                cua_volume: true,
                ..Default::default()
            },
            ..input
        };
        let p = with_storage(&page(&on), &storage, &on.experiments);
        assert_eq!(
            ids(&p),
            ["account", "teams", "general", "storage", "agents"]
        );
        assert_eq!(note(&p).as_deref(), Some(NOTE));
        // A settings file from before experiments: all off.
        let old: AppSettings = serde_json::from_str(r#"{"menuBar":true}"#).unwrap();
        assert_eq!(old.experiments, Experiments::default());
    }

    /// A settings file in a fresh temp dir.
    fn temp_settings(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("cua-app-core-new-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        (dir, path)
    }

    #[test]
    fn an_older_builds_saved_new_ui_migrates_once() {
        let (dir, path) = temp_settings("migrate");
        let beta = "0.8.1-beta.1";
        // Every older install saved `webUi: false` without anyone choosing it:
        // never chosen, so Beta gets the New UI.
        std::fs::write(
            &path,
            r#"{"updateChannel":"beta","experiments":{"cuaVolume":true,"webUi":false}}"#,
        )
        .unwrap();
        let s = load(&path);
        assert_eq!(s.new_ui, None);
        assert!(s.experiments.cua_volume);
        assert!(!s.experiments.web_ui, "on only by choice until resolved");
        assert!(resolve_new_ui(&s, beta).experiments.web_ui);
        assert!(!resolve_new_ui(&s, "0.8.0").experiments.web_ui);
        // Saving the resolved settings keeps it unchosen: the default is
        // never written as a choice.
        save(&path, &resolve_new_ui(&s, beta)).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["experiments"]["webUi"], false, "what older builds read");
        assert!(raw["newUi"].is_null());
        assert_eq!(load(&path).new_ui, None);

        // A saved `webUi: true` was chosen: kept, on any channel.
        std::fs::write(&path, r#"{"experiments":{"webUi":true}}"#).unwrap();
        let s = load(&path);
        assert_eq!(s.new_ui, Some(true));
        assert!(s.experiments.web_ui);
        assert!(resolve_new_ui(&s, "0.7.2").experiments.web_ui);
        save(&path, &s).unwrap();
        assert_eq!(load(&path).new_ui, Some(true));

        // No experiments at all (or no file): never chosen.
        std::fs::write(&path, r#"{"menuBar":true}"#).unwrap();
        assert_eq!(load(&path).new_ui, None);
        std::fs::remove_file(&path).unwrap();
        let fresh = load(&path);
        assert_eq!(fresh.new_ui, None);
        assert!(!resolve_new_ui(&fresh, beta).experiments.web_ui, "Stable");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn use_classic_ui_survives_later_launches() {
        let (dir, path) = temp_settings("classic");
        let beta = "0.8.1-beta.1";
        let s = resolve_new_ui(
            &AppSettings {
                update_channel: UpdateChannel::Beta,
                ..Default::default()
            },
            beta,
        );
        assert!(s.experiments.web_ui);
        // "Use classic UI" on: saved as the user's choice.
        let row = crate::experiments::CLASSIC_UI_ROW;
        let s = choose_experiment(&s, row, "on");
        assert_eq!(s.new_ui, Some(false));
        assert!(!s.experiments.web_ui);
        // Another row leaves the choice alone.
        assert_eq!(
            choose_experiment(&s, "experiment:sharing", "on").new_ui,
            Some(false)
        );
        save(&path, &s).unwrap();
        // Later launches, later versions, either channel: still classic.
        for version in [beta, "0.8.1-beta.2", "0.8.1", "0.9.0"] {
            let again = resolve_new_ui(&load(&path), version);
            assert_eq!(again.new_ui, Some(false));
            assert!(!again.experiments.web_ui, "{version}");
            save(&path, &again).unwrap();
        }
        // Off again: the New UI, kept as chosen even on Stable before 0.8.1.
        let mut s = choose_experiment(&load(&path), row, "off");
        assert_eq!(s.new_ui, Some(true));
        s.update_channel = UpdateChannel::Stable;
        save(&path, &s).unwrap();
        assert!(resolve_new_ui(&load(&path), "0.7.3").experiments.web_ui);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn switching_channel_moves_the_default_not_a_choice() {
        let beta = AppSettings {
            update_channel: UpdateChannel::Beta,
            ..Default::default()
        };
        let stable = AppSettings {
            update_channel: UpdateChannel::Stable,
            ..beta.clone()
        };
        let v = "0.8.1-beta.1";
        assert!(resolve_new_ui(&beta, v).experiments.web_ui);
        assert!(!resolve_new_ui(&stable, v).experiments.web_ui);
        // Back to Beta: on again.
        let back = AppSettings {
            update_channel: UpdateChannel::Beta,
            ..resolve_new_ui(&stable, v)
        };
        assert!(resolve_new_ui(&back, v).experiments.web_ui);
        // A choice holds on either channel.
        for (choice, channel) in [(false, UpdateChannel::Beta), (true, UpdateChannel::Stable)] {
            let s = AppSettings {
                update_channel: channel,
                new_ui: Some(choice),
                ..Default::default()
            };
            assert_eq!(resolve_new_ui(&s, v).experiments.web_ui, choice);
        }
        // From 0.8.1, Stable too.
        assert!(resolve_new_ui(&stable, "0.8.1").experiments.web_ui);
    }

    #[test]
    fn a_prerelease_installs_first_launch_is_on_beta() {
        use UpdateChannel::{Beta, Stable};
        assert_eq!(first_launch_channel(Stable, "0.8.1-beta.1"), Beta);
        assert_eq!(first_launch_channel(Stable, "0.8.1-beta.1+abc"), Beta);
        assert_eq!(first_launch_channel(Stable, "0.7.2"), Stable);
        assert_eq!(first_launch_channel(Stable, "0.8.0+build-7"), Stable);
        // A release keeps a channel already in the file.
        assert_eq!(first_launch_channel(Beta, "0.7.2"), Beta);
        // With it, a fresh beta opens the New UI.
        let fresh = AppSettings {
            update_channel: first_launch_channel(Stable, "0.8.1-beta.1"),
            ..Default::default()
        };
        assert!(resolve_new_ui(&fresh, "0.8.1-beta.1").experiments.web_ui);
    }

    #[test]
    fn sharing_on_by_default_migrates_a_0_9_0_false_once() {
        // Fresh install: on, and an "off" afterwards survives restarts.
        let (dir, path) = temp_settings("sharing-fresh");
        let s = load(&path);
        assert!(s.experiments.sharing);
        let off = choose_experiment(&s, "experiment:sharing", "off");
        save(&path, &off).unwrap();
        assert!(!load(&path).experiments.sharing);
        assert!(!load(&path).experiments.sharing, "and again");
        let _ = std::fs::remove_dir_all(dir);

        // 0.9.0: toggling any experiment saved sharing false, no marker.
        let (dir, path) = temp_settings("sharing-090");
        std::fs::write(
            &path,
            r#"{"experiments":{"cuaVolume":true,"sharing":false}}"#,
        )
        .unwrap();
        let s = load(&path);
        assert!(s.experiments.sharing && s.experiments.cua_volume);
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["experiments"]["sharing"], true, "persisted");
        // The user turns it off in 0.9.1: stays off across restarts.
        save(&path, &choose_experiment(&s, "experiment:sharing", "off")).unwrap();
        assert!(!load(&path).experiments.sharing);
        assert!(!load(&path).experiments.sharing);
        assert!(load(&path).experiments.cua_volume);
        let _ = std::fs::remove_dir_all(dir);

        // 0.9.0 settings with sharing already true stay true.
        let (dir, path) = temp_settings("sharing-true");
        std::fs::write(&path, r#"{"experiments":{"sharing":true}}"#).unwrap();
        assert!(load(&path).experiments.sharing);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn your_cloud_on_by_default_migrates_a_stored_false_once() {
        // Fresh install: on, and an "off" afterwards survives restarts.
        let (dir, path) = temp_settings("cloud-fresh");
        let s = load(&path);
        assert!(s.experiments.your_cloud);
        save(
            &path,
            &choose_experiment(&s, "experiment:your_cloud", "off"),
        )
        .unwrap();
        assert!(!load(&path).experiments.your_cloud);
        assert!(!load(&path).experiments.your_cloud, "and again");
        let _ = std::fs::remove_dir_all(dir);

        // No value stored: on. An older build's saved false (no marker for
        // it, though the sharing marker exists): on once, then the user's.
        let (dir, path) = temp_settings("cloud-old");
        std::fs::write(&path, r#"{"experiments":{"cuaVolume":true}}"#).unwrap();
        std::fs::write(
            path.with_file_name("settings-migrations.json"),
            r#"{"sharingDefaultOn":true,"sharingSwitchRetired":true}"#,
        )
        .unwrap();
        assert!(load(&path).experiments.your_cloud, "never set reads as on");
        std::fs::write(
            &path,
            r#"{"experiments":{"yourCloud":false,"sharing":false}}"#,
        )
        .unwrap();
        std::fs::remove_file(path.with_file_name("settings-migrations.json")).unwrap();
        std::fs::write(
            path.with_file_name("settings-migrations.json"),
            r#"{"sharingDefaultOn":true,"sharingSwitchRetired":true}"#,
        )
        .unwrap();
        let s = load(&path);
        assert!(s.experiments.your_cloud, "the saved default, not a choice");
        assert!(
            !s.experiments.sharing,
            "sharing's own marker kept its choice"
        );
        save(
            &path,
            &choose_experiment(&s, "experiment:your_cloud", "off"),
        )
        .unwrap();
        assert!(!load(&path).experiments.your_cloud);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_retired_sharing_and_cloud_switches_turn_a_stored_off_on_once() {
        // Both default-on migrations already ran; the switches were still
        // there, and someone turned both off.
        let (dir, path) = temp_settings("retired");
        std::fs::write(
            &path,
            r#"{"experiments":{"cuaVolume":true,"sharing":false,"yourCloud":false},"menuBar":true}"#,
        )
        .unwrap();
        std::fs::write(
            path.with_file_name("settings-migrations.json"),
            r#"{"sharingDefaultOn":true,"yourCloudDefaultOn":true}"#,
        )
        .unwrap();
        let s = load(&path);
        assert!(s.experiments.sharing && s.experiments.your_cloud);
        // Everything else the person set is kept.
        assert!(s.experiments.cua_volume && s.menu_bar);
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["experiments"]["sharing"], true);
        assert_eq!(raw["menuBar"], true);
        // It runs once: the markers say so, and a later load changes nothing.
        let marks: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path.with_file_name("settings-migrations.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marks["sharingSwitchRetired"], true);
        assert_eq!(marks["yourCloudSwitchRetired"], true);
        let again = load(&path);
        assert_eq!(again.experiments, s.experiments);
        let _ = std::fs::remove_dir_all(dir);
    }
}
