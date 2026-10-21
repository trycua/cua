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

#: Grace period between SIGTERM and SIGKILL when stopping ``cb run``:
#: SIGTERM cancels the batch so running variants release their sandboxes.
#: Must outlast cua-bench's bounded sandbox release (``TIME_TO_RELEASE_S``,
#: 300 s) or the SIGKILL can land mid-release and leak the sandbox the
#: cancel was freeing.
_TERM_GRACE_S = 360.0

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
        except OSError:
            proc.kill()
    else:
        proc.kill()


def _decode_output(data: str | bytes | None) -> str:
    # ``TimeoutExpired.output`` is bytes even when the pipes are text mode.
    if isinstance(data, bytes):
        return data.decode("utf-8", errors="replace")
    return data or ""


def _stop(proc: subprocess.Popen) -> tuple[str, str]:
    """SIGTERM cb alone (its cancel handler releases sandboxes), then
    SIGKILL the whole process group, draining output under bounded waits.
    Retried ``communicate`` calls return the output earlier calls already
    accumulated, so nothing cb printed is lost on the way out."""
    proc.terminate()
    try:
        return proc.communicate(timeout=_TERM_GRACE_S)
    except subprocess.TimeoutExpired:
        pass
    _kill_process_group(proc)
    try:
        return proc.communicate(timeout=_KILL_GRACE_S)
    except subprocess.TimeoutExpired as exc:
        stdout = _decode_output(exc.output)
        stderr = _decode_output(exc.stderr)
    for stream in (proc.stdout, proc.stderr):
        if stream is not None:
            stream.close()
    try:
        proc.wait(timeout=_KILL_GRACE_S)
    except subprocess.TimeoutExpired:
        pass
    return stdout, stderr


def _get_run_output_dir(run_id: str, env: dict[str, str]) -> Path:
    xdg_data = env.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
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
    timed_out = False
    try:
        stdout, stderr = proc.communicate(timeout=run_timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        print(
            f"[rollout] cb run exceeded {run_timeout}s; stopping it "
            f"(SIGTERM, SIGKILL after {_TERM_GRACE_S:.0f}s)"
        )
        stdout, stderr = _stop(proc)
    except BaseException:
        # cb leads its own process group, so nothing else will signal it:
        # a Ctrl-C or other exit must not orphan it with sandboxes running.
        if proc.poll() is None:
            _stop(proc)
        raise

    combined_output = stdout + stderr
    run_id = _parse_run_id(combined_output)

    if run_id is None:
        raise RuntimeError(
            "Could not parse run_id from cb output.\n" f"STDOUT:\n{stdout}\n" f"STDERR:\n{stderr}"
        )

    run_dir = _get_run_output_dir(run_id, env)

    if timed_out or proc.returncode != 0:
        # A stopped or failed cb leaves sessions that can never finish. cb
        # writes run.pid, run.log and the variant dirs before any variant
        # finishes, so a non-empty dir alone cannot tell a crashed run from
        # one with results: count result.json, the marker _finish writes per
        # finished variant (cb run exits 1 when any variant failed, so a
        # non-zero status is routine).
        reason = (
            "timed out and was stopped" if timed_out else f"exited with status {proc.returncode}"
        )
        finished = list(run_dir.glob("*/result.json"))
        if finished:
            print(
                f"[rollout] cb run {run_id} {reason}; "
                f"keeping {len(finished)} finished variant(s)"
            )
            tail = stderr.strip()[-200:]
            if tail:
                print(f"[rollout] cb stderr tail: {tail}")
            return run_id, run_dir
        raise RuntimeError(
            f"cb run {run_id} {reason} and left no finished variants in {run_dir}.\n"
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
