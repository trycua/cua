// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Volume": Cua Volume's status (Open in Finder, where it is mounted, sync
//! per device, conflicts), the access agents ask for and the grants in
//! force. Not a file browser: Finder is.
//!
//! Plain data in (`volume_requests`, `volume_grants`, `volume_mount_status`
//! and `volume_sync_status`, as the user) and plain data out (the Open
//! button, one line per request, grant, device and conflict, and the
//! command to run). Approving a request widens an
//! agent's access, so the daemon asks for presence before it grants;
//! denying and revoking only narrow.

use serde::{Deserialize, Serialize};

use crate::drive_settings::DriveMountInput;
use crate::persistent::{LineView, ago};

/// An access request (`volume_requests`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DriveRequestInput {
    pub id: String,
    /// `agent:ada`.
    pub principal: String,
    pub prefix: String,
    /// `r` or `rw`.
    pub mode: String,
    pub reason: String,
}

/// A grant (`volume_grants`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DriveGrantInput {
    pub id: String,
    pub principal: String,
    pub prefix: String,
    pub mode: String,
    pub revoked: bool,
}

/// A device in the change feed (`volume_sync_status.devices`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveDeviceInput {
    pub id: String,
    pub name: String,
    pub this_device: bool,
    /// Its last heartbeat or change (Unix ms).
    pub last_seen_ms: u64,
    pub last_change_ms: u64,
}

/// A file two devices wrote (`volume_sync_status.conflicts`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveConflictInput {
    /// The file (the drive's path).
    pub path: String,
    /// The visible copy holding the losing write.
    pub conflict_path: String,
    pub winner_device: String,
    pub loser_device: String,
    pub ts_ms: u64,
}

/// `volume_sync_status` (snake_case, the tool's shape).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveSyncInput {
    pub device_id: String,
    pub device_name: String,
    /// `live`, `off` (one device on this machine's store), `error`.
    pub feed: String,
    /// This device's last poll of the feed (Unix ms).
    pub last_poll_ms: u64,
    /// Local writes not yet in the bucket.
    pub pending_uploads: u32,
    pub conflicts: Vec<DriveConflictInput>,
    /// This device first.
    pub devices: Vec<DriveDeviceInput>,
    pub last_error: Option<String>,
}

/// Everything the Drive page reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DriveInput {
    pub requests: Vec<DriveRequestInput>,
    pub grants: Vec<DriveGrantInput>,
    /// Now (Unix ms), for "synced 5m ago".
    pub now_ms: u64,
    /// `volume_mount_status`, once read.
    pub mount: Option<DriveMountInput>,
    /// `volume_sync_status`, once read.
    pub sync: Option<DriveSyncInput>,
    /// The home folder, to show the mount point as `~/...`.
    pub home: Option<String>,
}

/// The command the shell runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DriveRequest {
    /// Read the page: `volume_requests`, `volume_grants`,
    /// `volume_mount_status`, `volume_sync_status`.
    Load,
    /// `volume_mount`, then show the mount point in the file manager.
    MountAndReveal,
    /// `volume_approve` (the daemon asks for presence).
    Approve { id: String },
    /// `volume_deny`.
    Deny { id: String },
    /// `volume_revoke`.
    Revoke { id: String },
    /// Show a path in the file manager (the shell's own call).
    Reveal { path: String },
    /// `volume_sync_resolve`.
    Resolve { path: String },
}

impl DriveRequest {
    /// One line naming the command.
    pub fn text(&self) -> String {
        match self {
            DriveRequest::Load => "load".into(),
            DriveRequest::MountAndReveal => "mount and reveal".into(),
            DriveRequest::Approve { id } => format!("approve {id}"),
            DriveRequest::Deny { id } => format!("deny {id}"),
            DriveRequest::Revoke { id } => format!("revoke {id}"),
            DriveRequest::Reveal { path } => format!("reveal {path}"),
            DriveRequest::Resolve { path } => format!("resolve {path}"),
        }
    }
}

/// The page's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DriveState {
    pub busy: bool,
    pub error: Option<String>,
    pub request: Option<DriveRequest>,
}

/// An input to the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DriveAction {
    /// The page's main button: show the mount point in the file manager,
    /// mounting first when the volume is not mounted (`mounted`: the mount
    /// point, when mounted).
    OpenVolume {
        mounted: Option<String>,
    },
    Approve {
        id: String,
    },
    Deny {
        id: String,
    },
    Revoke {
        id: String,
    },
    /// "Open in Finder" (the mount), or a conflict's Open.
    Reveal {
        path: String,
    },
    /// A conflict's Resolve (the files stay).
    Resolve {
        path: String,
    },
    Done,
    Failed {
        error: String,
    },
}

