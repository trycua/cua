"""Integration tests for every sandbox interface method across all guest OS types.

Imports exclusively from `cua` (the meta-package). Tests auto-skip via
skip_if_unsupported() when the required local runtime is absent.

Interface coverage:
  sb.shell      — Shell.run()
  sb.mouse      — click, right_click, double_click, move, scroll, mouse_down, mouse_up, drag
  sb.keyboard   — type, keypress, key_down, key_up
  sb.screen     — screenshot, screenshot_base64, size
  sb.clipboard  — get, set
  sb.tunnel     — forward (single, multi, await form)
  sb.terminal   — create, send_input, resize, close
  sb.window     — get_active_title
  sb.mobile     — tap, long_press, double_tap, type_text, swipe, scroll_up/down/left/right,
                  fling, gesture, pinch_in, pinch_out, key, home, back, recents, power,
                  volume_up, volume_down, enter, backspace, wake, notifications,
                  close_notifications

Guest OS coverage:
  Linux container  (Image.linux())                — shell, clipboard, tunnel, terminal
  Linux VM         (Image.linux(kind="vm"))       — + screen, mouse, keyboard, window
  macOS VM         (Image.macos())                — shell, screen, mouse, keyboard, clipboard,
                                                    tunnel, terminal, window
  Windows VM       (Image.windows())              — shell, screen, mouse, keyboard, clipboard,
                                                    tunnel, terminal, window
  Android VM       (Image.android())              — shell, screen, mobile.*

Run with:
    pytest tests/test_interfaces.py -v
    pytest tests/test_interfaces.py -v -k "linux and shell"
"""

from __future__ import annotations

import asyncio
import base64

import httpx
import pytest

from cua import Image, Sandbox, skip_if_unsupported

# ── helpers ──────────────────────────────────────────────────────────────────


async def _shell(sb, cmd: str, timeout: int = 30) -> str:
    r = await sb.shell.run(cmd, timeout=timeout)
    assert r.success, f"Command failed (rc={r.returncode}): {r.stderr}"
    return r.stdout.strip()


async def _start_http_server(sb, port: int) -> None:
    """Start a minimal Python HTTP echo server inside the sandbox.

    Uses python3 with POSIX background operator on Linux/macOS.
    macOS VMs ship python3; Linux containers have it in the base image.
    """
    server_code = (
        "import http.server as h, socketserver as ss; "
        f"ss.TCPServer(('0.0.0.0',{port}),"
        "type('H',(h.BaseHTTPRequestHandler,),"
        "{'do_GET':lambda s:(s.send_response(200),s.send_header('Content-Type','text/plain')"
        ",s.end_headers(),s.wfile.write(b'ok'))})"
        ").serve_forever()"
    )
    await sb.shell.run(f'python3 -c "{server_code}" &', timeout=5)
    await asyncio.sleep(1)


# ═══════════════════════════════════════════════════════════════════════════
# Shell — Linux container
# ═══════════════════════════════════════════════════════════════════════════


