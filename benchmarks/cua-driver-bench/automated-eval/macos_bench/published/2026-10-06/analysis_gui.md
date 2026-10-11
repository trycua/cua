# Claude Code benchmark: cc-cua-driver vs cc-codex-cu

A = `cc-cua-driver` (Claude Code + Cua Driver 0.34.0 MCP + Cua Driver skill); B = `cc-codex-cu` (Claude Code + Codex computer-use cua_repl MCP, no skill).

Parameters: bootstrap=10000, seed=1234, pass-k=3, headline groups=CDBG, coverage tags=coverage_probe, hover.

## 1. Data summary

| arm | rows | excluded (verified infra failure) | analyzed (non-excluded) | status completed | timeout | agent_error | infra_error (not excluded) | excluded reasons |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | 15 | 0 | 15 | 3 | 0 | 12 | 0 | none |
| cc-codex-cu | 15 | 0 | 15 | 11 | 0 | 4 | 0 | none |

Excluded trials are dropped from all outcome statistics. Non-excluded timeout, agent_error and infra_error trials count as failures. Per-task excluded counts are in the `excl` column of section 5.

## 2. Headline: task-macro success

Headline tasks (3): CDB-G02, CDB-G03, CDB-G04.
Coverage-tagged tasks, excluded from the headline (0): none.
Other non-headline tasks (0): none.

| arm | tasks (paired) | runs | task-macro success | 95% hierarchical bootstrap CI |
| --- | --- | --- | --- | --- |
| A: cc-cua-driver | 3 | 15 | 0.000 | [0.000, 0.000] |
| B: cc-codex-cu | 3 | 15 | 0.267 | [0.000, 0.735] |

Difference A - B (task-macro success): **-0.267**, 95% CI **[-0.735, 0.000]**, n = 3 paired headline tasks (15 runs in A, 15 runs in B).

Verdict: **no resolvable difference** (A = cc-cua-driver, B = cc-codex-cu; the verdict only reports whether the 95% CI of A - B excludes 0).

The CI is a percentile bootstrap (resample tasks with replacement, then runs within each sampled task, independently per arm). With a handful of tasks and runs it tends to be too narrow.

Every headline task has at least 5 non-excluded runs per arm.

### Non-headline scopes (descriptive; no verdict)

| scope | tasks | A macro (cc-cua-driver) | B macro (cc-codex-cu) | A - B (paired tasks) | 95% bootstrap CI | paired tasks |
| --- | --- | --- | --- | --- | --- | --- |
| coverage-tagged (not headline) | 0 | n/a (tasks=0, runs=0) | n/a (tasks=0, runs=0) | n/a | n/a | n/a |
| other non-headline (task_group not in headline groups) | 0 | n/a (tasks=0, runs=0) | n/a (tasks=0, runs=0) | n/a | n/a | n/a |

## 3. Per-task paired comparison (A minus B)

| task | scope | A successes/n = rate [Wilson 95%] | B successes/n = rate [Wilson 95%] | A - B success rate | Fisher exact p (two-sided, descriptive) |
| --- | --- | --- | --- | --- | --- |
| CDB-G02 | headline | 0/5 = 0.00 [0.00, 0.43] | 0/5 = 0.00 [0.00, 0.43] | 0.000 | 1.0000 |
| CDB-G03 | headline | 0/5 = 0.00 [0.00, 0.43] | 0/5 = 0.00 [0.00, 0.43] | 0.000 | 1.0000 |
| CDB-G04 | headline | 0/5 = 0.00 [0.00, 0.43] | 4/5 = 0.80 [0.38, 0.96] | -0.800 | 0.0476 |

Note: Per-task Fisher exact p-values are descriptive only. No multiplicity correction is applied and n per cell is tiny, so these p-values must not be used as decision thresholds.

## 4. pass@1 and pass^3

pass^k is the probability that all k runs succeed, estimated per task as C(s,k)/C(n,k) and averaged over tasks with n >= k (k=3). pass@1 is the mean per-task success rate, which equals the task-macro success. micro = pooled successes / pooled runs (tasks with more runs weigh more).