/// A conflict, one line: the file, who wrote the other copy and when, and
/// its buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictView {
    /// The file (what Resolve sends).
    pub path: String,
    pub text: String,
    pub trailing: String,
    /// The copy to show in the file manager (mounted only).
    pub reveal: Option<String>,
    /// "Open" when `reveal` is set.
    pub open_label: Option<String>,
    /// "Resolve".
    pub resolve_label: String,
}

/// The page as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveView {
    pub title: String,
    pub requests_title: String,
    pub requests: Vec<LineView>,
    pub grants_title: String,
    pub grants: Vec<LineView>,
    pub grants_empty: String,
    pub busy: bool,
    pub error: Option<String>,
    pub request: Option<DriveRequest>,
    /// [`DriveRequest::text`] of `request`.
    pub request_text: Option<String>,
    /// "Open in Finder" (Linux: "Open folder"): mounts first when the
    /// volume is not mounted. None where it cannot mount.
    pub open_label: Option<String>,
    /// The mount point, when mounted.
    pub mount_path: Option<String>,
    /// One quiet line: where it is mounted ("In Finder at ~/Cua Volume"),
    /// or that it is not.
    pub mount_line: Option<String>,
    /// "Devices".
    pub devices_title: String,
    /// One line per device, this one first: when it last synced, what is
    /// pending.
    pub devices: Vec<LineView>,
    /// Why nothing syncs, or the feed's error.
    pub sync_note: Option<String>,
    /// The note is an error.
    pub sync_error: bool,
    /// "Conflicts".
    pub conflicts_title: String,
    pub conflicts: Vec<ConflictView>,
}

fn mode_word(mode: &str) -> &'static str {
    if mode == "rw" {
        "read and write"
    } else {
        "read"
    }
}

/// The page at the drive's root.
pub fn drive_initial() -> DriveState {
    DriveState {
        request: Some(DriveRequest::Load),
        busy: true,
        ..Default::default()
    }
}

/// Advances the page.
pub fn drive_reduce(state: &DriveState, action: &DriveAction) -> DriveState {
    let mut s = state.clone();
    let start = |s: &mut DriveState, r: DriveRequest| {
        s.busy = true;
        s.error = None;
        s.request = Some(r);
    };
    match action {
        DriveAction::OpenVolume { mounted } if !s.busy => start(
            &mut s,
            match mounted {
                Some(path) => DriveRequest::Reveal { path: path.clone() },
                None => DriveRequest::MountAndReveal,
            },
        ),
        DriveAction::Approve { id } if !s.busy => {
            start(&mut s, DriveRequest::Approve { id: id.clone() })
        }
        DriveAction::Deny { id } if !s.busy => start(&mut s, DriveRequest::Deny { id: id.clone() }),
        DriveAction::Revoke { id } if !s.busy => {
            start(&mut s, DriveRequest::Revoke { id: id.clone() })
        }
        DriveAction::Reveal { path } if !s.busy => {
            start(&mut s, DriveRequest::Reveal { path: path.clone() })
        }
        DriveAction::Resolve { path } if !s.busy => {
            start(&mut s, DriveRequest::Resolve { path: path.clone() })
        }
        DriveAction::Done => {
            s.busy = false;
            s.request = None;
        }
        DriveAction::Failed { error } => {
            s.busy = false;
            s.request = None;
            s.error = Some(error.clone());
        }
        _ => {}
    }
    s
}

