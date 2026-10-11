"""Hermetic tests for RemoteDesktopSession over a faked cua_sandbox (no VM, no network)."""

import base64
import sys
import types
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest
from cua_bench.computers import remote
from cua_bench.types import ClickAction, DragAction, HotkeyAction, ScrollAction


def _fake_sandbox():
    sb = MagicMock()
    for attr in ("click", "right_click", "double_click", "move", "scroll", "drag"):
        setattr(sb.mouse, attr, AsyncMock())
    sb.keyboard.keypress = AsyncMock()
    sb.keyboard.type = AsyncMock()
    sb.disconnect = AsyncMock()
    sb.destroy = AsyncMock()
    sb.get_display_url = AsyncMock(return_value="http://vnc")
    return sb


@pytest.fixture
def fake_sdk(monkeypatch):
    sb = _fake_sandbox()
    mod = types.ModuleType("cua_sandbox")
    mod.Sandbox = MagicMock()
    mod.Sandbox.connect = AsyncMock(return_value=sb)
    mod.Sandbox.create = AsyncMock(return_value=sb)
    mod.Image = MagicMock()
    mod.Image.from_registry = MagicMock(return_value="IMAGE")
    monkeypatch.setitem(sys.modules, "cua_sandbox", mod)
    return mod, sb


@pytest.mark.asyncio
async def test_client_only_mode_connects_by_url(fake_sdk):
    mod, sb = fake_sdk
    async with remote.RemoteDesktopSession(api_url="http://h:1234/", transport="spacesd") as session:
        assert session.sandbox is sb and session.computer.interface.sandbox is sb
    mod.Sandbox.connect.assert_awaited_once_with(url="http://h:1234")
    sb.disconnect.assert_awaited_once()
    sb.destroy.assert_not_called()


@pytest.mark.asyncio
async def test_full_lifecycle_uses_the_runner_lifecycle(monkeypatch):
    """No api_url: the sandbox comes from sandboxes.open_sandbox (the canonical
    image for the OS, the caps as SDK kwargs) and close() releases it."""
    from cua_bench import sandboxes

    from .fakes import FakeSDK

    sdk = FakeSDK()
    monkeypatch.setattr(sandboxes, "_sdk", sdk.pair)
    monkeypatch.delenv("CUA_BENCH_ON", raising=False)
    monkeypatch.delenv("CUA_BENCH_IMAGE", raising=False)
    session = remote.RemoteDesktopSession(os_type="linux", memory="2GB", cpu="2")
    await session.start()
    (call,) = sdk.calls
    assert call["on"] == "local" and call["cpu"] == 2 and call["memory_mb"] == 2048
    assert call["image"].ref.startswith("ghcr.io/trycua/linux")
    assert call["image"].kind == "container"
    assert sdk.live == 1
    await session.close()
    assert sdk.live == 0 and sdk.released == 1


@pytest.mark.asyncio
async def test_full_lifecycle_windows_and_cloud(monkeypatch):
    from cua_bench import sandboxes

    from .fakes import FakeSDK

    sdk = FakeSDK()
    monkeypatch.setattr(sandboxes, "_sdk", sdk.pair)
    monkeypatch.delenv("CUA_BENCH_IMAGE", raising=False)
    async with remote.RemoteDesktopSession(os_type="win11", provider_type="cloud"):
        pass
    (call,) = sdk.calls
    assert call["on"] == "cloud" and call["image"].builtin == "windows"
    assert call["image"].kind == "vm"
    assert sdk.released == 1


@pytest.mark.asyncio
async def test_actions_map_to_sandbox_interfaces(fake_sdk):
    _, sb = fake_sdk
    session = remote.RemoteDesktopSession(api_url="http://h:3211")
    await session.execute_action(ClickAction(x=1, y=2))
    sb.mouse.click.assert_awaited_once_with(1, 2)
    await session.execute_action(DragAction(from_x=1, from_y=2, to_x=3, to_y=4))
    sb.mouse.drag.assert_awaited_once_with(1, 2, 3, 4)
    await session.execute_action(HotkeyAction(keys=["ctrl", "c"]))
    sb.keyboard.keypress.assert_awaited_once_with(["ctrl", "c"])
    await session.execute_action(ScrollAction(direction="up", amount=300))
    sb.mouse.scroll.assert_awaited_once_with(960, 540, scroll_x=0, scroll_y=-3)


