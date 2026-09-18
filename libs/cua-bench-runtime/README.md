# Cua Bench Runtime

Cua Bench Runtime executes task setup, agent work, target-state collection,
evaluation, cleanup, and result publication. It records immutable inputs and a
hash-chained lifecycle event log in each trial directory.

The Python package exposes the `cdb` command. It requires Python 3.11 or newer.

## Documentation

| Need | Document |
| --- | --- |
| Learn the lifecycle with a small trial | [Your first verified trial](../../benchmarks/cua-driver-bench/docs/tutorials/your-first-verified-trial.md) |
| Run and verify a task | [Run and verify a trial](../../benchmarks/cua-driver-bench/docs/how-to/run-and-verify-a-trial.md) |
| Export trials and build reports | [Export and report results](../../benchmarks/cua-driver-bench/docs/how-to/export-and-report-results.md) |
| Look up commands | [`cdb` command reference](../../benchmarks/cua-driver-bench/docs/reference/cli.md) |
| Look up trial files and decisions | [Trial directory reference](../../benchmarks/cua-driver-bench/docs/reference/trial-artifacts.md) |
| Understand the decision model | [Outcome, participation, certification, and comparison](../../benchmarks/cua-driver-bench/docs/explanation/outcome-participation-certification-and-comparison.md) |

## Source layout

| Path | Purpose |
| --- | --- |
| `src/cua_bench_runtime/engine.py` | Lifecycle orchestration |
| `src/cua_bench_runtime/adapters/` | Local, Cua Driver, Lume, and production harness adapters |
| `src/cua_bench_runtime/explain.py` | Trial integrity verification and explanation |
| `src/cua_bench_runtime/export_trial.py` | Verified v0.3 trial export |
| `src/cua_bench_runtime/report.py` | Deterministic report views |
| `tests/` | Unit, contract, and adversarial runtime coverage |

The [component map](../../benchmarks/cua-driver-bench/docs/reference/component-map.md) describes the boundary
between the runtime, tasks, schemas, profiles, and conformance suite.
