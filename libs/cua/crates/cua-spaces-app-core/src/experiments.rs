// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Experiments: features still being built, one switch each, all
//! off by default except Sharing (on, as cua.ai/pricing lists it for every
//! plan) and Your cloud (on, so a linked cloud shows under Run on).
//!
//! - Cua Volume ([`Experiment::CuaVolume`]): the first run's Volume page
//!   (and its Done line) and Settings' Storage section;
//! - Your cloud ([`Experiment::YourCloud`]): the New Space wizard's
//!   connected clouds (AWS, Google Cloud, Hetzner, Modal), "Connect a cloud…" and a
//!   default location in a cloud;
//! - Sharing ([`Experiment::Sharing`]): a Space's Share button;
//! - New UI ([`Experiment::WebUi`]): retired. The shared web UI is the only
//!   UI of every shell, so it has no row; the key stays so the macOS shell
//!   can report it ([`crate::settings::resolve_new_ui`]).
//! - Secure Spaces ([`Experiment::SecureSpaces`]) and Child Spaces
//!   ([`Experiment::ChildSpaces`]) and CI runners ([`Experiment::CiRunners`]): enforced, not only hidden. Their switches
//!   are also kept in the cua home, where the daemon and `cua experiments`
//!   read them; the shells load and save them through the app export
//!   (`app_settings_load` and `app_settings_save`).
//! - Ask first ([`Experiment::AskFirst`]): enforced the same way. An agent
//!   asks on a card before something leaves its Space (a push to GitHub),
//!   and the host acts only after its owner answers.
//! - Public TCP shares ([`Experiment::TcpShares`]): enforced the same way.
//! - Warm start ([`Experiment::WarmPools`]): enforced the same way. The cua
//!   daemon keeps one prepared warm Space ready on this computer.
//!
//! Off only hides the entry points. Nothing is undone: a mounted Volume
//! stays mounted (and keeps syncing), a connected cloud stays connected
//! and its Spaces stay in the list, and a shared Space stays shared. Turning
//! the switch on again shows them as they are.
//!
//! The switches live in the app's settings ([`crate::settings::AppSettings`],
//! `experiments`): one key per experiment, missing keys read as their default (off, but on for Sharing and
//! Your cloud) and unknown ones are ignored, so an older or newer build reads the file.

use serde::{Deserialize, Serialize};

use crate::settings::{
    SettingsOption, SettingsPage, SettingsRow, SettingsRowKind, SettingsSection,
};

/// The switches, as stored (`"experiments": {"cuaVolume": true}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Experiments {
    /// Cua Volume in the first run and Settings, Storage.
    pub cua_volume: bool,
    /// Your own clouds in the New Space wizard.
    pub your_cloud: bool,
    /// Sharing a Space with other accounts. On unless turned off: a missing
    /// key reads as on (the plans on cua.ai/pricing list sharing).
    pub sharing: bool,
    /// The shared web UI is the main window. Always on in the macOS shell
    /// ([`crate::settings::resolve_new_ui`]); kept for older files and builds.
    pub web_ui: bool,
    /// Secure Spaces: a Space reaches the internet and nothing else on the
    /// network unless it is opened. Off unless turned on; the daemon enforces
    /// it (the switch is kept in the cua home).
    pub secure_spaces: bool,
    /// Child Spaces: a Space may make Spaces of its own. Off unless turned
    /// on; the daemon enforces it (the switch is kept in the cua home).
    pub child_spaces: bool,
    /// Ask first: an agent asks on a card before something leaves its Space
    /// (a push to GitHub). Off unless turned on; the daemon enforces it (the
    /// switch is kept in the cua home).
    pub ask_first: bool,
    /// CI runners: GitHub Actions jobs run in a fresh Space each (`cua ci`
    /// and the CI Runners page). Off unless turned on; the daemon and the
    /// `cua` command enforce it (the switch is kept in the cua home).
    pub ci_runners: bool,
    /// Public TCP shares: a raw TCP service in a Space (a game server) at a
    /// public address. Off unless turned on; the daemon enforces it (the
    /// switch is kept in the cua home).
    pub tcp_shares: bool,
    /// Mobile: the computer answers the Cua Spaces phone app (its tools and
    /// push events). Off unless turned on; the daemon enforces it (the switch
    /// is kept in the cua home).
    pub mobile: bool,
    /// Warm pools: one Linux Space kept ready, with its coding apps
    /// installed, so the next Create from Home starts at once. Off unless
    /// turned on; the daemon enforces it (the switch is kept in the cua home).
    pub warm_pools: bool,
}

