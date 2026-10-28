// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the Machines page adds from the account: the machines waiting to be
//! added ("Add machine", "Not me"), this machine's own trust, and who reached
//! each machine lately. It replaces Settings > Trusted machines: one list of
//! machines, with the state on the row and everything you can do to a machine
//! on its page.
//!
//! [`view`] reads what the relay says of the account's sign-ins and its
//! access log ([`crate::devices::DevicesInput`]) and answers in plain words:
//! "machines", never "devices", and no jargon about enrolling. The web UI
//! in the Mac app, in Electron and in the browser all draw this one view.

use serde::{Deserialize, Serialize};

use crate::devices::{self, BannerTone, DevicesInput, EnrollmentKind, devices_view, platform_name};

/// Newest access lines one machine's page shows.
pub const RECENT_PER_MACHINE: usize = 5;

/// "Add machine": what the person does to a machine that waits.
pub const ADD_LABEL: &str = "Add machine";
/// "Not me": the machine is not the person's.
pub const NOT_ME_LABEL: &str = "Not me";
/// The pending group's title.
pub const PENDING_TITLE: &str = "Waiting for you";
/// A machine's recent access, under its page.
pub const RECENT_TITLE: &str = "Recent access";
/// Nothing reached it yet.
pub const RECENT_EMPTY: &str = "No access yet.";

/// A pending machine's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PendingKind {
    /// A machine that registered to the account without proof it was the
    /// person's: Add machine confirms it, Not me removes it.
    Machine,
    /// A machine that signed in and waits for an added machine to approve it
    /// (or one that needs a quick check again): Add machine opens the
    /// "Add this machine?" sheet, Not me denies it.
    SignIn,
}

/// One machine waiting to be added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingMachine {
    /// Row id: the machine's, or the sign-in's.
    pub id: String,
    /// What kind of wait it is.
    pub kind: PendingKind,
    /// Its name.
    pub name: String,
    /// `macOS`, `Windows`, `Linux`, or empty.
    pub os: String,
    /// The row's one line.
    pub line: String,
    /// When it last reached the relay (Unix seconds): "asked 2 minutes ago".
    pub since: Option<u64>,
    /// The question the "Add this machine?" page asks.
    pub text: String,
    /// The relay machine (`kind` machine).
    pub machine: Option<String>,
    /// The sign-in (`kind` sign-in).
    pub device: Option<String>,
    /// It was added before and needs a quick check again.
    pub expired: bool,
    /// The approval needs the person present (Touch ID or a password).
    pub requires_presence: bool,
    /// Title and body of the notification that asks for it.
    pub notify_title: String,
    /// See `notify_title`.
    pub notify_body: String,
    /// "Add machine".
    pub add_label: String,
    /// "Not me".
    pub not_me_label: String,
}

/// This machine's own trust, in plain words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThisMachineTrust {
    /// `enrolled`, `grace`, `needs-enrollment`, ...: as the devices view says it.
    pub kind: EnrollmentKind,
    /// "Trusted until", "Not trusted yet", ... (a date follows when `at` is set).
    pub status: String,
    /// The date after the status (Unix seconds); the shell formats it.
    pub at: Option<u64>,
    /// What it means and what to do, when there is something to say.
    pub text: Option<String>,
    /// `warning` or `critical`.
    pub tone: Option<String>,
    /// The button that fixes it ("Trust this machine"), when there is one.
    pub action_label: Option<String>,
}

/// One line of who reached a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessLine {
    /// Unix seconds.
    pub ts: u64,
    /// In words.
    pub text: String,
    /// Another account or an untrusted machine.
    pub notable: bool,
    /// The machine reached.
    pub machine: Option<String>,
    /// The sign-in that reached it.
    pub device: Option<String>,
}

/// The account's part of the Machines page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesAccountView {
    /// Waiting machines, newest wait last.
    pub pending: Vec<PendingMachine>,
    /// How many wait (the Machines sidebar item's count).
    pub pending_count: usize,
    /// The group's title.
    pub pending_title: String,
    /// This machine's trust; none while it is unknown.
    pub this_machine: Option<ThisMachineTrust>,
    /// Recent access, newest first, for every machine.
    pub access: Vec<AccessLine>,
    /// "Recent access".
    pub recent_title: String,
    /// "No access yet."
    pub recent_empty: String,
    /// Why the account could not be read (the relay's words), when it could not.
    pub read_error: Option<String>,
}

