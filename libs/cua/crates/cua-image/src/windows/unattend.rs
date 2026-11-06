//! The unattended install: the answer file (`Autounattend.xml`), the in-guest
//! setup script, and the small data disc that carries both next to the
//! untouched Windows ISO (Setup reads `Autounattend.xml` from any drive).
//!
//! What the answer file does: GPT disk (EFI, MSR, NTFS), the edition picked by
//! name (Server) or index, the TPM / Secure Boot / RAM checks bypassed so the
//! install does not depend on the hypervisor's firmware, virtio storage and
//! network drivers loaded in WinPE from the virtio-win disc, a local
//! administrator without a password signed in automatically, every OOBE page
//! skipped, UAC off, RDP on, sleep and Windows Update off, and the setup script
//! run at the first logon.

use super::joliet::{DiscFile, write_disc};
use super::table::{WindowsArch, WindowsVersion};

/// The cua-spacesd release the guest downloads when the newest one cannot be
/// looked up ([`latest_spacesd`] asks GitHub).
pub const SPACESD_VERSION: &str = "0.12.2";
/// The port cua-spacesd listens on in the guest.
pub const SPACESD_PORT: u16 = 3211;
/// The default local account (`CUA_WINDOWS_USER` overrides it: benchmark
/// images whose tasks assume a user name set it).
pub const DEFAULT_ACCOUNT: &str = "User";

const SETUP_SCRIPT: &str = include_str!("setup-spacesd.ps1");

/// Generic product key for Windows 11 Pro: selects the edition on consumer
/// media, activates nothing.
const PRO_GENERIC_KEY: &str = "VK7JG-NPHTM-C97JM-9MPGT-3V66T";

/// What the answer file is generated for.
#[derive(Clone, Debug)]
pub struct UnattendOptions<'a> {
    pub version: &'a WindowsVersion,
    pub arch: WindowsArch,
    /// The local administrator account.
    pub account: &'a str,
    /// Install this edition by name instead of the default one (a multi-edition
    /// consumer ISO: `Windows 11 Pro`).
    pub edition: Option<&'a str>,
    /// Whether the ISO is Microsoft's evaluation media (no product key) or one
    /// of yours.
    pub evaluation_media: bool,
    /// Whether to load the virtio drivers (QEMU yes, Hyper-V no).
    pub virtio: bool,
    /// The cua-spacesd release the guest installs (`0.12.2`).
    pub spacesd_version: &'a str,
}

/// The account name from the options or the environment, checked: it ends up
/// in XML and in a Windows account name.
pub fn account_name(requested: Option<&str>, env: Option<&str>) -> Result<String, String> {
    let name = requested
        .or(env)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_ACCOUNT);
    let ok = name.len() <= 20
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !name.eq_ignore_ascii_case("administrator")
        && !name.eq_ignore_ascii_case("guest");
    if ok {
        Ok(name.to_string())
    } else {
        Err(format!(
            "{name:?} is not usable as the Windows account name (letters, digits, - and _, at most 20 characters, not Administrator or Guest)"
        ))
    }
}

/// The virtio-win folder for a Windows version (`w11`, `2k22`, ...).
pub fn virtio_folder(v: &WindowsVersion) -> &'static str {
    match v.id {
        "11" => "w11",
        "10" => "w10",
        "2025" => "2k25",
        "2022" => "2k22",
        "2019" => "2k19",
        _ => "w11",
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn component(name: &str, arch: &str) -> String {
    format!(
        r#"<component name="{name}" processorArchitecture="{arch}" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">"#
    )
}

/// The virtio-win driver paths as a `DriverPaths` component: `component_name`
/// is the WinPE one (storage and network while Setup runs) or the one for the
/// installed system (offline servicing: the network adapter after the first boot).
fn driver_paths(o: &UnattendOptions<'_>, component_name: &str) -> String {
    if !o.virtio {
        return String::new();
    }
    let arch = o.arch.virtio_dir();
    let folder = virtio_folder(o.version);
    // The virtio disc is a drive letter Setup does not tell us: list them all.
    let mut paths = String::new();
    let mut n = 0;
    for dir in ["viostor", "NetKVM", "vioscsi"] {
        for letter in ["D", "E", "F", "G", "H"] {
            n += 1;
            paths.push_str(&format!(
                "        <PathAndCredentials wcm:action=\"add\" wcm:keyValue=\"{n}\"><Path>{letter}:\\{dir}\\{folder}\\{arch}</Path></PathAndCredentials>\n"
            ));
        }
    }
    format!(
        "    {}\n      <DriverPaths>\n{paths}      </DriverPaths>\n    </component>\n",
        component(component_name, o.arch.processor())
    )
}

impl WindowsArch {
    fn processor(self) -> &'static str {
        self.as_str()
    }
}

