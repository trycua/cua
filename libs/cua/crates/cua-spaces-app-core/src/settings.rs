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

fn location_or_local<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Location, D::Error> {
    let word = String::deserialize(d)?;
    Ok(serde_json::from_value(serde_json::Value::String(word)).unwrap_or(Location::Local))
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
    /// Where New Space starts. A word this build does not know (the
    /// retired `cloud`) reads as this Mac, so an old file keeps its other
    /// settings.
    #[serde(deserialize_with = "location_or_local")]
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
    /// ([`crate::experiments`]).
    pub experiments: Experiments,
    /// Keyvault copies (import ids) the user dismissed from the notch: it no
    /// longer shows them, nothing is revoked or wiped
    /// ([`crate::keyvault::view::prune_dismissed`] forgets the gone ones).
    pub dismissed_access: Vec<String>,
    /// The Spaces and the machines' desktops pinned to the notch ("Pin to
    /// notch" in a Space's "..." menu), in the order they were pinned: the
    /// notch lists them first ([`crate::notch::tiles_pinned`]). Ids only,
    /// nothing secret; an id whose Space is gone is kept and draws nothing.
    pub notch_pinned: Vec<String>,
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
    /// Tips ([`crate::tips`]): "Don't show tips" and the ones done. An
    /// older settings file without it reads as tips on, none done.
    pub tips: crate::tips::TipsState,
    /// Settings, Connections, Agent menu ([`crate::agent_menu`]): the apps
    /// Home's agent menu shows although the default hides them (an app
    /// outside Codex, Hermes, Claude Code and OpenCode, not signed in).
    pub agent_menu_shown: Vec<String>,
    /// The apps the menu hides although the default shows them. Nothing is
    /// signed out: a hidden app keeps its sign-ins and stays on the Agents
    /// page and available by name.
    pub agent_menu_hidden: Vec<String>,
    /// The working accounts the menu last saw (`app|variable|slot`): a new
    /// one is a connect, which shows its app again
    /// ([`crate::agent_menu::sync`]).
    pub agent_menu_seen: Vec<String>,
    /// Settings, General, "When I close the window": closing the main
    /// window leaves Cua Spaces and its background service running (on, the
    /// default); off, it quits Cua Spaces, as Quit does.
    pub close_window_keeps_running: bool,
    /// "Don't show this again" on the notice that Cua Spaces keeps running
    /// when the window closes, or the Settings switch turned off.
    pub close_notice_off: bool,
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
            dismissed_access: vec![],
            notch_pinned: vec![],
            keyvault_site_icons: true,
            teleport_choices: vec![],
            auto_connect: true,
            current_team_id: None,
            tips: crate::tips::TipsState::default(),
            agent_menu_shown: vec![],
            agent_menu_hidden: vec![],
            agent_menu_seen: vec![],
            close_window_keeps_running: true,
            close_notice_off: false,
        }
    }
}

/// Reads settings; a missing or damaged file gives the defaults. A damaged
/// file (not valid settings JSON) is kept beside it as
/// `<name>.damaged` before the shell saves the defaults over it, so a
/// person's settings are never lost without a copy.
pub fn load(path: &Path) -> AppSettings {
    let text = std::fs::read_to_string(path).ok();
    let parsed = text.as_deref().map(serde_json::from_str::<AppSettings>);
    if let Some(Err(e)) = &parsed {
        let kept = damaged_path(path);
        match std::fs::copy(path, &kept) {
            Ok(_) => eprintln!(
                "{} is damaged ({e}); using the defaults, the damaged file is kept as {}",
                path.display(),
                kept.display()
            ),
            Err(c) => eprintln!(
                "{} is damaged ({e}); using the defaults (could not keep a copy: {c})",
                path.display()
            ),
        }
    }
    parsed.and_then(Result::ok).unwrap_or_default()
}

/// Where a damaged settings file is kept (`settings.json.damaged`).
pub fn damaged_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".damaged");
    path.with_file_name(name)
}

