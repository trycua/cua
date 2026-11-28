"""``lume``: an AgentEnv sandbox provider whose sandboxes are macOS VMs on this Mac, run by lume.

A sandbox is one VM cloned from a pinned base, the bench image, booted headless. The computer's boot settings (its
``env``: driver settings, app settings, the control token) reach the VM as a JSON file on a read-only lume shared
directory, ``/Volumes/My Shared Files/agentenv/boot.json``, which the image's driver LaunchAgent reads at login, as a
Linux disk reads its NoCloud seed. The driver's port is reached on the VM's own address on lume's NAT network.

Reset deletes the clone and clones the base again under the same name and MAC address, so the sandbox, its id and its
URLs stay. Apple's macOS licence allows two macOS VMs per Mac, so ``create_vm`` refuses a third running VM on the
host, whoever started the others.

Configure it under ``[sandbox.providers.lume.config]``::

    base = "agentenv-macos-bench"          # a local lume VM to clone, or
    image = "ghcr.io/trycua/<repo>:<tag>"  # a registry image, pulled once into a local base
    ssh_user = "lume"                      # the image's login, for exec
    ssh_password = "env:LUME_SSH_PASSWORD"
    display = "1280x800"                   # the virtual display's size, set on each clone before it first boots
"""

from __future__ import annotations

import asyncio
import contextlib
import hashlib
import json
import logging
import os
import re
import shlex
import shutil
import uuid
from pathlib import Path
from typing import Any, ClassVar, Optional

from agent_env.providers.sandbox_providers.sandbox import NetworkPolicy, VmSandbox
from agent_env.providers.sandbox_providers.sandbox_provider import (
    SANDBOX_MODE_VM,
    SandboxNotFoundError,
    SandboxProvider,
)
from agentenv_cua.lume.cli import Lume, LumeError, LumeNotFound

logger = logging.getLogger(__name__)

#: The platform a macOS VM on Apple silicon runs.
PLATFORM = "darwin/arm64"
#: Apple's macOS licence: at most two macOS VMs running per Mac.
MAX_RUNNING_PER_HOST = 2
#: Where boot settings land in the guest, under lume's shared-directory mount.
GUEST_BOOT_FILE = "/Volumes/My Shared Files/agentenv/boot.json"
_NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
_DISPLAY = re.compile(r"^[1-9][0-9]{2,4}x[1-9][0-9]{2,4}$")
#: The size the conformance fixtures need; the image puts the guest in whatever size its clone is given.
DEFAULT_DISPLAY = "1280x800"
_IP_POLL_S = 2.0


class LumeVmSandbox(VmSandbox):
    """One macOS VM. ``exec`` runs a command over lume's ssh as the image's login user; there is no Docker in it."""

    type = "lume"
    ON_THIS_MACHINE = True

    def __init__(
        self, provider: "LumeSandboxProvider", name: str, ip: str, ports: list[int]
    ) -> None:
        self._provider = provider
        self.sandbox_id = name
        self.mode = SANDBOX_MODE_VM
        self.platform = PLATFORM
        self.vnc_url = None
        self.network_policy = None
        self.ports = list(ports)
        self.ip = ip
        self.tunnel_urls = {port: f"http://{ip}:{port}" for port in ports}

    async def exec(self, *command: str) -> Any:
        done = await self._provider.lume.ssh(
            self.sandbox_id,
            shlex.join(command),
            user=self._provider.ssh_user,
            password=self._provider.ssh_password,
            timeout=self._provider.exec_timeout_s,
        )
        return _Process(done.returncode, done.stdout.encode(), done.stderr.encode())

    async def exec_script(self, script: str, *, max_retries: int = 0) -> str:
        """Run ``script`` with bash as the login user (macOS has no passwordless sudo by default; a script that needs
        root says ``sudo`` itself, which the image allows for its user)."""
        for attempt in range(max_retries + 1):
            code, stdout, stderr = await self.exec_with_output("/bin/bash", "-c", script)
            if code == 0:
                return stdout
            if code != -1 or attempt == max_retries:
                raise RuntimeError(
                    f"script failed in {self.sandbox_id} (exit {code}): {stderr[-2000:]}"
                )
        raise AssertionError("unreachable")

    async def wait_for_vm(self) -> None:
        """Ready once ssh answers; a macOS VM has no Docker to wait for."""
        await self._provider._wait_for_ssh(self.sandbox_id)

    async def setup_vm_for_gateway(self, exposed_ports: Optional[list[int]] = None) -> None:
        """Nothing to open: lume's NAT network reaches every guest port from this Mac."""

    async def terminate(self) -> None:
        await self._provider._destroy(self.sandbox_id)


