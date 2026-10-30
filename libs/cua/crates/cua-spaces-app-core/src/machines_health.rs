// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A machine's issues on the Machines page: what its own cua daemon found
//! (`cua doctor --machine`'s checks, the SDK's `Spaces.machine_health`),
//! plus what this app knows of This Mac's service, in one plain sentence
//! each, with the fix where there is one.
//!
//! Only warnings and failures become issues; a healthy machine shows
//! nothing. A machine that is offline shows none either: its last checks
//! are not current, and the page already says it is offline.
//!
//! Shells fetch the health of the machines [`health_target`] names (This
//! Mac through its daemon, another of your machines through its daemon over
//! the relay or mesh), pass what came back in
//! [`crate::machines_view::MachinesViewInput::health`], and draw the
//! view's `issues` and `health`.

use serde::{Deserialize, Serialize};

use crate::host::{HostActionId, HostState};

/// The page's section title.
pub const ISSUES_TITLE: &str = "Issues";

/// One check as the machine's daemon reported it (the SDK's
/// `MachineHealthCheck`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewCheck {
    /// `machine.local_network`, `machine.disk`, ...
    pub id: String,
    /// `pass`, `warn`, `fail`, `skip`.
    pub status: String,
    /// What it found.
    #[serde(default)]
    pub message: String,
    /// What to do.
    #[serde(default)]
    pub remediation: String,
    /// `app`, `settings_url`, `free_gb`, ...
    #[serde(default)]
    pub facts: std::collections::HashMap<String, String>,
}

/// What the shell got for one machine's health.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewHealth {
    /// The machine's row id (`this-mac`, a relay machine id).
    pub id: String,
    /// Its checks.
    #[serde(default)]
    pub checks: Vec<MachinesViewCheck>,
    /// The Cua version of the daemon that checked.
    #[serde(default)]
    pub cua_version: String,
    /// Why it could not be checked, when it could not.
    #[serde(default)]
    pub error: Option<String>,
}

/// What fixes an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewIssueAction {
    /// "Open Local Network settings", "Free disk space", "Restart service".
    pub label: String,
    /// A System Settings pane to open (`host.openSettings`).
    #[serde(default)]
    pub url: Option<String>,
    /// A host action to run (`host.action`).
    #[serde(default)]
    pub host_action: Option<HostActionId>,
    /// Restart into the Cua Spaces update this app already downloaded
    /// (the shell's own update flow, the sidebar's "Restart to update").
    #[serde(default)]
    pub app_update: bool,
}

/// One issue on a machine's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewIssue {
    /// The check it comes from (`machine.disk`, `service`, `version`).
    pub id: String,
    /// `warn` or `fail`; `info` for a note that is no problem (it does not
    /// tint the row's dot).
    pub level: String,
    /// One plain sentence.
    pub text: String,
    /// Its fix, when there is one here.
    #[serde(default)]
    pub action: Option<MachinesViewIssueAction>,
}

/// What a shell passes to `Spaces.machine_health` for a row: `Some("")`
/// for This Mac, `Some(id)` for another of your machines that is online
/// and sharing, `None` when it can't be checked from here (offline, not
/// yours, a device that is not a machine).
pub fn health_target(
    current: bool,
    online: bool,
    stopped_sharing: bool,
    device_only: bool,
    owner: bool,
    id: &str,
) -> Option<String> {
    if current {
        Some(String::new())
    } else if online && !stopped_sharing && !device_only && owner {
        Some(id.to_string())
    } else {
        None
    }
}

/// What the issues are about.
pub struct Subject<'a> {
    /// This Mac.
    pub current: bool,
    /// Usable now (online and sharing); always for This Mac.
    pub live: bool,
    /// `macos`, `linux`, `windows`.
    pub os: &'a str,
    /// The Cua version This Mac's daemon reported, for the version check.
    pub local_version: Option<&'a str>,
    /// The Cua Spaces update this Mac's app found: `(version, ready)`, ready
    /// once downloaded. `None` when this app is the newest release.
    pub app_update: Option<(&'a str, bool)>,
}

