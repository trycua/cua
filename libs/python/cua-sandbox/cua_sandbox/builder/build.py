"""Build orchestrator — resolve or build base images, apply user layers, create session overlays.

Implements the 3-layer qcow2 chain:

  base (OS + computer-server)  →  user overlay (layers)  →  session overlay (ephemeral)

Usage::

    from cua_sandbox.builder.build import resolve_image_disk

    # Returns the disk path to boot — handles base building, layer caching, overlays
    disk_path = await resolve_image_disk(image, name="my-sandbox")
"""

from __future__ import annotations

import asyncio
import json
import logging
import platform as _plat
from pathlib import Path
from typing import Optional

from cua_sandbox.image import Image
from cua_sandbox.builder.overlay import (
    IMAGES_DIR,
    base_image_path,
    create_overlay,
    layers_hash,
    session_overlay_path,
    user_image_path,
)

logger = logging.getLogger(__name__)

# computer-server setup script — ported from setup-cua-server.ps1
# Runs inside the VM after first boot to install computer-server + scheduled task
SETUP_COMPUTER_SERVER_PS1 = r'''
$ErrorActionPreference = 'Continue'

# Install UV
powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
$uvPath = Join-Path $env:USERPROFILE ".local\bin"
$env:Path = "$uvPath;$env:Path"

# Create UV project
$ProjectDir = Join-Path $env:USERPROFILE "cua-server"
if (!(Test-Path (Join-Path $ProjectDir "pyproject.toml"))) {
    New-Item -ItemType Directory -Force -Path $ProjectDir | Out-Null
    & uv init --vcs none --no-readme --no-workspace --no-pin-python $ProjectDir
}

# Install cua-computer-server (no agent — VM only needs automation server)
& uv add --directory $ProjectDir cua-computer-server

# Install playwright + firefox
& uv add --directory $ProjectDir playwright
& uv run --directory $ProjectDir playwright install firefox

# Firewall rule for port 8000
netsh advfirewall firewall add rule name="CUA Computer Server" dir=in action=allow protocol=TCP localport=8000

# Create start script
$StartScript = Join-Path $ProjectDir "start-server.ps1"
@"
`$env:PYTHONUNBUFFERED = '1'
`$uvPath = Join-Path `$env:USERPROFILE '.local\bin'
`$env:Path = "`$uvPath;`$env:Path"
while (`$true) {
    & uv run --directory '$ProjectDir' python -m computer_server --port 8000
    Start-Sleep -Seconds 5
}
"@ | Set-Content -Path $StartScript -Encoding UTF8

# VBScript wrapper for hidden execution
$VbsWrapper = Join-Path $ProjectDir "start-server-hidden.vbs"
@"
Set objShell = CreateObject("WScript.Shell")
objShell.Run "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""$StartScript""", 0, False
"@ | Set-Content -Path $VbsWrapper -Encoding ASCII

# Scheduled task at logon
$TaskName = "Cua-Computer-Server"
$Username = $env:USERNAME
$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existing) { Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false }

$Action = New-ScheduledTaskAction -Execute "wscript.exe" -Argument "`"$VbsWrapper`""
$UserId = "$env:COMPUTERNAME\$Username"
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId
$Principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Highest
$Settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable `
    -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit (New-TimeSpan -Days 365) -Hidden

Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger `
    -Principal $Principal -Settings $Settings -Force | Out-Null

# Start the server now too
Start-Process wscript.exe -ArgumentList "`"$VbsWrapper`""

Write-Host "CUA Computer Server setup complete"
'''

# Linux equivalent
SETUP_COMPUTER_SERVER_SH = r'''#!/bin/bash
set -e

# Install uv
curl -LsSf https://astral.sh/uv/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"

# Create uv project
mkdir -p ~/cua-server
cd ~/cua-server
[ -f pyproject.toml ] || uv init --vcs none --no-readme --no-workspace --no-pin-python .

# Install cua-computer-server
uv add cua-computer-server "cua-agent[all]"

# Install playwright + firefox
uv add playwright
uv run playwright install firefox

# Create systemd service
sudo tee /etc/systemd/system/cua-computer-server.service > /dev/null <<UNIT
[Unit]
Description=CUA Computer Server
After=network.target

[Service]
Type=simple
User=$USER
WorkingDirectory=$HOME/cua-server
ExecStart=$HOME/.local/bin/uv run python -m computer_server --port 8000
Restart=always
RestartSec=5
Environment=PYTHONUNBUFFERED=1

[Install]
WantedBy=multi-user.target
UNIT

sudo systemctl daemon-reload
sudo systemctl enable --now cua-computer-server

echo "CUA Computer Server setup complete"
'''


async def ensure_base_image(
    os_type: str,
    version: str,
    *,
    windows_iso: Optional[str] = None,
    product_key: Optional[str] = None,
    force: bool = False,
) -> Path:
    """Ensure the base image (OS + computer-server) exists. Build if needed.

    Returns the path to the base qcow2.
    """
    base_path = base_image_path(os_type, version)

    if base_path.exists() and not force:
        logger.info(f"Using cached base image: {base_path}")
        return base_path

    logger.info(f"Building base image for {os_type} {version}...")

    if os_type == "windows":
        return await _build_windows_base(version, base_path, windows_iso, product_key)
    elif os_type == "linux":
        return await _build_linux_base(version, base_path)
    else:
        raise ValueError(f"Cannot build base image for os_type={os_type}")


