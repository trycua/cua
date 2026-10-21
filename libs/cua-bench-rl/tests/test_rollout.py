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
import stat
import time
from pathlib import Path

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
import json, os, signal, sys, time

print("Run ID: deadbeef1234", flush=True)
argv_file = os.environ.get("FAKE_CB_ARGV_FILE")
if argv_file:
    with open(argv_file, "w") as f:
        json.dump(sys.argv, f)
mode = os.environ.get("FAKE_CB_MODE", "ok")
if mode == "fail":
    sys.exit(3)
if mode == "sleep":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
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
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
    path = tmp_path / "cua-bench" / "runs" / RUN_ID
    path.mkdir(parents=True)
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
        "tasks/", "agent", "model", 5, run_timeout=0.5, extra_env=extra_env
    )
    assert (run_id, out) == (RUN_ID, run_dir)
    # SIGTERM (ignored) -> SIGKILL after the grace period; no poll, no 300 s wait.
    assert time.monotonic() - started < 10


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
