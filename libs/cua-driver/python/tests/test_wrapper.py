"""Tests for the cua-driver Python wrapper."""

import os
import subprocess
import sys
from pathlib import Path
from unittest.mock import Mock, patch

import pytest


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
    from cua_driver.wrapper import run_cua_driver, get_binary_path

    try:
        binary_path = get_binary_path()
    except FileNotFoundError:
        pytest.skip("Binary not bundled yet")

    # Run with --version
    exit_code = run_cua_driver(["--version"])
    assert exit_code == 0


def test_wrapper_preserves_exit_code():
    """Test that the wrapper preserves the binary's exit code."""
    from cua_driver.wrapper import run_cua_driver, get_binary_path

    try:
        binary_path = get_binary_path()
    except FileNotFoundError:
        pytest.skip("Binary not bundled yet")

    # Invalid command should return non-zero
    exit_code = run_cua_driver(["--this-flag-does-not-exist"])
    assert exit_code != 0


@patch("cua_driver.wrapper.subprocess.run")
@patch("cua_driver.wrapper.get_binary_path")
def test_subprocess_args(mock_get_binary, mock_run):
    """Test that subprocess is called with correct arguments."""
    from cua_driver.wrapper import run_cua_driver

    mock_binary = Path("/fake/path/cua-driver")
    mock_get_binary.return_value = mock_binary
    mock_run.return_value = Mock(returncode=0)

    run_cua_driver(["mcp", "--help"])

    mock_run.assert_called_once()
    call_args = mock_run.call_args
    assert call_args[0][0] == [str(mock_binary), "mcp", "--help"]
    assert call_args[1]["stdin"] == sys.stdin
    assert call_args[1]["stdout"] == sys.stdout
    assert call_args[1]["stderr"] == sys.stderr
    assert call_args[1]["env"]["CUA_DRIVER_INSTALL_CHANNEL"] == "python_package"


@patch("cua_driver.wrapper.subprocess.run")
@patch("cua_driver.wrapper.get_binary_path")
def test_subprocess_preserves_explicit_install_channel(
    mock_get_binary, mock_run, monkeypatch
):
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