| scope | arm | tasks | runs | pass@1 (= task-macro) | micro success | pass^3 | tasks omitted from pass^3 (n < k) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| headline | cc-cua-driver | 3 | 15 | 0.000 | 0.000 | 0.000 (tasks used=3) | none |
| headline | cc-codex-cu | 3 | 15 | 0.267 | 0.267 | 0.133 (tasks used=3) | none |
| coverage | cc-cua-driver | 0 | 0 | n/a | n/a | n/a (tasks used=0) | none |
| coverage | cc-codex-cu | 0 | 0 | n/a | n/a | n/a (tasks used=0) | none |
| other | cc-cua-driver | 0 | 0 | n/a | n/a | n/a (tasks used=0) | none |
| other | cc-codex-cu | 0 | 0 | n/a | n/a | n/a (tasks used=0) | none |

## 5. Per arm x task (non-excluded runs; failures never dropped)

### 5a. Outcomes

| task | group/scope | arm | excl | n runs | successes | success rate [Wilson 95%] | mean score (n scored) | failed, status completed | timeout | agent_error | infra_error (not excluded) | confirmation requested |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | CDBG/headline | cc-cua-driver | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.320 (n=5) | 1 | 0 | 4 | 0 | 0 |
| CDB-G02 | CDBG/headline | cc-codex-cu | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.200 (n=5) | 5 | 0 | 0 | 0 | 0 |
| CDB-G03 | CDBG/headline | cc-cua-driver | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.450 (n=5) | 1 | 0 | 4 | 0 | 0 |
| CDB-G03 | CDBG/headline | cc-codex-cu | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.450 (n=5) | 1 | 0 | 4 | 0 | 0 |
| CDB-G04 | CDBG/headline | cc-cua-driver | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.150 (n=5) | 1 | 0 | 4 | 0 | 0 |
| CDB-G04 | CDBG/headline | cc-codex-cu | 0 | 5 | 4 | 0.80 [0.38, 0.96] | 0.830 (n=5) | 1 | 0 | 0 | 0 | 0 |

Mean score is over runs with a non-null score only; `mean_null_as_zero` is in the JSON.

### 5b. wall_s (seconds)

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | cc-cua-driver | 5 | 155.5 | 156.0 | 141.4 | 160.7 | 133.5 | 188.9 |
| CDB-G02 | cc-codex-cu | 5 | 75.6 | 70.7 | 68.8 | 78.7 | 30.6 | 99.8 |
| CDB-G03 | cc-cua-driver | 5 | 155.2 | 154.0 | 141.2 | 158.7 | 132.4 | 182.7 |
| CDB-G03 | cc-codex-cu | 5 | 176.5 | 152.4 | 153.9 | 177.2 | 38.3 | 216.3 |
| CDB-G04 | cc-cua-driver | 5 | 149.4 | 163.2 | 132.4 | 207.6 | 63.3 | 263.5 |
| CDB-G04 | cc-codex-cu | 5 | 91.9 | 95.6 | 87.4 | 98.4 | 83.0 | 117.1 |

### 5c. steps (tool_calls.total)

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | cc-cua-driver | 5 | 55.0 | 58.0 | 51.0 | 63.0 | 47.0 | 74.0 |
| CDB-G02 | cc-codex-cu | 5 | 26.0 | 22.8 | 25.0 | 28.0 | 6.0 | 29.0 |
| CDB-G03 | cc-cua-driver | 5 | 62.0 | 67.8 | 62.0 | 67.0 | 58.0 | 90.0 |
| CDB-G03 | cc-codex-cu | 5 | 45.0 | 38.4 | 45.0 | 45.0 | 11.0 | 46.0 |
| CDB-G04 | cc-cua-driver | 5 | 50.0 | 65.0 | 49.0 | 89.0 | 33.0 | 104.0 |
| CDB-G04 | cc-codex-cu | 5 | 27.0 | 28.4 | 26.0 | 29.0 | 24.0 | 36.0 |

