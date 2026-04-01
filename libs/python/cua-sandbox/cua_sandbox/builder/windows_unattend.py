"""Shared Windows unattended install helpers.

Used by both the QEMU builder and Hyper-V runtime to create Windows images
from scratch via Autounattend.xml + CUA server setup script.
"""

from __future__ import annotations

import logging
import shutil
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)


# ── Download helpers ────────────────────────────────────────────────────────


def download_file(url: str, dest: Path, description: str = "") -> Path:
    """Download a file with progress, skipping if it already exists."""
    if dest.exists():
        logger.info(f"Already downloaded: {dest}")
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    logger.info(f"Downloading {description or url} → {dest}")
    import urllib.request

    urllib.request.urlretrieve(url, str(dest))
    logger.info(f"Downloaded {dest.stat().st_size / 1024 / 1024:.0f} MB")
    return dest


def _resolve_server_eval_url(server_version: str, culture: str = "en-us", country: str = "US") -> str:
    """Resolve Windows Server evaluation ISO download URL from Microsoft's eval center.

    Adapted from quickemu/quickget's download_windows_server() which is itself
    adapted from the Mido project (https://github.com/ElliotKillick/Mido).

    Args:
        server_version: e.g. "windows-server-2022", "windows-server-2025"
        culture: Language culture code, e.g. "en-us"
        country: Country code, e.g. "US"

    Returns:
        Direct download URL for the ISO.
    """
    import re
    import urllib.request

    eval_url = f"https://www.microsoft.com/en-us/evalcenter/download-{server_version}"
    logger.info(f"Parsing Microsoft Eval Center: {eval_url}")

    req = urllib.request.Request(eval_url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        html = resp.read().decode("utf-8", errors="replace")

    # Extract go.microsoft.com/fwlink links with the matching culture
    pattern = rf"https://go\.microsoft\.com/fwlink/p/\?LinkID=\d+&clcid=0x[0-9a-f]+&culture={re.escape(culture)}&country={re.escape(country)}"
    links = re.findall(pattern, html)

    if not links:
        # Fallback: try any English link
        pattern_fallback = r"https://go\.microsoft\.com/fwlink/p/\?LinkID=\d+&clcid=0x[0-9a-f]+&culture=en-us&country=US"
        links = re.findall(pattern_fallback, html)

    if not links:
        raise RuntimeError(
            f"Could not find download link on {eval_url}. "
            f"Microsoft may have changed the page layout."
        )

    # First link is typically the x64 ISO
    download_link = links[0]
    logger.info(f"Resolved download link: {download_link}")

    # Follow redirect to get the actual ISO URL
    req2 = urllib.request.Request(download_link, method="HEAD", headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req2, timeout=30) as resp2:
        final_url = resp2.url

    logger.info(f"Final ISO URL: {final_url}")
    return final_url


def download_windows_iso(
    version: str = "11",
    dest: Optional[Path] = None,
    iso_path: Optional[str] = None,
) -> Path:
    """Get a Windows ISO. If iso_path is given, use it directly."""
    if iso_path:
        p = Path(iso_path)
        if not p.exists():
            raise FileNotFoundError(f"Windows ISO not found: {iso_path}")
        return p

    dest_dir = dest or Path.home() / ".cua" / "cua-sandbox" / "iso"
    dest_dir.mkdir(parents=True, exist_ok=True)
    iso_file = dest_dir / f"windows-{version}.iso"

    if iso_file.exists():
        logger.info(f"Using cached ISO: {iso_file}")
        return iso_file

    if version == "11":
        url = "https://go.microsoft.com/fwlink/?linkid=2334167&clcid=0x409"
    elif version == "10":
        url = "https://go.microsoft.com/fwlink/?LinkId=691209"
    elif version in ("server-2022", "2022"):
        version = "server-2022"
        iso_file = dest_dir / f"windows-{version}.iso"
        if iso_file.exists():
            logger.info(f"Using cached ISO: {iso_file}")
            return iso_file
        url = _resolve_server_eval_url("windows-server-2022")
    elif version in ("server-2025", "2025"):
        version = "server-2025"
        iso_file = dest_dir / f"windows-{version}.iso"
        if iso_file.exists():
            logger.info(f"Using cached ISO: {iso_file}")
            return iso_file
        url = _resolve_server_eval_url("windows-server-2025")
    elif version in ("server-2019", "2019"):
        version = "server-2019"
        iso_file = dest_dir / f"windows-{version}.iso"
        if iso_file.exists():
            logger.info(f"Using cached ISO: {iso_file}")
            return iso_file
        url = _resolve_server_eval_url("windows-server-2019")
    else:
        raise ValueError(
            f"Cannot auto-download Windows {version}. "
            f"Supported: 10, 11, server-2022, server-2025, server-2019. "
            f"Or provide --iso-path to use your own ISO."
        )

    logger.info(f"Downloading Windows {version} Enterprise Evaluation ISO...")
    logger.info("This is ~6GB and may take a while.")
    return download_file(url, iso_file, f"Windows {version} ISO")


# ── Autounattend.xml generation ─────────────────────────────────────────────


def generate_autounattend_xml(
    product_key: Optional[str] = None,
    *,
    include_virtio_drivers: bool = True,
    version: str = "11",
) -> str:
    """Generate a Windows Autounattend.xml for unattended installation.

    Args:
        product_key: Windows product key. If None, uses generic key for edition.
        include_virtio_drivers: Include VirtIO driver paths in WinPE pass.
            Set False for Hyper-V (has enlightened drivers built-in).
        version: Windows version - "11", "10", "server-2022", "server-2025", etc.
    """
    is_server = version.startswith("server-")

    # KMS Generic Volume License Keys (GVLKs) for Server editions
    # https://learn.microsoft.com/en-us/windows-server/get-started/kms-client-activation-keys
    _server_gvlks = {
        "server-2025": "TVRH6-WHNXV-R9WG3-9XRFY-MY832",  # Standard
        "server-2022": "VDYBN-27WPP-V4HQT-9VMD4-VMK7H",  # Standard
        "server-2019": "N69G4-B89J2-4G8F4-WWYCC-J464C",  # Standard
    }

    key_section = ""
    if product_key:
        key_section = f"""<ProductKey>
          <Key>{product_key}</Key>
        </ProductKey>"""
    elif is_server:
        # Server editions: omit key for eval, or use GVLK for volume license
        gvlk = _server_gvlks.get(version)
        if gvlk:
            key_section = f"""<ProductKey>
          <Key>{gvlk}</Key>
        </ProductKey>"""
        # If no GVLK found, omit key entirely (eval mode)
    else:
        key_section = """<ProductKey>
          <Key>VK7JG-NPHTM-C97JM-9MPGT-3V66T</Key>
        </ProductKey>"""

    # Server uses image index 2 (Standard with Desktop Experience)
    # Desktop uses index 1
    image_index = "2" if is_server else "1"

    virtio_section = ""
    if include_virtio_drivers:
        virtio_section = """
    <component name="Microsoft-Windows-PnpCustomizationsWinPE" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <DriverPaths>
        <PathAndCredentials wcm:action="add" wcm:keyValue="1">
          <Path>D:\\viostor\\w11\\amd64</Path>
        </PathAndCredentials>
        <PathAndCredentials wcm:action="add" wcm:keyValue="2">
          <Path>E:\\viostor\\w11\\amd64</Path>
        </PathAndCredentials>
        <PathAndCredentials wcm:action="add" wcm:keyValue="3">
          <Path>F:\\viostor\\w11\\amd64</Path>
        </PathAndCredentials>
        <PathAndCredentials wcm:action="add" wcm:keyValue="4">
          <Path>D:\\NetKVM\\w11\\amd64</Path>
        </PathAndCredentials>
        <PathAndCredentials wcm:action="add" wcm:keyValue="5">
          <Path>E:\\NetKVM\\w11\\amd64</Path>
        </PathAndCredentials>
        <PathAndCredentials wcm:action="add" wcm:keyValue="6">
          <Path>F:\\NetKVM\\w11\\amd64</Path>
        </PathAndCredentials>
      </DriverPaths>
    </component>"""

    return f"""<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="windowsPE">
    <component name="Microsoft-Windows-International-Core-WinPE" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <SetupUILanguage>
        <UILanguage>en-US</UILanguage>
      </SetupUILanguage>
      <InputLocale>0409:00000409</InputLocale>
      <SystemLocale>en-US</SystemLocale>
      <UILanguage>en-US</UILanguage>
      <UserLocale>en-US</UserLocale>
    </component>{virtio_section}
    <component name="Microsoft-Windows-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <DiskConfiguration>
        <Disk wcm:action="add">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add">
              <Order>1</Order>
              <Type>EFI</Type>
              <Size>128</Size>
            </CreatePartition>
            <CreatePartition wcm:action="add">
              <Order>2</Order>
              <Type>MSR</Type>
              <Size>128</Size>
            </CreatePartition>
            <CreatePartition wcm:action="add">
              <Order>3</Order>
              <Type>Primary</Type>
              <Extend>true</Extend>
            </CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add">
              <Order>1</Order>
              <PartitionID>1</PartitionID>
              <Label>System</Label>
              <Format>FAT32</Format>
            </ModifyPartition>
            <ModifyPartition wcm:action="add">
              <Order>2</Order>
              <PartitionID>2</PartitionID>
            </ModifyPartition>
            <ModifyPartition wcm:action="add">
              <Order>3</Order>
              <PartitionID>3</PartitionID>
              <Label>Windows</Label>
              <Letter>C</Letter>
              <Format>NTFS</Format>
            </ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall>
        <OSImage>
          <InstallTo>
            <DiskID>0</DiskID>
            <PartitionID>3</PartitionID>
          </InstallTo>
          <InstallFrom>
            <MetaData wcm:action="add">
              <Key>/IMAGE/INDEX</Key>
              <Value>{image_index}</Value>
            </MetaData>
          </InstallFrom>
          <InstallToAvailablePartition>false</InstallToAvailablePartition>
        </OSImage>
      </ImageInstall>
      <UserData>
        <AcceptEula>true</AcceptEula>
        <FullName>User</FullName>
        <Organization>CUA</Organization>
        {key_section}
      </UserData>
      <EnableFirewall>false</EnableFirewall>
      <RunSynchronous>
        <RunSynchronousCommand wcm:action="add">
          <Order>1</Order>
          <Path>reg.exe add "HKLM\\SYSTEM\\Setup\\LabConfig" /v BypassTPMCheck /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add">
          <Order>2</Order>
          <Path>reg.exe add "HKLM\\SYSTEM\\Setup\\LabConfig" /v BypassSecureBootCheck /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add">
          <Order>3</Order>
          <Path>reg.exe add "HKLM\\SYSTEM\\Setup\\LabConfig" /v BypassRAMCheck /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add">
          <Order>4</Order>
          <Path>reg.exe add "HKLM\\SYSTEM\\Setup\\MoSetup" /v AllowUpgradesWithUnsupportedTPMOrCPU /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>
      </RunSynchronous>
    </component>
  </settings>
  <settings pass="offlineServicing">
    <component name="Microsoft-Windows-LUA-Settings" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <EnableLUA>false</EnableLUA>
    </component>
  </settings>
  <settings pass="specialize">
    <component name="Microsoft-Windows-Deployment" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <RunSynchronous>
        <RunSynchronousCommand wcm:action="add">
          <Order>1</Order>
          <Path>reg.exe add "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\OOBE" /v BypassNRO /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>
      </RunSynchronous>
    </component>
    <component name="Microsoft-Windows-TerminalServices-LocalSessionManager" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <fDenyTSConnections>false</fDenyTSConnections>
    </component>
    <component name="Microsoft-Windows-TerminalServices-RDP-WinStationExtensions" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <UserAuthentication>0</UserAuthentication>
    </component>
    <component name="Networking-MPSSVC-Svc" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <FirewallGroups>
        <FirewallGroup wcm:action="add" wcm:keyValue="RemoteDesktop">
          <Active>true</Active>
          <Profile>all</Profile>
          <Group>@FirewallAPI.dll,-28752</Group>
        </FirewallGroup>
      </FirewallGroups>
    </component>
  </settings>
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add">
            <Name>User</Name>
            <Group>Administrators</Group>
            <Password>
              <Value />
              <PlainText>true</PlainText>
            </Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>
      <AutoLogon>
        <Username>User</Username>
        <Enabled>true</Enabled>
        <LogonCount>65432</LogonCount>
        <Password>
          <Value />
          <PlainText>true</PlainText>
        </Password>
      </AutoLogon>
      <Display>
        <ColorDepth>32</ColorDepth>
        <HorizontalResolution>1280</HorizontalResolution>
        <VerticalResolution>720</VerticalResolution>
      </Display>
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
        <SynchronousCommand wcm:action="add">
          <Order>1</Order>
          <CommandLine>cmd /C POWERCFG -H OFF</CommandLine>
          <Description>Disable Hibernation</Description>
        </SynchronousCommand>
        <SynchronousCommand wcm:action="add">
          <Order>2</Order>
          <CommandLine>cmd /C POWERCFG -X -monitor-timeout-ac 0</CommandLine>
          <Description>Disable monitor blanking</Description>
        </SynchronousCommand>
        <SynchronousCommand wcm:action="add">
          <Order>3</Order>
          <CommandLine>cmd /C POWERCFG -X -standby-timeout-ac 0</CommandLine>
          <Description>Disable Sleep</Description>
        </SynchronousCommand>
        <SynchronousCommand wcm:action="add">
          <Order>4</Order>
          <CommandLine>reg.exe add "HKLM\\SOFTWARE\\Policies\\Microsoft\\Windows\\WindowsUpdate\\AU" /v "NoAutoUpdate" /t REG_DWORD /d 1 /f</CommandLine>
          <Description>Disable Windows Update</Description>
        </SynchronousCommand>
        <SynchronousCommand wcm:action="add">
          <Order>5</Order>
          <CommandLine>powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "foreach ($d in [System.IO.DriveInfo]::GetDrives()) {{ $f = Join-Path $d.Name 'setup-cua-server.ps1'; if (Test-Path $f) {{ &amp; $f; break }} }}"</CommandLine>
          <Description>Install CUA Computer Server</Description>
        </SynchronousCommand>
      </FirstLogonCommands>
    </component>
  </settings>
</unattend>"""


def create_unattend_iso(
    work_dir: Path,
    product_key: Optional[str] = None,
    *,
    include_virtio_drivers: bool = True,
    include_startup_nsh: bool = True,
    version: str = "11",
) -> Path:
    """Create a small data-only ISO containing Autounattend.xml + setup script.

    Args:
        include_virtio_drivers: Include VirtIO driver paths (QEMU needs them, Hyper-V doesn't).
        include_startup_nsh: Include UEFI shell startup.nsh (QEMU/OVMF needs it, Hyper-V doesn't).
        version: Windows version for autounattend generation.
    """
    unattend_iso = work_dir / "unattend.iso"
    if unattend_iso.exists():
        logger.info(f"Using cached unattend ISO: {unattend_iso}")
        return unattend_iso

    xml = generate_autounattend_xml(
        product_key, include_virtio_drivers=include_virtio_drivers, version=version
    )
    xml_path = work_dir / "Autounattend.xml"
    xml_path.write_text(xml, encoding="utf-8")

    from cua_sandbox.builder.build import SETUP_COMPUTER_SERVER_PS1

    setup_path = work_dir / "setup-cua-server.ps1"
    setup_path.write_text(SETUP_COMPUTER_SERVER_PS1, encoding="utf-8")

    iso_src = work_dir / "unattend-iso"
    iso_src.mkdir(parents=True, exist_ok=True)
    shutil.copy2(xml_path, iso_src / "Autounattend.xml")
    shutil.copy2(setup_path, iso_src / "setup-cua-server.ps1")

    if include_startup_nsh:
        startup_nsh = work_dir / "startup.nsh"
        startup_nsh.write_text("FS0:\\EFI\\BOOT\\BOOTX64.EFI\n", encoding="utf-8")
        shutil.copy2(startup_nsh, iso_src / "startup.nsh")

    _create_data_iso(iso_src, unattend_iso, "OEMDRV")
    logger.info(f"Created unattend ISO: {unattend_iso}")
    return unattend_iso


def _create_data_iso(src_dir: Path, dest_iso: Path, label: str = "OEMDRV") -> None:
    """Create a data-only ISO using pycdlib (pure Python, cross-platform)."""
    import pycdlib

    iso = pycdlib.PyCdlib()
    iso.new(joliet=3, vol_ident=label)

    for f in src_dir.iterdir():
        if not f.is_file():
            continue
        iso_name = f.name.upper().replace("-", "_")
        if "." in iso_name:
            base, ext = iso_name.rsplit(".", 1)
            iso9660_name = f"/{base[:8]}.{ext[:3]};1"
        else:
            iso9660_name = f"/{iso_name[:8]}.;1"
        iso.add_file(
            str(f),
            iso_path=iso9660_name,
            joliet_path=f"/{f.name}",
        )

    iso.write(str(dest_iso))
    iso.close()
