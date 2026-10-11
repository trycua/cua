// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spaces being created: the row a create shows the instant it starts, fed
//! by the SDK's create progress until the Space is registered.
//!
//! A shell starts a create with [`CreateAction::Start`], forwards every SDK
//! progress report ([`CreateAction::Progress`]), and ends it with
//! [`CreateAction::Finish`] (the Space's id) or [`CreateAction::Fail`] (why).
//! [`compose`] puts the pending rows next to the registry's Spaces, so the
//! Space list, the sidebar, the notch tiles and the notch activity see a
//! starting Space like any other. A failed create stays in the list with its
//! error until [`CreateAction::Dismiss`].
//!
//! Deletes are in flight here too. [`CreateAction::DeleteStart`] turns the
//! Space's row into a Deleting row at once (registry probes during the
//! delete cannot turn it back into "timed out" or "Running"),
//! [`CreateAction::DeleteDone`] hides it at once (even while the registry
//! still lists it) and [`CreateAction::DeleteFail`] restores it.

use super::scene_for_os;
use crate::model::{Space, SpaceKind, SpaceOs, SpaceProgress, SpaceProvider, SpaceStatus};
use serde::{Deserialize, Serialize};

/// Id prefix of a Space that is still being created.
pub const PENDING_PREFIX: &str = "pending:";

/// One create in flight (or failed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCreate {
    /// `pending:<token>`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Operating system.
    pub os: SpaceOs,
    /// Where it runs.
    pub provider: SpaceProvider,
    /// Epoch ms it started.
    pub started_at: i64,
    /// The SDK's last phase word (`preparing`, `pulling`, ...).
    pub phase: String,
    /// The phase's fraction, when the SDK knows it.
    pub fraction: Option<f64>,
    /// The image was pulled (the pull takes most of the bar).
    pub pulled: bool,
    /// Overall progress in thousandths; never goes backwards.
    pub permille: u32,
    /// Why it failed.
    pub error: Option<String>,
    /// It failed for want of Cua Cloud credit: the billing page.
    #[serde(default)]
    pub credit_url: Option<String>,
    /// The registered Space's id once it finished; the row goes when the
    /// registry lists it.
    pub space_id: Option<String>,
    /// The image asked for.
    #[serde(default)]
    pub image: Option<String>,
    /// Container or VM.
    #[serde(default)]
    pub kind: Option<SpaceKind>,
    /// The platform that will run (`arm64`, `amd64`).
    #[serde(default)]
    pub arch: Option<String>,
    /// It runs emulated: a local Space whose platform is not the host's.
    #[serde(default)]
    pub emulated: bool,
    /// Epoch ms the current phase began (the time-based progress within a
    /// phase that reports no fraction), or last moved (a new fraction):
    /// a phase that stays put longer than [`stall_limit_secs`] fails the
    /// row with [`stall_error`].
    #[serde(default)]
    pub phase_at: Option<i64>,
    /// Bytes of the download so far, when the SDK counts them.
    #[serde(default)]
    pub bytes_done: Option<u64>,
    /// Bytes the download has in all.
    #[serde(default)]
    pub bytes_total: Option<u64>,
    /// The download's smoothed rate, bytes per second.
    #[serde(default)]
    pub bytes_per_second: Option<f64>,
    /// Cancel was pressed: the SDK is stopping the create and removing
    /// what it made; the row goes when that ends.
    #[serde(default)]
    pub cancelling: bool,
    /// GPU acceleration was asked for.
    #[serde(default)]
    pub gpu: bool,
    /// A create on one of your machines: that machine's relay id (the
    /// `host:<machine>` of the create's `on`). The machine lists the Space
    /// it is creating as a record of its own, which [`compose`] folds into
    /// this row.
    #[serde(default)]
    pub host: Option<String>,
    /// That machine's name, for the row and for a failure that names it.
    #[serde(default)]
    pub host_name: Option<String>,
}

/// One delete in flight (or done, until the registry drops the Space).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDelete {
    /// The Space's id.
    pub id: String,
    /// Epoch ms the delete started.
    pub started_at: i64,
    /// The delete returned: the row stays hidden until the registry stops
    /// listing the Space.
    pub done: bool,
}

/// One power action in flight (or done until the registry shows it, or
/// failed until the next one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPower {
    /// The Space's id.
    pub id: String,
    /// Turning it on (else off).
    pub on: bool,
    /// Epoch ms it started.
    pub started_at: i64,
    /// The SDK returned: the row keeps saying so until the registry shows
    /// the Space on (or off).
    pub done: bool,
    /// Why it failed: shown inline until the next power action.
    #[serde(default)]
    pub error: Option<String>,
}

impl PendingPower {
    /// Still running (not done, not failed).
    pub fn in_flight(&self) -> bool {
        !self.done && self.error.is_none()
    }
}

/// Every pending create and delete, and every power action.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CreatesState {
    /// Oldest first.
    pub pending: Vec<PendingCreate>,
    /// Spaces being deleted, oldest first.
    #[serde(default)]
    pub deleting: Vec<PendingDelete>,
    /// Spaces being turned off or on (or whose last try failed).
    #[serde(default)]
    pub powering: Vec<PendingPower>,
}

/// An input to [`reduce`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum CreateAction {
    /// A create started: its row shows now.
    Start {
        /// `pending:<token>` (any unique token).
        id: String,
        /// The name asked for; empty for the default.
        name: String,
        /// Operating system.
        os: SpaceOs,
        /// Where.
        provider: SpaceProvider,
        /// Epoch ms.
        now: i64,
        /// The image (its catalog entry names the distribution, the kind
        /// and the platforms).
        #[serde(default)]
        image: Option<String>,
        /// Container or VM, when the shell knows (else the catalog's).
        #[serde(default)]
        kind: Option<SpaceKind>,
        /// This machine's CPU architecture (`arm64`, `x86_64`, ...).
        #[serde(default)]
        host_arch: Option<String>,
        /// GPU acceleration was asked for (the create's `gpu` option).
        #[serde(default)]
        gpu: bool,
        /// The machine it runs on, when it is one of yours (the create's
        /// `host:<machine>`): its relay id.
        #[serde(default)]
        host: Option<String>,
        /// That machine's name.
        #[serde(default)]
        host_name: Option<String>,
    },
    /// The SDK reported progress.
    Progress {
        /// Pending id.
        id: String,
        /// Phase word.
        phase: String,
        /// Fraction through the phase.
        fraction: Option<f64>,
        /// Epoch ms it arrived.
        #[serde(default)]
        now: Option<i64>,
        /// Bytes downloaded so far (an image pull).
        #[serde(default)]
        bytes_done: Option<u64>,
        /// Bytes the download has in all.
        #[serde(default)]
        bytes_total: Option<u64>,
        /// The download's rate, bytes per second.
        #[serde(default)]
        bytes_per_second: Option<f64>,
    },
    /// Cancel was pressed: the shell asks the SDK to cancel the create
    /// (`cancel_create` with the pending id, passed as the create's
    /// `create_id`); the row shows Cancelling until the create ends.
    CancelStart {
        /// Pending id.
        id: String,
    },
    /// The cancel finished (or the create ended cancelled): the row goes.
    CancelDone {
        /// Pending id.
        id: String,
    },
    /// The cancel itself failed: the row shows why.
    CancelFail {
        /// Pending id.
        id: String,
        /// Why, one line.
        error: String,
    },
    /// Time passed (a shell's timer, a few times a second while a create
    /// is pending): progress within a phase that reports no fraction
    /// advances with the time the phase usually takes.
    Tick {
        /// Epoch ms.
        now: i64,
    },
    /// The create returned the Space.
    Finish {
        /// Pending id.
        id: String,
        /// The registered Space's id.
        space_id: String,
    },
    /// The create failed.
    Fail {
        /// Pending id.
        id: String,
        /// Why, one line.
        error: String,
        /// The error enum's case name, when the shell has one.
        #[serde(default)]
        error_variant: String,
    },
    /// Remove a failed row.
    Dismiss {
        /// Pending id.
        id: String,
    },
    /// The user confirmed Delete: the row shows Deleting now. A second
    /// start for a Space already deleting does nothing.
    DeleteStart {
        /// The Space's id.
        id: String,
        /// Epoch ms.
        now: i64,
    },
    /// The delete failed: the row is back as the registry lists it.
    DeleteFail {
        /// The Space's id.
        id: String,
    },
    /// The delete returned: the row goes now, and stays gone while the
    /// registry still lists the Space.
    DeleteDone {
        /// The Space's id.
        id: String,
    },
    /// The power button was pressed: the row says Suspending, Resuming,
    /// Turning off or Turning on now and the button waits. A second press
    /// while one runs does nothing.
    PowerStart {
        /// The Space's id.
        id: String,
        /// Turn it on (else off).
        on: bool,
        /// Epoch ms.
        now: i64,
    },
    /// The SDK's stop or start returned: the row keeps saying so until the
    /// registry shows the Space on (or off).
    PowerDone {
        /// The Space's id.
        id: String,
    },
    /// It failed: the row shows why, inline, and the button works again.
    PowerFail {
        /// The Space's id.
        id: String,
        /// Why, one line.
        error: String,
    },
}

/// Whether a power action runs for the Space `id`: its button waits.
pub fn is_powering(state: &CreatesState, id: &str) -> bool {
    state.powering.iter().any(|p| p.id == id && p.in_flight())
}

/// Whether `id` is a Space still being created (or one that failed).
pub fn is_pending(id: &str) -> bool {
    id.starts_with(PENDING_PREFIX)
}

/// Whether the Space `id` is being deleted (or was, and the registry still
/// lists it): a Delete for it does nothing.
pub fn is_deleting(state: &CreatesState, id: &str) -> bool {
    state.deleting.iter().any(|d| d.id == id)
}

/// The name a pending row shows: the one asked for, else the OS's.
pub fn pending_name(name: &str, os: SpaceOs) -> String {
    let name = name.trim();
    if !name.is_empty() {
        // As the registry row will show it: as typed.
        return super::display_name(name);
    }
    match os {
        SpaceOs::Macos => "macOS Space".into(),
        SpaceOs::Windows => "Windows Space".into(),
        SpaceOs::Linux | SpaceOs::Unknown => "New Space".into(),
    }
}

/// The phases a create reports, in order ([`CreateAction::Progress`]).
pub const PHASES: [&str; 6] = [
    "preparing",
    "pulling",
    "creating",
    "booting",
    "waiting_for_services",
    "connecting",
];

/// The kinds of create, by how long their phases take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateFamily {
    /// No recognized OS; no timing estimate.
    Unknown,
    /// A Linux container on this Mac.
    Container,
    /// A Linux VM on this Mac, native (QEMU with HVF).
    Vm,
    /// A Linux VM on this Mac, emulated (an amd64 image such as Omarchy on
    /// Apple silicon: QEMU TCG).
    EmulatedVm,
    /// A macOS VM (Lume).
    Macos,
    /// A Windows VM (QEMU; emulated on Apple silicon).
    Windows,
    /// A cloud Space (a Fleet claim).
    Cloud,
}

