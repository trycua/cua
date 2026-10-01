// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Devices": this device as a client of the cua.ai account on the relay,
//! the account's other devices, approvals waiting for this device, and the
//! account's access log ("who accessed what, when").
//!
//! Plain data in (the relay's device list and audit events, as `cua-host`
//! returns them) and plain data out (this device's enrollment, rows, a
//! banner, approval prompts, activity lines, and the enroll and approve
//! sheets). Approving a device or re-verifying one widens who can reach the
//! user's machines, so every approval asks for presence (Touch ID or the
//! login password) before the shell calls the relay. Nothing here prompts
//! on its own: unattended agents keep their device session without the user.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How close to re-verification the banner starts warning.
pub const REVERIFY_WARNING_SECS: u64 = 3 * 86_400;

/// Newest access lines "Recent access" shows.
pub const RECENT_ACCESS_LIMIT: usize = 20;

/// A device as the relay lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInput {
    /// `dev_…`.
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// `pending`, `enrolled`, `expired` or `revoked`.
    #[serde(default)]
    pub state: String,
    /// Re-verification due at (Unix seconds).
    #[serde(default)]
    pub enrolled_until: Option<u64>,
    /// Last session (Unix seconds).
    #[serde(default)]
    pub last_seen: Option<u64>,
    /// This device.
    #[serde(default)]
    pub current: bool,
    /// Operating system the device reported (`macos`, `windows`, `linux`).
    #[serde(default)]
    pub platform: Option<String>,
}

/// One audit event as the relay returns it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditInput {
    /// Unix seconds.
    pub ts: u64,
    /// Kind.
    pub kind: String,
    /// Acting device.
    #[serde(default)]
    pub device: Option<String>,
    /// Machine id.
    #[serde(default)]
    pub machine: Option<String>,
    /// Other party.
    #[serde(default)]
    pub subject: Option<String>,
    /// Detail.
    #[serde(default)]
    pub detail: Option<String>,
}

/// Everything the Devices page reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesInput {
    /// The account's devices.
    #[serde(default)]
    pub devices: Vec<DeviceInput>,
    /// The account's audit events, newest last.
    #[serde(default)]
    pub audit: Vec<AuditInput>,
    /// This device's id when it has a key (it may not be registered yet).
    #[serde(default)]
    pub local_device_id: Option<String>,
    /// The one-time code this device shows while it waits for approval.
    #[serde(default)]
    pub pending_code: Option<String>,
    /// When the relay stops letting unenrolled devices through.
    #[serde(default)]
    pub enforce_after: Option<u64>,
    /// Machine ids to display names.
    #[serde(default)]
    pub machine_names: std::collections::HashMap<String, String>,
    /// The account's relay machines, for the "New machine" confirm badge
    /// (S5). Empty on a relay that predates it, or when this device cannot
    /// list them, same as `machine_names`.
    #[serde(default)]
    pub machines: Vec<MachineInput>,
}

/// One relay machine, as [`DevicesInput::machines`] reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineInput {
    /// Machine id.
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Registered with an enrolled device's proof, or confirmed since
    /// (S5). `true` from a relay that predates the check.
    #[serde(default = "machine_confirmed_default")]
    pub confirmed: bool,
}

fn machine_confirmed_default() -> bool {
    true
}

/// Banner tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BannerTone {
    /// Informational.
    Info,
    /// Needs the user soon.
    Warning,
    /// Blocks relay access now.
    Critical,
}

/// What a device button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceAction {
    /// Enroll this device (the enroll sheet).
    Enroll,
    /// Approve another device (after presence).
    Approve,
    /// Rename a device.
    Rename,
    /// Revoke a device (after its confirmation).
    Revoke,
}

/// The banner at the top of the page and the main window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceBanner {
    /// Tone.
    pub tone: BannerTone,
    /// Text.
    pub text: String,
    /// Button, if any.
    pub action: Option<DeviceAction>,
    /// The button's label.
    pub action_label: Option<String>,
}

/// Asked before a button runs (both shells show it as a native alert).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfirm {
    /// The question.
    pub title: String,
    /// What happens.
    pub message: String,
    /// The confirming button.
    pub confirm_label: String,
    /// The other button.
    pub cancel_label: String,
}

/// A relay machine this account owns that registered without an enrolled
/// device's signature or MFA (S5): shown as "new" in its own small section
/// until an enrolled device confirms it is really the user's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnconfirmedMachine {
    /// Machine id (the native side's `confirm_machine` call takes this).
    pub id: String,
    /// Display name, or the id when it has none yet.
    pub title: String,
    /// Asked before confirming (both shells show it as a native alert).
    pub confirm: DeviceConfirm,
}

/// One device row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRow {
    /// Device id.
    pub id: String,
    /// Name (`(this device)` suffix for the current one).
    pub title: String,
    /// State in words.
    pub subtitle: String,
    /// Buttons.
    pub actions: Vec<DeviceAction>,
    /// The name as stored (Rename starts from it).
    pub name: String,
    /// Operating system in words (`macOS`), empty when unknown.
    pub platform: String,
    /// Platform and state on one line.
    pub detail: String,
    /// Last session (Unix seconds); the shell says it relative to now.
    pub last_seen: Option<u64>,
    /// This device.
    pub current: bool,
    /// Asked before Revoke.
    pub revoke_confirm: Option<DeviceConfirm>,
}

/// A device waiting for this (enrolled) device's approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPrompt {
    /// Device id to approve.
    pub device_id: String,
    /// The question.
    pub text: String,
    /// Ask for Touch ID / the login password before approving.
    pub requires_presence: bool,
    /// The device's name.
    pub name: String,
    /// Re-verification of a device whose enrollment ran out (no code).
    pub expired: bool,
    /// The system notification's title.
    pub notify_title: String,
    /// The system notification's body.
    pub notify_body: String,
}

/// One access-log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    /// Unix seconds.
    pub ts: u64,
    /// The line.
    pub text: String,
    /// Access by another account or an unenrolled device.
    pub notable: bool,
}

/// This device's enrollment, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnrollmentKind {
    /// Enrolled: the relay lets it reach the account's machines.
    Enrolled,
    /// Not enrolled, still let through until the grace period ends.
    Grace,
    /// Not enrolled, and the relay refuses it.
    NeedsEnrollment,
    /// Registered, waiting for approval from an enrolled device.
    Waiting,
    /// Enrollment ran out: one approval re-verifies it.
    Due,
    /// Revoked.
    Revoked,
}

