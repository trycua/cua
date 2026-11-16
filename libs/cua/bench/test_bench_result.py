"""Unit tests of the bench-result v1 converters and the `cua bench` runner,
on the committed baselines and runs (no Spaces):

    python3 -m unittest discover -s libs/cua/bench -p 'test_bench_result.py' -v
"""

from __future__ import annotations

import csv
import glob
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import bench_result as br  # noqa: E402
import cuabench  # noqa: E402

RUNS = os.path.join(HERE, "spaces-startup", "runs", "2026-10-20-m2max-96g")


def load(*p):
    with open(os.path.join(HERE, *p)) as f:
        return json.load(f)


META = dict(
    bench="x",
    slug="x",
    measured_at="2026-10-24T00:00:00Z",
    build={"cua": "0.0.0", "source": "test"},
    host={"name": "test"},
    environment={"kind": "vm"},
    method="m",
    reproduce="cua bench run x",
)


class Percentiles(unittest.TestCase):
    def test_interpolates_like_the_bench_summaries(self):
        self.assertEqual(br.pct([1, 2, 3, 4], 50), 2.5)
        self.assertEqual(br.pct([10], 95), 10.0)
        self.assertIsNone(br.pct([], 50))
        p = br.percentiles([5, 1, 3])
        self.assertEqual((p["min"], p["p50"], p["max"]), (1.0, 3.0, 5.0))


class VmDensity(unittest.TestCase):
    def test_committed_runs_match_their_summaries(self):
        base = load("spaces-startup", "baseline", "macos-m2max-96g.json")
        c, s = br.convert_vm_density(os.path.join(RUNS, "vm-density-after-cow-rollout.jsonl"))
        # The committed baseline is this run's summary.
        self.assertEqual(c["value"], base["max_running"])
        self.assertEqual(c["extra"]["per_vm_mb"], base["per_vm_mb"])
        self.assertEqual(c["extra"]["fit_64g"], base["fit_by_memory"]["64g"])
        self.assertEqual((c["metric"], c["unit"], c["stat"]), ("max_running_vms", "vms", "max"))
        self.assertEqual(c["n"], len(s))
        self.assertEqual(s[-1]["vms"], 97)
        self.assertTrue(all(isinstance(v, (int, float)) for v in c["extra"].values()))

    def test_every_committed_run_converts_or_says_why(self):
        got = {}
        for f in sorted(glob.glob(os.path.join(RUNS, "*.jsonl"))):
            try:
                got[os.path.basename(f)] = br.convert_vm_density(f)[0]["value"]
            except br.NoResult:
                got[os.path.basename(f)] = None
        self.assertEqual(
            got,
            {
                # An older format: probes only, no vm-density summary.
                "before-main-hqm.jsonl": None,
                "vm-density-after-cow-rollout-oc32.jsonl": 128,
                "vm-density-after-cow-rollout.jsonl": 97,
                "vm-density-after-published-rollout.jsonl": 66,
            },
        )


