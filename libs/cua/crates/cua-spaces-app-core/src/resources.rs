// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Processor, memory and disk of this Mac, each extra Space on it and your
//! other computers, as every app shows them: Home's strips, the Space
//! detail, the menu bar warning.
//!
//! Plain data in (the runtime's `ResourcesSnapshot`, camelCase, as
//! `Spaces.resources` and `Spaces.watch_resources` return it) and plain
//! data out: rows of a label, a value in words and, for the small bars, a
//! fraction. The words live here, so no shell formats a number itself.
//!
//! Labels are only "Processor", "Memory" and "Space for files"; values say
//! "busy", "in use" and "free". Never vCPU, RSS, APFS, sparse, qcow2 or
//! cgroup. The CLI says the same things with the same numbers
//! (`cua_spaces::resources::words`).
//!
//! Numbers only: counts, bytes and percentages. Nothing here knows a file
//! name, a path, a process or a window.

use crate::spaces::sidebar::format_size;
use serde::{Deserialize, Serialize};

/// The Processor row's label.
pub const PROCESSOR: &str = "Processor";
/// The Memory row's label.
pub const MEMORY: &str = "Memory";
/// The disk row's label.
pub const FILES: &str = "Space for files";
/// The warning's button.
pub const SEE_WHAT_USES_SPACE: &str = "See what's using space";
/// The warning under the line.
pub const LOW_TEXT: &str = "This Mac is almost out of space. Delete old Spaces or snapshots.";
/// The warning under the floor where creating a Space already fails.
pub const CRITICAL_TEXT: &str =
    "This Mac is out of space, so new Spaces can't be created. Delete old Spaces or snapshots.";

/// How low the free disk is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskLevel {
    /// Plenty.
    #[default]
    Ok,
    /// Under 50 GB or 15% of the volume, whichever line is higher (never
    /// above 100 GB).
    Low,
    /// Under the floor where creating a Space already fails.
    Critical,
}

/// The low-disk signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiskWarning {
    /// How low it is.
    pub level: DiskLevel,
    /// Free bytes on the volume that holds Spaces.
    pub free: u64,
    /// Size of that volume.
    pub total: u64,
    /// The line that was crossed (0 when ok).
    pub threshold: u64,
}

/// A computer: this Mac, or one of your other computers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ComputerResources {
    /// `this-mac`, or the relay machine id.
    pub id: String,
    /// Display name (empty for this Mac).
    pub name: String,
    /// Reachable now.
    pub online: bool,
    /// When the computer measured this, Unix ms.
    pub sampled_at_ms: u64,
    /// Processor cores.
    pub cpu_count: u32,
    /// Busy now, 0 to 100.
    pub cpu_percent: f64,
    /// 1-minute load average.
    pub load_average_1m: f64,
    /// Memory size, bytes.
    pub memory_total: u64,
    /// Memory in use, bytes.
    pub memory_used: u64,
    /// Size of the volume that holds Spaces, bytes.
    pub disk_total: u64,
    /// Free bytes on it.
    pub disk_free: u64,
    /// Extra Spaces on it.
    pub spaces_count: u32,
    /// Of those, running.
    pub spaces_running: u32,
    /// Cores given to the running ones.
    pub spaces_cpu_allocated: u32,
    /// Memory given to the running ones, bytes.
    pub spaces_memory_allocated: u64,
    /// Memory the running guests report in use, bytes.
    pub spaces_memory_used: u64,
    /// Bytes all its extra Spaces take on it.
    pub spaces_disk_bytes: u64,
    /// False when some Space's disk could only be estimated.
    pub spaces_disk_exact: bool,
    /// The low-disk signal.
    pub warning: DiskWarning,
}

/// What a Space's guest reports about itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GuestUsage {
    /// Cores the guest sees.
    pub cpu_count: u32,
    /// Busy now, 0 to 100.
    pub cpu_percent: f64,
    /// 1-minute load average.
    pub load_average_1m: f64,
    /// Memory in use, bytes.
    pub memory_used: u64,
    /// Memory size, bytes.
    pub memory_total: u64,
    /// `memory_total` is the guest's own limit.
    pub memory_limited: bool,
    /// Disk in use, bytes.
    pub disk_used: u64,
    /// Disk size, bytes; none when the guest's file system has no size of
    /// its own. A sentinel (`i64::MAX`, `u64::MAX`, -1) or 0 reads as none
    /// ([`known_size`]), so no shell ever shows one.
    #[serde(deserialize_with = "de_known_size")]
    pub disk_total: Option<u64>,
    /// `disk_total` is the guest's own disk.
    pub disk_limited: bool,
}

/// Sizes at or above this are "unlimited" or "unknown" sentinels, never
/// sizes (no volume holds an exbibyte).
pub const SIZE_SENTINEL_FLOOR: u64 = 1 << 60;

/// A size, or none when it is 0 or a sentinel.
pub fn known_size(bytes: u64) -> Option<u64> {
    (bytes > 0 && bytes < SIZE_SENTINEL_FLOOR).then_some(bytes)
}

fn de_known_size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = Option::<serde_json::Number>::deserialize(d)?;
    Ok(v.and_then(|n| n.as_u64()).and_then(known_size))
}

/// What a Space costs this Mac's disk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiskOnHost {
    /// Bytes only this Space holds; none when they could not be measured.
    pub bytes: Option<u64>,
    /// Blocks its files occupy, clones counted in full.
    pub allocated: u64,
    /// Part of it is shared with other copies.
    pub shares_space: bool,
}

/// One extra Space on this Mac.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpaceResources {
    /// `local:<name>`.
    pub id: String,
    /// `running`, `stopped`, `suspended`, or empty when unknown.
    pub power_state: String,
    /// Cores given to it (0: unknown).
    pub cpu_allocated: u32,
    /// Memory given to it, bytes (0: unknown).
    pub memory_allocated: u64,
    /// Its disk's size limit, bytes (0: unknown).
    pub disk_allocated: u64,
    /// The guest's own numbers; none while it is off or unreachable.
    pub usage: Option<GuestUsage>,
    /// What it costs this Mac: its own data only, never the base it shares.
    pub disk_on_host: Option<DiskOnHost>,
    /// The base image it is a copy-on-write copy of, counted once.
    pub base: Option<SharedBase>,
}

/// A base image several Spaces share copy-on-write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SharedBase {
    /// Spaces with the same key share the same blocks.
    pub key: String,
    /// The image reference it came from (`ghcr.io/trycua/linux:24.04`).
    pub image: String,
    /// What it takes on this Mac, once; none when the runtime cannot tell
    /// it apart from the Spaces' own data.
    pub bytes: Option<u64>,
    /// Local Spaces that use it, this one included.
    pub shared_with: u32,
}

