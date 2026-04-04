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
        return False
    return all(s.get("status") in _terminal_statuses() for s in sessions)


def _run_single_rollout(
    tasks_path: str | Path,
    agent: str,
    model: str,
    max_steps: int,
    *,
    timeout: int = 3600,
    poll_interval: int = 10,
    extra_env: dict[str, str] | None = None,
) -> tuple[str, Path]:
    """Run a single ``cb run task`` invocation and block until it finishes."""
    env = {**os.environ, **(extra_env or {})}

    cmd = [
        "cb", "run", "dataset", str(tasks_path),
        "--agent", agent,
        "--model", model,
        "--max-steps", str(max_steps),
        "--with", "libs/python/agent/agent"
    ]

    print(f"[rollout] Launching: {' '.join(cmd)}")
    proc = subprocess.run(cmd, capture_output=True, text=True, env=env)

    combined_output = proc.stdout + proc.stderr
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


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def run_rollouts(
    tasks_path: str | Path,
    agent: str,
    model: str,
    max_steps: int,
    *,
    num_rollouts: int = 1,
    timeout: int = 3600,
    poll_interval: int = 10,
    extra_env: dict[str, str] | None = None,
) -> list[tuple[str, Path]]:
    """Run cua-bench rollouts and block until all sessions finish.
    """
    results: list[tuple[str, Path]] = []
    for i in range(num_rollouts):
        if num_rollouts > 1:
            print(f"[rollout] === Rollout {i + 1}/{num_rollouts} ===")
        run_id, run_dir = _run_single_rollout(
            tasks_path=tasks_path,
            agent=agent,
            model=model,
            max_steps=max_steps,
            timeout=timeout,
            poll_interval=poll_interval,
            extra_env=extra_env,
        )
        results.append((run_id, run_dir))
    return results
