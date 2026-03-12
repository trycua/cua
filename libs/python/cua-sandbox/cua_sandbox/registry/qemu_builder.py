"""Build QEMU Windows VM images and push/pull as OCI artifacts.

Workflow:
  1. Download Windows ISO from Microsoft (or use a provided path)
  2. Download virtio-win.iso from Fedora
  3. Create qcow2 disk + unattended install ISO
  4. Run unattended Windows install via QEMU
  5. Package the resulting qcow2 as an OCI image and push to registry

Usage:
  python -m cua_sandbox.registry.qemu_builder build \\
      --ref ghcr.io/trycua/cua-qemu-windows-vm:latest \\
      --windows-version 11 \\
      --disk-size 64

  python -m cua_sandbox.registry.qemu_builder push \\
      --ref ghcr.io/trycua/cua-qemu-windows-vm:latest \\
      --disk /path/to/disk.qcow2
"""

from __future__ import annotations

import gzip
import hashlib
import json
import logging
import shutil
import subprocess
import tempfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Optional

from cua_sandbox.builder.windows_unattend import (
    create_unattend_iso,
    download_file,
    download_windows_iso,
    generate_autounattend_xml,
    _create_data_iso,
)

logger = logging.getLogger(__name__)

WORK_DIR = Path.home() / ".cua" / "cua-sandbox" / "qemu-builder"
VIRTIO_ISO_URL = "https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso"
CHUNK_SIZE = 500 * 1024 * 1024  # 500 MB chunks for OCI layers


@dataclass
class QEMUImageConfig:
    """VM configuration stored as the OCI config blob."""
    guest_os: str = "windows"
    version: str = "11"
    cpu: int = 4
    ram_mb: int = 8192
    disk_size_gb: int = 64
    disk_format: str = "qcow2"
    tpm: bool = True
    architecture: str = "x86_64"
    display: dict = field(default_factory=lambda: {"width": 1920, "height": 1080})


# ── Download helpers ────────────────────────────────────────────────────────


def download_virtio_iso(work_dir: Optional[Path] = None) -> Path:
    """Download virtio-win.iso from Fedora."""
    dest = (work_dir or WORK_DIR) / "virtio-win.iso"
    return download_file(VIRTIO_ISO_URL, dest, "virtio-win.iso")


# ── Disk creation ───────────────────────────────────────────────────────────


def create_qcow2(path: Path, size_gb: int) -> Path:
    """Create a qcow2 disk image."""
    path.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        "qemu-img", "create", "-q", "-f", "qcow2",
        "-o", "lazy_refcounts=on,preallocation=off",
        str(path), f"{size_gb}G",
    ]
    # Try system qemu-img first, then portable
    try:
        subprocess.run(cmd, check=True, capture_output=True)
    except FileNotFoundError:
        from cua_sandbox.runtime.qemu_installer import qemu_bin
        cmd[0] = str(Path(qemu_bin("x86_64")).parent / "qemu-img")
        if not Path(cmd[0]).exists():
            cmd[0] = str(Path(qemu_bin("x86_64")).parent / "qemu-img.exe")
        subprocess.run(cmd, check=True, capture_output=True)
    logger.info(f"Created {size_gb}G qcow2 disk: {path}")
    return path


# ── WSL2 build helper ──────────────────────────────────────────────────────


def _win_to_wsl(p: Path | str) -> str:
    """Convert a Windows path to WSL /mnt/... path."""
    s = str(p).replace("\\", "/")
    if len(s) >= 2 and s[1] == ":":
        return f"/mnt/{s[0].lower()}{s[2:]}"
    return s


