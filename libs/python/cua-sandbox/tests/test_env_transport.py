"""EnvTransport: how each interface action maps onto cua-spacesd.

Replaces the computer-server wire tests (test_computer_server_transport.py,
test_parse_sse.py, test_transport_http*.py): the public interfaces are driven
through the cua SDK's SpacesdClient now, so these assert which spacesd call
(typed method or ``cua.env.v1`` JSON RPC) every action becomes. A recording
fake stands in for the native client; test_env_transport_mock.py runs the same
interfaces against the SDK's MockServer over the real gRPC stack.
"""

from __future__ import annotations

import base64

import pytest
from cua_sandbox import Sandbox, SpacesdNotAvailable
from cua_sandbox.transport.base import Transport
from cua_sandbox.transport.env import EnvTransport, env_url, key_input

from tests._fake_env import FakeEnv


def sandbox_with(env: FakeEnv, **kwargs) -> Sandbox:
    async def factory():
        return env

    return Sandbox(EnvTransport(env_factory=factory, **kwargs), name="fake")


async def test_connect_never_probes_the_guest():
    calls = []

    async def factory():
        calls.append("probe")
        raise AssertionError("connect must not reach the spacesd")

    sb = Sandbox(EnvTransport(env_factory=factory), name="lazy")
    await sb._connect()
    await sb.disconnect()
    assert calls == []


async def test_scroll_sends_position_and_wheel_deltas():
    """computer-server treated (x, y) as the scroll amount; the env path sends
    the position and the deltas separately (positive scroll_y scrolls up)."""
    env = FakeEnv()
    sb = sandbox_with(env)

    await sb.mouse.scroll(640, 400, scroll_x=2, scroll_y=3)

    assert env.calls == [
        (
            "pointer_json",
            {
                "scroll": {
                    "position": {"x": 640.0, "y": 400.0},
                    "deltaX": 2.0,
                    "deltaY": -3.0,
                    "unit": "SCROLL_UNIT_LINE",
                }
            },
        )
    ]


async def test_mouse_actions_use_typed_calls_and_pointer_json():
    env = FakeEnv()
    sb = sandbox_with(env)

    await sb.mouse.click(1, 2)
    await sb.mouse.click(3, 4, button="right")
    await sb.mouse.click(5, 6, button="middle")
    await sb.mouse.right_click(7, 8)
    await sb.mouse.double_click(9, 10)
    await sb.mouse.move(11, 12)
    await sb.mouse.mouse_down(13, 14)
    await sb.mouse.mouse_up(15, 16, button="right")
    await sb.mouse.drag(1, 1, 50, 60)
    await sb.mouse.drag(1, 1, 50, 60, button="right")

    assert env.calls == [
        ("click", 1.0, 2.0),
        ("right_click", 3.0, 4.0),
        (
            "pointer_json",
            {
                "click": {
                    "position": {"x": 5.0, "y": 6.0},
                    "button": "MOUSE_BUTTON_MIDDLE",
                    "count": 1,
                }
            },
        ),
        ("right_click", 7.0, 8.0),
        ("double_click", 9.0, 10.0),
        ("move_to", 11.0, 12.0),
        (
            "pointer_json",
            {"down": {"position": {"x": 13.0, "y": 14.0}, "button": "MOUSE_BUTTON_LEFT"}},
        ),
        (
            "pointer_json",
            {"up": {"position": {"x": 15.0, "y": 16.0}, "button": "MOUSE_BUTTON_RIGHT"}},
        ),
        ("drag", 1.0, 1.0, 50.0, 60.0),
        (
            "pointer_json",
            {
                "drag": {
                    "from": {"x": 1.0, "y": 1.0},
                    "to": {"x": 50.0, "y": 60.0},
                    "path": [],
                    "button": "MOUSE_BUTTON_RIGHT",
                }
            },
        ),
    ]