/// Settings → Devices' first section: this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThisDevice {
    /// State.
    pub kind: EnrollmentKind,
    /// "Enrolled until", "Grace ends", "Needs enrollment", ...
    pub title: String,
    /// The date after the title (Unix seconds); the shell formats it.
    pub at: Option<u64>,
    /// The device's name, once registered.
    pub name: Option<String>,
    /// "Enroll…" when this device needs it.
    pub action_label: Option<String>,
}

/// The words the Devices page uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesLabels {
    /// Page title.
    pub title: String,
    /// This device's section.
    pub this_device: String,
    /// The account's devices.
    pub devices: String,
    /// The access log's section.
    pub recent: String,
    /// When the access log is empty.
    pub recent_empty: String,
    /// "Last seen" before a relative time.
    pub last_seen: String,
    /// Approve button (the row's, and the sheet's for a new device).
    pub approve: String,
    /// The row's Deny: revokes a still-pending device in one click, or
    /// (a device due for re-verification) only dismisses it, like the
    /// sheet's Deny and Not Now.
    pub deny: String,
    /// Rename button.
    pub rename: String,
    /// Revoke button.
    pub revoke: String,
    /// The rename prompt's title.
    pub rename_title: String,
    /// The rename prompt's confirming button.
    pub rename_confirm: String,
    /// Cancel.
    pub cancel: String,
    /// Shown instead of the page while signed out.
    pub signed_out: String,
    /// The unconfirmed-machines section's heading.
    pub new_machines: String,
    /// Confirm button on a new machine.
    pub confirm_machine: String,
}

/// The Devices page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesView {
    /// Banner, if something needs the user.
    pub banner: Option<DeviceBanner>,
    /// This device is enrolled (relay machines are reachable).
    pub enrolled: bool,
    /// Device rows, this device first.
    pub rows: Vec<DeviceRow>,
    /// Devices this device can approve.
    pub approvals: Vec<ApprovalPrompt>,
    /// Access log, newest first.
    pub activity: Vec<ActivityRow>,
    /// This device's enrollment.
    pub this_device: ThisDevice,
    /// "Recent access": which device (or account) opened which machine,
    /// newest first, at most [`RECENT_ACCESS_LIMIT`].
    pub recent: Vec<ActivityRow>,
    /// Machines registered without an enrolled device's proof, still
    /// waiting to be confirmed (S5); empty most of the time.
    pub unconfirmed_machines: Vec<UnconfirmedMachine>,
    /// Words.
    pub labels: DevicesLabels,
}

fn days(secs: u64) -> String {
    let d = secs.div_ceil(86_400);
    if d == 1 {
        "1 day".into()
    } else {
        format!("{d} days")
    }
}

fn name_or_id<'a>(devices: &'a [DeviceInput], id: &'a str) -> &'a str {
    devices
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(id)
}

