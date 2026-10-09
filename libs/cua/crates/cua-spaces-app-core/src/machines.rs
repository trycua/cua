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

use serde::{Deserialize, Serialize};

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
        };
        let state = wizard::initial(&env);
        let offered: Vec<String> = wizard::placement_options(&state, &env)
            .into_iter()
            .filter_map(|o| o.id.strip_prefix("host:").map(str::to_string))
            .collect();
        assert_eq!(offered, machines);
    }
}
