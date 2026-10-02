"""bench_ui.child._client_origin: the toolkit's client-area origin, or None."""

import sys
import types

import pytest


@pytest.fixture
def child(monkeypatch):
    # bench_ui.child imports pywebview at module level; a stub is enough here.
    stub = types.ModuleType("webview")
    stub.Window = object
    monkeypatch.setitem(sys.modules, "webview", stub)
    monkeypatch.delitem(sys.modules, "bench_ui.child", raising=False)
    from bench_ui import child as mod

    return mod


def test_none_off_linux(child, monkeypatch):
    monkeypatch.setattr(child.sys, "platform", "darwin")
    assert child._client_origin(types.SimpleNamespace(native=object())) is None


def test_none_without_a_native_widget(child, monkeypatch):
    monkeypatch.setattr(child.sys, "platform", "linux")
    gi = types.ModuleType("gi")
    repo = types.ModuleType("gi.repository")
    repo.GLib = types.SimpleNamespace(idle_add=lambda f: f())
    monkeypatch.setitem(sys.modules, "gi", gi)
    monkeypatch.setitem(sys.modules, "gi.repository", repo)
    assert child._client_origin(types.SimpleNamespace(native=None)) is None


def test_origin_plus_widget_allocation(child, monkeypatch):
    monkeypatch.setattr(child.sys, "platform", "linux")
    gi = types.ModuleType("gi")
    repo = types.ModuleType("gi.repository")
    repo.GLib = types.SimpleNamespace(idle_add=lambda f: f())
    monkeypatch.setitem(sys.modules, "gi", gi)
    monkeypatch.setitem(sys.modules, "gi.repository", repo)
    gdk_window = types.SimpleNamespace(get_origin=lambda: (True, 485, 309))
    webview = types.SimpleNamespace(
        get_window=lambda: gdk_window, get_allocation=lambda: types.SimpleNamespace(x=0, y=0)
    )
    native = types.SimpleNamespace(webview=webview)
    assert child._client_origin(types.SimpleNamespace(native=native)) == (485, 309)
