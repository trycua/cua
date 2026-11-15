"""bin/ci must survive another run removing its logs (a vanished log file crashed the report at default parallelism)."""
import importlib.machinery
import importlib.util
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def load_bin_ci():
    loader = importlib.machinery.SourceFileLoader("bin_ci_under_test", str(ROOT / "bin/ci"))
    spec = importlib.util.spec_from_loader("bin_ci_under_test", loader)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["bin_ci_under_test"] = mod
    loader.exec_module(mod)
    return mod


def test_report_helpers_tolerate_a_vanished_log(tmp_path):
    ci = load_bin_ci()
    gone = tmp_path / "gone.log"
    assert ci.read_log(gone) == ""
    assert ci.excerpt(gone) == []


def test_each_run_gets_its_own_log_dir_and_keeps_a_concurrent_runs_logs(tmp_path):
    ci = load_bin_ci()
    first = ci.new_run_logdir(tmp_path)
    (first / "a.log").write_text("x")
    second = ci.new_run_logdir(tmp_path)
    assert first.exists() and second.exists()
    assert (first / "a.log").read_text() == "x"


def test_old_run_dirs_and_legacy_logs_are_pruned(tmp_path):
    ci = load_bin_ci()
    old = tmp_path / "run-1-1"
    old.mkdir()
    (old / "a.log").write_text("x")
    past = time.time() - 24 * 3600
    os.utime(old, (past, past))
    legacy = tmp_path / "legacy.log"
    legacy.write_text("x")
    ci.new_run_logdir(tmp_path)
    assert not old.exists()
    assert not legacy.exists()
