// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, About: the app's name and version, its links (acknowledgements,
//! privacy policy, terms, an issue report), the copyright line, and, where
//! the app updates itself (the macOS app, through Sparkle), the update
//! controls: automatic checks, automatic installs, the channel (Stable or
//! Beta), Check Now and the last check.
//!
//! Also what the first launch after an update does ([`after_launch`]): it
//! refreshes what cua installed into the coding agents (`cua agents
//! update`) and restarts the app's own daemon ([`restart_daemon`]), and
//! says nothing unless that failed ([`refresh_notice`]). The last version
//! seen lives in the settings file
//! ([`crate::settings::AppSettings::last_seen_version`]).

use serde::{Deserialize, Serialize};

use crate::settings::SettingsOption;

/// The privacy policy.
pub const PRIVACY_URL: &str = "https://cua.ai/privacy-policy";
/// The terms of service.
pub const TERMS_URL: &str = "https://cua.ai/terms-of-service";
/// A new issue from the repository's bug report form.
pub const ISSUE_URL: &str = "https://github.com/trycua/cua/issues/new";
/// The copyright line.
pub const COPYRIGHT: &str = "\u{a9} 2026 Cua AI, Inc. All rights reserved.";

/// Which releases the updater offers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Releases (the default).
    #[default]
    Stable,
    /// Releases and prereleases (`X.Y.Z-suffix`).
    Beta,
}

impl UpdateChannel {
    /// The id the channel picker and the settings file use.
    pub fn id(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
        }
    }

    /// The channel for a picker id (anything else is Stable).
    pub fn from_id(id: &str) -> UpdateChannel {
        if id == "beta" {
            UpdateChannel::Beta
        } else {
            UpdateChannel::Stable
        }
    }
}

/// The Sparkle channels an updater on `channel` may install from (Stable:
/// none but the default one).
pub fn allowed_channels(channel: UpdateChannel) -> Vec<String> {
    match channel {
        UpdateChannel::Stable => vec![],
        UpdateChannel::Beta => vec!["beta".into()],
    }
}

/// What the About pane shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AboutInput {
    /// `macos`, `linux` or `windows`.
    pub platform: String,
    /// The full version (`0.2.0`, `0.2.0-staging.6`).
    pub version: String,
    /// The build (the macOS app's `CFBundleVersion`, `0.2.0.123`; may be
    /// empty).
    pub build: String,
    /// This machine for an issue report (`macOS 26.0 (arm64)`).
    pub os: String,
    /// The app updates itself (its updater is running).
    pub updater: bool,
    /// The updater checks on its own.
    pub auto_check: bool,
    /// The updater downloads and installs on its own.
    pub auto_install: bool,
    /// The chosen channel.
    pub channel: UpdateChannel,
    /// The last check, as the shell formats a date; `None`: never.
    pub last_check: Option<String>,
    /// A check is running.
    pub checking: bool,
}

/// Which link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AboutLinkId {
    /// The third-party notices the app bundles.
    Acknowledgements,
    /// The privacy policy.
    Privacy,
    /// The terms of service.
    Terms,
    /// A new issue.
    Issue,
}

/// One link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutLink {
    /// Which.
    pub id: AboutLinkId,
    /// "Privacy Policy".
    pub label: String,
    /// What it opens; `None`: the notices the app bundles.
    pub url: Option<String>,
}

/// The update controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutUpdates {
    /// "Automatically check for updates".
    pub auto_check_label: String,
    /// Checked.
    pub auto_check: bool,
    /// "Install updates automatically".
    pub auto_install_label: String,
    /// Checked.
    pub auto_install: bool,
    /// It can be used (only with automatic checks).
    pub auto_install_enabled: bool,
    /// "Update to:".
    pub channel_label: String,
    /// Stable, Beta.
    pub channels: Vec<SettingsOption>,
    /// The channel picker's tooltip.
    pub channel_help: String,
    /// "Check Now" (or "Checking…").
    pub check_label: String,
    /// Check Now can be used.
    pub check_enabled: bool,
    /// "Last check: …".
    pub last_check: String,
}

/// Settings, About.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutView {
    /// "Cua Spaces for macOS".
    pub title: String,
    /// "Version 0.2.0 (0.2.0.123)".
    pub version_line: String,
    /// Acknowledgements, Privacy Policy, Terms of Service, Report an Issue.
    pub links: Vec<AboutLink>,
    /// "© 2026 Cua AI, Inc. All rights reserved."
    pub copyright: String,
    /// The update controls, when the app updates itself.
    pub updates: Option<AboutUpdates>,
}

