"""Shard planning for the cua SDK e2e suite (stdlib only).

python3 -m unittest tests/e2e/cua-sdk/test_sharding.py
"""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load(name: str):
    spec = importlib.util.spec_from_file_location(f"e2e_{name}", HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    import sys

    sys.path.insert(0, str(HERE))
    spec.loader.exec_module(module)
    return module


sharding = _load("sharding")
run = _load("run")
SHARDS = 4  # the E2E_SHARDS of .github/workflows/e2e-cua-sdk.yml


def known_units(timings: dict) -> list[tuple[str, str]]:
    units = [("py", f"test_x.py::{name}") for name in timings["py"]]
    units += [("ts", name) for name in timings["ts"]]
    units += [("rust", name) for name in timings["rust"]]
    return units


class Assign(unittest.TestCase):
    timings = sharding.load_timings()

    def test_every_unit_runs_on_exactly_one_shard(self) -> None:
        units = known_units(self.timings) + [("py", "test_new.py::test_added_later")]
        shards = sharding.assign(units, SHARDS, self.timings)
        flat = [u for shard in shards for u in shard]
        self.assertEqual(sorted(flat), sorted(set(units)))
        self.assertEqual(len(flat), len(set(flat)))

    def test_the_plan_is_deterministic_whatever_the_collection_order(self) -> None:
        units = known_units(self.timings)
        self.assertEqual(
            sharding.assign(units, SHARDS, self.timings),
            sharding.assign(list(reversed(units)), SHARDS, self.timings),
        )

    def test_shards_finish_within_ten_percent(self) -> None:
        shards = sharding.assign(known_units(self.timings), SHARDS, self.timings)
        loads = [sharding.predicted(s, self.timings) for s in shards]
        self.assertLess(sharding.spread(loads), 10.0, loads)

    def test_a_unit_without_a_timing_gets_the_default_weight(self) -> None:
        self.assertEqual(
            sharding.weight(self.timings, "py", "test_new.py::test_added_later"),
            self.timings["default"]["py"],
        )
        self.assertEqual(
            sharding.weight(self.timings, "ts", "new.test.mjs"), self.timings["default"]["ts"]
        )

    def test_py_units_are_weighed_by_test_name(self) -> None:
        name = next(k for k, v in self.timings["py"].items() if v > 10)
        self.assertEqual(
            sharding.weight(self.timings, "py", f"python/test_a.py::{name}"),
            self.timings["py"][name],
        )

    def test_more_shards_than_units_leaves_some_empty(self) -> None:
        shards = sharding.assign([("ts", "a.test.mjs")], 3, {})
        self.assertEqual(sum(1 for s in shards if s), 1)

    def test_shard_spec(self) -> None:
        self.assertEqual(sharding.parse_shard("2/4"), (2, 4))
        for bad in ("0/4", "5/4", "x", "1/0", "2"):
            with self.assertRaises(SystemExit):
                sharding.parse_shard(bad)


class Measured(unittest.TestCase):
    def test_timings_come_from_results_and_shard_files(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            a, b = Path(tmp, "a"), Path(tmp, "b")
            for d, secs in ((a, 4.0), (b, 6.0)):
                d.mkdir()
                (d / "py.jsonl").write_text(json.dumps({"test": "test_one", "secs": secs}) + "\n")
                (d / "shard.json").write_text(
                    json.dumps({"unit_secs": {"ts:core.test.mjs": secs * 2}})
                )
            t = sharding.build_timings([a, b])
        self.assertEqual(t["py"], {"test_one": 5.0})
        self.assertEqual(t["ts"], {"core.test.mjs": 10.0})
        self.assertEqual(t["rust"], {})
        self.assertEqual(t["default"]["py"], 5.0)


class Workflow(unittest.TestCase):
    def test_the_shard_matrix_matches_the_shard_count(self) -> None:
        import re

        text = (HERE.parents[2] / ".github" / "workflows" / "e2e-cua-sdk.yml").read_text()
        count = int(re.search(r'^  E2E_SHARDS: "(\d+)"', text, re.M).group(1))
        matrix = re.search(r"^\s+shard: \[([\d, ]+)\]", text, re.M).group(1)
        self.assertEqual([int(x) for x in matrix.split(",")], list(range(1, count + 1)))
        self.assertEqual(count, SHARDS)


class Coverage(unittest.TestCase):
    @staticmethod
    def meta(shard: int, units: dict, total: int = 4, of: int = 2) -> dict:
        return {"shard": shard, "of": of, "total_units": total, "units": units}

    def test_complete_shards_pass(self) -> None:
        metas = [
            self.meta(1, {"py": ["a", "b"], "ts": [], "rust": []}),
            self.meta(2, {"py": ["c"], "ts": ["x"], "rust": []}),
        ]
        self.assertEqual(sharding.coverage_problems(metas, 2), [])

    def test_a_missing_shard_is_reported(self) -> None:
        metas = [self.meta(1, {"py": ["a", "b", "c", "d"], "ts": [], "rust": []})]
        self.assertTrue(sharding.coverage_problems(metas, 2))

    def test_a_dropped_or_doubled_unit_is_reported(self) -> None:
        dropped = [
            self.meta(1, {"py": ["a", "b"], "ts": [], "rust": []}),
            self.meta(2, {"py": ["c"], "ts": [], "rust": []}),
        ]
        doubled = [
            self.meta(1, {"py": ["a", "b"], "ts": [], "rust": []}),
            self.meta(2, {"py": ["b", "c"], "ts": [], "rust": []}),
        ]
        self.assertTrue(sharding.coverage_problems(dropped, 2))
        self.assertTrue(sharding.coverage_problems(doubled, 2))


class RunnerChecks(unittest.TestCase):
    def test_a_shard_flags_unexpected_skips_but_not_allowed_ones(self) -> None:
        rows = [
            {
                "scenario": "s",
                "lane": "container",
                "lang": "py",
                "status": "skip",
                "reason": "docker is not on PATH",
            },
            {
                "scenario": "s",
                "lane": "hermetic",
                "lang": "ts",
                "status": "skip",
                "reason": "playwright is not installed",
            },
            {
                "scenario": "s",
                "lane": "qemu",
                "lang": "py",
                "status": "skip",
                "reason": "not requested",
            },
        ]
        problems = run.skip_problems(rows, ["hermetic", "container"])
        self.assertEqual(len(problems), 1)
        self.assertIn("docker is not on PATH", problems[0])

    def test_a_relative_rust_prebuilt_runs_from_the_invocation_directory(self) -> None:
        """The workflow passes `--rust-prebuilt tests/e2e/cua-sdk/rust/prebuilt` from the
        repository root; the binaries run with the Rust half's directory as cwd."""
        import os
        import subprocess
        import sys

        rel = "tests/e2e/cua-sdk/rust/prebuilt"
        with tempfile.TemporaryDirectory() as tmp:
            prebuilt = Path(tmp, rel)
            prebuilt.mkdir(parents=True)
            fake = prebuilt / "guides"
            fake.write_text('#!/bin/sh\necho "ran $0 $@" >"$CUA_E2E_RESULTS/ran.txt"\n')
            fake.chmod(0o755)
            (prebuilt / "manifest.json").write_text(
                json.dumps([{"name": "guides", "file": "guides"}])
            )
            proc = subprocess.run(
                [
                    sys.executable,
                    str(HERE / "run.py"),
                    "--lanes",
                    "hermetic",
                    "--langs",
                    "rust",
                    "--shard",
                    "1/1",
                    "--rust-prebuilt",
                    rel,
                    "--out",
                    "out",
                ],
                cwd=tmp,
                env=dict(os.environ, CUA_E2E_RUN="gh123456-1"),
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            ran = Path(tmp, "out", "ran.txt").read_text()
        self.assertTrue(ran.strip().endswith(f"{rel}/guides --test-threads=2"), ran)
        self.assertNotIn("rust/tests/e2e", ran)

    def test_prebuilt_rust_binaries_are_listed_by_the_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "manifest.json").write_text(json.dumps([{"name": "core", "file": "core"}]))
            self.assertEqual(run.rust_binaries(Path(tmp)), {"core": "core"})
            with self.assertRaises(SystemExit):
                run.rust_binaries(Path(tmp, "missing"))


if __name__ == "__main__":
    unittest.main()
