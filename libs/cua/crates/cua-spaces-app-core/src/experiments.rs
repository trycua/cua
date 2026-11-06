// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Experiments: one switch per feature. Secure Spaces, Child Spaces, Ask first and
//! Warm start are on by default (their on state is the default and a stored off stays off);
//! the others are off until turned on.
//!
//! - Cua Volume ([`Experiment::CuaVolume`]): the first run's Volume page
//!   (and its Done line) and Settings' Storage section;
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
//! `experiments`): one key per experiment, missing keys read as the experiment's default
//! ([`Experiments::default`]) and unknown ones are ignored, so an older or newer build reads the
//! file. A key that is stored, on or off, always wins over the default.

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
    /// Secure Spaces: a Space reaches the internet and nothing else on the
    /// network unless it is opened. On unless turned off; the daemon enforces
    /// it (the switch is kept in the cua home).
    pub secure_spaces: bool,
    /// Child Spaces: a Space may make Spaces of its own. On unless turned
    /// off; the daemon enforces it (the switch is kept in the cua home).
    pub child_spaces: bool,
    /// Ask first: an agent asks on a card before something leaves its Space
    /// (a push to GitHub). On unless turned off; the daemon enforces it (the
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
    /// installed, so the next Create from Home starts at once. On unless
    /// turned off; the daemon enforces it (the switch is kept in the cua home).
    pub warm_pools: bool,
}

impl Default for Experiments {
    /// What a person who never touched a switch has: Secure Spaces, Child Spaces, Ask first and
    /// Warm start on, the rest off. The daemon's gates ([`cua_spaces::experiments::Gate`]) and
    /// the web Labs flags say the same; a test in cua-spaces-ffi keeps them from drifting.
    fn default() -> Self {
        Experiments {
            cua_volume: false,
            secure_spaces: true,
            child_spaces: true,
            ask_first: true,
            ci_runners: false,
            tcp_shares: false,
            mobile: false,
            warm_pools: true,
        }
    }
}

/// One experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Experiment {
    /// Cua Volume.
    CuaVolume,
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
pub const ALL: [Experiment; 8] = [
    Experiment::CuaVolume,
    Experiment::SecureSpaces,
    Experiment::ChildSpaces,
    Experiment::AskFirst,
    Experiment::CiRunners,
    Experiment::TcpShares,
    Experiment::Mobile,
    Experiment::WarmPools,
];

impl Experiment {
    /// The stable id (`cua_volume`, `secure_spaces`, ...): the Settings
    /// row's suffix and the telemetry word.
    pub fn id(self) -> &'static str {
        match self {
            Experiment::CuaVolume => "cua_volume",
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

    /// Every experiment off (what a person who turned them all off has).
    pub fn all_off() -> Self {
        ALL.into_iter()
            .fold(Self::default(), |x, e| x.with(e, false))
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

/// The experiment a Settings row id names (`experiment:cua_volume`).
pub fn of_row(id: &str) -> Option<Experiment> {
    id.strip_prefix("experiment:").and_then(Experiment::parse)
}

/// The Experiments tab: one switch per experiment, each with its one line.
pub fn page(experiments: &Experiments) -> SettingsPage {
    let mut rows = Vec::new();
    for e in ALL {
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
/// other row or option.
pub fn choose(experiments: &Experiments, row: &str, option: &str) -> Experiments {
    match (of_row(row), option) {
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
    fn every_experiment_has_a_switch_and_one_line_and_the_defaults_are_as_shipped() {
        let p = page(&Experiments::default());
        assert_eq!(p.title, "Experiments");
        let ids: Vec<&str> = p.sections[0].rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "experiment:cua_volume",
                "experiment:cua_volume-note",
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
        // Secure Spaces, Child Spaces, Ask first and Warm start are on by default; the rest off.
        let on_by_default = ["secure_spaces", "child_spaces", "ask_first", "warm_pools"];
        for r in switches(&p) {
            let id = r.id.trim_start_matches("experiment:");
            let want = if on_by_default.contains(&id) {
                "on"
            } else {
                "off"
            };
            assert!(
                r.options.iter().any(|o| o.id == want && o.active),
                "{} should read {want}",
                r.id
            );
            assert!(r.enabled);
        }
        assert_eq!(Experiments::default().on_ids(), on_by_default);
        for e in ALL {
            assert!(!e.description().contains('\n'));
            assert!(!e.description().contains('\u{2014}'), "no em dashes");
        }
    }

    #[test]
    fn a_switch_turns_one_experiment_on_and_off() {
        let off = Experiments::all_off();
        let x = choose(&off, "experiment:cua_volume", "on");
        assert_eq!(x.on_ids(), ["cua_volume"]);
        assert_eq!(choose(&x, "experiment:cua_volume", "off"), off);
        let v = choose(&x, "experiment:mobile", "on");
        assert_eq!(v.on_ids(), ["cua_volume", "mobile"]);
        // A default-on switch turned off stays off, and is stored as off.
        let w = choose(&Experiments::default(), "experiment:warm_pools", "off");
        assert!(!w.warm_pools);
        assert_eq!(serde_json::to_value(w).unwrap()["warmPools"], false);
        assert_eq!(choose(&x, "notch", "on"), x, "other rows change nothing");
        assert_eq!(choose(&x, "experiment:nope", "on"), x);
        assert_eq!(
            Experiments::all_on().on_ids(),
            [
                "cua_volume",
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
    fn stored_keys_are_forward_compatible() {
        // Keys of switches that are gone (an older build's `yourCloud`,
        // `sharing`, `webUi`) and of future ones are ignored.
        let x: Experiments = serde_json::from_str(
            r#"{"cuaVolume":true,"yourCloud":false,"sharing":false,"webUi":true,"someFutureExperiment":true}"#,
        )
        .unwrap();
        assert!(x.cua_volume);
        let back = serde_json::to_value(x).unwrap();
        assert_eq!(
            back,
            serde_json::json!({"cuaVolume": true, "secureSpaces": true, "childSpaces": true, "askFirst": true, "ciRunners": false, "tcpShares": false, "mobile": false, "warmPools": true})
        );
        // A stored off wins over a default that is on.
        let off: Experiments =
            serde_json::from_str(r#"{"askFirst":false,"warmPools":false}"#).unwrap();
        assert!(!off.ask_first && !off.warm_pools && off.secure_spaces && off.child_spaces);
        assert_eq!(
            serde_json::from_str::<Experiments>("{}").unwrap(),
            Experiments::default()
        );
    }
}
