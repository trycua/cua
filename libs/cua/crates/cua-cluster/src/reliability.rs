//! How much a machine can be trusted to keep running a durable drive.
//!
//! The question a user never wants to answer: "which of my machines should
//! run this agent?" The answer is the machine least likely to stop: a desktop
//! that is always on, then a laptop on power, then a laptop on battery; never
//! the laptop the user is about to close. Each machine measures a few facts
//! about itself ([`Facts`], [`probe`]) and turns them into one number
//! ([`score`]) that placement ([`crate::placement::Policy::Durable`]) and the
//! Volume's choice of where to keep its copies both use, so a durable drive
//! and its files end up on the same machine.
//!
//! The number is a ladder of classes 1000 apart with small terms inside a
//! class (less than one class in total), so a small change never moves a
//! drive: only a different kind of machine, or the same machine changing
//! state (unplugged, lid about to close), does.

use serde::{Deserialize, Serialize};

/// A class step: scores this far apart are a different kind of machine.
pub const STEP: u64 = 1000;

/// What the machine's network is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    #[default]
    Unknown,
    Wired,
    Wifi,
    /// A phone's hotspot or another metered link.
    Metered,
}

/// What a machine knows about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// A laptop (it has a battery and a lid).
    pub has_battery: bool,
    /// Running on battery right now.
    pub on_battery: bool,
    /// Battery charge, 0 to 100, when there is a battery.
    #[serde(default)]
    pub battery_pct: Option<u8>,
    /// A durable run holds a sleep assertion here.
    #[serde(default)]
    pub sleep_prevented: bool,
    /// The user is working at this machine (input in the last 5 minutes).
    #[serde(default)]
    pub user_active: bool,
    /// Seconds since boot.
    #[serde(default)]
    pub uptime_secs: u64,
    /// Free disk where the volume lives, in GB.
    #[serde(default)]
    pub free_disk_gb: u64,
    #[serde(default)]
    pub link: Link,
    /// Times its peers lost it recently (from the Volume's failure detector).
    #[serde(default)]
    pub recent_flaps: u32,
    /// A dedicated host (`cua host setup --dedicated`, a cloud machine added
    /// to the account): always on by construction.
    #[serde(default)]
    pub dedicated: bool,
}

impl Facts {
    /// Whether this machine sleeps when its lid closes.
    pub fn sleep_prone(&self) -> bool {
        self.has_battery && !self.dedicated
    }
}

/// The reliability of a machine: bigger keeps the drive. See the module
/// docs for the ladder.
pub fn score(f: &Facts) -> u64 {
    let class: i64 = if f.dedicated || !f.has_battery {
        9000
    } else if !f.on_battery {
        5000
    } else if f.battery_pct.is_some_and(|p| p < 20) {
        1000
    } else {
        2000
    };
    let mut s = class;
    // A laptop the user is sitting at is the one about to be closed; a
    // desktop they work at only loses a tie.
    if f.user_active {
        s -= if f.sleep_prone() { 3500 } else { 300 };
    }
    s += i64::from(f.sleep_prevented) * 400;
    s += match f.link {
        Link::Wired => 250,
        Link::Wifi | Link::Unknown => 0,
        Link::Metered => -250,
    };
    s -= i64::from(f.recent_flaps.min(5)) * 60;
    s += (f.uptime_secs / 86_400).min(10) as i64 * 10;
    s += match f.free_disk_gb {
        0 => 0,
        1..=19 => 0,
        20..=99 => 25,
        _ => 50,
    };
    s.max(0) as u64
}

/// What a machine we know nothing about scores: between a laptop on power
/// and a laptop on battery, so a known good desktop wins and a known weak
/// laptop loses.
pub const UNKNOWN: u64 = 4000;

/// A sentence for the user about why this machine would (not) keep a drive.
pub fn describe(f: &Facts) -> String {
    if f.dedicated {
        "always on".into()
    } else if !f.has_battery {
        if f.user_active {
            "a desktop you are using".into()
        } else {
            "a desktop".into()
        }
    } else if f.user_active && f.sleep_prone() {
        "a laptop you are using".into()
    } else if f.on_battery {
        "a laptop on battery".into()
    } else {
        "a laptop on power".into()
    }
}

