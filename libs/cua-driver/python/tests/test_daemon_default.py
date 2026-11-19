"""CuaDriver.create() prefers the macOS daemon so the agent cursor works."""

import importlib.util
import warnings
from pathlib import Path


def load():
    path = Path(__file__).resolve().parents[1] / "src/cua_driver/_daemon.py"
    spec = importlib.util.spec_from_file_location("cua_driver_daemon", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module._warned = False
    return module


class Fake:
    def __init__(self, name, up_after=0):
        self.name = name
        self.calls = 0
        self.up_after = up_after

    def is_available(self):
        self.calls += 1
        return self.calls > self.up_after


def run(module, daemon, **kwargs):
    embedded = Fake("embedded")
    kwargs.setdefault("platform", "darwin")
    kwargs.setdefault("sleep", lambda _: None)
    result = module.create_driver(lambda: daemon, lambda: embedded, **kwargs)
    return result, embedded


def test_running_daemon_is_used(monkeypatch):
    monkeypatch.delenv("CUA_DRIVER_EMBEDDED", raising=False)
    m = load()
    daemon = Fake("daemon")
    result, _ = run(m, daemon)
    assert result is daemon


def test_installed_app_is_launched(monkeypatch):
    monkeypatch.delenv("CUA_DRIVER_EMBEDDED", raising=False)
    m = load()
    daemon = Fake("daemon", up_after=2)
    launched = []
    result, _ = run(
        m, daemon, app_installed=lambda: True, launch=lambda: launched.append(1) or True
    )
    assert result is daemon and launched == [1]


def test_fallback_warns_once(monkeypatch):
    monkeypatch.delenv("CUA_DRIVER_EMBEDDED", raising=False)
    m = load()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        for _ in range(2):
            result, embedded = run(m, Fake("d", up_after=10**6), app_installed=lambda: False)
            assert result is embedded
    messages = [str(w.message) for w in caught if "agent cursor" in str(w.message)]
    assert len(messages) == 1


def test_embedded_forced_or_non_macos(monkeypatch):
    monkeypatch.delenv("CUA_DRIVER_EMBEDDED", raising=False)
    m = load()
    daemon = Fake("daemon")
    for kwargs in (
        {"embedded": True},
        {"platform": "linux"},
        {"platform": "win32"},
        {"claude_code_compatibility": True},
    ):
        result, embedded = run(m, daemon, **kwargs)
        assert result is embedded
    monkeypatch.setenv("CUA_DRIVER_EMBEDDED", "1")
    result, embedded = run(m, daemon)
    assert result is embedded
    assert daemon.calls == 0
