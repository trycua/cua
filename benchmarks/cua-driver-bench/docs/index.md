# Cua Driver Bench documentation

Choose documentation by the kind of help you need. Tutorials teach through a
guided run. How-to guides solve a specific operation. Reference pages describe
the interfaces. Explanation pages discuss the design and tradeoffs of the benchmark.

## Tutorials

- [Run your first verified synthetic trial](tutorials/your-first-verified-trial.md)

## How-to guides

- [Run and verify an authorized task](how-to/run-and-verify-a-trial.md)
- [Compare local Cua Driver releases](how-to/compare-driver-releases.md)
- [Export trials and build reports](how-to/export-and-report-results.md)
- [Diagnose a protected trial timeout](how-to/diagnose-a-protected-trial-timeout.md)

## Reference

- [`cdb` command reference](reference/cli.md)
- [Component map](reference/component-map.md)
- [Task-pack interface](reference/task-pack-interface.md)
- [Manifest reference](reference/manifests.md)
- [Comparison views](reference/comparison-views.md)
- [Protected debug mode](reference/protected-debug-mode.md)
- [Trial artifacts](reference/trial-artifacts.md)

## Explanation

- [Benchmark design](explanation/benchmark-design.md)
- [Execution boundary](execution-boundary.md)
- [Outcome, participation, certification, and comparison](explanation/outcome-participation-certification-and-comparison.md)
- [Profiles, systems, and comparisons](explanation/profiles-systems-and-comparisons.md)
- [Task-difficulty calibration](explanation/task-difficulty-calibration.md)

## Decisions and provenance

- [Public monorepo boundary](decisions/0001-public-monorepo-boundary.md)
- [Snapshot provenance](../PROVENANCE.md)

## Active benchmark paths

- Runs write results under `artifacts/automated-eval/` unless you pass `--output`.
- Task shards can run in parallel, but each shard runs its baseline before its candidate.
- Execution defaults to one shard at a time. Parallel execution requires one prestarted Linux/X11 display for each active shard.
- The manual GitHub Actions workflow downloads a driver release, retrieves selected private tasks, runs the local runner on a runner you choose, and preserves the result bundle.
- S3 publishing is opt-in. It uploads the static report only when you pass `--publish` or run `publish`. The GitHub Actions artifact is always available.