async def _lume_run_processes() -> set[str]:
    """The VM names in this Mac's ``lume run <name> ...`` processes."""
    proc = await asyncio.create_subprocess_exec(
        "ps", "-axo", "command=", stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL
    )
    out, _ = await proc.communicate()
    names: set[str] = set()
    for line in out.decode(errors="replace").splitlines():
        try:
            argv = shlex.split(line)
        except ValueError:
            continue
        if not argv or Path(argv[0]).name != "lume" or "run" not in argv[1:3]:
            continue
        rest = argv[argv.index("run") + 1 :]
        # `lume run [flags] NAME [flags]`: the first word that isn't a flag or a flag's value.
        value_flags = {"--display", "--shared-dir", "--storage", "--vnc-port", "--cpu", "--memory"}
        skip = False
        for word in rest:
            if skip:
                skip = False
            elif word in value_flags:
                skip = True
            elif not word.startswith("-"):
                names.add(word)
                break
    return names


class _Stream:
    def __init__(self, data: bytes) -> None:
        self._data = data

    async def read(self) -> bytes:
        data, self._data = self._data, b""
        return data


class _Process:
    def __init__(self, code: int, stdout: bytes, stderr: bytes) -> None:
        self._code = code
        self.stdout, self.stderr = _Stream(stdout), _Stream(stderr)

    async def wait(self) -> int:
        return self._code


