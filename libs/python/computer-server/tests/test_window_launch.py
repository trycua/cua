from unittest.mock import Mock, patch

import pytest

try:
    import computer_server  # noqa: F401
    from computer_server.handlers.generic import GenericWindowHandler, build_launch_argv
except Exception as import_error:  # pragma: no cover - environment-dependent
    pytest.skip(
        f"computer_server.handlers.generic unavailable in this environment: {import_error}",
        allow_module_level=True,
    )


def test_build_launch_argv_splits_compatible_command_string():
    assert build_launch_argv("libreoffice --writer") == ["libreoffice", "--writer"]


def test_build_launch_argv_keeps_shell_metacharacters_literal():
    assert build_launch_argv("echo hello; touch /tmp/pwned") == [
        "echo",
        "hello;",
        "touch",
        "/tmp/pwned",
    ]


def test_build_launch_argv_uses_explicit_args_literally():
    assert build_launch_argv("echo", ["hello; touch /tmp/pwned"]) == [
        "echo",
        "hello; touch /tmp/pwned",
    ]


def test_build_launch_argv_preserves_windows_paths(monkeypatch):
    monkeypatch.setattr(build_launch_argv.__globals__["os"], "name", "nt")

    assert build_launch_argv('"C:\\Program Files\\App\\app.exe" --flag') == [
        "C:\\Program Files\\App\\app.exe",
        "--flag",
    ]
    assert build_launch_argv("C:\\Tools\\app.exe --flag") == [
        "C:\\Tools\\app.exe",
        "--flag",
    ]


@pytest.mark.asyncio
async def test_launch_uses_argv_without_shell():
    proc = Mock(pid=1234)
    with patch("computer_server.handlers.generic.subprocess.Popen", return_value=proc) as popen:
        result = await GenericWindowHandler().launch("echo hello; touch /tmp/pwned")

    assert result == {"success": True, "pid": 1234}
    popen.assert_called_once_with(["echo", "hello;", "touch", "/tmp/pwned"])


@pytest.mark.asyncio
async def test_launch_rejects_empty_app():
    result = await GenericWindowHandler().launch("")

    assert result["success"] is False
    assert "app must not be empty" in result["error"]
