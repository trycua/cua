#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///

import json
import os
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import traceback
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


@dataclass(frozen=True)
class ShutdownTimeouts:
    client_stop: float = 10.0
    graceful_wait: float = 10.0
    sigterm_wait: float = 10.0
    sigkill_wait: float = 5.0
    poll_interval: float = 0.25


@dataclass(frozen=True)
class ShutdownResult:
    returncode: int | None
    escalation: str
    forced_cleanup: bool
    client_stop_status: int
    stop_attempted: bool


class RunnerFailure(Exception):
    def __init__(self, message: str, status: int = 1):
        super().__init__(message)
        self.status = status


class RunnerSignal(Exception):
    def __init__(self, signum: int):
        super().__init__(f"received signal {signum}")
        self.status = 128 + signum


def normalized_status(returncode: int) -> int:
    return returncode if returncode >= 0 else 128 - returncode


def resolve_exit_status(
    primary_status: int,
    server_status: int | None,
    forced_cleanup: bool,
) -> int:
    """Prefer primary failures, then server failures, then forced cleanup."""
    if primary_status != 0:
        return primary_status
    if server_status is not None and server_status > 0:
        return server_status
    if forced_cleanup:
        return 1
    if server_status is not None and server_status < 0:
        return normalized_status(server_status)
    return 0


def wait_for_exit(
    process: subprocess.Popen[bytes],
    timeout: float,
    poll_interval: float,
) -> int | None:
    deadline = time.monotonic() + timeout
    while True:
        returncode = process.poll()
        if returncode is not None:
            return returncode
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        time.sleep(min(poll_interval, remaining))


def signal_process_group(process: subprocess.Popen[bytes], signum: int) -> None:
    if process.poll() is not None:
        return
    try:
        if os.getpgid(process.pid) == process.pid:
            os.killpg(process.pid, signum)
        else:
            process.send_signal(signum)
    except ProcessLookupError:
        pass


def run_stop_command(
    command: Sequence[str],
    client_log: Path,
    timeout: float,
    env: dict[str, str] | None,
) -> int:
    client_log.parent.mkdir(parents=True, exist_ok=True)
    with client_log.open("ab") as log:
        try:
            completed = subprocess.run(
                command,
                stdout=log,
                stderr=subprocess.STDOUT,
                timeout=timeout,
                env=env,
                check=False,
            )
        except subprocess.TimeoutExpired:
            log.write(f"client stop timed out after {timeout:g} seconds\n".encode())
            return 124
    return completed.returncode


def shutdown_server(
    process: subprocess.Popen[bytes],
    stop_command: Sequence[str],
    client_log: Path,
    timeouts: ShutdownTimeouts = ShutdownTimeouts(),
    env: dict[str, str] | None = None,
) -> ShutdownResult:
    returncode = process.poll()
    if returncode is not None:
        return ShutdownResult(returncode, "none", False, 0, False)

    stop_status = run_stop_command(stop_command, client_log, timeouts.client_stop, env)
    returncode = wait_for_exit(process, timeouts.graceful_wait, timeouts.poll_interval)
    if returncode is not None:
        return ShutdownResult(returncode, "none", stop_status != 0, stop_status, True)

    signal_process_group(process, signal.SIGTERM)
    returncode = wait_for_exit(process, timeouts.sigterm_wait, timeouts.poll_interval)
    if returncode is not None:
        return ShutdownResult(returncode, "sigterm", True, stop_status, True)

    signal_process_group(process, signal.SIGKILL)
    returncode = wait_for_exit(process, timeouts.sigkill_wait, timeouts.poll_interval)
    return ShutdownResult(returncode, "sigkill", True, stop_status, True)


def write_command(log: Path, command: Sequence[str]) -> None:
    with log.open("a", encoding="utf-8") as output:
        output.write(shlex.join(command))
        output.write("\n")