/// What a create usually takes, in seconds per phase of [`PHASES`]: the
/// wall time the SDK spends in each, measured through the SDK on an Apple
/// silicon Mac (2026-09, cold pulls for `pulling`):
///
/// - container, `linux:24.04-slim`: 1 s preparing, 13 s pulling, then
///   about 4 s to cua-spacesd;
/// - native VM, `linux:24.04-slim-disk`: 19 s pulling, 15 to 22 s booting
///   and waiting for cua-spacesd;
/// - emulated VM, `omarchy:edge` (amd64 under QEMU TCG): 63 s pulling,
///   112 to 119 s waiting for cua-spacesd.
///
/// macOS, Windows and cloud are estimates (not measured). A phase a family
/// does not report is 0.
pub fn expected_seconds(family: CreateFamily) -> [f64; 6] {
    match family {
        CreateFamily::Unknown => [0.0; 6],
        CreateFamily::Container => [1.0, 15.0, 0.3, 0.2, 3.5, 0.1],
        CreateFamily::Vm => [1.3, 19.0, 0.0, 0.2, 17.0, 0.2],
        CreateFamily::EmulatedVm => [1.4, 63.0, 0.0, 0.2, 115.0, 0.5],
        CreateFamily::Macos => [2.0, 600.0, 10.0, 30.0, 60.0, 3.0],
        CreateFamily::Windows => [2.0, 300.0, 0.0, 1.0, 300.0, 1.0],
        CreateFamily::Cloud => [2.0, 0.0, 3.0, 20.0, 0.0, 5.0],
    }
}

/// The create's family.
pub fn family(p: &PendingCreate) -> CreateFamily {
    match (p.provider, p.os) {
        (_, SpaceOs::Unknown) => CreateFamily::Unknown,
        (SpaceProvider::Cloud, _) => CreateFamily::Cloud,
        (_, SpaceOs::Macos) => CreateFamily::Macos,
        (_, SpaceOs::Windows) => CreateFamily::Windows,
        (_, SpaceOs::Linux) => match (p.kind, p.emulated) {
            (Some(SpaceKind::Vm), true) => CreateFamily::EmulatedVm,
            (Some(SpaceKind::Vm), false) => CreateFamily::Vm,
            _ => CreateFamily::Container,
        },
    }
}

/// First and last thousandth of the bar the phases share (the rest is
/// "Ready").
const BAR: (f64, f64) = (10.0, 990.0);

/// Where each phase of [`PHASES`] starts and ends on the bar, in
/// thousandths: shares of the family's expected time. Preparing keeps its
/// cold share either way, so learning that nothing is pulled never moves
/// the bar back; without a pull the other phases share the rest.
pub fn bands(family: CreateFamily, pulled: bool) -> [(u32, u32); 6] {
    let e = expected_seconds(family);
    let cold: f64 = e.iter().sum();
    let prep_end = BAR.0 + (BAR.1 - BAR.0) * e[0] / cold.max(1.0);
    let counts = |i: usize| i > 0 && (pulled || i != 1);
    let rest: f64 = (0..6).filter(|&i| counts(i)).map(|i| e[i]).sum();
    let mut out = [(0u32, 0u32); 6];
    out[0] = (BAR.0 as u32, prep_end.round() as u32);
    let mut at = prep_end;
    for (i, band) in out.iter_mut().enumerate().skip(1) {
        let width = if counts(i) && rest > 0.0 {
            (BAR.1 - prep_end) * e[i] / rest
        } else {
            0.0
        };
        *band = (at.round() as u32, (at + width).round() as u32);
        at += width;
    }
    out
}

/// How far into a phase that reports no fraction, from the share of its
/// expected time that passed: on pace up to 80 %, then easing toward (never
/// reaching) the phase's end, so a slow phase still moves.
pub fn eased(elapsed_share: f64) -> f64 {
    if !elapsed_share.is_finite() || elapsed_share <= 0.0 {
        0.0
    } else if elapsed_share <= 0.8 {
        elapsed_share
    } else {
        0.8 + 0.17 * (1.0 - (-(elapsed_share - 0.8) / 0.3).exp())
    }
}

/// Overall progress of `p` at epoch ms `now`: its phase's band, at the
/// phase's fraction when the SDK reports one (a pull's bytes), else at the
/// share of the phase's expected time that passed. Never below what it
/// already showed.
pub fn permille_at(p: &PendingCreate, now: Option<i64>) -> u32 {
    if p.phase == "ready" {
        return 1000;
    }
    let family = family(p);
    let Some(i) = PHASES.iter().position(|ph| *ph == p.phase) else {
        return p.permille;
    };
    let (lo, hi) = bands(family, p.pulled)[i];
    let width = (hi - lo) as f64;
    let within = match p.fraction.filter(|f| f.is_finite()) {
        Some(f) => (width * f.clamp(0.0, 1.0)).round(),
        // Rounded down: time alone never reaches the phase's end.
        None => match (p.phase_at, now) {
            (Some(at), Some(now)) => {
                let expected = expected_seconds(family)[i];
                if expected > 0.0 {
                    (width * eased((now - at) as f64 / 1000.0 / expected)).floor()
                } else {
                    0.0
                }
            }
            _ => 0.0,
        },
    };
    p.permille.max(lo + within as u32).min(999)
}

/// A download's bytes, rate and time left, as the apps show them under the
/// bar: "4.2 of 22.1 GB · 85 MB/s · about 4 min" (binary units labeled GB,
/// as the wizard sizes the download). `None` without a byte count.
pub fn transfer_text(
    done: Option<u64>,
    total: Option<u64>,
    per_second: Option<f64>,
) -> Option<String> {
    let done = done?;
    let total = total.unwrap_or(0);
    // Binary units labeled GB/MB/KB, as the wizard sizes downloads.
    let unit = |n: u64| -> (f64, &'static str) {
        match n {
            n if n >= 1 << 30 => ((1u64 << 30) as f64, "GB"),
            n if n >= 1 << 20 => ((1u64 << 20) as f64, "MB"),
            n if n >= 1 << 10 => (1024.0, "KB"),
            _ => (1.0, "bytes"),
        }
    };
    let scaled = |n: u64, u: (f64, &str), below: f64| {
        let v = n as f64 / u.0;
        if u.0 > 1.0 && v < below {
            format!("{v:.1}")
        } else {
            format!("{v:.0}")
        }
    };
    let mut out = if total > 0 {
        let u = unit(total);
        format!(
            "{} of {} {}",
            scaled(done.min(total), u, 100.0),
            scaled(total, u, 100.0),
            u.1
        )
    } else {
        let u = unit(done);
        format!("{} {}", scaled(done, u, 100.0), u.1)
    };
    let rate = per_second.filter(|r| r.is_finite() && *r > 0.0);
    if let Some(r) = rate {
        let u = unit(r as u64);
        out.push_str(&format!(" \u{b7} {} {}/s", scaled(r as u64, u, 10.0), u.1));
    }
    if let (Some(r), true) = (rate, total > 0) {
        out.push_str(&format!(
            " \u{b7} {}",
            time_left(total.saturating_sub(done) as f64 / r)
        ));
    }
    Some(out)
}

/// "about 4 min", "less than a minute", "about 1 h 20 min".
pub fn time_left(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return String::new();
    }
    if secs < 60.0 {
        return "less than a minute".into();
    }
    let mins = (secs / 60.0).round() as u64;
    if mins < 60 {
        format!("about {mins} min")
    } else if mins.is_multiple_of(60) {
        format!("about {} h", mins / 60)
    } else {
        format!("about {} h {} min", mins / 60, mins % 60)
    }
}

/// The words for a phase: "Starting…", "Downloading image…", ...
pub fn phase_label(phase: &str) -> &'static str {
    match phase {
        "pulling" => "Downloading image\u{2026}",
        "creating" => "Creating\u{2026}",
        "booting" => "Booting\u{2026}",
        "waiting_for_services" => "Starting services\u{2026}",
        "connecting" => "Connecting\u{2026}",
        "ready" => "Ready",
        _ => "Starting\u{2026}",
    }
}

/// The end of every stall message ([`stall_error`]): progress that arrives
/// after it clears the row again. A failure the SDK reported never ends
/// with it ([`failure_text`]).
pub const STALL_HINT: &str = "Dismiss it and try again.";

/// How long phase `i` of [`PHASES`] may go without moving (a new phase, or
/// a new fraction) before the row fails: ten times what it usually takes,
/// at least 3 minutes (15 for a download, whose bytes can pause while the
/// engine unpacks a layer) and at most 100, plus a minute so the SDK's own,
/// more specific timeouts (120 s for cua-spacesd, 300 s for a VM's
/// address) report first.
pub fn stall_limit_secs(family: CreateFamily, i: usize) -> f64 {
    let floor = if PHASES.get(i) == Some(&"pulling") {
        900.0
    } else {
        180.0
    };
    (expected_seconds(family).get(i).copied().unwrap_or(0.0) * 10.0).clamp(floor, 6000.0) + 60.0
}

/// [`stall_limit_secs`] for `p`. A create on another of your machines (relay
/// or direct) reports nothing while that machine downloads the image and
/// boots it (the host's own create is one call), so "preparing" there may
/// take as long as the family's whole download, not the few seconds it
/// takes here; otherwise its row failed at "Starting... 1%" while the host
/// was still pulling.
fn stall_limit(p: &PendingCreate, i: usize) -> f64 {
    let elsewhere = matches!(p.provider, SpaceProvider::Relay | SpaceProvider::Direct);
    if elsewhere && PHASES.get(i) == Some(&"preparing") {
        let pull = PHASES.iter().position(|ph| *ph == "pulling").unwrap_or(1);
        return stall_limit_secs(family(p), pull) + stall_limit_secs(family(p), i);
    }
    stall_limit_secs(family(p), i)
}

/// The error of a create stuck in `phase` for `secs`: what stalled, for how
/// long, and what to do.
pub fn stall_error(phase: &str, secs: f64) -> String {
    stall_text(phase, secs, None, None)
}

/// [`stall_error`], naming how far a stopped download got when its bytes
/// are known.
fn stall_text(phase: &str, secs: f64, done: Option<u64>, total: Option<u64>) -> String {
    let mins = (secs / 60.0).round().max(1.0) as u64;
    let what = match phase {
        "pulling" => match transfer_text(done, total, None) {
            Some(at) => format!(
                "The image download stopped at {at}: nothing arrived for {mins} min. \
                 Check your internet connection."
            ),
            None => format!(
                "The image download made no progress for {mins} min. Check your internet \
                 connection."
            ),
        },
        "waiting_for_services" => format!(
            "The Space booted, but its service did not answer for {mins} min. The image is \
             downloaded now, so another try is usually quick."
        ),
        _ => format!(
            "{} made no progress for {mins} min. If it keeps happening, run `cua doctor` in \
             Terminal.",
            phase_label(phase).trim_end_matches('\u{2026}')
        ),
    };
    format!("{what} {STALL_HINT}")
}

fn is_stalled(p: &PendingCreate) -> bool {
    p.error.as_deref().is_some_and(|e| e.ends_with(STALL_HINT))
}

/// A `Duration`'s debug form (`120s`, `1.5s`, `250ms`, `0ns`) in seconds.
fn debug_duration_secs(t: &str) -> Option<f64> {
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (n, unit) = t.split_at(split);
    let n: f64 = n.parse().ok()?;
    let scale = match unit {
        "s" => 1.0,
        "ms" => 1e-3,
        "\u{b5}s" | "us" => 1e-6,
        "ns" => 1e-9,
        _ => return None,
    };
    Some(n * scale)
}

/// "2 min", "45 s".
fn short_duration(secs: f64) -> String {
    if secs >= 90.0 {
        format!("{} min", (secs / 60.0).round() as u64)
    } else {
        format!("{} s", secs.round() as u64)
    }
}