impl Default for Experiments {
    fn default() -> Self {
        Self {
            cua_volume: false,
            your_cloud: true,
            sharing: true,
            web_ui: false,
            secure_spaces: false,
            child_spaces: false,
            ask_first: false,
            ci_runners: false,
            tcp_shares: false,
            mobile: false,
            warm_pools: false,
        }
    }
}

/// One experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Experiment {
    /// Cua Volume.
    CuaVolume,
    /// Your cloud.
    YourCloud,
    /// Sharing.
    Sharing,
    /// New UI (preview).
    WebUi,
    /// Secure Spaces.
    SecureSpaces,
    /// Child Spaces.
    ChildSpaces,
    /// Ask first.
    AskFirst,
    /// CI runners.
    CiRunners,
    /// Public TCP shares.
    TcpShares,
    /// Mobile.
    Mobile,
    /// Warm start.
    WarmPools,
}

/// Every experiment, in the order Settings lists them.
pub const ALL: [Experiment; 11] = [
    Experiment::CuaVolume,
    Experiment::YourCloud,
    Experiment::Sharing,
    Experiment::WebUi,
    Experiment::SecureSpaces,
    Experiment::ChildSpaces,
    Experiment::AskFirst,
    Experiment::CiRunners,
    Experiment::TcpShares,
    Experiment::Mobile,
    Experiment::WarmPools,
];

impl Experiment {
    /// The stable id (`cua_volume`, `your_cloud`, `sharing`, `web_ui`): the Settings
    /// row's suffix and the telemetry word.
    pub fn id(self) -> &'static str {
        match self {
            Experiment::CuaVolume => "cua_volume",
            Experiment::YourCloud => "your_cloud",
            Experiment::Sharing => "sharing",
            Experiment::WebUi => "web_ui",
            Experiment::SecureSpaces => "secure_spaces",
            Experiment::ChildSpaces => "child_spaces",
            Experiment::AskFirst => "ask_first",
            Experiment::CiRunners => "ci_runners",
            Experiment::TcpShares => "tcp_shares",
            Experiment::Mobile => "mobile",
            Experiment::WarmPools => "warm_pools",
        }
    }

    /// The experiment with `id`.
    pub fn parse(id: &str) -> Option<Self> {
        ALL.into_iter().find(|e| e.id() == id.trim())
    }

    /// The switch's label.
    pub fn title(self) -> &'static str {
        match self {
            Experiment::CuaVolume => "Cua Volume",
            Experiment::YourCloud => "Your cloud",
            Experiment::Sharing => "Sharing",
            Experiment::WebUi => "New UI",
            Experiment::SecureSpaces => "Secure Spaces",
            Experiment::ChildSpaces => "Child Spaces",
            Experiment::AskFirst => "Ask first",
            Experiment::CiRunners => "CI runners",
            Experiment::TcpShares => "Public TCP shares",
            Experiment::Mobile => "Phone app",
            Experiment::WarmPools => "Warm start",
        }
    }

    /// The one line under the switch.
    pub fn description(self) -> &'static str {
        match self {
            Experiment::CuaVolume => "Memory and files your agents share across every Space.",
            Experiment::YourCloud => {
                "Create Spaces in your own AWS, Google Cloud, Hetzner or Modal account."
            }
            Experiment::Sharing => "Share a Space with other Cua accounts.",
            Experiment::WebUi => "The Spaces window built on the shared web UI.",
            Experiment::SecureSpaces => {
                "A Space reaches the internet and nothing else on your network unless you open it."
            }
            Experiment::ChildSpaces => {
                "A Space can make Spaces of its own, inside a budget you set."
            }
            Experiment::AskFirst => {
                "An agent asks you on a card before anything leaves its Space, such as a push to GitHub."
            }
            Experiment::CiRunners => {
                "Run GitHub Actions jobs in a fresh Space each, on this computer."
            }
            Experiment::TcpShares => {
                "Share a game server or another TCP service in a Space at a public address."
            }
            Experiment::Mobile => {
                "Your computers answer the Cua Spaces phone app: start a task, follow the thread, review changes and get alerts."
            }
            Experiment::WarmPools => {
                "Keep one Linux Space ready on this computer, with its coding apps installed, so the next Create from Home starts at once."
            }
        }
    }
}