/// The issues for one machine (pure). `host` is This Mac's host state,
/// read only for This Mac.
pub fn issues(
    subject: &Subject<'_>,
    health: Option<&MachinesViewHealth>,
    host: Option<&HostState>,
) -> Vec<MachinesViewIssue> {
    if !subject.current && !subject.live {
        return vec![];
    }
    let mut out = vec![];
    if subject.current
        && let Some(h) = host
        && h.configured
    {
        if crate::machines_view::service_down(h) {
            out.push(issue(
                "service",
                "fail",
                "The Cua service isn\u{2019}t running, so your other machines can\u{2019}t reach \
                 this Mac."
                    .into(),
                Some(MachinesViewIssueAction {
                    label: "Restart service".into(),
                    url: None,
                    host_action: Some(HostActionId::ResumeSharing),
                    app_update: false,
                }),
            ));
        } else if h.service_running && h.sharing && h.online == Some(false) {
            out.push(issue(
                "relay",
                "warn",
                "This Mac can\u{2019}t reach the Cua relay, so your other machines can\u{2019}t \
                 reach it. Check its internet connection."
                    .into(),
                None,
            ));
        }
    }
    let Some(health) = health else {
        return out;
    };
    if let Some(error) = health.error.as_deref().filter(|e| !e.trim().is_empty()) {
        out.push(issue(
            "unchecked",
            "warn",
            sentence(&format!("Couldn\u{2019}t check it: {}", trim_dot(error))),
            None,
        ));
        return out;
    }
    let there = if subject.current {
        String::new()
    } else {
        format!(" on {}", crate::machines_view::that_machine(subject.os))
    };
    for c in &health.checks {
        let level = match c.status.as_str() {
            "warn" => "warn",
            "fail" => "fail",
            _ => continue,
        };
        let fact = |k: &str| c.facts.get(k).map(String::as_str).unwrap_or("");
        let settings = |label: &str| {
            subject
                .current
                .then(|| fact("settings_url"))
                .filter(|u| u.starts_with("x-apple.systempreferences:"))
                .map(|u| MachinesViewIssueAction {
                    label: label.into(),
                    url: Some(u.into()),
                    host_action: None,
                    app_update: false,
                })
        };
        // This Mac's page already lists its permissions, each with its
        // own button (the host panel): not twice.
        if subject.current && c.id.starts_with("machine.permissions.") {
            continue;
        }
        let (text, action) = match c.id.as_str() {
            "machine.local_network" => {
                let app = Some(fact("app"))
                    .filter(|a| !a.is_empty())
                    .unwrap_or("Cua Spaces");
                (
                    if subject.current {
                        format!(
                            "Cua can\u{2019}t reach this Mac\u{2019}s macOS Spaces: Local Network \
                             access is off for {app}."
                        )
                    } else {
                        format!(
                            "Cua can\u{2019}t reach the macOS Spaces{there}: Local Network access \
                             is off for {app} there."
                        )
                    },
                    settings("Open Local Network settings"),
                )
            }
            "machine.disk" => {
                let free = fact("free_gb");
                (
                    if level == "fail" {
                        format!("Only {free} GB free{there}, too little for a new Space.")
                    } else {
                        format!("Only {free} GB free{there}. A macOS Space needs about 30 GB.")
                    },
                    settings("Free disk space"),
                )
            }
            "machine.memory" => (
                format!(
                    "Running Spaces use {} of {} GB of memory{there}, so another may not start.",
                    fact("used_gb"),
                    fact("total_gb")
                ),
                None,
            ),
            id if id.starts_with("machine.permissions.") => {
                let p = Some(fact("permission"))
                    .filter(|p| !p.is_empty())
                    .unwrap_or("a permission");
                (
                    format!("Sharing the desktop{there} needs {p} for cua-driver."),
                    settings(&format!("Open {p} settings")),
                )
            }
            _ => (sentence(&c.message), None),
        };
        out.push(issue(&c.id, level, text, action));
    }
    // Daemons a minor version or more apart (the relay and host calls
    // change between them).
    if !subject.current
        && let (Some(local), theirs) = (subject.local_version, health.cua_version.as_str())
        && let (Some(a), Some(b)) = (minor(local), minor(theirs))
        && a != b
    {
        out.push(if b < a {
            issue(
                "version",
                "warn",
                format!(
                    "It runs Cua {theirs}, older than this Mac\u{2019}s {local}. Update Cua on it."
                ),
                None,
            )
        } else {
            newer_peer(theirs, local, subject.app_update)
        });
    }
    out
}

/// The peer runs a newer Cua than this Mac. Only a Cua Spaces update fixes
/// that here (the app bundles its Cua), so the row offers exactly what
/// exists: the downloaded update to restart into, a download in progress,
/// or, when this app is already the newest release, a plain note.
fn newer_peer(theirs: &str, local: &str, update: Option<(&str, bool)>) -> MachinesViewIssue {
    let head = format!("It runs Cua {theirs}, newer than this Mac\u{2019}s {local}.");
    match update {
        Some((v, true)) => issue(
            "version",
            "warn",
            format!("{head} Cua Spaces {v} is ready to install."),
            Some(MachinesViewIssueAction {
                label: "Restart to update".into(),
                url: None,
                host_action: None,
                app_update: true,
            }),
        ),
        Some((v, false)) => issue(
            "version",
            "warn",
            format!("{head} Cua Spaces {v} is downloading and will update this Mac."),
            None,
        ),
        None => issue(
            "version",
            "info",
            format!(
                "{head} Cua Spaces will update this Mac in its next release; there is nothing \
                 to install yet."
            ),
            None,
        ),
    }
}