/// The page as drawn.
pub fn drive_view(input: &DriveInput, state: &DriveState) -> DriveView {
    let requests = input
        .requests
        .iter()
        .map(|r| LineView {
            id: r.id.clone(),
            text: format!(
                "{} asks to {} {}",
                r.principal,
                mode_word(&r.mode),
                r.prefix
            ),
            trailing: r
                .reason
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(60)
                .collect(),
            action_label: Some("Approve".into()),
            secondary_label: Some("Deny".into()),
            on: None,
        })
        .collect();
    let grants = input
        .grants
        .iter()
        .filter(|g| !g.revoked)
        .map(|g| LineView {
            id: g.id.clone(),
            text: format!("{} can {} {}", g.principal, mode_word(&g.mode), g.prefix),
            trailing: String::new(),
            action_label: Some("Revoke".into()),
            secondary_label: None,
            on: None,
        })
        .collect();
    let mount = input.mount.as_ref();
    let mount_path = mount.and_then(|m| m.mounted_path()).map(str::to_string);
    let (devices, sync_note, sync_error, conflicts) = match &input.sync {
        Some(sync) => sync_lines(sync, mount_path.as_deref(), input.now_ms),
        None => (vec![], None, false, vec![]),
    };
    let finder = mount.is_none_or(|m| m.method != "fuse");
    DriveView {
        open_label: mount.filter(|m| m.supported()).map(|_| {
            if finder {
                "Open in Finder"
            } else {
                "Open folder"
            }
            .to_string()
        }),
        mount_line: mount.filter(|m| m.supported()).map(|m| match &mount_path {
            Some(p) => format!(
                "{} {}",
                if finder { "In Finder at" } else { "Mounted at" },
                crate::paths::display_path(p, input.home.as_deref())
            ),
            None if m.state == "mounting" => "Mounting\u{2026}".into(),
            None => "Not mounted".into(),
        }),
        mount_path,
        devices_title: "Devices".into(),
        devices,
        sync_note,
        sync_error,
        conflicts_title: "Conflicts".into(),
        conflicts,
        title: "Volume".into(),
        requests_title: "Requests".into(),
        requests,
        grants_title: "Grants".into(),
        grants,
        grants_empty: "Agents have only their own folders.".into(),
        busy: state.busy,
        error: state.error.clone(),
        request: state.request.clone(),
        request_text: state.request.as_ref().map(DriveRequest::text),
    }
}

type SyncLines = (Vec<LineView>, Option<String>, bool, Vec<ConflictView>);

