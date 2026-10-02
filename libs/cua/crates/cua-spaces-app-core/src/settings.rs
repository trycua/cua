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
    /// ([`crate::experiments`]).
    pub experiments: Experiments,
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
            keyvault_site_icons: true,
            teleport_choices: vec![],
            auto_connect: true,
        }
    }
}

/// Reads settings; a missing or damaged file gives the defaults.
pub fn load(path: &Path) -> AppSettings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes settings atomically (temp file, then rename).
pub fn save(path: &Path, settings: &AppSettings) -> Result<(), CoreError> {
    let json = serde_json::to_string_pretty(settings).map_err(|e| CoreError::Io(e.to_string()))?;
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
/// The Teams waitlist on the website (Settings' Teams row, the Sign in
/// page's line). The app collects nothing itself.
pub const TEAMS_WAITLIST_URL: &str = "https://cua.ai/teams";

/// The Teams row's label.
pub const TEAMS_LABEL: &str = "Teams";

/// Its value.
pub const TEAMS_VALUE: &str = "Coming soon";

/// Its button.
pub const TEAMS_BUTTON: &str = "Join the waitlist";

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
    },
    /// It failed.
    Failed {
        /// Why.
        message: String,
    },
}

/// The usage-telemetry switch's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TelemetryInput {
    /// Usage data is shared.
    pub enabled: bool,
    /// The environment decides (`DO_NOT_TRACK`, `CUA_TELEMETRY`, CI): who.
    pub locked_by: Option<String>,
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
            billing: None,
            login_item: None,
            experiments: Experiments::default(),
            keyvault_auto_wipe: None,
            keyvault_unlock_prompt: None,
            keyvault_site_icons: true,
            keyvault_protection: vec![],
            auto_connect: None,
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
            r.link_url = b.billing_url.clone();
            r.enabled = b.billing_url.is_some();
            account.push(r);
        }
    } else if let SignInPhase::Waiting { user_code } = &input.sign_in {
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
        if let SignInPhase::Failed { message } = &input.sign_in {
            account.push(row("sign-in-error", Error, message));
        }
    }
    // Teams: a waitlist on the website (the app collects nothing). Row id
    // `teams`; the button opens `link_url`.
    let mut teams = row("teams", Text, TEAMS_LABEL);
    teams.value = Some(TEAMS_VALUE.into());
    teams.button = Some(TEAMS_BUTTON.into());
    teams.link_url = Some(TEAMS_WAITLIST_URL.into());
    account.push(teams);

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
            id: "general".into(),
            title: "General".into(),
            button: None,
            button_enabled: false,
            button_help: None,
            rows: general,
        },
    ];

    if let Some(t) = &input.telemetry {
        let mut share = row("telemetry", Choice, "Share anonymous usage data");
        share.options = vec![opt("on", "On", t.enabled), opt("off", "Off", !t.enabled)];
        if let Some(by) = t.locked_by.clone().filter(|b| !b.is_empty()) {
            share.enabled = false;
            share.help = Some(format!("Set by {by}"));
        }
        let mut note = row(
            "telemetry-note",
            Note,
            "Features used, sandbox types, durations and error categories. Never paths, names, prompts, screen content or Keyvault items.",
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
        assert_eq!(ids, ["account", "general", "agents"]);
        let p = page(&SettingsInput {
            identity: Some("ada@example.com".into()),
            can_sign_out: true,
            telemetry: Some(TelemetryInput::default()),
            ..Default::default()
        });
        let ids: Vec<_> = p.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["account", "general", "privacy", "agents"]);
        assert_eq!(p.sections[0].rows[0].button.as_deref(), Some("Sign out"));
        let sign_in: SignInPhase =
            serde_json::from_str(r#"{"kind":"waiting","userCode":"AB-CD"}"#).unwrap();
        assert_eq!(
            sign_in,
            SignInPhase::Waiting {
                user_code: Some("AB-CD".into())
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
            ["account", "general", "privacy", "keyvault", "agents"]
        );
        let kv = &p.sections[3];
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
            p.sections[1]
                .rows
                .iter()
                .find(|r| r.id == "launch-at-login-note")
                .map(|r| r.label.clone())
        };
        let off = with_storage(&page(&input), &storage, &input.experiments);
        assert_eq!(ids(&off), ["account", "general", "agents"]);
        assert_eq!(note(&off).as_deref(), Some(NOTE_WITHOUT_VOLUME));
        let on = SettingsInput {
            experiments: Experiments {
                cua_volume: true,
                ..Default::default()
            },
            ..input
        };
        let p = with_storage(&page(&on), &storage, &on.experiments);
        assert_eq!(ids(&p), ["account", "general", "storage", "agents"]);
        assert_eq!(note(&p).as_deref(), Some(NOTE));
        // A settings file from before experiments: all off.
        let old: AppSettings = serde_json::from_str(r#"{"menuBar":true}"#).unwrap();
        assert_eq!(old.experiments, Experiments::default());
    }
}
