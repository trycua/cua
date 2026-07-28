#!/usr/bin/env python3
"""Build and validate the Daytona Windows snapshot used by cua-bench.

The base Windows VM must have an unlocked interactive desktop. Toolbox commands
run as SYSTEM, so this builder installs cua-computer-server system-wide and
registers an interactive logon task for the desktop user.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import json
import os
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any

from daytona_sdk import (
    CreateSandboxFromSnapshotParams,
    Daytona,
    DaytonaConfig,
    DaytonaNotFoundError,
)

DEFAULT_NAME = "cua-bench-windows-v1"
DEFAULT_BASE = "windows-medium"
SERVER_PORT = 8000
PYTHON_PATH = r"C:\Program Files\Python313\python.exe"
SERVER_DIR = r"C:\ProgramData\cua-bench"
LAUNCHER_PATH = SERVER_DIR + r"\start-computer-server.ps1"
SERVER_LOG_PATH = SERVER_DIR + r"\computer-server.log"
TASK_NAME = "Cua-Computer-Server"
PYTHON_INSTALLER_URL = (
    "https://www.python.org/ftp/python/3.13.9/python-3.13.9-amd64.exe"
)


@dataclass(frozen=True)
class Timeouts:
    create: float
    command: int
    health: float
    snapshot: float
    cleanup: float


@dataclass(frozen=True)
class ScreenshotEvidence:
    status_code: int
    encoded_size: int
    decoded_size: int


def parse_args() -> argparse.Namespace:
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", default=DEFAULT_NAME, help="snapshot name")
    parser.add_argument("--base", default=DEFAULT_BASE, help="base Windows snapshot")
    parser.add_argument(
        "--keep-sandbox",
        action="store_true",
        help="keep the builder sandbox after the run for debugging",
    )
    parser.add_argument("--create-timeout", type=float, default=600)
    parser.add_argument("--command-timeout", type=int, default=900)
    parser.add_argument("--health-timeout", type=float, default=600)
    parser.add_argument("--snapshot-timeout", type=float, default=1_200)
    parser.add_argument("--cleanup-timeout", type=float, default=300)
    return parser.parse_args()


def make_client() -> Daytona:
    """Create a Daytona client from the same environment variables as the harness."""
    api_key = os.environ.get("DAYTONA_API_KEY")
    if not api_key:
        raise RuntimeError("DAYTONA_API_KEY is required")
    api_url = os.environ.get("DAYTONA_API_URL")
    config = (
        DaytonaConfig(api_key=api_key, api_url=api_url)
        if api_url
        else DaytonaConfig(api_key=api_key)
    )
    return Daytona(config)


def powershell_command(script: str) -> str:
    """Encode a PowerShell command without relying on shell quoting."""
    script = "$ProgressPreference = 'SilentlyContinue'\n" + script
    encoded = base64.b64encode(script.encode("utf-16-le")).decode("ascii")
    return (
        "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass "
        f"-EncodedCommand {encoded}"
    )


def exec_checked(sandbox: Any, command: str, timeout: int, description: str) -> str:
    """Run one synchronous toolbox command and require a zero exit code."""
    response = sandbox.process.exec(command, timeout=timeout)
    output = response.result or ""
    if response.exit_code != 0:
        raise RuntimeError(
            f"{description} failed with exit code {response.exit_code}:\n{output}"
        )
    return output


def discover_interactive_user(sandbox: Any, timeout: int) -> tuple[str, str]:
    """Return the interactive username and the diagnostic query output."""
    query_output = exec_checked(
        sandbox,
        powershell_command("query user 2>&1; exit 0"),
        timeout,
        "interactive-user query",
    )
    for line in query_output.splitlines():
        columns = line.lstrip().lstrip(">").split()
        if columns and any(column.lower() == "active" for column in columns[1:]):
            return columns[0].split("\\")[-1], query_output

    fallback = exec_checked(
        sandbox,
        powershell_command(
            "$user = (Get-CimInstance Win32_ComputerSystem).UserName; "
            "if (-not $user) { throw 'No interactive desktop user is logged on' }; $user"
        ),
        timeout,
        "interactive-user fallback",
    ).strip()
    if not fallback:
        raise RuntimeError(f"No active interactive user found. query user output:\n{query_output}")
    return fallback.split("\\")[-1], query_output


def install_server(sandbox: Any, timeout: int) -> None:
    """Ensure system Python exists, then install and import-check the server."""
    exec_checked(
        sandbox,
        powershell_command(
            f"""
