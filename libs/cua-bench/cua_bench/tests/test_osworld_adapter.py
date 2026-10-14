"""OSWorld adapter (tasks/osworld) against a fake OSWorld server (hermetic).

The fixture tree (fixtures/osworld_tree) has the shape of the upstream
repo. ``CUA_BENCH_OSWORLD_ROOT=<checkout of xlang-ai/OSWorld@b138d34>`` also
runs the real upstream evaluators (``test_real_upstream_*``).
"""

from __future__ import annotations

import asyncio
import importlib.util
import json
import os
import re
import sys
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace

import pytest
from cua_bench.adapters import Endpoints

PKG = Path(__file__).resolve().parents[2]
FIXTURE = Path(__file__).resolve().parent / "fixtures" / "osworld_tree"
PARITY_ID = "e0df059f-28a6-4169-924f-b9623e7184cc"


def _load_main(name="osworld_main"):
    spec = importlib.util.spec_from_file_location(name, PKG / "tasks/osworld/main.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class FakeGuest:
    """A fake OSWorld server: /setup/execute and /execute over a tiny FS model."""

    def __init__(self):
        self.dirs: set[str] = set()
        self.files: dict[str, str] = {}
        self.commands: list[str] = []
        guest = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                self._send(200, {"status": "ok"})

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["content-length"])) or b"{}")
                out = guest.run(body.get("command"))
                self._send(200, {"status": "success", "output": out, "error": "", "returncode": 0})

            def _send(self, code, obj):
                data = json.dumps(obj).encode()
                self.send_response(code)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def run(self, command) -> str:
        cmd = command if isinstance(command, str) else " ".join(command)
        self.commands.append(cmd)
        if m := re.search(r"mkdir (\S+)", cmd):
            self.dirs.add(m.group(1))
        if m := re.search(r"mv (\S+) (\S+)", cmd):
            self.dirs.discard(m.group(1))
            self.dirs.add(m.group(2))
        if m := re.search(r"\[ -d (\S+) \]", cmd):
            return "Directory exists.\n" if m.group(1) in self.dirs else "Directory does not exist.\n"
        if cmd.startswith("echo 1"):
            return "1\n"
        return ""

    def close(self):
        self.server.shutdown()


class _Service:
    def __init__(self, port):
        self.port = port

    async def url(self):
        return f"http://127.0.0.1:{self.port}"

    async def request(self, method, path, json=None, timeout=30, **_):
        def call():
            req = urllib.request.Request(
                f"http://127.0.0.1:{self.port}{path}", method=method,
                data=__import__("json").dumps(json).encode(), headers={"content-type": "application/json"})
            with urllib.request.urlopen(req, timeout=timeout) as r:
                data = __import__("json").loads(r.read())
            return SimpleNamespace(status_code=200, json=lambda: data)

        return await asyncio.to_thread(call)


@pytest.fixture
def guest():
    g = FakeGuest()
    yield g
    g.close()


def _session(guest):
    sb = SimpleNamespace(exposed_ports={5000: guest.port, 9222: guest.port, 8080: guest.port},
                         service=lambda name: _Service(guest.port))
    return SimpleNamespace(sandbox=sb)


@pytest.fixture
def fixture_root(monkeypatch):
    monkeypatch.setenv("CUA_BENCH_OSWORLD_ROOT", str(FIXTURE))
    yield FIXTURE


def test_tasks_from_the_tree(fixture_root):
    mod = _load_main()
    tasks = mod.load()
    assert [t.metadata["osworld_id"] for t in tasks] == [PARITY_ID, "inf-1", "c-1"]
    os_task, _, chrome = tasks
    assert os_task.metadata["container_fidelity"] == "full"  # a file-only os task
    assert chrome.metadata["container_fidelity"] == "full"
    assert mod.container_fidelity({"x": "gsettings set ..."}, "chrome") == "degraded"
    assert mod.container_fidelity({"x": "install Spotify"}, "os") == "degraded"
    assert mod.container_fidelity({"x": "install an extension"}, "chrome") == "full"
    assert os_task.metadata["has_oracle"] and not chrome.metadata["has_oracle"]
    setup = os_task.computer["setup_config"]
    assert setup["server_port"] == 5000 and setup["ports"] == [9222, 8080]
    assert setup["requires"] == ["egress"] and setup["kinds"] == ["container", "vm"]
    assert setup["image"].startswith("ghcr.io/trycua/bench-osworld")


