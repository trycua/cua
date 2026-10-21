from __future__ import annotations

import os
import re
import signal
import subprocess
import time
from pathlib import Path

# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

_ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
_RUN_ID_RE = re.compile(r"Run ID:\s+([a-f0-9-]{8,})")

#: Grace period between SIGTERM and SIGKILL when ``run_timeout`` stops
#: ``cb run``: SIGTERM cancels the batch so running variants release their
#: sandboxes, then the process is killed if it still has not exited.
_TERM_GRACE_S = 60.0

#: Bound on the last ``communicate`` after the process group is killed: a
#: descendant holding cb's pipes open would otherwise wait forever.
_KILL_GRACE_S = 10.0


def _kill_process_group(proc: subprocess.Popen) -> None:
    """SIGKILL cb's whole process group (POSIX); the process alone elsewhere."""
    if hasattr(os, "killpg"):
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    else:
        proc.kill()


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
    variant_timeout: float | None = None,
    run_timeout: float | None = None,
    extra_env: dict[str, str] | None = None,
) -> tuple[str, Path]:
    """Run a single ``cb run task`` invocation and block until it finishes."""
    env = {**os.environ, **(extra_env or {})}

    cmd = [
        "cb",
        "run",
        "dataset",
        str(tasks_path),
        "--agent",
        agent,
        "--model",
        model,
        "--max-steps",
        str(max_steps),
        "--with",
        "libs/python/agent/agent",
    ]
    if variant_timeout is not None:
        cmd += ["--variant-timeout", str(variant_timeout)]

    print(f"[rollout] Launching: {' '.join(cmd)}")
    # Its own process group: a wedged cb is killed with the descendants that
    # may hold its stdout/stderr pipes open.
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        start_new_session=True,
    )
    stdout, stderr = "", ""
    timed_out = False
    try:
        stdout, stderr = proc.communicate(timeout=run_timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        print(
            f"[rollout] cb run exceeded {run_timeout}s; stopping it "
            f"(SIGTERM, SIGKILL after {_TERM_GRACE_S:.0f}s)"
        )
        # SIGTERM to cb alone: its cancel handler releases the sandboxes.
        proc.terminate()
        try:
            stdout, stderr = proc.communicate(timeout=_TERM_GRACE_S)
        except subprocess.TimeoutExpired:
            _kill_process_group(proc)
            try:
                stdout, stderr = proc.communicate(timeout=_KILL_GRACE_S)
            except subprocess.TimeoutExpired:
                for stream in (proc.stdout, proc.stderr):
                    if stream is not None:
                        stream.close()
                proc.wait()

    combined_output = stdout + stderr
    run_id = _parse_run_id(combined_output)

    if run_id is None:
        raise RuntimeError(
            "Could not parse run_id from cb output.\n" f"STDOUT:\n{stdout}\n" f"STDERR:\n{stderr}"
        )

    run_dir = _get_run_output_dir(run_id)

    if timed_out or proc.returncode != 0:
        # A stopped or failed cb leaves sessions that can never finish. Keep
        # the run directory when variants already finished (cb run exits 1
        # whenever any variant failed, so a non-zero status is routine); a
        # missing or empty one means cb died before writing anything.
        reason = (
            "timed out and was stopped" if timed_out else f"exited with status {proc.returncode}"
        )
        if run_dir.is_dir() and any(run_dir.iterdir()):
            print(f"[rollout] cb run {run_id} {reason}; keeping finished variants")
            tail = stderr.strip()[-200:]
            if tail:
                print(f"[rollout] cb stderr tail: {tail}")
            return run_id, run_dir
        raise RuntimeError(
            f"cb run {run_id} {reason} and left no results in {run_dir}.\n"
            f"STDERR (tail):\n{stderr.strip()[-500:]}"
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
    variant_timeout: float | None = None,
    run_timeout: float | None = None,
    extra_env: dict[str, str] | None = None,
) -> list[tuple[str, Path]]:
    """Run cua-bench rollouts and block until all sessions finish."""
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
            variant_timeout=variant_timeout,
            run_timeout=run_timeout,
            extra_env=extra_env,
        )
        results.append((run_id, run_dir))
    return results
