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
- [Fleet execution boundary](fleet-boundary.md)
- [Outcome, participation, certification, and comparison](explanation/outcome-participation-certification-and-comparison.md)
- [Profiles, systems, and comparisons](explanation/profiles-systems-and-comparisons.md)
- [Task-difficulty calibration](explanation/task-difficulty-calibration.md)

## Decisions and provenance

- [Public monorepo boundary](decisions/0001-public-monorepo-boundary.md)
- [Snapshot provenance](../PROVENANCE.md)

## Active benchmark paths

- Local comparisons write results under `artifacts/automated-eval/`.
- Fleet comparisons use isolated Linux/X11 workers and write results under `automated-eval/fleet-results/`.
- Task shards can run in parallel, but each shard runs its baseline before its candidate.
- Fleet defaults to two active task shards. Local execution defaults to one.
- Parallel local execution requires one prestarted Linux/X11 display for each active shard.
- The manual GitHub Actions workflow builds driver refs, retrieves selected private tasks, runs Fleet, and preserves the result bundle.
- S3 publishing uploads the static report. The GitHub Actions artifact remains the fallback for public-access failures.
