"""sandbox_read / sandbox_write: quoting and error-reporting regressions.

Fixes trycua/cua#8465 and #8467 (reported by soarkit).
"""

from __future__ import annotations

import asyncio
import base64
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

pytest.importorskip("claude_agent_sdk")

from cua_sandbox_apps.pipeline import sandbox_tools  # noqa: E402

NASTY_NAMES = [
    "plain.txt",
    "with space.txt",
    "semi;colon.txt",
    "dollar$(touch PWNED).txt",
    "back`touch PWNED`tick.txt",
    "single'quote.txt",
    'double"quote.txt',
    "-leading-dash.txt",
]

NASTY_CONTENT = [
    "hello\n",
    "line with 'single' and \"double\" quotes\n",
    "$(touch PWNED); echo `touch PWNED` && rm -rf nothing\n",
    "before\nSANDBOX_EOF\ntouch PWNED\nafter\n",
    "no trailing newline",
    "backslash \\\\ and \\n literal\n",
]


class LocalShell:
    """Runs commands with bash in a scratch directory, like a real guest."""

    def __init__(self, cwd: Path):
        self.cwd = cwd
        self.commands: list[str] = []

    async def run(self, command: str, timeout: int = 120):
        self.commands.append(command)
        p = subprocess.run(
            ["bash", "-c", command], cwd=self.cwd, capture_output=True, text=True, timeout=timeout
        )
        return SimpleNamespace(stdout=p.stdout, stderr=p.stderr, returncode=p.returncode)


class FileApi:
    """File API backed by the scratch directory; never touches a shell."""

    def __init__(self, cwd: Path):
        self.cwd = cwd

    async def write_text(self, path: str, content: str) -> None:
        (self.cwd / path).write_text(content, encoding="utf-8")

    async def read_text(self, path: str) -> str:
        return (self.cwd / path).read_text(encoding="utf-8")


class NoFileApi:
    async def write_text(self, path: str, content: str) -> None:
        raise NotImplementedError("no file API on this image")

    async def read_text(self, path: str) -> str:
        raise NotImplementedError("no file API on this image")


def _tools(sb):
    registry = {"sb": sb}
    tools = {t.name: t.handler for t in sandbox_tools.make_sandbox_tools(sandbox_registry=registry)}
    return tools["sandbox_write"], tools["sandbox_read"]


def _text(result) -> str:
    return result["content"][0]["text"]


def _run(coro):
    return asyncio.run(coro)


@pytest.mark.parametrize("fallback", [False, True], ids=["file-api", "shell-fallback"])
@pytest.mark.parametrize("name", NASTY_NAMES)
@pytest.mark.parametrize("content", NASTY_CONTENT)
def test_roundtrip_is_literal(tmp_path, fallback, name, content):
    shell = LocalShell(tmp_path)
    sb = SimpleNamespace(shell=shell, files=NoFileApi() if fallback else FileApi(tmp_path))
    write, read = _tools(sb)

    out = _run(write({"name": "sb", "path": name, "content": content}))
    assert _text(out).startswith("OK"), _text(out)
    assert (tmp_path / name).read_text() == content
    assert not (tmp_path / "PWNED").exists()
    assert sorted(p.name for p in tmp_path.iterdir()) == [name]

    got = _run(read({"name": "sb", "path": name, "max_lines": 1000}))
    assert _text(got) == content
    assert not (tmp_path / "PWNED").exists()
    if not fallback:
        assert shell.commands == []


@pytest.mark.parametrize("fallback", [False, True], ids=["file-api", "shell-fallback"])
def test_read_truncates_to_max_lines(tmp_path, fallback):
    sb = SimpleNamespace(
        shell=LocalShell(tmp_path), files=NoFileApi() if fallback else FileApi(tmp_path)
    )
    write, read = _tools(sb)
    _run(write({"name": "sb", "path": "f.txt", "content": "".join(f"l{i}\n" for i in range(10))}))
    assert _text(_run(read({"name": "sb", "path": "f.txt", "max_lines": 3}))) == "l0\nl1\nl2\n"
    assert _text(_run(read({"name": "sb", "path": "f.txt", "max_lines": 0}))) == ""