/// Reads this machine's facts (best effort; unknown things stay default).
// Per-OS blocks fill different fields, so a struct initializer does not fit.
#[allow(clippy::field_reassign_with_default)]
pub fn probe() -> Facts {
    let mut f = Facts::default();
    #[cfg(target_os = "macos")]
    {
        if let Some(text) = run("/usr/bin/pmset", &["-g", "batt"]) {
            let (has, on_battery, pct) = parse_pmset_batt(&text);
            f.has_battery = has;
            f.on_battery = on_battery;
            f.battery_pct = pct;
        }
        if let Some(text) = run("/usr/sbin/ioreg", &["-c", "IOHIDSystem", "-d", "4"]) {
            f.user_active = parse_hid_idle_secs(&text).is_some_and(|s| s < 300);
        }
        if let Some(text) = run("/usr/sbin/sysctl", &["-n", "kern.boottime"])
            && let Some(boot) = parse_boottime(&text)
        {
            f.uptime_secs = now_secs().saturating_sub(boot);
        }
        f.link = macos_link();
    }
    #[cfg(target_os = "linux")]
    {
        let (has, on_battery, pct) = linux_power();
        f.has_battery = has;
        f.on_battery = on_battery;
        f.battery_pct = pct;
        if let Ok(t) = std::fs::read_to_string("/proc/uptime") {
            f.uptime_secs = t
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f64>().ok())
                .map_or(0, |s| s as u64);
        }
    }
    f.free_disk_gb = free_disk_gb().unwrap_or(0);
    f
}

#[cfg(target_os = "macos")]
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(target_os = "macos")]
fn run(bin: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(bin).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `pmset -g batt`: (has a battery, running on battery, charge).
pub fn parse_pmset_batt(text: &str) -> (bool, bool, Option<u8>) {
    let on_battery = text.contains("'Battery Power'");
    let mut has = false;
    let mut pct = None;
    for line in text.lines() {
        if line.contains("InternalBattery") {
            has = true;
            pct = line
                .split('\t')
                .nth(1)
                .or(Some(line))
                .and_then(|l| l.split('%').next())
                .and_then(|l| l.split_whitespace().last())
                .and_then(|n| n.parse::<u8>().ok());
        }
    }
    (has, has && on_battery, pct)
}

/// `ioreg -c IOHIDSystem`: seconds since the last keyboard or mouse input.
pub fn parse_hid_idle_secs(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.contains("HIDIdleTime"))?;
    let ns: u64 = line.rsplit('=').next()?.trim().parse().ok()?;
    Some(ns / 1_000_000_000)
}

/// `sysctl -n kern.boottime`: `{ sec = 1700000000, usec = 0 } ...`.
pub fn parse_boottime(text: &str) -> Option<u64> {
    let rest = text.split("sec =").nth(1)?;
    rest.trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "macos")]
fn macos_link() -> Link {
    let Some(route) = run("/sbin/route", &["-n", "get", "default"]) else {
        return Link::Unknown;
    };
    let Some(iface) = route.lines().find_map(|l| {
        l.trim()
            .strip_prefix("interface:")
            .map(|s| s.trim().to_string())
    }) else {
        return Link::Unknown;
    };
    let Some(ports) = run("/usr/sbin/networksetup", &["-listallhardwareports"]) else {
        return Link::Unknown;
    };
    link_of_port(&ports, &iface)
}

/// Which kind of port `iface` is, from `networksetup -listallhardwareports`.
pub fn link_of_port(ports: &str, iface: &str) -> Link {
    let mut name = "";
    for line in ports.lines() {
        if let Some(n) = line.strip_prefix("Hardware Port:") {
            name = n.trim();
        }
        if let Some(d) = line.strip_prefix("Device:")
            && d.trim() == iface
        {
            let n = name.to_ascii_lowercase();
            return if n.contains("wi-fi") || n.contains("airport") {
                Link::Wifi
            } else if n.contains("iphone")
                || n.contains("ipad")
                || n.contains("usb") && n.contains("tether")
            {
                Link::Metered
            } else if n.contains("ethernet") || n.contains("thunderbolt") || n.contains("lan") {
                Link::Wired
            } else {
                Link::Unknown
            };
        }
    }
    Link::Unknown
}

#[cfg(target_os = "linux")]
fn linux_power() -> (bool, bool, Option<u8>) {
    let mut has = false;
    let mut on_battery = false;
    let mut pct = None;
    if let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") {
        for e in rd.flatten() {
            let p = e.path();
            let kind = std::fs::read_to_string(p.join("type")).unwrap_or_default();
            if kind.trim() == "Battery" {
                has = true;
                let st = std::fs::read_to_string(p.join("status")).unwrap_or_default();
                if st.trim() == "Discharging" {
                    on_battery = true;
                }
                pct = std::fs::read_to_string(p.join("capacity"))
                    .ok()
                    .and_then(|c| c.trim().parse().ok());
            }
        }
    }
    (has, on_battery, pct)
}