/// One sample of everything (`Spaces.resources`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResourcesSnapshot {
    /// Grows by one per sample.
    pub sequence: u64,
    /// Unix ms.
    pub sampled_at_ms: u64,
    /// This Mac.
    pub this_mac: ComputerResources,
    /// Extra Spaces on this Mac.
    pub spaces: Vec<SpaceResources>,
    /// Your other computers.
    pub computers: Vec<ComputerResources>,
    /// This Mac's low-disk signal, for the menu bar.
    pub warning: DiskWarning,
}

/// How a row's bar and value are coloured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceTone {
    /// Nothing to say.
    #[default]
    Normal,
    /// Running low.
    Warning,
    /// Out.
    Critical,
}

/// One line of a panel: "Processor: 12 cores · 38% busy" with its bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRow {
    /// "Processor", "Memory" or "Space for files".
    pub label: String,
    /// The numbers in words.
    pub value: String,
    /// How full the small bar is, 0 to 1; none for a row with no bar.
    pub fraction: Option<f64>,
    /// How to colour it.
    pub tone: ResourceTone,
}

/// The low-disk warning with its button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarningView {
    /// The sentence.
    pub text: String,
    /// The button ("See what's using space").
    pub button: String,
    /// Under the floor where creating a Space already fails.
    pub critical: bool,
}

/// This Mac's panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThisMacView {
    /// "This Mac".
    pub title: String,
    /// Processor, Memory, Space for files.
    pub rows: Vec<ResourceRow>,
    /// "Your extra Spaces use: 6 cores, 12 GB of memory, about 25 GB for
    /// files." None when there are no extra Spaces.
    pub extra_spaces: Option<String>,
    /// The warning, when the disk is low.
    pub warning: Option<WarningView>,
}

/// One extra Space's panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceResourcesView {
    /// The Space's id.
    pub id: String,
    /// Processor, Memory, Space for files: what it was given and, while it
    /// runs, what it uses now.
    pub rows: Vec<ResourceRow>,
    /// What it costs this Mac, not what the guest sees: "Uses about 31 GB
    /// on this Mac (shares space with other copies)". None for a container.
    pub on_this_mac: Option<String>,
    /// The Space is off: "Stopped · uses about 6 GB on this Mac".
    pub off_line: Option<String>,
    /// Whether the numbers are live (the Space runs and answered).
    pub live: bool,
}

/// Another of your computers' panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerView {
    /// The computer's id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Reachable now.
    pub online: bool,
    /// "Offline, as of 5 min ago" or "Updated 2 min ago"; none when the
    /// numbers are a moment old.
    pub age: Option<String>,
    /// Processor, Memory, Space for files.
    pub rows: Vec<ResourceRow>,
    /// "Its extra Spaces use: ..." (see [`ThisMacView::extra_spaces`]).
    pub extra_spaces: Option<String>,
    /// The warning, when its disk is low.
    pub warning: Option<WarningView>,
}

/// Everything the apps draw from one snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesView {
    /// This Mac.
    pub this_mac: ThisMacView,
    /// Each extra Space.
    pub spaces: Vec<SpaceResourcesView>,
    /// Each other computer.
    pub computers: Vec<ComputerView>,
    /// The menu bar's warning: none when the disk is fine.
    pub warning: Option<WarningView>,
}

/// "12 cores · 38% busy".
pub fn processor_text(cores: u32, busy: Option<f64>) -> String {
    let cores = format!("{cores} core{}", if cores == 1 { "" } else { "s" });
    match busy {
        Some(p) => format!("{cores} \u{b7} {}% busy", p.round().clamp(0.0, 100.0)),
        None => cores,
    }
}

/// "3.1 of 8 GB": the used number with the total's unit.
pub fn of_text(used: u64, total: u64) -> String {
    let used = used.min(total);
    let (u, t) = (format_size(used), format_size(total));
    match (u.rsplit_once(' '), t.rsplit_once(' ')) {
        (Some((n, unit)), Some((_, tu))) if unit == tu => format!("{n} of {t}"),
        _ => format!("{u} of {t}"),
    }
}

fn fraction(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| (part as f64 / whole as f64).clamp(0.0, 1.0))
}

fn row(label: &str, value: String, fraction: Option<f64>, tone: ResourceTone) -> ResourceRow {
    ResourceRow {
        label: label.into(),
        value,
        fraction,
        tone,
    }
}

/// The warning's sentence and button; none when the disk is fine.
pub fn warning_view(w: &DiskWarning) -> Option<WarningView> {
    let (text, critical) = match w.level {
        DiskLevel::Ok => return None,
        DiskLevel::Low => (LOW_TEXT, false),
        DiskLevel::Critical => (CRITICAL_TEXT, true),
    };
    Some(WarningView {
        text: text.into(),
        button: SEE_WHAT_USES_SPACE.into(),
        critical,
    })
}

fn level_tone(l: DiskLevel) -> ResourceTone {
    match l {
        DiskLevel::Ok => ResourceTone::Normal,
        DiskLevel::Low => ResourceTone::Warning,
        DiskLevel::Critical => ResourceTone::Critical,
    }
}

/// The Processor, Memory and disk rows of a computer.
fn computer_rows(c: &ComputerResources) -> Vec<ResourceRow> {
    let mut rows = Vec::new();
    if c.cpu_count > 0 {
        rows.push(row(
            PROCESSOR,
            processor_text(c.cpu_count, Some(c.cpu_percent)),
            Some((c.cpu_percent / 100.0).clamp(0.0, 1.0)),
            ResourceTone::Normal,
        ));
    }
    if c.memory_total > 0 {
        rows.push(row(
            MEMORY,
            format!("{} in use", of_text(c.memory_used, c.memory_total)),
            fraction(c.memory_used, c.memory_total),
            ResourceTone::Normal,
        ));
    }
    if c.disk_total > 0 {
        let free = c.disk_free.min(c.disk_total);
        rows.push(row(
            FILES,
            format!(
                "{} free of {}",
                format_size(free),
                format_size(c.disk_total)
            ),
            fraction(c.disk_total - free, c.disk_total),
            level_tone(c.warning.level),
        ));
    }
    rows
}

