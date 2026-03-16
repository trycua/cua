"""Daytona harness for cua-bench.

Manages a single Daytona sandbox that runs both the env (KiCad desktop) and
the solver.  Computer-use traffic stays on localhost:8000 inside the sandbox,
so no base64-exec calls are needed during the agent loop.
"""

import os
import threading
import time
import urllib.request
from pathlib import Path
from typing import Dict, List, Optional


class DaytonaHarness:
    """Manages a single Daytona sandbox for task execution.

    Architecture:
    - One sandbox runs supervisord (VNC + noVNC + cua-computer-server).
    - The solver also runs inside the sandbox, connecting to localhost:8000.
    - Only 2-3 process.exec() calls are made (bootstrap + solver launch).
    """

    def __init__(self) -> None:
        """Initialize DaytonaHarness.

        Reads DAYTONA_API_KEY from environment (required).
        Reads DAYTONA_API_URL from environment (optional; defaults to Daytona cloud).

        Raises:
            ImportError: If daytona-sdk is not installed.
            ValueError: If DAYTONA_API_KEY is not set.
        """
        api_key = os.environ.get("DAYTONA_API_KEY")
        if not api_key:
            raise ValueError(
                "DAYTONA_API_KEY not set. Please export DAYTONA_API_KEY=<your key> "
                "before running with --provider-type daytona."
            )

        try:
            from daytona_sdk import Daytona, DaytonaConfig
        except ImportError:
            raise ImportError(
                "daytona-sdk is required for the daytona provider.\n"
                "Install it with: pip install 'cua-bench[daytona]'"
            )

        api_url = os.environ.get("DAYTONA_API_URL")
        if api_url:
            self._daytona = Daytona(DaytonaConfig(api_key=api_key, api_url=api_url))
        else:
            self._daytona = Daytona(DaytonaConfig(api_key=api_key))

        self._sandbox = None
        self._keepalive_stop = threading.Event()
        self._keepalive_thread: Optional[threading.Thread] = None

    async def start_env_sandbox(
        self,
        image: str,
        env_vars: Optional[Dict[str, str]] = None,
    ) -> str:
        """Create and start the sandbox.

        The snapshot must have supervisord baked in as its entrypoint, so it
        starts automatically — no process.exec() calls are made here.

        Args:
            image: Daytona snapshot name (with supervisord entrypoint baked in).
            env_vars: Environment variables to bake into the sandbox at creation.

        Returns:
            Forwarded API URL for the cua-computer-server (port 8000).
        """
        from daytona_sdk.common.daytona import CreateSandboxFromSnapshotParams

        params = CreateSandboxFromSnapshotParams(
            snapshot=image,
            env_vars=env_vars or {},
            auto_stop_interval=0,
        )
        self._sandbox = self._daytona.create(params, timeout=300)
        sb = self._sandbox
        print(
            f"    [harness] Sandbox created:"
            f" id={sb.id}"
            f" state={sb.state}"
            f" auto_stop_interval={sb.auto_stop_interval}",
            flush=True,
        )

        # Explicitly disable auto-stop post-creation (platform may override the param).
        try:
            self._sandbox.set_autostop_interval(0)
            print("    [harness] set_autostop_interval(0) OK", flush=True)
        except Exception as exc:
            print(f"    [harness] WARNING: set_autostop_interval failed: {exc}", flush=True)

        # Get signed URL for port 8000 (health-check only; solver uses localhost)
        signed = self._sandbox.create_signed_preview_url(8000, expires_in_seconds=28800)
        url = signed.url
        self._wait_for_env_ready(url)

        # Start a lightweight keepalive thread: REST-only, no process.exec() calls.
        self._keepalive_stop.clear()
        self._keepalive_thread = threading.Thread(
            target=self._keepalive_loop, daemon=True
        )
        self._keepalive_thread.start()

        return url

    def _keepalive_loop(self) -> None:
        """Periodically refresh sandbox activity via REST (no process.exec calls)."""
        interval = 20  # every 20 seconds
        t0 = time.monotonic()
        while not self._keepalive_stop.wait(interval):
            if self._sandbox is None:
                break
            elapsed = int(time.monotonic() - t0)
            try:
                self._sandbox.refresh_activity()
            except Exception as e:
                print(f"    [keepalive T+{elapsed}s] refresh_activity FAILED: {e}", flush=True)
            try:
                self._sandbox.refresh_data()
                sb = self._sandbox
                print(
                    f"    [keepalive T+{elapsed}s]"
                    f" state={sb.state}"
                    f" auto_stop={sb.auto_stop_interval}",
                    flush=True,
                )
            except Exception as e:
                print(f"    [keepalive T+{elapsed}s] refresh_data FAILED: {e}", flush=True)

    def _wait_for_env_ready(
        self,
        api_url: str,
        timeout: int = 900,
        interval: int = 10,
    ) -> None:
        """Poll the cua-computer-server health endpoint until it responds."""
        health_url = api_url.rstrip("/") + "/status"
        deadline = time.monotonic() + timeout
        print(f"    Waiting for env sandbox to be ready at {api_url} ...", flush=True)
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(health_url, timeout=5) as resp:
                    if resp.status == 200:
                        print("    Env sandbox ready.", flush=True)
                        return
            except Exception:
                pass
            time.sleep(interval)
        raise TimeoutError(
            f"Env sandbox at {api_url} did not become ready within {timeout}s"
        )

    def upload_solver_code(self, cua_bench_root: Path) -> None:
        """Upload the cua_bench package directly to /tmp/solver_src/ in the sandbox.

        Uses fs.upload_files() to send each file individually — no exec/unzip needed.

        Args:
            cua_bench_root: Local path to the cua_bench package directory.
        """
        from daytona_sdk.common.filesystem import FileUpload

        files = []
        for file in cua_bench_root.rglob("*"):
            if file.is_file() and "__pycache__" not in file.parts:
                arcname = file.relative_to(cua_bench_root.parent)
                dest = f"/tmp/solver_src/{arcname}"
                files.append(FileUpload(source=str(file), destination=dest))

        total_kb = sum(Path(f.source).stat().st_size for f in files) // 1024
        print(f"    [harness] Uploading solver source ({len(files)} files, {total_kb} KB) ...", flush=True)
        self._sandbox.fs.upload_files(files)
        print("    [harness] Solver source uploaded.", flush=True)

    def upload_task_files(self, task_path: Path) -> str:
        """Upload the task directory to /tmp/task_env/ inside the sandbox.

        Uses fs.upload_files() to send each file individually — no exec/unzip needed.

        Args:
            task_path: Local path to the task directory.

        Returns:
            Remote path where the task was uploaded (/tmp/task_env).
        """
        from daytona_sdk.common.filesystem import FileUpload

        remote_task_dir = "/tmp/task_env"
        files = []
        for file in task_path.rglob("*"):
            if file.is_file():
                dest = f"{remote_task_dir}/{file.relative_to(task_path)}"
                files.append(FileUpload(source=str(file), destination=dest))

        self._sandbox.fs.upload_files(files)
        return remote_task_dir

    def run_solver_in_sandbox(
        self,
        command: List[str],
        env_vars: Dict[str, str],
        pythonpath_prepend: str = "/tmp/solver_src",
        timeout: Optional[int] = None,
    ) -> tuple[int, str]:
        """Run the solver command inside the sandbox and return (exit_code, logs).

        Uses the sessions API (not process.exec) to avoid the SDK's base64+exec
        wrapping which triggers Daytona's abuse detection.

        Args:
            command: Solver argv list.
            env_vars: Extra environment variables for the solver.
            pythonpath_prepend: Path to prepend to PYTHONPATH inside the sandbox.
            timeout: Max seconds to wait (None = wait forever).

        Returns:
            (exit_code, stdout+stderr output)
        """
        from daytona_sdk.common.process import SessionExecuteRequest

        # Replace the local python interpreter path with the sandbox venv python
        if command and (command[0].endswith("python") or command[0].endswith("python3")):
            command = ["/opt/venv/bin/python3"] + command[1:]

        full_env = dict(env_vars)
        existing_pp = full_env.get("PYTHONPATH", "")
        full_env["PYTHONPATH"] = (
            f"{pythonpath_prepend}:{existing_pp}" if existing_pp else pythonpath_prepend
        )
        full_env["CUA_ENV_API_URL"] = "http://localhost:8000"

        # Build env export prefix and full command string
        env_prefix = " ".join(f'{k}="{v}"' for k, v in full_env.items())
        cmd_str = f"{env_prefix} {' '.join(command)}"
        print(f"    [harness] Launching solver via session: {' '.join(command)}", flush=True)

        session_id = "solver"
        print("    [harness] Creating session...", flush=True)
        self._sandbox.process.create_session(session_id)
        print("    [harness] Session created.", flush=True)

        # Fire the command asynchronously
        resp = self._sandbox.process.execute_session_command(
            session_id,
            SessionExecuteRequest(command=cmd_str, run_async=True),
        )
        cmd_id = resp.cmd_id
        print(f"    [harness] Session command launched: cmd_id={cmd_id}", flush=True)

        # Poll for completion
        deadline = time.monotonic() + timeout if timeout else None
        poll_interval = 10
        t0 = time.monotonic()
        while True:
            if deadline and time.monotonic() > deadline:
                print("    [harness] Solver timed out.", flush=True)
                return -1, ""
            time.sleep(poll_interval)
            elapsed = int(time.monotonic() - t0)
            result = self._sandbox.process.get_session_command(session_id, cmd_id)
            exit_code = getattr(result, "exit_code", None)
            print(f"    [harness] poll T+{elapsed}s exit_code={exit_code}", flush=True)
            if exit_code is not None:
                logs = getattr(result, "output", "") or ""
                return exit_code, logs

    async def cleanup(self) -> None:
        """Delete the sandbox."""
        self._keepalive_stop.set()
        if self._keepalive_thread is not None:
            self._keepalive_thread.join(timeout=2)
            self._keepalive_thread = None

        if self._sandbox is not None:
            try:
                self._daytona.delete(self._sandbox)
            except Exception:
                pass
            self._sandbox = None
