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

## Follow-up: worker-output integrity and benchmark denominator (2026-10-09)
- P0: `run_benchmark` now validates each returned worker result against the scheduled task path and variant ID, requires finite normalized reward, sane step count, coherent success/reward semantics, and zero reward on worker errors. Invalid results become attributable failures with reward 0 rather than polluting success rate. Commits `34772d45bfe89e133fb766c46e6e3c931eb70210`, `4b46ac851ecc0c39aa4e19c6ac0f469ab000e717`.
- Counterexamples added for forged path, forged variant, NaN reward and contradictory success/reward: commit `fbf83e1aa44660a5bf4b7f2cecb2711dcdf74fec`.
- Source changes committed; no fresh pytest/CI execution or real sandbox E2E evidenced. Validation requires running `uv run pytest cua_bench/tests/test_run_benchmark.py -q` from `libs/cua-bench`, followed by dataset-level evaluation and independent oracle replay.
- Horizontal status: Function P, state P, integration B, security P, performance B, maintainability P, observability P, testability P, user value B, external compatibility B. No Verified Closed.

## Live CI evidence 2026-10-09
- GitHub run 37908647343 passed prior build-only workflow, not pytest.
- Run 37908806518: full-suite collection FAILED (3 errors importing optional `torch` from worker dataloader). No test pass may be inferred from collection failure.
- Run 37908930187: core suite produced **79 passed, 11 failed**; all 11 browser failures cited missing Chromium/Playwright executables. CI installed browser dependencies subsequently.
- Run 37909070828: core suite produced **89 passed, 1 failed**; only headed-browser interactive E2E failed because runner lacked X Server. Browser/Oracle noninteractive tests executed. Commit 55203190fe26282224a417dd22960de3c077f23e uses `xvfb-run -a` for headed test, with run 37909217474 queued/in progress when recorded. Do not treat it as passed until reviewed.
- Local offline CUA contract and Outcome Witness standalone bundles: **27 passed in 0.06s**; unrelated to full `cua_bench/tests` run and not interchangeable evidence.
- Still OPEN P0: full RL worker suite must run with optional `rl` dependencies and survive real execution; fault injection proving process/resource cleanup in dedicated sandbox; independent Oracle provenance and action/outcome witness against actual scene. No Verified Closed issued.

## Infrastructure thesis and stop/go gates — 2026-10-09

**Current hypothesis, not an adoption claim:** Build a reusable, provider-neutral CUA Execution Witness over existing agent action traces and evaluator outputs. Do not create a separate leaderboard or a competing runtime until direct users demonstrate a missing interface. OSWorld-V2.1's pinning of task/assets/website/provider images is a relevant mature precedent (https://github.com/xlang-ai/OSWorld-V2); match its provenance discipline, not copy its benchmark dataset.

### Audit of traceable vertical-to-horizontal gaps
| Existing requirement path | P0 defect/gap | Horizontal impact | Mandatory independent receipt |
| --- | --- | --- | --- |
| Task discovery -> scheduling | exceptions, zero variants and invalid concurrency were previously silent/hanging | denominator and task-identity errors | executed pytest plus dataset manifest/hash |
| Action -> environment | no-agent scoring risk | false positive independent of actual action | action count and environment before/after evidence |
| Environment -> oracle | nonfinite scores, unversioned evaluator | score provenance cannot be independently repeated | oracle ID/version, inputs and independent rerun |
| Worker -> aggregation | worker identity mismatch and dropped exceptions | false-success and lost failure provenance | schedule/return identity matching and raw failure list |
| Runtime -> cleanup | no independent teardown inventory or crash injection receipt | state leak, nonreproducibility | force stop, live resource enumeration, cleanup=0 |
| Outcome -> third-party use | no stable witness schema, consumer SDK or external review | no downstream dependency proof | independent package consumer, issue/PR and reviewer confirmation |

### Candidate implementation contract (not yet implemented)
`WitnessV0` should bind `run_id`, `task_id`, `variant_id`, `dataset_release`, `environment/provider/image_digest`, `agent_version`, `action_trace_digest`, `before_state_digest`, `after_state_digest`, `oracle_id/version`, `oracle_result`, `cleanup_state`, and `result_status`. Sensitive screenshots, tokens and personal data must remain optional protected artifacts, with hashes and redacted metadata sufficient to identify a replay target. An individual SHA-256 hash is integrity metadata, NOT a signature, origin attestation or proof of a true observation.

