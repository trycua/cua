"""Spawn a cua-bench dataset run and block until all sessions complete.

``run_rollouts`` is the single entry point.  It:

1. Calls ``cb run dataset`` as a subprocess, capturing stdout/stderr.
2. Parses the ``run-<id>`` token from the CLI output.
3. Polls ``cua_bench.sessions.manager.list_sessions`` until every session
   belonging to that run is in a terminal state (completed / failed / cancelled).
4. Returns the ``(run_id, run_output_dir)`` pair so the caller can load traces.

The poll uses the on-disk session store (``~/.local/state/cua-bench/runs.json``)
directly — no subprocess needed for monitoring.
"""

from __future__ import annotations

import os
import re
import subprocess
import time
from pathlib import Path


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

_ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
_RUN_ID_RE = re.compile(r"Run ID:\s+([a-f0-9-]{8,})")


def _get_run_output_dir(run_id: str) -> Path:
    xdg_data = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return Path(xdg_data) / "cua-bench" / "runs" / run_id


def _parse_run_id(text: str) -> str | None:
    clean = _ANSI_RE.sub("", text)
    m = _RUN_ID_RE.search(clean)
    return m.group(1) if m else None


def _terminal_statuses() -> frozenset[str]:
    return frozenset({"completed", "failed", "cancelled"})


def _all_sessions_terminal(run_id: str) -> bool:
    """Return True when every session in *run_id* has reached a terminal state."""
    from cua_bench.sessions.manager import list_sessions

    sessions = [s for s in list_sessions() if s.get("run_id") == run_id]
    if not sessions:
        # Sessions may not have been registered yet if the runner is still
        # starting containers; treat as "not done".
        return False
    return all(s.get("status") in _terminal_statuses() for s in sessions)


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def run_rollouts(
    tasks_path: str | Path,
    agent: str,
    model: str,
    max_steps: int,
    *,
    timeout: int = 3600,
    poll_interval: int = 10,
    extra_env: dict[str, str] | None = None,
) -> tuple[str, Path]:
    """Run cua-bench rollouts and block until all sessions finish.

    Parameters
    ----------
    tasks_path:
        Path to the task environment directory passed to ``cb run dataset``.
    agent:
        Agent name from the cua-bench registry (e.g. ``"opencua"``).
    model:
        litellm model string forwarded to the agent (e.g. ``"openai/opencua"``).
    max_steps:
        Maximum agent steps per episode.
    timeout:
        Maximum seconds to wait for all sessions to reach a terminal state.
    poll_interval:
        Seconds between session-state polls.
    extra_env:
        Additional environment variables merged into the subprocess environment.
        Use this to pass ``OPENCUA_BASE_URL``, ``OPENAI_API_KEY``, etc. without
        mutating the current process environment.

    Returns
    -------
    (run_id, run_output_dir)
        ``run_output_dir`` is the XDG data directory that holds ``task_N_trace/``
        subdirectories for each completed task.

    Raises
    ------
    RuntimeError
        If the run_id cannot be parsed from CLI output.
    TimeoutError
        If sessions do not finish within *timeout* seconds.
    FileNotFoundError
        If the run output directory does not exist after sessions finish.
    """
    env = {**os.environ, **(extra_env or {})}

    cmd = [
        "cb", "run", "dataset", str(tasks_path),
        "--agent", agent,
        "--model", model,
        "--max-steps", str(max_steps),
    ]
    
    # breakpoint()

    print(f"[rollout] Launching: {' '.join(cmd)}")
    proc = subprocess.run(cmd, capture_output=True, text=True, env=env)

    combined_output = proc.stdout + proc.stderr
    
    # print(combined_output)
    
    breakpoint()
    
    run_id = _parse_run_id(combined_output)

    if run_id is None:
        raise RuntimeError(
            "Could not parse run_id from cb output.\n"
            f"STDOUT:\n{proc.stdout}\n"
            f"STDERR:\n{proc.stderr}"
        )

    print(f"[rollout] Run {run_id} launched. Waiting for sessions to complete...")

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if _all_sessions_terminal(run_id):
            break
        time.sleep(poll_interval)
    else:
        raise TimeoutError(
            f"Run {run_id} did not complete within {timeout}s. "
            "Consider increasing the timeout or checking for stuck containers."
        )

    print(f"[rollout] Run {run_id} complete.")

    run_dir = _get_run_output_dir(run_id)
    if not run_dir.exists():
        raise FileNotFoundError(
            f"Expected run output directory not found: {run_dir}. "
            "The run may have used a custom output path."
        )

    return run_id, run_dir