@pytest.mark.asyncio
async def test_python_command_ships_source_and_parses_result(fake_sdk):
    _, sb = fake_sdk
    captured = {}

    async def fake_run(command, timeout=30):
        captured["command"] = command
        encoded = command.split("b64decode('")[1].split("')")[0]
        script = base64.b64decode(encoded).decode()
        ns = {}
        import contextlib
        import io

        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            exec(script, ns)  # the guest would run this; here it is pure Python
        return SimpleNamespace(stdout=buf.getvalue(), stderr="", returncode=0)

    sb.shell.run = fake_run
    session = remote.RemoteDesktopSession(api_url="http://h:3211")
    await session._ensure_computer()

    @session._python_command()
    def _add(a, b=0):
        return {"sum": a + b}

    assert await _add(2, b=3) == {"sum": 5}
    assert captured["command"].startswith("python3 -c ")


@pytest.mark.asyncio
async def test_python_command_raises_on_missing_result(fake_sdk):
    _, sb = fake_sdk
    sb.shell.run = AsyncMock(return_value=SimpleNamespace(stdout="", stderr="boom", returncode=1))
    session = remote.RemoteDesktopSession(api_url="http://h:3211")
    await session._ensure_computer()

    @session._python_command()
    def _noop():
        return None

    with pytest.raises(RuntimeError, match="boom"):
        await _noop()


def test_parse_memory_mb():
    assert remote._parse_memory_mb("8GB") == 8192
    assert remote._parse_memory_mb("512MB") == 512
    assert remote._parse_memory_mb("") is None


@pytest.mark.asyncio
async def test_scroll_convenience_method(fake_sdk):
    _, sb = fake_sdk
    session = remote.RemoteDesktopSession(api_url="http://h:3211")
    await session.scroll("down", 200)
    sb.mouse.scroll.assert_awaited_once_with(960, 540, scroll_x=0, scroll_y=2)


@pytest.mark.asyncio
async def test_sandbox_refs_connect_by_ref(fake_sdk):
    mod, sb = fake_sdk
    session = remote.RemoteDesktopSession(api_url="local:bench-box")
    await session._ensure_computer()
    mod.Sandbox.connect.assert_awaited_once_with("local:bench-box")
    direct = remote.RemoteDesktopSession(api_url="direct:10.0.0.7:3211")
    assert (direct._api_host, direct._api_port) == ("10.0.0.7", 3211)
    assert remote.resolve_transport("cloud:box") == "spacesd"
    assert remote.resolve_transport("direct:[::1]:3211") == "spacesd"


# ── Registry tasks on a real window manager ────────────────────────────────


def _scripted_session(monkeypatch, js_results, rects):
    """A session whose bench-ui calls are scripted (no guest)."""
    session = remote.RemoteDesktopSession(api_url="http://h:3211")
    scripts, actions = [], []

    async def execute_javascript(pid, javascript):
        scripts.append(javascript)
        for marker, result in js_results:
            if marker in javascript:
                return result
        return None

    async def get_element_rect(pid, selector, *, space="window", timeout=0.5):
        return rects.get(selector)

    async def execute_action(action):
        actions.append(action)

    monkeypatch.setattr(session, "execute_javascript", execute_javascript)
    monkeypatch.setattr(session, "get_element_rect", get_element_rect)
    monkeypatch.setattr(session, "execute_action", execute_action)
    return session, scripts, actions


@pytest.mark.asyncio
async def test_screen_origin_is_the_client_area(monkeypatch):
    """Tasks map page to screen as rect + window.screenX/Y; a WM frame's title
    bar must not shift that (bench-ui knows the client origin)."""
    session, scripts, _ = _scripted_session(
        monkeypatch,
        [("[window.screenX, window.screenY]", [390, 250])],
        {"html": {"x": 394, "y": 279, "width": 500, "height": 300}},
    )
    await session._pin_client_origin(42)
    assert remote._CLIENT_ORIGIN_JS % (4, 29) in scripts

    # No frame (or no toolkit origin): nothing is patched.
    session, scripts, _ = _scripted_session(
        monkeypatch,
        [("[window.screenX, window.screenY]", [390, 250])],
        {"html": {"x": 390, "y": 250, "width": 500, "height": 300}},
    )
    await session._pin_client_origin(42)
    assert not any("defineProperty" in s for s in scripts)


@pytest.mark.asyncio
async def test_click_element_scrolls_into_view_and_picks_options(monkeypatch):
    session, scripts, actions = _scripted_session(
        monkeypatch,
        [("'OPTION'", "visible")],
        {"#submit": {"x": 100, "y": 200, "width": 40, "height": 20}},
    )
    await session.click_element(7, "#submit")
    assert "scrollIntoView" in scripts[0] and '"#submit"' in scripts[0]
    assert actions == [ClickAction(x=120, y=210)]

    # An <option> is picked in the page; the open native popup is closed.
    session, scripts, actions = _scripted_session(
        monkeypatch, [("'OPTION'", "option")], {}
    )
    await session.click_element(7, 'option[value="apple"]')
    assert [type(a).__name__ for a in actions] == ["KeyAction"]
    assert actions[0].key == "Escape"