/// The update channel of an install's first launch, from the one in the
/// settings file (`channel`): Nightly for any prerelease (a nightly, or a
/// beta from before Nightly: someone who installed one keeps getting the
/// newest builds), else unchanged.
pub fn first_launch_channel(channel: UpdateChannel, version: &str) -> UpdateChannel {
    if version.split('+').next().unwrap_or("").contains('-') {
        UpdateChannel::Nightly
    } else {
        channel
    }
}

/// The settings after a Settings, Experiments row's choice
/// ([`crate::experiments::choose`]).
pub fn choose_experiment(settings: &AppSettings, row: &str, option: &str) -> AppSettings {
    let mut s = settings.clone();
    s.experiments = crate::experiments::choose(&s.experiments, row, option);
    s
}

/// Writes settings atomically (temp file, then rename).
pub fn save(path: &Path, settings: &AppSettings) -> Result<(), CoreError> {
    let mut value = serde_json::to_value(settings).map_err(|e| CoreError::Io(e.to_string()))?;
    // Nightly is stored as `beta`, its old name: a release from before
    // Nightly (someone going back to Stable by hand) reads the whole file
    // with serde and starts over with the defaults on a word it does not
    // know. This build reads `beta` as Nightly ([`UpdateChannel`]).
    if settings.update_channel == UpdateChannel::Nightly
        && let Some(o) = value.as_object_mut()
    {
        o.insert("updateChannel".into(), STORED_NIGHTLY.into());
    }
    let json = serde_json::to_string_pretty(&value).map_err(|e| CoreError::Io(e.to_string()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CoreError::Io(e.to_string()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| CoreError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| CoreError::Io(e.to_string()))
}

/// How the settings file stores [`UpdateChannel::Nightly`] (see [`save`]).
const STORED_NIGHTLY: &str = "beta";

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
    /// The Cua account through client credentials (an API key): its client id.
    pub api_key_client: Option<String>,
    /// The sign-in in progress.
    pub sign_in: SignInPhase,
    /// Signing out is possible (a user session).
    pub can_sign_out: bool,
    /// The Spaces tab is hidden from the notch (the menu bar item is the
    /// entry point).
    pub menu_bar: bool,
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
    /// Overflow into your clouds when this machine is full
    /// (`placement.overflow`); none: no row.
    #[serde(default)]
    pub overflow: Option<bool>,
    /// Settings, Advanced's runtimes: each kind's setting, what is
    /// installed for it and whether it runs here. None: nothing read yet
    /// (the rows still show, "Checking…").
    #[serde(default)]
    pub runtimes: Option<crate::runtimes::RuntimesInput>,
    /// Settings, General's Show tips switch: tips are on
    /// (`!AppSettings.tips.off`). A shell that does not say reads as on.
    #[serde(default = "default_true")]
    pub show_tips: bool,
    /// Settings, General's "When I close the window": the window's close
    /// button keeps Cua Spaces running in the background (none: no rows; a
    /// shell without a background service leaves them out).
    #[serde(default)]
    pub close_window_keeps_running: Option<bool>,
    /// The notice that Cua Spaces keeps running is on (shown with the row
    /// above).
    #[serde(default = "default_true")]
    pub close_notice: bool,
}

impl Default for SettingsInput {
    fn default() -> Self {
        Self {
            identity: None,
            api_key_client: None,
            sign_in: SignInPhase::Idle,
            can_sign_out: false,
            menu_bar: false,
            telemetry: None,
            agents: None,
            agents_busy: false,
            agents_pending: vec![],
            teams: None,
            login_item: None,
            experiments: Experiments::default(),
            keyvault_auto_wipe: None,
            keyvault_unlock_prompt: None,
            keyvault_site_icons: true,
            keyvault_protection: vec![],
            auto_connect: None,
            overflow: None,
            runtimes: None,
            show_tips: true,
            close_window_keeps_running: None,
            close_notice: true,
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

/// Settings, General, after Welcome: the Show tips switch (row `tips`,
/// options `on` and `off`) and Reset tips (row `reset-tips`, button
/// "Reset"). [`choose`] applies both.
pub fn tips_rows(on: bool) -> Vec<SettingsRow> {
    let mut show = row("tips", SettingsRowKind::Toggle, "Show tips");
    show.options = vec![opt("on", "On", on), opt("off", "Off", !on)];
    let mut reset = row("reset-tips", SettingsRowKind::Text, "Reset tips");
    reset.button = Some("Reset".into());
    vec![show, reset]
}

/// The close-window row's label.
pub const CLOSE_WINDOW_LABEL: &str = "When I close the window";

/// The close-notice switch's label.
pub const CLOSE_NOTICE_LABEL: &str = "Tell me when Cua Spaces keeps running";

/// Settings, General: what the window's close button does. Row
/// `close-window`, options `keep` (default) and `quit`.
pub fn close_window_row(keeps_running: bool) -> SettingsRow {
    let mut r = row("close-window", SettingsRowKind::Choice, CLOSE_WINDOW_LABEL);
    r.options = vec![
        opt("keep", "Keep running in the background", keeps_running),
        opt("quit", "Quit Cua Spaces", !keeps_running),
    ];
    r.help = Some(if keeps_running {
        "Your agents can still create and manage Spaces. Quit Cua Spaces in the menu bar, or press \u{2318}Q, to stop it.".into()
    } else {
        "Closing the window quits Cua Spaces and stops the background service.".into()
    });
    r
}

/// Settings, General: the notice shown when the window closes and Cua
/// Spaces keeps running. Row `close-notice`, options `on` and `off`.
pub fn close_notice_row(on: bool) -> SettingsRow {
    let mut r = row("close-notice", SettingsRowKind::Toggle, CLOSE_NOTICE_LABEL);
    r.options = vec![opt("on", "On", on), opt("off", "Off", !on)];
    r
}

/// The settings after a Settings row's choice or button that the core
/// owns: `tips` (`on`/`off`: Show tips; on brings back the tips not done)
/// and `reset-tips` (any option: every tip starts over, and tips are on).
/// Any other row leaves the settings as they are.
pub fn choose(settings: &AppSettings, row: &str, option: &str) -> AppSettings {
    use crate::tips::{TipsEvent, reduce};
    let mut s = settings.clone();
    match (row, option) {
        ("tips", "on" | "off") => {
            s.tips = reduce(&s.tips, &TipsEvent::ShowTips { on: option == "on" });
        }
        ("reset-tips", _) => s.tips = reduce(&s.tips, &TipsEvent::Reset),
        ("close-window", "keep" | "quit") => s.close_window_keeps_running = option == "keep",
        ("close-notice", "on" | "off") => s.close_notice_off = option == "off",
        _ => {}
    }
    s
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
pub const UNLOCK_PROMPT_NOTE: &str = "Show the explanation again.";

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
    if let Some(keeps) = input.close_window_keeps_running {
        general.push(close_window_row(keeps));
        general.push(close_notice_row(input.close_notice));
    }
    if let Some(on) = input.auto_connect {
        general.push(auto_connect_row(on));
    }
    if let Some(on) = input.overflow {
        general.push(overflow_row(on));
    }
    let mut welcome = row("welcome", Text, "Welcome");
    welcome.button = Some("Show again".into());
    general.push(welcome);
    general.extend(tips_rows(input.show_tips));

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

    // Every kind's runtime row, on every host, whatever is known yet.
    sections.push(crate::runtimes::section(input.runtimes.as_ref()));

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
        button_help: Some("Let every coding app on this computer use Cua.".into()),
        rows,
    });

    SettingsPage {
        title: "Settings".into(),
        sections,
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

    /// Every host's page has the Runtimes section with the three kinds,
    /// even before anything was read.
    #[test]
    fn the_runtimes_section_is_always_there() {
        let p = page(&SettingsInput::default());
        let s = p.sections.iter().find(|s| s.id == "runtimes").unwrap();
        let ids: Vec<_> = s.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["runtime:container", "runtime:vm", "runtime:microvm"]);
        // After General.
        let order: Vec<_> = p.sections.iter().map(|s| s.id.as_str()).collect();
        let at = |id: &str| order.iter().position(|s| *s == id).unwrap();
        assert!(at("general") < at("runtimes"));
    }

    #[test]
    fn a_damaged_settings_file_gives_the_defaults_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{\"updateChannel\": \"beta\", \"theme\": ").unwrap();
        let s = load(&path);
        assert_eq!(s, load(&dir.path().join("missing.json")), "the defaults");
        assert_eq!(
            std::fs::read_to_string(damaged_path(&path)).unwrap(),
            "{\"updateChannel\": \"beta\", \"theme\": "
        );
        // A valid file is read as is and leaves no copy.
        let ok = dir.path().join("ok.json");
        std::fs::write(&ok, "{}").unwrap();
        load(&ok);
        assert!(!damaged_path(&ok).exists());
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
            default_location: Location::Yours,
            last_seen_version: Some("0.2.0 (0.2.0.1)".into()),
            update_channel: UpdateChannel::Nightly,
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
        // The retired `cloud` default reads as this Mac; the rest is kept.
        std::fs::write(&path, r#"{"menuBar":true,"defaultLocation":"cloud"}"#).unwrap();
        let retired = load(&path);
        assert!(retired.menu_bar);
        assert_eq!(retired.default_location, Location::Local);
        std::fs::write(&path, "{nope").unwrap();
        assert_eq!(load(&path), AppSettings::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tips_persist_and_older_files_load() {
        let dir =
            std::env::temp_dir().join(format!("cua-app-core-settings-tips-{}", std::process::id()));
        let path = dir.join("settings.json");
        let s = AppSettings {
            tips: crate::tips::TipsState {
                off: true,
                done: vec!["try-agent".into()],
            },
            ..Default::default()
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
        // A file from before tips: on, none done.
        std::fs::write(&path, r#"{"menuBar":true,"autoConnect":false}"#).unwrap();
        let old = load(&path);
        assert!(old.menu_bar);
        assert_eq!(old.tips, crate::tips::TipsState::default());
        // A file saved while the Pro banner could still be closed carries a
        // key the app no longer knows; it loads, and the key is dropped.
        std::fs::write(&path, r#"{"menuBar":true,"proBannerDismissedUntilMs":42}"#).unwrap();
        assert!(load(&path).menu_bar);
        save(&path, &load(&path)).unwrap();
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("proBanner")
        );
        // A partial tips object reads too.
        std::fs::write(&path, r#"{"tips":{"done":["power"]}}"#).unwrap();
        assert_eq!(load(&path).tips.done, ["power"]);
        assert!(!load(&path).tips.off);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn show_tips_and_reset_tips_follow_welcome() {
        let general = |input: &SettingsInput| {
            page(input)
                .sections
                .into_iter()
                .find(|s| s.id == "general")
                .unwrap()
                .rows
        };
        let rows = general(&SettingsInput::default());
        let ids: Vec<_> = rows.iter().map(|r| r.id.as_str()).collect();
        let at = ids.iter().position(|i| *i == "welcome").unwrap();
        assert_eq!(&ids[at..], ["welcome", "tips", "reset-tips"]);
        let show = &rows[at + 1];
        assert_eq!(show.kind, SettingsRowKind::Toggle);
        assert_eq!(show.label, "Show tips");
        assert!(show.options[0].active, "on by default");
        let reset = &rows[at + 2];
        assert_eq!(reset.label, "Reset tips");
        assert_eq!(reset.button.as_deref(), Some("Reset"));
        let off = general(&SettingsInput {
            show_tips: false,
            ..Default::default()
        });
        assert!(off[at + 1].options[1].active);
        // A shell that does not say: on.
        let input: SettingsInput = serde_json::from_str("{}").unwrap();
        assert!(input.show_tips);
    }

    #[test]
    fn choosing_tips_rows_applies_them() {
        let s = AppSettings {
            tips: crate::tips::TipsState {
                off: false,
                done: vec!["try-agent".into(), "power".into()],
            },
            ..Default::default()
        };
        let off = choose(&s, "tips", "off");
        assert!(off.tips.off);
        assert_eq!(off.tips.done, s.tips.done, "turning off forgets nothing");
        let on = choose(&off, "tips", "on");
        assert!(!on.tips.off);
        assert_eq!(on.tips.done, s.tips.done, "the ones not done come back");
        let reset = choose(&off, "reset-tips", "press");
        assert_eq!(reset.tips, crate::tips::TipsState::default());
        // Anything else: unchanged.
        assert_eq!(choose(&s, "tips", "maybe"), s);
        assert_eq!(choose(&s, "notch", "hide"), s);
    }

    #[test]
    fn the_page_lists_account_general_privacy_agents() {
        let p = page(&SettingsInput::default());
        let ids: Vec<_> = p.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["account", "teams", "general", "runtimes", "agents"]);
        let p = page(&SettingsInput {
            identity: Some("ada@example.com".into()),
            can_sign_out: true,
            telemetry: Some(TelemetryInput::default()),
            ..Default::default()
        });
        let ids: Vec<_> = p.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "account", "teams", "general", "runtimes", "privacy", "agents"
            ]
        );
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
                "account", "teams", "general", "runtimes", "privacy", "keyvault", "agents"
            ]
        );
        let kv = &p.sections[5];
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
        assert!(old.notch_pinned.is_empty());
    }

    /// Pins to the notch are a per-user preference kept in the settings
    /// file: they survive a restart in the order they were made.
    #[test]
    fn notch_pins_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = AppSettings {
            notch_pinned: vec!["relay:wsl".into(), "local:dev".into()],
            ..AppSettings::default()
        };
        save(&path, &s).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"notchPinned\"")
        );
        assert_eq!(load(&path).notch_pinned, ["relay:wsl", "local:dev"]);
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

    #[test]
    fn closing_the_window_keeps_running_by_default_and_the_rows_follow_choose() {
        let d = AppSettings::default();
        assert!(d.close_window_keeps_running && !d.close_notice_off);
        // A file from before the setting reads as today's behavior.
        let old: AppSettings = serde_json::from_str(r#"{"menuBar":true}"#).unwrap();
        assert!(old.close_window_keeps_running && !old.close_notice_off);
        let quit = choose(&d, "close-window", "quit");
        assert!(!quit.close_window_keeps_running);
        assert!(choose(&quit, "close-window", "keep").close_window_keeps_running);
        assert!(choose(&d, "close-notice", "off").close_notice_off);
        assert!(!choose(&choose(&d, "close-notice", "off"), "close-notice", "on").close_notice_off);
        // An unknown option changes nothing.
        assert_eq!(choose(&d, "close-window", "bogus"), d);
        let general = |input: &SettingsInput| {
            page(input)
                .sections
                .into_iter()
                .find(|s| s.id == "general")
                .unwrap()
                .rows
        };
        // Not passed (Windows, Linux): no rows.
        assert!(
            !general(&SettingsInput::default())
                .iter()
                .any(|r| r.id.starts_with("close-"))
        );
        let rows = general(&SettingsInput {
            close_window_keeps_running: Some(true),
            ..Default::default()
        });
        let i = rows.iter().position(|r| r.id == "close-window").unwrap();
        assert_eq!(rows[i - 1].id, "notch");
        assert_eq!(rows[i + 1].id, "close-notice");
        assert_eq!(rows[i].label, "When I close the window");
        assert!(rows[i].options.iter().any(|o| o.id == "keep" && o.active));
        assert!(rows[i + 1].options.iter().any(|o| o.id == "on" && o.active));
        for r in &rows[i..=i + 1] {
            assert!(!r.label.contains('\u{00b7}') && !r.label.to_lowercase().contains("pool"));
        }
        let rows = general(&SettingsInput {
            close_window_keeps_running: Some(false),
            close_notice: false,
            ..Default::default()
        });
        assert!(
            rows.iter()
                .find(|r| r.id == "close-window")
                .unwrap()
                .options
                .iter()
                .any(|o| o.id == "quit" && o.active)
        );
        assert!(
            rows.iter()
                .find(|r| r.id == "close-notice")
                .unwrap()
                .options
                .iter()
                .any(|o| o.id == "off" && o.active)
        );
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
        assert_eq!(
            ids(&off),
            ["account", "teams", "general", "runtimes", "agents"]
        );
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
            [
                "account", "teams", "general", "storage", "runtimes", "agents"
            ]
        );
        assert_eq!(note(&p).as_deref(), Some(NOTE));
        // A settings file from before experiments: all off.
        let old: AppSettings = serde_json::from_str(r#"{"menuBar":true}"#).unwrap();
        assert_eq!(old.experiments, Experiments::default());
    }

    #[test]
    fn a_prerelease_installs_first_launch_is_on_nightly() {
        use UpdateChannel::{Nightly, Stable};
        assert_eq!(first_launch_channel(Stable, "0.8.1-beta.1"), Nightly);
        assert_eq!(first_launch_channel(Stable, "0.8.1-beta.1+abc"), Nightly);
        assert_eq!(
            first_launch_channel(Stable, "0.21.0-nightly.20261105+55e1f5d"),
            Nightly
        );
        assert_eq!(
            first_launch_channel(Stable, "0.21.0-nightly.20261105"),
            Nightly
        );
        assert_eq!(
            first_launch_channel(Stable, "0.21.0-nightly.20261105.42+55e1f5d"),
            Nightly
        );
        assert_eq!(first_launch_channel(Stable, "0.7.2"), Stable);
        assert_eq!(first_launch_channel(Stable, "0.8.0+build-7"), Stable);
        // A release keeps a channel already in the file.
        assert_eq!(first_launch_channel(Nightly, "0.7.2"), Nightly);
    }

    #[test]
    fn beta_users_move_to_nightly_and_older_releases_still_read_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "cua-app-core-settings-nightly-{}",
            std::process::id()
        ));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        // A file written by a release that had Beta: now Nightly, the rest kept.
        std::fs::write(&path, r#"{"updateChannel":"beta","menuBar":true}"#).unwrap();
        let s = load(&path);
        assert_eq!(s.update_channel, UpdateChannel::Nightly);
        assert!(s.menu_bar);
        // Saved as `beta`, so a release from before Nightly reads it (as
        // Beta, every other setting kept) instead of starting over.
        save(&path, &s).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["updateChannel"], "beta");
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        #[allow(dead_code)]
        enum OldChannel {
            Stable,
            Beta,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct OldSettings {
            update_channel: OldChannel,
            menu_bar: bool,
        }
        let old: OldSettings = serde_json::from_value(raw).unwrap();
        assert!(matches!(old.update_channel, OldChannel::Beta) && old.menu_bar);
        // A file a Nightly-aware build wrote by hand still loads.
        std::fs::write(&path, r#"{"updateChannel":"nightly","menuBar":true}"#).unwrap();
        assert_eq!(load(&path).update_channel, UpdateChannel::Nightly);
        // Stable stays `stable`.
        save(
            &path,
            &AppSettings {
                update_channel: UpdateChannel::Stable,
                ..s
            },
        )
        .unwrap();
        assert_eq!(load(&path).update_channel, UpdateChannel::Stable);
        let _ = std::fs::remove_dir_all(dir);
    }
}
