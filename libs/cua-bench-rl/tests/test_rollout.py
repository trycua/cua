"""Bounds on the ``cb run`` subprocess in tinker rollout (#5193).

``_run_single_rollout`` waited on ``cb run`` with no timeout and then polled
the session store for up to ``timeout`` (3600 s) even when the cb process
was already dead. These tests drive a real fake ``cb`` executable through
the real subprocess path: one that exits non-zero, one that ignores SIGTERM
and must be killed, and one that records the argv it was called with.
"""

from __future__ import annotations

import importlib.util
import json
import os
import signal
import stat
import subprocess
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

# The tinker package's __init__ needs the optional ``tinker`` extra; the CI
# test-rl job installs dev only, so rollout.py is loaded by path.
_ROLLOUT_PY = (
    Path(__file__).resolve().parents[1]
    / "cua_bench_rl"
    / "trainer"
    / "off_policy"
    / "tinker"
    / "rollout.py"
)


def _load_rollout():
    spec = importlib.util.spec_from_file_location("cb_tinker_rollout", _ROLLOUT_PY)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


rollout = _load_rollout()

FAKE_CB = """#!/usr/bin/env python3
import json, os, signal, subprocess, sys, time

mode = os.environ.get("FAKE_CB_MODE", "ok")
if mode == "sleep":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
ready_file = os.environ.get("FAKE_CB_READY_FILE")
if ready_file:
    open(ready_file, "w").write("1")
print("Run ID: deadbeef1234", flush=True)
argv_file = os.environ.get("FAKE_CB_ARGV_FILE")
if argv_file:
    with open(argv_file, "w") as f:
        json.dump(sys.argv, f)
if mode == "fail":
    print("cb exploded", file=sys.stderr)
    sys.exit(3)
if mode == "sleep":
    pid_file = os.environ.get("FAKE_CB_CHILD_PID_FILE")
    if pid_file:
        # A grandchild that inherits cb's pipes and outlives cb's own
        # termination unless the whole process group is killed.
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(300)"])
        open(pid_file, "w").write(str(child.pid))
    time.sleep(300)
"""

RUN_ID = "deadbeef1234"