/// The Autounattend.xml for `o`.
pub fn autounattend_xml(o: &UnattendOptions<'_>) -> String {
    let arch = o.arch.processor();
    let (key_name, key_value) = match (o.edition, o.version.server) {
        (Some(e), _) => ("/IMAGE/NAME", e.to_string()),
        (None, true) => ("/IMAGE/NAME", o.version.edition.to_string()),
        (None, false) => ("/IMAGE/INDEX", "1".to_string()),
    };
    // A product key only for a client edition picked on consumer media: the
    // evaluation ISOs carry their own and Server media needs none.
    let product_key = if !o.version.server
        && !o.evaluation_media
        && o.edition
            .is_some_and(|e| e.to_ascii_lowercase().contains("pro"))
    {
        format!("<ProductKey><Key>{PRO_GENERIC_KEY}</Key></ProductKey>")
    } else {
        String::new()
    };
    let bypass: String = [
        r#"HKLM\SYSTEM\Setup\LabConfig" /v BypassTPMCheck"#,
        r#"HKLM\SYSTEM\Setup\LabConfig" /v BypassSecureBootCheck"#,
        r#"HKLM\SYSTEM\Setup\LabConfig" /v BypassRAMCheck"#,
        r#"HKLM\SYSTEM\Setup\LabConfig" /v BypassCPUCheck"#,
    ]
    .iter()
    .enumerate()
    .map(|(i, v)| {
        format!(
            "        <RunSynchronousCommand wcm:action=\"add\"><Order>{}</Order><Path>reg.exe add \"{v} /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>\n",
            i + 1
        )
    })
    .collect();
    let account = xml_escape(o.account);
    let first_logon: String = [
        ("Disable hibernation", "cmd /C POWERCFG -H OFF"),
        ("Disable monitor blanking", "cmd /C POWERCFG -X -monitor-timeout-ac 0"),
        ("Disable sleep", "cmd /C POWERCFG -X -standby-timeout-ac 0"),
        (
            "Disable Windows Update",
            r#"reg.exe add "HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU" /v NoAutoUpdate /t REG_DWORD /d 1 /f"#,
        ),
        (
            "Install cua-spacesd",
            r#"powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "foreach ($d in [System.IO.DriveInfo]::GetDrives()) { $f = Join-Path $d.Name 'setup-spacesd.ps1'; if (Test-Path $f) { & $f; break } }""#,
        ),
    ]
    .iter()
    .enumerate()
    .map(|(i, (d, c))| {
        format!(
            "        <SynchronousCommand wcm:action=\"add\"><Order>{}</Order><CommandLine>{}</CommandLine><Description>{d}</Description></SynchronousCommand>\n",
            i + 1,
            xml_escape(c).replace("&quot;", "\"")
        )
    })
    .collect();

    let mut x = String::new();
    x.push_str(
        r#"<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="windowsPE">
"#,
    );
    x.push_str(&format!(
        "    {}\n      <SetupUILanguage><UILanguage>en-US</UILanguage></SetupUILanguage>\n      <InputLocale>0409:00000409</InputLocale><SystemLocale>en-US</SystemLocale><UILanguage>en-US</UILanguage><UserLocale>en-US</UserLocale>\n    </component>\n",
        component("Microsoft-Windows-International-Core-WinPE", arch)
    ));
    x.push_str(&driver_paths(o, "Microsoft-Windows-PnpCustomizationsWinPE"));
    x.push_str(&format!(
        r#"    {setup}
      <DiskConfiguration>
        <Disk wcm:action="add">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add"><Order>1</Order><Type>EFI</Type><Size>128</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>2</Order><Type>MSR</Type><Size>128</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>3</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add"><Order>1</Order><PartitionID>1</PartitionID><Label>System</Label><Format>FAT32</Format></ModifyPartition>
            <ModifyPartition wcm:action="add"><Order>2</Order><PartitionID>2</PartitionID></ModifyPartition>
            <ModifyPartition wcm:action="add"><Order>3</Order><PartitionID>3</PartitionID><Label>Windows</Label><Letter>C</Letter><Format>NTFS</Format></ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall>
        <OSImage>
          <InstallTo><DiskID>0</DiskID><PartitionID>3</PartitionID></InstallTo>
          <InstallFrom><MetaData wcm:action="add"><Key>{key_name}</Key><Value>{key_value}</Value></MetaData></InstallFrom>
          <InstallToAvailablePartition>false</InstallToAvailablePartition>
        </OSImage>
      </ImageInstall>
      <UserData>
        <AcceptEula>true</AcceptEula>
        <FullName>cua</FullName>
        <Organization>cua</Organization>
        {product_key}
      </UserData>
      <RunSynchronous>
{bypass}      </RunSynchronous>
    </component>
  </settings>
  <settings pass="offlineServicing">
    {lua}
      <EnableLUA>false</EnableLUA>
    </component>
{offline_drivers}  </settings>
  <settings pass="specialize">
    {deploy}
      <RunSynchronous>
        <RunSynchronousCommand wcm:action="add"><Order>1</Order><Path>reg.exe add "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\OOBE" /v BypassNRO /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
      </RunSynchronous>
    </component>
    {ts}
      <fDenyTSConnections>false</fDenyTSConnections>
    </component>
    {rdp}
      <UserAuthentication>0</UserAuthentication>
    </component>
  </settings>
  <settings pass="oobeSystem">
    {shell}
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add">
            <Name>{account}</Name>
            <Group>Administrators</Group>
            <Password><Value /><PlainText>true</PlainText></Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>
      <AutoLogon>
        <Username>{account}</Username>
        <Enabled>true</Enabled>
        <LogonCount>65432</LogonCount>
        <Password><Value /><PlainText>true</PlainText></Password>
      </AutoLogon>
      <Display><ColorDepth>32</ColorDepth><HorizontalResolution>1280</HorizontalResolution><VerticalResolution>720</VerticalResolution></Display>
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideLocalAccountScreen>true</HideLocalAccountScreen>
        <HideOEMRegistrationScreen>true</HideOEMRegistrationScreen>
        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <NetworkLocation>Home</NetworkLocation>
        <ProtectYourPC>3</ProtectYourPC>
        <SkipUserOOBE>true</SkipUserOOBE>
        <SkipMachineOOBE>true</SkipMachineOOBE>
      </OOBE>
      <FirstLogonCommands>
{first_logon}      </FirstLogonCommands>
    </component>
  </settings>
</unattend>
"#,
        offline_drivers = driver_paths(o, "Microsoft-Windows-PnpCustomizationsNonWinPE"),
        setup = component("Microsoft-Windows-Setup", arch),
        lua = component("Microsoft-Windows-LUA-Settings", arch),
        deploy = component("Microsoft-Windows-Deployment", arch),
        ts = component("Microsoft-Windows-TerminalServices-LocalSessionManager", arch),
        rdp = component("Microsoft-Windows-TerminalServices-RDP-WinStationExtensions", arch),
        shell = component("Microsoft-Windows-Shell-Setup", arch),
    ));
    x
}