def run_client(
    command: Sequence[str],
    *,
    env: dict[str, str],
    timeout: float,
    stdout: Path | None = None,
    stderr: Path | None = None,
    stdin: Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[bytes]:
    stdout_handle = stdout.open("wb") if stdout else None
    stderr_handle = stderr.open("ab") if stderr else None
    stdin_handle = stdin.open("rb") if stdin else None
    try:
        return subprocess.run(
            command,
            stdin=stdin_handle,
            stdout=stdout_handle,
            stderr=stderr_handle,
            timeout=timeout,
            env=env,
            check=check,
        )
    finally:
        for handle in (stdout_handle, stderr_handle, stdin_handle):
            if handle is not None:
                handle.close()


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def validate_results(artifact_dir: Path) -> None:
    before = load_json(artifact_dir / "config-before.json")
    after = load_json(artifact_dir / "config-after.json")
    permissions = load_json(artifact_dir / "permissions.json")
    sessions_before = load_json(artifact_dir / "sessions-before.json")
    sessions_after = load_json(artifact_dir / "sessions-after.json")

    assert before["platform"] == "linux"
    assert after["max_image_dimension"] == 640
    assert after["max_image_dimension"] != before["max_image_dimension"]
    assert permissions["x11"] is True
    assert sessions_before == {"count": 0, "sessions": []}
    assert sessions_after == {"count": 0, "sessions": []}

    responses = [
        json.loads(line)
        for line in (artifact_dir / "mcp-responses.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    by_id = {response.get("id"): response for response in responses}
    assert by_id[1]["result"]["serverInfo"]["name"] == "cua-driver"
    tools = by_id[2]["result"]["tools"]
    tool_names = {tool["name"] for tool in tools}
    required = {
        "get_config",
        "set_config",
        "list_apps",
        "list_windows",
        "get_window_state",
        "click",
        "type_text",
        "press_key",
    }
    assert required <= tool_names, f"missing tools: {sorted(required - tool_names)}"
    assert by_id[3]["result"].get("isError", False) is not True
    assert by_id[3]["result"]["structuredContent"]["max_image_dimension"] == 640
    assert by_id[4]["error"] == {
        "code": -32601,
        "message": "Unknown method: compatibility/unknown",
    }


def emit_server_logs(artifact_dir: Path) -> None:
    for path in (artifact_dir / "server.stderr", artifact_dir / "valgrind.log"):
        if path.is_file():
            sys.stdout.write(path.read_text(encoding="utf-8", errors="replace"))


def verify_shutdown_logs(artifact_dir: Path, memcheck_disabled: bool) -> None:
    server_stderr = (artifact_dir / "server.stderr").read_text(
        encoding="utf-8", errors="replace"
    )
    assert "Cua Driver daemon shutting down." in server_stderr
    if not memcheck_disabled:
        valgrind_log = (artifact_dir / "valgrind.log").read_text(
            encoding="utf-8", errors="replace"
        )
        assert "definitely lost: 0 bytes in 0 blocks" in valgrind_log
        assert "possibly lost: 0 bytes in 0 blocks" in valgrind_log
        assert "ERROR SUMMARY: 0 errors from 0 contexts" in valgrind_log


def run_e2e(driver: Path, artifact_dir: Path, smoke_dir: Path, env: dict[str, str]):
    socket_path = smoke_dir / "cua-driver.sock"
    client_log = artifact_dir / "client.log"
    commands_log = artifact_dir / "commands.log"
    memcheck_disabled = env.get("CUA_DRIVER_MEMCHECK_DISABLE", "0") == "1"

    server_command = [
        "valgrind",
        "--leak-check=full",
        "--gen-suppressions=all",
        "--num-callers=40",
        "--show-leak-kinds=definite,possible",
        "--errors-for-leak-kinds=definite,possible",
        "--error-exitcode=99",
        f"--log-file={artifact_dir / 'valgrind.log'}",
        str(driver),
        "serve",
        "--socket",
        str(socket_path),
        "--no-overlay",
        "--dangerously-bypass-approvals",
    ]
    if memcheck_disabled:
        server_command = [
            str(driver),
            "serve",
            "--socket",
            str(socket_path),
            "--no-overlay",
            "--dangerously-bypass-approvals",
        ]

    commands_log.write_text(f"server command: {shlex.join(server_command)}\n", encoding="utf-8")
    server_stdout = (artifact_dir / "server.stdout").open("wb")
    server_stderr = (artifact_dir / "server.stderr").open("wb")
    try:
        process = subprocess.Popen(
            server_command,
            stdout=server_stdout,
            stderr=server_stderr,
            env=env,
            start_new_session=True,
        )
    finally:
        server_stdout.close()
        server_stderr.close()

    shutdown_result = None
    primary_status = 0
    stop_command = [str(driver), "stop", "--socket", str(socket_path)]
    previous_handlers = {}

    def handle_signal(signum, _frame):
        raise RunnerSignal(signum)

    for signum in (signal.SIGINT, signal.SIGTERM):
        previous_handlers[signum] = signal.signal(signum, handle_signal)

    try:
        ready = False
        for _ in range(120):
            if process.poll() is not None:
                raise RunnerFailure("cua-driver server exited before readiness")
            try:
                completed = run_client(
                    [str(driver), "status", "--socket", str(socket_path)],
                    env=env,
                    timeout=3,
                    stdout=artifact_dir / "status.txt",
                    stderr=client_log,
                    check=False,
                )
            except subprocess.TimeoutExpired:
                completed = None
            if completed is not None and completed.returncode == 0:
                ready = True
                break
            time.sleep(0.5)
        if not ready:
            raise RunnerFailure("cua-driver server did not become ready within 60 seconds")

        assert "running" in (artifact_dir / "status.txt").read_text(
            encoding="utf-8", errors="replace"
        ).lower()
        recorded_commands = [
            [str(driver), "status", "--socket", str(socket_path)],
            [str(driver), "sessions", "list", "--json", "--socket", str(socket_path)],
            [str(driver), "call", "get_config", "{}", "--socket", str(socket_path)],
            [
                str(driver),
                "call",
                "set_config",
                '{"key":"max_image_dimension","value":640}',
                "--socket",
                str(socket_path),
            ],
            [str(driver), "call", "check_permissions", "{}", "--socket", str(socket_path)],
            [str(driver), "mcp", "--socket", str(socket_path)],
            stop_command,
        ]
        for command in recorded_commands:
            write_command(commands_log, command)

        run_client(
            [str(driver), "sessions", "list", "--json", "--socket", str(socket_path)],
            env=env,
            timeout=10,
            stdout=artifact_dir / "sessions-before.json",
        )
        run_client(
            [str(driver), "call", "get_config", "{}", "--socket", str(socket_path)],
            env=env,
            timeout=10,
            stdout=artifact_dir / "config-before.json",
        )
        run_client(
            [
                str(driver),
                "call",
                "set_config",
                '{"key":"max_image_dimension","value":640}',
                "--socket",
                str(socket_path),
            ],
            env=env,
            timeout=10,
            stdout=artifact_dir / "config-set.json",
        )
        run_client(
            [str(driver), "call", "get_config", "{}", "--socket", str(socket_path)],
            env=env,
            timeout=10,
            stdout=artifact_dir / "config-after.json",
        )
        run_client(
            [str(driver), "call", "check_permissions", "{}", "--socket", str(socket_path)],
            env=env,
            timeout=10,
            stdout=artifact_dir / "permissions.json",
        )

        requests = smoke_dir / "mcp-requests.jsonl"
        requests.write_text(
            "\n".join(
                [
                    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"valgrind-e2e","version":"1.0.0"}}}',
                    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
                    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}',
                    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_config","arguments":{}}}',
                    '{"jsonrpc":"2.0","id":4,"method":"compatibility/unknown","params":{}}',
                ]
            )
            + "\n",
            encoding="utf-8",
        )
        run_client(
            [str(driver), "mcp", "--socket", str(socket_path)],
            env=env,
            timeout=30,
            stdin=requests,
            stdout=artifact_dir / "mcp-responses.jsonl",
            stderr=client_log,
        )

        for _ in range(40):
            run_client(
                [str(driver), "sessions", "list", "--json", "--socket", str(socket_path)],
                env=env,
                timeout=10,
                stdout=artifact_dir / "sessions-after.json",
            )
            if load_json(artifact_dir / "sessions-after.json").get("count") == 0:
                break
            time.sleep(0.25)

        validate_results(artifact_dir)
        shutdown_result = shutdown_server(process, stop_command, client_log, env=env)
        emit_server_logs(artifact_dir)
        if not shutdown_result.stop_attempted:
            raise RunnerFailure(
                "cua-driver server exited before the planned stop with status "
                f"{shutdown_result.returncode}"
            )
        if shutdown_result.client_stop_status != 0:
            raise RunnerFailure(
                "cua-driver stop failed with status "
                f"{shutdown_result.client_stop_status}",
                normalized_status(shutdown_result.client_stop_status),
            )
        if shutdown_result.returncode not in (0, None):
            print(
                "Valgrind-wrapped cua-driver server exited with status "
                f"{shutdown_result.returncode}",
                file=sys.stderr,
            )
        elif shutdown_result.returncode is None:
            print("cua-driver server did not exit after SIGKILL timeout", file=sys.stderr)
        else:
            verify_shutdown_logs(artifact_dir, memcheck_disabled)
    except RunnerSignal as error:
        primary_status = error.status
        print(str(error), file=sys.stderr)
    except RunnerFailure as error:
        primary_status = error.status
        print(str(error), file=sys.stderr)
    except subprocess.CalledProcessError as error:
        primary_status = normalized_status(error.returncode)
        print(
            f"command failed with status {error.returncode}: {shlex.join(error.cmd)}",
            file=sys.stderr,
        )
    except subprocess.TimeoutExpired as error:
        primary_status = 124
        print(
            f"command timed out after {error.timeout} seconds: {shlex.join(error.cmd)}",
            file=sys.stderr,
        )
    except BaseException:
        primary_status = 1
        traceback.print_exc()
    finally:
        for signum in (signal.SIGINT, signal.SIGTERM):
            signal.signal(signum, signal.SIG_IGN)
        if shutdown_result is None:
            shutdown_result = shutdown_server(process, stop_command, client_log, env=env)
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)

    return resolve_exit_status(
        primary_status,
        shutdown_result.returncode,
        shutdown_result.forced_cleanup,
    )