async def _build_windows_base(
    version: str,
    base_path: Path,
    windows_iso: Optional[str],
    product_key: Optional[str],
) -> Path:
    """Build Windows base: unattend install + computer-server."""
    from cua_sandbox.registry.qemu_builder import (
        QEMUImageConfig,
        build_image,
    )

    config = QEMUImageConfig(guest_os="windows", version=version)
    work_dir = base_path.parent / "build"

    # Phase 1: Unattended Windows install
    raw_disk = build_image(config, windows_iso=windows_iso, work_dir=work_dir, product_key=product_key)

    # Phase 2: Boot and install computer-server
    logger.info("Installing computer-server into base image...")
    from cua_sandbox.builder.executor import LayerExecutor
    from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

    # Boot the raw disk to install computer-server
    # First we need to boot without expecting computer-server (it's not installed yet)
    # We use a temporary overlay so we can retry if needed
    import shutil
    temp_disk = work_dir / "temp-boot.qcow2"
    shutil.copy2(raw_disk, temp_disk)

    runtime = QEMUBaremetalRuntime(
        api_port=18099,
        memory_mb=config.ram_mb,
        cpu_count=config.cpu,
    )

    # Boot and wait for Windows to be accessible (but not computer-server — it's not installed)
    # We need to wait for Windows to boot then run the setup script
    # This is tricky because we don't have computer-server yet...
    # Use QMP or VNC to inject the setup script, or use the virtio-serial approach
    #
    # For now: the Autounattend.xml should include a FirstLogonCommand that
    # downloads and runs the setup script. Let's add that to the builder.
    logger.info(
        "Base image built at %s. Computer-server must be installed via "
        "Autounattend FirstLogonCommand or manual VNC session.",
        raw_disk,
    )

    # Move the built disk to the base path
    base_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(str(raw_disk), str(base_path))

    return base_path


async def _build_linux_base(version: str, base_path: Path) -> Path:
    """Build Linux base: install from cloud image + computer-server."""
    raise NotImplementedError("Linux base image building not yet implemented")


async def build_user_image(
    image: Image,
    base_path: Path,
    *,
    force: bool = False,
) -> Path:
    """Build a user image by applying Image layers on top of the base.

    Creates a qcow2 overlay backed by base_path, boots it, runs all layers
    via computer-server, then shuts down. The overlay is the user image.

    Returns the path to the user image qcow2.
    """
    if not image._layers:
        # No user layers — just use the base
        return base_path

    lhash = layers_hash(list(image._layers))
    user_path = user_image_path(image.os_type, image.version, lhash)

    if user_path.exists() and not force:
        logger.info(f"Using cached user image: {user_path} (hash={lhash})")
        return user_path

    logger.info(f"Building user image (hash={lhash}, {len(image._layers)} layers)...")

    # Create overlay on top of base
    create_overlay(base_path, user_path)

    # Boot the overlay and execute layers
    from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime
    from cua_sandbox.runtime.base import RuntimeInfo

    runtime = QEMUBaremetalRuntime(api_port=18098, memory_mb=8192, cpu_count=4)

    build_image = Image.from_file(str(user_path), os_type=image.os_type)
    try:
        info = await runtime.start(build_image, f"cua-build-{lhash}")

        # Execute layers via computer-server
        from cua_sandbox.builder.executor import LayerExecutor
        executor = LayerExecutor(f"http://{info.host}:{info.api_port}")
        await executor.execute_layers(list(image._layers))

        # Shut down cleanly
        logger.info("Layers applied, shutting down build VM...")
        try:
            await executor.run_command("shutdown /s /t 5" if image.os_type == "windows" else "sudo shutdown -h now")
        except Exception:
            pass
        await asyncio.sleep(10)
    finally:
        await runtime.stop(f"cua-build-{lhash}")

    logger.info(f"User image built: {user_path}")

    # Save layer metadata
    meta_path = user_path.with_suffix(".json")
    meta_path.write_text(json.dumps({
        "os_type": image.os_type,
        "version": image.version,
        "layers": list(image._layers),
        "base": str(base_path),
        "hash": lhash,
    }, indent=2))

    return user_path


async def create_session_disk(
    image: Image,
    name: str,
    *,
    base_disk: Optional[Path] = None,
) -> Path:
    """Create a session overlay for a sandbox run.

    If the image has layers and a cached user image exists, overlay on that.
    Otherwise overlay on the base image. If no base exists, returns the
    image's _disk_path directly (no overlay).

    Returns the disk path to boot.
    """
    # If image has a direct disk path and no layers, use it directly
    if image._disk_path and not image._layers:
        return Path(image._disk_path)

    # Determine the backing disk
    if base_disk:
        backing = base_disk
    elif image._disk_path:
        backing = Path(image._disk_path)
    else:
        # Auto-build base if it doesn't exist
        backing = await ensure_base_image(image.os_type, image.version)

    # If there are user layers, check for cached user image
    if image._layers:
        lhash = layers_hash(list(image._layers))
        user_disk = user_image_path(image.os_type, image.version, lhash)
        if user_disk.exists():
            backing = user_disk
        else:
            # Need to build the user image first
            backing = await build_user_image(image, backing)

    # Create ephemeral session overlay
    session_disk = session_overlay_path(name)
    if session_disk.exists():
        session_disk.unlink()
    create_overlay(backing, session_disk)

    return session_disk