/// `fail` when any issue fails, `warn` when any warns, else empty (a note
/// does not count): the tint of the row's dot.
pub fn health_level(issues: &[MachinesViewIssue]) -> String {
    if issues.iter().any(|i| i.level == "fail") {
        "fail".into()
    } else if issues.iter().any(|i| i.level == "warn") {
        "warn".into()
    } else {
        String::new()
    }
}

fn issue(
    id: &str,
    level: &str,
    text: String,
    action: Option<MachinesViewIssueAction>,
) -> MachinesViewIssue {
    MachinesViewIssue {
        id: id.into(),
        level: level.into(),
        text,
        action,
    }
}

fn trim_dot(s: &str) -> &str {
    s.trim().trim_end_matches('.')
}

/// Capitalized, one trailing period.
fn sentence(s: &str) -> String {
    let s = trim_dot(s);
    let mut c = s.chars();
    match c.next() {
        Some(f) => format!("{}{}.", f.to_uppercase(), c.as_str()),
        None => String::new(),
    }
}

/// `(major, minor)` of a version like `0.4.1` or `v0.5.0-rc.1`.
fn minor(v: &str) -> Option<(u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').split(['.', '-', '+']);
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &str, status: &str, facts: &[(&str, &str)]) -> MachinesViewCheck {
        MachinesViewCheck {
            id: id.into(),
            status: status.into(),
            message: format!("{id} message"),
            remediation: String::new(),
            facts: facts
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn health(checks: Vec<MachinesViewCheck>, version: &str) -> MachinesViewHealth {
        MachinesViewHealth {
            id: "m".into(),
            checks,
            cua_version: version.into(),
            error: None,
        }
    }

    const THIS: Subject<'static> = Subject {
        current: true,
        live: true,
        os: "macos",
        local_version: Some("0.4.1"),
        app_update: None,
    };
    const OTHER: Subject<'static> = Subject {
        current: false,
        live: true,
        os: "macos",
        local_version: Some("0.4.1"),
        app_update: None,
    };

    #[test]
    fn a_healthy_machine_has_no_issues() {
        let h = health(
            vec![
                check("machine.disk", "pass", &[]),
                check("machine.memory", "skip", &[]),
            ],
            "0.4.1",
        );
        assert!(issues(&THIS, Some(&h), None).is_empty());
        assert!(issues(&OTHER, Some(&h), None).is_empty());
        assert_eq!(health_level(&[]), "");
    }

    #[test]
    fn local_network_on_this_mac_names_the_app_and_opens_settings() {
        let url = "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";
        let h = health(
            vec![check(
                "machine.local_network",
                "fail",
                &[("app", "Cua Spaces"), ("settings_url", url)],
            )],
            "0.4.1",
        );
        let i = issues(&THIS, Some(&h), None);
        assert_eq!(i.len(), 1);
        assert_eq!(
            i[0].text,
            "Cua can\u{2019}t reach this Mac\u{2019}s macOS Spaces: Local Network access is off for Cua Spaces."
        );
        let a = i[0].action.as_ref().unwrap();
        assert_eq!(a.label, "Open Local Network settings");
        assert_eq!(a.url.as_deref(), Some(url));
        assert_eq!(health_level(&i), "fail");
        // On another Mac: said there, with no button (its settings are there).
        let i = issues(&OTHER, Some(&h), None);
        assert!(i[0].text.contains("on that Mac"), "{}", i[0].text);
        assert!(i[0].action.is_none());
    }

    #[test]
    fn disk_memory_and_permissions_read_plainly() {
        let h = health(
            vec![
                check(
                    "machine.disk",
                    "warn",
                    &[
                        ("free_gb", "21"),
                        (
                            "settings_url",
                            "x-apple.systempreferences:com.apple.settings.Storage",
                        ),
                    ],
                ),
                check(
                    "machine.memory",
                    "warn",
                    &[("used_gb", "12"), ("total_gb", "16")],
                ),
                check(
                    "machine.permissions.screen-recording",
                    "warn",
                    &[
                        ("permission", "Screen Recording"),
                        ("settings_url", "x-apple.systempreferences:x"),
                    ],
                ),
            ],
            "0.4.1",
        );
        let i = issues(&THIS, Some(&h), None);
        let texts: Vec<&str> = i.iter().map(|i| i.text.as_str()).collect();
        // This Mac's permissions are listed on its page already.
        assert_eq!(
            texts,
            [
                "Only 21 GB free. A macOS Space needs about 30 GB.",
                "Running Spaces use 12 of 16 GB of memory, so another may not start.",
            ]
        );
        assert_eq!(i[0].action.as_ref().unwrap().label, "Free disk space");
        assert!(i[1].action.is_none());
        // Another Mac's: said there, without a button (its settings are there).
        let i = issues(&OTHER, Some(&h), None);
        assert_eq!(
            i[2].text,
            "Sharing the desktop on that Mac needs Screen Recording for cua-driver."
        );
        assert!(i[2].action.is_none());
        assert_eq!(health_level(&i), "warn");
    }

    #[test]
    fn an_offline_machine_shows_no_stale_issues() {
        let h = health(
            vec![check("machine.disk", "fail", &[("free_gb", "3")])],
            "0.4.1",
        );
        let offline = Subject {
            live: false,
            ..OTHER
        };
        assert!(issues(&offline, Some(&h), None).is_empty());
    }

    #[test]
    fn this_macs_stopped_service_offers_a_restart() {
        let host = HostState {
            configured: true,
            service_installed: true,
            service_running: false,
            sharing: true,
            provide_spaces: true,
            ..HostState::default()
        };
        let i = issues(&THIS, None, Some(&host));
        // Nothing to share (both switches off): the stopped service is
        // expected, so it is no issue.
        let nothing = HostState {
            provide_spaces: false,
            share_desktop: false,
            ..host.clone()
        };
        assert!(issues(&THIS, None, Some(&nothing)).is_empty());
        assert_eq!(i[0].id, "service");
        let a = i[0].action.as_ref().unwrap();
        assert_eq!(
            (a.label.as_str(), a.host_action),
            ("Restart service", Some(HostActionId::ResumeSharing))
        );
        let host = HostState {
            configured: true,
            service_installed: true,
            service_running: true,
            sharing: true,
            online: Some(false),
            ..HostState::default()
        };
        assert_eq!(issues(&THIS, None, Some(&host))[0].id, "relay");
    }

    #[test]
    fn version_skew_and_an_unchecked_machine_say_so() {
        let i = issues(&OTHER, Some(&health(vec![], "0.3.9")), None);
        assert_eq!(
            i[0].text,
            "It runs Cua 0.3.9, older than this Mac\u{2019}s 0.4.1. Update Cua on it."
        );
        assert!(issues(&OTHER, Some(&health(vec![], "0.4.7")), None).is_empty());
        let h = MachinesViewHealth {
            error: Some("Studio runs an older Cua that can't be checked; update Cua on it".into()),
            ..MachinesViewHealth::default()
        };
        assert_eq!(
            issues(&OTHER, Some(&h), None)[0].text,
            "Couldn\u{2019}t check it: Studio runs an older Cua that can't be checked; update Cua on it."
        );
    }

    #[test]
    fn a_newer_peer_offers_only_the_update_that_exists() {
        let h = health(vec![], "0.5.0");
        // This app is the newest release: a plain note, no warning, no button.
        let i = issues(&OTHER, Some(&h), None);
        assert_eq!((i[0].id.as_str(), i[0].level.as_str()), ("version", "info"));
        assert!(i[0].action.is_none());
        assert!(!i[0].text.contains("Update Cua here"));
        assert_eq!(health_level(&i), "");
        // The update is downloading: a warning, no button yet.
        let downloading = Subject {
            app_update: Some(("0.12.2", false)),
            ..OTHER
        };
        let i = issues(&downloading, Some(&h), None);
        assert_eq!(i[0].level, "warn");
        assert!(i[0].action.is_none());
        assert_eq!(health_level(&i), "warn");
        // Downloaded: "Restart to update".
        let ready = Subject {
            app_update: Some(("0.12.2", true)),
            ..OTHER
        };
        let i = issues(&ready, Some(&h), None);
        let a = i[0].action.as_ref().unwrap();
        assert_eq!(
            (a.label.as_str(), a.app_update),
            ("Restart to update", true)
        );
        assert!(i[0].text.contains("Cua Spaces 0.12.2"));
    }

    #[test]
    fn only_online_owned_machines_are_checked() {
        assert_eq!(
            health_target(true, false, false, false, false, "this-mac"),
            Some(String::new())
        );
        assert_eq!(
            health_target(false, true, false, false, true, "m1"),
            Some("m1".into())
        );
        assert_eq!(health_target(false, false, false, false, true, "m1"), None);
        assert_eq!(health_target(false, true, true, false, true, "m1"), None);
        assert_eq!(health_target(false, true, false, true, true, "d1"), None);
        assert_eq!(health_target(false, true, false, false, false, "t1"), None);
    }
}