def test_split_env_and_unknown_split(fixture_root, monkeypatch):
    mod = _load_main()
    monkeypatch.setattr(mod.oracles, "PARITY", [PARITY_ID])
    assert [t.metadata["osworld_id"] for t in mod.OSWorldAdapter(split="parity").load_tasks("train")] == [PARITY_ID]
    with pytest.raises(ValueError, match="unknown OSWorld split"):
        mod.OSWorldAdapter(split="nope").load_tasks("train")


def test_shims_keep_host_safe_and_fail_late(fixture_root):
    mod = _load_main()
    ns = mod.upstream.import_desktop_env(FIXTURE)
    assert "easyocr_not_installed_anywhere" in ns.shimmed
    assert "pyautogui" not in sys.modules or not getattr(sys.modules["pyautogui"], "__cua_bench_shim__", False)
    with pytest.raises(mod.upstream.OSWorldDependencyError, match="easyocr_not_installed_anywhere"):
        ns.metrics.needs_ocr("x", {})
    # A library probing an optional dependency still gets ImportError.
    with pytest.raises(ImportError):
        __import__("easyocr_not_installed_anywhere")


def test_setup_oracle_and_evaluate(fixture_root, guest):
    mod = _load_main()
    adapter = mod.ADAPTER
    task = next(t for t in adapter.tasks("train") if t.metadata["osworld_id"] == PARITY_ID)
    session = _session(guest)
    ep = Endpoints(session, adapter.server)

    async def run():
        await adapter.setup(task, session, ep)
        noop = await adapter.evaluate(task, session, ep)
        await adapter.oracle(task, session, ep)
        solved = await adapter.evaluate(task, session, ep)
        return noop, solved

    noop, solved = asyncio.run(run())
    assert (noop, solved) == (0.0, 1.0)
    assert any("sudo -S mkdir" in c and "password" in c for c in guest.commands)  # {CLIENT_PASSWORD}


def test_infeasible_and_no_oracle(fixture_root, guest):
    mod = _load_main()
    adapter = mod.ADAPTER
    inf = next(t for t in adapter.tasks("train") if t.metadata["osworld_id"] == "inf-1")
    session = _session(guest)
    ep = Endpoints(session, adapter.server)
    assert asyncio.run(adapter.evaluate(inf, session, ep)) == 0.0
    session.infeasible = True
    assert asyncio.run(adapter.evaluate(inf, session, ep)) == 1.0
    chrome = next(t for t in adapter.tasks("train") if t.metadata["osworld_id"] == "c-1")
    with pytest.raises(mod.oracles.NoOracle):
        asyncio.run(adapter.oracle(chrome, session, ep))


REAL = os.environ.get("CUA_BENCH_OSWORLD_ROOT_REAL")


@pytest.mark.skipif(not REAL, reason="set CUA_BENCH_OSWORLD_ROOT_REAL to an OSWorld@b138d34 checkout")
def test_real_upstream_evaluators(guest, monkeypatch):
    """The real upstream SetupController/getters/metrics, two metric shapes."""
    monkeypatch.setenv("CUA_BENCH_OSWORLD_ROOT", REAL)
    mod = _load_main("osworld_main_real")
    adapter = mod.OSWorldAdapter(split="test_all")
    session = _session(guest)
    ep = Endpoints(session, adapter.server)
    root = adapter.root
    single = mod.upstream.load_task(root, "os", PARITY_ID)   # exact_match + vm_command_line
    task = mod.cb.Task(description="x", metadata={"osworld": single, "osworld_id": PARITY_ID})

    async def run(t):
        await adapter.setup(t, session, ep)
        before = await adapter.evaluate(t, session, ep)
        await adapter.oracle(t, session, ep)
        return before, await adapter.evaluate(t, session, ep)

    assert asyncio.run(run(task)) == (0.0, 1.0)
    # A metric list with a conjunction (bedcedc4: two exact_match on vm_command_line).
    multi = mod.upstream.load_task(root, "os", "bedcedc4-4d72-425e-ad62-21960b11fe0d")
    assert isinstance(multi["evaluator"]["func"], list)
    env = asyncio.run(adapter._env(mod.cb.Task(description="y", metadata={"osworld": multi}), ep))
    assert asyncio.run(asyncio.to_thread(env.evaluate)) == 0  # fake guest prints nothing matching