class Forks(unittest.TestCase):
    def test_fork_fanout_baseline_waves(self):
        h = load("fork-fanout", "baseline.json")["hosts"]["macos-hvf"]
        run = {
            "checkpoint_ms": h["checkpoint_ms"],
            "memory_mb": 2048,
            "dirty_mb": 300,
            "waves": [dict(w, forks=int(n)) for n, w in h["waves"].items()],
        }
        c, s = br.convert_fork_fanout(run)
        self.assertEqual((c["value"], c["n"]), (4351, 64))
        self.assertEqual(c["percentiles"], {"p50": 4351, "p95": 5180})
        self.assertEqual(c["extra"]["wall_s"], 5.52)
        self.assertEqual(c["extra"]["wall_s_8"], 0.47)
        self.assertEqual(c["extra"]["checkpoint_ms"], 607)
        self.assertEqual([r["forks"] for r in s], [8, 32, 64])

    def test_fork_fanout_without_waves_is_no_result(self):
        with self.assertRaises(br.NoResult):
            br.convert_fork_fanout({"checkpoint_ms": 600, "waves": []})

    def test_fork_batch_baseline_profiles(self):
        for name, prof in load("fork-batch", "baseline.json")["profiles"].items():
            c, s = br.convert_fork_batch(dict(prof, failed=0))
            self.assertEqual(c["value"], prof["fork_ms_p50"], name)
            self.assertEqual(c["n"], prof["ok"], name)
            self.assertEqual(c["extra"]["total_ms"], prof["total_ms"], name)

    def test_fork_batch_items_give_percentiles(self):
        items = [{"index": i, "host": "local", "fork_ms": ms, "elapsed_ms": ms + 5, "attempt": 1, "error": ""} for i, ms in enumerate([400, 500, 600, 700])]
        items.append({"index": 4, "host": "local", "fork_ms": None, "error": "boom", "error_code": "X"})
        c, s = br.convert_fork_batch({"ok": 4, "failed": 1, "items": items, "total_ms": 1200})
        self.assertEqual((c["value"], c["n"], c["extra"]["failed"]), (550.0, 4, 1))
        self.assertEqual(len(s), 5, "a failed fork stays in the samples")


class Channel(unittest.TestCase):
    def test_committed_baselines(self):
        for f in sorted(glob.glob(os.path.join(HERE, "channel", "baselines", "*.json"))):
            with open(f) as fh:
                doc = json.load(fh)
            # Baselines keep percentiles, not n: they ran channel_bench's default ITERS.
            c, s = br.convert_channel(dict(doc, iters=300))
            self.assertEqual(c["value"], doc["metrics"]["exec_run_ms"]["p50"], f)
            self.assertEqual(c["percentiles"].get("p95"), doc["metrics"]["exec_run_ms"].get("p95"), f)
            self.assertEqual(c["extra"]["rpc_health_p50_ms"], doc["metrics"]["rpc_health_ms"]["p50"], f)
            self.assertIn("stream_stdout_MiBps", c["extra"], f)
            self.assertEqual(len(s), len(doc["metrics"]), f)

    def test_summary_line_shape(self):
        line = {"summary": True, "results": [{"metric": "exec_run_ms", "n": 300, "p50": 1.2, "p95": 1.4, "p99": 2.0, "max": 3.0}, {"metric": "upload_1MiB_ms", "n": 20, "p50": 4.0, "MiBps_p50": 250.0}]}
        c, _ = br.convert_channel(line)
        self.assertEqual((c["value"], c["n"]), (1.2, 300))
        self.assertEqual(c["extra"], {"upload_1MiB_p50_ms": 4.0, "upload_1MiB_MiBps": 250.0})


class Rollouts(unittest.TestCase):
    def test_guard_without_samples_is_no_result(self):
        # A guard.json alone has p50s but no samples: never a result.
        with self.assertRaises(br.NoResult):
            br.convert_rollouts(load("rollouts", "baseline", "macos-m2max-96g.json"), [])

    def test_guard_with_raw_lines(self):
        guard = load("rollouts", "baseline", "macos-m2max-96g.json")
        raw = [{"cmd": "create", "label": "headless-serial", "i": i, "create_ms": 200 + i, "ready_ms": 230 + i, "ok": True} for i in range(10)]
        raw.append({"cmd": "create", "label": "headless-serial", "summary": True, "p50": 234.5})
        c, s = br.convert_rollouts(guard, raw)
        self.assertEqual((c["value"], c["n"]), (234.5, 10))
        self.assertEqual(c["extra"]["rollout_steps_per_sec"], guard["rollout_steps_per_sec"])
        self.assertNotIn("_host", c["extra"])