/// The cua-spacesd downloads the guest tries, best first. ARM64 Windows runs
/// x64 programs under its own emulation, so while no native ARM64 build is
/// released the x64 one is the fallback (the native one is used as soon as a
/// release carries it).
pub fn spacesd_urls(arch: WindowsArch, version: &str) -> Vec<String> {
    let url = |target: &str| {
        format!(
            "https://github.com/trycua/cua/releases/download/cua-spacesd-v{version}/cua-spacesd-{target}.exe"
        )
    };
    match arch {
        WindowsArch::Amd64 => vec![url("x86_64-pc-windows-msvc")],
        WindowsArch::Arm64 => vec![
            url("aarch64-pc-windows-msvc"),
            url("x86_64-pc-windows-msvc"),
        ],
    }
}

/// The newest published cua-spacesd release that carries a Windows build, from
/// GitHub's release list; `None` when it cannot be read.
pub fn latest_spacesd(releases_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(releases_json).ok()?;
    v.as_array()?.iter().find_map(|r| {
        let tag = r.get("tag_name")?.as_str()?.strip_prefix("cua-spacesd-v")?;
        let live = !r.get("draft")?.as_bool()? && !r.get("prerelease")?.as_bool()?;
        let windows = r
            .get("assets")?
            .as_array()?
            .iter()
            .any(|a| a.get("name").and_then(|n| n.as_str()) == Some("cua-spacesd-x86_64-pc-windows-msvc.exe"));
        (live && windows).then(|| tag.to_string())
    })
}