/// The account's part of the Machines page at `now` (Unix seconds).
pub fn view(input: &DevicesInput, now: u64, read_error: Option<&str>) -> MachinesAccountView {
    let v = devices_view(input, now);
    let os_of = |id: &str| {
        input
            .devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| platform_name(d.platform.as_deref()))
            .unwrap_or_default()
    };
    let mut pending: Vec<PendingMachine> = v
        .unconfirmed_machines
        .iter()
        .map(|m| PendingMachine {
            id: m.id.clone(),
            kind: PendingKind::Machine,
            name: m.title.clone(),
            os: String::new(),
            line: "Waiting to be added".into(),
            since: None,
            text: format!(
                "Add \u{201c}{}\u{201d} to your machines? It signed in without proof it was you. Only add a machine you recognize.",
                m.title
            ),
            machine: Some(m.id.clone()),
            device: None,
            expired: false,
            requires_presence: false,
            notify_title: format!("Add \u{201c}{}\u{201d}?", m.title),
            notify_body: "A machine signed in to your Cua account and asks to be added.".into(),
            add_label: ADD_LABEL.into(),
            not_me_label: NOT_ME_LABEL.into(),
        })
        .collect();
    pending.extend(v.approvals.iter().map(|a| {
        PendingMachine {
            id: a.device_id.clone(),
            kind: PendingKind::SignIn,
            name: a.name.clone(),
            os: os_of(&a.device_id),
            line: if a.expired {
                "Needs a quick check".into()
            } else {
                "Waiting to be added".into()
            },
            since: input
                .devices
                .iter()
                .find(|d| d.id == a.device_id)
                .and_then(|d| d.last_seen),
            text: a.text.clone(),
            machine: None,
            device: Some(a.device_id.clone()),
            expired: a.expired,
            requires_presence: a.requires_presence,
            notify_title: a.notify_title.clone(),
            notify_body: a.notify_body.clone(),
            add_label: ADD_LABEL.into(),
            not_me_label: NOT_ME_LABEL.into(),
        }
    }));

    let this = &v.this_device;
    let this_machine = Some(ThisMachineTrust {
        kind: this.kind,
        status: this.title.clone(),
        at: this.at,
        text: v.banner.as_ref().map(|b| b.text.clone()),
        tone: v.banner.as_ref().map(|b| {
            match b.tone {
                BannerTone::Critical => "critical",
                _ => "warning",
            }
            .to_string()
        }),
        action_label: this.action_label.clone(),
    });

    let access: Vec<AccessLine> = input
        .audit
        .iter()
        .rev()
        .zip(v.activity.iter())
        .filter(|(e, _)| devices::is_access(&e.kind))
        .map(|(e, row)| AccessLine {
            ts: row.ts,
            text: row.text.clone(),
            notable: row.notable,
            machine: e.machine.clone(),
            device: e.device.clone(),
        })
        .take(devices::RECENT_ACCESS_LIMIT * 5)
        .collect();

    MachinesAccountView {
        pending_count: pending.len(),
        pending,
        pending_title: PENDING_TITLE.into(),
        this_machine,
        access,
        recent_title: RECENT_TITLE.into(),
        recent_empty: RECENT_EMPTY.into(),
        read_error: read_error
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(String::from),
    }
}