### Prioritization and falsifiable next gates
- **Gate A, P0:** Python 3.12 benchmark core/oracle suite passes in real browser/Xvfb; full RL worker suite collects and runs with declared extras. Report number of tests, all failures and CI run/commit.
- **Gate B, P0:** intentional process interruption during a disposable CUA VM task, independent observation of remaining processes/containers, repeatable zero-resource cleanup. Not yet executed.
- **Gate C, P1:** independent evaluator reads raw action and state evidence to reproduce at least one task result; reject forged reward, edited task path, missing actions and mismatched image versions.
- **Gate D, P1:** a second repository consumes the versioned witness contract without fork-local imports and publishes a reproducible report.
- **Stop/pivot condition:** if an external adopter cannot name a contract deficiency after two genuine integrations, prefer upstream conformance contributions rather than branding another infra repo. If external feedback shows existing OSWorld tools solve the same job at lower cost, re-use them and kill redundant implementation.

### Anti-claims
No verified external adoption, certification, real VM cleanup receipt or long-term defensibility proven yet. Evidence artifacts and test coverage are not themselves network effects.

## Reassessment against 2026-10-09 upstream and external practice

The official `trycua/cua` project remains active. Its open PRs include #4899 (worker stdout pipe saturation at long run), #4898 (worker lock/timeout deadlock), #4884 (reject action-truth claims inconsistent with E2E oracles), and #4883 (metadata-only runtime conformance witness). **Do not describe those as our novel discoveries or duplicate their patches.** Pursue narrow upstream-compatible changes after base comparison, not a large mixed PR from this feature branch.

External comparison: OSWorld-V2's supported benchmark release defines pinned task/assets/web/provider image versions (https://github.com/xlang-ai/OSWorld-V2/blob/main/benchmark_releases/README.md). This fork's manifest hashes local task files only and currently does not bind runtime/provider assets. OpenTelemetry GenAI agent/tool span conventions are still in Development and are useful for trace interoperability, but spans alone are not proof of correctness (https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/gen-ai-agent-spans.md).

### 10-axis horizontal audit (evidence available at this time)

| Axis | Status | Evidence/gap |
| --- | --- | --- |
| Function | Partial | core runner and dataset manifest exist; no product-entry one-click provenance comparison |
| State | Partial | file hashes and test-result checks; no runtime state diff or replay |
| Integration | Partial | GitHub CI builds/tests; real VM provider receipt missing |
| Security/correctness | Partial | worker identity, scoring, path/hidden-file checks; manifests not origin authenticated |
| Performance | Blocked | no cross-provider latency/throughput/long-run fixture from this branch |
| Maintainability | Partial | script CLIs and v1 JSON schema; manifest not yet stable library API |
| Observability | Partial | pytest report SHA; no per-step trace, environment image digest or teardown inventory |
| Testability | Partial | mock and Chromium E2E tests present; force-kill/recovery independent test absent |
| User value | Partial | reproducibility helper available; no independent consumer demonstrated |
| External compatibility | Partial | uses pytest-json-report/jsonschema; OSWorld/provider/OpenTelemetry adapters absent |

### Priority decisions and falsification

P0: finish repeatable end-to-end benchmarks and worker failure recovery; **upstream #4899/#4898** should be reviewed and reused rather than copied. P0: close dataset integrity blind spots (hidden config drift fixed on branch, subject to CI). P1: add release manifest pins for provider/image/environment and independent action/outcome snapshots only after a real provider scenario verifies the need. P1: third-party consumption of a minimal stable verifier API. P2: optional OTEL trace export; adopt current semantic conventions with careful version pinning.

**External standing:** no official PR, merge, maintainer endorsement, external dependency or documented third-party consumption established by this audit. Fork CI success is valuable engineering evidence, not external adoption. 6–12 month credential requires upstream accepted work and/or independent consumer reproduction. 5–10-year defensibility is a hypothesis, not a promise; evaluate by recurring downstream reliance and standards participation. Pivot if two genuine external users decline adoption because an existing upstream tool already meets the same need.
