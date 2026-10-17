// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Machines page's list: the machines a shell found (this machine, the
//! account's machines on the relay, hosts added by address) and the
//! account's devices, with each computer listed once.
//!
//! A Mac set up with `cua host setup` is a relay machine ("gamma-4 Mac
//! Studio", its id a machine id) and, once the app signs in there, also an
//! enrolled device ("gamma-4.example.com", `dev_…`). The relay keeps no
//! link between the two, so [`merge`] matches them:
//!
//! 1. the same id;
//! 2. this device and this machine;
//! 3. the hostname the machine's cua-spacesd reported equals the device's
//!    name (devices are named after their hostname);
//! 4. the same name;
//! 5. the device's name is a hostname whose first label is the machine's
//!    first word (`gamma-4.example.com` and `gamma-4 Mac Studio`).
//!
//! Steps 3 to 5 merge only a pair that matches nobody else, on either
//! side, and whose systems agree: two Macs that share a name stay two
//! rows. The merged row keeps the machine's id (what `on="host:<id>"`
//! takes) and its name, and is online when the app reached it or the
//! relay sees it connected. A device that matches no machine is listed on
//! its own, marked `device_only`: it is not a host, so New Space's "Run
//! on" never offers it.
//!
//! [`page_rows`] is the whole Machines page from what a shell has: this
//! machine, the machines its Spaces list shows (a machine sharing its own
//! desktop, `relay:<machine>`, and a machine that provides Spaces), what
//! the relay says of each, and the account's devices, merged. Every UI
//! (the SwiftUI app, the web UI in the Mac app and in Electron) lists
//! these rows.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::host::THIS_MACHINE_ID;
use crate::model::{Space, SpaceOs, SpaceProvider, SpaceStatus};

/// A device whose last session is this recent counts as online.
pub const DEVICE_ONLINE_SECS: u64 = 15 * 60;

/// A machine a shell found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineIn {
    /// Machine id (`this-mac`, a relay machine id, a direct host's name).
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// The app reached it just now.
    #[serde(default)]
    pub online: bool,
    /// The relay sees it connected (`None`: the shell does not know).
    #[serde(default)]
    pub presence: Option<bool>,
    /// The hostname its cua-spacesd reported, when the app reached it.
    #[serde(default)]
    pub hostname: Option<String>,
    /// `macos`, `linux`, `windows`, or empty.
    #[serde(default)]
    pub os: String,
    /// The machine this app runs on.
    #[serde(default)]
    pub current: bool,
}

/// One of the account's devices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIn {
    /// `dev_…`.
    pub id: String,
    /// Its name (by default its hostname).
    #[serde(default)]
    pub name: String,
    /// `macos`, `windows`, `linux` (any case), or empty.
    #[serde(default)]
    pub platform: String,
    /// Last session (Unix seconds).
    #[serde(default)]
    pub last_seen: Option<u64>,
    /// This device.
    #[serde(default)]
    pub current: bool,
    /// `pending`, `enrolled`, `expired`, `revoked`, or empty.
    #[serde(default)]
    pub state: String,
}

/// What [`merge`] reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesInput {
    /// Machines, in the order the page lists them.
    #[serde(default)]
    pub machines: Vec<MachineIn>,
    /// The account's devices.
    #[serde(default)]
    pub devices: Vec<DeviceIn>,
    /// Unix seconds.
    #[serde(default)]
    pub now: u64,
}

/// One row of the Machines page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineOut {
    /// The machine's id (a device's id for a device of its own).
    pub id: String,
    /// The machine's name (the device's when the machine has none).
    pub name: String,
    /// Reached just now, connected to the relay, or (a device of its own)
    /// in a session in the last [`DEVICE_ONLINE_SECS`].
    pub online: bool,
    /// The device listed in this row, if any.
    pub device: Option<String>,
    /// A device that is none of the machines: listed, never a "Run on"
    /// choice.
    pub device_only: bool,
    /// The machine this app runs on.
    pub current: bool,
}

