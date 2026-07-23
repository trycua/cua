"""Tests for the cua-driver Python wrapper."""

import importlib.util
import io
import os
import subprocess
import sys
from pathlib import Path
from typing import Any
from unittest.mock import Mock, patch

import pytest


def load_wrapper_module() -> Any:
    module_path = Path(__file__).resolve().parents[1] / "src/cua_driver/wrapper.py"
    spec = importlib.util.spec_from_file_location("cua_driver_wrapper", module_path)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_get_binary_path():
    """Test that get_binary_path returns a valid path."""
    from cua_driver.wrapper import get_binary_path

    # This will raise FileNotFoundError if binary doesn't exist
    # In CI, we need to build the package first for this to pass
    try:
        binary_path = get_binary_path()
        assert binary_path.exists()
        assert binary_path.name in ("cua-driver", "cua-driver.exe")
    except FileNotFoundError:
        # Expected in development without building
        pytest.skip("Binary not bundled yet (run build_wheel.py first)")


def test_run_cua_driver_version(monkeypatch):
    """Test running cua-driver --version through the wrapper."""
    from cua_driver.wrapper import get_binary_path, run_cua_driver

    try:
        binary_path = get_binary_path()
    except FileNotFoundError:
        pytest.skip("Binary not bundled yet")

    # Run with --version
    exit_code = run_cua_driver(["--version"])
    assert exit_code == 0


def test_wrapper_preserves_exit_code():
    """Test that the wrapper preserves the binary's exit code."""
    from cua_driver.wrapper import get_binary_path, run_cua_driver

    try:
        binary_path = get_binary_path()
    except FileNotFoundError:
        pytest.skip("Binary not bundled yet")

    # Invalid command should return non-zero
    exit_code = run_cua_driver(["--this-flag-does-not-exist"])
    assert exit_code != 0


def test_subprocess_args(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Test that subprocess is called with correct arguments."""
    wrapper = load_wrapper_module()
    mock_binary = Path("/fake/path/cua-driver")
    mock_run = Mock(return_value=Mock(returncode=0))
    monkeypatch.setattr(wrapper, "get_binary_path", Mock(return_value=mock_binary))
    monkeypatch.setattr(wrapper.subprocess, "run", mock_run)

    stdin_path = tmp_path / "stdin"
    stdin_path.write_text("")
    with (
        stdin_path.open() as stdin,
        (tmp_path / "stdout").open("w") as stdout,
        (tmp_path / "stderr").open("w") as stderr,
    ):
        monkeypatch.setattr(sys, "stdin", stdin)
        monkeypatch.setattr(sys, "stdout", stdout)
        monkeypatch.setattr(sys, "stderr", stderr)

        wrapper.run_cua_driver(["mcp", "--help"])

        mock_run.assert_called_once()
        call_args = mock_run.call_args
        assert call_args[0][0] == [str(mock_binary), "mcp", "--help"]
        assert call_args[1]["stdin"] is stdin
        assert call_args[1]["stdout"] is stdout
        assert call_args[1]["stderr"] is stderr
        assert call_args[1]["env"]["CUA_DRIVER_INSTALL_CHANNEL"] == "python_package"


@pytest.mark.parametrize("stream_name", ["stdin", "stdout", "stderr"])
def test_subprocess_falls_back_for_fileno_less_stdio(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, stream_name: str
) -> None:
    """A captured stream falls back without changing usable descriptors."""
    wrapper = load_wrapper_module()
    monkeypatch.setattr(wrapper, "get_binary_path", Mock(return_value=Path(sys.executable)))

    stdin_path = tmp_path / "stdin"
    stdin_path.write_text("")
    with (
        stdin_path.open() as stdin,
        (tmp_path / "stdout").open("w") as stdout,
        (tmp_path / "stderr").open("w") as stderr,
    ):
        streams = {"stdin": stdin, "stdout": stdout, "stderr": stderr}
        streams[stream_name] = io.StringIO()
        for name, stream in streams.items():
            monkeypatch.setattr(sys, name, stream)

        exit_code = wrapper.run_cua_driver(["-c", "pass"])

        assert exit_code == 0


@pytest.mark.parametrize("descriptor", [None, -1, "1"])
def test_stdio_falls_back_for_invalid_fileno(descriptor: object) -> None:
    """A non-descriptor fileno result is not forwarded to subprocess."""
    wrapper = load_wrapper_module()
    stream = Mock()
    stream.fileno.return_value = descriptor

    assert wrapper._stdio_with_fileno(stream) is None


def test_subprocess_preserves_exact_exit_code(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The stdio fallback does not change the child's exit-code contract."""
    wrapper = load_wrapper_module()
    monkeypatch.setattr(
        wrapper, "get_binary_path", Mock(return_value=Path("/fake/path/cua-driver"))
    )
    monkeypatch.setattr(wrapper.subprocess, "run", Mock(return_value=Mock(returncode=42)))

    assert wrapper.run_cua_driver(["--version"]) == 42


@patch("cua_driver.wrapper.subprocess.run")
@patch("cua_driver.wrapper.get_binary_path")
def test_subprocess_preserves_explicit_install_channel(mock_get_binary, mock_run, monkeypatch):
    """An update/install caller's bounded channel takes precedence."""
    from cua_driver.wrapper import run_cua_driver

    mock_get_binary.return_value = Path("/fake/path/cua-driver")
    mock_run.return_value = Mock(returncode=0)
    monkeypatch.setenv("CUA_DRIVER_INSTALL_CHANNEL", "update_apply")

    run_cua_driver(["--version"])

    child_env = mock_run.call_args.kwargs["env"]
    assert child_env["CUA_DRIVER_INSTALL_CHANNEL"] == "update_apply"
    assert os.environ["CUA_DRIVER_INSTALL_CHANNEL"] == "update_apply"


@patch("cua_driver.wrapper.subprocess.run")
@patch("cua_driver.wrapper.get_binary_path")
def test_keyboard_interrupt_handling(mock_get_binary, mock_run):
    """Test that KeyboardInterrupt returns exit code 130."""
    from cua_driver.wrapper import run_cua_driver

    mock_binary = Path("/fake/path/cua-driver")
    mock_get_binary.return_value = mock_binary
    mock_run.side_effect = KeyboardInterrupt()

    exit_code = run_cua_driver(["mcp"])
    assert exit_code == 130