class LumeSandboxProvider(SandboxProvider):
    """macOS VMs on this Mac through lume; see the module docstring."""

    ON_THIS_MACHINE: ClassVar[bool] = True

    def __init__(
        self,
        *,
        base: Optional[str] = None,
        image: Optional[str] = None,
        prefix: str = "agentenv",
        storage: Optional[str] = None,
        lume: str = "lume",
        ssh_user: str = "lume",
        ssh_password: str = "lume",
        state_dir: Optional[str] = None,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        display: str = DEFAULT_DISPLAY,
        boot_timeout_s: float = 600,
        exec_timeout_s: float = 600,
        max_running: int = MAX_RUNNING_PER_HOST,
        lume_cli: Optional[Lume] = None,
    ) -> None:
        if not (base or image):
            raise ValueError(
                "the lume provider needs `base` (a local lume VM to clone) or `image` (a registry ref)"
            )
        if not _NAME.match(prefix):
            raise ValueError(f"prefix {prefix!r} must be lowercase letters, digits and dashes")
        if not _DISPLAY.match(display):
            raise ValueError(f"display {display!r} must be WIDTHxHEIGHT, such as 1280x800")
        if max_running > MAX_RUNNING_PER_HOST:
            raise ValueError(
                f"max_running {max_running} is above the {MAX_RUNNING_PER_HOST} macOS VMs per Mac that "
                "Apple's licence allows"
            )
        self.base, self.image, self.prefix = base, image, prefix
        self.lume = lume_cli or Lume(lume, storage=storage)
        self.ssh_user, self.ssh_password = ssh_user, ssh_password
        self.state_dir = Path(state_dir or Path.home() / ".agentenv" / "lume")
        self.cpu, self.memory_mb, self.display = cpu, memory_mb, display
        self.boot_timeout_s, self.exec_timeout_s = boot_timeout_s, exec_timeout_s
        self.max_running = max_running
        self._create_lock = asyncio.Lock()

    @classmethod
    def from_config(cls, **config: Any) -> "LumeSandboxProvider":
        known = {
            "base",
            "image",
            "prefix",
            "storage",
            "lume",
            "ssh_user",
            "ssh_password",
            "state_dir",
            "cpu",
            "memory_mb",
            "display",
            "boot_timeout_s",
            "exec_timeout_s",
            "max_running",
        }
        if unknown := sorted(set(config) - known):
            raise ValueError(
                f"[sandbox.providers.lume.config] has unknown keys: {', '.join(unknown)}"
            )
        return cls(**config)

    @classmethod
    def native_platforms(cls) -> frozenset[str]:
        return frozenset({PLATFORM})

    @classmethod
    def disk_image_platforms(cls) -> frozenset[str]:
        return frozenset({PLATFORM})

    async def create_sandbox(self, **_: Any) -> LumeVmSandbox:
        raise NotImplementedError(
            "a lume sandbox is a macOS computer, not an agent's container; run agents on a "
            "container provider such as `local`"
        )

    async def create_vm(
        self,
        *,
        image: Optional[str] = None,
        boot_mode: Optional[str] = None,
        cpu: float = 1.0,
        memory: int = 8192,
        disk_size_gb: float = 10,
        timeout: int = 3600 * 2,
        exposed_ports: Optional[list[int]] = None,
        setup_for_gateway: bool = True,
        attribution: Optional[dict] = None,
        network_policy: Optional[NetworkPolicy] = None,
        env: Optional[dict[str, str]] = None,
    ) -> LumeVmSandbox:
        """A fresh clone of the base (or of ``image``, when it names a lume image), booted headless with ``env`` as
        its boot settings, reached on ``exposed_ports`` at its NAT address."""
        if network_policy is not None and network_policy.restricts_egress:
            raise ValueError("the lume provider can't restrict a macOS VM's egress")
        if boot_mode:
            logger.info("lume: boot_mode %r ignored; a macOS VM boots its own way", boot_mode)
        logger.info(
            "lume: cpu=%s memory=%s disk=%s ignored; the base's size is used unless the provider config "
            "sets cpu/memory_mb",
            cpu,
            memory,
            disk_size_gb,
        )
        source = await self._source(image)
        name = f"{self.prefix}-{uuid.uuid4().hex[:10]}"
        ports = list(exposed_ports or [])
        async with self._create_lock:
            running = await self._running_vms()
            if len(running) >= self.max_running:
                raise RuntimeError(
                    f"{len(running)} macOS VMs already run on this Mac ({', '.join(running)}); the "
                    f"lume provider starts none past {self.max_running}"
                )
            await self._write_state(
                name, {"source": source, "ports": ports, "attribution": attribution or {}}
            )
            await self._write_boot(name, env or {})
            try:
                await self.lume.clone(source, name)
                await self.lume.set(
                    name, cpu=self.cpu, memory_mb=self.memory_mb, display=self.display
                )
                await self._remember_mac(name)
                await self._run(name)
            except BaseException:
                await self._destroy(name)
                raise
        try:
            ip = await self._wait_for_ip(name)
        except BaseException:
            await self._destroy(name)
            raise
        return LumeVmSandbox(self, name, ip, ports)

    async def get_sandbox(self, sandbox_id: str) -> LumeVmSandbox:
        state = self._read_state(sandbox_id)
        try:
            info = await self.lume.get(sandbox_id)
        except LumeNotFound:
            raise SandboxNotFoundError(sandbox_id) from None
        if state is None or not info.running or not info.ip:
            raise SandboxNotFoundError(sandbox_id)
        return LumeVmSandbox(self, sandbox_id, info.ip, state["ports"])

    async def terminate_sandbox(self, sandbox_id: str) -> None:
        await self._destroy(sandbox_id)

    async def reset_vm(self, sandbox: Any, *, image: str, env: dict[str, str]) -> None:
        """Delete the clone and clone its source again under the same name and MAC address, with ``env`` as its boot
        settings, so it comes back at the same address with a clean disk."""
        name = sandbox.sandbox_id
        state = self._read_state(name)
        if state is None:
            raise RuntimeError(f"{name} wasn't made by this lume provider")
        old_ip = getattr(sandbox, "ip", None)
        await self.lume.stop(name)
        await self.lume.delete(name)
        await self._write_boot(name, env)
        await self.lume.clone(state["source"], name)
        await self.lume.set(
            name, cpu=self.cpu, memory_mb=self.memory_mb, display=self.display
        )
        await self._restore_mac(name, state.get("mac"))
        await self._run(name)
        ip = await self._wait_for_ip(name)
        if old_ip and ip != old_ip:
            raise RuntimeError(
                f"{name} came back at {ip}, not {old_ip}, after its reset; its URLs would change"
            )

    # Internals.

    async def _source(self, image: Optional[str]) -> str:
        """The local VM a sandbox is cloned from: ``image`` when it names a local lume VM or a registry image (pulled
        once, under a name derived from the reference), else the configured base."""
        ref = image if image and not image.startswith(("sha256:", "localhost:5000/")) else None
        ref = ref or self.base or self.image
        if "/" not in ref:
            return ref
        local = f"{self.prefix}-base-{hashlib.sha256(ref.encode()).hexdigest()[:12]}"
        try:
            await self.lume.get(local)
        except LumeNotFound:
            logger.info("lume: pulling %s into %s, once", ref, local)
            await self.lume.pull(ref, local)
        return local

    async def _run(self, name: str) -> None:
        await self.lume.run_detached(name, shared_dir=f"{self._share_dir(name)}:ro")

    async def _running_vms(self) -> list[str]:
        """Names of the VMs running on this Mac: what lume lists as running, plus every VM a ``lume run`` process
        holds. `lume ls` can show a VM another lume instance runs as stopped, and the host limit counts it."""
        names = {vm.name for vm in await self.lume.list() if vm.running}
        names.update(await _lume_run_processes())
        return sorted(names)

    async def _wait_for_ip(self, name: str) -> str:
        try:
            async with asyncio.timeout(self.boot_timeout_s):
                while True:
                    info = await self.lume.get(name)
                    if info.running and info.ip:
                        return info.ip
                    await asyncio.sleep(_IP_POLL_S)
        except TimeoutError:
            raise TimeoutError(
                f"{name} had no address within {self.boot_timeout_s:.0f} s"
            ) from None

    async def _wait_for_ssh(self, name: str) -> None:
        try:
            async with asyncio.timeout(self.boot_timeout_s):
                while (
                    await self.lume.ssh(
                        name, "true", user=self.ssh_user, password=self.ssh_password, timeout=15
                    )
                ).returncode != 0:
                    await asyncio.sleep(_IP_POLL_S)
        except TimeoutError:
            raise TimeoutError(
                f"{name} didn't answer ssh within {self.boot_timeout_s:.0f} s"
            ) from None

    async def _destroy(self, name: str) -> None:
        if self._read_state(name) is None:
            logger.warning("lume: not deleting %s, which this provider didn't make", name)
            return
        with contextlib.suppress(LumeError):
            await self.lume.stop(name)
        await self.lume.delete(name)
        shutil.rmtree(self.state_dir / name, ignore_errors=True)

    def _share_dir(self, name: str) -> Path:
        # lume mounts a shared folder at /Volumes/My Shared Files/<its basename>, so this one is named `agentenv`.
        return self.state_dir / name / "agentenv"

    async def _write_boot(self, name: str, env: dict[str, str]) -> None:
        """The boot settings, readable only by this user on the host and read-only in the guest."""
        folder = self._share_dir(name)
        folder.mkdir(parents=True, exist_ok=True, mode=0o700)
        path = folder / "boot.json"
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as out:
            json.dump({"env": env}, out)

    async def _write_state(self, name: str, state: dict) -> None:
        folder = self.state_dir / name
        folder.mkdir(parents=True, exist_ok=True, mode=0o700)
        (folder / "state.json").write_text(json.dumps(state))

    def _read_state(self, name: str) -> Optional[dict]:
        try:
            return json.loads((self.state_dir / name / "state.json").read_text())
        except (OSError, ValueError):
            return None

    def _vm_config(self, name: str) -> Path:
        storage = (
            Path(self.lume.storage).expanduser() if self.lume.storage else Path.home() / ".lume"
        )
        return storage / name / "config.json"

    async def _remember_mac(self, name: str) -> None:
        state = self._read_state(name) or {}
        with contextlib.suppress(OSError, ValueError):
            state["mac"] = json.loads(self._vm_config(name).read_text()).get("macAddress")
        await self._write_state(name, state)

    async def _restore_mac(self, name: str, mac: Optional[str]) -> None:
        """Give the fresh clone the MAC address the first one had, so lume's DHCP hands it the same address."""
        if not mac:
            return
        path = self._vm_config(name)
        config = json.loads(path.read_text())
        config["macAddress"] = mac
        path.write_text(json.dumps(config, indent=2))