fn platform_name(platform: &str) -> &'static str {
    match platform {
        "windows" => "Windows",
        "linux" => "Linux",
        _ => "macOS",
    }
}

/// `s` for a URL query value (RFC 3986 unreserved characters kept).
fn query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// "Version 0.2.0 (0.2.0.123)" (no build: "Version 0.2.0").
pub fn version_line(version: &str, build: &str) -> String {
    if build.is_empty() || build == version {
        format!("Version {version}")
    } else {
        format!("Version {version} ({build})")
    }
}

/// The bug report form with the app and this machine filled in.
pub fn issue_url(input: &AboutInput) -> String {
    let mut environment = format!(
        "Cua Spaces for {} {}",
        platform_name(&input.platform),
        version_line(&input.version, &input.build).trim_start_matches("Version ")
    );
    if !input.os.is_empty() {
        environment.push('\n');
        environment.push_str(&input.os);
    }
    format!(
        "{ISSUE_URL}?template=bug.yml&environment={}",
        query_value(&environment)
    )
}

/// The About pane for `input`.
pub fn view(input: &AboutInput) -> AboutView {
    let link = |id, label: &str, url: Option<String>| AboutLink {
        id,
        label: label.into(),
        url,
    };
    let updates = input.updater.then(|| AboutUpdates {
        auto_check_label: "Automatically check for updates".into(),
        auto_check: input.auto_check,
        auto_install_label: "Install updates automatically".into(),
        auto_install: input.auto_install,
        auto_install_enabled: input.auto_check,
        channel_label: "Update to:".into(),
        channels: [
            (UpdateChannel::Stable, "Stable"),
            (UpdateChannel::Beta, "Beta"),
        ]
        .into_iter()
        .map(|(c, label)| SettingsOption {
            id: c.id().into(),
            label: label.into(),
            active: c == input.channel,
        })
        .collect(),
        channel_help: "Stable gets tested releases. Beta also gets prereleases: new features \
                       sooner, with rough edges. Back on Stable, this version stays until a \
                       newer stable release."
            .into(),
        check_label: if input.checking {
            "Checking\u{2026}".into()
        } else {
            "Check Now".into()
        },
        check_enabled: !input.checking,
        last_check: format!(
            "Last check: {}",
            input.last_check.as_deref().unwrap_or("Never")
        ),
    });
    AboutView {
        title: format!("Cua Spaces for {}", platform_name(&input.platform)),
        version_line: version_line(&input.version, &input.build),
        links: vec![
            link(AboutLinkId::Acknowledgements, "Acknowledgements", None),
            link(
                AboutLinkId::Privacy,
                "Privacy Policy",
                Some(PRIVACY_URL.into()),
            ),
            link(
                AboutLinkId::Terms,
                "Terms of Service",
                Some(TERMS_URL.into()),
            ),
            link(
                AboutLinkId::Issue,
                "Report an Issue\u{2026}",
                Some(issue_url(input)),
            ),
        ],
        copyright: COPYRIGHT.into(),
        updates,
    }
}

/// A launch: the version last seen and this one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchInput {
    /// `AppSettings.last_seen_version` (`None`: never recorded).
    pub last_seen: Option<String>,
    /// This version (`0.2.0-staging.6`).
    pub version: String,
    /// This build (`0.2.0.123`; may be empty).
    pub build: String,
    /// The first run finished (a record-less launch after it is an update
    /// from a build that kept no record; before it, a fresh install).
    pub onboarded: bool,
}

/// What a launch does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    /// This launch follows an update: refresh the agents' skills and MCP
    /// entries (`cua agents update`) and restart the app's own daemon.
    pub refresh: bool,
    /// Save as `AppSettings.last_seen_version` (`None`: unchanged).
    pub save: Option<String>,
}

/// The version and build as one record ("0.2.0-staging.6 (0.2.0.123)").
pub fn identity(version: &str, build: &str) -> String {
    version_line(version, build)
        .trim_start_matches("Version ")
        .to_string()
}

/// What this launch does: refresh after any change of version or build
/// (an update, or a downgrade), never on a fresh install, never twice.
pub fn after_launch(input: &LaunchInput) -> LaunchPlan {
    let current = identity(&input.version, &input.build);
    match input.last_seen.as_deref() {
        Some(seen) if seen == current => LaunchPlan {
            refresh: false,
            save: None,
        },
        Some(_) => LaunchPlan {
            refresh: true,
            save: Some(current),
        },
        None => LaunchPlan {
            refresh: input.onboarded,
            save: Some(current),
        },
    }
}