/// The SDK's disk refusal ("not enough disk space to pull ...: it needs
/// about 30.0 GB and cua keeps 5.0 GB free, but only 12.0 GB is available
/// on /Users/me/.lume (...)") in plain words, with its numbers when present.
fn disk_text(raw: &str) -> String {
    let fix = "Free up space on this Mac (or run `cua cache prune` in Terminal), then try again.";
    let numbers = (|| {
        let rest = &raw[raw.find("it needs about ")? + "it needs about ".len()..];
        let (need, rest) = rest.split_once(" and cua keeps ")?;
        let (keep, rest) = rest.split_once(" free, but only ")?;
        let (avail, _) = rest.split_once(" is available")?;
        Some((need.trim(), keep.trim(), avail.trim()))
    })();
    match numbers {
        Some((need, keep, avail)) => format!(
            "Not enough disk space: this Space needs about {need}, plus {keep} kept free, and \
             only {avail} is available. {fix}"
        ),
        None => format!("Not enough disk space for this Space. {fix}"),
    }
}

/// Who a failure is about: the machine that runs the Space (by name, or
/// "The Mac running this Space" when the name is not known) when it is
/// another of yours, else "This Mac". Never this Mac for a create that ran
/// elsewhere.
fn machine_subject(provider: Option<SpaceProvider>, host_name: Option<&str>) -> String {
    match host_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => name.to_string(),
        None if matches!(provider, Some(SpaceProvider::Relay | SpaceProvider::Direct)) => {
            "The Mac running this Space".into()
        }
        None => "This Mac".into(),
    }
}

/// What a failed create shows for the SDK's `raw` error, naming the cause
/// when it is a known one: no disk space, the Space's service not
/// answering in time (the image is cached now, so a retry is quick), a
/// relay host that refused the sign-in, GPU acceleration that could not
/// be turned on. `phase` is where it failed, `provider` where it ran and
/// `gpu` whether GPU acceleration was asked for, when known. Anything else
/// is the SDK's words ([`crate::errors::plain_error`]).
pub fn failure_text(
    raw: &str,
    phase: Option<&str>,
    provider: Option<SpaceProvider>,
    gpu: bool,
) -> String {
    failure_text_on(raw, phase, provider, None, gpu)
}

/// [`failure_text`] for a create on one of your machines named `host_name`:
/// the causes that are about the Mac that runs the Space (Local Network
/// access, Apple's two macOS VMs) name that machine, not this Mac.
pub fn failure_text_on(
    raw: &str,
    phase: Option<&str>,
    provider: Option<SpaceProvider>,
    host_name: Option<&str>,
    gpu: bool,
) -> String {
    let plain = crate::errors::plain_error(raw);
    let l = plain.to_ascii_lowercase();
    if l.contains("not enough disk space")
        || l.contains("insufficient disk")
        || l.contains("insufficient_disk")
        || l.contains("no space left on device")
    {
        return disk_text(&plain);
    }
    // macOS Local Network privacy: the process that runs the VM (this
    // app's daemon, or a host's cua-spacesd service) may not reach the
    // guest on vmnet. The SDK says "Local Network access is not available:
    // ... cannot reach the VM at 192.168.64.x (No route to host)"; older
    // ones only the bare `EHOSTUNREACH` to a vmnet address.
    let vmnet = l.contains("192.168.64.");
    if l.contains("local network access")
        || (vmnet && (l.contains("no route to host") || l.contains("os error 65")))
    {
        let who = machine_subject(provider, host_name);
        return format!(
            "{who} can't reach its new VM because Cua doesn't have Local Network access there. \
             On that Mac, open System Settings > Privacy & Security > Local Network, turn on Cua \
             Spaces (or cua-spacesd), then try again."
        );
    }
    // Virtualization.framework's `virtualMachineLimitExceeded` ("The number
    // of virtual machines exceeds the limit"): Apple's two macOS VMs per Mac.
    if l.contains("virtualmachinelimitexceeded")
        || ((l.contains("virtual machine") || l.contains("vms"))
            && l.contains("limit")
            && (l.contains("exceed") || l.contains("maximum") || l.contains("reached")))
    {
        return format!(
            "{} is already running two macOS VMs, the most Apple's macOS license allows at once. \
             Stop one, then try again; the image is downloaded now, so it is quicker.",
            machine_subject(provider, host_name)
        );
    }
    if gpu && (l.contains("gpu") || l.contains("paravirtual")) {
        return format!(
            "GPU acceleration could not be turned on here ({plain}). Create the Space again \
             with GPU acceleration off."
        );
    }
    if l.contains("unauthenticated") {
        return if provider == Some(SpaceProvider::Relay) {
            "Your other Mac did not accept the sign-in: its Cua sign-in may have expired or \
             belong to another account. Open Cua Spaces on that Mac, sign in again, then try \
             again."
                .into()
        } else {
            format!(
                "The new Space's service did not accept the app's token ({plain}). Try again; if \
                 it keeps happening, run `cua doctor` in Terminal."
            )
        };
    }
    let spacesd = "cua-spacesd did not answer within ";
    let timed_out =
        l.contains("timed out") || l.contains("timeout") || l.contains("not ready after");
    if let Some(at) = l.find(spacesd) {
        let token = l[at + spacesd.len()..]
            .split_whitespace()
            .next()
            .unwrap_or("");
        let within = debug_duration_secs(token)
            .filter(|s| *s >= 5.0)
            .map(|s| format!(" within {}", short_duration(s)));
        return match within {
            Some(within) => format!(
                "The Space booted, but its service did not answer{within}. The image is \
                 downloaded now, so trying again usually works."
            ),
            None => "The Space booted, but the time allowed for creating it ran out before its \
                     service answered. The image is downloaded now, so trying again usually \
                     works."
                .into(),
        };
    }
    match phase {
        Some("waiting_for_services") if timed_out => "The Space booted, but its service did not \
             answer in time. The image is downloaded now, so trying again usually works."
            .into(),
        Some("pulling") if timed_out => "The image download did not finish in the time \
             allowed. Check your internet connection, then try again."
            .into(),
        _ => plain,
    }
}

/// The platform a create will run ([`crate::model::run_arch`] over the
/// catalog's platforms for the image).
fn running_arch(
    image: Option<&str>,
    provider: SpaceProvider,
    host: Option<&str>,
) -> Option<String> {
    let platforms = image.map(crate::wizard::image_arch).unwrap_or_default();
    crate::model::run_arch(&platforms, provider != SpaceProvider::Cloud, host)
}

/// Advances the pending creates.
pub fn reduce(state: &CreatesState, action: &CreateAction) -> CreatesState {
    let mut next = state.clone();
    match action {
        CreateAction::Start {
            id,
            name,
            os,
            provider,
            now,
            image,
            kind,
            host_arch,
            gpu,
            host: machine,
            host_name: machine_name,
        } => {
            if *os == SpaceOs::Unknown || next.pending.iter().any(|p| &p.id == id) {
                return next;
            }
            let image = image.clone().filter(|i| !i.trim().is_empty());
            let kind = kind
                .or_else(|| image.as_deref().and_then(crate::wizard::image_kind))
                .or((*os != SpaceOs::Linux).then_some(SpaceKind::Vm));
            let host = host_arch.as_deref();
            let arch = running_arch(image.as_deref(), *provider, host);
            let mut p = PendingCreate {
                id: id.clone(),
                name: pending_name(name, *os),
                os: *os,
                provider: *provider,
                started_at: *now,
                phase: "preparing".into(),
                fraction: None,
                pulled: false,
                permille: 0,
                error: None,
                credit_url: None,
                space_id: None,
                emulated: crate::model::emulation_warning(
                    *provider != SpaceProvider::Cloud,
                    host,
                    arch.as_deref(),
                )
                .is_some(),
                image,
                kind,
                arch,
                phase_at: Some(*now),
                bytes_done: None,
                bytes_total: None,
                bytes_per_second: None,
                cancelling: false,
                gpu: *gpu,
                host: machine.clone().filter(|h| !h.trim().is_empty()),
                host_name: machine_name.clone().filter(|h| !h.trim().is_empty()),
            };
            p.permille = permille_at(&p, Some(*now));
            next.pending.push(p);
        }
        CreateAction::Progress {
            id,
            phase,
            fraction,
            now,
            bytes_done,
            bytes_total,
            bytes_per_second,
        } => {
            if let Some(p) = next.pending.iter_mut().find(|p| {
                &p.id == id
                    && (p.error.is_none() || is_stalled(p))
                    && p.space_id.is_none()
                    && !p.cancelling
            }) {
                p.pulled |= phase == "pulling";
                // Real byte movement counts as progress: a slow download
                // that still moves never stalls.
                let bytes_moved =
                    bytes_done.is_some_and(|b| p.bytes_done.is_none_or(|old| b > old));
                let moved = &p.phase != phase
                    || fraction.is_some_and(|f| p.fraction.is_none_or(|old| f > old))
                    || bytes_moved;
                if bytes_moved {
                    p.phase_at = now.or(p.phase_at);
                }
                if moved {
                    // Moving again: a stall it reported is over.
                    if is_stalled(p) {
                        p.error = None;
                    }
                    if &p.phase != phase || fraction.is_some() {
                        p.phase_at = now.or(p.phase_at);
                    }
                }
                if &p.phase != phase {
                    p.phase_at = *now;
                }
                if &p.phase != phase {
                    p.bytes_done = None;
                    p.bytes_total = None;
                    p.bytes_per_second = None;
                }
                p.phase = phase.clone();
                p.fraction = *fraction;
                // Setting up a runtime: its download done, its boot reports
                // a fraction only (no bytes left to show).
                if phase == "preparing" && bytes_done.is_none() && fraction.is_some() {
                    p.bytes_done = None;
                    p.bytes_total = None;
                    p.bytes_per_second = None;
                }
                if bytes_done.is_some() {
                    p.bytes_done = *bytes_done;
                    p.bytes_total = bytes_total.or(p.bytes_total);
                    p.bytes_per_second = bytes_per_second.or(p.bytes_per_second);
                }
                p.permille = permille_at(p, *now);
            }
        }
        CreateAction::CancelStart { id } => {
            if let Some(p) = next
                .pending
                .iter_mut()
                .find(|p| &p.id == id && p.error.is_none() && p.space_id.is_none())
            {
                p.cancelling = true;
            }
        }
        CreateAction::CancelDone { id } => next.pending.retain(|p| &p.id != id),
        CreateAction::CancelFail { id, error } => {
            if let Some(p) = next.pending.iter_mut().find(|p| &p.id == id) {
                p.cancelling = false;
                p.error = Some(if error.trim().is_empty() {
                    "Could not cancel the create".into()
                } else {
                    crate::errors::plain_error(error)
                });
            }
        }
        CreateAction::Tick { now } => {
            for p in next
                .pending
                .iter_mut()
                .filter(|p| p.error.is_none() && p.space_id.is_none() && !p.cancelling)
            {
                p.permille = permille_at(p, Some(*now));
                // Never an endless wait: a phase that stops moving fails
                // the row with what stalled and what to do.
                if let (Some(at), Some(i)) =
                    (p.phase_at, PHASES.iter().position(|ph| *ph == p.phase))
                {
                    let limit = stall_limit(p, i);
                    let stuck = (*now - at) as f64 / 1000.0;
                    if stuck > limit {
                        p.error = Some(stall_text(&p.phase, stuck, p.bytes_done, p.bytes_total));
                    }
                }
            }
        }
        CreateAction::Finish { id, space_id } => {
            // A new Space under a deleted one's id is not being deleted.
            next.deleting.retain(|d| &d.id != space_id);
            if let Some(p) = next.pending.iter_mut().find(|p| &p.id == id) {
                p.phase = "ready".into();
                p.fraction = None;
                p.permille = 1000;
                p.error = None;
                p.space_id = Some(space_id.clone());
            }
        }
        CreateAction::Fail { id, error, .. } => {
            // A create that ends while being cancelled was cancelled: its
            // row goes, it did not fail.
            if next.pending.iter().any(|p| &p.id == id && p.cancelling) {
                next.pending.retain(|p| &p.id != id);
                return next;
            }
            if let Some(p) = next.pending.iter_mut().find(|p| &p.id == id) {
                // Out of credit: one plain line, and the billing page (the
                // page only while the apps show billing).
                let credit = crate::billing::credit_notice(error);
                p.credit_url = credit
                    .as_ref()
                    .filter(|_| crate::billing::BILLING_SHOWN)
                    .map(|n| n.url.clone());
                p.error = Some(match credit {
                    Some(n) => n.text,
                    None if error.trim().is_empty() => "Could not create the Space".into(),
                    None => failure_text_on(
                        error,
                        Some(&p.phase),
                        Some(p.provider),
                        p.host_name.as_deref(),
                        p.gpu,
                    ),
                });
            }
        }
        CreateAction::Dismiss { id } => next.pending.retain(|p| &p.id != id),
        CreateAction::DeleteStart { id, now } => {
            // A failed create is dismissed, not deleted.
            if !is_pending(id) && !is_deleting(state, id) {
                next.deleting.push(PendingDelete {
                    id: id.clone(),
                    started_at: *now,
                    done: false,
                });
            }
        }
        CreateAction::DeleteFail { id } => next.deleting.retain(|d| &d.id != id),
        CreateAction::DeleteDone { id } => {
            // Nothing is turned on or off once it is gone.
            next.powering.retain(|p| &p.id != id);
            if let Some(d) = next.deleting.iter_mut().find(|d| &d.id == id) {
                d.done = true;
            }
        }
        CreateAction::PowerStart { id, on, now } => {
            if !is_pending(id) && !is_deleting(state, id) && !is_powering(state, id) {
                next.powering.retain(|p| &p.id != id);
                next.powering.push(PendingPower {
                    id: id.clone(),
                    on: *on,
                    started_at: *now,
                    done: false,
                    error: None,
                });
            }
        }
        CreateAction::PowerDone { id } => {
            if let Some(p) = next.powering.iter_mut().find(|p| &p.id == id) {
                p.done = true;
            }
        }
        CreateAction::PowerFail { id, error } => {
            if let Some(p) = next.powering.iter_mut().find(|p| &p.id == id) {
                p.done = false;
                p.error = Some(power_failed_text(p.on, error));
            }
        }
    }
    next
}

