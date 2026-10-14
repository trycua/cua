// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Experiments: features still being built, one switch each, all
//! off by default except the New UI, which [`new_ui_default`] turns on for
//! the Beta channel and then for every release.
//!
//! - Cua Volume ([`Experiment::CuaVolume`]): the first run's Volume page
//!   (and its Done line) and Settings' Storage section;
//! - Your cloud ([`Experiment::YourCloud`]): the New Space wizard's
//!   connected clouds (AWS, Google Cloud, Modal), "Connect a cloud…" and a
//!   default location in a cloud;
//! - Sharing ([`Experiment::Sharing`]): a Space's Share button;
//! - New UI ([`Experiment::WebUi`]): the shared web UI as the main window
//!   (the macOS shell). Settings shows it as "Use classic UI"
//!   ([`CLASSIC_UI_ROW`]), the way back to the SwiftUI window.
//!
//! Off only hides the entry points. Nothing is undone: a mounted Volume
//! stays mounted (and keeps syncing), a connected cloud stays connected
//! and its Spaces stay in the list, and a shared Space stays shared. Turning
//! the switch on again shows them as they are.
//!
//! The switches live in the app's settings ([`crate::settings::AppSettings`],
//! `experiments`): one key per experiment, missing keys read as off and
//! unknown ones are ignored, so an older or newer build reads the file.
//!
//! The New UI is the one switch with a default of its own: `web_ui` is
//! what this launch shows, the user's own choice lives in
//! [`crate::settings::AppSettings::new_ui`] and, without one,
//! [`new_ui_default`] decides ([`crate::settings::resolve_new_ui`]).

use serde::{Deserialize, Serialize};

use crate::about::UpdateChannel;

use crate::settings::{
    SettingsOption, SettingsPage, SettingsRow, SettingsRowKind, SettingsSection,
};

/// The switches, as stored (`"experiments": {"cuaVolume": true}`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Experiments {
    /// Cua Volume in the first run and Settings, Storage.
    pub cua_volume: bool,
    /// Your own clouds in the New Space wizard.
    pub your_cloud: bool,
    /// Sharing a Space with other accounts.
    pub sharing: bool,
    /// The New UI (the shared web UI) is the main window: the user's choice
    /// or, without one, [`new_ui_default`] ([`crate::settings::resolve_new_ui`]).
    pub web_ui: bool,
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
}