def _build_image_wsl2(
    config: QEMUImageConfig,
    cmd: list[str],
    disk_path: Path,
    work_dir: Path,
    proc_port: int = 18000,
) -> Path:
    """Run QEMU build inside WSL2 with KVM acceleration.

    Converts all Windows paths in the QEMU command to WSL /mnt/... paths,
    uses the WSL2 qemu-system-x86_64 with -enable-kvm and -cpu host.
    """
    import socket, time, threading, urllib.request

    def _convert_arg(arg: str) -> str:
        """Convert Windows paths in a QEMU argument to WSL paths."""
        if len(arg) >= 3 and arg[1] == ":" and arg[2] in ("/", "\\"):
            return _win_to_wsl(arg)
        if "=" in arg and (":\\" in arg or ":/" in arg):
            parts = arg.split(",")
            converted = []
            for part in parts:
                if "=" in part:
                    k, v = part.split("=", 1)
                    if len(v) >= 3 and v[1] == ":" and v[2] in ("/", "\\"):
                        v = _win_to_wsl(v)
                    converted.append(f"{k}={v}")
                else:
                    converted.append(part)
            return ",".join(converted)
        return arg

    # Build WSL2 command: replace binary, convert paths, use native OVMF
    wsl_cmd = ["qemu-system-x86_64"]
    skip_next = False
    for i, arg in enumerate(cmd[1:], 1):
        if skip_next:
            skip_next = False
            continue

        # Replace -cpu with host passthrough
        if arg == "-cpu":
            wsl_cmd += ["-cpu", "host"]
            skip_next = True
            continue

        # Replace OVMF pflash with WSL2 native paths
        if arg == "-drive" and i + 1 <= len(cmd) and "if=pflash" in cmd[i] if i < len(cmd) else False:
            pass  # handled below
        if "if=pflash" in arg:
            if "readonly=on" in arg:
                # OVMF code — use WSL2 native
                wsl_cmd.append("if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd")
            else:
                # EFI vars — create from WSL2 native template (must match OVMF_CODE_4M size)
                wsl_efivars = _win_to_wsl(work_dir / "efivars-wsl.fd")
                wsl_cmd.append(f"if=pflash,format=raw,file={wsl_efivars}")
            continue

        wsl_cmd.append(_convert_arg(arg))

    wsl_cmd.append("-enable-kvm")

    # Bind QMP and network to 0.0.0.0 so Windows host can reach them
    for i, a in enumerate(wsl_cmd):
        if "tcp:127.0.0.1:4444" in a:
            wsl_cmd[i] = a.replace("127.0.0.1", "0.0.0.0")
        if "hostfwd=tcp::" in a:
            wsl_cmd[i] = a.replace("hostfwd=tcp::", "hostfwd=tcp:0.0.0.0:")

    # Create WSL2-compatible EFI vars from WSL native template
    wsl_efivars_win = work_dir / "efivars-wsl.fd"
    if not wsl_efivars_win.exists():
        subprocess.run(
            ["wsl", "-e", "bash", "-c",
             f"cp /usr/share/OVMF/OVMF_VARS_4M.fd '{_win_to_wsl(wsl_efivars_win)}'"],
            check=True, capture_output=True,
        )

    shell_cmd = " ".join(wsl_cmd)
    logger.info(f"WSL2 QEMU command: {shell_cmd}")

    # Launch QEMU in WSL2
    proc = subprocess.Popen(
        ["wsl", "-e", "bash", "-c", shell_cmd],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )
    logger.info(f"QEMU WSL2 install running as PID {proc.pid}. Monitor via VNC on port 5900.")

    def _qmp_cmd(cmd_json: bytes):
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(3)
            s.connect(("127.0.0.1", 4444))
            s.recv(4096)
            s.sendall(b'{"execute":"qmp_capabilities"}\n')
            s.recv(4096)
            s.sendall(cmd_json + b"\n")
            s.recv(4096)
            s.close()
            return True
        except Exception:
            return False

    def _boot_and_monitor():
        # Phase 1: boot keypresses (fewer needed with KVM — faster boot)
        for i in range(15):
            time.sleep(2)
            if _qmp_cmd(b'{"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":"ret"}]}}'):
                logger.info(f"Sent boot keypress ({i+1}/15)")

        # Phase 2: wait for CUA server (KVM is much faster, ~15-30 min)
        logger.info(f"Waiting for CUA computer-server on port {proc_port}...")
        for i in range(180):  # 180 * 10s = 30 min
            time.sleep(10)
            try:
                r = urllib.request.urlopen(f"http://127.0.0.1:{proc_port}/", timeout=5)
                if r.status == 200:
                    logger.info("CUA computer-server is running! Sending ACPI shutdown...")
                    break
            except Exception:
                if i % 6 == 0:
                    logger.info(f"Still waiting... ({i * 10 // 60} min elapsed)")
        else:
            logger.warning("Timed out waiting for CUA server. Shutting down anyway.")

        # Phase 3: ACPI shutdown
        time.sleep(5)
        _qmp_cmd(b'{"execute":"system_powerdown"}')
        for _ in range(12):
            time.sleep(10)
            if proc.poll() is not None:
                return
            _qmp_cmd(b'{"execute":"system_powerdown"}')
        logger.warning("ACPI shutdown did not work, terminating.")
        proc.terminate()

    t = threading.Thread(target=_boot_and_monitor, daemon=True)
    t.start()

    logger.info("Waiting for QEMU WSL2 to exit (15-30 min with KVM)...")
    proc.wait()
    if proc.returncode != 0:
        stderr = proc.stderr.read().decode() if proc.stderr else ""
        raise RuntimeError(f"QEMU WSL2 install failed (exit {proc.returncode}): {stderr}")

    logger.info(f"Windows install complete. Disk image: {disk_path}")
    config_path = work_dir / "config.json"
    config_path.write_text(json.dumps(asdict(config), indent=2))
    return disk_path


