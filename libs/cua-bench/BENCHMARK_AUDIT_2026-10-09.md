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

## Continuation: dataset discovery integrity
- **P0 failure mechanism:** `run_benchmark` previously swallowed all task-config exceptions and replaced the true variant count with `1`; an empty `tasks_config_fn()` produced zero runs and misleading empty metrics. Both violate truthful dataset denominators.
- **Code fix:** `runners.py` raises an informative ValueError containing task path and split on configuration errors and rejects zero variants, before scheduling any workers. Commit `55999795483d944a9f14635f59d3afaad5c3554a`.
- **Counterexamples authored:** `test_run_benchmark.py` creates a task whose config raises `RuntimeError` and another which returns zero tasks; both must now raise ValueError rather than fabricate results. Commit `ff1e8009e61b8d58d943f0bd73381c59b002d566`.
- **Status:** source committed and remote source verified; no pytest or end-to-end execution result witnessed in this round. Therefore NOT Verified Closed.
- **HCA:** function P, state P, integration B, security N/A (no new credentials/privileges), scale B, maintainability P, observation P, testability P, user value B, external compatibility N/A (pure benchmark orchestration). To close: run relevant pytest on actual branch, ensure valid dataset cases still pass, inspect run counts before and after, and run actual simulated provider end-to-end.

## Score and aggregation integrity follow-up (2026-10-09)

- **P0 score integrity:** `run_single_task` now rejects unsupported evaluator outputs, nonfinite values and rewards outside the normalized [0,1] contract. Missing evaluators no longer return a nominal score without an explicit error. Commit `24e815587c08a9a52014fa4bc5c5d5219db3493b`.
- **P0 failure provenance:** `run_benchmark` retains task path and variant ID for gathered worker exceptions, while accounting for failure in total_tasks and mean reward denominator. Commit `24e815587c08a9a52014fa4bc5c5d5219db3493b`.
- **Adversarial tests added:** malformed evaluator output (NaN, Infinity, negative, >1, nonnumeric), missing evaluator and two-task exception aggregation retaining both run identities and expected 1/2 success fraction. Commits `558d201661f9a7dfcf92838103f8c2e35eea9567`, `edd98cfbadfdb4fc82cb736d23799a2b4691d14b`.
- **Evidence status:** remote GitHub fetch confirms both code and tests. No local pytest execution or browser/sandbox E2E was witnessed; no User Story may be called Verified Closed.
- **HCA:** functional P; task/result state P; integration B; security NA (no new trust boundary); performance B; maintainability P; traceability P; testability P; end-user value B; external compatibility B. Challenge the normalized [0,1] reward assumption against each existing dataset/oracle before treating it as ecosystem-compatible. Regression suite and live sandbox tests remain mandatory.

## Follow-up — no-agent false-positive guard (2026-10-09)
- **P0 finding:** `run_single_task(oracle=False, agent_fn=None)` previously reset the environment, skipped agent actions, then evaluated. A pre-satisfied environment could be reported as successful despite zero agent actions.
- **Code mitigation:** explicit failure `No agent_fn supplied; task was not executed`, with reward 0 and success False; no evaluator is invoked on this path. Commit `614e8ad561583e14c99f8f28107c4008398c1aab`.
- **Independent counterexample:** mock evaluator that raises if called, confirming absence of agent actions never earns score. Commit `6c3e54561474b67572f5598e410f3b008705bf42`.
- **Verification state:** remote code and tests present. Runtime container has Python and pytest but not full CUA repository sources at the relevant branch, so repository-level pytest and real simulated-provider tests were NOT executed. No Verified Closed.
- **Next P0:** execute `cd libs/cua-bench && uv run pytest cua_bench/tests/test_run_benchmark.py -q` on this branch; verify no existing legitimate no-agent setup-only callers rely on scoring; run an independent simulated provider case with initial evaluator reward 1.0 but no actions and assert failure.
