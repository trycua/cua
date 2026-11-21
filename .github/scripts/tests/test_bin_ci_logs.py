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


def _state(mode, **kw):
    base = {"head": "a" * 40, "clean": True, "mode": mode, "seconds": 1.0, "passed": ["spdx"], "failed": [], "skipped": [], "deferred": []}
    base.update(kw)
    return base


def _run_signoff(ci, monkeypatch, state):
    posted = []
    monkeypatch.setattr(ci, "github_repo", lambda: "trycua/cua")
    monkeypatch.setattr(ci, "have", lambda tool: True)

    class Done:
        returncode, stdout, stderr = 0, "", ""

    monkeypatch.setattr(ci.subprocess, "run", lambda cmd, **kw: posted.append(cmd) or Done())
    return ci.signoff(state), posted


def test_quick_signoff_allows_the_build_tier_left_to_ci(monkeypatch):
    ci = load_bin_ci()
    rc, posted = _run_signoff(ci, monkeypatch, _state("quick", deferred=["sdk-bindings"]))
    assert rc == 0 and posted
    assert any("1 build check(s) left to CI" in part for part in posted[0])


def test_signoff_still_refuses_a_check_skipped_for_a_missing_tool(monkeypatch):
    ci = load_bin_ci()
    for mode in ("quick", "affected", "full"):
        rc, posted = _run_signoff(ci, monkeypatch, _state(mode, skipped=["image-refs"], deferred=["sdk-bindings"]))
        assert rc == 1 and not posted


def test_full_mode_never_defers_and_a_failure_still_blocks(monkeypatch):
    ci = load_bin_ci()
    assert _run_signoff(ci, monkeypatch, _state("full", failed=["x"]))[0] == 1
    assert _run_signoff(ci, monkeypatch, _state("full"))[0] == 0


def test_quick_marks_build_tier_checks_as_profile_skips_and_keeps_them_out_of_skipped(monkeypatch):
    ci = load_bin_ci()
    build = ci.Check("sdk-bindings", "t", "true", tier="build")
    quick = ci.Check("spdx", "t", "true")
    monkeypatch.setattr(ci, "state_dir", lambda: Path(os.environ["TMPDIR_FOR_TEST"]))
    monkeypatch.setattr(ci, "git", lambda *a: "a" * 40 if a[0] == "rev-parse" else "")
    # Mirror build_plan's --quick handling through the real function on a two-check plan.
    class Args:
        quick, only = True, None
    class Ctx:
        args = Args()
    monkeypatch.setattr(ci, "static_checks", lambda changes, ctx: [build, quick])
    monkeypatch.setattr(ci, "rust_checks", lambda *a: [])
    monkeypatch.setattr(ci, "python_checks", lambda *a: [])
    monkeypatch.setattr(ci, "web_checks", lambda *a: [])
    plan = ci.build_plan(None, Ctx())
    assert build.skip_kind == "profile" and build.skip_reason == "--quick" and quick.skip_reason is None
    results = [ci.Result(build, "skip", detail="--quick"), ci.Result(quick, "pass")]
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        os.environ["TMPDIR_FOR_TEST"] = d
        state = ci.save_state(results, "quick", 1.0)
    assert state["skipped"] == [] and state["deferred"] == ["sdk-bindings"]