# ── Full build ──────────────────────────────────────────────────────────────


def build_image(
    config: Optional[QEMUImageConfig] = None,
    *,
    windows_iso: Optional[str] = None,
    work_dir: Optional[Path] = None,
    product_key: Optional[str] = None,
    use_wsl2: bool = False,
) -> Path:
    """Build a Windows qcow2 image via unattended QEMU install.

    Returns the path to the completed qcow2 disk.

    This is a long-running operation (30-60 minutes for Windows install).
    """
    config = config or QEMUImageConfig()
    work_dir = work_dir or WORK_DIR / f"windows-{config.version}"
    work_dir.mkdir(parents=True, exist_ok=True)

    disk_path = work_dir / "disk.qcow2"
    if disk_path.exists():
        logger.info(f"Disk already exists: {disk_path}")
        return disk_path

    # Step 1: Get ISOs
    win_iso = download_windows_iso(config.version, work_dir, windows_iso)
    virtio_iso = download_virtio_iso(work_dir)
    # Create a small data-only ISO with Autounattend.xml + setup script.
    # Windows Setup searches all drives for Autounattend.xml automatically,
    # so we keep the original Windows ISO unchanged (it's UEFI-bootable).
    unattend_iso = create_unattend_iso(work_dir, product_key)

    # Step 2: Create disk
    create_qcow2(disk_path, config.disk_size_gb)

    # Step 3: Run QEMU install
    logger.info("Starting unattended Windows install via QEMU...")
    logger.info("This will take 30-60 minutes. Monitor via VNC on port 5900.")

    from cua_sandbox.runtime.qemu_installer import qemu_bin
    import platform as _plat

    qemu = qemu_bin(config.architecture)
    qemu_dir = Path(qemu).parent

    # Locate OVMF UEFI firmware (required for Windows 10/11)
    ovmf_code = None
    for candidate in [
        qemu_dir / "share" / "edk2-x86_64-code.fd",
        Path("/usr/share/OVMF/OVMF_CODE.fd"),
        Path("/usr/share/ovmf/OVMF_CODE.fd"),
        Path("/usr/share/qemu/edk2-x86_64-code.fd"),
    ]:
        if candidate.exists():
            ovmf_code = candidate
            break

    if not ovmf_code:
        raise RuntimeError(
            "OVMF UEFI firmware not found. Windows 11 requires UEFI boot. "
            "Expected edk2-x86_64-code.fd in QEMU share directory."
        )

    # Create EFI vars file (writable copy for this install)
    efivars = work_dir / "efivars.fd"
    if not efivars.exists():
        vars_template = qemu_dir / "share" / "edk2-i386-vars.fd"
        if vars_template.exists():
            shutil.copy2(vars_template, efivars)
        else:
            efivars.write_bytes(b"\x00" * (256 * 1024))

    cmd = [
        qemu,
        "-name", "windows-install",
        "-machine", "q35,smm=off",
        "-m", str(config.ram_mb),
        "-smp", str(config.cpu),
        "-cpu", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt",
        # UEFI firmware
        "-drive", f"if=pflash,format=raw,readonly=on,file={ovmf_code}",
        "-drive", f"if=pflash,format=raw,file={efivars}",
        # Main disk — virtio-blk (bootindex=0 so after reboot it boots from disk)
        "-drive", f"file={disk_path},id=disk0,format=qcow2,if=none",
        "-device", "virtio-blk-pci,drive=disk0,bootindex=0",
        # AHCI controller for all CDROMs (Q35 built-in IDE only supports 1 unit)
        "-device", "ich9-ahci,id=sata",
        # Windows ISO — bootindex=0 so OVMF boots from it first
        "-drive", f"file={win_iso},if=none,media=cdrom,readonly=on,id=cd0",
        "-device", "ide-cd,drive=cd0,bus=sata.0,bootindex=1",
        # Unattend ISO (data-only: Autounattend.xml + setup script + startup.nsh)
        "-drive", f"file={unattend_iso},if=none,media=cdrom,readonly=on,id=cd1",
        "-device", "ide-cd,drive=cd1,bus=sata.1",
        # VirtIO drivers ISO
        "-drive", f"file={virtio_iso},if=none,media=cdrom,readonly=on,id=cd2",
        "-device", "ide-cd,drive=cd2,bus=sata.2",
        "-vnc", ":0",
        # QMP monitor for sending keypresses (bypass "press any key to boot from CD")
        "-qmp", "tcp:127.0.0.1:4444,server,nowait",
        # Network (localhost only, no firewall prompt)
        "-netdev", "user,id=net0,hostfwd=tcp::18000-:8000",
        "-device", "virtio-net-pci,netdev=net0,mac=52:55:00:d1:55:01",
        # Disable S3/S4 (avoids ACPI sleep issues during install) — same as dockur
        "-global", "ICH9-LPC.disable_s3=1",
        "-global", "ICH9-LPC.disable_s4=1",
        # RTC base localtime for Windows
        "-rtc", "base=localtime",
    ]

    # TPM (requires swtpm — not available on Windows)
    if config.tpm and shutil.which("swtpm") and _plat.system() != "Windows":
        tpm_dir = work_dir / "tpm"
        tpm_dir.mkdir(exist_ok=True)
        cmd += [
            "-chardev", f"socket,id=chrtpm,path={tpm_dir}/swtpm-sock",
            "-tpmdev", "emulator,id=tpm0,chardev=chrtpm",
            "-device", "tpm-tis,tpmdev=tpm0",
        ]
        subprocess.Popen([
            "swtpm", "socket",
            "--tpmstate", f"dir={tpm_dir}",
            "--ctrl", f"type=unixio,path={tpm_dir}/swtpm-sock",
            "--tpm2",
            "--log", f"file={tpm_dir}/swtpm.log",
        ])

    # Hardware acceleration
    if use_wsl2:
        # WSL2 path: rewrite entire command for WSL2 with KVM
        return _build_image_wsl2(
            config, cmd, disk_path, work_dir, proc_port=18000,
        )

    # WHPX on Windows has MMIO emulation bugs with OVMF pflash drives,
    # so we use TCG (software emulation) on Windows for UEFI builds.
    if _plat.system() == "Windows":
        cmd += ["-accel", "tcg"]
    elif shutil.which("kvm-ok"):
        r = subprocess.run(["kvm-ok"], capture_output=True)
        if r.returncode == 0:
            cmd.append("-enable-kvm")
    elif Path("/dev/kvm").exists():
        cmd.append("-enable-kvm")

    # Run QEMU install
    logger.info(f"QEMU command: {' '.join(cmd)}")
    if _plat.system() == "Windows":
        proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        logger.info(f"QEMU install running as PID {proc.pid}. Monitor via VNC on port 5900.")

        # Background thread: send boot keypresses, then health-check server, then shutdown
        import socket, time, threading, urllib.request
        def _qmp_cmd(cmd_json: bytes):
            """Send a QMP command and return."""
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                s.settimeout(3)
                s.connect(("127.0.0.1", 4444))
                s.recv(4096)  # QMP greeting
                s.sendall(b'{"execute":"qmp_capabilities"}\n')
                s.recv(4096)
                s.sendall(cmd_json + b"\n")
                s.recv(4096)
                s.close()
                return True
            except Exception:
                return False

        def _boot_and_monitor():
            """Phase 1: send Enter keys for 30s to bypass boot prompts.
               Phase 2: poll CUA server on port 18000 until it responds.
               Phase 3: send ACPI shutdown via QMP."""
            # Phase 1: boot keypresses
            for i in range(15):
                time.sleep(2)
                if _qmp_cmd(b'{"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":"ret"}]}}'):
                    logger.info(f"Sent boot keypress ({i+1}/15)")

            # Phase 2: wait for CUA server (up to 90 min for TCG installs)
            logger.info("Waiting for CUA computer-server to become available on port 18000...")
            for i in range(540):  # 540 * 10s = 90 min
                time.sleep(10)
                try:
                    r = urllib.request.urlopen("http://127.0.0.1:18000/", timeout=5)
                    if r.status == 200:
                        logger.info("CUA computer-server is running! Sending ACPI shutdown...")
                        break
                except Exception:
                    if i % 6 == 0:
                        logger.info(f"Still waiting for CUA server... ({i * 10 // 60} min elapsed)")
            else:
                logger.warning("Timed out waiting for CUA server (90 min). Shutting down anyway.")

            # Phase 3: ACPI shutdown
            time.sleep(5)
            _qmp_cmd(b'{"execute":"system_powerdown"}')
            # Wait and retry if needed
            for _ in range(12):
                time.sleep(10)
                if proc.poll() is not None:
                    return
                _qmp_cmd(b'{"execute":"system_powerdown"}')
            logger.warning("ACPI shutdown did not work, terminating QEMU.")
            proc.terminate()

        t = threading.Thread(target=_boot_and_monitor, daemon=True)
        t.start()

        logger.info("Waiting for QEMU to exit (this takes 30-60 minutes)...")
        proc.wait()
        if proc.returncode != 0:
            stderr = proc.stderr.read().decode() if proc.stderr else ""
            raise RuntimeError(f"QEMU install failed (exit {proc.returncode}): {stderr}")
    else:
        result = subprocess.run(cmd)
        if result.returncode != 0:
            raise RuntimeError(f"QEMU install failed with exit code {result.returncode}")

    logger.info(f"Windows install complete. Disk image: {disk_path}")

    # Save config alongside disk
    config_path = work_dir / "config.json"
    config_path.write_text(json.dumps(asdict(config), indent=2))

    return disk_path