/// Every experiment, in the order Settings lists them.
pub const ALL: [Experiment; 4] = [
    Experiment::CuaVolume,
    Experiment::YourCloud,
    Experiment::Sharing,
    Experiment::WebUi,
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
        }
    }

    /// The one line under the switch.
    pub fn description(self) -> &'static str {
        match self {
            Experiment::CuaVolume => "Memory and files your agents share across every Space.",
            Experiment::YourCloud => {
                "Create Spaces in your own AWS, Google Cloud or Modal account."
            }
            Experiment::Sharing => "Share a Space with other Cua accounts.",
            Experiment::WebUi => "The Spaces window built on the shared web UI.",
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
        }
    }

    /// With `e` turned `on`.
    pub fn with(mut self, e: Experiment, on: bool) -> Self {
        match e {
            Experiment::CuaVolume => self.cua_volume = on,
            Experiment::YourCloud => self.your_cloud = on,
            Experiment::Sharing => self.sharing = on,
            Experiment::WebUi => self.web_ui = on,
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

/// The New UI's row on the Experiments tab: "Use classic UI", on while the
/// New UI is off. (`experiment:web_ui`, on for the New UI, is still read.)
pub const CLASSIC_UI_ROW: &str = "experiment:classic_ui";

/// From this version the New UI is on by default for the Beta channel:
/// every 0.8.1 prerelease (`0.8.1-0` is the lowest SemVer allows; 0.8.0
/// shipped without the default).
pub const NEW_UI_BETA_FROM: &str = "0.8.1-0";

/// The first release with the New UI on by default on every channel.
pub const NEW_UI_ALL_FROM: &str = "0.8.1";

/// Whether the New UI is on for someone who never chose: on the Beta
/// channel from [`NEW_UI_BETA_FROM`], everywhere from [`NEW_UI_ALL_FROM`]
/// (so 0.8.1 turns it on for Stable with no change here). `version` is the
/// running app's (`0.8.1-beta.1`); one that does not read as a version
/// (a dev build's, or none) keeps the classic UI.
pub fn new_ui_default(channel: UpdateChannel, version: &str) -> bool {
    let Some(v) = Version::parse(version) else {
        return false;
    };
    let at_least = |from: &str| Version::parse(from).is_some_and(|f| v >= f);
    at_least(NEW_UI_ALL_FROM) || (channel == UpdateChannel::Beta && at_least(NEW_UI_BETA_FROM))
}

/// The New UI choice a row's option makes: `Some(on)` for the New UI's
/// row ("Use classic UI" on is the New UI off), `None` for any other row or
/// option. The shell saves it as [`crate::settings::AppSettings::new_ui`].
pub fn new_ui_choice(row: &str, option: &str) -> Option<bool> {
    match (row, option) {
        (CLASSIC_UI_ROW, "on") => Some(false),
        (CLASSIC_UI_ROW, "off") => Some(true),
        (r, "on") if of_row(r) == Some(Experiment::WebUi) => Some(true),
        (r, "off") if of_row(r) == Some(Experiment::WebUi) => Some(false),
        _ => None,
    }
}

/// A release version, compared as SemVer does (`0.8.0-beta.6` < `0.8.0`;
/// build metadata after `+` ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    pre: Vec<PreId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PreId {
    // Numeric identifiers sort before alphanumeric ones.
    Num(u64),
    Alpha(String),
}

impl Version {
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let mut parts = core.split('.');
        let mut n = [0u64; 3];
        for slot in &mut n {
            *slot = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => vec![],
            Some(p) => p
                .split('.')
                .map(|id| match id.parse() {
                    Ok(n) => Some(PreId::Num(n)),
                    Err(_) if !id.is_empty() => Some(PreId::Alpha(id.to_string())),
                    Err(_) => None,
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Self { core: n, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A release sorts after its prereleases.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

/// The Experiments tab: one switch per experiment, each with its one line.
pub fn page(experiments: &Experiments) -> SettingsPage {
    let mut rows = Vec::new();
    for e in ALL {
        if e == Experiment::WebUi {
            // "Use classic UI": on while the New UI is off.
            let classic = !experiments.web_ui;
            let mut toggle =
                crate::settings::row(CLASSIC_UI_ROW, SettingsRowKind::Toggle, "Use classic UI");
            toggle.options = vec![
                SettingsOption {
                    id: "on".into(),
                    label: "On".into(),
                    active: classic,
                },
                SettingsOption {
                    id: "off".into(),
                    label: "Off".into(),
                    active: !classic,
                },
            ];
            rows.push(toggle);
            rows.push(crate::settings::row(
                &format!("{CLASSIC_UI_ROW}-note"),
                SettingsRowKind::Note,
                "Open the previous Spaces window instead of the New UI.",
            ));
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
/// other row or option.
pub fn choose(experiments: &Experiments, row: &str, option: &str) -> Experiments {
    if let Some(on) = new_ui_choice(row, option) {
        return experiments.with(Experiment::WebUi, on);
    }
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
                "experiment:your_cloud",
                "experiment:your_cloud-note",
                "experiment:sharing",
                "experiment:sharing-note",
                "experiment:classic_ui",
                "experiment:classic_ui-note",
            ]
        );
        for r in switches(&p) {
            // Off, the New UI shows as "Use classic UI" on.
            let shown = if r.id == CLASSIC_UI_ROW { "on" } else { "off" };
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
        let x = choose(&Experiments::default(), "experiment:sharing", "on");
        assert_eq!(
            x,
            Experiments {
                sharing: true,
                ..Default::default()
            }
        );
        assert_eq!(x.on_ids(), ["sharing"]);
        assert_eq!(
            choose(&x, "experiment:sharing", "off"),
            Experiments::default()
        );
        assert_eq!(choose(&x, "notch", "on"), x, "other rows change nothing");
        assert_eq!(choose(&x, "experiment:nope", "on"), x);
        assert_eq!(
            Experiments::all_on().on_ids(),
            ["cua_volume", "your_cloud", "sharing", "web_ui"]
        );
    }

    #[test]
    fn use_classic_ui_is_the_new_ui_switch_inverted() {
        let on = Experiments {
            web_ui: true,
            ..Default::default()
        };
        let row = |x: &Experiments| {
            let p = page(x);
            let r = switches(&p)
                .into_iter()
                .find(|r| r.id == CLASSIC_UI_ROW)
                .cloned()
                .unwrap();
            (
                r.label.clone(),
                r.options.iter().find(|o| o.active).unwrap().id.clone(),
            )
        };
        assert_eq!(row(&on), ("Use classic UI".to_string(), "off".to_string()));
        assert_eq!(row(&Experiments::default()).1, "on");
        // Classic on is the New UI off, and back.
        assert_eq!(choose(&on, CLASSIC_UI_ROW, "on"), Experiments::default());
        assert_eq!(choose(&Experiments::default(), CLASSIC_UI_ROW, "off"), on);
        // The older row id still means the New UI itself.
        assert_eq!(
            choose(&Experiments::default(), "experiment:web_ui", "on"),
            on
        );
        assert_eq!(
            choose(&on, "experiment:web_ui", "off"),
            Experiments::default()
        );
        assert_eq!(new_ui_choice(CLASSIC_UI_ROW, "on"), Some(false));
        assert_eq!(new_ui_choice(CLASSIC_UI_ROW, "off"), Some(true));
        assert_eq!(new_ui_choice("experiment:web_ui", "on"), Some(true));
        assert_eq!(new_ui_choice("experiment:sharing", "on"), None);
        assert_eq!(new_ui_choice(CLASSIC_UI_ROW, "maybe"), None);
    }

    #[test]
    fn the_new_ui_is_on_for_beta_from_0_8_1_betas_and_for_everyone_from_0_8_1() {
        use UpdateChannel::{Beta, Stable};
        for (channel, version, on) in [
            // Before: classic on both channels, 0.8.0 included (it shipped
            // without the default).
            (Beta, "0.7.2", false),
            (Stable, "0.7.2", false),
            (Beta, "0.8.0-beta.5", false),
            (Beta, "0.8.0-beta.6", false),
            (Beta, "0.8.0", false),
            (Stable, "0.8.0", false),
            // Beta: every 0.8.1 prerelease, from the first.
            (Beta, "0.8.1-0", true),
            (Beta, "0.8.1-beta.0", true),
            (Beta, "0.8.1-beta.1", true),
            (Beta, "0.8.1-beta.10", true),
            (Beta, "0.8.1-rc.1", true),
            (Beta, "0.8.1-beta.1+build.7", true),
            // Stable keeps classic until 0.8.1.
            (Stable, "0.8.1-beta.1", false),
            (Stable, "0.8.1-rc.1", false),
            (Stable, "0.8.1", true),
            (Beta, "0.8.1", true),
            (Stable, "0.8.2", true),
            (Stable, "0.9.0-beta.1", true),
            (Stable, "1.0.0", true),
            // Not a version: classic.
            (Beta, "", false),
            (Beta, "dev", false),
            (Beta, "0.8", false),
        ] {
            assert_eq!(
                new_ui_default(channel, version),
                on,
                "{channel:?} {version}"
            );
        }
    }

    #[test]
    fn versions_compare_as_semver() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert!(
            v("0.8.0-beta.6") < v("0.8.0-beta.10"),
            "numeric parts as numbers"
        );
        assert!(v("0.8.0-beta.6") < v("0.8.0-rc.1"));
        assert!(v("0.8.0-rc.1") < v("0.8.0"));
        assert!(v("0.7.2") < v("0.8.0-beta.1"));
        assert!(v("0.8.0-beta") < v("0.8.0-beta.1"));
        assert!(v("0.8.0") < v("0.8.1-0"));
        assert!(
            v("0.8.1-0") < v("0.8.1-beta.0"),
            "numeric before alphanumeric"
        );
        assert_eq!(v("v0.8.0+abc"), v("0.8.0"));
        assert!(Version::parse("0.8.0-").is_none());
        assert!(Version::parse("0.8.0.1").is_none());
    }

    #[test]
    fn stored_keys_are_forward_compatible() {
        let x: Experiments =
            serde_json::from_str(r#"{"cuaVolume":true,"someFutureExperiment":true}"#).unwrap();
        assert!(x.cua_volume && !x.your_cloud && !x.sharing && !x.web_ui);
        let back = serde_json::to_value(x).unwrap();
        assert_eq!(
            back,
            serde_json::json!({"cuaVolume": true, "yourCloud": false, "sharing": false, "webUi": false})
        );
        assert_eq!(
            serde_json::from_str::<Experiments>("{}").unwrap(),
            Experiments::default()
        );
    }
}
