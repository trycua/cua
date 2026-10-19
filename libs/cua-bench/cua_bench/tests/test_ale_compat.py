"""cua-bench 0.2.x compatibility of RemoteDesktopSession (the ALE contract).

Agents' Last Exam (rdi-berkeley/agents-last-exam) runs cua-bench as a
library against its own computer-server VMs. These tests pin what it relies
on, with a loopback fake computer-server (no VM, no host command):

* Tier 2: the async method set, ``run_command`` returning ``return_code`` 0
  and never raising by default (0.2.7 semantics), no shell timeout;
* Tier 3: ``_os_type``/``_api_host``/``_api_port``/``_vnc_port``, a writable
  ``_computer``/``_initialized``, ``import computer`` and the cua-computer
  interface (``create_dir``, ``run_command -> CommandResult``, a patchable
  ``_send_command``) on computer-server and on cua-spacesd sandboxes;
* opt-in: ``strict_exit_codes`` and ``timeout``.
"""

from __future__ import annotations

import asyncio

import pytest
from cua_bench.computers.remote import RemoteDesktopSession, resolve_transport

from .fake_computer_server import FakeComputerServer
from .fakes import FakeSandbox


@pytest.fixture
async def server():
    fake = await FakeComputerServer().start()
    try:
        yield fake
    finally:
        await fake.stop()


def _init_computer_skip_wait(session):
    """ALE's ale_run/environments/providers/gcloud.py:722-744, verbatim logic."""
    from computer import Computer
    from computer.interface.factory import InterfaceFactory

    computer = Computer(
        os_type=session._os_type,
        use_host_computer_server=True,
        api_host=session._api_host,
        api_port=session._api_port,
        noVNC_port=session._vnc_port,
    )
    interface = InterfaceFactory.create_interface_for_os(
        os=session._os_type, ip_address=session._api_host, api_port=session._api_port
    )
    computer._interface = interface
    computer._original_interface = interface
    computer._initialized = True
    session._computer = computer
    session._initialized = True


def test_transport_selection():
    assert resolve_transport("http://10.0.0.5:5000") == "computer-server"
    assert resolve_transport("http://localhost:8000") == "computer-server"
    assert resolve_transport("http://10.0.0.5:3211") == "spacesd"
    assert resolve_transport("http://h:9", "spacesd") == "spacesd"
    with pytest.raises(ValueError):
        resolve_transport("http://h:9", "ssh")


def test_private_attributes_keep_their_meaning():
    session = RemoteDesktopSession(api_url="http://10.1.2.3:5000/", os_type="windows")
    assert (session._os_type, session._api_host, session._api_port) == ("windows", "10.1.2.3", 5000)
    assert session._vnc_port == 8006 and session._initialized is False
    assert session._computer is None
    bare = RemoteDesktopSession(api_url="http://vm")
    assert bare._api_port == 5000


async def test_client_only_session_speaks_computer_server(server):
    session = RemoteDesktopSession(api_url=server.url, os_type="linux")
    await session.run_command("mkdir -p /data/out", check=False)
    await session.write_file("/data/out/a.txt", "hello")
    assert await session.read_file("/data/out/a.txt") == "hello"
    assert await session.read_bytes("/data/out/a.txt") == b"hello"
    await session.write_bytes("/data/out/b.bin", b"\x00\x01")
    assert await session.file_exists("/data/out/a.txt") is True
    assert await session.file_exists("/data/out") is False  # files only, as in 0.2.7
    assert await session.directory_exists("/data/out") is True
    assert sorted(await session.list_dir("/data/out")) == ["a.txt", "b.bin"]
    assert (await session.screenshot()).startswith(b"\x89PNG")
    assert await session.check_status() is True
    assert await session.wait_until_ready(timeout=5) is True
    await session.close()


async def test_run_command_legacy_semantics_by_default(server):
    server.exit_codes["false"] = 1
    session = RemoteDesktopSession(api_url=server.url)
    result = await session.run_command("false")  # check=True by default
    assert result["return_code"] == 0 and result["success"] is True
    assert result["exit_code"] == 1  # the real code is additive
    assert set(result) >= {"stdout", "stderr", "return_code", "success"}
    out = await session.run_command("echo hi", check=False)
    assert out["stdout"] == "hi\n"


async def test_strict_exit_codes_opt_in(server, monkeypatch):
    server.exit_codes["false"] = 3
    strict = RemoteDesktopSession(api_url=server.url, strict_exit_codes=True)
    with pytest.raises(RuntimeError, match="return code 3"):
        await strict.run_command("false")
    assert (await strict.run_command("false", check=False))["return_code"] == 3
    monkeypatch.setenv("CUA_BENCH_STRICT_EXIT_CODES", "1")
    env_strict = RemoteDesktopSession(api_url=server.url)
    assert (await env_strict.run_command("false", check=False))["return_code"] == 3


