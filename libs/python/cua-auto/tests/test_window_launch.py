from unittest.mock import Mock, patch

import cua_auto.window as window
import pytest
from cua_auto.window import _build_launch_argv, launch


def test_build_launch_argv_splits_compatible_command_string():
    assert _build_launch_argv("libreoffice --writer") == ["libreoffice", "--writer"]


def test_build_launch_argv_keeps_shell_metacharacters_literal():
    assert _build_launch_argv("echo hello; touch /tmp/pwned") == [
        "echo",
        "hello;",
        "touch",
        "/tmp/pwned",
    ]


def test_build_launch_argv_uses_explicit_args_literally():
    assert _build_launch_argv("echo", ["hello; touch /tmp/pwned"]) == [
        "echo",
        "hello; touch /tmp/pwned",
    ]


def test_build_launch_argv_preserves_windows_paths(monkeypatch):
    monkeypatch.setattr(window.os, "name", "nt")

    assert _build_launch_argv('"C:\\Program Files\\App\\app.exe" --flag') == [
        "C:\\Program Files\\App\\app.exe",
        "--flag",
    ]
    assert _build_launch_argv("C:\\Tools\\app.exe --flag") == [
        "C:\\Tools\\app.exe",
        "--flag",
    ]


def test_launch_uses_argv_without_shell():
    proc = Mock(pid=1234)
    with patch("cua_auto.window.subprocess.Popen", return_value=proc) as popen:
        assert launch("echo hello; touch /tmp/pwned") == 1234

    popen.assert_called_once_with(["echo", "hello;", "touch", "/tmp/pwned"])


def test_launch_rejects_empty_app():
    with pytest.raises(ValueError, match="app must not be empty"):
        launch("")