/// A reported operating system in words (`macOS`); empty when unknown.
pub fn platform_name(platform: Option<&str>) -> String {
    let p = platform.map(str::trim).unwrap_or("");
    match p.to_ascii_lowercase().as_str() {
        "" => String::new(),
        "macos" | "darwin" | "mac" => "macOS".into(),
        "windows" | "win32" => "Windows".into(),
        "linux" => "Linux".into(),
        "ios" => "iOS".into(),
        "android" => "Android".into(),
        "freebsd" => "FreeBSD".into(),
        _ => {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

/// One access-log line in words.
pub fn activity_text(
    e: &AuditInput,
    devices: &[DeviceInput],
    machines: &HashMap<String, String>,
) -> String {
    let device = e.device.as_deref().map(|d| name_or_id(devices, d));
    let machine = e
        .machine
        .as_deref()
        .map(|m| machines.get(m).map(String::as_str).unwrap_or(m));
    let subject = e.subject.as_deref().unwrap_or("");
    let by = |who: Option<&str>| who.map(|d| format!(" from {d}")).unwrap_or_default();
    let target = device.unwrap_or("A device");
    match e.kind.as_str() {
        "machine_access" if subject == "unenrolled device" => format!(
            "An unenrolled device opened {}",
            machine.unwrap_or("a machine")
        ),
        "machine_access" => format!("{target} opened {}", machine.unwrap_or("a machine")),
        "shared_access" => format!("{subject} opened {}", machine.unwrap_or("a machine")),
        "unenrolled_access" => "An unenrolled device used your account".into(),
        "device_registered" => format!("{target} asked to join"),
        "device_enrolled" if subject == "bootstrap:grace" => {
            format!("{target} was enrolled during the grace period")
        }
        "device_enrolled" if subject.starts_with("bootstrap:") => {
            format!("{target} was enrolled by a fresh sign-in")
        }
        "device_rekeyed" => format!(
            "{target} replaced {} on the same machine",
            name_or_id(devices, subject)
        ),
        "device_enrolled" => format!(
            "{target} was approved{}",
            by(Some(name_or_id(devices, subject)))
        ),
        "device_session" => format!("{target} connected"),
        "device_renamed" => format!("{} was renamed{}", name_or_id(devices, subject), by(device)),
        "device_revoked" => format!("{} was revoked{}", name_or_id(devices, subject), by(device)),
        "share_added" => format!(
            "{} was shared with {subject}{}",
            machine.unwrap_or("A machine"),
            by(device)
        ),
        "share_removed" => format!(
            "{} is no longer shared with {subject}{}",
            machine.unwrap_or("A machine"),
            by(device)
        ),
        "sharing_stopped" => format!("{} stopped sharing", machine.unwrap_or("A machine")),
        "sharing_started" => format!("{} started sharing", machine.unwrap_or("A machine")),
        "machine_registered" => format!("{} was added", machine.unwrap_or("A machine")),
        "machine_reregistered" => format!("{} was set up again", machine.unwrap_or("A machine")),
        "machine_removed" => format!("{} was removed", machine.unwrap_or("A machine")),
        other => other.replace('_', " "),
    }
}

/// Whether an audit event is someone reaching a machine.
pub fn is_access(kind: &str) -> bool {
    matches!(
        kind,
        "machine_access" | "shared_access" | "unenrolled_access"
    )
}

/// The page's words.
pub fn labels() -> DevicesLabels {
    DevicesLabels {
        title: "Devices".into(),
        this_device: "This Device".into(),
        devices: "Your Devices".into(),
        recent: "Recent Access".into(),
        recent_empty: "No access yet".into(),
        last_seen: "Last seen".into(),
        approve: "Approve\u{2026}".into(),
        deny: "Deny".into(),
        rename: "Rename\u{2026}".into(),
        revoke: "Revoke\u{2026}".into(),
        rename_title: "Rename Device".into(),
        rename_confirm: "Rename".into(),
        cancel: "Cancel".into(),
        signed_out: "Sign in to Cua to see the devices that can reach your machines.".into(),
        new_machines: "New Machines".into(),
        confirm_machine: "Confirm".into(),
    }
}

/// Asked before confirming a new machine is really the user's (S5).
fn confirm_machine_confirm(name: &str) -> DeviceConfirm {
    DeviceConfirm {
        title: format!("Confirm \u{201c}{name}\u{201d}?"),
        message: format!(
            "{name} registered to your account without proof it was you: an enrolled \
             device's signature or a sign-in with a second factor. Confirm it only if you \
             recognize it."
        ),
        confirm_label: "Confirm".into(),
        cancel_label: "Cancel".into(),
    }
}

/// A device name as typed for Rename: trimmed, control characters
/// dropped, at most 64 characters; `None` when nothing is left.
pub fn clean_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_owned();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn revoke_confirm(name: &str, this: bool) -> DeviceConfirm {
    DeviceConfirm {
        title: format!("Revoke \u{201c}{name}\u{201d}?"),
        message: if this {
            "This device will no longer list or reach your machines until you enroll it again."
                .into()
        } else {
            format!(
                "{name} will no longer list or reach your machines. To use it again, enroll it again."
            )
        },
        confirm_label: "Revoke".into(),
        cancel_label: "Cancel".into(),
    }
}

const ENROLL_LABEL: &str = "Enroll\u{2026}";

fn this_device(current: Option<&DeviceInput>, input: &DevicesInput, now: u64) -> ThisDevice {
    let name = current.map(|d| d.name.clone()).filter(|n| !n.is_empty());
    let (kind, title, at) = match current.map(|d| (d.state.as_str(), d.enrolled_until)) {
        Some(("enrolled", Some(until))) => {
            (EnrollmentKind::Enrolled, "Enrolled until", Some(until))
        }
        Some(("enrolled", None)) => (EnrollmentKind::Enrolled, "Enrolled", None),
        Some(("pending", _)) => (EnrollmentKind::Waiting, "Waiting for approval", None),
        Some(("expired", _)) => (EnrollmentKind::Due, "Re-verification due", None),
        Some(("revoked", _)) => (EnrollmentKind::Revoked, "Revoked", None),
        _ => match input.enforce_after {
            Some(t) if t > now => (
                EnrollmentKind::Grace,
                "Needs enrollment \u{b7} grace ends",
                Some(t),
            ),
            _ => (EnrollmentKind::NeedsEnrollment, "Needs enrollment", None),
        },
    };
    ThisDevice {
        kind,
        title: title.into(),
        at,
        name,
        action_label: (kind != EnrollmentKind::Enrolled).then(|| ENROLL_LABEL.into()),
    }
}

/// The Devices page at `now` (Unix seconds).
pub fn devices_view(input: &DevicesInput, now: u64) -> DevicesView {
    let current = input
        .devices
        .iter()
        .find(|d| d.current || input.local_device_id.as_deref() == Some(d.id.as_str()));
    let enrolled = current.is_some_and(|d| d.state == "enrolled");
    let enroll = |tone, text: String| DeviceBanner {
        tone,
        text,
        action: Some(DeviceAction::Enroll),
        action_label: Some(ENROLL_LABEL.into()),
    };
    let banner = match current.map(|d| (d.state.as_str(), d.enrolled_until)) {
        Some(("enrolled", Some(until))) if until.saturating_sub(now) < REVERIFY_WARNING_SECS => {
            Some(DeviceBanner {
                tone: BannerTone::Warning,
                text: format!(
                    "Re-verify this device within {}: sign in again, or approve it from another enrolled device.",
                    days(until.saturating_sub(now))
                ),
                action: None,
                action_label: None,
            })
        }
        Some(("enrolled", _)) => None,
        Some(("expired", _)) => Some(enroll(
            BannerTone::Critical,
            "Re-verification is due: sign in again, or approve this device from another enrolled device.".into(),
        )),
        Some(("revoked", _)) => Some(enroll(
            BannerTone::Critical,
            "This device was revoked. Enroll it again to reach your machines.".into(),
        )),
        // Signing in again enrolls it at once: the banner offers that
        // before the code.
        Some(("pending", _)) => Some(enroll(
            BannerTone::Warning,
            match &input.pending_code {
                Some(code) => format!(
                    "Sign in again to enroll this device, or approve it from an enrolled device with the code {code}."
                ),
                None => "Sign in again to enroll this device, or approve it from an enrolled device.".into(),
            },
        )),
        _ => Some(enroll(
            match input.enforce_after {
                Some(t) if t <= now => BannerTone::Critical,
                _ => BannerTone::Warning,
            },
            match input.enforce_after {
                Some(t) if t > now => format!(
                    "Enroll this device within {} to keep reaching your machines.",
                    days(t - now)
                ),
                _ => "Enroll this device to reach your machines.".into(),
            },
        )),
    };
    let mut ordered: Vec<&DeviceInput> = input.devices.iter().collect();
    ordered.sort_by_key(|d| {
        (
            !(current.is_some_and(|c| c.id == d.id)),
            d.name.to_lowercase(),
        )
    });
    let rows = ordered
        .iter()
        .map(|d| {
            let this = current.is_some_and(|c| c.id == d.id);
            let subtitle = match d.state.as_str() {
                "enrolled" => match d.enrolled_until {
                    Some(u) if u > now => format!("Enrolled \u{b7} re-verify in {}", days(u - now)),
                    _ => "Enrolled".into(),
                },
                "pending" => "Waiting for approval".into(),
                "expired" => "Re-verification due".into(),
                "revoked" => "Revoked".into(),
                other => other.into(),
            };
            let mut actions = Vec::new();
            if enrolled && !this && matches!(d.state.as_str(), "pending" | "expired") {
                actions.push(DeviceAction::Approve);
            }
            if enrolled && d.state != "revoked" {
                actions.push(DeviceAction::Rename);
                actions.push(DeviceAction::Revoke);
            }
            let platform = platform_name(d.platform.as_deref());
            let detail = if platform.is_empty() {
                subtitle.clone()
            } else {
                format!("{platform} \u{b7} {subtitle}")
            };
            let display = if d.name.is_empty() {
                d.id.as_str()
            } else {
                d.name.as_str()
            };
            DeviceRow {
                id: d.id.clone(),
                title: if this {
                    format!("{display} (this device)")
                } else {
                    display.to_string()
                },
                subtitle,
                revoke_confirm: actions
                    .contains(&DeviceAction::Revoke)
                    .then(|| revoke_confirm(display, this)),
                actions,
                name: d.name.clone(),
                platform,
                detail,
                last_seen: d.last_seen,
                current: this,
            }
        })
        .collect();
    let approvals = if enrolled {
        input
            .devices
            .iter()
            .filter(|d| !current.is_some_and(|c| c.id == d.id))
            .filter(|d| matches!(d.state.as_str(), "pending" | "expired"))
            .map(|d| {
                let expired = d.state == "expired";
                let name = if d.name.is_empty() { d.id.clone() } else { d.name.clone() };
                ApprovalPrompt {
                    device_id: d.id.clone(),
                    text: if expired {
                        format!("Re-verify \u{201c}{name}\u{201d} for another 30 days?")
                    } else {
                        format!(
                            "Let \u{201c}{name}\u{201d} list and open your machines? Only approve a device you just signed in on."
                        )
                    },
                    requires_presence: true,
                    notify_title: if expired {
                        format!("Re-verify \u{201c}{name}\u{201d}?")
                    } else {
                        format!("Approve \u{201c}{name}\u{201d}?")
                    },
                    notify_body: if expired {
                        "It needs one approval to keep reaching your machines.".into()
                    } else {
                        "A device signed in to your Cua account and asks to reach your machines."
                            .into()
                    },
                    name,
                    expired,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let activity: Vec<ActivityRow> = input
        .audit
        .iter()
        .rev()
        .map(|e| ActivityRow {
            ts: e.ts,
            text: activity_text(e, &input.devices, &input.machine_names),
            notable: matches!(e.kind.as_str(), "shared_access" | "unenrolled_access")
                || e.subject.as_deref() == Some("unenrolled device"),
        })
        .collect();
    let recent = input
        .audit
        .iter()
        .rev()
        .zip(activity.iter())
        .filter(|(e, _)| is_access(&e.kind))
        .map(|(_, row)| row.clone())
        .take(RECENT_ACCESS_LIMIT)
        .collect();
    let unconfirmed_machines = input
        .machines
        .iter()
        .filter(|m| !m.confirmed)
        .map(|m| {
            let title = if m.name.is_empty() {
                m.id.clone()
            } else {
                m.name.clone()
            };
            UnconfirmedMachine {
                id: m.id.clone(),
                confirm: confirm_machine_confirm(&title),
                title,
            }
        })
        .collect();
    DevicesView {
        banner,
        enrolled,
        rows,
        approvals,
        activity,
        this_device: this_device(current, input, now),
        recent,
        unconfirmed_machines,
        labels: labels(),
    }
}

// ---- Enroll this device ------------------------------------------------

/// How this device enrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnrollMethod {
    /// A fresh interactive sign-in (enrolls this device at once).
    SignIn,
    /// A one-time code approved from an enrolled device.
    Approve,
}

/// Where the enroll sheet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnrollPhase {
    /// Choosing a method.
    Choose,
    /// The shell runs an interactive sign-in.
    SigningIn,
    /// The shell registers this device with the relay.
    Registering,
    /// The code shows; the shell polls until an enrolled device approves.
    Waiting,
    /// Enrolled.
    Enrolled,
    /// Something failed; Back returns to the choice.
    Failed,
}

/// The enroll sheet's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollState {
    /// Phase.
    pub phase: EnrollPhase,
    /// The chosen method.
    pub method: Option<EnrollMethod>,
    /// The one-time code, while waiting.
    pub code: Option<String>,
    /// The last failure.
    pub error: Option<String>,
}

/// An input to the enroll sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum EnrollAction {
    /// A method was chosen.
    Choose {
        /// Method.
        method: EnrollMethod,
    },
    /// The interactive sign-in finished.
    SignedIn,
    /// The relay registered this device.
    Registered {
        /// Enrolled at once (a fresh sign-in).
        enrolled: bool,
        /// The one-time code otherwise.
        code: Option<String>,
    },
    /// An enrolled device approved this one.
    Approved,
    /// A step failed.
    Failed {
        /// Why.
        error: String,
    },
    /// Back to the choice.
    Back,
}

/// One way to enroll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollOption {
    /// Method.
    pub method: EnrollMethod,
    /// Title.
    pub title: String,
    /// One line under it.
    pub detail: String,
}

/// The enroll sheet as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollView {
    /// Title.
    pub title: String,
    /// One line under the title.
    pub lede: String,
    /// The methods (while choosing).
    pub options: Vec<EnrollOption>,
    /// The one-time code (while waiting).
    pub code: Option<String>,
    /// How to use the code.
    pub code_help: Option<String>,
    /// What is happening now.
    pub status: Option<String>,
    /// A failure.
    pub error: Option<String>,
    /// Working (a spinner).
    pub busy: bool,
    /// Enrolled.
    pub done: bool,
    /// Back (after a failure).
    pub back_label: Option<String>,
    /// Cancel, or Done once enrolled.
    pub close_label: String,
}

/// The enroll sheet's first state.
pub fn enroll_initial() -> EnrollState {
    EnrollState {
        phase: EnrollPhase::Choose,
        method: None,
        code: None,
        error: None,
    }
}

/// Advances the enroll sheet. Inputs that do not fit the phase are ignored.
pub fn enroll_reduce(state: &EnrollState, action: &EnrollAction) -> EnrollState {
    let mut s = state.clone();
    match (state.phase, action) {
        (EnrollPhase::Choose, EnrollAction::Choose { method }) => {
            s.method = Some(*method);
            s.error = None;
            s.phase = match method {
                EnrollMethod::SignIn => EnrollPhase::SigningIn,
                EnrollMethod::Approve => EnrollPhase::Registering,
            };
        }
        (EnrollPhase::SigningIn, EnrollAction::SignedIn) => s.phase = EnrollPhase::Registering,
        (EnrollPhase::Registering, EnrollAction::Registered { enrolled: true, .. }) => {
            s.phase = EnrollPhase::Enrolled;
            s.code = None;
        }
        (
            EnrollPhase::Registering,
            EnrollAction::Registered {
                enrolled: false,
                code,
            },
        ) => {
            s.phase = EnrollPhase::Waiting;
            s.code = code.clone();
        }
        (EnrollPhase::Waiting, EnrollAction::Approved) => {
            s.phase = EnrollPhase::Enrolled;
            s.code = None;
        }
        (
            EnrollPhase::SigningIn | EnrollPhase::Registering | EnrollPhase::Waiting,
            EnrollAction::Failed { error },
        ) => {
            s.phase = EnrollPhase::Failed;
            s.error = Some(error.clone());
            s.code = None;
        }
        (EnrollPhase::Failed, EnrollAction::Back) => return enroll_initial(),
        _ => {}
    }
    s
}

/// The enroll sheet as drawn.
pub fn enroll_view(state: &EnrollState) -> EnrollView {
    let options = if state.phase == EnrollPhase::Choose {
        vec![
            EnrollOption {
                method: EnrollMethod::SignIn,
                title: "Sign in again".into(),
                detail: "A fresh sign-in enrolls this device right away.".into(),
            },
            EnrollOption {
                method: EnrollMethod::Approve,
                title: "Approve from another device".into(),
                detail: "Show a one-time code to approve from an enrolled device.".into(),
            },
        ]
    } else {
        Vec::new()
    };
    let (status, busy) = match state.phase {
        EnrollPhase::SigningIn => (Some("Finish signing in in your browser\u{2026}"), true),
        EnrollPhase::Registering => (Some("Registering this device\u{2026}"), true),
        EnrollPhase::Waiting => (Some("Waiting for approval\u{2026}"), true),
        EnrollPhase::Enrolled => (Some("This device is enrolled."), false),
        EnrollPhase::Choose | EnrollPhase::Failed => (None, false),
    };
    EnrollView {
        title: "Enroll This Device".into(),
        lede: "Your machines can be reached only from enrolled devices.".into(),
        options,
        code: state.code.clone(),
        code_help: state.code.as_ref().map(|code| {
            format!(
                "On an enrolled device, approve it in Cua Spaces or run `cua devices approve {code}`. The code expires in 10 minutes."
            )
        }),
        status: status.map(str::to_owned),
        error: state.error.clone(),
        busy,
        done: state.phase == EnrollPhase::Enrolled,
        back_label: (state.phase == EnrollPhase::Failed).then(|| "Back".into()),
        close_label: if state.phase == EnrollPhase::Enrolled {
            "Done".into()
        } else {
            "Cancel".into()
        },
    }
}

// ---- Approve another device -----------------------------------------------

/// The approval sheet's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveSheetState {
    /// The device to approve.
    pub device_id: String,
    /// Its name.
    pub name: String,
    /// Re-verification (no code) rather than a new device.
    pub expired: bool,
    /// The code as typed.
    pub code: String,
    /// The approval runs (presence, then the relay).
    pub busy: bool,
    /// The last failure.
    pub error: Option<String>,
    /// The last code the relay saw expired: offers approving the waiting
    /// device of this name by id instead (see [`code_expired_fallback`]).
    /// Typing a new code clears it.
    pub code_expired: bool,
}