/// The setup script for `arch`, placeholders filled.
pub fn setup_script(arch: WindowsArch, version: &str) -> String {
    let urls = spacesd_urls(arch, version)
        .iter()
        .map(|u| format!("'{u}'"))
        .collect::<Vec<_>>()
        .join(", ");
    SETUP_SCRIPT
        .replace("__VERSION__", version)
        .replace("__PORT__", &SPACESD_PORT.to_string())
        .replace("__URLS__", &urls)
}

/// The data disc: `Autounattend.xml` and `setup-spacesd.ps1`, labelled `OEMDRV`.
pub fn data_disc(o: &UnattendOptions<'_>) -> Vec<u8> {
    let xml = autounattend_xml(o);
    let ps1 = setup_script(o.arch, o.spacesd_version);
    write_disc(
        "OEMDRV",
        &[
            DiscFile {
                name: "Autounattend.xml",
                data: xml.as_bytes(),
            },
            DiscFile {
                name: "setup-spacesd.ps1",
                // PowerShell 5 reads a BOM-less file as the ANSI code page;
                // the script is ASCII, so either way is the same text.
                data: ps1.as_bytes(),
            },
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::table::lookup;

    fn opts<'a>(v: &str, arch: WindowsArch, account: &'a str) -> UnattendOptions<'a> {
        UnattendOptions {
            version: lookup(v).unwrap(),
            arch,
            account,
            edition: None,
            evaluation_media: true,
            virtio: true,
            spacesd_version: "0.12.2",
        }
    }

    /// Every opening tag has its closing tag: the answer file is well formed
    /// enough that Setup will not reject it for syntax.
    fn balanced(xml: &str) {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(i) = rest.find('<') {
            let tail = &rest[i..];
            let end = tail.find('>').unwrap();
            let tag = &tail[1..end];
            rest = &tail[end + 1..];
            if tag.starts_with('?') || tag.ends_with('/') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                assert_eq!(stack.pop().as_deref(), Some(name), "closing {name}");
            } else {
                stack.push(tag.split_whitespace().next().unwrap().to_string());
            }
        }
        assert!(stack.is_empty(), "unclosed {stack:?}");
    }

    #[test]
    fn windows_11_evaluation_on_amd64() {
        let x = autounattend_xml(&opts("11", WindowsArch::Amd64, "User"));
        balanced(&x);
        assert!(x.contains("<Key>/IMAGE/INDEX</Key><Value>1</Value>"));
        assert!(
            !x.contains("ProductKey"),
            "evaluation media carries its key"
        );
        assert!(x.contains("BypassTPMCheck") && x.contains("BypassSecureBootCheck"));
        assert!(x.contains(r"E:\viostor\w11\amd64"));
        // The network adapter's driver goes into the installed system too.
        assert!(x.contains("Microsoft-Windows-PnpCustomizationsNonWinPE"));
        assert!(x.contains("<Name>User</Name>") && x.contains("<Username>User</Username>"));
        assert!(x.contains("<fDenyTSConnections>false"));
        assert!(!x.contains("arm64"));
    }

    #[test]
    fn server_is_selected_by_name() {
        let x = autounattend_xml(&opts("2025", WindowsArch::Amd64, "User"));
        balanced(&x);
        assert!(x.contains(
            "<Key>/IMAGE/NAME</Key><Value>Windows Server 2025 Standard Evaluation (Desktop Experience)</Value>"
        ));
        assert!(x.contains(r"\viostor\2k25\amd64"));
    }

    #[test]
    fn arm64_uses_arm64_components_and_drivers() {
        let x = autounattend_xml(&opts("11", WindowsArch::Arm64, "User"));
        balanced(&x);
        assert!(x.contains(r#"processorArchitecture="arm64""#));
        assert!(!x.contains(r#"processorArchitecture="amd64""#));
        assert!(x.contains(r"\viostor\w11\ARM64"));
    }

    #[test]
    fn your_consumer_iso_with_pro_gets_the_generic_key() {
        let mut o = opts("11", WindowsArch::Arm64, "User");
        o.evaluation_media = false;
        o.edition = Some("Windows 11 Pro");
        let x = autounattend_xml(&o);
        assert!(x.contains("<Key>/IMAGE/NAME</Key><Value>Windows 11 Pro</Value>"));
        assert!(x.contains(PRO_GENERIC_KEY));
        o.edition = Some("Windows 11 Enterprise");
        assert!(!autounattend_xml(&o).contains("ProductKey"));
    }

    #[test]
    fn hyper_v_skips_virtio() {
        let mut o = opts("11", WindowsArch::Amd64, "User");
        o.virtio = false;
        assert!(!autounattend_xml(&o).contains("viostor"));
    }

    #[test]
    fn the_account_name_is_checked_and_defaulted() {
        assert_eq!(account_name(None, None).unwrap(), "User");
        assert_eq!(account_name(None, Some("Docker")).unwrap(), "Docker");
        assert_eq!(account_name(Some("Bob"), Some("Docker")).unwrap(), "Bob");
        assert_eq!(account_name(None, Some("  ")).unwrap(), "User");
        for bad in ["a<b", "Administrator", "x y", "waytoolongaccountname123"] {
            assert!(account_name(Some(bad), None).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_setup_script_has_what_the_dogfooding_needed() {
        let s = setup_script(WindowsArch::Arm64, "0.12.2");
        assert!(!s.contains("__"), "placeholders left: {s}");
        // The native ARM64 build first, the x64 one (emulated) as the fallback.
        let native = s.find("cua-spacesd-aarch64-pc-windows-msvc.exe").unwrap();
        let x64 = s.find("cua-spacesd-x86_64-pc-windows-msvc.exe").unwrap();
        assert!(native < x64);
        assert!(s.contains("cua-spacesd-v0.12.2/"));
        // The program firewall rule (without it Windows prompts and blocks the daemon).
        assert!(s.contains("-Program $Exe"));
        // The disk guard.
        assert!(s.contains("Cua-DiskGuard") && s.contains("disk-low.txt"));
        // Bootstrap mode on every interface, as the published Windows image runs it.
        assert!(s.contains("serve --listen 0.0.0.0:$Port --insecure-bootstrap --token-file"));
        assert!(s.contains("$Port = '3211'"));
        // Sealed and powered off.
        assert!(s.contains("machine-seal.key") && s.contains("shutdown.exe /s"));
        assert!(s.is_ascii());
    }

    #[test]
    fn the_newest_release_with_a_windows_build_is_found() {
        let json = r#"[
          {"tag_name":"cua-spacesd-v0.12.3","draft":true,"prerelease":false,"assets":[{"name":"cua-spacesd-x86_64-pc-windows-msvc.exe"}]},
          {"tag_name":"cua-spaces-v0.30.0","draft":false,"prerelease":false,"assets":[]},
          {"tag_name":"cua-spacesd-v0.12.2","draft":false,"prerelease":false,"assets":[{"name":"cua-spacesd-x86_64-pc-windows-msvc.exe"}]},
          {"tag_name":"cua-spacesd-v0.12.1","draft":false,"prerelease":false,"assets":[]}]"#;
        assert_eq!(latest_spacesd(json).as_deref(), Some("0.12.2"));
        assert_eq!(latest_spacesd("not json"), None);
    }

    #[test]
    fn the_data_disc_carries_both_files() {
        let disc = data_disc(&opts("11", WindowsArch::Amd64, "User"));
        let text = String::from_utf8_lossy(&disc);
        assert!(text.contains("CD001"));
        assert!(text.contains("<unattend") && text.contains("setup-spacesd.ps1"));
        // `CUA_DUMP_DISC=<path> cargo test` writes the disc for a look with isoinfo / 7z.
        if let Ok(path) = std::env::var("CUA_DUMP_DISC") {
            std::fs::write(path, &disc).unwrap();
        }
    }
}