/// The inline line a failed power action shows: "Could not turn it on:
/// <why>".
pub fn power_failed_text(on: bool, error: &str) -> String {
    let what = if on { "turn it on" } else { "turn it off" };
    if error.trim().is_empty() {
        format!("Could not {what}")
    } else {
        format!("Could not {what}: {}", crate::errors::plain_error(error))
    }
}

/// Drops finished creates whose Space the registry now lists, and finished
/// deletes whose Space it no longer lists. Shells settle after each
/// registry refresh.
pub fn settle(state: &CreatesState, registry: &[Space]) -> CreatesState {
    let listed = |id: &str| registry.iter().any(|s| s.id == id);
    CreatesState {
        pending: state
            .pending
            .iter()
            .filter(|p| p.space_id.as_deref().is_none_or(|sid| !listed(sid)))
            .cloned()
            .collect(),
        deleting: state
            .deleting
            .iter()
            .filter(|d| !d.done || listed(&d.id))
            .cloned()
            .collect(),
        // A finished power action goes once the registry shows the Space
        // on (or off); any goes with its Space.
        powering: state
            .powering
            .iter()
            .filter(|p| match registry.iter().find(|s| s.id == p.id) {
                None => false,
                Some(s) if p.done => s.power.as_ref().is_some_and(|w| w.off == p.on),
                Some(_) => true,
            })
            .cloned()
            .collect(),
    }
}

/// A registry row with its power action: turning on or off, or why the
/// last one failed.
pub fn powering_space(space: &Space, p: &PendingPower) -> Space {
    let mut s = space.clone();
    if let Some(power) = s.power.as_mut() {
        if p.error.is_some() {
            power.error = p.error.clone();
        } else {
            power.turning_on = Some(p.on);
        }
    }
    s
}

/// A registry row while its Space is being deleted: dimmed, not live, not
/// reachable (nothing streams from it), whatever the registry's probe said.
pub fn deleting_space(space: &Space) -> Space {
    let mut s = space.clone();
    s.status = SpaceStatus::Deleting;
    s.detail = SpaceStatus::Deleting.label().into();
    s.progress = None;
    if let Some(sdk) = s.sdk.as_mut() {
        sdk.reachable = false;
        sdk.error = None;
    }
    s
}

/// The create is still preparing but moves (bytes, or a fraction): the
/// SDK is setting up the local runtime it needs (downloaded once, on first
/// use, then booted: the built-in Linux runtime's VM).
fn setting_up_runtime(p: &PendingCreate) -> bool {
    p.phase == "preparing" && (p.bytes_total.is_some_and(|t| t > 0) || p.fraction.is_some())
}

/// A pending create as a Space row.
pub fn pending_space(p: &PendingCreate) -> Space {
    let failed = p.error.is_some();
    let where_ = match p.provider {
        SpaceProvider::Cloud => "Cua Cloud",
        SpaceProvider::Local => "This Mac",
        // Created on another of your machines: not this Mac.
        SpaceProvider::Relay | SpaceProvider::Direct => "Another machine",
    };
    let label = if failed {
        "Failed".to_string()
    } else if p.cancelling {
        "Cancelling\u{2026}".to_string()
    } else if setting_up_runtime(p) {
        // Bytes before the image: the runtime cua sets up on first use
        // (the built-in Lume for a macOS Space).
        match p.os {
            SpaceOs::Macos => "Setting up Lume\u{2026}".to_string(),
            SpaceOs::Linux => "Setting up Linux runtime\u{2026}".to_string(),
            _ => "Setting up the runtime\u{2026}".to_string(),
        }
    } else {
        phase_label(&p.phase).to_string()
    };
    // The catalog's distribution: the right icon and System from the start.
    let distro = p.image.as_deref().and_then(crate::wizard::image_distro);
    Space {
        id: p.id.clone(),
        name: p.name.clone(),
        os: p.os,
        status: if failed {
            SpaceStatus::Suspended
        } else {
            SpaceStatus::Provisioning
        },
        detail: match &p.error {
            Some(e) => e.clone(),
            None => format!("{where_} \u{b7} {label}"),
        },
        last_used_at: p.started_at,
        started_at: Some(p.started_at),
        scene: scene_for_os(p.os),
        fleet_id: None,
        size: None,
        region: None,
        provider: Some(p.provider),
        sdk: None,
        os_name: distro.as_ref().map(|d| d.name.clone()),
        progress: Some(SpaceProgress {
            phase: p.phase.clone(),
            permille: p.permille,
            label,
            error: p.error.clone(),
            credit_url: p.credit_url.clone(),
            transfer: (!failed && !p.cancelling && (p.phase == "pulling" || setting_up_runtime(p)))
                .then(|| transfer_text(p.bytes_done, p.bytes_total, p.bytes_per_second))
                .flatten(),
            cancellable: !failed && !p.cancelling && p.space_id.is_none(),
            cancelling: p.cancelling,
        }),
        os_pretty_name: distro.map(|d| d.name),
        image: p.image.clone(),
        image_digest: None,
        kind: p.kind,
        arch: p.arch.clone(),
        host: None,
        host_name: p.host_name.clone(),
        power: None,
        cloud: None,
        cloud_place: None,
        cloud_delete: None,
    }
}

/// How far a machine's clock may be behind this one's when a record it
/// lists is compared with the moment a create here started.
const HOST_CLOCK_SKEW_MS: i64 = 5 * 60_000;

/// Whether `listed` is the record a machine keeps of the Space `p` is
/// creating there.
///
/// A machine lists the Space it creates for you as soon as it takes the
/// create, before the Space is up, as a record with no OS yet, not
/// answering ("Stopped, Linux on gamma-4"), under the name the create asked
/// for (or its own for an unnamed one). While `p` is in flight, or after it
/// failed, that record is the same Space as `p`'s row: it must not be a
/// second tile next to it. It is not a record of an older Space: one that
/// was already there when the create started (when the machine says when it
/// added it) stays.
fn is_hosts_record_of(listed: &Space, p: &PendingCreate) -> bool {
    let Some(host) = p.host.as_deref() else {
        return false;
    };
    // A finished create hands over to the registry's own row of that Space.
    if p.space_id.is_some()
        || listed.provider != Some(SpaceProvider::Relay)
        || listed.host.as_deref() != Some(host)
    {
        return false;
    }
    if listed
        .started_at
        .is_some_and(|at| at < p.started_at - HOST_CLOCK_SKEW_MS)
    {
        return false;
    }
    // Its name; an unnamed create is named by the machine, so any record
    // there that is not up yet is the one.
    let unnamed = p.name == pending_name("", p.os);
    listed.name == p.name || (unnamed && listed.status == SpaceStatus::Suspended)
}