$ErrorActionPreference = 'Stop'
if (-not (Test-Path '{PYTHON_PATH}')) {{
    $installer = Join-Path $env:TEMP 'python-3.13.9-amd64.exe'
    try {{
        Invoke-WebRequest -UseBasicParsing -Uri '{PYTHON_INSTALLER_URL}' -OutFile $installer
        $arguments = '/quiet InstallAllUsers=1 PrependPath=1 Include_launcher=1 Include_pip=1 TargetDir="C:\\Program Files\\Python313"'
        $process = Start-Process -FilePath $installer -ArgumentList $arguments -Wait -PassThru
        if ($process.ExitCode -ne 0) {{
            throw "Python installer exited with code $($process.ExitCode)"
        }}
    }} finally {{
        Remove-Item $installer -Force -ErrorAction SilentlyContinue
    }}
}}
if (-not (Test-Path '{PYTHON_PATH}')) {{
    throw 'System Python was not found after installation'
}}
& '{PYTHON_PATH}' --version
if (-not $?) {{ throw 'System Python did not execute' }}
"""
        ),
        timeout,
        "system Python verification",
    )
    # The all-users installer restarts guest services after returning. Let the
    # toolbox settle before starting the comparatively long dependency install.
    time.sleep(15)
    exec_checked(
        sandbox,
        "cmd.exe /d /c echo toolbox-ready",
        timeout,
        "post-Python-install toolbox check",
    )
    exec_checked(
        sandbox,
        powershell_command(
            f"& '{PYTHON_PATH}' -m pip install --upgrade --quiet "
            "--disable-pip-version-check --no-warn-script-location "
            "--progress-bar off cua-computer-server; "
            "if (-not $?) { throw 'pip install failed' }"
        ),
        timeout,
        "cua-computer-server installation",
    )
    exec_checked(
        sandbox,
        powershell_command(
            f"& '{PYTHON_PATH}' -c \"import computer_server\"; "
            "if (-not $?) { throw 'computer_server import failed' }"
        ),
        timeout,
        "computer_server import check",
    )


def launcher_contents() -> bytes:
    """Return the restart-forever server launcher stored in the snapshot."""
    script = f"""$ErrorActionPreference = 'Continue'
$env:PYTHONUNBUFFERED = '1'
$LogFile = '{SERVER_LOG_PATH}'
while ($true) {{
    "[$(Get-Date -Format o)] Starting cua-computer-server" | Out-File -FilePath $LogFile -Append -Encoding utf8
    & '{PYTHON_PATH}' -m computer_server --host 0.0.0.0 --port {SERVER_PORT} --log-level info 2>&1 | Out-File -FilePath $LogFile -Append -Encoding utf8
    $code = $LASTEXITCODE
    "[$(Get-Date -Format o)] Server exited with code $code; restarting in 5 seconds" | Out-File -FilePath $LogFile -Append -Encoding utf8
    Start-Sleep -Seconds 5
}}
"""
    return script.encode("utf-8-sig")


def configure_autostart(sandbox: Any, username: str, timeout: int) -> None:
    """Open the firewall and register the interactive scheduled task."""
    exec_checked(
        sandbox,
        powershell_command(
            f"New-Item -ItemType Directory -Path '{SERVER_DIR}' -Force | Out-Null; "
            "$rule = Get-NetFirewallRule -DisplayName 'Cua Computer Server 8000' "
            "-ErrorAction SilentlyContinue; if ($rule) { $rule | Remove-NetFirewallRule }; "
            "New-NetFirewallRule -DisplayName 'Cua Computer Server 8000' "
            f"-Direction Inbound -Action Allow -Protocol TCP -LocalPort {SERVER_PORT} | Out-Null"
        ),
        timeout,
        "firewall configuration",
    )
    sandbox.fs.upload_file(launcher_contents(), LAUNCHER_PATH, timeout=timeout)

    setup_script = f"""
