// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Experiments: features still being built, one switch each, all
//! off by default.
//!
//! - Cua Volume ([`Experiment::CuaVolume`]): the first run's Volume page
//!   (and its Done line) and Settings' Storage section;
//! - CI runners ([`Experiment::CiRunners`]): enforced, not only hidden. Its switch
//!   is also kept in the cua home, where the daemon and `cua experiments`
//!   read it; the shells load and save it through the app export
//!   (`app_settings_load` and `app_settings_save`).
//! - Public TCP shares ([`Experiment::TcpShares`]): enforced the same way.
//!
//! Secure Spaces, Ask first, Child Spaces and Warm start used to be switches here; they are
//! always on now, and a stored key of theirs is ignored (and gone after the next save).
//!
//! Off only hides the entry points. Nothing is undone: a mounted Volume
//! stays mounted (and keeps syncing), a connected cloud stays connected
//! and its Spaces stay in the list, and a shared Space stays shared. Turning
//! the switch on again shows them as they are.
//!
//! The switches live in the app's settings ([`crate::settings::AppSettings`],
//! `experiments`): one key per experiment, missing keys read as off
//! and unknown ones are ignored, so an older or newer build reads the file.

use serde::{Deserialize, Serialize};

use crate::settings::{
    SettingsOption, SettingsPage, SettingsRow, SettingsRowKind, SettingsSection,
};

/// The switches, as stored (`"experiments": {"cuaVolume": true}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Experiments {
    /// Cua Volume in the first run and Settings, Storage.
    pub cua_volume: bool,
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
}

/// One experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Experiment {
    /// Cua Volume.
    CuaVolume,
    /// CI runners.
    CiRunners,
    /// Public TCP shares.
    TcpShares,
    /// Mobile.
    Mobile,
}

/// Every experiment, in the order Settings lists them.
pub const ALL: [Experiment; 4] = [
    Experiment::CuaVolume,
    Experiment::CiRunners,
    Experiment::TcpShares,
    Experiment::Mobile,
];

impl Experiment {
    /// The stable id (`cua_volume`, `child_spaces`, ...): the Settings
    /// row's suffix and the telemetry word.
    pub fn id(self) -> &'static str {
        match self {
            Experiment::CuaVolume => "cua_volume",
            Experiment::CiRunners => "ci_runners",
            Experiment::TcpShares => "tcp_shares",
            Experiment::Mobile => "mobile",
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
            Experiment::CiRunners => "CI runners",
            Experiment::TcpShares => "Public TCP shares",
            Experiment::Mobile => "Phone app",
        }
    }

    /// The one line under the switch.
    pub fn description(self) -> &'static str {
        match self {
            Experiment::CuaVolume => "Memory and files your agents share across every Space.",
            Experiment::CiRunners => {
                "Run GitHub Actions jobs in a fresh Space each, on this computer."
            }
            Experiment::TcpShares => {
                "Share a game server or another TCP service in a Space at a public address."
            }
            Experiment::Mobile => {
                "Your computers answer the Cua Spaces phone app: start a task, follow the thread, review changes and get alerts."
            }
        }
    }
}

impl Experiments {
    /// Whether `e` is on.
    pub fn is_on(&self, e: Experiment) -> bool {
        match e {
            Experiment::CuaVolume => self.cua_volume,
            Experiment::CiRunners => self.ci_runners,
            Experiment::TcpShares => self.tcp_shares,
            Experiment::Mobile => self.mobile,
        }
    }

    /// With `e` turned `on`.
    pub fn with(mut self, e: Experiment, on: bool) -> Self {
        match e {
            Experiment::CuaVolume => self.cua_volume = on,
            Experiment::CiRunners => self.ci_runners = on,
            Experiment::TcpShares => self.tcp_shares = on,
            Experiment::Mobile => self.mobile = on,
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
    fn every_experiment_is_off_by_default_with_one_line_each() {
        let p = page(&Experiments::default());
        assert_eq!(p.title, "Experiments");
        let ids: Vec<&str> = p.sections[0].rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "experiment:cua_volume",
                "experiment:cua_volume-note",
                "experiment:ci_runners",
                "experiment:ci_runners-note",
                "experiment:tcp_shares",
                "experiment:tcp_shares-note",
                "experiment:mobile",
                "experiment:mobile-note",
            ]
        );
        for r in switches(&p) {
            assert!(
                r.options.iter().any(|o| o.id == "off" && o.active),
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
    fn the_always_on_features_have_no_switch_row_or_id() {
        let p = page(&Experiments::all_on());
        for id in ["secure_spaces", "ask_first", "child_spaces", "warm_pools"] {
            assert!(Experiment::parse(id).is_none(), "{id}");
            assert!(
                p.sections[0]
                    .rows
                    .iter()
                    .all(|r| !r.id.contains(id) && !r.id.contains(&id.replace('_', "-"))),
                "{id}"
            );
            assert_eq!(of_row(&format!("experiment:{id}")), None);
        }
        // A choice for one of them changes nothing.
        let x = Experiments::default();
        assert_eq!(choose(&x, "experiment:secure_spaces", "off"), x);
        assert_eq!(choose(&x, "experiment:ask_first", "on"), x);
        assert_eq!(choose(&x, "experiment:child_spaces", "off"), x);
        assert_eq!(choose(&x, "experiment:warm_pools", "on"), x);
    }

    #[test]
    fn a_switch_turns_one_experiment_on_and_off() {
        let off = Experiments::default();
        let x = choose(&off, "experiment:cua_volume", "on");
        assert_eq!(x.on_ids(), ["cua_volume"]);
        assert_eq!(choose(&x, "experiment:cua_volume", "off"), off);
        let v = choose(&x, "experiment:mobile", "on");
        assert_eq!(v.on_ids(), ["cua_volume", "mobile"]);
        assert_eq!(choose(&x, "notch", "on"), x, "other rows change nothing");
        assert_eq!(choose(&x, "experiment:nope", "on"), x);
        assert_eq!(
            Experiments::all_on().on_ids(),
            ["cua_volume", "ci_runners", "tcp_shares", "mobile"]
        );
    }

    #[test]
    fn stored_keys_are_forward_compatible() {
        // Keys of switches that are gone (an older build's `yourCloud`,
        // `sharing`, `webUi`) and of future ones are ignored.
        let x: Experiments = serde_json::from_str(
            r#"{"cuaVolume":true,"yourCloud":false,"sharing":false,"webUi":true,"secureSpaces":false,"askFirst":false,"childSpaces":false,"warmPools":true,"someFutureExperiment":true}"#,
        )
        .unwrap();
        assert!(x.cua_volume);
        let back = serde_json::to_value(x).unwrap();
        assert_eq!(
            back,
            serde_json::json!({"cuaVolume": true, "ciRunners": false, "tcpShares": false, "mobile": false})
        );
        assert_eq!(
            serde_json::from_str::<Experiments>("{}").unwrap(),
            Experiments::default()
        );
    }
}
