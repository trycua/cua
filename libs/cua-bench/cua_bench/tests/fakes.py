"""In-memory stand-ins for cua-sandbox, shared by the hermetic tests.

Nothing here starts a sandbox, touches the network or runs a command on the
host: the fake shell understands a handful of file commands (``rm -f``,
``cp``, ``mkdir -p``, ``cat``, ``echo``, ``true``/``false``) against an
in-memory file table and records everything else.
"""

from __future__ import annotations

import asyncio
import shlex
from contextlib import asynccontextmanager
from types import SimpleNamespace
from typing import Any, Optional


def png_bytes(width: int = 8, height: int = 6) -> bytes:
    """A real (tiny) PNG, so traces and previews can decode it."""
    from io import BytesIO

    from PIL import Image

    buf = BytesIO()
    Image.new("RGB", (width, height), (40, 90, 160)).save(buf, format="PNG")
    return buf.getvalue()


class FakeFiles:
    def __init__(self) -> None:
        self.data: dict[str, bytes] = {}
        self.dirs: set[str] = {"/", "/tmp"}

    async def write_text(self, path: str, content: str) -> None:
        self.data[path] = content.encode()

    async def read_text(self, path: str) -> str:
        if path not in self.data:
            raise FileNotFoundError(path)
        return self.data[path].decode()

    async def write_bytes(self, path: str, content: bytes) -> None:
        self.data[path] = bytes(content)

    async def read_bytes(self, path: str) -> bytes:
        if path not in self.data:
            raise FileNotFoundError(path)
        return self.data[path]

    async def exists(self, path: str) -> bool:
        return path in self.data or path in self.dirs

    async def is_dir(self, path: str) -> bool:
        return path in self.dirs

    async def make_dir(self, path: str) -> None:
        self.dirs.add(path.rstrip("/") or "/")

    async def remove_dir(self, path: str) -> None:
        self.dirs.discard(path)

    async def delete(self, path: str) -> None:
        self.data.pop(path, None)

    async def list(self, path: str) -> list:
        prefix = path.rstrip("/") + "/"
        names = {p[len(prefix) :].split("/")[0] for p in self.data if p.startswith(prefix)}
        return [SimpleNamespace(name=n) for n in sorted(names)]


class FakeShell:
    """A tiny, bounded interpreter for the commands the bundled tasks use."""

    def __init__(self, files: FakeFiles) -> None:
        self.files = files
        self.commands: list[tuple[str, Any]] = []

    async def run(self, command: str, timeout: Any = None, background: bool = False):
        self.commands.append((command, timeout))
        rc, out = 0, ""
        for part in command.split("&&")[:16]:
            try:
                argv = shlex.split(part)
            except ValueError:
                argv = part.split()
            if not argv:
                continue
            head = argv[0]
            if head == "rm":
                for path in (a for a in argv[1:] if not a.startswith("-")):
                    self.files.data.pop(path, None)
            elif head == "cp" and len(argv) >= 3:
                src, dst = argv[-2], argv[-1]
                if src not in self.files.data:
                    rc = 1
                    break
                self.files.data[dst] = self.files.data[src]
            elif head == "mkdir":
                for path in (a for a in argv[1:] if not a.startswith("-")):
                    self.files.dirs.add(path)
            elif head == "cat":
                out += "".join(self.files.data.get(p, b"").decode() for p in argv[1:])
            elif head == "echo":
                out += " ".join(argv[1:]) + "\n"
            elif head == "false":
                rc = 1
                break
        return SimpleNamespace(returncode=rc, stdout=out, stderr="" if rc == 0 else "failed")


class FakeScreen:
    async def size(self):
        return (1024, 768)


class _Recorder:
    """Records every awaited call (mouse/keyboard actions)."""

    def __init__(self, log: list) -> None:
        self._log = log

    def __getattr__(self, name: str):
        async def call(*args, **kwargs):
            self._log.append((name, args, kwargs))

        return call