/// A hostname or name as compared: trimmed, lowercase, no trailing dot.
fn key(name: &str) -> String {
    name.trim().trim_end_matches('.').to_lowercase()
}

/// A name that is a hostname with a domain (`gamma-4.example.com`): its
/// first label.
fn host_label(name: &str) -> Option<String> {
    let k = key(name);
    if k.contains(char::is_whitespace) || !k.contains('.') {
        return None;
    }
    let first = k.split('.').next()?;
    (!first.is_empty()).then(|| first.to_string())
}

/// The first word of a display name ("gamma-4" of "gamma-4 Mac Studio").
fn first_word(name: &str) -> Option<String> {
    let k = key(name);
    let w = k.split_whitespace().next()?;
    (k.contains(char::is_whitespace) && !w.is_empty()).then(|| w.to_string())
}

/// A device's platform as a machine's `os`.
fn platform_os(platform: &str) -> String {
    match platform.trim().to_lowercase().as_str() {
        "darwin" | "mac" | "macos" => "macos".into(),
        other => other.to_string(),
    }
}

/// Their systems do not rule the pair out.
fn same_system(m: &MachineIn, d: &DeviceIn) -> bool {
    let os = platform_os(&d.platform);
    m.os.is_empty() || os.is_empty() || m.os.eq_ignore_ascii_case(&os)
}

/// Pairs `devices[d]` with `machines[m]` by `rule`, only where the pair is
/// the one match on both sides among the ones still open.
fn pair_unique(
    input: &MachinesInput,
    machine_of: &mut [Option<usize>],
    taken: &mut [bool],
    rule: impl Fn(&MachineIn, &DeviceIn) -> bool,
) {
    let open_machines: Vec<usize> = (0..input.machines.len())
        .filter(|&m| !taken[m] && !input.machines[m].current)
        .collect();
    let open_devices: Vec<usize> = (0..input.devices.len())
        .filter(|&d| machine_of[d].is_none() && !input.devices[d].current)
        .collect();
    let matches = |m: usize, d: usize| {
        let (mi, di) = (&input.machines[m], &input.devices[d]);
        same_system(mi, di) && rule(mi, di)
    };
    let mut pairs = Vec::new();
    for &d in &open_devices {
        let ms: Vec<usize> = open_machines
            .iter()
            .copied()
            .filter(|&m| matches(m, d))
            .collect();
        let [m] = ms[..] else { continue };
        if open_devices.iter().filter(|&&o| matches(m, o)).count() == 1 {
            pairs.push((m, d));
        }
    }
    for (m, d) in pairs {
        machine_of[d] = Some(m);
        taken[m] = true;
    }
}