# ── OCI push/pull ───────────────────────────────────────────────────────────


def _sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8 * 1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def push_image(
    disk_path: str | Path,
    ref: str,
    config: Optional[QEMUImageConfig] = None,
) -> None:
    """Push a qcow2 disk image to an OCI registry.

    Chunks the disk into gzip-compressed ~500MB layers with part annotations.
    """
    import oras.provider

    from cua_sandbox.registry.media_types import QEMU_CONFIG, QEMU_DISK_GZIP
    from cua_sandbox.registry.ref import parse_ref

    disk_path = Path(disk_path)
    if not disk_path.exists():
        raise FileNotFoundError(f"Disk image not found: {disk_path}")

    config = config or QEMUImageConfig()
    registry, org, name, tag = parse_ref(ref)
    full_repo = f"{registry}/{org}/{name}"

    r = oras.provider.Registry()

    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)

        # Create config blob
        config_data = json.dumps(asdict(config)).encode()
        config_digest = f"sha256:{hashlib.sha256(config_data).hexdigest()}"
        config_path = tmp_dir / "config.json"
        config_path.write_bytes(config_data)

        # Chunk the disk into gzip-compressed parts
        disk_size = disk_path.stat().st_size
        part_total = (disk_size + CHUNK_SIZE - 1) // CHUNK_SIZE
        layers = []

        logger.info(f"Chunking {disk_size / 1024 / 1024:.0f} MB disk into {part_total} parts...")

        with open(disk_path, "rb") as f:
            for part_num in range(1, part_total + 1):
                chunk_data = f.read(CHUNK_SIZE)
                if not chunk_data:
                    break

                # Gzip compress
                compressed = gzip.compress(chunk_data, compresslevel=6)
                chunk_path = tmp_dir / f"disk.part.{part_num:04d}.gz"
                chunk_path.write_bytes(compressed)

                chunk_digest = f"sha256:{hashlib.sha256(compressed).hexdigest()}"

                layers.append({
                    "mediaType": QEMU_DISK_GZIP,
                    "digest": chunk_digest,
                    "size": len(compressed),
                    "annotations": {
                        "org.trycua.qemu.part.number": str(part_num),
                        "org.trycua.qemu.part.total": str(part_total),
                        "org.trycua.qemu.uncompressed-size": str(len(chunk_data)),
                        "org.opencontainers.image.title": f"disk.qcow2.part.{part_num:04d}",
                    },
                })

                logger.info(f"  part {part_num}/{part_total}: {len(compressed) / 1024 / 1024:.1f} MB compressed")

                # Push blob
                r.push_blob(full_repo, chunk_path, chunk_digest)

        # Push config blob
        r.push_blob(full_repo, config_path, config_digest)

        # Create and push manifest
        manifest = {
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": QEMU_CONFIG,
                "digest": config_digest,
                "size": len(config_data),
            },
            "layers": layers,
            "annotations": {
                "org.trycua.qemu.guest-os": config.guest_os,
                "org.trycua.qemu.version": config.version,
                "org.trycua.qemu.disk-format": config.disk_format,
                "org.trycua.qemu.uncompressed-disk-size": str(disk_size),
            },
        }

        r.put_manifest(f"{full_repo}:{tag}", json.dumps(manifest))
        logger.info(f"Pushed {ref} ({part_total} layers, {disk_size / 1024 / 1024:.0f} MB)")