$ErrorActionPreference = 'Stop'
$TaskName = '{TASK_NAME}'
$UserId = "$env:COMPUTERNAME\\{username}"
$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existing) {{
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
}}
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "{LAUNCHER_PATH}"'
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId
$principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Days 365) -Hidden
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
Start-Sleep -Seconds 2
Get-ScheduledTask -TaskName $TaskName | Select-Object TaskName, State | Format-List
"""
    output = exec_checked(
        sandbox,
        powershell_command(setup_script),
        timeout,
        "scheduled-task configuration",
    )
    print(f"Scheduled task registered for {username}:\n{output.strip()}", flush=True)


def open_json(url: str, timeout: float) -> tuple[int, dict[str, Any]]:
    """GET a JSON endpoint and return its status and decoded object."""
    request = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.status, json.loads(response.read().decode("utf-8"))


def wait_for_health(url: str, timeout: float) -> tuple[int, dict[str, Any]]:
    """Poll the external signed preview URL until the Windows server is healthy."""
    deadline = time.monotonic() + timeout
    last_error = "no response"
    while time.monotonic() < deadline:
        try:
            status_code, payload = open_json(f"{url.rstrip('/')}/status", timeout=15)
            if (
                status_code == 200
                and payload.get("status") == "ok"
                and payload.get("os_type") == "windows"
            ):
                return status_code, payload
            last_error = f"HTTP {status_code}: {payload!r}"
        except (OSError, ValueError, urllib.error.HTTPError) as exc:
            last_error = f"{type(exc).__name__}: {exc}"
        time.sleep(5)
    raise TimeoutError(f"computer-server health check timed out: {last_error}")


def screenshot_check(url: str) -> ScreenshotEvidence:
    """Prove the server can capture the interactive desktop through /cmd."""
    body = json.dumps({"command": "screenshot", "params": {}}).encode("utf-8")
    request = urllib.request.Request(
        f"{url.rstrip('/')}/cmd",
        data=body,
        headers={"Accept": "text/plain", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        status_code = response.status
        response_text = response.read().decode("utf-8")

    payloads = []
    for line in response_text.splitlines():
        if line.startswith("data:"):
            payloads.append(json.loads(line.removeprefix("data:").strip()))
    if not payloads:
        raise RuntimeError(f"screenshot response was not an SSE data payload: {response_text[:1000]}")
    payload = payloads[-1]
    image_data = payload.get("image_data")
    if payload.get("success") is not True or not isinstance(image_data, str):
        raise RuntimeError(f"screenshot command failed: {payload!r}")
    try:
        decoded = base64.b64decode(image_data, validate=True)
    except (ValueError, binascii.Error) as exc:
        raise RuntimeError("screenshot image_data is not valid base64") from exc
    if len(decoded) < 10_000:
        raise RuntimeError(f"screenshot was unexpectedly small: {len(decoded)} decoded bytes")
    return ScreenshotEvidence(status_code, len(image_data), len(decoded))


def print_diagnostics(sandbox: Any, query_output: str, timeout: int) -> None:
    """Print actionable Windows desktop and server diagnostics after a failure."""
    print(f"query user output:\n{query_output.strip()}", file=sys.stderr, flush=True)
    try:
        task_output = exec_checked(
            sandbox,
            powershell_command(
                f"Get-ScheduledTask -TaskName '{TASK_NAME}' -ErrorAction SilentlyContinue | "
                "Get-ScheduledTaskInfo | Format-List *; "
                f"Get-ScheduledTask -TaskName '{TASK_NAME}' -ErrorAction SilentlyContinue | "
                "Select-Object TaskName,State,Principal | Format-List"
            ),
            timeout,
            "scheduled-task diagnostics",
        )
        print(f"scheduled task diagnostics:\n{task_output.strip()}", file=sys.stderr, flush=True)
    except Exception as exc:
        print(f"scheduled task diagnostics failed: {exc}", file=sys.stderr, flush=True)
    try:
        log = sandbox.fs.download_file(SERVER_LOG_PATH, timeout).decode(
            "utf-8", errors="replace"
        )
        print(f"server log tail:\n{log[-8_000:]}", file=sys.stderr, flush=True)
    except Exception as exc:
        print(f"server log download failed: {exc}", file=sys.stderr, flush=True)


def delete_existing_snapshot(daytona: Daytona, name: str, timeout: float) -> None:
    """Delete an existing owned snapshot so this build is deterministic."""
    try:
        snapshot = daytona.snapshot.get(name)
    except DaytonaNotFoundError:
        return
    print(f"Deleting existing snapshot {name!r}", flush=True)
    deadline = time.monotonic() + timeout
    while True:
        try:
            daytona.snapshot.delete(snapshot)
            return
        except DaytonaNotFoundError:
            return
        except Exception:
            if time.monotonic() >= deadline:
                raise
            time.sleep(5)
            try:
                snapshot = daytona.snapshot.get(name)
            except DaytonaNotFoundError:
                return


def state_value(state: Any) -> str:
    """Normalize SDK string enums across daytona-sdk versions."""
    return str(getattr(state, "value", state)).lower()


def wait_for_active_snapshot(
    daytona: Daytona,
    sandbox: Any,
    name: str,
    started_at: float,
    timeout: float,
) -> Any:
    """Wait for a live-sandbox snapshot record, treating initial 404s as pending."""
    deadline = started_at + timeout
    absent_after_transition_at: float | None = None
    last_state = "unknown"
    while time.monotonic() < deadline:
        try:
            snapshot = daytona.snapshot.get(name)
        except DaytonaNotFoundError:
            sandbox.refresh_data()
            last_state = state_value(sandbox.state)
            if last_state != "snapshotting":
                absent_after_transition_at = absent_after_transition_at or time.monotonic()
                if time.monotonic() - absent_after_transition_at > 60:
                    raise RuntimeError(
                        f"snapshot {name!r} never appeared after sandbox left "
                        f"snapshotting (state={last_state})"
                    )
            time.sleep(5)
            continue

        state = state_value(snapshot.state)
        if state == "active":
            return snapshot
        if state in {"error", "build_failed", "failed"}:
            raise RuntimeError(f"snapshot {name!r} failed with state={snapshot.state}")
        absent_after_transition_at = None
        last_state = state
        time.sleep(5)
    raise TimeoutError(f"snapshot {name!r} did not become active; last state={last_state}")


def create_live_snapshot(
    daytona: Daytona,
    sandbox: Any,
    name: str,
    timeout: float,
) -> Any:
    """Stop the VM, snapshot its disk, and wait for the record to become active."""
    started_at = time.monotonic()
    sandbox.stop(timeout=min(timeout, 300))
    remaining = max(1, timeout - (time.monotonic() - started_at))
    sandbox._experimental_create_snapshot(name, timeout=remaining)
    return wait_for_active_snapshot(daytona, sandbox, name, started_at, timeout)


def delete_sandbox(daytona: Daytona, sandbox: Any, timeout: float) -> None:
    """Delete a sandbox, retrying while it is in a transitional state."""
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            sandbox.delete(timeout=min(60, max(1, deadline - time.monotonic())), wait=True)
            return
        except DaytonaNotFoundError:
            return
        except Exception as exc:
            last_error = exc
            time.sleep(5)
            try:
                sandbox = daytona.get(sandbox.id)
            except DaytonaNotFoundError:
                return
    raise TimeoutError(f"failed to delete sandbox {sandbox.id}: {last_error}")


def create_sandbox(daytona: Daytona, snapshot: str, label: str, timeout: float) -> Any:
    """Create a no-autostop sandbox from a snapshot and wait for it to start."""
    params = CreateSandboxFromSnapshotParams(
        snapshot=snapshot,
        auto_stop_interval=0,
        labels={label: "true"},
    )
    sandbox = daytona.create(params, timeout=timeout)
    sandbox.wait_for_sandbox_start(timeout=timeout)
    try:
        sandbox.set_autostop_interval(0)
    except Exception as exc:
        print(f"Warning: could not enforce auto-stop interval: {exc}", flush=True)
    return sandbox


def validate_snapshot(daytona: Daytona, name: str, timeouts: Timeouts) -> None:
    """Restore one fresh VM and prove server autostart plus interactive capture."""
    sandbox = None
    try:
        sandbox = create_sandbox(
            daytona,
            name,
            "cua-bench-snapshot-validator",
            timeouts.create,
        )
        signed = sandbox.create_signed_preview_url(
            SERVER_PORT,
            expires_in_seconds=max(3_600, int(timeouts.health + 600)),
        )
        status_code, status = wait_for_health(signed.url, timeouts.health)
        screenshot = screenshot_check(signed.url)
        print(
            "Validation evidence: "
            f"/status HTTP {status_code} {status!r}; "
            f"screenshot HTTP {screenshot.status_code}, "
            f"base64_chars={screenshot.encoded_size}, "
            f"decoded_bytes={screenshot.decoded_size}",
            flush=True,
        )
    finally:
        if sandbox is not None:
            delete_sandbox(daytona, sandbox, timeouts.cleanup)


def main() -> int:
    """Build, externally verify, snapshot, restore, and validate the Windows VM."""
    args = parse_args()
    timeouts = Timeouts(
        create=args.create_timeout,
        command=args.command_timeout,
        health=args.health_timeout,
        snapshot=args.snapshot_timeout,
        cleanup=args.cleanup_timeout,
    )
    daytona = make_client()
    timings: dict[str, float] = {}
    builder = None
    snapshot = None
    query_output = "query user was not run"

    delete_existing_snapshot(daytona, args.name, timeouts.cleanup)
    try:
        phase = time.monotonic()
        builder = create_sandbox(
            daytona,
            args.base,
            "cua-bench-snapshot-builder",
            timeouts.create,
        )
        timings["create"] = time.monotonic() - phase
        print(f"Builder sandbox: {builder.id}", flush=True)

        username, query_output = discover_interactive_user(builder, timeouts.command)
        print(f"Interactive user: {username}\n{query_output.strip()}", flush=True)

        phase = time.monotonic()
        install_server(builder, timeouts.command)
        timings["install"] = time.monotonic() - phase

        phase = time.monotonic()
        configure_autostart(builder, username, timeouts.command)
        signed = builder.create_signed_preview_url(
            SERVER_PORT,
            expires_in_seconds=max(3_600, int(timeouts.snapshot + timeouts.health + 600)),
        )
        try:
            status_code, status = wait_for_health(signed.url, timeouts.health)
            screenshot = screenshot_check(signed.url)
        except Exception:
            print_diagnostics(builder, query_output, timeouts.command)
            raise
        timings["configure_and_probe"] = time.monotonic() - phase
        print(
            "Builder evidence: "
            f"/status HTTP {status_code} {status!r}; "
            f"screenshot HTTP {screenshot.status_code}, "
            f"base64_chars={screenshot.encoded_size}, "
            f"decoded_bytes={screenshot.decoded_size}",
            flush=True,
        )

        phase = time.monotonic()
        snapshot = create_live_snapshot(daytona, builder, args.name, timeouts.snapshot)
        timings["snapshot"] = time.monotonic() - phase
        print(f"Snapshot active: {snapshot.name} state={snapshot.state}", flush=True)
    finally:
        if builder is not None and not args.keep_sandbox:
            phase = time.monotonic()
            delete_sandbox(daytona, builder, timeouts.cleanup)
            timings["builder_cleanup"] = time.monotonic() - phase
        elif builder is not None:
            print(f"Keeping builder sandbox {builder.id}", flush=True)

    if snapshot is None:
        return 1

    try:
        phase = time.monotonic()
        validate_snapshot(daytona, args.name, timeouts)
        timings["validate"] = time.monotonic() - phase
    except Exception:
        print("Validation failed; deleting the unusable snapshot", file=sys.stderr, flush=True)
        try:
            daytona.snapshot.delete(snapshot)
        except Exception as exc:
            print(f"Snapshot cleanup also failed: {exc}", file=sys.stderr, flush=True)
        raise

    print(
        "Timings: " + ", ".join(f"{name}={seconds:.3f}s" for name, seconds in timings.items()),
        flush=True,
    )
    print(f"Snapshot ready: {args.name}", flush=True)
    print(f"Usage: export DAYTONA_WINDOWS_SNAPSHOT={args.name}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