### 5d. tokens: input

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | cc-cua-driver | 5 | 90 | 85 | 90 | 90 | 66 | 90 |
| CDB-G02 | cc-codex-cu | 5 | 54 | 47 | 48 | 58 | 14 | 60 |
| CDB-G03 | cc-cua-driver | 5 | 90 | 84 | 90 | 90 | 58 | 90 |
| CDB-G03 | cc-codex-cu | 5 | 90 | 76 | 90 | 90 | 22 | 90 |
| CDB-G04 | cc-cua-driver | 5 | 90 | 80 | 90 | 90 | 40 | 90 |
| CDB-G04 | cc-codex-cu | 5 | 52 | 56 | 52 | 58 | 48 | 72 |

### 5e. tokens: cached input

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | cc-cua-driver | 5 | 5929109 | 5146386 | 4890803 | 6001005 | 2579756 | 6331256 |
| CDB-G02 | cc-codex-cu | 5 | 525809 | 474458 | 449618 | 581585 | 75384 | 739893 |
| CDB-G03 | cc-cua-driver | 5 | 3874061 | 3707121 | 3550368 | 4102419 | 2478690 | 4530067 |
| CDB-G03 | cc-codex-cu | 5 | 1867411 | 1702185 | 1606530 | 1937247 | 124077 | 2975658 |
| CDB-G04 | cc-cua-driver | 5 | 3505060 | 3030637 | 3343806 | 3684038 | 501159 | 4119121 |
| CDB-G04 | cc-codex-cu | 5 | 732638 | 751346 | 721329 | 751881 | 608270 | 942610 |

### 5f. tokens: output

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | cc-cua-driver | 5 | 9313 | 9613 | 9182 | 10488 | 8473 | 10611 |
| CDB-G02 | cc-codex-cu | 5 | 4471 | 4009 | 4194 | 4564 | 1110 | 5705 |
| CDB-G03 | cc-cua-driver | 5 | 10207 | 10714 | 10065 | 10221 | 7807 | 15268 |
| CDB-G03 | cc-codex-cu | 5 | 10369 | 8781 | 9108 | 10493 | 1741 | 12195 |
| CDB-G04 | cc-cua-driver | 5 | 7429 | 9200 | 6704 | 12165 | 4132 | 15568 |
| CDB-G04 | cc-codex-cu | 5 | 4858 | 4971 | 4742 | 4930 | 4344 | 5979 |

### 5g. estimated cost (USD)

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | cc-cua-driver | 5 | 2.0333 | 1.7924 | 1.6153 | 2.0516 | 1.0669 | 2.1950 |
| CDB-G02 | cc-codex-cu | 5 | 0.2413 | 0.2319 | 0.2241 | 0.2540 | 0.0859 | 0.3542 |
| CDB-G03 | cc-cua-driver | 5 | 1.2637 | 1.3749 | 1.2483 | 1.4811 | 1.2351 | 1.6461 |
| CDB-G03 | cc-codex-cu | 5 | 0.7600 | 0.6881 | 0.6726 | 0.8375 | 0.0654 | 1.1051 |
| CDB-G04 | cc-cua-driver | 5 | 1.1586 | 1.0894 | 1.1071 | 1.3555 | 0.2976 | 1.5284 |
| CDB-G04 | cc-codex-cu | 5 | 0.3398 | 0.3506 | 0.3289 | 0.3861 | 0.2977 | 0.4006 |

Quartiles use linear interpolation (IQR = Q3 - Q1). Cost and token counts only include runs where the field is present (see n).

### 5h. Tool calls by class (mean per run; failed = summed over runs)