#[cfg(unix)]
fn free_disk_gb() -> Option<u64> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let c = std::ffi::CString::new(home.to_string_lossy().as_bytes()).ok()?;
    // SAFETY: an all-zero statvfs is a valid out value.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `st` a valid out pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0 {
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64) >> 30)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn free_disk_gb() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laptop(on_battery: bool) -> Facts {
        Facts {
            has_battery: true,
            on_battery,
            battery_pct: Some(80),
            ..Default::default()
        }
    }

    #[test]
    fn a_desktop_beats_a_plugged_laptop_beats_a_laptop_on_battery() {
        let desktop = Facts::default();
        let plugged = laptop(false);
        let battery = laptop(true);
        assert!(score(&desktop) >= score(&plugged) + STEP);
        assert!(score(&plugged) >= score(&battery) + STEP);
        let low = Facts {
            battery_pct: Some(10),
            ..laptop(true)
        };
        assert!(score(&battery) >= score(&low) + STEP / 2);
    }

    #[test]
    fn the_laptop_the_user_is_using_loses_but_a_desktop_only_loses_a_tie() {
        let mine = Facts {
            user_active: true,
            ..laptop(false)
        };
        let away = laptop(true);
        assert!(
            score(&away) > score(&mine),
            "the laptop in use is the one about to close"
        );
        let desk_used = Facts {
            user_active: true,
            ..Default::default()
        };
        assert!(score(&desk_used) > score(&laptop(false)));
        assert!(score(&desk_used) < score(&Facts::default()));
    }

    #[test]
    fn small_terms_never_cross_a_class() {
        // The best a laptop on battery can do is below the worst a plugged
        // laptop does.
        let best_battery = Facts {
            sleep_prevented: true,
            link: Link::Wired,
            uptime_secs: 30 * 86_400,
            free_disk_gb: 500,
            ..laptop(true)
        };
        let worst_plugged = Facts {
            link: Link::Metered,
            recent_flaps: 9,
            ..laptop(false)
        };
        assert!(score(&best_battery) < score(&worst_plugged));
        let best_plugged = Facts {
            sleep_prevented: true,
            link: Link::Wired,
            uptime_secs: 30 * 86_400,
            free_disk_gb: 500,
            ..laptop(false)
        };
        let worst_desktop = Facts {
            link: Link::Metered,
            recent_flaps: 9,
            ..Default::default()
        };
        assert!(score(&best_plugged) < score(&worst_desktop));
        // An unknown machine sits between plugged and battery.
        assert!(score(&laptop(true)) < UNKNOWN && UNKNOWN < score(&laptop(false)));
    }

    #[test]
    fn a_dedicated_host_is_always_a_desktop() {
        let f = Facts {
            dedicated: true,
            has_battery: true,
            on_battery: true,
            ..Default::default()
        };
        assert_eq!(score(&f), 9000);
        assert!(!f.sleep_prone());
    }

    #[test]
    fn pmset_and_ioreg_and_sysctl_output_parse() {
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t100%; charged; 0:00 remaining present: true\n";
        assert_eq!(parse_pmset_batt(ac), (true, false, Some(100)));
        let bat = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t43%; discharging; 3:12 remaining present: true\n";
        assert_eq!(parse_pmset_batt(bat), (true, true, Some(43)));
        let desk = "Now drawing from 'AC Power'\n";
        assert_eq!(parse_pmset_batt(desk), (false, false, None));
        let hid = "    | |   \"HIDIdleTime\" = 125000000000\n";
        assert_eq!(parse_hid_idle_secs(hid), Some(125));
        assert_eq!(
            parse_boottime("{ sec = 1700000000, usec = 123 } Tue Nov 14 22:13:20 2023"),
            Some(1_700_000_000)
        );
        let ports = "Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: x\n\nHardware Port: Thunderbolt Ethernet Slot 1\nDevice: en5\n\nHardware Port: iPhone USB\nDevice: en7\n";
        assert_eq!(link_of_port(ports, "en0"), Link::Wifi);
        assert_eq!(link_of_port(ports, "en5"), Link::Wired);
        assert_eq!(link_of_port(ports, "en7"), Link::Metered);
        assert_eq!(link_of_port(ports, "en9"), Link::Unknown);
    }

    #[test]
    fn this_machine_can_be_probed() {
        let f = probe();
        // Whatever it is, it scores and describes itself.
        assert!(!describe(&f).is_empty());
        let _ = score(&f);
    }
}