/// The machines, each with the device that is the same computer, then the
/// devices that are none of them. Revoked devices are left out.
pub fn merge(input: &MachinesInput) -> Vec<MachineOut> {
    let n = input.machines.len();
    let mut machine_of: Vec<Option<usize>> = vec![None; input.devices.len()];
    let mut taken = vec![false; n];
    // 1. The same id.
    for (d, dev) in input.devices.iter().enumerate() {
        if let Some(m) = input.machines.iter().position(|m| m.id == dev.id)
            && !taken[m]
        {
            machine_of[d] = Some(m);
            taken[m] = true;
        }
    }
    // 2. This device is this machine.
    if let Some(m) = input.machines.iter().position(|m| m.current)
        && !taken[m]
        && let Some(d) = input
            .devices
            .iter()
            .enumerate()
            .find(|(d, dev)| dev.current && machine_of[*d].is_none())
            .map(|(d, _)| d)
    {
        machine_of[d] = Some(m);
        taken[m] = true;
    }
    // 3. The hostname it reported is the device's name.
    pair_unique(input, &mut machine_of, &mut taken, |m, d| {
        m.hostname
            .as_deref()
            .is_some_and(|h| !key(h).is_empty() && key(h) == key(&d.name))
    });
    // 4. The same name.
    pair_unique(input, &mut machine_of, &mut taken, |m, d| {
        !key(&m.name).is_empty() && key(&m.name) == key(&d.name)
    });
    // 5. A hostname device and a machine named after its first label.
    pair_unique(input, &mut machine_of, &mut taken, |m, d| {
        host_label(&d.name).is_some_and(|l| {
            first_word(&m.name).as_deref() == Some(l.as_str())
                || m.hostname.as_deref().and_then(host_label).as_deref() == Some(l.as_str())
        })
    });

    let mut out: Vec<MachineOut> = input
        .machines
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let device = input
                .devices
                .iter()
                .zip(&machine_of)
                .find(|(_, of)| **of == Some(i))
                .map(|(d, _)| d);
            MachineOut {
                id: m.id.clone(),
                name: if m.name.trim().is_empty() {
                    device.map_or_else(|| m.id.clone(), |d| d.name.clone())
                } else {
                    m.name.clone()
                },
                online: m.online || m.presence == Some(true),
                device: device.map(|d| d.id.clone()),
                device_only: false,
                current: m.current,
            }
        })
        .collect();
    for (d, of) in input.devices.iter().zip(&machine_of) {
        if of.is_some() || d.current || d.state == "revoked" {
            continue;
        }
        out.push(MachineOut {
            id: d.id.clone(),
            name: if d.name.trim().is_empty() {
                d.id.clone()
            } else {
                d.name.clone()
            },
            online: d
                .last_seen
                .is_some_and(|s| input.now.saturating_sub(s) <= DEVICE_ONLINE_SECS),
            device: Some(d.id.clone()),
            device_only: true,
            current: false,
        });
    }
    out
}

/// The machines New Space can create on (`on="host:<id>"`): every row but
/// this machine and the devices of their own.
pub fn run_on_ids(rows: &[MachineOut]) -> Vec<String> {
    rows.iter()
        .filter(|r| !r.current && !r.device_only)
        .map(|r| r.id.clone())
        .collect()
}

/// What [`page_rows`] reads: what a shell knows of its machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesPageInput {
    /// The system this app runs on (names the first row "This Mac").
    #[serde(default = "this_host_os")]
    pub host_os: SpaceOs,
    /// The Spaces list as the shell has it (registry rows as Spaces).
    #[serde(default)]
    pub spaces: Vec<Space>,
    /// The account's devices (none while signed out).
    #[serde(default)]
    pub devices: Vec<DeviceIn>,
    /// The hostname each machine's cua-spacesd reported, by machine id.
    #[serde(default)]
    pub hostnames: std::collections::HashMap<String, String>,
    /// Whether the relay sees each machine connected now, by machine id.
    #[serde(default)]
    pub presence: std::collections::HashMap<String, bool>,
    /// Unix seconds.
    #[serde(default)]
    pub now: u64,
}

fn this_host_os() -> SpaceOs {
    SpaceOs::Macos
}

impl Default for MachinesPageInput {
    fn default() -> Self {
        Self {
            host_os: this_host_os(),
            spaces: vec![],
            devices: vec![],
            hostnames: HashMap::new(),
            presence: HashMap::new(),
            now: 0,
        }
    }
}

/// One row of the Machines page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinePageRow {
    /// The machine's id (`this-mac`, a relay machine id), or the device's
    /// id for a device of its own.
    pub id: String,
    /// What the page calls it.
    pub name: String,
    /// Reached, connected to the relay, or (a device of its own) in a
    /// session in the last [`DEVICE_ONLINE_SECS`].
    pub online: bool,
    /// `macos`, `linux`, `windows`, or empty when unknown.
    pub os: String,
    /// `local` (this machine) or `relay`.
    pub via: String,
    /// The machine this app runs on.
    pub current: bool,
    /// The device listed in this row, if any.
    pub device: Option<String>,
    /// A device that is none of the machines: listed, never a "Run on"
    /// choice, and it has no Spaces or desktop of its own here.
    pub device_only: bool,
    /// The Spaces list's row that is this machine's own desktop
    /// (`this-mac`, `relay:<machine>`), when there is one: the classic
    /// UI's detail for it.
    pub space_id: Option<String>,
    /// Online, but its owner stopped sharing it.
    pub stopped_sharing: bool,
    /// The device's last session (Unix seconds), when it has a device.
    pub last_seen: Option<u64>,
    /// The row's status word, the same in every UI ([`status_word`]):
    /// "Online", "Not sharing" or "Offline".
    #[serde(default)]
    pub status: String,
    /// The Spaces it runs that the Spaces list shows: this machine's local
    /// Spaces, or the Spaces another machine provides (`host`).
    #[serde(default)]
    pub spaces: u32,
    /// "1 Space", "3 Spaces", "No Spaces" ([`spaces_label`]); empty for a
    /// device of its own, which runs no Spaces.
    #[serde(default)]
    pub spaces_label: String,
}