impl Experiments {
    /// Whether `e` is on.
    pub fn is_on(&self, e: Experiment) -> bool {
        match e {
            Experiment::CuaVolume => self.cua_volume,
            Experiment::YourCloud => self.your_cloud,
            Experiment::Sharing => self.sharing,
            Experiment::WebUi => self.web_ui,
            Experiment::SecureSpaces => self.secure_spaces,
            Experiment::ChildSpaces => self.child_spaces,
            Experiment::AskFirst => self.ask_first,
            Experiment::CiRunners => self.ci_runners,
            Experiment::TcpShares => self.tcp_shares,
            Experiment::Mobile => self.mobile,
            Experiment::WarmPools => self.warm_pools,
        }
    }

    /// With `e` turned `on`.
    pub fn with(mut self, e: Experiment, on: bool) -> Self {
        match e {
            Experiment::CuaVolume => self.cua_volume = on,
            Experiment::YourCloud => self.your_cloud = on,
            Experiment::Sharing => self.sharing = on,
            Experiment::WebUi => self.web_ui = on,
            Experiment::SecureSpaces => self.secure_spaces = on,
            Experiment::ChildSpaces => self.child_spaces = on,
            Experiment::AskFirst => self.ask_first = on,
            Experiment::CiRunners => self.ci_runners = on,
            Experiment::TcpShares => self.tcp_shares = on,
            Experiment::Mobile => self.mobile = on,
            Experiment::WarmPools => self.warm_pools = on,
        }
        self
    }

    /// Every experiment on (tests, captures).
    pub fn all_on() -> Self {
        ALL.into_iter()
            .fold(Self::default(), |x, e| x.with(e, true))
    }

    /// The ids of the experiments that are on, in [`ALL`] order.
    pub fn on_ids(&self) -> Vec<&'static str> {
        ALL.into_iter()
            .filter(|e| self.is_on(*e))
            .map(Experiment::id)
            .collect()
    }
}

/// The Settings row id of `e`'s switch: `experiment:cua_volume`.
pub fn row_id(e: Experiment) -> String {
    format!("experiment:{}", e.id())
}

/// The experiment a Settings row id names (`experiment:sharing`).
pub fn of_row(id: &str) -> Option<Experiment> {
    id.strip_prefix("experiment:").and_then(Experiment::parse)
}

/// The Experiments tab: one switch per experiment, each with its one line.
pub fn page(experiments: &Experiments) -> SettingsPage {
    let mut rows = Vec::new();
    for e in ALL {
        if e == Experiment::WebUi {
            // Retired: the web UI is the only UI, so there is nothing to choose.
            continue;
        }
        let on = experiments.is_on(e);
        let mut toggle = crate::settings::row(&row_id(e), SettingsRowKind::Toggle, e.title());
        toggle.options = vec![
            SettingsOption {
                id: "on".into(),
                label: "On".into(),
                active: on,
            },
            SettingsOption {
                id: "off".into(),
                label: "Off".into(),
                active: !on,
            },
        ];
        rows.push(toggle);
        rows.push(crate::settings::row(
            &format!("{}-note", row_id(e)),
            SettingsRowKind::Note,
            e.description(),
        ));
    }
    SettingsPage {
        title: "Experiments".into(),
        sections: vec![SettingsSection {
            id: "experiments".into(),
            title: "Experiments".into(),
            button: None,
            button_enabled: false,
            button_help: None,
            rows,
        }],
    }
}

/// The switches after a row's choice (`on` or `off`); unchanged for any
/// other row or option, and for the retired New UI rows
/// (`experiment:web_ui`, `experiment:classic_ui`).
pub fn choose(experiments: &Experiments, row: &str, option: &str) -> Experiments {
    match (of_row(row).filter(|e| *e != Experiment::WebUi), option) {
        (Some(e), "on") => experiments.with(e, true),
        (Some(e), "off") => experiments.with(e, false),
        _ => *experiments,
    }
}