def _fake_cb(tmp_path, monkeypatch, mode="ok"):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    cb = bin_dir / "cb"
    cb.write_text(FAKE_CB)
    cb.chmod(cb.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    monkeypatch.setenv("FAKE_CB_MODE", mode)
    return {"PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}"}


@pytest.fixture
def run_dir(tmp_path, monkeypatch):
    """The real run-dir layout: cb writes run.pid/run.log at startup, before
    any variant finishes; ``*/result.json`` is the finished-variant marker."""
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
    path = tmp_path / "cua-bench" / "runs" / RUN_ID
    variant = path / "hello_env_v0"
    variant.mkdir(parents=True)
    (path / "run.pid").write_text("1234")
    (path / "run.log").write_text("plan")
    (variant / "result.json").write_text("{}")
    return path


def test_a_cb_that_exited_nonzero_is_not_polled(tmp_path, monkeypatch, run_dir):
    extra_env = _fake_cb(tmp_path, monkeypatch, mode="fail")
    polls = []
    monkeypatch.setattr(rollout, "_all_sessions_terminal", lambda rid: polls.append(rid) or False)
    run_id, out = rollout._run_single_rollout(
        "tasks/", "agent", "model", 5, timeout=2, poll_interval=0.05, extra_env=extra_env
    )
    assert (run_id, out) == (RUN_ID, run_dir)
    assert polls == []


def test_a_run_timeout_stops_cb_and_keeps_the_run_dir(tmp_path, monkeypatch, run_dir):
    extra_env = _fake_cb(tmp_path, monkeypatch, mode="sleep")
    monkeypatch.setattr(rollout, "_TERM_GRACE_S", 0.5)
    monkeypatch.setattr(
        rollout,
        "_all_sessions_terminal",
        lambda rid: pytest.fail("polled a stopped cb"),
    )
    started = time.monotonic()
    run_id, out = rollout._run_single_rollout(
        "tasks/", "agent", "model", 5, run_timeout=5, extra_env=extra_env
    )
    assert (run_id, out) == (RUN_ID, run_dir)
    # SIGTERM (ignored) -> SIGKILL after the grace period; no poll, no 300 s wait.
    assert time.monotonic() - started < 30


@pytest.mark.skipif(not hasattr(os, "killpg"), reason="process-group kill is POSIX-only")
def test_a_run_timeout_kills_the_process_group(tmp_path, monkeypatch, run_dir):
    """A pipe-holding grandchild dies with cb instead of holding EOF open."""
    pid_file = tmp_path / "child.pid"
    monkeypatch.setenv("FAKE_CB_CHILD_PID_FILE", str(pid_file))
    extra_env = _fake_cb(tmp_path, monkeypatch, mode="sleep")
    monkeypatch.setattr(rollout, "_TERM_GRACE_S", 0.5)
    monkeypatch.setattr(
        rollout,
        "_all_sessions_terminal",
        lambda rid: pytest.fail("polled a stopped cb"),
    )
    started = time.monotonic()
    run_id, out = rollout._run_single_rollout(
        "tasks/", "agent", "model", 5, run_timeout=5, extra_env=extra_env
    )
    assert (run_id, out) == (RUN_ID, run_dir)
    assert time.monotonic() - started < 30
    child_pid = int(pid_file.read_text())
    deadline = time.monotonic() + 5
    while True:
        try:
            os.kill(child_pid, 0)
        except ProcessLookupError:
            break
        if time.monotonic() > deadline:
            pytest.fail("cb's grandchild survived the process-group kill")
        time.sleep(0.1)


def test_a_failed_cb_with_no_finished_variants_raises(tmp_path, monkeypatch):
    """A crashed cb fails loudly with its stderr tail. Its run dir is never
    empty in production (run.pid and run.log come before any result.json),
    so the gate looks for finished variants, not any file."""
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
    path = tmp_path / "cua-bench" / "runs" / RUN_ID
    path.mkdir(parents=True)
    (path / "run.pid").write_text("1234")
    (path / "run.log").write_text("plan")
    extra_env = _fake_cb(tmp_path, monkeypatch, mode="fail")
    monkeypatch.setattr(
        rollout,
        "_all_sessions_terminal",
        lambda rid: pytest.fail("polled a stopped cb"),
    )
    with pytest.raises(RuntimeError) as excinfo:
        rollout._run_single_rollout("tasks/", "agent", "model", 5, extra_env=extra_env)
    assert "no finished variants" in str(excinfo.value) and "cb exploded" in str(excinfo.value)


def test_an_interrupted_rollout_still_stops_cb(tmp_path, monkeypatch):
    """cb leads its own process group, so a Ctrl-C no longer reaches it:
    every exit path must still run the stop sequence or cb leaks, running
    its sandboxes, after the trainer died."""
    ready_file = tmp_path / "cb-ready"
    monkeypatch.setenv("FAKE_CB_READY_FILE", str(ready_file))
    extra_env = _fake_cb(tmp_path, monkeypatch, mode="sleep")
    monkeypatch.setattr(rollout, "_TERM_GRACE_S", 0.5)

    spawned = []

    class InterruptOncePopen(subprocess.Popen):
        def communicate(self, *args, **kwargs):
            if not getattr(self, "_interrupted", False):
                self._interrupted = True
                # Interrupt only once cb is up with its SIGTERM handler
                # installed, so the stop must escalate to the group SIGKILL.
                deadline = time.monotonic() + 10
                while not ready_file.exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                raise KeyboardInterrupt
            return super().communicate(*args, **kwargs)

    def popen_factory(*args, **kwargs):
        proc = InterruptOncePopen(*args, **kwargs)
        spawned.append(proc)
        return proc

    monkeypatch.setattr(
        rollout,
        "subprocess",
        SimpleNamespace(
            Popen=popen_factory,
            PIPE=subprocess.PIPE,
            TimeoutExpired=subprocess.TimeoutExpired,
        ),
    )
    with pytest.raises(KeyboardInterrupt):
        rollout._run_single_rollout("tasks/", "agent", "model", 5, extra_env=extra_env)
    assert ready_file.exists()  # the interrupt fired after cb was really up
    # cb ignores SIGTERM in sleep mode, so it is dead only if the stop
    # sequence escalated to the process-group SIGKILL.
    assert spawned[0].returncode == -signal.SIGKILL


def test_variant_timeout_is_forwarded_to_cb(tmp_path, monkeypatch, run_dir):
    argv_file = tmp_path / "argv.json"
    monkeypatch.setenv("FAKE_CB_ARGV_FILE", str(argv_file))
    extra_env = _fake_cb(tmp_path, monkeypatch)
    monkeypatch.setattr(rollout, "_all_sessions_terminal", lambda rid: True)
    rollout._run_single_rollout(
        "tasks/", "agent", "model", 5, variant_timeout=30, extra_env=extra_env
    )
    argv = json.loads(argv_file.read_text())
    assert argv[argv.index("--variant-timeout") + 1] == "30"


def test_a_clean_cb_run_polls_sessions_until_terminal(tmp_path, monkeypatch, run_dir):
    extra_env = _fake_cb(tmp_path, monkeypatch)
    polls = []

    def terminal(rid):
        polls.append(rid)
        return len(polls) >= 2

    monkeypatch.setattr(rollout, "_all_sessions_terminal", terminal)
    run_id, out = rollout._run_single_rollout(
        "tasks/", "agent", "model", 5, timeout=5, poll_interval=0.05, extra_env=extra_env
    )
    assert (run_id, out) == (RUN_ID, run_dir)
    assert polls == [RUN_ID, RUN_ID]
