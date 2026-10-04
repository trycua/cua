//! Guest IP discovery for Lume VMs without waiting on `lume serve`.
//!
//! `GET /lume/vms/:name` reports `ipAddress` by looking the VM's MAC up in
//! the host's vmnet DHCP leases, but only while the VM is `running` from the
//! server's point of view and only once its own lookup succeeds; right after
//! `lume run --detach` it can lag or report nothing. The same facts are on
//! the host, readable without privileges:
//!
//! * `/var/db/dhcpd_leases` (macOS `bootpd`, which serves vmnet NAT): one
//!   `{ name= ip_address= hw_address=1,<mac> lease=0x<expiry> }` block per
//!   lease, newest first, with MAC octets not zero-padded (`e2:2d:52:4:4e:bb`);
//! * the ARP table (`arp -an`), for bridged guests or once the guest talked.
//!
//! Both are read-only; the lease file is never written.
//!
//! **Why `lume serve` alone is flaky for Linux guests:** it matches leases on
//! `hw_address` = the VM's MAC, but systemd-networkd (Debian/Ubuntu cloud
//! images) sends an RFC 4361 client identifier, so `bootpd` records
//! `hw_address=ff,<IAID+DUID>` and the MAC never appears in the lease file.
//! `lume serve` then only finds the IP through its ARP fallback, i.e. once
//! host and guest happened to exchange packets, which may be never. The
//! guest's hostname is in the lease (`name=`), and cloud-init sets it to the
//! VM name, so leases are also matched by name, restricted to leases granted
//! after the VM was started (not present in a snapshot taken before).

use std::path::Path;

/// One `bootpd` lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// Client host name (the guest hostname; cloud-init sets it to the VM name).
    pub name: Option<String>,
    pub ip: String,
    /// Normalised MAC (lowercase, two hex digits per octet); `None` when the
    /// client identified itself with a DUID (`hw_address=ff,...`).
    pub mac: Option<String>,
    /// Expiry, seconds since the Unix epoch.
    pub expires: u64,
}

/// Default lease file of macOS `bootpd`.
pub const LEASES_PATH: &str = "/var/db/dhcpd_leases";

/// `E2:2D:52:4:4E:BB` → `e2:2d:52:04:4e:bb`. `None` if it is not a MAC.
pub fn normalize_mac(mac: &str) -> Option<String> {
    let parts: Vec<&str> = mac.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = Vec::with_capacity(6);
    for p in parts {
        let v = u8::from_str_radix(p, 16).ok()?;
        out.push(format!("{v:02x}"));
    }
    Some(out.join(":"))
}

