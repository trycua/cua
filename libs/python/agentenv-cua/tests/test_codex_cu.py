"""The Codex Computer Use driver's pins, login precondition and forwarding, offline: a fake MCP session, and no Mac."""

from __future__ import annotations

import asyncio
import contextlib
import json
from types import SimpleNamespace

import pytest
from agentenv_protocol.computer.platform import Platform
from mcp.types import CallToolResult, TextContent


def pinned(codex):
    return json.loads(codex.MANIFEST.read_text())


def test_it_declares_macos_on_apple_silicon_and_serves_js_and_js_reset(codex):
    card = codex.CodexComputerUse.declaration(Platform(os="macos", arch="arm64"))
    names = {t.name for t in card.capabilities.tools}
    assert {"js", "js_reset", "computer_click"} <= names
    assert [t["name"] for t in pinned(codex)["tools"]] == ["js", "js_reset"]


def test_the_tool_manifest_is_the_pinned_apps_and_leaves_out_host_hooks(codex):
    manifest = pinned(codex)
    assert (
        manifest["app_version"] == codex.APP_VERSION == "26.930.51102"
        and manifest["surfaces"] == "computer"
    )
    assert "turn_ended" not in {t["name"] for t in manifest["tools"]}
    js = manifest["tools"][0]
    assert (
        js["inputSchema"]["required"] == ["code"]
        and "Browser APIs are disabled" in js["description"]
    )


def test_the_pins_name_the_app_zip_and_executable(codex):
    assert codex.APP_ZIP_URL.endswith("ChatGPT-darwin-arm64-26.930.51102.zip")
    assert len(codex.APP_ZIP_SHA256) == len(codex.APP_EXECUTABLE_SHA256) == 64


def make_plugin(root, version="26.930.51102", env=None):
    folder = root / version
    folder.mkdir(parents=True)
    entry = {
        "command": "/app/node",
        "args": ["/app/cua-repl.mjs"],
        "env": env or {"CUA_REPL_ENABLED_SURFACES": "browser,computer", "X": "1"},
    }
    (folder / ".mcp.json").write_text(json.dumps({"mcpServers": {"cua_repl": entry}}))
    return folder


def test_the_newest_plugin_version_wins(codex, tmp_path):
    make_plugin(tmp_path, "26.930.51102")
    newer = make_plugin(tmp_path, "26.1007.21159")
    (tmp_path / "junk").mkdir()
    assert codex.newest_plugin_dir(tmp_path) == newer
    assert codex.newest_plugin_dir(tmp_path / "none") is None


def test_the_server_is_the_plugins_entry_unchanged_but_for_the_computer_surface(codex, tmp_path):
    params = codex.server_parameters(make_plugin(tmp_path), {"HOME": "/Users/lume"})
    assert (params.command, params.args) == ("/app/node", ["/app/cua-repl.mjs"])
    assert params.env["CUA_REPL_ENABLED_SURFACES"] == "computer" and params.env["X"] == "1"
    assert params.env["HOME"] == "/Users/lume"


def test_the_login_precondition_fails_clearly_and_names_the_fix(codex):
    missing = codex.login_check(1, "Not logged in")
    assert (
        not missing.ok
        and "ChatGPT login" in missing.detail
        and "login" in missing.detail
        and "Not logged in" in missing.detail
    )
    api_key = codex.login_check(0, "Logged in using an API key")
    assert not api_key.ok
    assert codex.login_check(0, "Logged in using ChatGPT").ok


def test_the_app_check_wants_the_pinned_version_and_executable(codex):
    assert codex.app_check(codex.APP_VERSION, codex.APP_EXECUTABLE_SHA256).ok
    wrong = codex.app_check("26.1007.21159", "abc")
    assert not wrong.ok and "26.1007.21159" in wrong.detail and codex.APP_VERSION in wrong.detail
    assert not codex.app_check(None, None).ok


def test_the_plugin_check_says_when_the_app_hasnt_installed_it(codex, tmp_path):
    check = codex.plugin_check(None)
    assert not check.ok and "ChatGPT login" in check.detail
    assert codex.plugin_check(make_plugin(tmp_path)).ok
    assert not codex.plugin_check(make_plugin(tmp_path, "26.1007.21159")).ok


def test_the_tools_check_compares_descriptions_and_schemas(codex):
    manifest = pinned(codex)["tools"]
    running = [
        {"name": t["name"], "description": t["description"], "inputSchema": t["inputSchema"]}
        for t in manifest
    ]
    assert codex.tools_check(running, manifest).ok
    running[0] = {**running[0], "description": "changed"}
    check = codex.tools_check(running, manifest)
    assert not check.ok and "js" in check.detail


