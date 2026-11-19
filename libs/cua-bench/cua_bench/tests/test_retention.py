"""Results retention and the size warning (hermetic: temp dirs only)."""

import os
import time

import pytest
from cua_bench import retention
from cua_bench.retention import Retention


def make_run(root, name, mib, age_days, now):
    run = root / name
    run.mkdir(parents=True)
    (run / "summary.json").write_bytes(b"x" * (mib << 20))
    stamp = now - age_days * 86_400
    os.utime(run, (stamp, stamp))
    return run


def test_parse_and_format_sizes():
    assert retention.parse_size("20G") == 20 * 1024**3
    assert retention.parse_size("1.5t") == int(1.5 * 1024**4)
    assert retention.parse_size("512MiB") == 512 * 1024**2
    assert retention.parse_size("17") == 17
    with pytest.raises(ValueError):
        retention.parse_size("lots")
    assert retention.format_size(3 * 1024**3 / 2) == "1.5 GiB"


def test_nothing_is_deleted_without_explicit_retention(tmp_path):
    now = time.time()
    for i in range(5):
        make_run(tmp_path, f"r{i}", 1, 100 + i, now)
    assert not Retention().enabled
    assert retention.apply(tmp_path, Retention()) == []
    assert len(list(tmp_path.iterdir())) == 5


def test_keep_runs_age_and_size_delete_oldest_first(tmp_path):
    now = time.time()
    runs = [make_run(tmp_path, f"r{i}", 1, i, now) for i in range(5)]  # r0 newest
    assert retention.plan(tmp_path, Retention(keep_runs=3), now=now) == [runs[4], runs[3]]
    assert retention.plan(tmp_path, Retention(max_age_days=2.5), now=now) == [runs[4], runs[3]]
    # 5 MiB total, keep at most 2 MiB: the three oldest go.
    doomed = retention.plan(tmp_path, Retention(max_bytes=2 << 20), now=now)
    assert doomed == [runs[4], runs[3], runs[2]]


def test_the_current_run_and_live_runs_are_never_deleted(tmp_path):
    now = time.time()
    current = make_run(tmp_path, "current", 1, 50, now)
    live = make_run(tmp_path, "live", 1, 60, now)
    (live / "run.pid").write_text(str(os.getpid()))
    old = make_run(tmp_path, "old", 1, 70, now)
    done = retention.apply(tmp_path, Retention(keep_runs=0), protect=[current])
    assert done == [old]
    assert current.exists() and live.exists() and not old.exists()


def test_dry_run_deletes_nothing(tmp_path):
    now = time.time()
    make_run(tmp_path, "a", 1, 10, now)
    make_run(tmp_path, "b", 1, 20, now)
    assert len(retention.apply(tmp_path, Retention(keep_runs=0), dry_run=True)) == 2
    assert len(list(tmp_path.iterdir())) == 2


def test_env_and_flags_merge():
    env = Retention.from_env(
        {"CUA_BENCH_KEEP_RUNS": "10", "CUA_BENCH_MAX_RESULTS_SIZE": "5G"}
    )
    assert env == Retention(keep_runs=10, max_bytes=5 * 1024**3)
    merged = env.merged(Retention(keep_runs=3))
    assert merged == Retention(keep_runs=3, max_bytes=5 * 1024**3)


def test_size_warning(tmp_path, monkeypatch):
    now = time.time()
    make_run(tmp_path, "a", 2, 1, now)
    assert retention.size_warning(tmp_path, threshold=10 << 20) is None
    warning = retention.size_warning(tmp_path, threshold=1 << 20)
    assert warning and "cb prune --runs --keep" in warning
    monkeypatch.setenv("CUA_BENCH_WARN_SIZE", "0")
    assert retention.size_warning(tmp_path) is None
    monkeypatch.setenv("CUA_BENCH_WARN_SIZE", "1M")
    assert retention.size_warning(tmp_path) is not None


def test_interact_traces_are_bounded_and_stale_temp_dirs_go(tmp_path, monkeypatch):
    import tempfile

    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "data"))
    monkeypatch.setattr(tempfile, "tempdir", str(tmp_path / "tmp"))
    (tmp_path / "tmp").mkdir()
    stale = tmp_path / "tmp" / "cua_trace_old"
    fresh = tmp_path / "tmp" / "cua_trace_new"
    stale.mkdir()
    fresh.mkdir()
    old = time.time() - 3 * 86_400
    os.utime(stale, (old, old))
    made = []
    for i in range(retention.INTERACT_TRACES_KEPT + 3):
        d = retention.new_interact_trace_dir()
        stamp = time.time() - 1000 + i
        os.utime(d, (stamp, stamp))
        made.append(d)
    kept = retention.run_dirs(retention.interact_traces_dir())
    assert len(kept) == retention.INTERACT_TRACES_KEPT
    assert made[-1] in kept and made[0] not in kept
    assert not stale.exists() and fresh.exists()