/// Parse the `bootpd` lease file format.
pub fn parse_leases(text: &str) -> Vec<Lease> {
    let mut out = Vec::new();
    let (mut name, mut ip, mut mac, mut expires) = (None, None, None, None);
    for line in text.lines().map(str::trim) {
        match line {
            "{" => (name, ip, mac, expires) = (None, None, None, None),
            "}" => {
                if let Some(i) = ip.take() {
                    out.push(Lease {
                        name: name.take(),
                        ip: i,
                        mac: mac.take(),
                        expires: expires.take().unwrap_or(0),
                    });
                }
            }
            l => {
                let Some((k, v)) = l.split_once('=') else {
                    continue;
                };
                match k {
                    "name" => name = Some(v.to_string()),
                    "ip_address" => ip = Some(v.to_string()),
                    // `1,<mac>` (ethernet); other hardware types are ignored.
                    "hw_address" => {
                        mac = v
                            .split_once(',')
                            .filter(|(ty, _)| *ty == "1")
                            .and_then(|(_, m)| normalize_mac(m))
                    }
                    "lease" => {
                        expires = u64::from_str_radix(v.trim_start_matches("0x"), 16).ok();
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// The IP of the newest lease for `mac` that has not expired at `now`.
pub fn lease_ip(leases: &[Lease], mac: &str, now: u64) -> Option<String> {
    let mac = normalize_mac(mac)?;
    leases
        .iter()
        .filter(|l| l.mac.as_deref() == Some(mac.as_str()) && l.expires > now)
        .max_by_key(|l| l.expires)
        .map(|l| l.ip.clone())
}

/// The IP of a live lease for host `name` that is not in `before` (a
/// snapshot taken before the VM started), newest first.
pub fn fresh_lease_by_name(
    leases: &[Lease],
    before: &[Lease],
    name: &str,
    now: u64,
) -> Option<String> {
    leases
        .iter()
        .filter(|l| l.name.as_deref() == Some(name) && l.expires > now && !before.contains(l))
        .max_by_key(|l| l.expires)
        .map(|l| l.ip.clone())
}

/// Current leases (empty when the file is missing or unreadable).
pub async fn read_leases(leases_path: &Path) -> Vec<Lease> {
    tokio::fs::read_to_string(leases_path)
        .await
        .map(|t| parse_leases(&t))
        .unwrap_or_default()
}

/// `arp -an` line `? (192.168.64.5) at e2:2d:52:4:4e:bb on bridge100 ...`.
pub fn arp_ip(arp_output: &str, mac: &str) -> Option<String> {
    let mac = normalize_mac(mac)?;
    arp_output.lines().find_map(|l| {
        let ip = l.split_once('(')?.1.split_once(')')?.0;
        let at = l.split(" at ").nth(1)?.split_whitespace().next()?;
        (normalize_mac(at).as_deref() == Some(mac.as_str())).then(|| ip.to_string())
    })
}

/// Look the VM up in the lease file (by MAC, then by host name among leases
/// newer than `before`), then in the ARP table by MAC. Read-only.
pub async fn lookup(
    mac: Option<&str>,
    name: &str,
    before: &[Lease],
    leases_path: &Path,
) -> Option<(String, &'static str)> {
    let now = crate::host::now_secs();
    let leases = read_leases(leases_path).await;
    if let Some(ip) = mac.and_then(|m| lease_ip(&leases, m, now)) {
        return Some((ip, "dhcpd_leases (mac)"));
    }
    if let Some(ip) = fresh_lease_by_name(&leases, before, name, now) {
        return Some((ip, "dhcpd_leases (hostname)"));
    }
    let mac = mac?;
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::process::Command::new("/usr/sbin/arp")
            .arg("-an")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    arp_ip(&String::from_utf8_lossy(&out.stdout), mac).map(|ip| (ip, "arp"))
}

/// Directories Lume keeps VMs in: `~/.lume` plus every `path:` under
/// `vmLocations` in `~/.config/lume/config.yaml`.
pub fn vm_dirs(home: &Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![home.join(".lume")];
    if let Ok(cfg) = std::fs::read_to_string(home.join(".config/lume/config.yaml")) {
        for l in cfg.lines() {
            if let Some(p) = l.trim().strip_prefix("path:") {
                let p = p.trim().trim_matches('"').trim_matches('\'');
                let p = match p.strip_prefix("~/") {
                    Some(rest) => home.join(rest),
                    None => std::path::PathBuf::from(p),
                };
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Removes the empty `.<name>.resize.guard` lock files Lume leaves next to
/// a deleted VM (Lume releases before the fix never unlink them), in every
/// VM directory. Only `name`'s, and only while no VM of that name exists
/// there; a guard with content is not Lume's and stays.
pub fn remove_resize_guards(home: &Path, name: &str) {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return;
    }
    for dir in vm_dirs(home) {
        let guard = dir.join(format!(".{name}.resize.guard"));
        let empty = std::fs::symlink_metadata(&guard).is_ok_and(|m| m.is_file() && m.len() == 0);
        if empty
            && !dir.join(name).exists()
            && let Err(e) = std::fs::remove_file(&guard)
        {
            tracing::debug!(path = %guard.display(), error = %e, "could not remove a Lume resize guard");
        }
    }
}

/// The VM's MAC from its Lume `config.json` (read-only). `lume serve`
/// releases before 0.6 do not report `macAddress` over the API.
pub fn config_mac(home: &Path, name: &str) -> Option<String> {
    vm_dirs(home).into_iter().find_map(|d| {
        let raw = std::fs::read(d.join(name).join("config.json")).ok()?;
        let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
        v.get("macAddress")?.as_str().and_then(normalize_mac)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deleted_vms_resize_guard_goes_and_no_other() {
        let home = tempfile::tempdir().unwrap();
        let lume = home.path().join(".lume");
        std::fs::create_dir_all(lume.join("kept")).unwrap();
        for f in ["gone", "kept", "other"] {
            std::fs::write(lume.join(format!(".{f}.resize.guard")), b"").unwrap();
        }
        std::fs::write(lume.join(".full.resize.guard"), b"x").unwrap();
        remove_resize_guards(home.path(), "gone");
        remove_resize_guards(home.path(), "kept");
        remove_resize_guards(home.path(), "full");
        remove_resize_guards(home.path(), "../escape");
        assert!(!lume.join(".gone.resize.guard").exists());
        assert!(
            lume.join(".kept.resize.guard").exists(),
            "its VM still exists"
        );
        assert!(
            lume.join(".other.resize.guard").exists(),
            "another VM's guard"
        );
        assert!(
            lume.join(".full.resize.guard").exists(),
            "not an empty guard"
        );
    }

    const LEASES: &str = "{\n\tname=cua-e2e-full03-lume-rs\n\tip_address=192.168.64.15\n\thw_address=1,e2:2d:52:4:4e:bb\n\tidentifier=1,e2:2d:52:4:4e:bb\n\tlease=0x6ab354b8\n}\n{\n\tname=cua-e2e-full03-lume-rs\n\tip_address=192.168.64.14\n\thw_address=1,e6:6f:de:87:e5:c2\n\tidentifier=1,e6:6f:de:87:e5:c2\n\tlease=0x6ab354a6\n}\n{\n\tname=old\n\tip_address=192.168.64.3\n\thw_address=1,e2:2d:52:4:4e:bb\n\tlease=0x10\n}\n";

    #[test]
    fn macs_are_normalised() {
        assert_eq!(
            normalize_mac("E2:2D:52:4:4E:BB").as_deref(),
            Some("e2:2d:52:04:4e:bb")
        );
        assert_eq!(
            normalize_mac("e2-2d-52-04-4e-bb").as_deref(),
            Some("e2:2d:52:04:4e:bb")
        );
        assert_eq!(normalize_mac("nope"), None);
    }

    #[test]
    fn leases_parse_and_the_newest_live_one_wins() {
        let l = parse_leases(LEASES);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].mac.as_deref(), Some("e2:2d:52:04:4e:bb"));
        assert_eq!(l[0].expires, 0x6ab354b8);
        assert_eq!(l[0].name.as_deref(), Some("cua-e2e-full03-lume-rs"));
        // The same MAC has a stale lease too; the live, newest one is used.
        assert_eq!(
            lease_ip(&l, "e2:2d:52:04:4e:bb", 0x6ab35000).as_deref(),
            Some("192.168.64.15")
        );
        // Expired leases are ignored (a reused MAC must not return an old IP).
        assert_eq!(lease_ip(&l, "e2:2d:52:04:4e:bb", 0x7000_0000), None);
        assert_eq!(lease_ip(&l, "aa:bb:cc:dd:ee:ff", 0), None);
    }

    #[test]
    fn duid_leases_match_by_name_only_when_new() {
        // Verbatim shape of a Debian genericcloud guest under lume: the
        // client id is a systemd DUID, not the MAC.
        let old = "{\n\tname=vm1\n\tip_address=192.168.64.2\n\thw_address=ff,f1:f5:dd:7f:0:2:0:0:ab:11:b8:13\n\tlease=0x6ab31d90\n}\n";
        let new = format!(
            "{{\n\tname=vm1\n\tip_address=192.168.64.28\n\thw_address=ff,f1:f5:dd:7f:0:2:0:0:ab:11:b7:8e\n\tidentifier=ff,f1:f5\n\tlease=0x6ab37cb4\n}}\n{old}"
        );
        let before = parse_leases(old);
        let now = parse_leases(&new);
        assert_eq!(now[0].mac, None, "DUID leases carry no MAC");
        assert_eq!(lease_ip(&now, "e2:2d:52:04:4e:bb", 0), None);
        assert_eq!(
            fresh_lease_by_name(&now, &before, "vm1", 0x6ab30000).as_deref(),
            Some("192.168.64.28")
        );
        // Only the stale lease: nothing (the VM has not asked yet).
        assert_eq!(fresh_lease_by_name(&before, &before, "vm1", 0), None);
        assert_eq!(fresh_lease_by_name(&now, &before, "vm2", 0), None);
    }

    #[test]
    fn config_mac_is_read_from_lume_vm_dirs() {
        let home = tempfile::tempdir().unwrap();
        let other = home.path().join("vms");
        std::fs::create_dir_all(home.path().join(".config/lume")).unwrap();
        std::fs::write(
            home.path().join(".config/lume/config.yaml"),
            format!("vmLocations:\n  - name: \"home\"\n    path: \"~/.lume\"\n  - name: x\n    path: {}\n", other.display()),
        )
        .unwrap();
        std::fs::create_dir_all(other.join("vm1")).unwrap();
        std::fs::write(
            other.join("vm1/config.json"),
            r#"{"macAddress":"E2:2D:52:4:4E:BB","os":"linux"}"#,
        )
        .unwrap();
        assert_eq!(
            config_mac(home.path(), "vm1").as_deref(),
            Some("e2:2d:52:04:4e:bb")
        );
        assert_eq!(config_mac(home.path(), "nope"), None);
    }

    #[test]
    fn arp_lines_match_by_mac() {
        let arp = "? (192.168.64.1) at 3e:22:fb:b1:e:64 on bridge100 ifscope permanent [bridge]\n\
                   ? (192.168.64.9) at e2:2d:52:4:4e:bb on bridge100 ifscope [bridge]\n\
                   ? (192.168.64.10) at (incomplete) on bridge100 ifscope [bridge]\n";
        assert_eq!(
            arp_ip(arp, "E2:2D:52:04:4E:BB").as_deref(),
            Some("192.168.64.9")
        );
        assert_eq!(arp_ip(arp, "00:00:00:00:00:01"), None);
    }
}