/// An input to the approval sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ApproveSheetAction {
    /// The code field changed.
    SetCode {
        /// Text.
        code: String,
    },
    /// Approve was pressed (the shell asks for presence, then the relay).
    Submit,
    /// Presence or the relay refused.
    Failed {
        /// Why.
        error: String,
    },
}

/// What the shell sends the relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveRequest {
    /// The code (a new device).
    pub code: Option<String>,
    /// The device id (re-verification).
    pub device_id: Option<String>,
}

/// The approval sheet as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveSheetView {
    /// Title.
    pub title: String,
    /// What approving does.
    pub message: String,
    /// A code field shows.
    pub needs_code: bool,
    /// The code field's label.
    pub code_label: String,
    /// Its placeholder.
    pub code_placeholder: String,
    /// The code, normalized (`K7QX-M2RP`).
    pub code: String,
    /// Approve is enabled.
    pub can_approve: bool,
    /// Approve.
    pub approve_label: String,
    /// Deny (a new device; revokes it) or Not Now (re-verification).
    pub deny_label: String,
    /// Deny revokes the device (one click, no confirmation).
    pub deny_revokes: bool,
    /// The reason the presence prompt shows.
    pub presence_reason: String,
    /// Working.
    pub busy: bool,
    /// A failure.
    pub error: Option<String>,
    /// What to send once presence is confirmed (`None` until ready).
    pub request: Option<ApproveRequest>,
}