/// "Your extra Spaces use: 6 cores, 12 GB of memory, about 25 GB for
/// files."
fn extra_spaces_text(c: &ComputerResources, whose: &str) -> Option<String> {
    if c.spaces_count == 0 {
        return None;
    }
    let cores = format!(
        "{} core{}",
        c.spaces_cpu_allocated,
        if c.spaces_cpu_allocated == 1 { "" } else { "s" }
    );
    let files = if c.spaces_disk_exact {
        format!("about {}", format_size(c.spaces_disk_bytes))
    } else {
        format!("up to about {}", format_size(c.spaces_disk_bytes))
    };
    Some(format!(
        "{whose} extra Spaces use: {cores}, {} of memory, {files} for files.",
        format_size(c.spaces_memory_allocated),
    ))
}

/// This Mac's panel.
pub fn this_mac_view(c: &ComputerResources) -> ThisMacView {
    ThisMacView {
        title: "This Mac".into(),
        rows: computer_rows(c),
        extra_spaces: extra_spaces_text(c, "Your"),
        warning: warning_view(&c.warning),
    }
}

/// What a Space costs this Mac, in words (the same for a running and a
/// stopped Space).
fn on_this_mac_text(d: &DiskOnHost) -> Option<String> {
    match (d.bytes, d.shares_space) {
        (Some(b), true) => Some(format!(
            "Uses about {} on this Mac (shares space with other copies)",
            format_size(b)
        )),
        (Some(b), false) => Some(format!("Uses about {} on this Mac", format_size(b))),
        (None, true) => Some("Shares space with other copies".into()),
        (None, false) => None,
    }
}

/// One extra Space's panel: what it was given and what it uses now, and
/// what it costs this Mac.
pub fn space_view(s: &SpaceResources) -> SpaceResourcesView {
    let off = matches!(s.power_state.as_str(), "stopped" | "suspended");
    let live = s.usage.is_some() && !off;
    let usage = s.usage.filter(|_| live);
    let mut rows = Vec::new();

    let cores = if s.cpu_allocated > 0 {
        s.cpu_allocated
    } else {
        usage.map(|u| u.cpu_count).unwrap_or(0)
    };
    if cores > 0 {
        rows.push(row(
            PROCESSOR,
            processor_text(cores, usage.map(|u| u.cpu_percent)),
            usage.map(|u| (u.cpu_percent / 100.0).clamp(0.0, 1.0)),
            ResourceTone::Normal,
        ));
    }
    let memory = if s.memory_allocated > 0 {
        s.memory_allocated
    } else {
        usage
            .filter(|u| u.memory_limited)
            .map(|u| u.memory_total)
            .unwrap_or(0)
    };
    if memory > 0 {
        rows.push(match usage {
            Some(u) => row(
                MEMORY,
                of_text(u.memory_used, memory),
                fraction(u.memory_used, memory),
                ResourceTone::Normal,
            ),
            None => row(MEMORY, format_size(memory), None, ResourceTone::Normal),
        });
    }
    // Space for files: what the Space itself holds on this Mac (never the
    // base image it shares) of the size it was given. A file system with
    // no size of its own has no total: only what it holds.
    let disk = known_size(s.disk_allocated).or_else(|| {
        usage
            .filter(|u| u.disk_limited)
            .and_then(|u| u.disk_total)
            .and_then(known_size)
    });
    let own = s.disk_on_host.and_then(|d| d.bytes).or_else(|| {
        // Not measured on this Mac: what the guest says it holds, only when
        // that is its own data alone (an overlay with no size of its own; a
        // VM's guest counts its base system too).
        usage
            .filter(|u| !u.disk_limited && u.disk_total.is_none())
            .map(|u| u.disk_used)
            .filter(|b| *b < SIZE_SENTINEL_FLOOR)
    });
    match (own, disk) {
        (Some(b), Some(d)) => rows.push(row(
            FILES,
            of_text(b, d),
            fraction(b, d),
            ResourceTone::Normal,
        )),
        (Some(b), None) => rows.push(row(FILES, format_size(b), None, ResourceTone::Normal)),
        (None, Some(d)) => rows.push(row(
            FILES,
            format!("up to {}", format_size(d)),
            None,
            ResourceTone::Normal,
        )),
        (None, None) => {}
    }

    let on_this_mac = s.disk_on_host.as_ref().and_then(on_this_mac_text);
    let off_line = off.then(|| {
        let word = if s.power_state == "suspended" {
            "Suspended"
        } else {
            "Stopped"
        };
        match s.disk_on_host.as_ref().and_then(on_this_mac_text) {
            Some(t) => format!("{word} \u{b7} {}", lowercase_first(&t)),
            None => word.to_string(),
        }
    });
    SpaceResourcesView {
        id: s.id.clone(),
        rows,
        on_this_mac,
        off_line,
        live,
    }
}

/// What one piece of "How it fits on this Mac" is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiskFitKind {
    /// Everything on the volume that is not a Space: your own files and apps.
    Yours,
    /// This Space's own data.
    ThisSpace,
    /// Every other Space's own data, together.
    OtherSpaces,
    /// A base image Spaces share, counted once.
    Base,
    /// Free space.
    Free,
}

/// One piece of the bar and its legend entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskFitPart {
    /// Stable key (`yours`, `space`, `others`, `free`, a base's key).
    pub key: String,
    /// What it is.
    pub kind: DiskFitKind,
    /// "Your own files and apps", the Space's name, "Ubuntu 24.04 image".
    pub label: String,
    /// Its size in words ("4.4 GB").
    pub value: String,
    /// More words, for a base: "shared by 3 Spaces", "only this Space uses
    /// it".
    pub detail: Option<String>,
    /// Bytes, for the bar.
    pub bytes: u64,
    /// Its share of the bar, 0 to 1; the parts add up to 1.
    pub fraction: f64,
}

/// "How it fits on this Mac": the volume's pieces, each counted once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskFitView {
    /// The volume's size in words ("2 TB").
    pub total: String,
    /// Your own files and apps, this Space, other Spaces, shared bases,
    /// free: they add up to the volume.
    pub parts: Vec<DiskFitPart>,
    /// What could not be told apart, in plain words; none when everything
    /// was measured.
    pub note: Option<String>,
    /// The low-disk warning, when it is true.
    pub warning: Option<WarningView>,
}

/// An image reference as a name: `ghcr.io/trycua/linux:24.04` is
/// "linux 24.04".
pub fn image_name(reference: &str) -> String {
    let r = reference.split('@').next().unwrap_or(reference);
    let last = r.rsplit('/').next().unwrap_or(r);
    last.replacen(':', " ", 1)
}