async def test_keyboard_actions():
    env = FakeEnv()
    sb = sandbox_with(env)

    await sb.keyboard.type("hello")
    await sb.keyboard.keypress("enter")
    await sb.keyboard.keypress(["ctrl", "c"])
    await sb.keyboard.key_down("shift")
    await sb.keyboard.key_up("a")

    assert env.calls == [
        ("type_text", "hello"),
        ("press", "enter"),
        ("hotkey", ["ctrl", "c"]),
        ("keyboard_json", {"down": {"key": {"named": "KEY_SHIFT"}}}),
        ("keyboard_json", {"up": {"key": {"character": "a"}}}),
    ]


@pytest.mark.parametrize(
    "name, expected",
    [
        ("ctrl", {"named": "KEY_CONTROL"}),
        ("cmd", {"named": "KEY_META"}),
        ("KEY_ESCAPE", {"named": "KEY_ESCAPE"}),
        ("page-down", {"named": "KEY_PAGE_DOWN"}),
        ("f5", {"named": "KEY_F5"}),
        ("left", {"named": "KEY_ARROW_LEFT"}),
        ("é", {"character": "é"}),
    ],
)
def test_key_names_resolve_like_the_driver(name, expected):
    assert key_input(name) == expected


def test_unknown_key_names_are_rejected():
    with pytest.raises(ValueError, match="unknown key"):
        key_input("hyperdrive")


async def test_get_active_title_reads_the_focused_window():
    """computer-server had no handler for get_active_window_title."""
    env = FakeEnv(
        windows=[
            {"title": "Terminal", "focused": False},
            {"title": "Firefox — cua", "focused": True},
        ]
    )
    sb = sandbox_with(env)

    assert await sb.window.get_active_title() == "Firefox — cua"
    assert env.calls == [("call_json", "WindowsService/ListWindows", {})]


async def test_get_active_title_is_empty_without_a_focused_window():
    assert await sandbox_with(FakeEnv()).window.get_active_title() == ""


async def test_shell_run_maps_process_output():
    env = FakeEnv()
    sb = sandbox_with(env)

    ok = await sb.shell.run("echo hi", timeout=5)
    failed = await sb.shell.run("false")

    assert (ok.stdout, ok.returncode, ok.success) == ("out\n", 0, True)
    assert (failed.returncode, failed.stderr, failed.success) == (3, "boom\n", False)
    assert env.calls == [("sh", "echo hi", 5000), ("sh", "false", 30000)]


async def test_background_shell_and_terminal_use_pty_processes():
    env = FakeEnv()
    sb = sandbox_with(env)

    background = await sb.shell.run("sleep 60", background=True)
    session = await sb.terminal.create(cols=100, rows=30)
    await sb.terminal.send_input(session["pid"], "ls\n")
    info = await sb.terminal.info(session["pid"])
    assert await sb.terminal.close(session["pid"]) is True

    assert background.stdout == "100"
    assert session == {"pid": 101, "cols": 100, "rows": 30}
    assert info == {"pid": 101, "running": True, "pty": False}
    assert env.processes[1].pty_input == [b"ls\n"]
    assert env.processes[1].killed
    spawns = [c for c in env.calls if c[0] == "spawn"]
    assert spawns[0][1:3] == ("/bin/sh", ["-c", "sleep 60"])
    assert spawns[1][1:3] == ("/bin/sh", ["-l"])
    assert (spawns[1][3].cols, spawns[1][3].rows) == (100, 30)
    await sb.disconnect()
    assert env.processes[0].detached, "background processes outlive the connection"