class PoolSessions(unittest.TestCase):
    def test_jobs(self):
        jobs = [{"id": i, "state": "succeeded", "node": "a" if i % 2 else "b", "create_ms": 300 + i, "run_ms": 60000, "delete_ms": 90, "finished_ms": 1000 * i} for i in range(1, 5)]
        jobs.append({"id": 9, "state": "failed", "error": "x"})
        c, s = br.convert_pool_sessions(jobs)
        self.assertEqual((c["value"], c["n"], c["extra"]["failed"], c["extra"]["computers"]), (302.5, 4, 1, 2))
        self.assertEqual(len(s), 5)


class Phases(unittest.TestCase):
    def test_microvm_fork_committed_run(self):
        raw = os.path.join(HERE, "microvm-fork", "runs", "2026-10-24-m2max-96g")
        c, s = br.convert_microvm_fork(raw)
        summary = load("microvm-fork", "runs", "2026-10-24-m2max-96g", "summary.json")
        self.assertEqual(c["value"], summary["restore"]["p50_ms"])
        self.assertEqual(c["n"], summary["restore"]["ok"])
        self.assertEqual(c["extra"]["checkpoint_p50_ms"], summary["checkpoint"]["p50_ms"])
        self.assertEqual(c["extra"]["per_fork_mib"], summary["memory"]["per_fork_mib"])

    def test_failed_samples_are_counted_not_measured(self):
        phases = {"gvisor-start": [{"name": "a", "ms": 300, "rc": 0}, {"name": "b", "ms": 9000, "rc": 1}, {"name": "c", "ms": 320, "rc": 0}]}
        c, s = br.convert_phases(phases, "gvisor-start", "create_to_ready_ms", {})
        self.assertEqual((c["value"], c["n"], c["extra"]["failed"]), (310.0, 2, 1))
        with self.assertRaises(br.NoResult):
            br.convert_phases({"gvisor-start": [{"ms": 1, "rc": 1}]}, "gvisor-start", "m", {})


class Writing(unittest.TestCase):
    def test_finish_write_validate(self):
        c, s = br.convert_vm_density(os.path.join(RUNS, "vm-density-after-cow-rollout.jsonl"))
        res = br.finish(c, **META)
        self.assertEqual(list(res), list(br.FIELDS))
        self.assertEqual(br.validate(res), [])
        with tempfile.TemporaryDirectory() as d:
            br.write(d, res, s)
            with open(os.path.join(d, "result.json")) as f:
                back = json.load(f)
            self.assertEqual(back["schema"], "cua.bench-result/v1")
            with open(os.path.join(d, "samples.csv")) as f:
                rows = list(csv.reader(f))
            self.assertEqual(rows[0][0], "i")
            self.assertEqual(len(rows) - 1, len(s))

    def test_validate_rejects_unmeasured(self):
        res = br.finish(br.core("m", "ms", "p50", 1.0, 1, {}, {}), **META)
        self.assertEqual(br.validate(dict(res, build={"cua": "1"})), ["build.source"])
        self.assertIn("extra.x is not a number", br.validate(dict(res, extra={"x": "a"})))
        with self.assertRaises(br.NoResult):
            br.core("m", "ms", "p50", None, 3, {}, {})