class FakeSession:
    def __init__(self):
        self.calls = []

    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        if name == "boom":
            raise RuntimeError("tool failed")
        return CallToolResult(content=[TextContent(type="text", text=f"ran {name}")])


def fake_connect(session, started):
    @contextlib.asynccontextmanager
    async def connect():
        started.append(1)
        yield session

    return connect


async def test_one_server_process_serves_all_of_a_sessions_calls_and_stops_on_close(codex):
    session, started = FakeSession(), []
    repl = codex.Repl(fake_connect(session, started))
    first = await repl.call("call_tool", "js", {"code": "1"})
    second = await repl.call("call_tool", "js_reset", {})
    assert [c.text for r in (first, second) for c in r.content] == ["ran js", "ran js_reset"]
    assert started == [1] and session.calls == [("js", {"code": "1"}), ("js_reset", {})]
    await repl.close()


async def test_a_failed_call_reaches_its_caller_and_the_server_stays_up(codex):
    session, started = FakeSession(), []
    repl = codex.Repl(fake_connect(session, started))
    with pytest.raises(RuntimeError, match="tool failed"):
        await repl.call("call_tool", "boom", {})
    assert (await repl.call("call_tool", "js", {})).content[0].text == "ran js"
    await repl.close()


async def test_a_server_that_wont_start_is_an_unavailable_device(codex):
    from agentenv_protocol.computer import DeviceUnavailable

    @contextlib.asynccontextmanager
    async def connect():
        raise OSError("no such binary")
        yield

    repl = codex.Repl(connect)
    with pytest.raises(DeviceUnavailable, match="no such binary"):
        await repl.call("call_tool", "js", {})


def driver_with(codex, monkeypatch, *, login, pinned_ok=True):
    d = codex.CodexComputerUse()
    ok = SimpleNamespace(name="x", ok=pinned_ok, detail="")

    async def pinned_checks():
        return [codex.ReadyCheck("ChatGPT app", pinned_ok, "app")]

    async def login_status():
        return login

    async def answers():
        return codex.ReadyCheck("Computer Use answers", True, "ok")

    d._pinned_checks, d._login_status, d._answers = pinned_checks, login_status, answers
    return d


async def test_a_vm_without_the_login_fails_its_ready_check_with_the_instruction(
    codex, monkeypatch
):
    checks = await driver_with(codex, monkeypatch, login=(1, "Not logged in")).ready_checks()
    login = next(c for c in checks if c.name == "ChatGPT login")
    assert not login.ok and "codex" in login.detail and "login" in login.detail
    assert "Computer Use answers" not in [
        c.name for c in checks
    ]  # it doesn't try without the login


async def test_with_the_login_it_also_checks_that_computer_use_answers(codex, monkeypatch):
    checks = await driver_with(
        codex, monkeypatch, login=(0, "Logged in using ChatGPT")
    ).ready_checks()
    assert [c.name for c in checks] == [
        "ChatGPT app",
        "ChatGPT login",
        "Computer Use answers",
    ] and all(c.ok for c in checks)


async def test_the_login_is_checked_every_time_since_it_lives_outside_the_image(codex, monkeypatch):
    d = driver_with(codex, monkeypatch, login=(0, "Logged in using ChatGPT"))
    assert all(c.ok for c in await d.ready_checks())
    status = (1, "Not logged in")

    async def login_status():
        return status

    d._login_status = login_status
    assert not (await d.ready_checks())[1].ok


async def test_forwarding_without_the_plugin_is_a_clear_error_result(codex, monkeypatch):
    monkeypatch.setattr(codex, "newest_plugin_dir", lambda *a: None)
    monkeypatch.setattr(
        codex.ToolContext,
        "current",
        staticmethod(lambda: SimpleNamespace(caller=SimpleNamespace(session="s"))),
    )
    d = codex.CodexComputerUse()
    result = await d.forwarded_tools()[0].call(code="await cua.getState();")
    assert result.isError and "ChatGPT login" in result.content[0].text


async def test_a_call_goes_to_its_callers_own_repl(codex, monkeypatch, tmp_path):
    session, started = FakeSession(), []
    plugin = make_plugin(tmp_path)
    monkeypatch.setattr(codex, "newest_plugin_dir", lambda *a: plugin)
    monkeypatch.setattr(codex, "stdio_session", lambda params: fake_connect(session, started)())
    who = {"session": "a"}
    monkeypatch.setattr(
        codex.ToolContext,
        "current",
        staticmethod(lambda: SimpleNamespace(caller=SimpleNamespace(session=who["session"]))),
    )
    d = codex.CodexComputerUse()
    js = d.forwarded_tools()[0]
    await js.call(code="1")
    await js.call(code="2")
    who["session"] = "b"
    await js.call(code="3")
    assert len(started) == 2 and len(d._repls) == 2
    await asyncio.gather(*(r.close() for r in d._repls.values()))