/// A one-time code as typed: letters and digits upper-cased, `XXXX-XXXX`
/// once all eight are there.
pub fn normalize_code(code: &str) -> String {
    let compact: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .take(8)
        .collect();
    if compact.len() > 4 {
        format!("{}-{}", &compact[..4], &compact[4..])
    } else {
        compact
    }
}

fn code_complete(code: &str) -> bool {
    normalize_code(code)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .count()
        == 8
}

/// Opens the approval sheet for a prompt.
pub fn approve_open(prompt: &ApprovalPrompt) -> ApproveSheetState {
    ApproveSheetState {
        device_id: prompt.device_id.clone(),
        name: prompt.name.clone(),
        expired: prompt.expired,
        code: String::new(),
        busy: false,
        error: None,
        code_expired: false,
    }
}

/// Whether the relay refused an approval because the code expired (ten
/// minutes after the device registered): `cua-relay`'s device API answer,
/// matched on its stable phrase rather than the whole sentence (which also
/// names the code and points at `cua devices approve`).
fn looks_like_expired_code(error: &str) -> bool {
    error.contains("codes expire after 10 minutes")
}

/// The one still-pending device named like `state`'s, when its code
/// expired and exactly one such device is waiting: approving it by id
/// instead of the code means trusting that name, so this only offers it
/// without ambiguity. `None` while the code has not expired, or expired
/// re-verification is already by id and needs no fallback.
fn code_expired_fallback<'a>(
    state: &ApproveSheetState,
    devices: &'a [DeviceInput],
) -> Option<&'a DeviceInput> {
    if state.expired || !state.code_expired {
        return None;
    }
    let mut matches = devices
        .iter()
        .filter(|d| d.state == "pending" && !d.name.is_empty() && d.name == state.name);
    let first = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(first)
    }
}

/// Advances the approval sheet. `devices` is the account's current devices
/// (for the code-expired fallback, see [`code_expired_fallback`]).
pub fn approve_reduce(
    state: &ApproveSheetState,
    action: &ApproveSheetAction,
    devices: &[DeviceInput],
) -> ApproveSheetState {
    let mut s = state.clone();
    match action {
        ApproveSheetAction::SetCode { code } if !s.busy => {
            s.code = normalize_code(code);
            s.error = None;
            s.code_expired = false;
        }
        ApproveSheetAction::Submit if !s.busy && approve_ready(&s, devices) => {
            s.busy = true;
            s.error = None;
        }
        ApproveSheetAction::Failed { error } => {
            s.busy = false;
            s.code_expired = !s.expired && looks_like_expired_code(error);
            s.error = Some(if s.code_expired {
                "The code expired.".into()
            } else {
                error.clone()
            });
        }
        _ => {}
    }
    s
}

fn approve_ready(s: &ApproveSheetState, devices: &[DeviceInput]) -> bool {
    s.expired || code_complete(&s.code) || code_expired_fallback(s, devices).is_some()
}