class FakeSandbox:
    def __init__(self, name: str = "fake", pool: Optional[str] = None, image_info: Any = None):
        self.name = name
        self.pool_name = pool
        self.files = FakeFiles()
        self.shell = FakeShell(self.files)
        self.screen = FakeScreen()
        self.actions: list = []
        self.mouse = _Recorder(self.actions)
        self.keyboard = _Recorder(self.actions)
        self.image_info = image_info
        self.disconnected = False

    async def screenshot(self, *args, **kwargs) -> bytes:
        return png_bytes()

    async def disconnect(self) -> None:
        self.disconnected = True

    async def destroy(self) -> None:
        self.disconnected = True

    #: What ``get_display_url`` returns (a str), or raises (an exception).
    display: Any = None

    async def get_display_url(self, *, share: bool = False) -> Optional[str]:
        if isinstance(self.display, BaseException):
            raise self.display
        return self.display

    @property
    def id(self) -> str:
        return f"{'cloud' if self.pool_name else 'local'}:{self.name}"


class FakeImage:
    def __init__(self, ref=None, os_type="linux", kind=None, builtin=None):
        self.ref, self.os_type, self.kind, self.builtin = ref, os_type, kind, builtin

    @classmethod
    def from_registry(cls, ref, *, os_type="linux", kind=None):
        return cls(ref, os_type, kind)

    @classmethod
    def linux(cls, kind="vm", version=None):
        return cls(None, "linux", kind, "linux")

    @classmethod
    def windows(cls, kind="vm", version=None):
        image = cls(None, "windows", kind, "windows")
        image.version = version
        return image

    @classmethod
    def macos(cls, kind="vm", version=None):
        image = cls(None, "macos", kind, "macos")
        image.version = version
        return image

    android = linux


class FakeSDK:
    """``(Image, Sandbox)`` pair recording every ``Sandbox.ephemeral`` call."""

    def __init__(self, delay: float = 0.0, image_info: Any = None) -> None:
        self.calls: list[dict] = []
        self.sandboxes: list[FakeSandbox] = []
        self.live = 0
        self.peak = 0
        self.released = 0
        self.image_info = image_info
        #: ``FakeSandbox.display`` of every sandbox this SDK opens.
        self.display: Any = None
        sdk = self

        class Sandbox:
            @classmethod
            @asynccontextmanager
            async def ephemeral(
                cls,
                image=None,
                *,
                pool=None,
                on=None,
                local=None,
                runtime=None,
                cpu=None,
                memory_mb=None,
                time_to_start=None,
                server_port=None,
                telemetry_enabled=True,
                warm=None,
                max_pool_size=None,
                claim_ttl=None,
                progress=None,
                services=None,
            ):
                kwargs = {
                    k: v
                    for k, v in dict(
                        pool=pool,
                        on=on,
                        local=local,
                        runtime=runtime,
                        cpu=cpu,
                        memory_mb=memory_mb,
                        time_to_start=time_to_start,
                        server_port=server_port,
                        telemetry_enabled=telemetry_enabled,
                        warm=warm,
                        max_pool_size=max_pool_size,
                        claim_ttl=claim_ttl,
                        progress=progress,
                        services=services,
                    ).items()
                    if v is not None
                }
                sdk.calls.append({"image": image, **kwargs})
                ref = getattr(image, "ref", None) or getattr(image, "builtin", None) or "img"
                cloud = kwargs.get("on") == "cloud" or kwargs.get("local") is False
                pool = f"cua-auto-{abs(hash(ref)) % 10000}" if cloud else None
                progress = kwargs.get("progress")
                sdk.live += 1
                sdk.peak = max(sdk.peak, sdk.live)
                try:
                    if delay:
                        await asyncio.sleep(delay)
                    if progress is not None:
                        progress(SimpleNamespace(stage="ready", pool=pool, message="ready"))
                    sb = FakeSandbox(f"sb-{len(sdk.calls)}", pool, sdk.image_info)
                    sb.display = sdk.display
                    sdk.sandboxes.append(sb)
                    yield sb
                finally:
                    sdk.live -= 1
                    sdk.released += 1

        self.Image = FakeImage
        self.Sandbox = Sandbox

    def pair(self):
        return self.Image, self.Sandbox