| task | arm | n runs | observe | click | type | key | scroll | drag | set_value | other | builtin | total (mean) | failed calls (sum) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | cc-cua-driver | 5 | 28.2 | 13.6 | 2.4 | 4.2 | 1.0 | 0.0 | 3.8 | 1.6 | 3.2 | 58.0 | 31 |
| CDB-G02 | cc-codex-cu | 5 | 7.2 | 5.0 | 0.0 | 3.6 | 0.8 | 0.0 | 0.0 | 4.6 | 1.6 | 22.8 | 16 |
| CDB-G03 | cc-cua-driver | 5 | 30.2 | 23.2 | 1.0 | 5.6 | 1.0 | 0.0 | 2.4 | 2.0 | 2.4 | 67.8 | 26 |
| CDB-G03 | cc-codex-cu | 5 | 6.4 | 18.6 | 1.8 | 4.6 | 0.2 | 0.2 | 0.0 | 5.4 | 1.2 | 38.4 | 30 |
| CDB-G04 | cc-cua-driver | 5 | 26.8 | 19.0 | 7.2 | 7.2 | 0.6 | 0.0 | 0.4 | 1.6 | 2.2 | 65.0 | 58 |
| CDB-G04 | cc-codex-cu | 5 | 5.2 | 13.6 | 1.0 | 1.2 | 0.0 | 0.0 | 0.6 | 4.8 | 2.0 | 28.4 | 24 |

## 6. Raw per-run values (variance is visible; runs in run_index order)

| task | arm | passed (1/0) | wall_s | steps | non-completed status | excluded runs |
| --- | --- | --- | --- | --- | --- | --- |
| CDB-G02 | cc-cua-driver | 0 0 0 0 0 | 141.4 160.7 133.5 188.9 155.5 | 47 51 55 74 63 | run 1=agent_error, run 2=agent_error, run 3=agent_error, run 4=agent_error | - |
| CDB-G02 | cc-codex-cu | 0 0 0 0 0 | 30.6 68.8 75.6 99.8 78.7 | 6 25 28 29 26 | - | - |
| CDB-G03 | cc-cua-driver | 0 0 0 0 0 | 155.2 182.7 158.7 141.2 132.4 | 67 90 62 62 58 | run 0=agent_error, run 1=agent_error, run 2=agent_error, run 4=agent_error | - |
| CDB-G03 | cc-codex-cu | 0 0 0 0 0 | 216.3 153.9 177.2 176.5 38.3 | 46 45 45 45 11 | run 0=agent_error, run 1=agent_error, run 2=agent_error, run 3=agent_error | - |
| CDB-G04 | cc-cua-driver | 0 0 0 0 0 | 149.4 207.6 263.5 132.4 63.3 | 49 89 104 50 33 | run 0=agent_error, run 1=agent_error, run 2=agent_error, run 3=agent_error | - |
| CDB-G04 | cc-codex-cu | 1 1 1 1 0 | 98.4 117.1 87.4 91.9 83.0 | 27 36 24 26 29 | - | - |

## 7. Efficiency

Medians with [Q1, Q3] and n. Conditional = successful runs only; unconditional = all non-excluded runs. Pooled across tasks, so the task mix of each arm affects these numbers; compare per-task values in section 5 before drawing conclusions. Total tokens = input + output (assumes cached input is a subset of input and reasoning a subset of output).

### Headline tasks

| arm | basis | trials | wall_s (s) | steps | tokens in | tokens cached | tokens out | tokens total | cost (USD) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | success only | 0 | n/a (n=0) | n/a (n=0) | n/a (n=0) | n/a (n=0) | n/a (n=0) | n/a (n=0) | n/a (n=0) |
| cc-cua-driver | all non-excluded | 15 | 155.2 [137.3, 171.7] (n=15) | 62.0 [50.5, 70.5] (n=15) | 90 [90, 90] (n=15) | 3874061 [3424433, 4710435] (n=15) | 10065 [8140, 10550] (n=15) | 10123 [8218, 10640] (n=15) | 1.3555 [1.1968, 1.6307] (n=15) |
| cc-codex-cu | success only | 4 | 95.2 [90.8, 103.1] (n=4) | 26.5 [25.5, 29.2] (n=4) | 52 [51, 57] (n=4) | 742260 [701546, 799563] (n=4) | 4800 [4642, 5138] (n=4) | 4850 [4694, 5192] (n=4) | 0.3629 [0.3293, 0.3897] (n=4) |
| cc-codex-cu | all non-excluded | 15 | 91.9 [77.1, 135.5] (n=15) | 28.0 [25.5, 40.5] (n=15) | 58 [50, 81] (n=15) | 732638 [553697, 1274570] (n=15) | 4858 [4408, 7544] (n=15) | 4906 [4460, 7624] (n=15) | 0.3398 [0.2476, 0.5366] (n=15) |

