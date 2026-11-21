"""ADB adapter contracts using real public interfaces and an inert subprocess seam."""

from __future__ import annotations

import subprocess
import threading

import pytest
from cua_sandbox import Sandbox
from cua_sandbox.transport.adb import ADBTransport


@pytest.fixture
def recorded_adb(monkeypatch):
    transport = ADBTransport(serial="recorded-android", sdk_root="/recorded/android-sdk")
    transport._adb = "/recorded/android-sdk/platform-tools/adb"
    calls = []
    result = subprocess.CompletedProcess([], 0, stdout=b"out\n", stderr=b"warn\n")
    error = None

    def run(args, *, capture_output, env, timeout):
        assert capture_output is True
        assert env["ANDROID_SDK_ROOT"] == "/recorded/android-sdk"
        calls.append((args, timeout, threading.get_ident()))
        if error is not None:
            raise error
        return result

    monkeypatch.setattr(subprocess, "run", run)
    sandbox = Sandbox(transport, name="recorded-android", _telemetry_enabled=False)

    def fail_with(exception):
        nonlocal error
        error = exception

    return sandbox, transport, calls, result, fail_with


@pytest.mark.parametrize("timeout", [0, 1, 10, 75])
async def test_public_shell_forwards_explicit_timeout(recorded_adb, timeout):
    sandbox, _, calls, _, _ = recorded_adb
    loop_thread = threading.get_ident()

    result = await sandbox.shell.run("echo hello", timeout=timeout)

    assert (result.stdout, result.stderr, result.returncode, result.success) == (
        "out\n",
        "warn\n",
        0,
        True,
    )
    assert len(calls) == 1
    args, received_timeout, worker_thread = calls[0]
    assert args == [
        "/recorded/android-sdk/platform-tools/adb",
        "-s",
        "recorded-android",
        "shell",
        "[ -f /data/local/tmp/.cua_env ] && . /data/local/tmp/.cua_env; echo hello",
    ]
    assert received_timeout == timeout
    assert worker_thread != loop_thread


async def test_public_shell_keeps_its_thirty_second_default(recorded_adb):
    sandbox, _, calls, _, _ = recorded_adb

    await sandbox.shell.run("echo hello")

    assert len(calls) == 1
    assert calls[0][1] == 30


@pytest.mark.parametrize("action", ["shell", "execute", "run_command"])
@pytest.mark.parametrize("options,expected_timeout", [({}, 15), ({"timeout": 45}, 45)])
async def test_direct_shell_aliases_keep_default_and_forward_overrides(
    recorded_adb, action, options, expected_timeout
):
    _, transport, calls, _, _ = recorded_adb

    result = await transport.send(action, command="echo hello", **options)

    assert result == {"stdout": "out\n", "stderr": "warn\n", "returncode": 0}
    assert len(calls) == 1
    assert calls[0][1] == expected_timeout


@pytest.mark.parametrize("options,expected_timeout", [({}, 30), ({"timeout": 4}, 4)])
async def test_public_shell_propagates_original_timeout_without_retry(
    recorded_adb, options, expected_timeout
):
    sandbox, _, calls, _, fail_with = recorded_adb
    error = subprocess.TimeoutExpired("recorded adb", expected_timeout, output=b"partial")
    fail_with(error)

    with pytest.raises(subprocess.TimeoutExpired) as caught:
        await sandbox.shell.run("echo hello", **options)

    assert caught.value is error
    assert caught.value.output == b"partial"
    assert len(calls) == 1
    assert calls[0][1] == expected_timeout


async def test_shell_preserves_nonzero_status_and_replacement_decoding(recorded_adb):
    sandbox, _, calls, result, _ = recorded_adb
    result.returncode = 7
    result.stdout = b"out\xff"
    result.stderr = b"err\xff"

    output = await sandbox.shell.run("echo hello", timeout=15)

    assert (output.stdout, output.stderr, output.returncode, output.success) == (
        "out\ufffd",
        "err\ufffd",
        7,
        False,
    )
    assert len(calls) == 1
    assert calls[0][1] == 15


async def test_screen_size_keeps_its_existing_adb_timeout(recorded_adb):
    _, transport, calls, result, _ = recorded_adb
    result.stdout = b"Physical size: 1080x1920\n"

    assert await transport.get_screen_size() == {"width": 1080, "height": 1920}

    assert len(calls) == 1
    assert calls[0][0][-3:] == ["shell", "wm", "size"]
    assert calls[0][1] == 15
