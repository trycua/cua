"""Build orchestrator — resolve or build base images, apply user layers, create session overlays.

Implements the 3-layer qcow2 chain:

  base (OS + cua-spacesd)  →  user overlay (layers)  →  session overlay (ephemeral)

Usage::

    from cua_sandbox.builder.build import resolve_image_disk

    # Returns the disk path to boot — handles base building, layer caching, overlays
    disk_path = await resolve_image_disk(image, name="my-sandbox")
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import re
import shlex
from pathlib import Path
from typing import Optional

from cua_sandbox.builder.overlay import (
    base_image_path,
    create_overlay,
    layers_hash,
    session_overlay_path,
    user_image_path,
)
from cua_sandbox.image import Image

logger = logging.getLogger(__name__)

# cua-spacesd release artifacts. The Linux name matches
# libs/cua-spacesd/packaging/install.sh; the Windows name is a placeholder
# until the release workflow publishes a Windows build.
SPACESD_VERSION = "0.1.0"
SPACESD_PORT = 3211
SPACESD_RELEASE_URL = (
    "https://github.com/trycua/cua/releases/download/"
    "cua-spacesd-v{version}/cua-spacesd-{target}.{ext}"
)

# Base-layer setup for Windows: runs inside the VM at first logon to install
# cua-spacesd and a hidden logon task serving 0.0.0.0:3211. The token lives
# in C:\ProgramData\cua\env-token (generated) and reaches the driver through
# the environment, never the command line.
SETUP_SPACESD_PS1 = (
    r'''
$ErrorActionPreference = 'Continue'

$Version = '__VERSION__'
$Port = '__PORT__'
$InstallDir = 'C:\Program Files\cua-spacesd'
$DataDir = 'C:\ProgramData\cua'
$TokenFile = Join-Path $DataDir 'env-token'
$Url = '__WINDOWS_URL__'

New-Item -ItemType Directory -Force -Path $InstallDir, $DataDir | Out-Null
$zip = Join-Path $env:TEMP 'cua-spacesd.zip'
for ($i = 1; $i -le 5; $i++) {
    try {
        Invoke-WebRequest -Uri $Url -OutFile $zip -UseBasicParsing
        Expand-Archive -Path $zip -DestinationPath $InstallDir -Force
        break
    } catch { Start-Sleep -Seconds ($i * 5) }
}
$Exe = Join-Path $InstallDir 'cua-spacesd.exe'

if (!(Test-Path $TokenFile)) {
    $bytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    Set-Content -Path $TokenFile -Value (($bytes | ForEach-Object { $_.ToString('x2') }) -join '') -NoNewline -Encoding ASCII
}

netsh advfirewall firewall add rule name="cua-spacesd $Port" dir=in action=allow protocol=TCP localport=$Port

$StartScript = Join-Path $InstallDir 'start-spacesd.ps1'
@"
`$env:CUA_ENV_TOKEN = (Get-Content -Raw '$TokenFile').Trim()
while (`$true) {
    & '$Exe' --listen '0.0.0.0:$Port'
    Start-Sleep -Seconds 5
}
"@ | Set-Content -Path $StartScript -Encoding UTF8

$VbsWrapper = Join-Path $InstallDir 'start-spacesd-hidden.vbs'
@"
Set objShell = CreateObject("WScript.Shell")
objShell.Run "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""$StartScript""", 0, False
"@ | Set-Content -Path $VbsWrapper -Encoding ASCII

$TaskName = "Cua-Spacesd"
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