/// The registry's Spaces plus a row per pending create (finished creates
/// the registry already lists are left out, so a Space never shows twice;
/// nor does the record a machine lists of the Space it is creating for you,
/// [`is_hosts_record_of`]). A Space being deleted shows Deleting; one whose
/// delete finished is left out even while the registry still lists it.
pub fn compose(registry: &[Space], state: &CreatesState) -> Vec<Space> {
    let mut out: Vec<Space> = registry
        .iter()
        .filter(|s| !state.pending.iter().any(|p| is_hosts_record_of(s, p)))
        .filter_map(|s| match state.deleting.iter().find(|d| d.id == s.id) {
            Some(d) if d.done => None,
            Some(_) => Some(deleting_space(s)),
            None => Some(match state.powering.iter().find(|p| p.id == s.id) {
                Some(p) => powering_space(s, p),
                None => s.clone(),
            }),
        })
        .collect();
    for p in &settle(state, registry).pending {
        out.push(pending_space(p));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(id: &str, name: &str, os: SpaceOs) -> CreateAction {
        CreateAction::Start {
            id: id.into(),
            name: name.into(),
            os,
            provider: SpaceProvider::Local,
            now: 1_000,
            image: None,
            kind: None,
            host_arch: None,
            gpu: false,
            host: None,
            host_name: None,
        }
    }

    fn progress(id: &str, phase: &str, fraction: Option<f64>) -> CreateAction {
        CreateAction::Progress {
            id: id.into(),
            phase: phase.into(),
            fraction,
            now: None,
            bytes_done: None,
            bytes_total: None,
            bytes_per_second: None,
        }
    }

    fn start_image(id: &str, image: &str, kind: Option<SpaceKind>) -> CreateAction {
        CreateAction::Start {
            id: id.into(),
            name: String::new(),
            os: SpaceOs::Linux,
            provider: SpaceProvider::Local,
            now: 1_000,
            image: Some(image.into()),
            kind,
            host_arch: Some("aarch64".into()),
            gpu: false,
            host: None,
            host_name: None,
        }
    }

    fn at(id: &str, phase: &str, fraction: Option<f64>, now: i64) -> CreateAction {
        CreateAction::Progress {
            id: id.into(),
            phase: phase.into(),
            fraction,
            now: Some(now),
            bytes_done: None,
            bytes_total: None,
            bytes_per_second: None,
        }
    }

    fn bytes_at(id: &str, done: u64, total: u64, rate: f64, now: i64) -> CreateAction {
        CreateAction::Progress {
            id: id.into(),
            phase: "pulling".into(),
            fraction: Some(done as f64 / total as f64),
            now: Some(now),
            bytes_done: Some(done),
            bytes_total: Some(total),
            bytes_per_second: Some(rate),
        }
    }

    #[test]
    fn unknown_os_cannot_start_a_create() {
        let state = CreatesState::default();
        assert_eq!(
            reduce(&state, &start("pending:unknown", "", SpaceOs::Unknown)),
            state
        );
    }

    /// The first macOS create on a Mac without Lume: the built-in Lume's
    /// download shows on the Space's own row, in words, with its bytes.
    #[test]
    fn the_runtime_set_up_on_first_use_shows_on_the_create() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:m", "", SpaceOs::Macos),
        );
        let s = reduce(
            &s,
            &CreateAction::Progress {
                id: "pending:m".into(),
                phase: "preparing".into(),
                fraction: Some(0.5),
                now: Some(2_000),
                bytes_done: Some(3 << 20),
                bytes_total: Some(6 << 20),
                bytes_per_second: Some((1 << 20) as f64),
            },
        );
        let row = pending_space(&s.pending[0]);
        let p = row.progress.unwrap();
        assert_eq!(p.label, "Setting up Lume\u{2026}");
        assert!(p.transfer.unwrap().contains("MB"));
        assert_eq!(row.detail, "This Mac \u{b7} Setting up Lume\u{2026}");
        // Then the image, as before.
        let s = reduce(&s, &bytes_at("pending:m", 1 << 30, 20 << 30, 5e7, 3_000));
        assert_eq!(
            pending_space(&s.pending[0]).progress.unwrap().label,
            "Downloading image\u{2026}"
        );
    }

    /// The first Linux create on a Mac with no Docker: the built-in Linux
    /// runtime's download, then its VM's boot, show on the Space's row.
    #[test]
    fn the_linux_runtime_set_up_shows_its_download_then_its_boot() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:l", "", SpaceOs::Linux),
        );
        let prep = |fraction: f64, bytes: Option<(u64, u64)>| CreateAction::Progress {
            id: "pending:l".into(),
            phase: "preparing".into(),
            fraction: Some(fraction),
            now: Some(2_000),
            bytes_done: bytes.map(|b| b.0),
            bytes_total: bytes.map(|b| b.1),
            bytes_per_second: bytes.map(|_| 5e7),
        };
        let s = reduce(&s, &prep(0.5, Some((240 << 20, 480 << 20))));
        let row = pending_space(&s.pending[0]);
        assert_eq!(
            row.detail,
            "This Mac \u{b7} Setting up Linux runtime\u{2026}"
        );
        assert!(row.progress.unwrap().transfer.unwrap().contains("MB"));
        // The boot: still setting up, no bytes any more.
        let s = reduce(&s, &prep(0.3, None));
        let p = pending_space(&s.pending[0]).progress.unwrap();
        assert_eq!(p.label, "Setting up Linux runtime\u{2026}");
        assert!(p.transfer.is_none());
        // Plain preparing (resolving the image) says what it always did.
        let plain = reduce(
            &reduce(
                &CreatesState::default(),
                &start("pending:p", "", SpaceOs::Linux),
            ),
            &CreateAction::Progress {
                id: "pending:p".into(),
                phase: "preparing".into(),
                fraction: None,
                now: Some(2_000),
                bytes_done: None,
                bytes_total: None,
                bytes_per_second: None,
            },
        );
        assert_ne!(
            pending_space(&plain.pending[0]).progress.unwrap().label,
            "Setting up Linux runtime\u{2026}"
        );
    }

    #[test]
    fn a_download_shows_its_bytes_rate_and_time_left() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:b", "", SpaceOs::Macos),
        );
        let mib = 1048576.0;
        let s = reduce(
            &s,
            &bytes_at(
                "pending:b",
                4_509_715_661,
                23_779_654_034,
                85.0 * mib,
                2_000,
            ),
        );
        let row = &compose(&[], &s)[0];
        let p = row.progress.as_ref().unwrap();
        assert_eq!(
            p.transfer.as_deref(),
            Some("4.2 of 22.1 GB \u{b7} 85 MB/s \u{b7} about 4 min")
        );
        assert!(p.cancellable && !p.cancelling);
        // Smooth: each new byte count moves the bar a little.
        let before = s.pending[0].permille;
        let s = reduce(
            &s,
            &bytes_at(
                "pending:b",
                4_609_715_661,
                23_779_654_034,
                85.0 * mib,
                3_000,
            ),
        );
        assert!(s.pending[0].permille > before);
        // After the download the words go.
        let s = reduce(&s, &at("pending:b", "booting", None, 4_000));
        assert_eq!(
            compose(&[], &s)[0].progress.as_ref().unwrap().transfer,
            None
        );
    }

    #[test]
    fn a_slow_download_that_still_moves_never_stalls() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:s", "", SpaceOs::Macos),
        );
        let limit_ms = (stall_limit_secs(CreateFamily::Macos, 1) * 1000.0) as i64;
        let total = 23_900_000_000u64;
        // 1 byte per report and no fraction (an engine that reports bytes
        // only): the bytes alone are the movement.
        let bytes_only = |done: u64, now: i64| CreateAction::Progress {
            id: "pending:s".into(),
            phase: "pulling".into(),
            fraction: None,
            now: Some(now),
            bytes_done: Some(done),
            bytes_total: Some(total),
            bytes_per_second: Some(10.0),
        };
        let mut s = reduce(&s, &bytes_only(1_000, 1_000));
        let mut now = 1_000;
        for i in 0..5 {
            now += limit_ms / 2;
            s = reduce(&s, &bytes_only(1_001 + i, now));
            s = reduce(&s, &CreateAction::Tick { now: now + 1 });
            assert_eq!(s.pending[0].error, None, "moving bytes are progress");
        }
        // Bytes that stop moving do stall.
        let s = reduce(
            &s,
            &CreateAction::Tick {
                now: now + limit_ms + 1_000,
            },
        );
        assert!(
            s.pending[0]
                .error
                .as_deref()
                .is_some_and(|e| e.ends_with(STALL_HINT))
        );
    }

    #[test]
    fn cancel_shows_cancelling_then_the_row_goes() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:c", "", SpaceOs::Linux),
        );
        let s = reduce(&s, &bytes_at("pending:c", 10, 100, 5.0, 2_000));
        let s = reduce(
            &s,
            &CreateAction::CancelStart {
                id: "pending:c".into(),
            },
        );
        let p = compose(&[], &s)[0].progress.clone().unwrap();
        assert_eq!(p.label, "Cancelling\u{2026}");
        assert!(p.cancelling && !p.cancellable && p.transfer.is_none());
        // No stall while it cleans up, and late progress is ignored.
        let s = reduce(&s, &CreateAction::Tick { now: 10_000_000 });
        assert_eq!(s.pending[0].error, None);
        let s2 = reduce(&s, &bytes_at("pending:c", 20, 100, 5.0, 3_000));
        assert_eq!(s2.pending[0].bytes_done, Some(10));
        // The create then ends with the SDK's Cancelled error: not a failure.
        let gone = reduce(
            &s,
            &CreateAction::Fail {
                id: "pending:c".into(),
                error: "cancelled: Cancelled local:space-1".into(),
                error_variant: String::new(),
            },
        );
        assert!(gone.pending.is_empty());
        let gone = reduce(
            &s,
            &CreateAction::CancelDone {
                id: "pending:c".into(),
            },
        );
        assert!(gone.pending.is_empty());
        // A cancel that fails says so.
        let failed = reduce(
            &s,
            &CreateAction::CancelFail {
                id: "pending:c".into(),
                error: "timed out: the daemon did not answer".into(),
            },
        );
        assert!(!failed.pending[0].cancelling && failed.pending[0].error.is_some());
    }

    fn band_of(p: &PendingCreate) -> (u32, u32) {
        bands(family(p), p.pulled)[PHASES.iter().position(|ph| *ph == p.phase).unwrap()]
    }

    fn registered(id: &str) -> Space {
        let mut s = pending_space(
            &reduce(
                &CreatesState::default(),
                &start("pending:x", "reg", SpaceOs::Linux),
            )
            .pending[0],
        );
        s.id = id.into();
        s.status = SpaceStatus::Running;
        s.progress = None;
        s
    }

    #[test]
    fn a_create_shows_at_once_then_follows_the_sdk() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:1", "", SpaceOs::Linux),
        );
        let rows = compose(&[], &s);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(
            (row.id.as_str(), row.name.as_str(), row.status),
            ("pending:1", "New Space", SpaceStatus::Provisioning)
        );
        let p = row.progress.as_ref().unwrap();
        assert_eq!((p.label.as_str(), p.permille), ("Starting\u{2026}", 10));
        assert_eq!(row.detail, "This Mac \u{b7} Starting\u{2026}");

        let s = reduce(&s, &progress("pending:1", "pulling", Some(0.5)));
        let (lo, hi) = band_of(&s.pending[0]);
        assert_eq!(s.pending[0].permille, (lo + hi).div_ceil(2));
        assert_eq!(
            compose(&[], &s)[0].progress.as_ref().unwrap().label,
            "Downloading image\u{2026}"
        );
        let s = reduce(&s, &progress("pending:1", "booting", None));
        let booting = band_of(&s.pending[0]).0;
        assert_eq!(s.pending[0].permille, booting);
        // Never backwards, even if a report arrives late.
        let s = reduce(&s, &progress("pending:1", "pulling", Some(0.5)));
        assert_eq!(s.pending[0].permille, booting);
        let s = reduce(&s, &progress("pending:1", "connecting", None));
        assert_eq!(s.pending[0].permille, band_of(&s.pending[0]).0);
        assert!(s.pending[0].permille < 1000);
    }

    #[test]
    fn bands_cover_the_bar_in_order_for_every_family() {
        use CreateFamily::*;
        for f in [Container, Vm, EmulatedVm, Macos, Windows, Cloud] {
            for pulled in [true, false] {
                let b = bands(f, pulled);
                assert_eq!(b[0].0, 10, "{f:?}");
                assert_eq!(b[5].1, 990, "{f:?} {pulled}");
                for w in b.windows(2) {
                    assert_eq!(w[0].1, w[1].0, "{f:?} {pulled}: contiguous");
                    assert!(w[0].0 <= w[0].1);
                }
                // Preparing ends at the same place with or without a pull.
                assert_eq!(b[0], bands(f, !pulled)[0]);
            }
            assert_eq!(bands(f, false)[1].0, bands(f, false)[1].1, "no pull band");
        }
    }

    #[test]
    fn without_a_fraction_time_moves_the_bar_toward_the_phase_end() {
        let s = reduce(
            &CreatesState::default(),
            &start_image("pending:o", "ghcr.io/trycua/omarchy:edge", None),
        );
        let p = &s.pending[0];
        assert_eq!(family(p), CreateFamily::EmulatedVm);
        let s = reduce(&s, &at("pending:o", "waiting_for_services", None, 2_000));
        let (lo, hi) = band_of(&s.pending[0]);
        assert!(hi > lo + 100, "the long phase");
        assert_eq!(s.pending[0].permille, lo);
        let expected = expected_seconds(CreateFamily::EmulatedVm)[4];
        let tick = |s: &CreatesState, secs: f64| {
            reduce(
                s,
                &CreateAction::Tick {
                    now: 2_000 + (secs * 1000.0) as i64,
                },
            )
        };
        let mut last = lo;
        let mut s2 = s.clone();
        for step in 1..=40 {
            s2 = tick(&s2, expected * step as f64 / 10.0);
            let now = s2.pending[0].permille;
            assert!(now >= last, "never backwards");
            assert!(now < hi, "never the phase's end before it ends");
            last = now;
        }
        // On pace: half the expected time is half the band.
        let half = tick(&s, expected / 2.0).pending[0].permille;
        assert_eq!(half, lo + (hi - lo) / 2);
        // The next phase takes over from where the bar is.
        let s3 = reduce(&s2, &at("pending:o", "connecting", None, 900_000));
        assert!(s3.pending[0].permille >= last);
        // A tick before the phase started, or a failed create, moves nothing.
        assert_eq!(tick(&s, -5.0), s);
        let failed = reduce(
            &s,
            &CreateAction::Fail {
                id: "pending:o".into(),
                error: "x".into(),
                error_variant: String::new(),
            },
        );
        assert_eq!(tick(&failed, 50.0), failed);
    }

    /// "Starting services…" (or any phase) never waits forever: a phase
    /// that stops moving fails the row with what stalled and what to do,
    /// after the SDK's own timeouts had their chance; progress that comes
    /// later clears it, and a late success still lands.
    #[test]
    fn a_stalled_phase_fails_the_row_with_an_actionable_error_and_recovers() {
        let s = reduce(
            &CreatesState::default(),
            &start_image("pending:c", "ghcr.io/trycua/linux:24.04", None),
        );
        assert_eq!(family(&s.pending[0]), CreateFamily::Container);
        let s = reduce(&s, &at("pending:c", "waiting_for_services", None, 2_000));
        let limit = stall_limit_secs(CreateFamily::Container, 4);
        // After the SDK's 120 s cua-spacesd timeout, so its error wins.
        assert!(limit > 180.0, "{limit}");
        let tick = |s: &CreatesState, secs: f64| {
            reduce(
                s,
                &CreateAction::Tick {
                    now: 2_000 + (secs * 1000.0) as i64,
                },
            )
        };
        assert_eq!(tick(&s, limit - 1.0).pending[0].error, None);
        let stuck = tick(&s, limit + 1.0);
        let err = stuck.pending[0]
            .error
            .clone()
            .expect("a stalled phase fails");
        assert_eq!(
            err,
            "The Space booted, but its service did not answer for 4 min. The image is \
             downloaded now, so another try is usually quick. Dismiss it and try again."
        );
        assert_eq!(err, stall_error("waiting_for_services", limit + 1.0));
        // More ticks leave the error alone.
        assert_eq!(tick(&stuck, limit * 3.0), stuck);
        // Moving again clears it; a late Finish lands.
        let moving = reduce(&stuck, &at("pending:c", "connecting", None, 300_000));
        assert_eq!(moving.pending[0].error, None);
        assert_eq!(moving.pending[0].phase, "connecting");
        let done = reduce(
            &stuck,
            &CreateAction::Finish {
                id: "pending:c".into(),
                space_id: "local:c".into(),
            },
        );
        assert_eq!(done.pending[0].error, None);
        assert_eq!(done.pending[0].phase, "ready");
        // A real failure is not a stall: later progress does not clear it.
        let failed = reduce(
            &s,
            &CreateAction::Fail {
                id: "pending:c".into(),
                error: "boom".into(),
                error_variant: String::new(),
            },
        );
        let after = reduce(&failed, &at("pending:c", "connecting", None, 9_000));
        assert!(after.pending[0].error.is_some());

        // A download that keeps moving never stalls, however long it takes;
        // one whose bytes stop does.
        let pull = reduce(
            &CreatesState::default(),
            &start_image("pending:p", "ghcr.io/trycua/linux:24.04", None),
        );
        let pull_limit = stall_limit_secs(CreateFamily::Container, 1);
        assert!(pull_limit >= 900.0, "{pull_limit}");
        let mut p = pull;
        let mut now = 1_000i64;
        for step in 1..=20 {
            now += (pull_limit * 500.0) as i64; // half the limit per step
            p = reduce(
                &p,
                &at("pending:p", "pulling", Some(step as f64 / 21.0), now),
            );
            p = reduce(&p, &CreateAction::Tick { now: now + 1 });
            assert_eq!(p.pending[0].error, None, "moving at step {step}");
        }
        // The same fraction again is not movement.
        p = reduce(
            &p,
            &at("pending:p", "pulling", Some(20.0 / 21.0), now + 1_000),
        );
        let p = reduce(
            &p,
            &CreateAction::Tick {
                now: now + (pull_limit * 1000.0) as i64 + 2_000,
            },
        );
        assert!(
            p.pending[0]
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("The image download made no progress")),
            "{:?}",
            p.pending[0].error
        );

        // Every phase of every family has a bound.
        for fam in [
            CreateFamily::Container,
            CreateFamily::Vm,
            CreateFamily::EmulatedVm,
            CreateFamily::Macos,
            CreateFamily::Windows,
            CreateFamily::Cloud,
        ] {
            for (i, phase) in PHASES.iter().enumerate() {
                let l = stall_limit_secs(fam, i);
                assert!((240.0..=6060.0).contains(&l), "{fam:?} {phase} {l}");
            }
        }
    }

    #[test]
    fn the_catalog_names_the_distribution_kind_and_platform_from_the_start() {
        let s = reduce(
            &CreatesState::default(),
            &start_image("pending:o", "ghcr.io/trycua/omarchy:edge", None),
        );
        let row = &compose(&[], &s)[0];
        assert_eq!(row.os_pretty_name.as_deref(), Some("Omarchy"));
        assert_eq!(
            crate::notch::os_icon(row.os, row.os_name.as_deref()),
            "os-omarchy"
        );
        assert_eq!(row.image.as_deref(), Some("ghcr.io/trycua/omarchy:edge"));
        assert_eq!(row.kind, Some(SpaceKind::Vm));
        assert_eq!(row.arch.as_deref(), Some("amd64"));
        assert!(s.pending[0].emulated);
        // A multi-platform image runs the host's platform, natively.
        let s = reduce(
            &CreatesState::default(),
            &start_image("pending:u", "ghcr.io/trycua/linux:24.04-slim", None),
        );
        let row = &compose(&[], &s)[0];
        assert_eq!(row.os_pretty_name.as_deref(), Some("Ubuntu 24.04"));
        assert_eq!(
            crate::notch::os_icon(row.os, row.os_name.as_deref()),
            "os-ubuntu"
        );
        assert_eq!(
            (row.kind, row.arch.as_deref()),
            (Some(SpaceKind::Container), Some("arm64"))
        );
        assert_eq!(family(&s.pending[0]), CreateFamily::Container);
        // An image the catalog does not list: the kind asked for, the
        // platform unknown until the Space reports it.
        let s = reduce(
            &CreatesState::default(),
            &start_image("pending:c", "registry.example/app:1", Some(SpaceKind::Vm)),
        );
        let row = &compose(&[], &s)[0];
        assert_eq!(row.os_pretty_name, None);
        assert_eq!((row.kind, row.arch.as_deref()), (Some(SpaceKind::Vm), None));
        assert_eq!(family(&s.pending[0]), CreateFamily::Vm);
    }

    #[test]
    fn without_a_pull_the_other_phases_share_the_bar() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:m", "Mac", SpaceOs::Macos),
        );
        let s = reduce(&s, &progress("pending:m", "creating", None));
        let creating = band_of(&s.pending[0]);
        assert_eq!(creating.0, bands(CreateFamily::Macos, true)[0].1);
        let s = reduce(&s, &progress("pending:m", "waiting_for_services", None));
        assert_eq!(s.pending[0].permille, band_of(&s.pending[0]).0);
        assert_eq!(pending_name("  ", SpaceOs::Macos), "macOS Space");
        assert_eq!(pending_name("cua-e2e-mac", SpaceOs::Macos), "cua-e2e-mac");
    }

    #[test]
    fn finish_keeps_the_row_until_the_registry_lists_it() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:1", "a", SpaceOs::Linux),
        );
        let s = reduce(
            &s,
            &CreateAction::Finish {
                id: "pending:1".into(),
                space_id: "local:a".into(),
            },
        );
        let rows = compose(&[], &s);
        assert_eq!(rows.len(), 1, "no gap before the refresh");
        assert_eq!(rows[0].progress.as_ref().unwrap().permille, 1000);
        let reg = [registered("local:a")];
        let rows = compose(&reg, &s);
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["local:a"],
            "never twice"
        );
        assert!(settle(&s, &reg).pending.is_empty());
    }

    #[test]
    fn a_failure_stays_inline_until_dismissed() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:1", "a", SpaceOs::Linux),
        );
        let s = reduce(
            &s,
            &CreateAction::Fail {
                id: "pending:1".into(),
                error: "no local runtime".into(),
                error_variant: String::new(),
            },
        );
        let row = &compose(&[], &s)[0];
        assert_eq!(row.status, SpaceStatus::Suspended);
        assert_eq!(row.detail, "no local runtime");
        let p = row.progress.as_ref().unwrap();
        assert_eq!(
            (p.label.as_str(), p.error.as_deref()),
            ("Failed", Some("no local runtime"))
        );
        // Progress after a failure is ignored.
        let s2 = reduce(&s, &progress("pending:1", "booting", None));
        assert_eq!(s2, s);
        let s = reduce(
            &s,
            &CreateAction::Dismiss {
                id: "pending:1".into(),
            },
        );
        assert!(compose(&[], &s).is_empty());
        assert!(is_pending("pending:1") && !is_pending("local:a"));
    }

    fn del(kind: &str, id: &str) -> CreateAction {
        match kind {
            "start" => CreateAction::DeleteStart {
                id: id.into(),
                now: 5_000,
            },
            "fail" => CreateAction::DeleteFail { id: id.into() },
            _ => CreateAction::DeleteDone { id: id.into() },
        }
    }

    #[test]
    fn a_delete_shows_deleting_at_once_whatever_the_probe_says() {
        let s = reduce(&CreatesState::default(), &del("start", "local:a"));
        assert!(is_deleting(&s, "local:a"));
        // A probe during the delete sees it unreachable (timed out), or
        // still running: the row says Deleting either way.
        let mut running = registered("local:a");
        running.sdk = Some(crate::model::SpaceSdkRef {
            features: vec!["desktop_stream".into()],
            spacesd_version: "0.4.0".into(),
            reachable: true,
            error: None,
        });
        let mut probe = running.clone();
        probe.status = SpaceStatus::Suspended;
        probe.detail = "Unreachable \u{b7} timed out".into();
        for reg in [[running], [probe]] {
            let row = &compose(&reg, &s)[0];
            assert_eq!(row.status, SpaceStatus::Deleting);
            assert_eq!(row.detail, "Deleting\u{2026}");
            assert!(!row.status.is_live());
            assert!(!row.sdk.as_ref().unwrap().reachable);
        }
        // A second Delete does nothing.
        assert_eq!(reduce(&s, &del("start", "local:a")), s);
        // A failed create is dismissed, never deleted.
        assert_eq!(reduce(&s, &del("start", "pending:1")), s);
    }

    #[test]
    fn a_finished_delete_hides_the_row_until_the_registry_drops_it() {
        let s = reduce(&CreatesState::default(), &del("start", "local:a"));
        let s = reduce(&s, &del("done", "local:a"));
        let reg = [registered("local:a"), registered("local:b")];
        let ids = |rows: Vec<Space>| rows.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(compose(&reg, &s)), ["local:b"]);
        // Still listed: kept; dropped: settled away.
        assert_eq!(settle(&s, &reg), s);
        let s = settle(&s, &reg[1..]);
        assert!(s.deleting.is_empty());
        // A new Space under the same id shows again.
        let s = reduce(&s, &del("start", "local:c"));
        let s = reduce(&s, &del("done", "local:c"));
        let s = reduce(&s, &start("pending:9", "c", SpaceOs::Linux));
        let s = reduce(
            &s,
            &CreateAction::Finish {
                id: "pending:9".into(),
                space_id: "local:c".into(),
            },
        );
        assert!(!is_deleting(&s, "local:c"));
    }

    #[test]
    fn a_failed_delete_restores_the_row() {
        let s = reduce(&CreatesState::default(), &del("start", "local:a"));
        let s = reduce(&s, &del("fail", "local:a"));
        assert_eq!(s, CreatesState::default());
        let reg = [registered("local:a")];
        assert_eq!(compose(&reg, &s)[0].status, SpaceStatus::Running);
        // Old states (no `deleting`) still decode.
        let old: CreatesState = serde_json::from_str(r#"{"pending":[]}"#).unwrap();
        assert_eq!(old, CreatesState::default());
    }

    #[test]
    fn out_of_credit_is_one_line_and_add_credit_only_while_billing_shows() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:1", "demo", SpaceOs::Linux),
        );
        let s = reduce(
            &s,
            &CreateAction::Fail {
                id: "pending:1".into(),
                error: "You're out of Cua Cloud credit. Add credit at https://run.cua.ai/billing"
                    .into(),
                error_variant: String::new(),
            },
        );
        let space = pending_space(&s.pending[0]);
        assert_eq!(space.detail, crate::billing::OUT_OF_CREDIT);
        let notice = crate::spaces::sidebar::detail(&space).credit_notice;
        if crate::billing::BILLING_SHOWN {
            let n = notice.expect("notice");
            assert_eq!(
                (n.text.as_str(), n.button.as_str(), n.url.as_str()),
                (
                    crate::billing::OUT_OF_CREDIT,
                    "Add credit",
                    "https://run.cua.ai/billing"
                )
            );
        } else {
            assert_eq!(notice, None, "no Add credit while the apps hide billing");
        }
        // Any other failure has no notice.
        let s = reduce(
            &s,
            &CreateAction::Dismiss {
                id: "pending:1".into(),
            },
        );
        let s = reduce(&s, &start("pending:2", "b", SpaceOs::Linux));
        let s = reduce(
            &s,
            &CreateAction::Fail {
                id: "pending:2".into(),
                error: "no local runtime".into(),
                error_variant: String::new(),
            },
        );
        assert!(
            crate::spaces::sidebar::detail(&pending_space(&s.pending[0]))
                .credit_notice
                .is_none()
        );
    }

    #[test]
    fn a_stopped_download_says_how_far_it_got() {
        let s = reduce(
            &CreatesState::default(),
            &start("pending:m", "", SpaceOs::Macos),
        );
        let s = reduce(
            &s,
            &CreateAction::Progress {
                id: "pending:m".into(),
                phase: "pulling".into(),
                fraction: None,
                now: Some(1_000),
                bytes_done: Some(4_509_715_660),
                bytes_total: Some(23_729_694_310),
                bytes_per_second: Some(1_000_000.0),
            },
        );
        let limit_ms = (stall_limit_secs(CreateFamily::Macos, 1) * 1000.0) as i64;
        let s = reduce(
            &s,
            &CreateAction::Tick {
                now: 1_000 + limit_ms + 1_000,
            },
        );
        let err = s.pending[0].error.clone().unwrap();
        assert!(
            err.starts_with("The image download stopped at 4.2 of 22.1 GB: nothing arrived for"),
            "{err}"
        );
        assert!(err.ends_with("Check your internet connection. Dismiss it and try again."));
        assert!(is_stalled(&s.pending[0]));
        // Other phases keep `cua doctor` as the last resort.
        assert_eq!(
            stall_error("booting", 300.0),
            "Booting made no progress for 5 min. If it keeps happening, run `cua doctor` in \
             Terminal. Dismiss it and try again."
        );
    }

    #[test]
    fn a_failure_names_its_cause_when_it_is_known() {
        let disk = "not enough disk space to pull ghcr.io/trycua/macos:26: it needs about 30.0 GB \
                    and cua keeps 5.0 GB free, but only 12.4 GB is available on /Users/me/.lume \
                    (run `cua cache prune` to free space, or lower CUA_DISK_MIN_FREE)";
        assert_eq!(
            failure_text(disk, Some("preparing"), Some(SpaceProvider::Local), false),
            "Not enough disk space: this Space needs about 30.0 GB, plus 5.0 GB kept free, and \
             only 12.4 GB is available. Free up space on this Mac (or run `cua cache prune` in \
             Terminal), then try again."
        );
        assert_eq!(
            failure_text("not enough disk space", None, None, false),
            "Not enough disk space for this Space. Free up space on this Mac (or run `cua cache \
             prune` in Terminal), then try again."
        );
        // The Space's service missed what was left of the budget.
        let spacesd = |t: &str| {
            format!(
                "timed out: sandbox cua-mac-1: cua-spacesd did not answer within {t} \
                 (connection refused); see `cua sb logs cua-mac-1`"
            )
        };
        assert_eq!(
            failure_text(&spacesd("120s"), Some("waiting_for_services"), None, false),
            "The Space booted, but its service did not answer within 2 min. The image is \
             downloaded now, so trying again usually works."
        );
        assert_eq!(
            failure_text(&spacesd("45.2s"), None, None, false),
            "The Space booted, but its service did not answer within 45 s. The image is \
             downloaded now, so trying again usually works."
        );
        for spent in ["0ns", "1.2s", "350ms"] {
            assert_eq!(
                failure_text(&spacesd(spent), Some("waiting_for_services"), None, false),
                "The Space booted, but the time allowed for creating it ran out before its \
                 service answered. The image is downloaded now, so trying again usually works.",
                "{spent}"
            );
        }
        assert_eq!(
            failure_text(
                "timed out: waiting",
                Some("waiting_for_services"),
                None,
                false
            ),
            "The Space booted, but its service did not answer in time. The image is downloaded \
             now, so trying again usually works."
        );
        assert_eq!(
            failure_text(
                "sandbox 'x' was not ready after 600s: pull",
                Some("pulling"),
                None,
                false
            ),
            "The image download did not finish in the time allowed. Check your internet \
             connection, then try again."
        );
        // Local Network privacy on the Mac that runs the VM.
        let sdk = "Local Network access is not available: cua cannot reach the VM at \
                   192.168.64.45:3211 (No route to host (os error 65)). macOS blocks an app's \
                   local network connections until they are allowed";
        assert_eq!(
            failure_text(sdk, Some("booting"), Some(SpaceProvider::Relay), false),
            "The Mac running this Space can't reach its new VM because Cua doesn't have Local \
             Network access there. On that Mac, open System Settings > Privacy & Security > \
             Local Network, turn on Cua Spaces (or cua-spacesd), then try again."
        );
        for raw in [
            sdk,
            "host: create failed: connect 192.168.64.7:3211: No route to host (os error 65)",
        ] {
            assert!(
                failure_text(raw, None, Some(SpaceProvider::Local), false)
                    .starts_with("This Mac can't reach its new VM"),
                "{raw}"
            );
        }
        // A LAN address with no route is not this (a machine is down).
        let lan = "connect 10.0.0.9:7400: No route to host (os error 65)";
        assert_eq!(failure_text(lan, None, None, false), lan);
        // Apple's two-macOS-VM limit, however Lume words it.
        for raw in [
            "lume API 500: The number of virtual machines exceeds the limit.",
            "Error Domain=VZErrorDomain Code=6 virtualMachineLimitExceeded",
            "booting: the maximum number of VMs has been reached (limit 2)",
        ] {
            assert!(
                failure_text(raw, Some("booting"), Some(SpaceProvider::Local), false)
                    .starts_with("This Mac is already running two macOS VMs"),
                "{raw}"
            );
        }
        // A relay host that refused the sign-in.
        let unauth = "unauthenticated: token rejected";
        assert!(
            failure_text(unauth, Some("preparing"), Some(SpaceProvider::Relay), false)
                .starts_with("Your other Mac did not accept the sign-in")
        );
        assert!(
            failure_text(unauth, None, Some(SpaceProvider::Local), false)
                .starts_with("The new Space's service did not accept the app's token")
        );
        // GPU acceleration, only when it was asked for.
        let gpu = "invalid request: GPU acceleration: Needs a Mac with Apple silicon";
        assert_eq!(
            failure_text(gpu, Some("preparing"), None, true),
            format!(
                "GPU acceleration could not be turned on here ({gpu}). Create the Space again \
                 with GPU acceleration off."
            )
        );
        assert_eq!(failure_text(gpu, None, None, false), gpu);
        // Anything else is the SDK's words (a dead daemon in plain words).
        assert_eq!(failure_text(" boom \n", None, None, false), "boom");
        assert_eq!(
            failure_text(
                "transport: Connection refused (os error 61)",
                None,
                None,
                false
            ),
            crate::errors::DAEMON_NOT_RUNNING
        );
        // None of them reads as a stall (progress would clear it).
        for raw in [disk, unauth, gpu, &spacesd("0ns")] {
            assert!(
                !failure_text(raw, None, None, true).ends_with(STALL_HINT),
                "{raw}"
            );
        }
        // The row carries it, with where it failed and where it ran.
        let s = reduce(
            &CreatesState::default(),
            &start("pending:w", "", SpaceOs::Macos),
        );
        let s = reduce(&s, &progress("pending:w", "waiting_for_services", None));
        let s = reduce(
            &s,
            &CreateAction::Fail {
                id: "pending:w".into(),
                error: spacesd("0ns"),
                error_variant: String::new(),
            },
        );
        assert!(
            s.pending[0]
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("The Space booted, but the time allowed"))
        );
        assert_eq!(
            crate::wizard::create_failed_text("not enough disk space"),
            "Could not create the Space: Not enough disk space for this Space. Free up space on \
             this Mac (or run `cua cache prune` in Terminal), then try again."
        );
    }

    #[test]
    fn a_create_on_another_machine_waits_for_its_download_and_says_where() {
        let relay = |os| CreateAction::Start {
            id: "pending:r".into(),
            name: String::new(),
            os,
            provider: SpaceProvider::Relay,
            now: 0,
            image: None,
            kind: None,
            host_arch: Some("arm64".into()),
            gpu: false,
            host: None,
            host_name: None,
        };
        let s = reduce(&CreatesState::default(), &relay(SpaceOs::Macos));
        assert_eq!(
            compose(&[], &s)[0].detail,
            "Another machine \u{b7} Starting\u{2026}"
        );
        // The host pulls 22 GB without reporting: no stall at 5 min, nor
        // at the local preparing limit.
        let local_limit = stall_limit_secs(CreateFamily::Macos, 0);
        let s5 = reduce(
            &s,
            &CreateAction::Tick {
                now: ((local_limit + 60.0) * 1000.0) as i64,
            },
        );
        assert_eq!(s5.pending[0].error, None);
        let limit = stall_limit(&s.pending[0], 0);
        assert!(limit >= stall_limit_secs(CreateFamily::Macos, 1), "{limit}");
        let late = reduce(
            &s,
            &CreateAction::Tick {
                now: ((limit + 1.0) * 1000.0) as i64,
            },
        );
        assert!(late.pending[0].error.is_some());
        // Here, preparing keeps its own limit.
        let mut here = s.pending[0].clone();
        here.provider = SpaceProvider::Local;
        assert_eq!(stall_limit(&here, 0), local_limit);
    }

    // ---- a machine's record of the Space it is creating ----

    const GAMMA: &str = "96fedb7e1be65c3d31fa18587febde2c";
    const GAMMA_NAME: &str = "gamma-4 Mac Studio";
    const SKEW_BASE: i64 = 1_790_000_000_000;

    fn start_on(
        id: &str,
        name: &str,
        os: SpaceOs,
        provider: SpaceProvider,
        host: Option<&str>,
    ) -> CreateAction {
        CreateAction::Start {
            id: id.into(),
            name: name.into(),
            os,
            provider,
            now: SKEW_BASE,
            image: Some("ghcr.io/trycua/macos:26".into()),
            kind: Some(SpaceKind::Vm),
            host_arch: Some("arm64".into()),
            gpu: false,
            host: host.map(str::to_string),
            host_name: host.map(|_| GAMMA_NAME.to_string()),
        }
    }

    /// A create on gamma-4 through the relay.
    fn relay_start(id: &str, name: &str, os: SpaceOs) -> CreateAction {
        start_on(id, name, os, SpaceProvider::Relay, Some(GAMMA))
    }

    /// The row a machine lists for a Space it is still creating: no OS
    /// yet, not answering, so it reads "Stopped, Linux on <machine>".
    fn hosts_record(id: &str, name: &str, host: &str, added_at_ms: Option<i64>) -> Space {
        let mut s = crate::spaces::row_to_space(
            &crate::model::SpaceRow {
                id: id.into(),
                name: name.into(),
                provider: "relay".into(),
                spacesd_version: String::new(),
                features: vec![],
                added_at: None,
                os: None,
                os_name: None,
                os_pretty_name: None,
                image: None,
                image_digest: None,
                kind: None,
                arch: None,
                reachable: false,
                error: None,
                host: Some(host.into()),
                host_name: Some(GAMMA_NAME.into()),
                power: None,
                power_state: None,
                cloud: None,
                cloud_place: None,
                cloud_delete: None,
            },
            SKEW_BASE,
        );
        s.started_at = added_at_ms;
        s
    }

    fn names(rows: &[Space]) -> Vec<&str> {
        rows.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn a_relay_create_and_its_machines_record_of_it_are_one_row() {
        let ghost = hosts_record(
            "relay:96fe/space-46e0",
            "e2e-1005-gamma-macos",
            GAMMA,
            Some(SKEW_BASE + 9_000),
        );
        // Not the create's: another Space on the same machine.
        let other = registered("relay:96fe/space-9c73");
        let registry = [other.clone(), ghost.clone()];
        let s = reduce(
            &CreatesState::default(),
            &relay_start("pending:gamma", "e2e-1005-gamma-macos", SpaceOs::Macos),
        );

        // While it runs: the create's row, once; not "Stopped, Linux".
        let rows = compose(&registry, &s);
        assert_eq!(rows.len(), 2, "{:?}", names(&rows));
        assert_eq!(rows[0].id, other.id);
        assert_eq!(
            (rows[1].id.as_str(), rows[1].status, rows[1].os),
            ("pending:gamma", SpaceStatus::Provisioning, SpaceOs::Macos)
        );
        assert_eq!(rows[1].host_name.as_deref(), Some(GAMMA_NAME));

        // The machine could not reach its VM: the row says Failed and names
        // the machine (not "This Mac"); the record is still not a second
        // tile.
        let failed = reduce(
            &s,
            &CreateAction::Fail {
                id: "pending:gamma".into(),
                error: "Local Network access is not available: cua cannot reach the VM at \
                        192.168.64.45:3211 (No route to host (os error 65))"
                    .into(),
                error_variant: String::new(),
            },
        );
        let rows = compose(&registry, &failed);
        assert_eq!(rows.len(), 2, "{:?}", names(&rows));
        let row = &rows[1];
        assert_eq!(row.id, "pending:gamma");
        assert!(
            row.detail
                .starts_with("gamma-4 Mac Studio can't reach its new VM because Cua doesn't have Local Network access there."),
            "{}",
            row.detail
        );
        assert_eq!(
            row.progress.as_ref().unwrap().error.as_ref(),
            Some(&row.detail)
        );

        // Removed from the list: the machine's own record is what is left
        // (it is a Space there, and can be deleted).
        let gone = reduce(
            &failed,
            &CreateAction::Dismiss {
                id: "pending:gamma".into(),
            },
        );
        assert_eq!(compose(&registry, &gone).len(), 2);
        assert!(compose(&registry, &gone).iter().any(|s| s.id == ghost.id));

        // Ready: the pending row hands over to the registry's one row.
        let done = reduce(
            &s,
            &CreateAction::Finish {
                id: "pending:gamma".into(),
                space_id: ghost.id.clone(),
            },
        );
        let rows = compose(&registry, &done);
        assert_eq!(rows.len(), 2, "{:?}", names(&rows));
        assert_eq!(rows.iter().filter(|r| r.id == ghost.id).count(), 1);
        assert!(rows.iter().all(|r| !is_pending(&r.id)));
    }

    #[test]
    fn an_unnamed_relay_create_hides_the_machines_new_record_of_it() {
        let s = reduce(
            &CreatesState::default(),
            &relay_start("pending:u", "", SpaceOs::Macos),
        );
        assert_eq!(s.pending[0].name, "macOS Space");
        // The machine names the Space itself, and it is not up yet.
        let ghost = hosts_record(
            "relay:96fe/space-464e",
            "space-464e2d931db4b3ec",
            GAMMA,
            Some(SKEW_BASE + 8_000),
        );
        let rows = compose(std::slice::from_ref(&ghost), &s);
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["pending:u"]
        );

        // A Space that is up on that machine is somebody else's (or an
        // older one's): it stays, as does one the machine added before
        // this create began.
        let up = registered("relay:96fe/space-up");
        let mut up = Space {
            host: Some(GAMMA.into()),
            provider: Some(SpaceProvider::Relay),
            ..up
        };
        up.status = SpaceStatus::Running;
        let older = hosts_record(
            "relay:96fe/space-old",
            "space-old",
            GAMMA,
            Some(SKEW_BASE - 3_600_000),
        );
        let rows = compose(&[ghost, up.clone(), older.clone()], &s);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, [up.id.as_str(), older.id.as_str(), "pending:u"]);
    }

    #[test]
    fn a_record_that_is_not_the_creates_stays_listed() {
        let s = reduce(
            &CreatesState::default(),
            &relay_start("pending:n", "dev", SpaceOs::Linux),
        );
        // Same name on another machine, or on this one (a local Space): not
        // this create's.
        let elsewhere = hosts_record("relay:aaaa/space-1", "dev", "aaaa", Some(SKEW_BASE + 1));
        let mut here = registered("local:dev");
        here.name = "dev".into();
        // Same machine and name, but listed long before this create began:
        // an older Space of the same name.
        let older = hosts_record(
            "relay:96fe/space-2",
            "dev",
            GAMMA,
            Some(SKEW_BASE - 3_600_000),
        );
        // Another name on the same machine, not up yet: only an unnamed
        // create takes those.
        let other = hosts_record("relay:96fe/space-3", "build", GAMMA, Some(SKEW_BASE + 1));
        let registry = [
            elsewhere.clone(),
            here.clone(),
            older.clone(),
            other.clone(),
        ];
        let rows = compose(&registry, &s);
        assert_eq!(rows.len(), 5, "{:?}", names(&rows));
        assert!(rows.iter().any(|r| r.id == "pending:n"));

        // A create that is not on a machine of yours hides nothing.
        let local = reduce(
            &CreatesState::default(),
            &start_on(
                "pending:l",
                "dev",
                SpaceOs::Linux,
                SpaceProvider::Local,
                None,
            ),
        );
        assert_eq!(compose(&registry, &local).len(), 5);

        // A finished create hands over by its Space's id, not by name.
        let sibling = hosts_record("relay:96fe/space-4", "dev", GAMMA, Some(SKEW_BASE + 5));
        let done = reduce(
            &s,
            &CreateAction::Finish {
                id: "pending:n".into(),
                space_id: "relay:96fe/space-5".into(),
            },
        );
        let rows = compose(std::slice::from_ref(&sibling), &done);
        assert_eq!(rows.iter().filter(|r| r.id == sibling.id).count(), 1);
    }

    #[test]
    fn a_failure_on_another_mac_names_that_mac() {
        let sdk = "Local Network access is not available: cua cannot reach the VM at \
                   192.168.64.45:3211 (No route to host (os error 65))";
        // The machine's name when known; else who runs it, never this Mac.
        assert!(
            failure_text_on(
                sdk,
                None,
                Some(SpaceProvider::Relay),
                Some(GAMMA_NAME),
                false
            )
            .starts_with("gamma-4 Mac Studio can't reach its new VM because")
        );
        assert!(
            failure_text_on(sdk, None, Some(SpaceProvider::Relay), None, false)
                .starts_with("The Mac running this Space can't reach its new VM because")
        );
        assert!(
            failure_text_on(sdk, None, Some(SpaceProvider::Local), None, false)
                .starts_with("This Mac can't reach its new VM because")
        );
        // The toast the wizard shows after a create fails.
        assert_eq!(
            crate::wizard::create_failed_text_on(sdk, Some(SpaceProvider::Relay), Some(GAMMA_NAME)),
            format!(
                "Could not create the Space: {}",
                failure_text_on(
                    sdk,
                    None,
                    Some(SpaceProvider::Relay),
                    Some(GAMMA_NAME),
                    false
                )
            )
        );
        assert_eq!(
            crate::wizard::create_failed_text_on(sdk, None, None),
            crate::wizard::create_failed_text(sdk)
        );
        // Apple's two macOS VMs are the other Mac's too.
        let limit = "booting: the maximum number of VMs has been reached (limit 2)";
        assert!(
            failure_text_on(
                limit,
                Some("booting"),
                Some(SpaceProvider::Relay),
                Some(GAMMA_NAME),
                false
            )
            .starts_with("gamma-4 Mac Studio is already running two macOS VMs")
        );
        assert!(
            failure_text_on(
                limit,
                Some("booting"),
                Some(SpaceProvider::Local),
                None,
                false
            )
            .starts_with("This Mac is already running two macOS VMs")
        );
    }
}