@pytest.mark.parametrize("name", ["with space.sh", "x;y.sh", "q'uote.sh", "d$(id).sh"])
@pytest.mark.parametrize("fallback", [False, True], ids=["file-api", "shell-fallback"])
def test_executable_flag_quotes_path(tmp_path, fallback, name):
    sb = SimpleNamespace(
        shell=LocalShell(tmp_path), files=NoFileApi() if fallback else FileApi(tmp_path)
    )
    write, _ = _tools(sb)
    out = _run(write({"name": "sb", "path": name, "content": "#!/bin/sh\n", "executable": True}))
    assert _text(out).startswith("OK"), _text(out)
    assert (tmp_path / name).stat().st_mode & 0o111


def test_write_reports_real_error(tmp_path):
    sb = SimpleNamespace(shell=LocalShell(tmp_path), files=NoFileApi())
    write, _ = _tools(sb)
    out = _text(_run(write({"name": "sb", "path": "no/such/dir/f.txt", "content": "x"})))
    assert out.startswith("ERROR")
    assert "No such file or directory" in out


def test_write_reports_file_api_and_shell_exceptions(tmp_path):
    class Boom:
        async def run(self, command, timeout=120):
            raise RuntimeError("shell down")

    sb = SimpleNamespace(shell=Boom(), files=NoFileApi())
    write, read = _tools(sb)
    assert "shell down" in _text(_run(write({"name": "sb", "path": "f", "content": "x"})))
    assert "shell down" in _text(_run(read({"name": "sb", "path": "f"})))


def test_read_missing_file_is_error(tmp_path):
    sb = SimpleNamespace(shell=LocalShell(tmp_path), files=NoFileApi())
    _, read = _tools(sb)
    out = _text(_run(read({"name": "sb", "path": "missing.txt"})))
    assert out.startswith("ERROR")
    assert "missing.txt" in out


def test_unknown_sandbox():
    write, read = _tools(SimpleNamespace())
    assert "not found" in _text(_run(write({"name": "nope", "path": "p", "content": "c"})))
    assert "not found" in _text(_run(read({"name": "nope", "path": "p"})))


def test_no_heredoc_or_unquoted_interpolation_in_fallback(tmp_path):
    cmd = sandbox_tools._shell_write_command("a b;c", "SANDBOX_EOF\n", windows=False)
    assert "SANDBOX_EOF" not in cmd
    assert "<<" not in cmd
    assert "'a b;c'" in cmd


def test_windows_branch_is_encoded_powershell():
    for cmd in (
        sandbox_tools._shell_write_command("C:\\a b\\it's;x.txt", "$(calc)'\"", windows=True),
        sandbox_tools._shell_read_command("C:\\a b\\it's;x.txt", 5, windows=True),
    ):
        assert cmd.startswith("powershell -NoProfile")
        assert "-EncodedCommand" in cmd
        assert "calc" not in cmd and ";x" not in cmd
        script = base64.b64decode(cmd.rsplit(" ", 1)[1]).decode("utf-16-le")
        assert "'C:\\a b\\it''s;x.txt'" in script
    assert "-TotalCount 5" in base64.b64decode(cmd.rsplit(" ", 1)[1]).decode("utf-16-le")


def test_windows_sandbox_skips_chmod(tmp_path):
    shell = LocalShell(tmp_path)
    sb = SimpleNamespace(
        shell=shell,
        files=FileApi(tmp_path),
        _runtime_info=SimpleNamespace(environment="windows"),
    )
    write, _ = _tools(sb)
    out = _run(write({"name": "sb", "path": "a.bat", "content": "x", "executable": True}))
    assert _text(out).startswith("OK")
    assert shell.commands == []