async def test_files_round_trip():
    env = FakeEnv()
    sb = sandbox_with(env)
    payload = bytes(range(256)) * 4

    await sb.files.write_bytes("/tmp/blob", payload)
    await sb.files.write_text("/tmp/note.txt", "héllo")
    await sb.files.make_dir("/tmp/dir")

    assert await sb.files.read_bytes("/tmp/blob") == payload
    assert await sb.files.read_bytes("/tmp/blob", offset=10, length=5) == payload[10:15]
    assert await sb.files.read_text("/tmp/note.txt") == "héllo"
    assert await sb.files.exists("/tmp/blob") is True
    assert await sb.files.exists("/tmp/missing") is False
    assert await sb.files.exists("/tmp/dir") is False
    assert await sb.files.is_dir("/tmp/dir") is True
    assert await sb.files.size("/tmp/blob") == len(payload)
    names = sorted(e.name for e in await sb.files.list("/tmp"))
    assert names == ["blob", "note.txt"]
    await sb.files.remove("/tmp/blob")
    await sb.files.remove_dir("/tmp/dir")
    assert ("remove", "/tmp/blob", False) in env.calls
    assert ("remove", "/tmp/dir", True) in env.calls
    assert env.files["/tmp/note.txt"] == "héllo".encode()


async def test_clipboard_screen_and_environment():
    env = FakeEnv(os_family="macos")
    sb = sandbox_with(env)

    await sb.clipboard.set("copied")
    assert await sb.clipboard.get() == "copied"
    png = await sb.screenshot()
    assert png.startswith(b"\x89PNG")
    assert await sb.screenshot_base64() == base64.b64encode(png).decode()
    assert await sb.get_dimensions() == (1280, 800)
    assert await sb.get_environment() == "mac"
    assert await sb.spacesd() is env
    assert not hasattr(sb, "env")


async def test_raw_rpc_passthrough():
    env = FakeEnv()
    sb = sandbox_with(env)

    assert await sb._transport.send("SystemService/Health") == {}
    assert env.calls == [("call_json", "SystemService/Health", {})]


class _Fallback(Transport):
    def __init__(self):
        self.sent = []
        self.connected = False

    async def connect(self):
        self.connected = True

    async def disconnect(self):
        self.connected = False

    async def send(self, action, **params):
        self.sent.append((action, params))

    async def screenshot(self, format="png", quality=95):
        return b"\x89PNG-from-qmp"

    async def get_screen_size(self):
        return {"width": 800, "height": 600}

    async def get_environment(self):
        return "linux"


async def test_without_spacesd_the_agentless_fallback_takes_over():
    from cua._native import CuaError

    async def factory():
        raise CuaError.SpacesdNotAvailable("nothing on 3211")

    fallback = _Fallback()
    sb = Sandbox(EnvTransport(env_factory=factory, fallback=fallback), name="plain")

    assert await sb.screenshot() == b"\x89PNG-from-qmp"
    await sb.mouse.click(1, 2)
    assert fallback.connected
    assert fallback.sent == [("left_click", {"x": 1, "y": 2, "button": "left"})]
    with pytest.raises(SpacesdNotAvailable):
        await sb.spacesd()


async def test_without_spacesd_or_fallback_interfaces_raise():
    from cua._native import CuaError

    async def factory():
        raise CuaError.SpacesdNotAvailable("nothing on 3211")

    sb = Sandbox(EnvTransport(env_factory=factory), name="plain")
    with pytest.raises(SpacesdNotAvailable, match="nothing on 3211"):
        await sb.shell.run("true")


async def test_the_driver_is_retried_until_ready_timeout():
    from cua._native import CuaError

    attempts = []
    env = FakeEnv()

    async def factory():
        attempts.append(1)
        if len(attempts) < 3:
            raise CuaError.SpacesdNotAvailable("booting")
        return env

    sb = Sandbox(EnvTransport(env_factory=factory, ready_timeout=30), name="booting")
    await sb.clipboard.set("x")
    assert len(attempts) == 3


def test_env_url_brackets_ipv6():
    assert env_url("10.0.0.5") == "http://10.0.0.5:3211"
    assert env_url("::1", 4000) == "http://[::1]:4000"


def test_computer_server_ws_url_is_rejected_with_guidance():
    from cua_sandbox.sandbox import _make_transport

    with pytest.raises(ValueError, match="cua-spacesd"):
        _make_transport(ws_url="ws://host:8000/ws")
    assert isinstance(_make_transport(http_url="http://host:3211"), EnvTransport)