/// The rows of `page` that are switches (helpers for shells and tests).
pub fn switches(page: &SettingsPage) -> Vec<&SettingsRow> {
    page.sections
        .iter()
        .flat_map(|s| s.rows.iter())
        .filter(|r| r.kind == SettingsRowKind::Toggle)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_experiment_but_sharing_and_your_cloud_is_off_by_default_with_one_line_each() {
        let p = page(&Experiments::default());
        assert_eq!(p.title, "Experiments");
        let ids: Vec<&str> = p.sections[0].rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "experiment:cua_volume",
                "experiment:cua_volume-note",
                "experiment:your_cloud",
                "experiment:your_cloud-note",
                "experiment:sharing",
                "experiment:sharing-note",
                "experiment:secure_spaces",
                "experiment:secure_spaces-note",
                "experiment:child_spaces",
                "experiment:child_spaces-note",
                "experiment:ask_first",
                "experiment:ask_first-note",
                "experiment:ci_runners",
                "experiment:ci_runners-note",
                "experiment:tcp_shares",
                "experiment:tcp_shares-note",
                "experiment:mobile",
                "experiment:mobile-note",
                "experiment:warm_pools",
                "experiment:warm_pools-note",
            ]
        );
        for r in switches(&p) {
            let shown = if r.id == "experiment:sharing" || r.id == "experiment:your_cloud" {
                "on"
            } else {
                "off"
            };
            assert!(
                r.options.iter().any(|o| o.id == shown && o.active),
                "{}",
                r.id
            );
            assert!(r.enabled);
        }
        for e in ALL {
            assert!(!e.description().contains('\n'));
            assert!(!e.description().contains('\u{2014}'), "no em dashes");
        }
    }

    #[test]
    fn a_switch_turns_one_experiment_on_and_off() {
        let off = Experiments {
            sharing: false,
            ..Default::default()
        };
        let x = choose(&off, "experiment:sharing", "on");
        assert_eq!(x, Experiments::default());
        assert_eq!(x.on_ids(), ["your_cloud", "sharing"]);
        assert_eq!(choose(&x, "experiment:sharing", "off"), off);
        let v = choose(&x, "experiment:cua_volume", "on");
        assert_eq!(v.on_ids(), ["cua_volume", "your_cloud", "sharing"]);
        assert_eq!(choose(&x, "notch", "on"), x, "other rows change nothing");
        assert_eq!(choose(&x, "experiment:nope", "on"), x);
        assert_eq!(
            Experiments::all_on().on_ids(),
            [
                "cua_volume",
                "your_cloud",
                "sharing",
                "web_ui",
                "secure_spaces",
                "child_spaces",
                "ask_first",
                "ci_runners",
                "tcp_shares",
                "mobile",
                "warm_pools"
            ]
        );
    }

    #[test]
    fn the_retired_new_ui_rows_are_gone_and_change_nothing() {
        let on = Experiments {
            web_ui: true,
            ..Default::default()
        };
        for x in [on, Experiments::default()] {
            let p = page(&x);
            assert!(
                p.sections[0]
                    .rows
                    .iter()
                    .all(|r| !r.id.contains("classic_ui") && !r.id.contains("web_ui")),
                "no New UI row"
            );
            for row in ["experiment:classic_ui", "experiment:web_ui"] {
                for option in ["on", "off"] {
                    assert_eq!(choose(&x, row, option), x, "{row} {option}");
                }
            }
        }
    }

    #[test]
    fn stored_keys_are_forward_compatible() {
        let x: Experiments =
            serde_json::from_str(r#"{"cuaVolume":true,"someFutureExperiment":true}"#).unwrap();
        assert!(x.cua_volume && x.your_cloud && x.sharing && !x.web_ui);
        let back = serde_json::to_value(x).unwrap();
        assert_eq!(
            back,
            serde_json::json!({"cuaVolume": true, "yourCloud": true, "sharing": true, "webUi": false, "secureSpaces": false, "childSpaces": false, "askFirst": false, "ciRunners": false, "tcpShares": false, "mobile": false, "warmPools": false})
        );
        // An explicit off is kept.
        let off: Experiments = serde_json::from_str(r#"{"yourCloud":false}"#).unwrap();
        assert!(!off.your_cloud);
        // Sharing is on unless the key says off (a missing key is not off).
        let off: Experiments = serde_json::from_str(r#"{"sharing":false}"#).unwrap();
        assert!(!off.sharing);
        assert!(Experiments::default().sharing);
        assert_eq!(
            serde_json::from_str::<Experiments>("{}").unwrap(),
            Experiments::default()
        );
    }
}