### All tasks (non-excluded trials)

| arm | basis | trials | wall_s (s) | steps | tokens in | tokens cached | tokens out | tokens total | cost (USD) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | success only | 0 | n/a (n=0) | n/a (n=0) | n/a (n=0) | n/a (n=0) | n/a (n=0) | n/a (n=0) | n/a (n=0) |
| cc-cua-driver | all non-excluded | 15 | 155.2 [137.3, 171.7] (n=15) | 62.0 [50.5, 70.5] (n=15) | 90 [90, 90] (n=15) | 3874061 [3424433, 4710435] (n=15) | 10065 [8140, 10550] (n=15) | 10123 [8218, 10640] (n=15) | 1.3555 [1.1968, 1.6307] (n=15) |
| cc-codex-cu | success only | 4 | 95.2 [90.8, 103.1] (n=4) | 26.5 [25.5, 29.2] (n=4) | 52 [51, 57] (n=4) | 742260 [701546, 799563] (n=4) | 4800 [4642, 5138] (n=4) | 4850 [4694, 5192] (n=4) | 0.3629 [0.3293, 0.3897] (n=4) |
| cc-codex-cu | all non-excluded | 15 | 91.9 [77.1, 135.5] (n=15) | 28.0 [25.5, 40.5] (n=15) | 58 [50, 81] (n=15) | 732638 [553697, 1274570] (n=15) | 4858 [4408, 7544] (n=15) | 4906 [4460, 7624] (n=15) | 0.3398 [0.2476, 0.5366] (n=15) |

## 8. Time per tool call by class

Per trial, `action_latency_ms` gives a median and p90 per action class. Here: the median of the per-trial medians (and the median of per-trial p90s) across trials, with the number of trials that timed that class. All non-excluded trials.

| class | A median of trial medians (cc-cua-driver) | A median of trial p90 | B median of trial medians (cc-codex-cu) | B median of trial p90 |
| --- | --- | --- | --- | --- |
| observe | 209.2 ms (trials=15, actions=426) | 1327.9 ms | 133.7 ms (trials=15, actions=94) | 389.9 ms |
| click | 781.3 ms (trials=15, actions=279) | 2013.2 ms | 687.9 ms (trials=13, actions=186) | 1505.6 ms |
| type | 3804.1 ms (trials=11, actions=53) | 4415.2 ms | 1681.4 ms (trials=6, actions=14) | 2193.1 ms |
| key | 1141.0 ms (trials=14, actions=85) | 1614.2 ms | 634.3 ms (trials=12, actions=47) | 1058.3 ms |
| scroll | 507.3 ms (trials=12, actions=13) | 507.3 ms | 404.0 ms (trials=5, actions=5) | 404.0 ms |
| drag | n/a (0 trials) | n/a | 848.4 ms (trials=1, actions=1) | 848.4 ms |
| set_value | 3205.2 ms (trials=4, actions=33) | 6472.5 ms | 1833.6 ms (trials=2, actions=3) | 2200.8 ms |
| other | 636.0 ms (trials=12, actions=26) | 1137.0 ms | 91.4 ms (trials=15, actions=74) | 784.6 ms |

## 9. Disturbance

Only non-excluded trials with `disturbance.available` true and `human_input_suspected` false are used. Cells show mean / max per trial. hid_events total = move + down + key + scroll. Any disturbance = front_changes, key_loss, keystrokes_leaked or clicks_leaked > 0, or any hid event, or pointer_deviation_episodes > 0.