def pull_qemu_image(ref: str, dest_dir: Optional[Path] = None) -> tuple[QEMUImageConfig, Path]:
    """Pull a QEMU VM image from an OCI registry.

    Downloads gzip-compressed disk chunks, decompresses, and reassembles
    into a qcow2 disk image.

    Returns (config, disk_path).
    """
    import oras.provider

    from cua_sandbox.registry.cache import ImageCache
    from cua_sandbox.registry.media_types import QEMU_CONFIG, QEMU_DISK, QEMU_DISK_GZIP
    from cua_sandbox.registry.manifest import get_manifest
    from cua_sandbox.registry.ref import parse_ref

    registry, org, name, tag = parse_ref(ref)
    full_repo = f"{registry}/{org}/{name}"

    cache = ImageCache()
    if dest_dir is None:
        dest_dir = cache.image_dir(registry, org, name, tag)
    dest_dir.mkdir(parents=True, exist_ok=True)

    disk_path = dest_dir / "disk.qcow2"
    config_path = dest_dir / "config.json"

    # Check cache
    if disk_path.exists() and config_path.exists():
        logger.info(f"Using cached QEMU image: {dest_dir}")
        data = json.loads(config_path.read_text())
        return QEMUImageConfig(**{k: v for k, v in data.items() if k in QEMUImageConfig.__dataclass_fields__}), disk_path

    manifest = get_manifest(ref)
    r = oras.provider.Registry()

    # Download config
    config_blob = manifest.get("config", {})
    if config_blob.get("digest"):
        resp = r.get_blob(full_repo, config_blob["digest"])
        config_path.write_bytes(resp.content)
        config_data = resp.json() if hasattr(resp, "json") else {}
    else:
        config_data = {}

    config = QEMUImageConfig(**{k: v for k, v in config_data.items() if k in QEMUImageConfig.__dataclass_fields__})

    # Download and reassemble disk parts
    layers = manifest.get("layers", [])
    disk_parts: list[tuple[int, dict]] = []

    for layer in layers:
        mt = layer.get("mediaType", "")
        annot = layer.get("annotations", {})

        if mt in (QEMU_DISK, QEMU_DISK_GZIP):
            part_num = int(annot.get("org.trycua.qemu.part.number", 1))
            disk_parts.append((part_num, layer))

    if not disk_parts:
        raise RuntimeError(f"No QEMU disk layers found in manifest for {ref}")

    disk_parts.sort(key=lambda x: x[0])
    total = len(disk_parts)
    logger.info(f"Downloading {total} disk parts for {ref}...")

    with open(disk_path, "wb") as f:
        for part_num, layer in disk_parts:
            mt = layer.get("mediaType", "")
            digest = layer["digest"]
            size = layer.get("size", 0)
            logger.info(f"  part {part_num}/{total}: {size / 1024 / 1024:.1f} MB")

            resp = r.get_blob(full_repo, digest)
            data = resp.content

            # Decompress gzip if needed
            if mt == QEMU_DISK_GZIP or mt.endswith("+gzip"):
                data = gzip.decompress(data)

            f.write(data)

    logger.info(f"Assembled disk: {disk_path} ({disk_path.stat().st_size / 1024 / 1024:.0f} MB)")

    # Save manifest
    cache.save_manifest(registry, org, name, tag, manifest)

    return config, disk_path