# Start the driver now too
Start-Process wscript.exe -ArgumentList "`"$VbsWrapper`""

Write-Host "cua-spacesd setup complete"
'''.replace("__VERSION__", SPACESD_VERSION)
    .replace("__PORT__", str(SPACESD_PORT))
    .replace(
        "__WINDOWS_URL__",
        SPACESD_RELEASE_URL.format(
            version=SPACESD_VERSION, target="x86_64-pc-windows-msvc", ext="zip"
        ),
    )
)

# Linux equivalent: install the release tarball and a systemd unit on :3211.
SETUP_SPACESD_SH = r"""#!/bin/bash
set -e

VERSION="${CUA_SPACESD_VERSION:-${CUA_GUESTD_VERSION:-${CUA_ENV_DRIVER_VERSION:-__VERSION__}}}"
PORT="${CUA_ENV_PORT:-__PORT__}"
case "$(uname -m)" in
    x86_64|amd64) ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
URL="https://github.com/trycua/cua/releases/download/cua-spacesd-v${VERSION}/cua-spacesd-${ARCH}-unknown-linux-gnu.tar.gz"
curl -fsSL "$URL" | sudo tar -xz -C /usr/local/bin cua-spacesd
sudo chmod 0755 /usr/local/bin/cua-spacesd
sudo ln -sfn cua-spacesd /usr/local/bin/cua-guestd  # older names, one release
sudo ln -sfn cua-spacesd /usr/local/bin/cua-env-driver

sudo install -d -m 0755 /etc/cua
if [ ! -s /etc/cua/env-token ]; then
    head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' | sudo tee /etc/cua/env-token >/dev/null
fi
sudo chown root:"$USER" /etc/cua/env-token
sudo chmod 0640 /etc/cua/env-token

sudo tee /etc/systemd/system/cua-spacesd.service > /dev/null <<UNIT
[Unit]
Description=cua-spacesd (in-sandbox daemon, :$PORT)
After=network.target

[Service]
Type=simple
User=$USER
ExecStart=/bin/sh -c 'CUA_ENV_TOKEN="\$(cat /etc/cua/env-token)" exec /usr/local/bin/cua-spacesd --listen 0.0.0.0:$PORT'
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
Alias=cua-env-driver.service
Alias=cua-guestd.service
UNIT

sudo systemctl daemon-reload
sudo systemctl enable --now cua-spacesd

echo "cua-spacesd setup complete"
""".replace("__VERSION__", SPACESD_VERSION).replace("__PORT__", str(SPACESD_PORT))


async def ensure_base_image(
    os_type: str,
    version: str,
    *,
    windows_iso: Optional[str] = None,
    product_key: Optional[str] = None,
    force: bool = False,
) -> Path:
    """Ensure the base image (OS + cua-spacesd) exists. Build if needed.

    Returns the path to the base qcow2.
    """
    base_path = base_image_path(os_type, version)

    _MIN_BASE_SIZE = {
        "windows": 1 * 1024 * 1024 * 1024,  # 1 GB — incomplete install guard
        "linux": 100 * 1024 * 1024,  # 100 MB
    }
    if base_path.exists() and not force:
        min_size = _MIN_BASE_SIZE.get(os_type, 0)
        if base_path.stat().st_size >= min_size:
            logger.info(f"Using cached base image: {base_path}")
            return base_path
        logger.warning(
            f"Cached base image {base_path} is too small "
            f"({base_path.stat().st_size} bytes < {min_size}), rebuilding."
        )
        base_path.unlink()

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
    """Build Windows base: unattend install + cua-spacesd."""
    from cua_sandbox.registry.qemu_builder import (
        QEMUImageConfig,
        build_image,
    )

    config = QEMUImageConfig(guest_os="windows", version=version)
    work_dir = base_path.parent / "build"

    # Phase 1: Unattended Windows install
    raw_disk = build_image(
        config, windows_iso=windows_iso, work_dir=work_dir, product_key=product_key
    )

    # Phase 2: cua-spacesd is installed by the Autounattend FirstLogonCommand
    # (SETUP_SPACESD_PS1 on the unattend ISO) during the install above.
    logger.info("Finalizing base image with cua-spacesd...")
    import shutil

    temp_disk = work_dir / "temp-boot.qcow2"
    shutil.copy2(raw_disk, temp_disk)

    logger.info("Base image built at %s (cua-spacesd via FirstLogonCommand).", raw_disk)

    # Move the built disk to the base path
    base_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(str(raw_disk), str(base_path))

    return base_path


async def _build_linux_base(version: str, base_path: Path) -> Path:
    """Build Linux base: install from cloud image + cua-spacesd."""
    raise NotImplementedError("Linux base image building not yet implemented")


_ENV_NAME_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def has_build_work(image: Image) -> bool:
    """Whether anything on this image has to be baked into a user disk.

    ``.env()`` and ``.copy()`` count: they are as much a part of the built image
    as ``.apt_install()`` is, and a layers-only check silently drops them.
    """
    return bool(image._layers or image._env or image._files)


def build_hash(image: Image) -> str:
    """Stable cache key for the user image built from this spec.

    Everything baked in has to participate, or two images differing only by an
    env var or a copied file would share one cached disk. File *contents* are
    hashed, not just paths, so editing a copied file rebuilds the image.

    Layers-only images keep their historical :func:`layers_hash` key so existing
    caches stay valid.
    """
    layers = list(image._layers)
    if not image._env and not image._files:
        return layers_hash(layers)

    files: list[list[str]] = []
    for src, dst in image._files:
        digest = ""
        try:
            digest = hashlib.sha256(Path(src).read_bytes()).hexdigest()
        except OSError:
            # Unreadable now — let the copy layer raise the real error at build time.
            digest = f"<unreadable:{src}>"
        files.append([dst, digest])

    raw = json.dumps(
        {"layers": layers, "env": [list(e) for e in image._env], "files": files},
        sort_keys=True,
    ).encode()
    return hashlib.sha256(raw).hexdigest()[:16]


async def _apply_env(executor, image: Image) -> None:
    """Bake ``.env()`` variables into the image being built.

    Linux/macOS get a sourceable ``/etc/profile.d/cua-env.sh`` (the same file the
    Docker and Lume runtimes write, and the one ``LayerExecutor`` sources before
    each ``run`` layer). Windows gets machine-scoped ``setx`` variables.
    """
    if not image._env:
        return

    for key, _ in image._env:
        if not _ENV_NAME_RE.fullmatch(key):
            raise ValueError(f"Unsafe env var name: {key!r}")

    if image.os_type == "windows":
        for key, value in image._env:
            await executor.run_command(f'setx {key} "{value}" /M')
        return

    sudo = "echo lume | sudo -S" if image.os_type == "macos" else "sudo"
    await executor.run_command(
        f"printf '#!/bin/sh\\n' | {sudo} tee /etc/profile.d/cua-env.sh > /dev/null"
    )
    for key, value in image._env:
        await executor.run_command(
            f"printf 'export {key}=%s\\n' {shlex.quote(value)} "
            f"| {sudo} tee -a /etc/profile.d/cua-env.sh > /dev/null"
        )


async def build_user_image(
    image: Image,
    base_path: Path,
    *,
    force: bool = False,
) -> Path:
    """Build a user image by applying Image layers on top of the base.

    Creates a qcow2 overlay backed by base_path, boots it, runs all layers
    through cua-spacesd, then shuts down. The overlay is the user image.

    Returns the path to the user image qcow2.
    """
    if not has_build_work(image):
        # Nothing to bake in — just use the base
        return base_path

    lhash = build_hash(image)
    user_path = user_image_path(image.os_type, image.version, lhash)

    if user_path.exists() and not force:
        logger.info(f"Using cached user image: {user_path} (hash={lhash})")
        return user_path

    logger.info(f"Building user image (hash={lhash}, {len(image._layers)} layers)...")

    # Create overlay on top of base
    create_overlay(base_path, user_path)

    # Boot the overlay and execute layers
    from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

    # The legacy launcher boots user_path in place, so the layers land in it
    # (the SDK backend would put a throwaway instance overlay on top).
    runtime = QEMUBaremetalRuntime(api_port=18098, memory_mb=8192, cpu_count=4, use_sdk=False)

    build_image = Image.from_file(str(user_path), os_type=image.os_type)
    try:
        info = await runtime.start(build_image, f"cua-build-{lhash}")

        # Execute layers through cua-spacesd (baked into the base image).
        # os_type matters: the executor wraps `run` commands per-OS (sudo bash
        # on Linux, plain cmd on Windows). The driver starts late on a fresh
        # boot, so the executor waits for it.
        from cua_sandbox.builder.executor import LayerExecutor

        executor = LayerExecutor(
            f"http://{info.host}:{info.api_port}", os_type=image.os_type, ready_timeout=900
        )

        # Env first, so copied files and run layers can reference the variables.
        await _apply_env(executor, image)
        # Files before layers, so later run layers can reference copied files.
        for src, dst in image._files:
            await executor.execute_layers([{"type": "copy", "src": src, "dst": dst}])
        if image._layers:
            await executor.execute_layers(list(image._layers))

        # Shut down cleanly
        logger.info("Layers applied, shutting down build VM...")
        try:
            await executor.run_command(
                "shutdown /s /t 5" if image.os_type == "windows" else "sudo shutdown -h now"
            )
        except Exception:
            pass
        await asyncio.sleep(10)
    finally:
        await runtime.stop(f"cua-build-{lhash}")

    logger.info(f"User image built: {user_path}")

    # Save layer metadata
    meta_path = user_path.with_suffix(".json")
    meta_path.write_text(
        json.dumps(
            {
                "os_type": image.os_type,
                "version": image.version,
                "layers": list(image._layers),
                "env": [list(e) for e in image._env],
                "files": [list(f) for f in image._files],
                "base": str(base_path),
                "hash": lhash,
            },
            indent=2,
        )
    )

    return user_path


async def resolve_backing_disk(image: Image) -> Path:
    """Resolve the disk a local session overlays, preferring the registry containerDisk.

    Built-in images (and explicit ``Image.from_registry(...)`` refs) map to a
    KubeVirt containerDisk in the registry — the very image Fleet cloud boots. Pulling
    it keeps local QEMU runs on the same disk as the cloud instead of a separately
    built base. Images with no registry counterpart fall back to the local base build.
    """
    from cua_sandbox.image import cloud_registry_image

    ref = cloud_registry_image(image)
    if ref is not None:
        from cua_sandbox._sdk import local_runtime, native

        logger.info(f"Resolving containerDisk {ref} for local session...")
        n = native()
        try:
            # The SDK resolves the VM variant (a canonical image's -disk
            # sibling), pulls it into its image cache and returns the disk.
            pulled = await local_runtime().local().pull_image(f"vm:{ref}")
            return Path(pulled.location)
        except (n.CuaError.Unsupported, n.CuaError.NotFound) as exc:
            # Not a containerDisk (e.g. a lume/tart/qemu-format VM image) — fall back.
            logger.info(f"{ref} is not a containerDisk ({exc}); falling back to base image")

    return await ensure_base_image(image.os_type, image.version)


async def create_session_disk(
    image: Image,
    name: str,
    *,
    base_disk: Optional[Path] = None,
) -> Path:
    """Create a session overlay for a sandbox run.

    If the image has layers and a cached user image exists, overlay on that.
    Otherwise overlay on the registry containerDisk (the same disk Fleet cloud
    boots) or on a locally built base image. If the image carries a direct disk
    path and no layers, that disk is returned as-is (no overlay).

    Returns the disk path to boot.
    """
    # If image has a direct disk path and nothing to bake in, use it directly
    if image._disk_path and not has_build_work(image):
        return Path(image._disk_path)

    # Determine the backing disk
    if base_disk:
        backing = base_disk
    elif image._disk_path:
        backing = Path(image._disk_path)
    else:
        backing = await resolve_backing_disk(image)

    # If anything has to be baked in, check for a cached user image
    if has_build_work(image):
        lhash = build_hash(image)
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