| arm | trials used | dropped: human_input_suspected | unavailable | front_changes | key_loss | keystrokes_leaked | clicks_leaked | scrolls_leaked (not in 'any') | hid_events total | pointer_max_deviation_px | any disturbance [Wilson 95%] |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | 4 | 11 | 0 | 4.75 / 14.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 2.75 / 11.00 | 72.2 / 288.8 | 3/4 = 0.75 [0.30, 0.95] |
| cc-codex-cu | 15 | 0 | 0 | 0.13 / 1.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.0 / 0.0 | 2/15 = 0.13 [0.04, 0.38] |

## 10. Data-quality flags

| flag | value |
| --- | --- |
| trials with passed=true but status != completed (counted as failures) | none |
| excluded trials with no excluded_reason | none |
| infra_error trials NOT excluded (counted as failures) | cc-cua-driver: 0, cc-codex-cu: 0 |
| human_input_suspected (non-excluded; dropped from disturbance only) | cc-cua-driver: 11, cc-codex-cu: 0 |
| evaluator_read_suspected (non-excluded; kept in all statistics) | cc-cua-driver: 0, cc-codex-cu: 0 |
| confirmation_requested (non-excluded) | cc-cua-driver: 0, cc-codex-cu: 0 |
| tasks with inconsistent task_group or dimension_tags across rows | none |
| tasks with unequal non-excluded run counts between arms | none |

## 11. Method notes

- Success = passed AND status == completed, among non-excluded trials. Excluded trials (verified infrastructure failures) are dropped from outcome statistics and counted per arm and per task.
- Task-macro success: mean over tasks of the per-task success rate; each task weighted equally regardless of attempts.
- Headline set: task_group in the headline groups and no dimension tag in the coverage tags. Tasks that fail this test are reported separately and never enter the headline.
- Difference CI: percentile bootstrap, tasks resampled with replacement, then runs within each sampled task resampled with replacement, independently per arm (runs are not paired by index). Only tasks with non-excluded runs in both arms enter the comparison.
- Wilson 95% intervals for proportions; Fisher exact two-sided p-values (sum of tables no more probable than the observed one). Per-task Fisher exact p-values are descriptive only. No multiplicity correction is applied and n per cell is tiny, so these p-values must not be used as decision thresholds.
- pass^k = mean over tasks (with n >= k) of C(s,k)/C(n,k); here k=3.
- The pilot-size warning is printed (as a block quote in section 2) whenever any headline task has fewer than 5 non-excluded runs in either arm.
- Latency: the median of per-trial medians is reported (not an n-weighted median of medians), with the number of trials.
- Quartiles use linear interpolation between order statistics.
- Results are deterministic given the seed.

## Claude Code run details

Equivalent cost is `total_cost_usd` from each trial's result event (subscription run, nothing billed per token). `input` excludes cache tokens; `cache_read` and `cache_write` are separate.

| arm | trials | passed | equivalent cost sum USD | cost per trial | input | output | cache read | cache write | turns | ToolSearch turns | baseline prompt tokens | declared DONE |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | 15 | 0 | 21.28 | 1.42 (median 1.36, n=15) | 1244 | 147633 | 59420718 | 1980151 | 41.5 (median 45, n=15) | 1.67 (median 2, n=15) | 6.81e+03 (median 6.74e+03, n=15) | 0 |
| cc-codex-cu | 15 | 4 | 6.35 | 0.424 (median 0.34, n=15) | 898 | 88803 | 14639940 | 633814 | 29.9 (median 29, n=15) | 1 (median 1, n=15) | 5.39e+03 (median 5.33e+03, n=15) | 3 |

Trial status by arm: cc-codex-cu completed=11, cc-codex-cu max_turns=4, cc-cua-driver completed=3, cc-cua-driver max_turns=12
- 7-day quota used per trial, cc-cua-driver: mean 0.0007 over 15 trials (the login is shared, so other sessions add noise)
- 7-day quota used per trial, cc-codex-cu: mean 0.0013 over 15 trials (the login is shared, so other sessions add noise)
- pauses: 0 totalling 0.0 min (see pauses.jsonl)
