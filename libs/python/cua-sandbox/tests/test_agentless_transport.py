"""A local Lume sandbox without cua-spacesd: ``sb.shell`` and ``sb.screen``
go through the SDK's agentless fallback (``Sandbox.guest_sh`` over
``lume ssh``, ``guest_screenshot`` over VNC). A fake SDK handle stands in
for the native one; nothing starts a VM.
"""

from __future__ import annotations

from types import SimpleNamespace

import pytest
from cua_sandbox import Sandbox
from cua_sandbox.runtime.base import RuntimeInfo
from cua_sandbox.sandbox import _env_transport
from cua_sandbox.transport.agentless import AgentlessTransport, supports_agentless


class _Handle:
    """The parts of the SDK ``Sandbox`` handle the fallback uses."""

    def __init__(self, *, runtime: str = "lume", env: bool = False):
        services = {"env": 3211} if env else {}
        self._info = SimpleNamespace(location="local", runtime=runtime, services=services)
        self.commands: list[tuple[str, object]] = []

    def info(self):
        return self._info

    def lacks_spacesd(self) -> bool:
        return "env" not in self._info.services

    async def spacesd(self, _timeout):
        raise AssertionError("never probed: the image declares no cua-spacesd")

    async def guest_sh(self, line, timeout_ms):
        self.commands.append((line, timeout_ms))
        return SimpleNamespace(
            stdout=b"ProductName:\tmacOS\n",
            stderr=b"warn\n",
            exit=SimpleNamespace(code=0, success=True),
        )

    async def guest_screenshot(self):
        return SimpleNamespace(image=b"\x89PNG-from-vnc", width=1024, height=768)

    async def guest_display(self):
        return SimpleNamespace(
            url=lambda: "vnc://****@127.0.0.1:5901",
            url_with_password=lambda: "vnc://:pw@127.0.0.1:5901",
        )


def _info(handle) -> RuntimeInfo:
    return RuntimeInfo(
        host="127.0.0.1",
        api_port=0,
        vnc_port=5901,
        name="cua-e2e-mac",
        environment="mac",
        guest_server_port=None,
        native=handle,
    )


def test_only_local_lume_sandboxes_qualify():
    assert supports_agentless(_Handle())
    assert supports_agentless(_Handle(env=True))
    assert not supports_agentless(_Handle(runtime="qemu"))
    assert not supports_agentless(object())


async def test_spacesd_wins_when_it_answers():
    handle = _Handle(env=True)
    client = object()

    async def spacesd(_timeout):
        return client

    handle.spacesd = spacesd
    info = _info(handle)
    info.guest_server_port = 3211
    info.api_port = 0
    t = _env_transport(info, environment="mac")
    assert await t._env_or_fallback() is client
    assert handle.commands == []


async def test_shell_and_screen_use_the_sdk_fallback():
    handle = _Handle()
    sb = Sandbox(_env_transport(_info(handle), environment="mac"), name="cua-e2e-mac")

    r = await sb.shell.run("sw_vers", timeout=45)
    assert (r.stdout, r.stderr, r.returncode) == ("ProductName:\tmacOS\n", "warn\n", 0)
    assert handle.commands == [("sw_vers", 45000)]

    assert await sb.screen.screenshot() == b"\x89PNG-from-vnc"
    assert await sb.screen.size() == (1024, 768)
    assert await sb.get_display_url() == "vnc://****@127.0.0.1:5901"

    with pytest.raises(NotImplementedError, match="needs cua-spacesd"):
        await sb.mouse.click(1, 2)


async def test_other_sandboxes_keep_the_vnc_fallback():
    t = _env_transport(_info(_Handle(runtime="qemu")), environment="linux")
    assert not isinstance(t._fallback, AgentlessTransport)


def test_the_native_guest_display_never_prints_its_password():
    """The real binding: ``GuestDisplay`` is an opaque object. Its repr and
    str are ``object``'s (no fields), so the password shows only through
    the explicit ``url_with_password()``."""
    from cua import _native

    cls = _native.GuestDisplay
    for name in ("__repr__", "__str__", "__format__"):
        owner = next(c for c in cls.__mro__ if name in c.__dict__)
        assert owner is object, f"{owner.__name__} defines {name}"
    assert not hasattr(cls, "__dataclass_fields__")
    assert {"url", "url_with_password", "via", "open_command"} <= set(dir(cls))
    # A fake instance proves the default forms carry no attribute values.
    fake = object.__new__(cls)
    for shown in (repr(fake), str(fake)):
        assert "vnc://" not in shown and "pw" not in shown
