"""``Sandbox.create(overlay=...)`` and ``Sandbox.overlay``: hermetic.

No sandbox is created: a stand-in native handle records the call. The real
overlay (copy, atomic rename, restart) is covered by the Rust SDK and the
cua-sandbox action's local lane.
"""

from __future__ import annotations

import importlib

import pytest

sandbox_mod = importlib.import_module("cua_sandbox.sandbox")
Sandbox = sandbox_mod.Sandbox


class _Handle:
    def __init__(self, fail: bool = False) -> None:
        self.calls: list = []
        self.fail = fail

    async def overlay(self, overlays, timeout_ms):
        self.calls.append((overlays, timeout_ms))
        if self.fail:
            raise RuntimeError("overlay installing: no root in the guest")
        return [f"result:{o.name}" for o in overlays]


class _Fake:
    """Just what ``_with_overlays`` and ``Sandbox.overlay`` use."""

    name = "cua-e2e-overlay"

    def __init__(self, handle: _Handle) -> None:
        self.handle = handle
        self.destroyed = False

    async def _native_handle(self):
        return self.handle

    async def destroy(self):
        self.destroyed = True

    overlay = Sandbox.overlay


def test_specs_from_a_dict_are_native_overlays():
    specs = sandbox_mod._overlay_specs(
        {"cua-driver": "./target/release/cua-driver", "tool": ("./tool", "/opt/bin/tool")}
    )
    assert [(o.name, o.path, o.target) for o in specs] == [
        ("cua-driver", "./target/release/cua-driver", None),
        ("tool", "./tool", "/opt/bin/tool"),
    ]
    assert sandbox_mod._overlay_specs(None) == []
    marker = object()
    assert sandbox_mod._overlay_specs([marker]) == [marker]


@pytest.mark.asyncio
async def test_overlay_calls_the_native_handle():
    fake = _Fake(_Handle())
    out = await fake.overlay({"cua-driver": "./cua-driver"}, timeout=90)
    assert out == ["result:cua-driver"]
    ((overlays, timeout_ms),) = fake.handle.calls
    assert overlays[0].name == "cua-driver" and timeout_ms == 90_000


@pytest.mark.asyncio
async def test_a_failed_overlay_deletes_the_new_sandbox():
    fake = _Fake(_Handle(fail=True))
    with pytest.raises(RuntimeError, match="no root"):
        await sandbox_mod._with_overlays(fake, {"cua-driver": "./cua-driver"})
    assert fake.destroyed


@pytest.mark.asyncio
async def test_no_overlay_is_a_no_op():
    fake = _Fake(_Handle())
    assert await sandbox_mod._with_overlays(fake, None) is fake
    assert fake.handle.calls == [] and not fake.destroyed
