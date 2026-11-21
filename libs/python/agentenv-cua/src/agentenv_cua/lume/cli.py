"""A thin async wrapper over the ``lume`` CLI, the only way this package touches macOS VMs.

Every call goes through one ``Runner``, so tests swap in a fake and nothing here needs a Mac."""

from __future__ import annotations

import asyncio
import json
import shlex
from dataclasses import dataclass
from typing import Awaitable, Callable, Optional, Protocol


class LumeError(RuntimeError):
    """A lume command that exited non-zero, with what it printed."""


class LumeNotFound(LumeError):
    """A VM lume doesn't know."""


@dataclass(frozen=True)
class Completed:
    returncode: int
    stdout: str
    stderr: str


class Runner(Protocol):
    def __call__(self, argv: list[str], *, timeout: float) -> Awaitable[Completed]: ...


async def run_process(argv: list[str], *, timeout: float) -> Completed:
    """Run ``argv`` and return its exit code and output; past ``timeout`` it is killed and reported as -1."""
    process = await asyncio.create_subprocess_exec(
        *argv,
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        stdout, stderr = await asyncio.wait_for(process.communicate(), timeout)
    except asyncio.TimeoutError:
        process.kill()
        await process.wait()
        return Completed(-1, "", f"{shlex.join(argv)} timed out after {timeout:.0f}s")
    return Completed(
        process.returncode or 0, stdout.decode(errors="replace"), stderr.decode(errors="replace")
    )


@dataclass(frozen=True)
class VmInfo:
    name: str
    status: str
    ip: Optional[str]
    cpu: Optional[int] = None
    memory_bytes: Optional[int] = None

    @property
    def running(self) -> bool:
        return self.status == "running"


class Lume:
    """The lume commands the provider needs: clone, run detached, get, ssh, stop, delete, list."""

    def __init__(
        self,
        executable: str = "lume",
        *,
        storage: Optional[str] = None,
        runner: Callable[..., Awaitable[Completed]] = run_process,
    ) -> None:
        self.executable = executable
        self.storage = storage
        self._run = runner

    def _storage(self, flag: str = "--storage") -> list[str]:
        return [flag, self.storage] if self.storage else []

    async def _check(self, *args: str, timeout: float = 120) -> str:
        argv = [self.executable, *args]
        done = await self._run(argv, timeout=timeout)
        if done.returncode != 0:
            text = (done.stderr or done.stdout).strip()
            if "not found" in text.lower() or "does not exist" in text.lower():
                raise LumeNotFound(text)
            raise LumeError(f"{shlex.join(argv)} exited {done.returncode}: {text[-2000:]}")
        return done.stdout

    async def clone(self, source: str, name: str) -> None:
        await self._check(
            "clone",
            source,
            name,
            *self._storage("--source-storage"),
            *self._storage("--dest-storage"),
            timeout=1800,
        )

    async def pull(self, image: str, name: str) -> None:
        """Pull an image from a registry (``ghcr.io/org/repo:tag``) into a local VM ``name``."""
        registry, _, rest = image.partition("/")
        organization, _, repository = rest.partition("/")
        if not repository:
            raise ValueError(f"{image!r} isn't a registry/organization/repository:tag reference")
        await self._check(
            "pull",
            repository,
            name,
            "--registry",
            registry,
            "--organization",
            organization,
            *self._storage(),
            timeout=7200,
        )

    async def set(
        self,
        name: str,
        *,
        cpu: Optional[int] = None,
        memory_mb: Optional[int] = None,
        display: Optional[str] = None,
    ) -> None:
        args = [
            *(["--display", display] if display else []),
            *(["--cpu", str(cpu)] if cpu else []),
            *(["--memory", f"{memory_mb}MB"] if memory_mb else []),
        ]
        if args:
            await self._check("set", name, *args, *self._storage())

    async def run_detached(self, name: str, *, shared_dir: Optional[str] = None) -> None:
        """Boot ``name`` headless in the background, with ``shared_dir`` (``path`` or ``path:ro``) mounted in the
        guest under ``/Volumes/My Shared Files``."""
        share = ["--shared-dir", shared_dir] if shared_dir else []
        await self._check(
            "run", name, "--display", "none", "--detach", *share, *self._storage(), timeout=300
        )

    async def get(self, name: str) -> VmInfo:
        data = json.loads(await self._check("get", name, "--format", "json", *self._storage()))
        if isinstance(data, list):
            if not data:
                raise LumeNotFound(f"lume get {name} returned nothing")
            data = data[0]
        return VmInfo(
            name=data["name"],
            status=data.get("status") or "unknown",
            ip=data.get("ipAddress"),
            cpu=data.get("cpuCount"),
            memory_bytes=data.get("memorySize"),
        )

    async def list(self) -> list[VmInfo]:
        data = json.loads(await self._check("ls", "--format", "json", *self._storage()))
        return [
            VmInfo(name=d["name"], status=d.get("status") or "unknown", ip=d.get("ipAddress"))
            for d in data
        ]

    async def ssh(
        self, name: str, command: str, *, user: str, password: str, timeout: float = 60
    ) -> Completed:
        """Run ``command`` (one shell string) in the VM over lume's ssh; the result is returned, not raised."""
        argv = [
            self.executable,
            "ssh",
            name,
            command,
            "--user",
            user,
            "--password",
            password,
            "--timeout",
            str(int(timeout)),
            *self._storage(),
        ]
        return await self._run(argv, timeout=timeout + 30)

    async def stop(self, name: str) -> None:
        try:
            await self._check("stop", name, *self._storage(), timeout=180)
        except LumeNotFound:
            return

    async def delete(self, name: str) -> None:
        try:
            await self._check("delete", name, "--force", *self._storage(), timeout=300)
        except LumeNotFound:
            return