/// "How it fits on this Mac" for Space `this` (named `name`) among the
/// computer's `spaces` (this one included or not). `base_name` names this
/// Space's base ("Ubuntu 24.04"); other bases are named from their image.
pub fn disk_fit(
    host: &ComputerResources,
    this: &SpaceResources,
    name: &str,
    spaces: &[SpaceResources],
    base_name: Option<&str>,
) -> DiskFitView {
    let total = host.disk_total;
    let free = host.disk_free.min(total);
    let used = total - free;
    let own = |s: &SpaceResources| s.disk_on_host.and_then(|d| d.bytes);
    let this_own = own(this);
    let others: u64 = spaces
        .iter()
        .filter(|s| s.id != this.id)
        .filter_map(own)
        .sum();
    // Each base once, in the order the Spaces come (this Space's first).
    let mut bases: Vec<&SharedBase> = Vec::new();
    for b in std::iter::once(this)
        .chain(spaces.iter().filter(|s| s.id != this.id))
        .filter_map(|s| s.base.as_ref())
    {
        if !bases.iter().any(|o| o.key == b.key) {
            bases.push(b);
        }
    }
    let known_bases: u64 = bases.iter().filter_map(|b| b.bytes).sum();
    let counted = this_own
        .unwrap_or(0)
        .saturating_add(others)
        .saturating_add(known_bases);
    let yours = used.saturating_sub(counted);
    // Measurements a little off never make the parts overflow the bar.
    let whole = total
        .max(counted.saturating_add(yours).saturating_add(free))
        .max(1);
    let part = |key: &str, kind, label: String, detail: Option<String>, bytes: u64| DiskFitPart {
        key: key.into(),
        kind,
        label,
        value: format_size(bytes),
        detail,
        bytes,
        fraction: bytes as f64 / whole as f64,
    };
    let mut parts = vec![part(
        "yours",
        DiskFitKind::Yours,
        "Your own files and apps".into(),
        None,
        yours,
    )];
    if let Some(b) = this_own {
        parts.push(part("space", DiskFitKind::ThisSpace, name.into(), None, b));
    }
    if others > 0 {
        parts.push(part(
            "others",
            DiskFitKind::OtherSpaces,
            "Other Spaces".into(),
            None,
            others,
        ));
    }
    let mut note = this_own
        .is_none()
        .then(|| format!("Can't tell {name}'s own data apart from the image it shares."));
    for b in &bases {
        let label = match base_name.filter(|_| this.base.as_ref().is_some_and(|t| t.key == b.key)) {
            Some(n) => format!("{n} image"),
            None => format!("{} image", image_name(&b.image)),
        };
        let mine = this.base.as_ref().is_some_and(|t| t.key == b.key);
        let detail = match (b.shared_with, mine) {
            (0 | 1, true) => "only this Space uses it".to_string(),
            (n, true) => format!("shared by {n} Spaces"),
            (n, false) => format!("used by {n} Space{}", if n == 1 { "" } else { "s" }),
        };
        match b.bytes {
            Some(bytes) => parts.push(part(&b.key, DiskFitKind::Base, label, Some(detail), bytes)),
            None if mine && note.is_none() => {
                note = Some(format!("Can't tell how much the shared {label} takes."));
            }
            None => {}
        }
    }
    parts.push(part("free", DiskFitKind::Free, "Free".into(), None, free));
    DiskFitView {
        total: format_size(total),
        parts,
        note,
        warning: warning_view(&host.warning),
    }
}

/// One card of a Space's resources dialog: the label, the main number,
/// the one "Given ..." line and the bar. No explanatory sentences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacePanelCard {
    /// "Processor", "Memory" or "Space for files".
    pub label: String,
    /// The main number: "13% busy", "717 MB of 4 GB", "244 KB".
    pub value: String,
    /// "Given 2 of this Mac's 18 cores"; none when there is nothing given
    /// to say.
    pub given: Option<String>,
    /// The bar, 0 to 1; none for a card with nothing to fill it against.
    pub fraction: Option<f64>,
}

/// A Space's resources dialog: the three cards and how it fits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacePanelView {
    /// Processor, Memory, Space for files.
    pub cards: Vec<SpacePanelCard>,
    /// "How it fits on this Mac"; none without the computer's numbers.
    pub fit: Option<DiskFitView>,
}

/// What a Space's resources dialog reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpacePanelInput {
    /// The Space's numbers.
    pub space: SpaceResources,
    /// Its name ("pipbox").
    pub name: String,
    /// Its computer's numbers, when it sent them.
    pub host: Option<ComputerResources>,
    /// "this Mac", or the other computer's name.
    pub computer: String,
    /// Every Space on that computer, for how it fits.
    pub spaces: Vec<SpaceResources>,
    /// The name of its base image's system ("Ubuntu 24.04"), when known.
    pub base_name: Option<String>,
}

/// "this Mac's" / "Studio's".
fn whose(computer: &str) -> String {
    format!(
        "{}\u{2019}s",
        if computer.is_empty() {
            "this Mac"
        } else {
            computer
        }
    )
}