def main(argv: Sequence[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if len(args) != 1:
        print("usage: run-valgrind-e2e.py /path/to/cua-driver", file=sys.stderr)
        return 2

    try:
        driver = Path(args[0]).resolve(strict=True)
    except FileNotFoundError:
        print(f"cua-driver not found: {args[0]}", file=sys.stderr)
        return 2

    runner_temp = Path(os.environ.get("RUNNER_TEMP", "/tmp"))
    artifact_dir = Path(
        os.environ.get("CUA_VALGRIND_ARTIFACT_DIR", runner_temp / "cua-driver-valgrind")
    )
    shutil.rmtree(artifact_dir, ignore_errors=True)
    artifact_dir.mkdir(parents=True)

    smoke_dir = Path(tempfile.mkdtemp())
    env = os.environ.copy()
    env.update(
        {
            "HOME": str(smoke_dir / "home"),
            "XDG_CACHE_HOME": str(smoke_dir / "cache"),
            "XDG_CONFIG_HOME": str(smoke_dir / "config"),
            "XDG_DATA_HOME": str(smoke_dir / "data"),
            "XDG_RUNTIME_DIR": str(smoke_dir / "runtime"),
        }
    )
    for variable in (
        "HOME",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_RUNTIME_DIR",
    ):
        Path(env[variable]).mkdir(parents=True)
    Path(env["XDG_RUNTIME_DIR"]).chmod(0o700)

    try:
        return run_e2e(driver, artifact_dir, smoke_dir, env)
    finally:
        try:
            shutil.copytree(smoke_dir, artifact_dir / "smoke-dir", dirs_exist_ok=True)
        except OSError as error:
            print(f"failed to copy smoke directory: {error}", file=sys.stderr)
        shutil.rmtree(smoke_dir, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
