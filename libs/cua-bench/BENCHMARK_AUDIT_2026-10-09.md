# CUA Bench — 2026-10-09 engineering evidence

Project: `hippoley/cua` (upstream `trycua/cua`), **not OpenLEADR**.

## Baseline
- Relevant implementation: `libs/cua-bench/cua_bench/runners.py`, `run_benchmark`.
- Related tests: `libs/cua-bench/cua_bench/tests/test_run_benchmark.py`.
- Issue: `asyncio.Semaphore(max_parallel)` with zero permits never grants a worker slot when the dataset has runnable tasks, potentially hanging a benchmark indefinitely.

## Change and cross-story audit
- Validate `max_parallel`, `max_steps`, and optional `max_variants` as positive integers at the API boundary; disallow Python booleans (subclass of int) and floats/strings.
- Add negative tests using pytest parametrization for zero, negative, boolean and wrong-type inputs.
- Cross-story impact: valid positive integer inputs keep their previous behavior. No external adapter, fixture or dataset schema modified.
- Dataset/benchmark truthfulness: this prevents the runner from silently hanging due to invalid configuration. It does **not** demonstrate improved agent task success, episode replay, sandbox compatibility, oracle correctness or runtime reproducibility.

## Verification
- GitHub source fetch confirmed both changes on the feature branch.
- No repository-level pytest execution: execution container cannot resolve `github.com` for cloning and has no mounted CUA source. Absence of CI runs at commit inspection is not a pass.
- State: **CODE COMMITTED / TESTS NOT EXECUTED / NOT VERIFIED CLOSED**.

## Next execution entry
```sh
cd libs/cua-bench
uv run pytest cua_bench/tests/test_run_benchmark.py -q
```
Follow with positive real dataset runs on `cua-bench-basic`, a parallelism stress run, and evidence of termination and cleanup. Run from the branch `fix/cua-bench-validate-parallelism-20261009`.