async def test_ale_init_computer_skip_wait_and_interface(server):
    """ALE builds its own cua-computer Computer from the private attributes."""
    session = RemoteDesktopSession(api_url=server.url, os_type="linux")
    _init_computer_skip_wait(session)
    iface = session.computer.interface
    assert session.interface is iface
    await session.interface.create_dir("/media/user/data/agenthle/demo")
    assert "/media/user/data/agenthle/demo" in server.dirs
    await iface.write_text("/tmp/log.txt", "a")
    await iface.write_text("/tmp/log.txt", "b", append=True)
    assert await session.read_file("/tmp/log.txt") == "ab"
    result = await iface.run_command("echo x")
    from computer.interface.models import CommandResult

    assert isinstance(result, CommandResult) and result.returncode == 0
    await iface.delete_dir("/media/user/data/agenthle/demo")
    await iface.press_key("enter")
    iface.force_close()
    await session.close()
    assert session._computer is None


async def test_ale_resilient_send_command_patch_applies(server):
    """ALE wraps interface._send_command and interface.run_command in place."""
    session = RemoteDesktopSession(api_url=server.url)
    _init_computer_skip_wait(session)
    iface = session.computer.interface
    original = iface._send_command
    calls = []

    async def resilient(command, params=None):
        for _ in range(3):
            result = await original(command, params)
            calls.append(command)
            if result.get("success", True) or "Connection reset" not in result.get("error", ""):
                return result
        return result

    iface._send_command = resilient
    raw_run = iface.run_command

    async def patched_run(command):  # one positional arg, like ALE's wrapper
        return await raw_run(command)

    iface.run_command = patched_run
    iface._ale_resilient_commands = True
    server.fail_next.append("run_command")
    result = await session.run_command("echo ok", check=False)
    assert result["stdout"] == "ok\n"
    assert calls.count("run_command") == 2  # retried through the patched method
    # A session timeout wraps the patched run_command instead of passing timeout=.
    assert (await session.run_command("echo t", timeout=10))["stdout"] == "t\n"


async def test_computer_alias_and_run(server):
    """tasks/psychology_neuro: Computer(...).run() then .interface."""
    from computer import Computer

    computer = Computer(
        os_type="linux", use_host_computer_server=True, api_host="127.0.0.1", api_port=server.port
    )
    await computer.run()
    assert (await computer.interface.get_screen_size())["width"] == 1280
    with pytest.raises(RuntimeError, match="retired"):
        Computer(os_type="linux")


async def test_sandbox_session_has_the_cua_computer_surface():
    sb = FakeSandbox()
    session = RemoteDesktopSession.attach(sb, os_type="linux")
    iface = session.interface
    await iface.create_dir("/work")
    assert "/work" in sb.files.dirs
    await iface.write_text("/work/x.txt", "1")
    await iface.write_text("/work/x.txt", "2", append=True)
    assert await iface.read_text("/work/x.txt") == "12"
    assert await session.file_exists("/work/x.txt") and not await session.file_exists("/work")
    result = await iface.run_command("false")
    assert result.returncode == 1
    reply = await iface._send_command("run_command", {"command": "echo a"})
    assert reply["success"] and reply["stdout"] == "a\n" and reply["return_code"] == 0
    assert (await iface._send_command("file_exists", {"path": "/work/x.txt"}))["exists"]
    await iface.delete_dir("/work")
    # The sandbox's own API is still reachable through both facades.
    assert iface.files is sb.files and session.computer.interface is iface
    assert session.computer.files is sb.files
    assert (await session.run_command("false"))["return_code"] == 0  # 0.2.7 default


async def test_sandbox_shell_timeout_is_none_unless_asked(monkeypatch):
    sb = FakeSandbox()
    session = RemoteDesktopSession.attach(sb)
    await session.run_command("true", check=False)
    assert sb.shell.commands[-1] == ("true", None)
    await session.run_command("true", check=False, timeout=7)
    assert sb.shell.commands[-1] == ("true", 7)
    timed = RemoteDesktopSession.attach(sb, timeout=42)
    await timed.run_command("true", check=False)
    assert sb.shell.commands[-1] == ("true", 42)
    monkeypatch.setenv("CUA_BENCH_SHELL_TIMEOUT", "9")
    env_timed = RemoteDesktopSession.attach(sb)
    await env_timed.run_command("true", check=False)
    assert sb.shell.commands[-1] == ("true", 9.0)


async def test_computer_server_timeout_bounds_the_wait(server, monkeypatch):
    from cua_bench.compat import legacy_interface

    slow = legacy_interface.ComputerServerInterface._post

    async def slower(self, payload, timeout):
        if payload["command"] == "run_command":
            await asyncio.sleep(5)
        return await slow(self, payload, timeout)

    monkeypatch.setattr(legacy_interface.ComputerServerInterface, "_post", slower)
    session = RemoteDesktopSession(api_url=server.url)
    with pytest.raises(asyncio.TimeoutError):
        await session.run_command("sleep 100", timeout=0.2)
