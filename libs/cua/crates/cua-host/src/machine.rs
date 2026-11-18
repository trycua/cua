//! What identifies this machine to the relay across device keys: a hash of
//! the host's hardware or install identity (the macOS hardware UUID, the
//! Linux machine id with the host name, the Windows machine GUID) and the
//! cua home, so every build of cua one user runs on one machine is the same
//! device, while other users of the machine (and isolated homes) are not.
//! The raw value never leaves the machine, and the relay stores the hash
//! keyed per account. It is a claim, not a secret: the relay uses it only to let a
//! new key of the same machine replace the old record once that key
//! enrolled on its own (a fresh sign-in or an approval).

use sha2::Digest as _;

/// This machine's id for device enrollment (for the current cua home), when
/// the platform has one.
pub fn machine_id() -> Option<String> {
    raw_machine_id().map(|raw| hash(&format!("{raw}\n{}", cua_home::cua_home().display())))
}

fn hash(raw: &str) -> String {
    let digest = sha2::Sha256::digest(format!("cua-device-machine/v1\n{raw}").as_bytes());
    hex::encode(&digest[..16])
}

#[cfg(target_os = "macos")]
fn raw_machine_id() -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    parse_ioreg_uuid(&String::from_utf8_lossy(&out.stdout))
}

/// `"IOPlatformUUID" = "…"` from `ioreg -rd1 -c IOPlatformExpertDevice`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_ioreg_uuid(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.contains("\"IOPlatformUUID\""))
        .and_then(|l| l.split('=').nth(1))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .filter(|v| !v.is_empty())
}

/// The machine id with the host name: containers of one image often share
/// `/etc/machine-id`, and each has its own host name.
#[cfg(target_os = "linux")]
fn raw_machine_id() -> Option<String> {
    let id = ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())?;
    Some(format!("{id}\n{}", crate::host::hostname()))
}

#[cfg(windows)]
fn raw_machine_id() -> Option<String> {
    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .ok()?;
    parse_reg_machine_guid(&String::from_utf8_lossy(&out.stdout))
}

/// `MachineGuid    REG_SZ    …` from `reg query`.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_reg_machine_guid(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.trim_start().starts_with("MachineGuid"))
        .and_then(|l| l.split_whitespace().nth(2))
        .map(str::to_owned)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn raw_machine_id() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_outputs_parse() {
        let ioreg = r#"+-o J316sAP  <class IOPlatformExpertDevice, id 0x100000210>
    {
      "IOPlatformSerialNumber" = "XYZ"
      "IOPlatformUUID" = "0A1B2C3D-0000-1111-2222-333344445555"
    }"#;
        assert_eq!(
            parse_ioreg_uuid(ioreg).as_deref(),
            Some("0A1B2C3D-0000-1111-2222-333344445555")
        );
        assert_eq!(parse_ioreg_uuid("nothing here"), None);
        let reg = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n    MachineGuid    REG_SZ    6f1e2d3c-aaaa-bbbb-cccc-0123456789ab\r\n";
        assert_eq!(
            parse_reg_machine_guid(reg).as_deref(),
            Some("6f1e2d3c-aaaa-bbbb-cccc-0123456789ab")
        );
    }

    #[test]
    fn the_id_is_a_stable_hash_that_hides_the_raw_value() {
        let a = hash("0A1B2C3D-0000-1111-2222-333344445555");
        assert_eq!(a, hash("0A1B2C3D-0000-1111-2222-333344445555"));
        assert_ne!(a, hash("another machine"));
        assert_eq!(a.len(), 32);
        assert!(!a.contains("0A1B2C3D"));
        // A Mac always has one, and it does not change between calls.
        #[cfg(target_os = "macos")]
        assert!(machine_id().is_some());
        assert_eq!(machine_id(), machine_id());
    }
}
