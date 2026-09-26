"""Safe, supervised subprocess execution."""

from __future__ import annotations

import os
import signal
import stat
import subprocess
import sys
import time
from collections.abc import Mapping, Sequence
from contextlib import ExitStack
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

from cua_bench_runtime.errors import DeadlineExceeded, ValidationFailure
from cua_bench_runtime.signals import InterruptFlag

ENV_ALLOWLIST = (
    "PATH",
    "HOME",
    "USERPROFILE",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
)
DEFAULT_STDIN_LIMIT = 1_048_576


@dataclass(frozen=True)
class ProcessResult:
    returncode: int
    duration_ms: int
    stdout: Path
    stderr: Path
    truncated: bool
    output_exceeded: bool


def clean_environment(extra: Mapping[str, str] | None = None) -> dict[str, str]:
    environment = {key: os.environ[key] for key in ENV_ALLOWLIST if key in os.environ}
    environment.update(extra or {})
    return environment


def command_for(path: Path, extra: Sequence[str]) -> list[str]:
    if path.suffix.lower() == ".py":
        return [sys.executable, str(path), *extra]
    return [str(path), *extra]


def _open_regular_input(path: Path) -> BinaryIO:
    try:
        before = path.lstat()
    except OSError as error:
        raise ValidationFailure("cannot open process stdin") from error
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        raise ValidationFailure("process stdin must be a regular file")

    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise ValidationFailure("cannot open process stdin") from error
    handle: BinaryIO | None = None
    try:
        handle = os.fdopen(descriptor, "rb")
        opened = os.fstat(handle.fileno())
        after = path.lstat()
        if (
            not stat.S_ISREG(opened.st_mode)
            or stat.S_ISLNK(after.st_mode)
            or (opened.st_dev, opened.st_ino) != (after.st_dev, after.st_ino)
        ):
            raise ValidationFailure("process stdin identity changed while opening")
        return handle
    except BaseException:
        if handle is not None:
            handle.close()
        else:
            os.close(descriptor)
        raise


def _terminate_group(process: subprocess.Popen[bytes], grace_seconds: float = 2.0) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":
        try:
            process.send_signal(signal.CTRL_BREAK_EVENT)
        except (OSError, ValueError):
            process.terminate()
    else:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
    try:
        process.wait(timeout=grace_seconds)
        return
    except subprocess.TimeoutExpired:
        pass
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        pass


def _cap_file(path: Path, limit: int) -> bool:
    size = path.stat().st_size
    if size <= limit:
        return False
    with path.open("r+b") as handle:
        handle.truncate(limit)
    path.with_suffix(path.suffix + ".truncated").write_text(
        f"captured output exceeded {limit} bytes\n", encoding="utf-8"
    )
    return True


def run_process(
    argv: Sequence[str],
    *,
    cwd: Path,
    stdout_path: Path,
    stderr_path: Path,
    timeout_seconds: float,
    interrupt: InterruptFlag,
    extra_env: Mapping[str, str] | None = None,
    output_limit: int = 1_048_576,
    stdin_path: Path | None = None,
    stdin_limit: int = DEFAULT_STDIN_LIMIT,
) -> ProcessResult:
    if stdin_limit <= 0:
        raise ValueError("stdin_limit must be positive")
    started = time.monotonic()
    creationflags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    output_exceeded = False
    with ExitStack() as stack:
        stdin: BinaryIO | int = subprocess.DEVNULL
        if stdin_path is not None:
            stdin_file = stack.enter_context(_open_regular_input(Path(stdin_path)))
            stdin_stat = os.fstat(stdin_file.fileno())
            if not stat.S_ISREG(stdin_stat.st_mode):
                raise ValidationFailure("process stdin must be a regular file")
            stdin_size = stdin_stat.st_size
            if stdin_size > stdin_limit:
                raise ValidationFailure(f"process stdin exceeded {stdin_limit} bytes")
            stdin = stdin_file
        stdout = stack.enter_context(stdout_path.open("xb"))
        stderr = stack.enter_context(stderr_path.open("xb"))
        process = subprocess.Popen(
            list(argv),
            cwd=cwd,
            env=clean_environment(extra_env),
            shell=False,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            start_new_session=os.name != "nt",
            creationflags=creationflags,
        )
        while process.poll() is None:
            if interrupt.count:
                _terminate_group(process)
                interrupt.raise_if_requested()
            if time.monotonic() - started >= timeout_seconds:
                _terminate_group(process)
                raise DeadlineExceeded(f"process exceeded {timeout_seconds:g} seconds")
            if (
                stdout_path.stat().st_size > output_limit
                or stderr_path.stat().st_size > output_limit
            ):
                _terminate_group(process)
                output_exceeded = True
                break
            time.sleep(0.05)
        returncode = int(process.returncode)

    truncated = _cap_file(stdout_path, output_limit)
    truncated = _cap_file(stderr_path, output_limit) or truncated
    return ProcessResult(
        returncode=returncode,
        duration_ms=int((time.monotonic() - started) * 1000),
        stdout=stdout_path,
        stderr=stderr_path,
        truncated=truncated,
        output_exceeded=output_exceeded,
    )