/// The running daemon, for [`restart_daemon`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DaemonCheck {
    /// The running daemon's executable (`None`: no daemon, or unknown).
    pub daemon_exe: Option<String>,
    /// This app's bundle (`/Applications/Cua Spaces.app`).
    pub bundle: String,
}

/// Restart the daemon after an update only when it is this app's own: its
/// executable is inside this app's bundle (the bundled `cua`, which the
/// installed `~/.local/bin/cua` links to). A daemon from another `cua` is
/// someone else's and keeps running.
pub fn restart_daemon(check: &DaemonCheck) -> bool {
    let bundle = check.bundle.trim_end_matches('/');
    match check.daemon_exe.as_deref() {
        Some(exe) if !bundle.is_empty() => exe
            .strip_prefix(bundle)
            .is_some_and(|rest| rest.starts_with('/')),
        _ => false,
    }
}

/// How the refresh after an update went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RefreshReport {
    /// `cua agents update` failed (its words).
    pub agents_error: Option<String>,
    /// Restarting the daemon failed (its words).
    pub daemon_error: Option<String>,
}

/// The one quiet notice after a refresh, or nothing when it worked.
pub fn refresh_notice(report: &RefreshReport) -> Option<String> {
    let detail = |e: &str| {
        let e = e.trim().trim_end_matches('.');
        if e.is_empty() {
            String::new()
        } else {
            format!(" ({e})")
        }
    };
    match (
        report.agents_error.as_deref(),
        report.daemon_error.as_deref(),
    ) {
        (None, None) => None,
        (Some(a), None) => Some(format!(
            "Cua Spaces updated, but could not refresh the Cua skills in your AI agents{}. \
             Run `cua agents update` to try again.",
            detail(a)
        )),
        (None, Some(d)) => Some(format!(
            "Cua Spaces updated, but could not restart the Cua daemon{}. Quit and reopen \
             Cua Spaces to use the new version.",
            detail(d)
        )),
        (Some(_), Some(_)) => Some(
            "Cua Spaces updated, but could not refresh the Cua skills in your AI agents or \
             restart the Cua daemon. Run `cua agents update`, then quit and reopen Cua Spaces."
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> AboutInput {
        AboutInput {
            platform: "macos".into(),
            version: "0.2.0".into(),
            build: "0.2.0.123".into(),
            os: "macOS 26.0 (arm64)".into(),
            updater: true,
            auto_check: true,
            auto_install: false,
            channel: UpdateChannel::Stable,
            last_check: None,
            checking: false,
        }
    }

    #[test]
    fn the_pane_names_the_app_and_its_version() {
        let v = view(&input());
        assert_eq!(v.title, "Cua Spaces for macOS");
        assert_eq!(v.version_line, "Version 0.2.0 (0.2.0.123)");
        assert_eq!(v.copyright, "\u{a9} 2026 Cua AI, Inc. All rights reserved.");
        let labels: Vec<_> = v.links.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Acknowledgements",
                "Privacy Policy",
                "Terms of Service",
                "Report an Issue\u{2026}"
            ]
        );
        assert_eq!(v.links[0].url, None);
        assert_eq!(
            v.links[1].url.as_deref(),
            Some("https://cua.ai/privacy-policy")
        );
        assert_eq!(
            v.links[2].url.as_deref(),
            Some("https://cua.ai/terms-of-service")
        );
    }

    #[test]
    fn the_issue_form_carries_the_app_and_the_os() {
        assert_eq!(
            issue_url(&input()),
            "https://github.com/trycua/cua/issues/new?template=bug.yml&environment=\
             Cua%20Spaces%20for%20macOS%200.2.0%20%280.2.0.123%29%0AmacOS%2026.0%20%28arm64%29"
        );
    }

    #[test]
    fn update_controls_follow_the_updater() {
        let u = view(&input()).updates.unwrap();
        assert!(u.auto_check && !u.auto_install && u.auto_install_enabled);
        assert_eq!(u.last_check, "Last check: Never");
        assert_eq!(u.check_label, "Check Now");
        assert!(u.check_enabled);
        assert_eq!(
            u.channels
                .iter()
                .filter(|o| o.active)
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>(),
            ["stable"]
        );

        let mut i = input();
        i.auto_check = false;
        i.channel = UpdateChannel::Beta;
        i.last_check = Some("9/30/26, 3:04 PM".into());
        i.checking = true;
        let u = view(&i).updates.unwrap();
        assert!(!u.auto_install_enabled);
        assert_eq!(u.last_check, "Last check: 9/30/26, 3:04 PM");
        assert_eq!(u.check_label, "Checking\u{2026}");
        assert!(!u.check_enabled);
        assert!(u.channels[1].active && !u.channels[0].active);

        i.updater = false;
        assert!(view(&i).updates.is_none());
    }

    #[test]
    fn channels_map_to_sparkle() {
        assert!(allowed_channels(UpdateChannel::Stable).is_empty());
        assert_eq!(allowed_channels(UpdateChannel::Beta), ["beta"]);
        assert_eq!(UpdateChannel::from_id("beta"), UpdateChannel::Beta);
        assert_eq!(UpdateChannel::from_id("nightly"), UpdateChannel::Stable);
    }

    fn launch(last_seen: Option<&str>, version: &str, build: &str, onboarded: bool) -> LaunchPlan {
        after_launch(&LaunchInput {
            last_seen: last_seen.map(Into::into),
            version: version.into(),
            build: build.into(),
            onboarded,
        })
    }

    #[test]
    fn a_launch_after_an_update_refreshes_once() {
        let plan = launch(
            Some("0.2.0-staging.5 (0.2.0.105)"),
            "0.2.0-staging.6",
            "0.2.0.106",
            true,
        );
        assert!(plan.refresh);
        assert_eq!(plan.save.as_deref(), Some("0.2.0-staging.6 (0.2.0.106)"));
        // The next launch of the same build does nothing.
        let again = launch(plan.save.as_deref(), "0.2.0-staging.6", "0.2.0.106", true);
        assert_eq!(
            again,
            LaunchPlan {
                refresh: false,
                save: None
            }
        );
        // A rebuild of the same version is a change too.
        assert!(launch(Some("0.2.0 (0.2.0.1)"), "0.2.0", "0.2.0.2", true).refresh);
        // So is a downgrade.
        assert!(launch(Some("0.3.0 (0.3.0.9)"), "0.2.0", "0.2.0.8", true).refresh);
    }

    #[test]
    fn a_fresh_install_records_without_refreshing() {
        let plan = launch(None, "0.2.0", "0.2.0.1", false);
        assert!(!plan.refresh);
        assert_eq!(plan.save.as_deref(), Some("0.2.0 (0.2.0.1)"));
        // No record after the first run: an update from a build that kept none.
        assert!(launch(None, "0.2.0", "0.2.0.1", true).refresh);
    }

    #[test]
    fn only_the_apps_own_daemon_restarts() {
        let check = |exe: Option<&str>| {
            restart_daemon(&DaemonCheck {
                daemon_exe: exe.map(Into::into),
                bundle: "/Applications/Cua Spaces.app".into(),
            })
        };
        assert!(check(Some(
            "/Applications/Cua Spaces.app/Contents/MacOS/cua"
        )));
        assert!(!check(Some(
            "/Applications/Cua Spaces.app.old/Contents/MacOS/cua"
        )));
        assert!(!check(Some("/Users/me/.cargo/bin/cua")));
        assert!(!check(None));
        assert!(!restart_daemon(&DaemonCheck {
            daemon_exe: Some("/x/cua".into()),
            bundle: String::new()
        }));
    }

    #[test]
    fn the_notice_is_silent_unless_something_failed() {
        assert_eq!(refresh_notice(&RefreshReport::default()), None);
        let a = refresh_notice(&RefreshReport {
            agents_error: Some("exit status 1.".into()),
            daemon_error: None,
        })
        .unwrap();
        assert_eq!(
            a,
            "Cua Spaces updated, but could not refresh the Cua skills in your AI agents \
             (exit status 1). Run `cua agents update` to try again."
        );
        assert!(
            refresh_notice(&RefreshReport {
                agents_error: None,
                daemon_error: Some(String::new()),
            })
            .unwrap()
            .contains("restart the Cua daemon. Quit")
        );
        assert!(
            refresh_notice(&RefreshReport {
                agents_error: Some("a".into()),
                daemon_error: Some("b".into()),
            })
            .unwrap()
            .contains("or restart the Cua daemon")
        );
    }
}