class TestShellLinux:
    _IMAGE = Image.linux()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_run_success(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("echo hello")
            assert r.success
            assert r.returncode == 0
            assert "hello" in r.stdout

    async def test_run_failure_returncode(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("false")
            assert not r.success
            assert r.returncode != 0

    async def test_run_stderr(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("ls /no_such_dir_xyz 2>&1; true")
            # At minimum the command ran
            assert r.returncode is not None

    async def test_run_pipeline(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("echo hello | tr a-z A-Z")
            assert r.success
            assert "HELLO" in r.stdout

    async def test_run_multiline_output(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("printf 'a\\nb\\nc\\n'")
            assert r.success
            assert r.stdout.strip().splitlines() == ["a", "b", "c"]

    async def test_run_sequential_state(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r1 = await sb.shell.run("echo persistent > /tmp/st.txt")
            assert r1.success
            r2 = await sb.shell.run("cat /tmp/st.txt")
            assert r2.success
            assert "persistent" in r2.stdout

    async def test_run_timeout(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            with pytest.raises(BaseException):  # noqa: B017
                await sb.shell.run("sleep 60", timeout=2)

    async def test_commandresult_fields(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("echo out; echo err >&2; exit 0")
            assert isinstance(r.stdout, str)
            assert isinstance(r.stderr, str)
            assert isinstance(r.returncode, int)
            assert r.success is True


class TestShellMacOS:
    _IMAGE = Image.macos()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_run_success(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("echo hello macos")
            assert r.success
            assert "hello" in r.stdout

    async def test_run_pipeline(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("echo hello | tr a-z A-Z")
            assert r.success
            assert "HELLO" in r.stdout


class TestShellWindows:
    _IMAGE = Image.windows()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_run_success(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("echo hello windows")
            assert r.success
            assert "hello" in r.stdout

    async def test_run_failure(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("exit 1")
            assert not r.success


class TestShellAndroid:
    _IMAGE = Image.android()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_run_adb_shell(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("echo hello android")
            assert r.success
            assert "hello" in r.stdout

    async def test_run_getprop(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            r = await sb.shell.run("getprop ro.build.version.release")
            assert r.success
            assert r.stdout.strip()  # non-empty version string


# ═══════════════════════════════════════════════════════════════════════════
# Clipboard — Linux container, macOS, Windows
# ═══════════════════════════════════════════════════════════════════════════


class TestClipboardLinux:
    _IMAGE = Image.linux()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_set_and_get(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.clipboard.set("hello clipboard")
            text = await sb.clipboard.get()
            assert text == "hello clipboard"

    async def test_overwrite(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.clipboard.set("first")
            await sb.clipboard.set("second")
            assert await sb.clipboard.get() == "second"

    async def test_unicode(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            payload = "héllo wörld 🌸"
            await sb.clipboard.set(payload)
            assert await sb.clipboard.get() == payload

    async def test_multiline(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            payload = "line1\nline2\nline3"
            await sb.clipboard.set(payload)
            assert await sb.clipboard.get() == payload

    async def test_empty_string(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.clipboard.set("non-empty")
            await sb.clipboard.set("")
            assert await sb.clipboard.get() == ""


class TestClipboardMacOS:
    _IMAGE = Image.macos()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_set_and_get(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.clipboard.set("macos clipboard")
            assert await sb.clipboard.get() == "macos clipboard"

    async def test_unicode(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            payload = "café ☕"
            await sb.clipboard.set(payload)
            assert await sb.clipboard.get() == payload


class TestClipboardWindows:
    _IMAGE = Image.windows()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_set_and_get(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.clipboard.set("windows clipboard")
            assert await sb.clipboard.get() == "windows clipboard"


# ═══════════════════════════════════════════════════════════════════════════
# Screen — Linux VM, macOS, Windows, Android
# ═══════════════════════════════════════════════════════════════════════════

_LINUX_VM = Image.linux(kind="vm")


class TestScreenLinuxVM:
    def setup_method(self):
        skip_if_unsupported(_LINUX_VM)

    async def test_screenshot_returns_bytes(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            data = await sb.screen.screenshot()
            assert isinstance(data, bytes) and len(data) > 0

    async def test_screenshot_png_magic(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            data = await sb.screen.screenshot(format="png")
            assert data[:8] == b"\x89PNG\r\n\x1a\n"

    async def test_screenshot_jpeg(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            data = await sb.screen.screenshot(format="jpeg", quality=80)
            assert data[:2] == b"\xff\xd8"

    async def test_screenshot_base64(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            b64 = await sb.screen.screenshot_base64()
            assert isinstance(b64, str)
            assert len(base64.b64decode(b64)) > 0

    async def test_size(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            w, h = await sb.screen.size()
            assert isinstance(w, int) and w >= 640
            assert isinstance(h, int) and h >= 480


class TestScreenMacOS:
    _IMAGE = Image.macos()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_screenshot_returns_bytes(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            data = await sb.screen.screenshot()
            assert isinstance(data, bytes) and len(data) > 0

    async def test_screenshot_png_magic(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            data = await sb.screen.screenshot(format="png")
            assert data[:8] == b"\x89PNG\r\n\x1a\n"

    async def test_screenshot_base64(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            b64 = await sb.screen.screenshot_base64()
            decoded = base64.b64decode(b64)
            assert len(decoded) > 0

    async def test_size(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            assert w >= 640 and h >= 480


class TestScreenWindows:
    _IMAGE = Image.windows()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_screenshot_returns_bytes(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            data = await sb.screen.screenshot()
            assert isinstance(data, bytes) and len(data) > 0

    async def test_size(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            assert w > 0 and h > 0


class TestScreenAndroid:
    _IMAGE = Image.android()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_screenshot_returns_bytes(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            data = await sb.screen.screenshot()
            assert isinstance(data, bytes) and len(data) > 0

    async def test_size(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            assert w > 0 and h > 0


# ═══════════════════════════════════════════════════════════════════════════
# Mouse — Linux VM, macOS, Windows
# ═══════════════════════════════════════════════════════════════════════════


class TestMouseLinuxVM:
    def setup_method(self):
        skip_if_unsupported(_LINUX_VM)

    async def test_click(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.click(w // 2, h // 2)  # must not raise

    async def test_right_click(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.right_click(w // 2, h // 2)

    async def test_double_click(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.double_click(w // 2, h // 2)

    async def test_move(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            await sb.mouse.move(100, 100)
            await sb.mouse.move(200, 200)

    async def test_scroll(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.scroll(w // 2, h // 2, scroll_y=3)

    async def test_mouse_down_and_up(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.mouse_down(w // 2, h // 2)
            await sb.mouse.mouse_up(w // 2, h // 2)

    async def test_drag(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.drag(100, h // 2, 300, h // 2)


class TestMouseMacOS:
    _IMAGE = Image.macos()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_click(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.click(w // 2, h // 2)

    async def test_right_click(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.right_click(w // 2, h // 2)

    async def test_double_click(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.double_click(w // 2, h // 2)

    async def test_move(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mouse.move(100, 100)

    async def test_scroll(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.scroll(w // 2, h // 2, scroll_y=3)

    async def test_mouse_down_and_up(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.mouse_down(w // 2, h // 2)
            await sb.mouse.mouse_up(w // 2, h // 2)

    async def test_drag(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.drag(100, h // 2, 300, h // 2)


class TestMouseWindows:
    _IMAGE = Image.windows()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_click(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.click(w // 2, h // 2)

    async def test_right_click(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.right_click(w // 2, h // 2)

    async def test_double_click(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.double_click(w // 2, h // 2)

    async def test_move(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mouse.move(100, 100)

    async def test_scroll(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.scroll(w // 2, h // 2, scroll_y=3)

    async def test_mouse_down_and_up(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.mouse_down(w // 2, h // 2)
            await sb.mouse.mouse_up(w // 2, h // 2)

    async def test_drag(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mouse.drag(100, h // 2, 300, h // 2)


# ═══════════════════════════════════════════════════════════════════════════
# Keyboard — Linux VM, macOS, Windows
# ═══════════════════════════════════════════════════════════════════════════


class TestKeyboardLinuxVM:
    def setup_method(self):
        skip_if_unsupported(_LINUX_VM)

    async def test_type(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            await sb.keyboard.type("hello")  # must not raise

    async def test_keypress_single(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            await sb.keyboard.keypress("enter")

    async def test_keypress_combination(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            await sb.keyboard.keypress(["ctrl", "a"])

    async def test_keypress_string_list(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            await sb.keyboard.keypress(["shift", "tab"])

    async def test_key_down_and_up(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            await sb.keyboard.key_down("shift")
            await sb.keyboard.key_up("shift")

    async def test_type_unicode(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            await sb.keyboard.type("héllo")


class TestKeyboardMacOS:
    _IMAGE = Image.macos()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_type(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.keyboard.type("hello macos")

    async def test_keypress_single(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.keyboard.keypress("enter")

    async def test_keypress_combination(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.keyboard.keypress(["cmd", "a"])

    async def test_key_down_and_up(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.keyboard.key_down("shift")
            await sb.keyboard.key_up("shift")


class TestKeyboardWindows:
    _IMAGE = Image.windows()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_type(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.keyboard.type("hello windows")

    async def test_keypress_single(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.keyboard.keypress("enter")

    async def test_keypress_combination(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.keyboard.keypress(["ctrl", "a"])

    async def test_key_down_and_up(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.keyboard.key_down("shift")
            await sb.keyboard.key_up("shift")


# ═══════════════════════════════════════════════════════════════════════════
# Tunnel — Linux container, macOS, Windows
# ═══════════════════════════════════════════════════════════════════════════


class TestTunnelLinux:
    _IMAGE = Image.linux()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_forward_single_context_manager(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await _start_http_server(sb, 8888)
            async with sb.tunnel.forward(8888) as t:
                assert t.host
                assert t.port > 0
                assert t.sandbox_port == 8888
                assert t.url.startswith("http://")
                async with httpx.AsyncClient() as client:
                    r = await client.get(t.url, timeout=5)
                assert r.status_code == 200

    async def test_tunnel_info_repr(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await _start_http_server(sb, 8889)
            async with sb.tunnel.forward(8889) as t:
                assert "8889" in repr(t)

    async def test_forward_multiple_ports(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await _start_http_server(sb, 8890)
            await _start_http_server(sb, 8891)
            async with sb.tunnel.forward(8890, 8891) as tunnels:
                assert 8890 in tunnels
                assert 8891 in tunnels
                async with httpx.AsyncClient() as client:
                    for t in tunnels.values():
                        r = await client.get(t.url, timeout=5)
                        assert r.status_code == 200

    async def test_forward_await_form(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await _start_http_server(sb, 8892)
            t = await sb.tunnel.forward(8892)
            try:
                async with httpx.AsyncClient() as client:
                    r = await client.get(t.url, timeout=5)
                assert r.status_code == 200
            finally:
                await t.close()

    async def test_forward_no_ports_raises(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            with pytest.raises(ValueError):
                sb.tunnel.forward()


class TestTunnelMacOS:
    _IMAGE = Image.macos()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_forward_single(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await _start_http_server(sb, 8888)
            async with sb.tunnel.forward(8888) as t:
                assert t.sandbox_port == 8888
                async with httpx.AsyncClient() as client:
                    r = await client.get(t.url, timeout=5)
                assert r.status_code == 200


class TestTunnelWindows:
    _IMAGE = Image.windows()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_forward_single(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await _start_http_server(sb, 8888)
            async with sb.tunnel.forward(8888) as t:
                async with httpx.AsyncClient() as client:
                    r = await client.get(t.url, timeout=5)
                assert r.status_code == 200


# ═══════════════════════════════════════════════════════════════════════════
# Terminal (PTY) — Linux container, macOS, Windows
# ═══════════════════════════════════════════════════════════════════════════


class TestTerminalLinux:
    _IMAGE = Image.linux()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_create_returns_pid(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create()
            assert "pid" in info and isinstance(info["pid"], int)
            await sb.terminal.close(info["pid"])

    async def test_create_custom_shell(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create(shell="sh")
            assert info["pid"] > 0
            await sb.terminal.close(info["pid"])

    async def test_create_custom_dimensions(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create(cols=120, rows=40)
            assert info.get("cols") == 120
            assert info.get("rows") == 40
            await sb.terminal.close(info["pid"])

    async def test_send_input(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create()
            await sb.terminal.send_input(info["pid"], "echo hello\n")
            await asyncio.sleep(0.3)
            await sb.terminal.close(info["pid"])

    async def test_resize(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create(cols=80, rows=24)
            await sb.terminal.resize(info["pid"], cols=160, rows=48)
            await sb.terminal.close(info["pid"])

    async def test_close_returns_exit_code_or_none(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create()
            await sb.terminal.send_input(info["pid"], "exit 0\n")
            await asyncio.sleep(0.3)
            result = await sb.terminal.close(info["pid"])
            assert result is None or isinstance(result, int)

    async def test_multiple_sessions_independent(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            a = await sb.terminal.create()
            b = await sb.terminal.create()
            assert a["pid"] != b["pid"]
            await sb.terminal.close(a["pid"])
            await sb.terminal.close(b["pid"])


class TestTerminalMacOS:
    _IMAGE = Image.macos()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_create_and_close(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create()
            assert info["pid"] > 0
            await sb.terminal.send_input(info["pid"], "echo hi\n")
            await asyncio.sleep(0.3)
            await sb.terminal.close(info["pid"])

    async def test_resize(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create(cols=80, rows=24)
            await sb.terminal.resize(info["pid"], 120, 40)
            await sb.terminal.close(info["pid"])


class TestTerminalWindows:
    _IMAGE = Image.windows()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_create_and_close(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create(shell="cmd.exe")
            assert info["pid"] > 0
            await sb.terminal.close(info["pid"])

    async def test_send_input(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            info = await sb.terminal.create(shell="cmd.exe")
            await sb.terminal.send_input(info["pid"], "echo hello\r\n")
            await asyncio.sleep(0.5)
            await sb.terminal.close(info["pid"])


# ═══════════════════════════════════════════════════════════════════════════
# Window — Linux VM, macOS, Windows
# ═══════════════════════════════════════════════════════════════════════════


class TestWindowLinuxVM:
    def setup_method(self):
        skip_if_unsupported(_LINUX_VM)

    async def test_get_active_title_returns_string(self):
        async with Sandbox.ephemeral(_LINUX_VM, local=True) as sb:
            title = await sb.window.get_active_title()
            assert isinstance(title, str)


class TestWindowMacOS:
    _IMAGE = Image.macos()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_get_active_title_returns_string(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            title = await sb.window.get_active_title()
            assert isinstance(title, str)


class TestWindowWindows:
    _IMAGE = Image.windows()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def test_get_active_title_returns_string(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            title = await sb.window.get_active_title()
            assert isinstance(title, str)


# ═══════════════════════════════════════════════════════════════════════════
# Mobile — Android only
# ═══════════════════════════════════════════════════════════════════════════


class TestMobileAndroid:
    _IMAGE = Image.android()

    def setup_method(self):
        skip_if_unsupported(self._IMAGE)

    async def _sb(self):
        return Sandbox.ephemeral(self._IMAGE, local=True)

    # Touch
    async def test_tap(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.tap(w // 2, h // 2)

    async def test_long_press(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.long_press(w // 2, h // 2, duration_ms=500)

    async def test_double_tap(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.double_tap(w // 2, h // 2)

    async def test_type_text(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.type_text("hello")

    # Gestures
    async def test_swipe(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.swipe(w // 2, h * 3 // 4, w // 2, h // 4)

    async def test_scroll_up(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.scroll_up(w // 2, h // 2)

    async def test_scroll_down(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.scroll_down(w // 2, h // 2)

    async def test_scroll_left(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.scroll_left(w // 2, h // 2)

    async def test_scroll_right(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.scroll_right(w // 2, h // 2)

    async def test_fling(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.fling(w // 2, h * 3 // 4, w // 2, h // 4)

    async def test_pinch_in(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.pinch_in(w // 2, h // 2, spread=200)

    async def test_pinch_out(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            await sb.mobile.pinch_out(w // 2, h // 2, spread=200)

    async def test_gesture_two_finger(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            w, h = await sb.screen.size()
            cx, cy = w // 2, h // 2
            await sb.mobile.gesture(
                (cx - 20, cy),
                (cx - 200, cy),
                (cx + 20, cy),
                (cx + 200, cy),
            )

    async def test_gesture_invalid_raises(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            with pytest.raises(ValueError):
                await sb.mobile.gesture((100, 100), (200, 200), (300, 300))

    # Hardware keys
    async def test_key_home(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.home()

    async def test_key_back(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.back()

    async def test_key_recents(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.recents()

    async def test_key_volume_up(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.volume_up()

    async def test_key_volume_down(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.volume_down()

    async def test_key_enter(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.enter()

    async def test_key_backspace(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.backspace()

    async def test_key_power(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.power()

    async def test_key_arbitrary(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.key(3)  # HOME keycode

    # System
    async def test_wake(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.wake()

    async def test_notifications(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.notifications()

    async def test_close_notifications(self):
        async with Sandbox.ephemeral(self._IMAGE, local=True) as sb:
            await sb.mobile.close_notifications()