/// The approval sheet as drawn. `devices` is the account's current devices
/// (for the code-expired fallback, see [`code_expired_fallback`]).
pub fn approve_view(state: &ApproveSheetState, devices: &[DeviceInput]) -> ApproveSheetView {
    let name = &state.name;
    let fallback = code_expired_fallback(state, devices);
    let ready = approve_ready(state, devices);
    ApproveSheetView {
        title: if state.expired {
            format!("Re-verify \u{201c}{name}\u{201d}?")
        } else {
            format!("Approve \u{201c}{name}\u{201d}?")
        },
        message: if state.expired {
            format!("{name} needs one approval to keep reaching your machines for another 30 days.")
        } else if fallback.is_some() {
            format!(
                "The code expired. Approve \u{201c}{name}\u{201d} by ID instead? Only approve a device you just signed in on."
            )
        } else {
            format!(
                "Enter the code {name} shows to let it list and open your machines. Only approve a device you just signed in on."
            )
        },
        needs_code: !state.expired,
        code_label: "Code".into(),
        code_placeholder: "XXXX-XXXX".into(),
        code: state.code.clone(),
        can_approve: ready && !state.busy,
        approve_label: if fallback.is_some() {
            "Approve by ID".into()
        } else {
            "Approve".into()
        },
        deny_label: if state.expired {
            "Not Now".into()
        } else {
            "Deny".into()
        },
        deny_revokes: !state.expired,
        presence_reason: format!("approve \u{201c}{name}\u{201d} for your Cua account"),
        busy: state.busy,
        error: state.error.clone(),
        request: ready.then(|| {
            if state.expired {
                ApproveRequest {
                    code: None,
                    device_id: Some(state.device_id.clone()),
                }
            } else if let Some(d) = fallback {
                ApproveRequest {
                    code: None,
                    device_id: Some(d.id.clone()),
                }
            } else {
                ApproveRequest {
                    code: Some(state.code.clone()),
                    device_id: None,
                }
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str, state: &str, until: Option<u64>, current: bool) -> DeviceInput {
        DeviceInput {
            id: id.into(),
            name: name.into(),
            state: state.into(),
            enrolled_until: until,
            last_seen: None,
            current,
            platform: None,
        }
    }

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn an_unenrolled_device_is_asked_to_enroll_with_the_grace_deadline() {
        let v = devices_view(
            &DevicesInput {
                enforce_after: Some(NOW + 5 * 86_400),
                ..Default::default()
            },
            NOW,
        );
        let b = v.banner.unwrap();
        assert_eq!(b.tone, BannerTone::Warning);
        assert_eq!(
            b.text,
            "Enroll this device within 5 days to keep reaching your machines."
        );
        assert_eq!(b.action, Some(DeviceAction::Enroll));
        assert!(!v.enrolled);
        let after = devices_view(
            &DevicesInput {
                enforce_after: Some(NOW - 1),
                ..Default::default()
            },
            NOW,
        );
        assert_eq!(after.banner.unwrap().tone, BannerTone::Critical);
    }

    #[test]
    fn an_enrolled_device_approves_others_with_presence_and_can_revoke() {
        let input = DevicesInput {
            devices: vec![
                device("dev_b", "Phone", "pending", None, false),
                device(
                    "dev_a",
                    "MacBook",
                    "enrolled",
                    Some(NOW + 20 * 86_400),
                    true,
                ),
                device("dev_c", "Old laptop", "expired", Some(NOW - 10), false),
                device("dev_d", "Stolen", "revoked", None, false),
            ],
            ..Default::default()
        };
        let v = devices_view(&input, NOW);
        assert!(v.enrolled);
        assert!(v.banner.is_none());
        assert_eq!(v.rows[0].title, "MacBook (this device)");
        assert_eq!(v.rows[0].subtitle, "Enrolled \u{b7} re-verify in 20 days");
        assert_eq!(
            v.rows[0].actions,
            [DeviceAction::Rename, DeviceAction::Revoke]
        );
        let phone = v.rows.iter().find(|r| r.id == "dev_b").unwrap();
        assert_eq!(
            phone.actions,
            [
                DeviceAction::Approve,
                DeviceAction::Rename,
                DeviceAction::Revoke
            ]
        );
        let stolen = v.rows.iter().find(|r| r.id == "dev_d").unwrap();
        assert!(stolen.actions.is_empty());
        assert_eq!(v.approvals.len(), 2);
        assert!(v.approvals.iter().all(|a| a.requires_presence));
        assert!(v.approvals[0].text.contains("Phone") || v.approvals[1].text.contains("Phone"));
        // A pending device cannot approve anything.
        let mut pending = input.clone();
        for d in &mut pending.devices {
            d.current = d.id == "dev_b";
        }
        pending.pending_code = Some("K7QX-M2RP".into());
        let v = devices_view(&pending, NOW);
        assert!(v.approvals.is_empty());
        assert!(v.rows.iter().all(|r| r.actions.is_empty()));
        let banner = v.banner.unwrap();
        assert_eq!(
            banner.text,
            "Sign in again to enroll this device, or approve it from an enrolled device with the code K7QX-M2RP."
        );
        // The banner opens the enroll sheet (Sign in again).
        assert_eq!(banner.action, Some(DeviceAction::Enroll));
    }

    #[test]
    fn enrollment_by_sign_in_and_re_keys_read_as_words() {
        let devices = vec![device(
            "dev_new",
            "MacBook",
            "enrolled",
            Some(NOW + 86_400 * 20),
            true,
        )];
        let text = |kind: &str, subject: &str| {
            activity_text(
                &AuditInput {
                    ts: NOW,
                    kind: kind.into(),
                    device: Some("dev_new".into()),
                    subject: Some(subject.into()),
                    ..Default::default()
                },
                &devices,
                &HashMap::new(),
            )
        };
        assert_eq!(
            text("device_enrolled", "bootstrap:fresh-sign-in"),
            "MacBook was enrolled by a fresh sign-in"
        );
        assert_eq!(
            text("device_enrolled", "bootstrap:grace"),
            "MacBook was enrolled during the grace period"
        );
        // The replaced key is no longer listed: it reads as its id.
        assert_eq!(
            text("device_rekeyed", "dev_old"),
            "MacBook replaced dev_old on the same machine"
        );
    }

    #[test]
    fn re_verification_is_announced_before_and_when_due() {
        let soon = DevicesInput {
            devices: vec![device(
                "dev_a",
                "MacBook",
                "enrolled",
                Some(NOW + 86_400),
                true,
            )],
            ..Default::default()
        };
        let b = devices_view(&soon, NOW).banner.unwrap();
        assert_eq!(b.tone, BannerTone::Warning);
        assert!(
            b.text.starts_with("Re-verify this device within 1 day"),
            "{}",
            b.text
        );
        let due = DevicesInput {
            devices: vec![device("dev_a", "MacBook", "expired", Some(NOW - 1), true)],
            ..Default::default()
        };
        assert_eq!(
            devices_view(&due, NOW).banner.unwrap().tone,
            BannerTone::Critical
        );
    }

    #[test]
    fn the_access_log_says_who_opened_what_newest_first() {
        let mut machines = HashMap::new();
        machines.insert("m1".to_string(), "studio-mac".to_string());
        let input = DevicesInput {
            devices: vec![
                device("dev_a", "MacBook", "enrolled", Some(NOW + 86_400 * 9), true),
                device("dev_b", "Phone", "enrolled", Some(NOW + 86_400 * 9), false),
            ],
            audit: vec![
                AuditInput {
                    ts: 1,
                    kind: "device_enrolled".into(),
                    device: Some("dev_b".into()),
                    subject: Some("dev_a".into()),
                    ..Default::default()
                },
                AuditInput {
                    ts: 2,
                    kind: "machine_access".into(),
                    device: Some("dev_b".into()),
                    machine: Some("m1".into()),
                    detail: Some("owner".into()),
                    ..Default::default()
                },
                AuditInput {
                    ts: 3,
                    kind: "shared_access".into(),
                    machine: Some("m1".into()),
                    subject: Some("bob@example.com".into()),
                    ..Default::default()
                },
                AuditInput {
                    ts: 4,
                    kind: "share_removed".into(),
                    device: Some("dev_a".into()),
                    machine: Some("m1".into()),
                    subject: Some("bob@example.com".into()),
                    ..Default::default()
                },
            ],
            machine_names: machines,
            ..Default::default()
        };
        let v = devices_view(&input, NOW);
        let lines: Vec<_> = v.activity.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(
            lines,
            [
                "studio-mac is no longer shared with bob@example.com from MacBook",
                "bob@example.com opened studio-mac",
                "Phone opened studio-mac",
                "Phone was approved from MacBook",
            ]
        );
        assert!(v.activity[1].notable);
        assert!(!v.activity[2].notable);
        assert_eq!(
            v.recent.iter().map(|a| a.text.as_str()).collect::<Vec<_>>(),
            [
                "bob@example.com opened studio-mac",
                "Phone opened studio-mac"
            ]
        );
    }

    #[test]
    fn settings_shows_this_devices_enrollment_with_a_date() {
        let enrolled = DevicesInput {
            devices: vec![device(
                "dev_a",
                "MacBook",
                "enrolled",
                Some(NOW + 20 * 86_400),
                true,
            )],
            ..Default::default()
        };
        let t = devices_view(&enrolled, NOW).this_device;
        assert_eq!(t.kind, EnrollmentKind::Enrolled);
        assert_eq!(t.title, "Enrolled until");
        assert_eq!(t.at, Some(NOW + 20 * 86_400));
        assert_eq!(t.action_label, None);
        let grace = DevicesInput {
            enforce_after: Some(NOW + 5 * 86_400),
            ..Default::default()
        };
        let t = devices_view(&grace, NOW).this_device;
        assert_eq!(t.kind, EnrollmentKind::Grace);
        assert_eq!(t.at, Some(NOW + 5 * 86_400));
        assert_eq!(t.action_label.as_deref(), Some("Enroll\u{2026}"));
        let enforced = DevicesInput {
            enforce_after: Some(NOW - 5),
            ..Default::default()
        };
        let t = devices_view(&enforced, NOW).this_device;
        assert_eq!(
            (t.kind, t.title.as_str(), t.at),
            (EnrollmentKind::NeedsEnrollment, "Needs enrollment", None)
        );
    }

    #[test]
    fn rows_carry_platform_last_seen_and_a_revoke_confirmation() {
        let mut phone = device("dev_b", "Phone", "enrolled", Some(NOW + 9 * 86_400), false);
        phone.platform = Some("linux".into());
        phone.last_seen = Some(NOW - 60);
        let input = DevicesInput {
            devices: vec![
                phone,
                device("dev_a", "MacBook", "enrolled", Some(NOW + 9 * 86_400), true),
            ],
            ..Default::default()
        };
        let v = devices_view(&input, NOW);
        let row = v.rows.iter().find(|r| r.id == "dev_b").unwrap();
        assert_eq!(row.platform, "Linux");
        assert_eq!(
            row.detail,
            "Linux \u{b7} Enrolled \u{b7} re-verify in 9 days"
        );
        assert_eq!(row.last_seen, Some(NOW - 60));
        let confirm = row.revoke_confirm.as_ref().unwrap();
        assert_eq!(confirm.title, "Revoke \u{201c}Phone\u{201d}?");
        assert_eq!(confirm.confirm_label, "Revoke");
        assert!(v.rows[0].current);
        assert_eq!(v.rows[0].detail, "Enrolled \u{b7} re-verify in 9 days");
        assert_eq!(platform_name(Some("macos")), "macOS");
        assert_eq!(platform_name(None), "");
    }

    #[test]
    fn recent_access_lists_only_machine_access_newest_first() {
        let mut names = HashMap::new();
        names.insert("m1".to_string(), "studio".to_string());
        let mut audit = vec![AuditInput {
            ts: 1,
            kind: "device_enrolled".into(),
            device: Some("dev_a".into()),
            subject: Some("bootstrap:fresh-sign-in".into()),
            ..Default::default()
        }];
        for ts in 2..30 {
            audit.push(AuditInput {
                ts,
                kind: "machine_access".into(),
                device: Some("dev_a".into()),
                machine: Some("m1".into()),
                ..Default::default()
            });
        }
        let v = devices_view(
            &DevicesInput {
                devices: vec![device(
                    "dev_a",
                    "MacBook",
                    "enrolled",
                    Some(NOW + 86_400 * 9),
                    true,
                )],
                audit,
                machine_names: names,
                ..Default::default()
            },
            NOW,
        );
        assert_eq!(v.recent.len(), RECENT_ACCESS_LIMIT);
        assert_eq!(v.recent[0].ts, 29);
        assert_eq!(v.recent[0].text, "MacBook opened studio");
        assert_eq!(v.activity.len(), 29);
    }

    #[test]
    fn enrolling_by_sign_in_or_by_code() {
        let s = enroll_initial();
        assert_eq!(enroll_view(&s).options.len(), 2);
        let s = enroll_reduce(
            &s,
            &EnrollAction::Choose {
                method: EnrollMethod::SignIn,
            },
        );
        assert_eq!(s.phase, EnrollPhase::SigningIn);
        assert!(enroll_view(&s).busy);
        let s = enroll_reduce(&s, &EnrollAction::SignedIn);
        // An older relay (or an unverified email) answers the sign-in with a
        // code.
        let s = enroll_reduce(
            &s,
            &EnrollAction::Registered {
                enrolled: false,
                code: Some("K7QX-M2RP".into()),
            },
        );
        assert_eq!(s.phase, EnrollPhase::Waiting);
        let v = enroll_view(&s);
        assert_eq!(v.code.as_deref(), Some("K7QX-M2RP"));
        assert!(
            v.code_help
                .unwrap()
                .contains("cua devices approve K7QX-M2RP")
        );
        // Out-of-phase inputs change nothing.
        assert_eq!(enroll_reduce(&s, &EnrollAction::SignedIn), s);
        let s = enroll_reduce(&s, &EnrollAction::Approved);
        let v = enroll_view(&s);
        assert!(v.done && v.close_label == "Done" && v.code.is_none());
        let s = enroll_reduce(
            &enroll_initial(),
            &EnrollAction::Choose {
                method: EnrollMethod::Approve,
            },
        );
        assert_eq!(s.phase, EnrollPhase::Registering);
        let s = enroll_reduce(
            &s,
            &EnrollAction::Failed {
                error: "relay unreachable".into(),
            },
        );
        assert_eq!(enroll_view(&s).back_label.as_deref(), Some("Back"));
        assert_eq!(enroll_reduce(&s, &EnrollAction::Back), enroll_initial());
    }

    #[test]
    fn approving_needs_the_full_code_and_denying_revokes() {
        let prompt = ApprovalPrompt {
            device_id: "dev_b".into(),
            text: String::new(),
            requires_presence: true,
            name: "Phone".into(),
            expired: false,
            notify_title: String::new(),
            notify_body: String::new(),
        };
        let no_devices: Vec<DeviceInput> = Vec::new();
        let s = approve_open(&prompt);
        let v = approve_view(&s, &no_devices);
        assert!(v.needs_code && !v.can_approve && v.request.is_none() && v.deny_revokes);
        let s = approve_reduce(
            &s,
            &ApproveSheetAction::SetCode {
                code: "k7qx m2r".into(),
            },
            &no_devices,
        );
        assert_eq!(s.code, "K7QX-M2R");
        assert!(!approve_view(&s, &no_devices).can_approve);
        // Submit before the code is complete does nothing.
        assert_eq!(
            approve_reduce(&s, &ApproveSheetAction::Submit, &no_devices),
            s
        );
        let s = approve_reduce(
            &s,
            &ApproveSheetAction::SetCode {
                code: "k7qxm2rp9".into(),
            },
            &no_devices,
        );
        let v = approve_view(&s, &no_devices);
        assert_eq!(v.code, "K7QX-M2RP");
        assert_eq!(
            v.request,
            Some(ApproveRequest {
                code: Some("K7QX-M2RP".into()),
                device_id: None
            })
        );
        let s = approve_reduce(&s, &ApproveSheetAction::Submit, &no_devices);
        assert!(s.busy && !approve_view(&s, &no_devices).can_approve);
        let s = approve_reduce(
            &s,
            &ApproveSheetAction::Failed {
                error: "authentication was cancelled".into(),
            },
            &no_devices,
        );
        assert!(!s.busy && approve_view(&s, &no_devices).error.is_some());
        let expired = approve_open(&ApprovalPrompt {
            expired: true,
            ..prompt
        });
        let v = approve_view(&expired, &no_devices);
        assert!(!v.needs_code && v.can_approve && !v.deny_revokes);
        assert_eq!(v.deny_label, "Not Now");
        assert_eq!(v.request.unwrap().device_id.as_deref(), Some("dev_b"));
        assert_eq!(
            clean_name("  work\u{7}phone  ").as_deref(),
            Some("workphone")
        );
        assert_eq!(clean_name("   "), None);
    }

    #[test]
    fn an_expired_code_offers_approving_the_sole_match_by_id() {
        let prompt = ApprovalPrompt {
            device_id: "dev_b".into(),
            text: String::new(),
            requires_presence: true,
            name: "Phone".into(),
            expired: false,
            notify_title: String::new(),
            notify_body: String::new(),
        };
        let failed = |s: &ApproveSheetState, devices: &[DeviceInput]| {
            approve_reduce(
                s,
                &ApproveSheetAction::Failed {
                    error: "not found: relay: no device is waiting with the code K7QX-M2RP \
                        (codes expire after 10 minutes); approve by id instead: \
                        `cua devices approve dev_b` (ids in `cua devices ls`)"
                        .into(),
                },
                devices,
            )
        };

        // Exactly one pending device of that name: the short message offers
        // approving it by id, and the same Approve button now sends its id.
        let one_match = [device("dev_b", "Phone", "pending", None, false)];
        let s = failed(&approve_open(&prompt), &one_match);
        assert!(s.code_expired);
        assert_eq!(s.error.as_deref(), Some("The code expired."));
        let v = approve_view(&s, &one_match);
        assert!(v.can_approve && v.needs_code);
        assert_eq!(v.approve_label, "Approve by ID");
        assert!(
            v.message.contains("Phone")
                && v.message
                    .contains("Only approve a device you just signed in on.")
        );
        assert_eq!(
            v.request,
            Some(ApproveRequest {
                code: None,
                device_id: Some("dev_b".into())
            })
        );
        // Submit goes through without a complete code.
        let submitted = approve_reduce(&s, &ApproveSheetAction::Submit, &one_match);
        assert!(submitted.busy);
        // Typing a code again drops the fallback.
        let retyped = approve_reduce(
            &s,
            &ApproveSheetAction::SetCode { code: "a".into() },
            &one_match,
        );
        assert!(!retyped.code_expired);
        assert!(!approve_view(&retyped, &one_match).can_approve);

        // No pending device of that name: no fallback, just the short error.
        let no_match = [device("dev_b", "Phone", "enrolled", None, false)];
        let s = failed(&approve_open(&prompt), &no_match);
        let v = approve_view(&s, &no_match);
        assert!(!v.can_approve && v.request.is_none());
        assert_eq!(v.approve_label, "Approve");

        // Two pending devices sharing the name: ambiguous, no fallback.
        let ambiguous = [
            device("dev_b", "Phone", "pending", None, false),
            device("dev_c", "Phone", "pending", None, false),
        ];
        let s = failed(&approve_open(&prompt), &ambiguous);
        let v = approve_view(&s, &ambiguous);
        assert!(!v.can_approve && v.request.is_none());

        // Re-verification never looks for a fallback: it is already by id.
        let expired = approve_open(&ApprovalPrompt {
            expired: true,
            ..prompt
        });
        let s = failed(&expired, &one_match);
        assert!(!s.code_expired);
    }
}