/// A Space's resources dialog.
pub fn space_panel(i: &SpacePanelInput) -> SpacePanelView {
    let s = &i.space;
    let usage = s.usage.filter(|_| s.power_state == "running");
    let host = i.host.as_ref();
    let whose = whose(&i.computer);
    let cores = |n: u32| format!("{n} core{}", if n == 1 { "" } else { "s" });

    let cpu_given = (s.cpu_allocated > 0).then(|| match host.filter(|h| h.cpu_count > 0) {
        Some(h) => format!(
            "Given {} of {whose} {}",
            s.cpu_allocated,
            cores(h.cpu_count)
        ),
        None => format!("Given {}", cores(s.cpu_allocated)),
    });
    let processor = SpacePanelCard {
        label: PROCESSOR.into(),
        value: match usage {
            Some(u) => format!("{}% busy", u.cpu_percent.round().clamp(0.0, 100.0)),
            None => "Stopped".into(),
        },
        given: cpu_given,
        fraction: Some(
            usage
                .map(|u| (u.cpu_percent / 100.0).clamp(0.0, 1.0))
                .unwrap_or(0.0),
        ),
    };

    let mem_total = if s.memory_allocated > 0 {
        s.memory_allocated
    } else {
        usage
            .filter(|u| u.memory_limited)
            .map(|u| u.memory_total)
            .unwrap_or(0)
    };
    let mem_given = (s.memory_allocated > 0).then(|| match host.filter(|h| h.memory_total > 0) {
        Some(h) => format!(
            "Given {} of {whose} {}",
            format_size(s.memory_allocated),
            format_size(h.memory_total)
        ),
        None => format!("Given {}", format_size(s.memory_allocated)),
    });
    let memory = SpacePanelCard {
        label: MEMORY.into(),
        value: match usage {
            Some(u) if mem_total > 0 => of_text(u.memory_used, mem_total),
            Some(u) => format_size(u.memory_used),
            None => "Off".into(),
        },
        given: mem_given,
        fraction: Some(
            usage
                .and_then(|u| fraction(u.memory_used, mem_total))
                .unwrap_or(0.0),
        ),
    };

    // Space for files: what the Space itself holds on its computer (never
    // the base image it shares, which "How it fits" counts once), of the
    // size it was given. With no size of its own, only what it holds.
    let size = known_size(s.disk_allocated).or_else(|| {
        usage
            .filter(|u| u.disk_limited)
            .and_then(|u| u.disk_total)
            .and_then(known_size)
    });
    let own = s.disk_on_host.and_then(|d| d.bytes).or_else(|| {
        usage
            .filter(|u| !u.disk_limited && u.disk_total.is_none())
            .map(|u| u.disk_used)
            .filter(|b| *b < SIZE_SENTINEL_FLOOR)
    });
    let files = SpacePanelCard {
        label: FILES.into(),
        value: match (own, size) {
            (Some(b), Some(d)) => of_text(b, d),
            (Some(b), None) => format_size(b),
            (None, Some(d)) => format!("Up to {}", format_size(d)),
            (None, None) => "\u{2014}".into(),
        },
        // The size it was given is already in the main number.
        given: None,
        fraction: match (own, size) {
            (Some(b), Some(d)) => fraction(b, d),
            _ => None,
        },
    };

    SpacePanelView {
        cards: vec![processor, memory, files],
        fit: host
            .filter(|h| h.disk_total > 0)
            .map(|h| disk_fit(h, s, &i.name, &i.spaces, i.base_name.as_deref())),
    }
}

fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// "just now", "5 min ago", "2 h ago", "3 d ago".
pub fn age_text(sampled_at_ms: u64, now_ms: u64) -> String {
    let secs = now_ms.saturating_sub(sampled_at_ms) / 1000;
    match secs {
        0..=89 => "just now".into(),
        90..=3_599 => format!("{} min ago", (secs + 30) / 60),
        3_600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// Another of your computers' panel. `now_ms` is the shell's clock.
pub fn computer_view(c: &ComputerResources, now_ms: u64) -> ComputerView {
    let age = match (c.online, age_text(c.sampled_at_ms, now_ms)) {
        (true, a) if a == "just now" => None,
        (true, a) => Some(format!("Updated {a}")),
        (false, a) => Some(format!("Offline, as of {a}")),
    }
    .filter(|_| c.sampled_at_ms > 0 || !c.online);
    ComputerView {
        id: c.id.clone(),
        name: c.name.clone(),
        online: c.online,
        age,
        rows: computer_rows(c),
        extra_spaces: extra_spaces_text(c, "Its"),
        warning: warning_view(&c.warning),
    }
}

/// Everything the apps draw from one snapshot.
pub fn view(s: &ResourcesSnapshot, now_ms: u64) -> ResourcesView {
    ResourcesView {
        this_mac: this_mac_view(&s.this_mac),
        spaces: s.spaces.iter().map(space_view).collect(),
        computers: s
            .computers
            .iter()
            .map(|c| computer_view(c, now_ms))
            .collect(),
        warning: warning_view(&s.warning),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;
    const TB: u64 = 1024 * GIB;

    fn mac() -> ComputerResources {
        ComputerResources {
            id: "this-mac".into(),
            online: true,
            sampled_at_ms: 1_000_000,
            cpu_count: 12,
            cpu_percent: 38.0,
            memory_total: 64 * GIB,
            memory_used: 41 * GIB,
            disk_total: TB,
            disk_free: 41 * GIB,
            spaces_count: 3,
            spaces_running: 2,
            spaces_cpu_allocated: 6,
            spaces_memory_allocated: 12 * GIB,
            spaces_disk_bytes: 25 * GIB,
            spaces_disk_exact: true,
            warning: DiskWarning {
                level: DiskLevel::Low,
                free: 41 * GIB,
                total: TB,
                threshold: 100 * GIB,
            },
            ..Default::default()
        }
    }

    fn space() -> SpaceResources {
        SpaceResources {
            id: "local:aurora".into(),
            power_state: "running".into(),
            cpu_allocated: 4,
            memory_allocated: 8 * GIB,
            disk_allocated: 80 * GIB,
            usage: Some(GuestUsage {
                cpu_count: 4,
                cpu_percent: 22.0,
                memory_used: 3 * GIB + GIB / 10,
                memory_total: 8 * GIB,
                memory_limited: true,
                disk_used: 20 * GIB,
                disk_total: Some(80 * GIB),
                disk_limited: true,
                ..Default::default()
            }),
            disk_on_host: Some(DiskOnHost {
                bytes: Some(31 * GIB),
                allocated: 87 * GIB,
                shares_space: true,
            }),
            base: None,
        }
    }

    fn texts(rows: &[ResourceRow]) -> Vec<(String, String)> {
        rows.iter()
            .map(|r| (r.label.clone(), r.value.clone()))
            .collect()
    }

    #[test]
    fn this_mac_in_words_with_bars() {
        let v = this_mac_view(&mac());
        assert_eq!(v.title, "This Mac");
        assert_eq!(
            texts(&v.rows),
            [
                (
                    "Processor".to_string(),
                    "12 cores \u{b7} 38% busy".to_string()
                ),
                ("Memory".to_string(), "41 of 64 GB in use".to_string()),
                (
                    "Space for files".to_string(),
                    "41 GB free of 1 TB".to_string()
                ),
            ]
        );
        assert_eq!(v.rows[0].fraction, Some(0.38));
        assert_eq!(v.rows[1].fraction, Some(41.0 / 64.0));
        // The disk bar fills with what is used, in the warning's colour.
        assert!((v.rows[2].fraction.unwrap() - (1.0 - 41.0 / 1024.0)).abs() < 1e-9);
        assert_eq!(v.rows[2].tone, ResourceTone::Warning);
        assert_eq!(
            v.extra_spaces.as_deref(),
            Some("Your extra Spaces use: 6 cores, 12 GB of memory, about 25 GB for files.")
        );
    }

    #[test]
    fn the_warning_has_its_words_and_button() {
        let w = warning_view(&mac().warning).unwrap();
        assert_eq!(
            w.text,
            "This Mac is almost out of space. Delete old Spaces or snapshots."
        );
        assert_eq!(w.button, "See what's using space");
        assert!(!w.critical);
        let critical = DiskWarning {
            level: DiskLevel::Critical,
            ..mac().warning
        };
        let w = warning_view(&critical).unwrap();
        assert!(w.critical && w.text.contains("out of space"));
        assert_eq!(warning_view(&DiskWarning::default()), None);
        let mut ok = mac();
        ok.warning.level = DiskLevel::Ok;
        let v = this_mac_view(&ok);
        assert_eq!(v.warning, None);
        assert_eq!(v.rows[2].tone, ResourceTone::Normal);
    }

    #[test]
    fn a_running_space_shows_what_it_uses_and_what_it_costs() {
        let v = space_view(&space());
        assert!(v.live);
        assert_eq!(
            texts(&v.rows),
            [
                (
                    "Processor".to_string(),
                    "4 cores \u{b7} 22% busy".to_string()
                ),
                ("Memory".to_string(), "3.1 of 8 GB".to_string()),
                // Its own data on this Mac, of the size it was given.
                ("Space for files".to_string(), "31 of 80 GB".to_string()),
            ]
        );
        assert_eq!(v.rows[0].fraction, Some(0.22));
        assert_eq!(v.rows[2].fraction, Some(31.0 / 80.0));
        assert_eq!(
            v.on_this_mac.as_deref(),
            Some("Uses about 31 GB on this Mac (shares space with other copies)")
        );
        assert_eq!(v.off_line, None);
    }

    #[test]
    fn a_stopped_space_shows_its_allocation_and_disk_only() {
        let s = SpaceResources {
            power_state: "stopped".into(),
            usage: None,
            disk_on_host: Some(DiskOnHost {
                bytes: Some(6 * GIB),
                allocated: 6 * GIB,
                shares_space: false,
            }),
            ..space()
        };
        let v = space_view(&s);
        assert!(!v.live);
        assert_eq!(
            texts(&v.rows),
            [
                ("Processor".to_string(), "4 cores".to_string()),
                ("Memory".to_string(), "8 GB".to_string()),
                ("Space for files".to_string(), "6 of 80 GB".to_string()),
            ]
        );
        assert!(v.rows[..2].iter().all(|r| r.fraction.is_none()));
        assert_eq!(v.rows[2].fraction, Some(6.0 / 80.0));
        assert_eq!(
            v.on_this_mac.as_deref(),
            Some("Uses about 6 GB on this Mac")
        );
        assert_eq!(
            v.off_line.as_deref(),
            Some("Stopped \u{b7} uses about 6 GB on this Mac")
        );
        let suspended = SpaceResources {
            power_state: "suspended".into(),
            ..s.clone()
        };
        assert!(
            space_view(&suspended)
                .off_line
                .unwrap()
                .starts_with("Suspended")
        );
    }

    #[test]
    fn a_disk_that_could_not_be_measured_says_it_shares_space() {
        let s = SpaceResources {
            disk_on_host: Some(DiskOnHost {
                bytes: None,
                allocated: 12 * GIB,
                shares_space: true,
            }),
            ..space()
        };
        assert_eq!(
            space_view(&s).on_this_mac.as_deref(),
            Some("Shares space with other copies")
        );
        // A container has nothing to say about its disk.
        let c = SpaceResources {
            disk_on_host: None,
            ..space()
        };
        assert_eq!(space_view(&c).on_this_mac, None);
    }

    #[test]
    fn a_space_with_nothing_known_has_no_rows() {
        let v = space_view(&SpaceResources {
            id: "local:x".into(),
            ..Default::default()
        });
        assert!(v.rows.is_empty() && !v.live);
        assert_eq!((v.on_this_mac, v.off_line), (None, None));
    }

    #[test]
    fn a_guest_that_reports_its_own_limits_fills_in_what_was_not_recorded() {
        let s = SpaceResources {
            cpu_allocated: 0,
            memory_allocated: 0,
            disk_allocated: 0,
            ..space()
        };
        let v = space_view(&s);
        assert_eq!(v.rows[0].value, "4 cores \u{b7} 22% busy");
        assert_eq!(v.rows[1].value, "3.1 of 8 GB");
        assert_eq!(v.rows[2].value, "31 of 80 GB");
    }

    #[test]
    fn another_computer_says_how_old_its_numbers_are() {
        let c = ComputerResources {
            id: "m1".into(),
            name: "Mac mini".into(),
            online: true,
            sampled_at_ms: 1_000_000,
            ..mac()
        };
        let at = |secs: u64| computer_view(&c, 1_000_000 + secs * 1000);
        assert_eq!(at(10).age, None);
        assert_eq!(at(120).age.as_deref(), Some("Updated 2 min ago"));
        assert_eq!(at(7_300).age.as_deref(), Some("Updated 2 h ago"));
        let off = ComputerResources {
            online: false,
            ..c.clone()
        };
        assert_eq!(
            computer_view(&off, 1_000_000 + 300_000).age.as_deref(),
            Some("Offline, as of 5 min ago")
        );
        let v = at(10);
        assert_eq!(v.name, "Mac mini");
        assert_eq!(v.rows.len(), 3);
        assert!(v.extra_spaces.unwrap().starts_with("Its extra Spaces use"));
    }

    #[test]
    fn the_snapshot_reads_the_runtimes_json() {
        let json = serde_json::json!({
            "sequence": 3, "sampledAtMs": 5,
            "thisMac": {"id": "this-mac", "online": true, "cpuCount": 12, "cpuPercent": 38.0,
                        "memoryTotal": 68719476736u64, "memoryUsed": 44023414784u64,
                        "diskTotal": 1099511627776u64, "diskFree": 44023414784u64,
                        "warning": {"level": "low", "free": 44023414784u64,
                                    "total": 1099511627776u64, "threshold": 107374182400u64}},
            "spaces": [{"id": "local:a", "powerState": "running", "cpuAllocated": 2,
                        "usage": {"cpuCount": 2, "cpuPercent": 10.0,
                                  "memoryUsed": 1073741824u64, "memoryTotal": 2147483648u64,
                                  "memoryLimited": true},
                        "diskOnHost": {"bytes": null, "allocated": 1, "sharesSpace": true}}],
            "computers": [],
            "warning": {"level": "low", "free": 44023414784u64,
                        "total": 1099511627776u64, "threshold": 107374182400u64}
        });
        let s: ResourcesSnapshot = serde_json::from_value(json).unwrap();
        let v = view(&s, 5);
        assert_eq!(v.this_mac.rows[0].value, "12 cores \u{b7} 38% busy");
        assert!(v.warning.is_some());
        assert_eq!(v.spaces[0].rows[1].value, "1 of 2 GB");
        assert_eq!(
            v.spaces[0].on_this_mac.as_deref(),
            Some("Shares space with other copies")
        );
        // Older shells leave fields out: every one has a default.
        let bare: ResourcesSnapshot = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(view(&bare, 0).spaces.is_empty());
    }

    #[test]
    fn the_copy_has_no_jargon() {
        let v = view(
            &ResourcesSnapshot {
                this_mac: mac(),
                spaces: vec![space()],
                computers: vec![mac()],
                warning: mac().warning,
                ..Default::default()
            },
            2_000_000,
        );
        let text = serde_json::to_string(&v).unwrap().to_lowercase();
        for jargon in [
            "vcpu", "rss", "apfs", "sparse", "qcow", "cgroup", "clone", "host",
        ] {
            assert!(!text.contains(jargon), "{jargon} in {text}");
        }
    }

    /// A running Ubuntu container under gVisor: no size of its own.
    fn container(json_disk_total: &str) -> SpaceResources {
        serde_json::from_str(&format!(
            r#"{{"id":"local:pipbox","powerState":"running","cpuAllocated":2,"memoryAllocated":{mem},
                "diskAllocated":0,
                "usage":{{"cpuCount":2,"cpuPercent":13,"memoryUsed":751619276,"memoryTotal":{mem},
                         "memoryLimited":true,"diskUsed":249856,"diskTotal":{json_disk_total},"diskLimited":false}},
                "diskOnHost":{{"bytes":249856,"allocated":249856,"sharesSpace":false}},
                "base":{{"key":"sha256:9ac0","image":"ghcr.io/trycua/linux:24.04","bytes":4410000000,"sharedWith":1}}}}"#,
            mem = 4 * GIB
        ))
        .unwrap()
    }

    #[test]
    fn a_sentinel_size_never_reaches_a_row() {
        for sentinel in [
            "9223372036854775807",  // i64::MAX
            "9223372036854771712",  // gVisor: i64::MAX to a page
            "18446744073709551615", // u64::MAX
            "-1",
            "0",
            "null",
        ] {
            let s = container(sentinel);
            assert_eq!(s.usage.unwrap().disk_total, None, "{sentinel}");
            let v = space_view(&s);
            let files = v.rows.iter().find(|r| r.label == FILES).unwrap();
            // Only what it holds: no fake total, never "0 GB".
            assert_eq!(files.value, "244 KB", "{sentinel}");
            assert_eq!(files.fraction, None);
            for r in &v.rows {
                assert!(!r.value.contains("PB") && !r.value.contains("TB"), "{r:?}");
            }
        }
        assert_eq!(known_size(i64::MAX as u64), None);
        assert_eq!(known_size(u64::MAX), None);
        assert_eq!(known_size(0), None);
        assert_eq!(known_size(80 * GIB), Some(80 * GIB));
    }

    #[test]
    fn memory_reads_the_way_macos_shows_it() {
        // Given 4 GiB of a 36 GiB Mac: "4 GB" and "36 GB", never 4.3 and 39.
        let s = container("0");
        let v = space_view(&s);
        assert_eq!(
            v.rows.iter().find(|r| r.label == MEMORY).unwrap().value,
            "717 MB of 4 GB"
        );
        assert_eq!(format_size(36 * GIB), "36 GB");
        assert_eq!(format_size(500 * (1 << 20)), "500 MB");
    }

    fn host(free: u64) -> ComputerResources {
        ComputerResources {
            disk_total: 2 * TB,
            disk_free: free,
            ..mac()
        }
    }

    fn total(v: &DiskFitView) -> f64 {
        v.parts.iter().map(|p| p.fraction).sum()
    }

    #[test]
    fn the_fit_counts_a_copy_on_write_base_once() {
        let this = container("0");
        let twin = SpaceResources {
            id: "local:twin".into(),
            disk_on_host: Some(DiskOnHost {
                bytes: Some(2 * GIB),
                allocated: 2 * GIB,
                shares_space: false,
            }),
            base: this.base.clone().map(|b| SharedBase {
                shared_with: 2,
                ..b
            }),
            ..this.clone()
        };
        let this = SpaceResources {
            base: twin.base.clone(),
            ..this
        };
        let free = 6 * GIB;
        let v = disk_fit(
            &host(free),
            &this,
            "pipbox",
            &[this.clone(), twin],
            Some("Ubuntu 24.04"),
        );
        let kinds: Vec<_> = v.parts.iter().map(|p| (p.kind, p.label.as_str())).collect();
        assert_eq!(
            kinds,
            [
                (DiskFitKind::Yours, "Your own files and apps"),
                (DiskFitKind::ThisSpace, "pipbox"),
                (DiskFitKind::OtherSpaces, "Other Spaces"),
                (DiskFitKind::Base, "Ubuntu 24.04 image"),
                (DiskFitKind::Free, "Free"),
            ]
        );
        let base = &v.parts[3];
        assert_eq!(base.detail.as_deref(), Some("shared by 2 Spaces"));
        assert_eq!(base.bytes, 4_410_000_000);
        assert_eq!(v.parts[1].value, "244 KB");
        // Used + this Space + others + the base once + free = the volume.
        let sum: u64 = v.parts.iter().map(|p| p.bytes).sum();
        assert_eq!(sum, 2 * TB);
        assert!((total(&v) - 1.0).abs() < 1e-9);
        assert_eq!(v.total, "2 TB");
        assert_eq!(v.note, None);
        assert!(v.warning.is_some(), "this Mac is low: the warning stays");
    }

    #[test]
    fn the_last_user_of_a_base_says_so() {
        let this = container("0");
        let v = disk_fit(
            &host(100 * GIB),
            &this,
            "pipbox",
            std::slice::from_ref(&this),
            None,
        );
        let base = v
            .parts
            .iter()
            .find(|p| p.kind == DiskFitKind::Base)
            .unwrap();
        assert_eq!(base.label, "linux 24.04 image");
        assert_eq!(base.detail.as_deref(), Some("only this Space uses it"));
        assert!(v.parts.iter().all(|p| p.kind != DiskFitKind::OtherSpaces));
    }

    #[test]
    fn a_space_with_no_base_is_its_own_data_only() {
        let this = SpaceResources {
            base: None,
            ..space()
        };
        let v = disk_fit(
            &host(100 * GIB),
            &this,
            "aurora",
            std::slice::from_ref(&this),
            None,
        );
        assert!(v.parts.iter().all(|p| p.kind != DiskFitKind::Base));
        assert_eq!(v.parts[1].bytes, 31 * GIB);
        let sum: u64 = v.parts.iter().map(|p| p.bytes).sum();
        assert_eq!(sum, 2 * TB);
    }

    #[test]
    fn what_cannot_be_told_apart_is_said_plainly() {
        let this = SpaceResources {
            disk_on_host: Some(DiskOnHost {
                bytes: None,
                allocated: 40 * GIB,
                shares_space: true,
            }),
            base: Some(SharedBase {
                key: "macos".into(),
                image: "ghcr.io/trycua/macos:26".into(),
                bytes: None,
                shared_with: 1,
            }),
            ..space()
        };
        let v = disk_fit(
            &host(100 * GIB),
            &this,
            "aurora",
            std::slice::from_ref(&this),
            None,
        );
        assert_eq!(
            v.note.as_deref(),
            Some("Can't tell aurora's own data apart from the image it shares.")
        );
        // Nothing guessed: no part for it.
        assert!(
            v.parts
                .iter()
                .all(|p| p.kind != DiskFitKind::ThisSpace && p.kind != DiskFitKind::Base)
        );
        let base_only = SpaceResources {
            disk_on_host: Some(DiskOnHost {
                bytes: Some(GIB),
                allocated: 40 * GIB,
                shares_space: true,
            }),
            ..this
        };
        let v = disk_fit(
            &host(100 * GIB),
            &base_only,
            "aurora",
            std::slice::from_ref(&base_only),
            None,
        );
        assert_eq!(
            v.note.as_deref(),
            Some("Can't tell how much the shared macos 26 image takes.")
        );
    }

    #[test]
    fn measurements_a_little_off_never_overflow_the_bar() {
        let mut this = container("0");
        this.disk_on_host = Some(DiskOnHost {
            bytes: Some(3 * TB),
            allocated: 3 * TB,
            shares_space: false,
        });
        let v = disk_fit(
            &host(GIB),
            &this,
            "pipbox",
            std::slice::from_ref(&this),
            None,
        );
        assert_eq!(v.parts[0].bytes, 0);
        assert!((total(&v) - 1.0).abs() < 1e-9);
    }

    /// The owner's Mac: 18 cores, 36 GiB, a 2 TB disk with 5.6 GB free.
    fn owners_mac() -> ComputerResources {
        ComputerResources {
            cpu_count: 18,
            memory_total: 36 * GIB,
            disk_total: 2_000_000_000_000,
            disk_free: 5_600_000_000,
            warning: DiskWarning {
                level: DiskLevel::Critical,
                free: 5_600_000_000,
                total: 2_000_000_000_000,
                threshold: 5 * GIB,
            },
            ..mac()
        }
    }

    fn panel(s: SpaceResources) -> SpacePanelView {
        space_panel(&SpacePanelInput {
            spaces: vec![s.clone()],
            space: s,
            name: "pipbox".into(),
            host: Some(owners_mac()),
            computer: "this Mac".into(),
            base_name: Some("Ubuntu 24.04".into()),
        })
    }

    fn cards(v: &SpacePanelView) -> Vec<(String, String, Option<String>)> {
        v.cards
            .iter()
            .map(|c| (c.label.clone(), c.value.clone(), c.given.clone()))
            .collect()
    }

    #[test]
    fn the_resources_dialog_of_a_gvisor_container() {
        for sentinel in [
            "9223372036854771712",
            "9223372036854775807",
            "18446744073709551615",
            "-1",
            "0",
        ] {
            let v = panel(container(sentinel));
            assert_eq!(
                cards(&v),
                [
                    (
                        "Processor".to_string(),
                        "13% busy".to_string(),
                        Some("Given 2 of this Mac\u{2019}s 18 cores".to_string())
                    ),
                    (
                        "Memory".to_string(),
                        "717 MB of 4 GB".to_string(),
                        // GiB both sides, labelled the way macOS does.
                        Some("Given 4 GB of this Mac\u{2019}s 36 GB".to_string())
                    ),
                    // Its own data, no fake total, never "0 GB".
                    ("Space for files".to_string(), "244 KB".to_string(), None),
                ],
                "{sentinel}"
            );
            assert_eq!(v.cards[2].fraction, None);
            let fit = v.fit.unwrap();
            assert_eq!(fit.total, "1.8 TB");
            let legend: Vec<_> = fit
                .parts
                .iter()
                .map(|p| {
                    format!(
                        "{} \u{b7} {}{}",
                        p.label,
                        p.value,
                        p.detail
                            .as_ref()
                            .map(|d| format!(", {d}"))
                            .unwrap_or_default()
                    )
                })
                .collect();
            assert_eq!(
                legend,
                [
                    "Your own files and apps \u{b7} 1.8 TB",
                    "pipbox \u{b7} 244 KB",
                    "Ubuntu 24.04 image \u{b7} 4.1 GB, only this Space uses it",
                    "Free \u{b7} 5.2 GB",
                ]
            );
            let sum: u64 = fit.parts.iter().map(|p| p.bytes).sum();
            assert_eq!(sum, 2_000_000_000_000);
            // This Mac really is nearly full: the warning stays.
            assert!(fit.warning.unwrap().critical);
        }
    }

    #[test]
    fn the_resources_dialog_of_a_stopped_vm() {
        let s = SpaceResources {
            power_state: "stopped".into(),
            usage: None,
            ..space()
        };
        let v = space_panel(&SpacePanelInput {
            spaces: vec![s.clone()],
            space: s,
            name: "aurora".into(),
            host: None,
            computer: "this Mac".into(),
            base_name: None,
        });
        assert_eq!(
            cards(&v),
            [
                (
                    "Processor".to_string(),
                    "Stopped".to_string(),
                    Some("Given 4 cores".to_string())
                ),
                (
                    "Memory".to_string(),
                    "Off".to_string(),
                    Some("Given 8 GB".to_string())
                ),
                (
                    "Space for files".to_string(),
                    "31 of 80 GB".to_string(),
                    None
                ),
            ]
        );
        assert_eq!(v.fit, None);
    }

    #[test]
    fn the_dialog_never_says_more_than_the_numbers() {
        let v = panel(container("0"));
        let all = serde_json::to_string(&v).unwrap();
        for banned in [
            "Busy means",
            "free for its apps",
            "Uses about",
            "Updates every",
            "file names",
            "9223372",
        ] {
            assert!(!all.contains(banned), "{banned}: {all}");
        }
    }
}