/// A machine that is reachable from here.
pub const ONLINE: &str = "Online";
/// A machine that is connected to the relay but whose owner stopped
/// sharing it (the relay's own word for it, and New Space's "(not
/// sharing)"): every call to it is refused.
pub const NOT_SHARING: &str = "Not sharing";
/// A machine the app did not reach and the relay does not see connected.
pub const OFFLINE: &str = "Offline";

/// A Machines row's status word. A machine is not a Space: it is never
/// "Suspended" or "Not reachable", only online, online but not sharing,
/// or offline.
pub fn status_word(online: bool, stopped_sharing: bool) -> &'static str {
    match (online, stopped_sharing) {
        (false, _) => OFFLINE,
        (true, true) => NOT_SHARING,
        (true, false) => ONLINE,
    }
}

/// "1 Space", "3 Spaces", "No Spaces".
pub fn spaces_label(n: u32) -> String {
    match n {
        0 => "No Spaces".into(),
        1 => "1 Space".into(),
        n => format!("{n} Spaces"),
    }
}

/// The Spaces on a machine: this machine's local Spaces, or those another
/// machine provides (the classic UI's `hosted_rows`). A machine's own
/// desktop is not one of them.
fn spaces_on(spaces: &[Space], id: &str, current: bool) -> u32 {
    let n = spaces
        .iter()
        .filter(|s| {
            if current {
                s.provider == Some(SpaceProvider::Local) && s.id != THIS_MACHINE_ID
            } else {
                s.host.as_deref() == Some(id)
            }
        })
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A Space's error says its machine refused because its owner stopped
/// sharing it (the web UI's `STOPPED_SHARING_ERROR`).
fn says_stopped_sharing(space: &Space) -> bool {
    space
        .sdk
        .as_ref()
        .and_then(|s| s.error.as_deref())
        .is_some_and(|e| e.to_lowercase().contains("stopped sharing"))
}

/// The machine whose own desktop `space` is (`relay:<machine>`, no host
/// above it), if it is one.
fn own_desktop(space: &Space) -> Option<&str> {
    if space.provider != Some(SpaceProvider::Relay) || space.host.is_some() {
        return None;
    }
    space
        .id
        .strip_prefix("relay:")
        .filter(|own| !own.is_empty() && !own.contains('/') && !own.starts_with("space-"))
}

/// A Space's system as a machine's `os`: empty when unknown, so it never
/// keeps a machine and its device apart.
fn os_word(os: SpaceOs) -> String {
    match os {
        SpaceOs::Unknown => String::new(),
        os => os.as_str().into(),
    }
}

/// The Machines page: this machine first, then the machines the Spaces
/// list shows (in its order), then the account's devices that are none
/// of them; each computer once ([`merge`]).
pub fn page_rows(input: &MachinesPageInput) -> Vec<MachinePageRow> {
    struct Extra {
        via: &'static str,
        space_id: Option<String>,
        stopped_sharing: bool,
    }
    let mut machines = vec![MachineIn {
        id: THIS_MACHINE_ID.into(),
        name: input.host_os.this_machine().into(),
        online: true,
        os: os_word(input.host_os),
        current: true,
        ..Default::default()
    }];
    let mut extra = vec![Extra {
        via: "local",
        space_id: input
            .spaces
            .iter()
            .find(|s| s.id == THIS_MACHINE_ID)
            .map(|s| s.id.clone()),
        stopped_sharing: false,
    }];
    for space in &input.spaces {
        if let Some(own) = own_desktop(space) {
            // A machine that refused because its owner stopped sharing it
            // is connected (that is how it refused), not offline.
            let stopped = says_stopped_sharing(space);
            let online =
                matches!(space.status, SpaceStatus::Running | SpaceStatus::Local) || stopped;
            let row = MachineIn {
                id: own.into(),
                name: space.name.clone(),
                online,
                os: os_word(space.os),
                ..Default::default()
            };
            let e = Extra {
                via: "relay",
                space_id: Some(space.id.clone()),
                stopped_sharing: stopped,
            };
            match machines.iter().position(|m| m.id == own) {
                Some(i) if !machines[i].current => {
                    machines[i] = row;
                    extra[i] = e;
                }
                Some(_) => {}
                None => {
                    machines.push(row);
                    extra.push(e);
                }
            }
            continue;
        }
        let Some(host) = space.host.as_deref().filter(|h| !h.is_empty()) else {
            continue;
        };
        if machines.iter().any(|m| m.id == host) {
            continue;
        }
        machines.push(MachineIn {
            id: host.into(),
            name: space.host_name.clone().unwrap_or_else(|| host.into()),
            online: true,
            ..Default::default()
        });
        extra.push(Extra {
            via: "relay",
            space_id: None,
            stopped_sharing: false,
        });
    }
    for m in machines.iter_mut() {
        if let Some(h) = input.hostnames.get(&m.id).filter(|h| !h.is_empty()) {
            m.hostname = Some(h.clone());
        }
        if !m.current {
            m.presence = input.presence.get(&m.id).copied();
        }
    }
    let merged = merge(&MachinesInput {
        machines: machines.clone(),
        devices: input.devices.clone(),
        now: input.now,
    });
    merged
        .into_iter()
        .map(|r| {
            let device = r
                .device
                .as_deref()
                .and_then(|d| input.devices.iter().find(|x| x.id == d));
            let last_seen = device.and_then(|d| d.last_seen);
            if r.device_only {
                return MachinePageRow {
                    os: device.map(|d| platform_os(&d.platform)).unwrap_or_default(),
                    via: "relay".into(),
                    space_id: None,
                    stopped_sharing: false,
                    last_seen,
                    status: status_word(r.online, false).into(),
                    spaces: 0,
                    spaces_label: String::new(),
                    id: r.id,
                    name: r.name,
                    online: r.online,
                    current: false,
                    device: r.device,
                    device_only: true,
                };
            }
            let i = machines.iter().position(|m| m.id == r.id).unwrap_or(0);
            let m = &machines[i];
            let os = if m.os.is_empty() {
                device.map(|d| platform_os(&d.platform)).unwrap_or_default()
            } else {
                m.os.clone()
            };
            let stopped_sharing = extra[i].stopped_sharing && r.online;
            let spaces = spaces_on(&input.spaces, &r.id, r.current);
            MachinePageRow {
                os,
                via: extra[i].via.into(),
                space_id: extra[i].space_id.clone(),
                stopped_sharing,
                last_seen,
                status: status_word(r.online, stopped_sharing).into(),
                spaces,
                spaces_label: spaces_label(spaces),
                id: r.id,
                name: r.name,
                online: r.online,
                current: r.current,
                device: r.device,
                device_only: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn this_mac() -> MachineIn {
        MachineIn {
            id: "this-mac".into(),
            name: "This Mac".into(),
            online: true,
            os: "macos".into(),
            current: true,
            ..Default::default()
        }
    }

    fn relay(id: &str, name: &str, online: bool) -> MachineIn {
        MachineIn {
            id: id.into(),
            name: name.into(),
            online,
            os: "macos".into(),
            ..Default::default()
        }
    }

    fn device(id: &str, name: &str) -> DeviceIn {
        DeviceIn {
            id: id.into(),
            name: name.into(),
            platform: "macos".into(),
            last_seen: Some(NOW - 3_600),
            state: "enrolled".into(),
            ..Default::default()
        }
    }

    fn input(machines: Vec<MachineIn>, devices: Vec<DeviceIn>) -> MachinesInput {
        MachinesInput {
            machines,
            devices,
            now: NOW,
        }
    }

    fn names(rows: &[MachineOut]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn a_relay_machine_and_its_device_are_one_row_named_as_the_machine() {
        // gamma-4 as a relay machine and as an enrolled device.
        let mut current = device("dev_juno", "juno-2.example.com");
        current.current = true;
        let rows = merge(&input(
            vec![
                this_mac(),
                relay(
                    "96fedb7e1be65c3d31fa18587febde2c",
                    "gamma-4 Mac Studio",
                    false,
                ),
            ],
            vec![
                device("dev_2278814aabab5c117d348c6b", "gamma-4.example.com"),
                current,
            ],
        ));
        assert_eq!(names(&rows), ["This Mac", "gamma-4 Mac Studio"]);
        assert_eq!(rows[1].id, "96fedb7e1be65c3d31fa18587febde2c");
        assert_eq!(
            rows[1].device.as_deref(),
            Some("dev_2278814aabab5c117d348c6b")
        );
        assert_eq!(rows[0].device.as_deref(), Some("dev_juno"));
        assert!(!rows[1].device_only);
    }

    #[test]
    fn the_reported_hostname_matches_whatever_the_machine_is_called() {
        let mut m = relay("m1", "Studio", true);
        m.hostname = Some("Build-Box.example.com.".into());
        let rows = merge(&input(
            vec![m],
            vec![device("dev_b", "build-box.example.com")],
        ));
        assert_eq!(names(&rows), ["Studio"]);
        assert_eq!(rows[0].device.as_deref(), Some("dev_b"));
    }

    #[test]
    fn the_same_id_or_the_same_name_merges() {
        let rows = merge(&input(
            vec![relay("dev_a", "Alpha", true), relay("m2", "Beta Mac", true)],
            vec![device("dev_a", "alpha.local"), device("dev_b", "beta mac")],
        ));
        assert_eq!(names(&rows), ["Alpha", "Beta Mac"]);
        assert_eq!(rows[1].device.as_deref(), Some("dev_b"));
    }

    #[test]
    fn two_macs_that_share_a_name_are_not_merged() {
        // Two relay machines named "studio …" and one "studio.lan" device:
        // which one it is cannot be told.
        let rows = merge(&input(
            vec![
                relay("m1", "studio Mac Studio", true),
                relay("m2", "studio Mac mini", true),
            ],
            vec![device("dev_s", "studio.lan")],
        ));
        assert_eq!(
            names(&rows),
            ["studio Mac Studio", "studio Mac mini", "studio.lan"]
        );
        assert!(rows[2].device_only);
        // Nor two devices for one machine.
        let rows = merge(&input(
            vec![relay("m1", "Studio", true)],
            vec![device("dev_1", "Studio"), device("dev_2", "studio")],
        ));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].device, None);
    }

    #[test]
    fn different_systems_are_not_merged() {
        let mut linux = relay("m1", "box Linux", true);
        linux.os = "linux".into();
        let rows = merge(&input(
            vec![linux],
            vec![device("dev_b", "box.example.com")],
        ));
        assert_eq!(rows.len(), 2);
        assert!(rows[1].device_only);
    }

    #[test]
    fn a_plain_word_device_is_not_a_hostname() {
        let rows = merge(&input(
            vec![relay("m1", "gamma-4 Mac Studio", true)],
            vec![device("dev_g", "gamma-4")],
        ));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn online_is_reached_or_connected_to_the_relay() {
        let mut seen = relay("m1", "gamma-4 Mac Studio", false);
        seen.presence = Some(true);
        let mut gone = relay("m2", "Other", false);
        gone.presence = Some(false);
        let rows = merge(&input(
            vec![seen, gone, relay("m3", "Reached", true)],
            vec![device("dev_g", "gamma-4.example.com")],
        ));
        assert_eq!(
            rows.iter().map(|r| r.online).collect::<Vec<_>>(),
            [true, false, true]
        );
    }

    #[test]
    fn a_device_alone_is_online_after_a_recent_session_and_never_a_run_on_choice() {
        let mut recent = device("dev_r", "Laptop");
        recent.last_seen = Some(NOW - 60);
        let mut revoked = device("dev_x", "Lost phone");
        revoked.state = "revoked".into();
        let rows = merge(&input(
            vec![this_mac(), relay("m1", "Studio", false)],
            vec![recent, device("dev_o", "Old"), revoked],
        ));
        assert_eq!(names(&rows), ["This Mac", "Studio", "Laptop", "Old"]);
        assert!(rows[2].online && !rows[3].online);
        assert!(rows[2].device_only && rows[3].device_only);
        assert_eq!(run_on_ids(&rows), ["m1"]);
    }

    fn space(v: serde_json::Value) -> Space {
        let mut base = serde_json::json!({
            "name": "", "os": "linux", "status": "running", "detail": "",
            "lastUsedAt": 0, "scene": "linux-terminal"
        });
        for (k, val) in v.as_object().unwrap() {
            base[k] = val.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn the_page_lists_this_machine_its_machines_and_devices_of_their_own() {
        let mut current = device("dev_me", "juno-2.example.com");
        current.current = true;
        let rows = page_rows(&MachinesPageInput {
            spaces: vec![
                space(
                    serde_json::json!({"id": "this-mac", "name": "This Mac", "os": "macos", "status": "local"}),
                ),
                space(
                    serde_json::json!({"id": "local:fbd1", "name": "fbd1", "status": "suspended", "provider": "local"}),
                ),
                // A machine sharing its desktop; its system unknown.
                space(serde_json::json!({
                    "id": "relay:b695", "name": "studio.local", "os": "unknown",
                    "status": "suspended", "provider": "relay",
                    "sdk": {"features": [], "spacesdVersion": "", "reachable": false,
                            "error": "unauthenticated: studio.local stopped sharing: ask its owner"}
                })),
                // A Space a machine provides, with no row of its own.
                space(serde_json::json!({
                    "id": "relay:m-mini/space-1", "name": "dev box", "status": "running",
                    "provider": "relay", "host": "m-mini", "hostName": "Spare mini"
                })),
            ],
            devices: vec![
                current,
                device("dev_s", "studio.local"),
                device("dev_lap", "Laptop"),
            ],
            presence: [("m-mini".to_string(), false)].into(),
            now: NOW,
            ..Default::default()
        });
        let lines: Vec<String> = rows
            .iter()
            .map(|r| {
                format!(
                    "{} {} {} {} {:?} {:?} {}",
                    r.name, r.via, r.os, r.online, r.device, r.space_id, r.device_only
                )
            })
            .collect();
        assert_eq!(
            lines,
            [
                "This Mac local macos true Some(\"dev_me\") Some(\"this-mac\") false",
                // Unknown system, same name: one computer; online, since it
                // refused because its owner stopped sharing.
                "studio.local relay macos true Some(\"dev_s\") Some(\"relay:b695\") false",
                "Spare mini relay  true None None false",
                "Laptop relay macos false Some(\"dev_lap\") None true",
            ]
        );
        assert!(rows[1].stopped_sharing);
        assert_eq!(rows[3].last_seen, Some(NOW - 3_600));
        assert_eq!(run_on_ids(&merge_rows(&rows)), ["b695", "m-mini"]);
        // One status word and Spaces count per row, for every UI.
        let words: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.status.as_str(), r.spaces_label.as_str()))
            .collect();
        assert_eq!(
            words,
            [
                ("Online", "1 Space"),
                ("Not sharing", "No Spaces"),
                ("Online", "1 Space"),
                ("Offline", ""),
            ]
        );
    }

    /// CUA-1617: your other Mac stopped sharing; the relay lists its
    /// desktop as unreachable (a Space would read "Suspended") and it
    /// provides no Spaces. Every UI says "Not sharing" and "No Spaces",
    /// never a Space's "Suspended" or "Not reachable".
    #[test]
    fn a_machine_that_stopped_sharing_reads_not_sharing_with_no_spaces() {
        let rows = page_rows(&MachinesPageInput {
            spaces: vec![space(serde_json::json!({
                "id": "relay:b695", "name": "Dillons-MacBook-Pro-2.local", "os": "macos",
                "status": "suspended", "provider": "relay",
                "sdk": {"features": [], "spacesdVersion": "", "reachable": false,
                        "error": "unauthenticated: Dillons-MacBook-Pro-2.local stopped sharing: ask its owner to Resume sharing (or run `cua host start` there)"}
            }))],
            presence: [("b695".to_string(), false)].into(),
            now: NOW,
            ..Default::default()
        });
        let r = &rows[1];
        assert_eq!(
            (
                r.online,
                r.status.as_str(),
                r.spaces,
                r.spaces_label.as_str()
            ),
            (true, NOT_SHARING, 0, "No Spaces")
        );
        // Unreachable without a refusal: offline, not "Suspended".
        let rows = page_rows(&MachinesPageInput {
            spaces: vec![space(serde_json::json!({
                "id": "relay:b695", "name": "studio", "os": "macos",
                "status": "suspended", "provider": "relay"
            }))],
            ..Default::default()
        });
        assert_eq!(rows[1].status, OFFLINE);
        assert_eq!(spaces_label(3), "3 Spaces");
    }

    fn merge_rows(rows: &[MachinePageRow]) -> Vec<MachineOut> {
        rows.iter()
            .map(|r| MachineOut {
                id: r.id.clone(),
                name: r.name.clone(),
                online: r.online,
                device: r.device.clone(),
                device_only: r.device_only,
                current: r.current,
            })
            .collect()
    }

    /// Every "Run on" choice the page makes from the merged rows is a
    /// machine `on="host:<id>"` resolves (a relay machine's id), never a
    /// device's.
    #[test]
    fn every_offered_placement_is_a_machine() {
        use crate::model::Location;
        use crate::wizard::{self, SpaceHost, WizardEnv};
        let rows = merge(&input(
            vec![
                this_mac(),
                relay(
                    "96fedb7e1be65c3d31fa18587febde2c",
                    "gamma-4 Mac Studio",
                    true,
                ),
            ],
            vec![
                device("dev_2278814aabab5c117d348c6b", "gamma-4.example.com"),
                device("dev_lap", "Laptop"),
            ],
        ));
        let machines: Vec<&str> = ["96fedb7e1be65c3d31fa18587febde2c"].into();
        let env = WizardEnv {
            hosts: rows
                .iter()
                .filter(|r| run_on_ids(&rows).contains(&r.id))
                .map(|r| SpaceHost {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    via: "relay".into(),
                    online: r.online,
                    os: "macos".into(),
                    limits: vec![],
                })
                .collect(),
            default_location: Location::Local,
            cloud_available: false,
            local_available: true,
            local_reason: None,
            local_backends: Some(vec!["docker".into()]),
            local_details: None,
            max_cpus: 8,
            host_arch: Some("arm64".into()),
            lume_source: None,
            linux_source: None,
            storage: None,
            cloud_pricing: None,
            clouds: vec![],
            experiments: crate::experiments::Experiments::default(),
            gpus: None,
            name_seed: None,
        };
        let state = wizard::initial(&env);
        let offered: Vec<String> = wizard::placement_options(&state, &env)
            .into_iter()
            .filter_map(|o| o.id.strip_prefix("host:").map(str::to_string))
            .collect();
        assert_eq!(offered, machines);
    }
}