/// The access lines for one machine: those that reached its id, or its
/// sign-in's, newest first, at most [`RECENT_PER_MACHINE`].
pub fn access_for<'a>(
    lines: &'a [AccessLine],
    machine: &str,
    device: Option<&str>,
) -> Vec<&'a AccessLine> {
    lines
        .iter()
        .filter(|l| {
            l.machine.as_deref() == Some(machine)
                || device.is_some_and(|d| l.device.as_deref() == Some(d) && l.machine.is_none())
        })
        .take(RECENT_PER_MACHINE)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{AuditInput, DeviceInput, MachineInput};

    fn dev(id: &str, name: &str, state: &str, current: bool) -> DeviceInput {
        DeviceInput {
            id: id.into(),
            name: name.into(),
            state: state.into(),
            enrolled_until: Some(2_000_000_000),
            last_seen: Some(1_000),
            current,
            platform: Some("macos".into()),
        }
    }

    fn input() -> DevicesInput {
        DevicesInput {
            devices: vec![
                dev("dev_me", "My Mac", "enrolled", true),
                dev("dev_new", "Ana's laptop", "pending", false),
                dev("dev_old", "Studio", "expired", false),
            ],
            audit: vec![
                AuditInput {
                    ts: 10,
                    kind: "machine_access".into(),
                    device: Some("dev_old".into()),
                    machine: Some("studio".into()),
                    subject: None,
                    detail: None,
                },
                AuditInput {
                    ts: 20,
                    kind: "device_session".into(),
                    device: Some("dev_old".into()),
                    machine: None,
                    subject: None,
                    detail: None,
                },
                AuditInput {
                    ts: 30,
                    kind: "unenrolled_access".into(),
                    device: None,
                    machine: Some("studio".into()),
                    subject: None,
                    detail: None,
                },
            ],
            machines: vec![MachineInput {
                id: "unknown-box".into(),
                name: "unknown-box".into(),
                confirmed: false,
            }],
            ..DevicesInput::default()
        }
    }

    #[test]
    fn waiting_machines_come_first_with_two_choices_and_a_count() {
        let v = view(&input(), 100, None);
        assert_eq!(v.pending_count, 3);
        assert_eq!(v.pending_title, "Waiting for you");
        let names: Vec<(&str, PendingKind, &str)> = v
            .pending
            .iter()
            .map(|p| (p.name.as_str(), p.kind, p.line.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("unknown-box", PendingKind::Machine, "Waiting to be added"),
                ("Ana's laptop", PendingKind::SignIn, "Waiting to be added"),
                ("Studio", PendingKind::SignIn, "Needs a quick check"),
            ]
        );
        for p in &v.pending {
            assert_eq!(
                (p.add_label.as_str(), p.not_me_label.as_str()),
                ("Add machine", "Not me")
            );
            // The words are the machine's: no "device" and no "computer".
            for text in [&p.text, &p.line, &p.notify_title, &p.notify_body] {
                assert!(!text.to_lowercase().contains("device"), "{text}");
                assert!(!text.to_lowercase().contains("computer"), "{text}");
            }
        }
        assert_eq!(v.pending[1].os, "macOS");
        assert!(v.pending[1].requires_presence);
        assert_eq!(v.pending[1].device.as_deref(), Some("dev_new"));
        assert_eq!(v.pending[0].machine.as_deref(), Some("unknown-box"));
    }

    #[test]
    fn a_machine_that_is_not_added_yet_cannot_approve_others() {
        let mut i = input();
        i.devices[0].state = "pending".into();
        let v = view(&i, 100, None);
        // Only the relay's unconfirmed machine waits for this person; the other
        // sign-ins wait for a machine that is already trusted.
        assert_eq!(v.pending_count, 1);
        let me = v.this_machine.unwrap();
        assert_eq!(me.status, "Waiting to be added");
        assert_eq!(me.action_label.as_deref(), Some("Trust this machine"));
        assert!(!me.text.unwrap().to_lowercase().contains("device"));
    }

    #[test]
    fn this_machine_says_it_is_trusted_in_plain_words() {
        let v = view(&input(), 100, None);
        let me = v.this_machine.unwrap();
        assert_eq!(me.kind, EnrollmentKind::Enrolled);
        assert_eq!(me.status, "Trusted until");
        assert_eq!(me.at, Some(2_000_000_000));
        assert_eq!(me.action_label, None);
    }

    #[test]
    fn recent_access_is_per_machine_and_in_machine_words() {
        let v = view(&input(), 100, None);
        // The session is not access; the other two are, newest first.
        assert_eq!(v.access.len(), 2);
        assert!(v.access[0].notable);
        assert_eq!(v.access[0].text, "An untrusted machine used your account");
        let studio = access_for(&v.access, "studio", Some("dev_old"));
        assert_eq!(studio.len(), 2);
        assert!(access_for(&v.access, "other", None).is_empty());
        for l in &v.access {
            assert!(!l.text.to_lowercase().contains("device"), "{}", l.text);
        }
    }

    #[test]
    fn a_relay_that_refused_to_list_is_said_plainly() {
        let v = view(&DevicesInput::default(), 100, Some("  Needs approval  "));
        assert_eq!(v.read_error.as_deref(), Some("Needs approval"));
        assert_eq!(v.pending_count, 0);
    }
}