fn sync_lines(sync: &DriveSyncInput, mount: Option<&str>, now_ms: u64) -> SyncLines {
    let name_for = |id: &str| {
        sync.devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.to_string())
    };
    let mut devices: Vec<LineView> = Vec::new();
    let this_name = if sync.device_name.is_empty() {
        name_for(&sync.device_id)
    } else {
        sync.device_name.clone()
    };
    let this_trailing = if sync.feed == "off" {
        "Not syncing".to_string()
    } else {
        let synced = if sync.last_poll_ms == 0 {
            "not synced yet".to_string()
        } else {
            format!("synced {}", ago(sync.last_poll_ms, now_ms))
        };
        if sync.pending_uploads > 0 {
            format!("{} pending, {synced}", sync.pending_uploads)
        } else {
            let mut t = synced;
            t[..1].make_ascii_uppercase();
            t
        }
    };
    devices.push(LineView {
        id: sync.device_id.clone(),
        text: format!("{this_name} (this device)"),
        trailing: this_trailing,
        action_label: None,
        secondary_label: None,
        on: None,
    });
    for d in sync
        .devices
        .iter()
        .filter(|d| !d.this_device && d.id != sync.device_id)
    {
        devices.push(LineView {
            id: d.id.clone(),
            text: if d.name.is_empty() {
                d.id.clone()
            } else {
                d.name.clone()
            },
            trailing: format!("Seen {}", ago(d.last_seen_ms.max(d.last_change_ms), now_ms)),
            action_label: None,
            secondary_label: None,
            on: None,
        });
    }
    let (note, error) = match sync.feed.as_str() {
        "off" => (
            Some("Syncing across devices needs S3-compatible storage.".into()),
            false,
        ),
        "error" => (
            Some(
                sync.last_error
                    .clone()
                    .filter(|e| !e.is_empty())
                    .unwrap_or_else(|| "Sync stopped.".into()),
            ),
            true,
        ),
        _ => (None, false),
    };
    let conflicts = sync
        .conflicts
        .iter()
        .map(|c| {
            let reveal = mount.map(|m| {
                format!(
                    "{}/{}",
                    m.trim_end_matches('/'),
                    c.conflict_path.trim_start_matches('/')
                )
            });
            ConflictView {
                path: c.path.clone(),
                text: c.path.clone(),
                trailing: format!(
                    "From {}, {}",
                    name_for(&c.loser_device),
                    ago(c.ts_ms, now_ms)
                ),
                open_label: reveal.as_ref().map(|_| "Open".into()),
                reveal,
                resolve_label: "Resolve".into(),
            }
        })
        .collect();
    (devices, note, error, conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_mounts_first_then_approve() {
        let s = drive_initial();
        assert_eq!(s.request, Some(DriveRequest::Load));
        let s = drive_reduce(&s, &DriveAction::Done);
        let open = drive_reduce(&s, &DriveAction::OpenVolume { mounted: None });
        assert_eq!(open.request, Some(DriveRequest::MountAndReveal));
        let open = drive_reduce(
            &s,
            &DriveAction::OpenVolume {
                mounted: Some("/Users/maya/Cua Volume".into()),
            },
        );
        assert_eq!(
            open.request,
            Some(DriveRequest::Reveal {
                path: "/Users/maya/Cua Volume".into()
            })
        );
        let s = drive_reduce(&s, &DriveAction::Approve { id: "q1".into() });
        assert_eq!(s.request, Some(DriveRequest::Approve { id: "q1".into() }));
        let s = drive_reduce(
            &s,
            &DriveAction::Failed {
                error: "not confirmed".into(),
            },
        );
        assert!(!s.busy && s.error.is_some());
        let input = DriveInput {
            requests: vec![DriveRequestInput {
                id: "q1".into(),
                principal: "agent:ada".into(),
                prefix: "agents/bob/outputs/".into(),
                mode: "r".into(),
                reason: "cite the draft".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            drive_view(&input, &s).requests[0].text,
            "agent:ada asks to read agents/bob/outputs/"
        );
    }

    fn synced() -> DriveInput {
        let now = 1_790_683_200_000;
        DriveInput {
            now_ms: now,
            mount: Some(DriveMountInput {
                enabled: true,
                state: "mounted".into(),
                method: "fskit".into(),
                path: Some("/Volumes/Cua Volume".into()),
                volume_name: "Cua Volume".into(),
                ..Default::default()
            }),
            sync: Some(DriveSyncInput {
                device_id: "d1".into(),
                device_name: "maya-mbp".into(),
                feed: "live".into(),
                last_poll_ms: now - 3_000,
                pending_uploads: 2,
                conflicts: vec![DriveConflictInput {
                    path: "public/plan.md".into(),
                    conflict_path: "public/plan (conflict from maya-linux 2026-09-29 14.02.11).md"
                        .into(),
                    winner_device: "d1".into(),
                    loser_device: "d2".into(),
                    ts_ms: now - 120_000,
                }],
                devices: vec![
                    DriveDeviceInput {
                        id: "d1".into(),
                        name: "maya-mbp".into(),
                        this_device: true,
                        ..Default::default()
                    },
                    DriveDeviceInput {
                        id: "d2".into(),
                        name: "maya-linux".into(),
                        last_seen_ms: now - 300_000,
                        ..Default::default()
                    },
                ],
                last_error: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn devices_conflicts_and_open_in_finder() {
        let input = synced();
        let v = drive_view(&input, &DriveState::default());
        assert_eq!(v.open_label.as_deref(), Some("Open in Finder"));
        assert_eq!(v.mount_path.as_deref(), Some("/Volumes/Cua Volume"));
        let lines: Vec<_> = v
            .devices
            .iter()
            .map(|l| (l.text.as_str(), l.trailing.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                ("maya-mbp (this device)", "2 pending, synced just now"),
                ("maya-linux", "Seen 5m ago")
            ]
        );
        let c = &v.conflicts[0];
        assert_eq!(
            c.reveal.as_deref(),
            Some(
                "/Volumes/Cua Volume/public/plan (conflict from maya-linux 2026-09-29 14.02.11).md"
            )
        );
        assert_eq!(c.trailing, "From maya-linux, 2m ago");
        let s = drive_reduce(
            &DriveState::default(),
            &DriveAction::Resolve {
                path: c.path.clone(),
            },
        );
        assert_eq!(
            s.request,
            Some(DriveRequest::Resolve {
                path: "public/plan.md".into()
            })
        );
        // Not mounted: no Open anywhere; one device on this machine's store.
        let mut input = input;
        input.mount.as_mut().unwrap().state = "off".into();
        let sync = input.sync.as_mut().unwrap();
        sync.feed = "off".into();
        let v = drive_view(&input, &DriveState::default());
        assert_eq!(
            v.open_label.as_deref(),
            Some("Open in Finder"),
            "mounts first"
        );
        assert_eq!(v.mount_line.as_deref(), Some("Not mounted"));
        assert!(v.conflicts[0].open_label.is_none());
        assert_eq!(v.devices[0].trailing, "Not syncing");
        assert!(v.sync_note.is_some() && !v.sync_error);
    }
}