class RepoClone(unittest.TestCase):
    RUN = {
        "schema": "cua.repo-clone/v1",
        "repos": {
            "small": {
                "url": "small",
                "cases": {
                    "off": {"failed": 0, "samples": [{"ok": True, "clone_ms": 1000.0, "total_ms": 1010.0}, {"ok": True, "clone_ms": 1200.0, "total_ms": 1210.0}], "stats": {"clone_ms": {"p50": 1100.0}, "total_ms": {"p50": 1110.0}}},
                    "hit": {"failed": 1, "samples": [{"ok": True, "clone_ms": 300.0, "total_ms": 320.0}, {"ok": False, "error": "head"}], "stats": {"clone_ms": {"p50": 300.0}, "total_ms": {"p50": 320.0}}},
                    "hit_lease": {"failed": 0, "samples": [{"ok": True, "clone_ms": 200.0, "total_ms": 210.0}], "stats": {"clone_ms": {"p50": 200.0}, "total_ms": {"p50": 210.0}}},
                },
                "canary": {"ran": True, "ok": True},
            }
        },
    }

    def test_the_value_is_the_first_hit_cases_clone_and_failures_are_counted_not_measured(self):
        c, rows = br.convert_repo_clone(self.RUN)
        self.assertEqual((c["metric"], c["value"], c["n"]), ("clone_ms", 300.0, 1))
        self.assertEqual(c["extra"]["failed"], 1)
        self.assertEqual(c["extra"]["small_off_clone_p50_ms"], 1100.0)
        self.assertEqual(c["extra"]["small_canary_ok"], 1)
        self.assertEqual(len(rows), 5)
        self.assertEqual([r["ok"] for r in rows if r["case"] == "hit"], [True, False])

    def test_no_measured_clone_is_no_result(self):
        bad = {"repos": {"small": {"cases": {"hit": {"failed": 2, "samples": [{"ok": False}, {"ok": False}], "stats": {}}}}}}
        with self.assertRaises(br.NoResult):
            br.convert_repo_clone(bad)

    def test_the_bench_stats_are_the_same_percentiles(self):
        sys.path.insert(0, os.path.join(HERE, "repo-clone"))
        import importlib

        rc = importlib.import_module("bench")
        self.assertEqual(rc.stats([1.0, 2.0, 3.0, 4.0]), {"p50": 2.5, "p95": 3.9, "min": 1.0, "max": 4.0})
        self.assertEqual(rc.stats([]), {})
        self.assertEqual(rc.CASES, ("off", "miss", "hit", "hit_lease"))


class Runner(unittest.TestCase):
    def test_every_bench_has_a_converter_and_its_files(self):
        for b in cuabench.BENCHES:
            self.assertIn(b.name, br.CONVERTERS, b.name)
        for f in cuabench.manifest():
            self.assertTrue(os.path.isfile(os.path.join(HERE, f)), f)

    def test_guards_name_existing_checkers(self):
        for b in cuabench.BENCHES:
            if b.guard:
                self.assertTrue(os.path.isfile(os.path.join(HERE, b.guard.split()[0])), b.name)

    def test_shipped_files_name_only_our_product(self):
        # `cua bench` embeds these files: no other sandbox product's name.
        bad = re.compile(r"\b(e2b|daytona|vercel|modal sandboxes?|runloop|codesandbox)\b", re.I)
        for f in cuabench.manifest():
            with open(os.path.join(HERE, f)) as fh:
                text = fh.read()
            self.assertIsNone(bad.search(text), f"{f}: {bad.search(text)}")

    def test_list_json(self):
        out = subprocess.run([sys.executable, os.path.join(HERE, "cuabench.py"), "list", "--json"], capture_output=True, text=True, check=True).stdout
        rows = json.loads(out)
        self.assertEqual([r["name"] for r in rows], [b.name for b in cuabench.BENCHES])

    def test_run_refuses_without_its_needs(self):
        env = dict(os.environ, CUA=sys.executable)
        env.pop("CHANNEL_BENCH", None)
        p = subprocess.run([sys.executable, os.path.join(HERE, "cuabench.py"), "run", "nope"], capture_output=True, text=True, env=env)
        self.assertEqual(p.returncode, 2)
        self.assertIn("no bench", p.stderr)

    def test_check_refuses_a_bench_without_a_guard(self):
        with tempfile.TemporaryDirectory() as d:
            res = br.finish(br.core("m", "ms", "p50", 1.0, 1, {}, {}), **dict(META, bench="spaces-startup.gvisor"))
            br.write(d, res, [{"i": 1}])
            p = subprocess.run([sys.executable, os.path.join(HERE, "cuabench.py"), "check", os.path.join(d, "result.json"), "--baseline", "rollouts/check.py"], capture_output=True, text=True)
            self.assertEqual(p.returncode, 2)
            self.assertIn("no perf guard", p.stderr)


if __name__ == "__main__":
    unittest.main()
