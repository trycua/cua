# Comparison view reference

`cdb report` derives five views from frozen v0.3 trial, system, and
execution-policy manifests.

| View | Varying factor | Fixed factors |
| --- | --- | --- |
| `system-track` | Complete system digest | Task, attempt, environment, policy, and apparatus |
| `harness-comparison` | Harness slice | Observed model route, driver, profile, tool contract, task, attempt, policy, environment, and apparatus |
| `model-comparison` | Declared and observed model route | Harness, driver, profile, tool contract, task, attempt, policy, environment, and apparatus |
| `driver-profile` | Driver presentation profile | Candidate, harness, model, task, attempt, policy, environment, and apparatus |
| `tool-surface` | Consolidated, factored, or fine-grained surface | Backend, harness, capabilities, guidance, task, attempt, policy, environment, and apparatus |

`apparatus_digest` identifies the shared benchmark-owned experimental
apparatus. Do not bind it to a system-specific guest-launch contract: harness
launch and configuration belong to the complete system identity. A
system-track report rejects per-system apparatus digests as a confound even
when every trial is descriptive or comparison-ineligible.

Run `cdb preregister-report` over the complete frozen template block before
execution. Its receipt records the selected view, arms, fixed-factor values,
required attempts, pairing keys, and content hashes of every template, system,
and policy input. The command rejects a confounded or incomplete plan before
the benchmark spends an attempt.

## Common statistics

Reports use task-macro success and a deterministic hierarchical bootstrap over
tasks and attempts. Controlled views also report paired deltas and paired
intervals. Each arm records `pass^k`, rank range, median wall time, token and
cost sample counts, termination mix, and missingness.

## Exclusions

Infrastructure and reset failures do not enter outcome scores. A paired block
is excluded when one arm has such a failure. The report records the failure and
paired exclusion.

## Decision-bearing conditions

A report is descriptive when it lacks required completed attempts, contains no
comparison-eligible trials, exceeds the registered infrastructure-failure
rate, includes uncertified outcomes where certification is required, or cannot
prove its fixed-factor bindings.

## Related material

- [How to export and report results](../how-to/export-and-report-results.md)
- [Profiles, systems, and comparisons](../explanation/profiles-systems-and-comparisons.md)