# ── CLI entry point ─────────────────────────────────────────────────────────


def main():
    import argparse

    logging.basicConfig(level=logging.INFO, format="%(message)s")

    parser = argparse.ArgumentParser(description="Build and push QEMU VM images as OCI artifacts")
    sub = parser.add_subparsers(dest="command")

    # build
    build_p = sub.add_parser("build", help="Build a Windows VM image")
    build_p.add_argument("--windows-version", default="11", help="Windows version (10 or 11)")
    build_p.add_argument("--iso-path", help="Path to Windows ISO (skip download)")
    build_p.add_argument("--disk-size", type=int, default=64, help="Disk size in GB")
    build_p.add_argument("--ram", type=int, default=8192, help="RAM in MB")
    build_p.add_argument("--cpu", type=int, default=4, help="CPU cores")
    build_p.add_argument("--no-tpm", action="store_true", help="Disable TPM")
    build_p.add_argument("--work-dir", type=Path, help="Working directory")
    build_p.add_argument("--product-key", help="Windows product key")
    build_p.add_argument("--wsl2", action="store_true", help="Run QEMU inside WSL2 with KVM acceleration")

    # push
    push_p = sub.add_parser("push", help="Push a qcow2 disk as OCI image")
    push_p.add_argument("--ref", required=True, help="OCI reference (e.g. ghcr.io/trycua/win11:latest)")
    push_p.add_argument("--disk", required=True, help="Path to qcow2 disk")
    push_p.add_argument("--guest-os", default="windows")
    push_p.add_argument("--version", default="11")
    push_p.add_argument("--disk-size", type=int, default=64)
    push_p.add_argument("--ram", type=int, default=8192)
    push_p.add_argument("--cpu", type=int, default=4)
    push_p.add_argument("--no-tpm", action="store_true")

    # pull
    pull_p = sub.add_parser("pull", help="Pull a QEMU VM image from registry")
    pull_p.add_argument("--ref", required=True, help="OCI reference")
    pull_p.add_argument("--dest", type=Path, help="Destination directory")

    # build-and-push
    bp_p = sub.add_parser("build-and-push", help="Build then push")
    bp_p.add_argument("--ref", required=True)
    bp_p.add_argument("--windows-version", default="11")
    bp_p.add_argument("--iso-path", help="Path to Windows ISO")
    bp_p.add_argument("--disk-size", type=int, default=64)
    bp_p.add_argument("--ram", type=int, default=8192)
    bp_p.add_argument("--cpu", type=int, default=4)
    bp_p.add_argument("--no-tpm", action="store_true")
    bp_p.add_argument("--product-key")
    bp_p.add_argument("--wsl2", action="store_true")

    args = parser.parse_args()

    if args.command == "build":
        cfg = QEMUImageConfig(
            guest_os="windows", version=args.windows_version,
            cpu=args.cpu, ram_mb=args.ram, disk_size_gb=args.disk_size,
            tpm=not args.no_tpm,
        )
        disk = build_image(cfg, windows_iso=args.iso_path, work_dir=args.work_dir, product_key=args.product_key, use_wsl2=args.wsl2)
        print(f"Built: {disk}")

    elif args.command == "push":
        cfg = QEMUImageConfig(
            guest_os=args.guest_os, version=args.version,
            cpu=args.cpu, ram_mb=args.ram, disk_size_gb=args.disk_size,
            tpm=not args.no_tpm,
        )
        push_image(args.disk, args.ref, cfg)

    elif args.command == "pull":
        cfg, disk = pull_qemu_image(args.ref, args.dest)
        print(f"Pulled: {disk}")
        print(f"Config: {json.dumps(asdict(cfg), indent=2)}")

    elif args.command == "build-and-push":
        cfg = QEMUImageConfig(
            guest_os="windows", version=args.windows_version,
            cpu=args.cpu, ram_mb=args.ram, disk_size_gb=args.disk_size,
            tpm=not args.no_tpm,
        )
        disk = build_image(cfg, windows_iso=args.iso_path, product_key=args.product_key, use_wsl2=args.wsl2)
        push_image(disk, args.ref, cfg)

    else:
        parser.print_help()


if __name__ == "__main__":
    main()
