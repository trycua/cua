"""file_exists / directory_exists on a sandbox session when the sandbox does not answer.

A task's ``evaluate`` commonly scores "answer file missing" as 0 (the starter
template and example_tasks/hello_file_env do). If a sandbox that stopped
answering also reads as "missing", a frozen or crashed sandbox records a
reward of 0 for an episode nobody scored. An unreachable sandbox has to raise.
"""

from __future__ import annotations

import pytest
from cua_bench.computers.remote import RemoteDesktopSession

from .fakes import FakeSandbox


class _FailingFiles:
    """The files API of a sandbox whose every call fails with ``error``."""

    def __init__(self, error: BaseException) -> None:
        self.error = error

    async def exists(self, path: str) -> bool:
        raise self.error

    async def is_dir(self, path: str) -> bool:
        raise self.error


def _session(files=None) -> RemoteDesktopSession:
    sb = FakeSandbox()
    if files is not None:
        sb.files = files
    return RemoteDesktopSession.attach(sb, os_type="linux")


def _spacesd_not_available():
    from cua_sandbox import SpacesdNotAvailable

    return SpacesdNotAvailable("no cua-spacesd answered")


@pytest.mark.parametrize(
    "error",
    [
        TimeoutError("operation timed out"),
        ConnectionResetError("connection reset by peer"),
        _spacesd_not_available(),
    ],
    ids=["timeout", "connection", "spacesd-not-available"],
)
async def test_an_unreachable_sandbox_raises_instead_of_reading_as_missing(error):
    session = _session(_FailingFiles(error))
    with pytest.raises(type(error)):
        await session.file_exists("/tmp/cb_answer.txt")
    with pytest.raises(type(error)):
        await session.directory_exists("/tmp")


async def test_the_native_transport_error_raises_and_not_found_reads_as_missing():
    try:
        from cua import CuaError
    except Exception:  # noqa: BLE001 - the native SDK library is not built here
        pytest.skip("native cua SDK not available")

    frozen = CuaError.Transport("transport error: http2 error (operation timed out)")
    with pytest.raises(type(frozen)):
        await _session(_FailingFiles(frozen)).file_exists("/tmp/cb_answer.txt")
    with pytest.raises(CuaError.Timeout):
        await _session(_FailingFiles(CuaError.Timeout("deadline exceeded"))).directory_exists("/t")
    missing = _session(_FailingFiles(CuaError.NotFound("no such file")))
    assert await missing.file_exists("/tmp/cb_answer.txt") is False


async def test_present_and_missing_paths_answer_as_before():
    session = _session()
    await session.write_file("/tmp/cb_answer.txt", "hello")
    assert await session.file_exists("/tmp/cb_answer.txt") is True
    assert await session.file_exists("/tmp/missing.txt") is False
    assert await session.file_exists("/tmp") is False
    assert await session.directory_exists("/tmp") is True
    assert await session.directory_exists("/tmp/missing") is False


async def test_other_errors_still_read_as_missing():
    session = _session(_FailingFiles(PermissionError("permission denied")))
    assert await session.file_exists("/root/secret.txt") is False
    assert await session.directory_exists("/root") is False
