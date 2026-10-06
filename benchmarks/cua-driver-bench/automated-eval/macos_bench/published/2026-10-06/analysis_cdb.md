# Claude Code benchmark: cc-cua-driver vs cc-codex-cu

A = `cc-cua-driver` (Claude Code + Cua Driver 0.34.0 MCP + Cua Driver skill); B = `cc-codex-cu` (Claude Code + Codex computer-use cua_repl MCP, no skill).

Parameters: bootstrap=10000, seed=1234, pass-k=3, headline groups=CDB, coverage tags=coverage_probe, hover.

## 1. Data summary

| arm | rows | excluded (verified infra failure) | analyzed (non-excluded) | status completed | timeout | agent_error | infra_error (not excluded) | excluded reasons |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | 35 | 0 | 35 | 35 | 0 | 0 | 0 | none |
| cc-codex-cu | 35 | 0 | 35 | 35 | 0 | 0 | 0 | none |

Excluded trials are dropped from all outcome statistics. Non-excluded timeout, agent_error and infra_error trials count as failures. Per-task excluded counts are in the `excl` column of section 5.

## 2. Headline: task-macro success

Headline tasks (4): CDB-S01, CDB-S02, CDB-S03, CDB-S04.
Coverage-tagged tasks, excluded from the headline (2): MB-10, MB-11.
Other non-headline tasks (1): MB-09.

| arm | tasks (paired) | runs | task-macro success | 95% hierarchical bootstrap CI |
| --- | --- | --- | --- | --- |
| A: cc-cua-driver | 4 | 20 | 0.650 | [0.250, 1.000] |
| B: cc-codex-cu | 4 | 20 | 0.750 | [0.250, 1.000] |

Difference A - B (task-macro success): **-0.100**, 95% CI **[-0.350, 0.000]**, n = 4 paired headline tasks (20 runs in A, 20 runs in B).

Verdict: **no resolvable difference** (A = cc-cua-driver, B = cc-codex-cu; the verdict only reports whether the 95% CI of A - B excludes 0).

The CI is a percentile bootstrap (resample tasks with replacement, then runs within each sampled task, independently per arm). With a handful of tasks and runs it tends to be too narrow.

Every headline task has at least 5 non-excluded runs per arm.

### Non-headline scopes (descriptive; no verdict)

| scope | tasks | A macro (cc-cua-driver) | B macro (cc-codex-cu) | A - B (paired tasks) | 95% bootstrap CI | paired tasks |
| --- | --- | --- | --- | --- | --- | --- |
| coverage-tagged (not headline) | 2 | 0.80 (tasks=2, runs=10) | 0.00 (tasks=2, runs=10) | 0.800 | [0.500, 1.000] | 2 |
| other non-headline (task_group not in headline groups) | 1 | 1.00 (tasks=1, runs=5) | 1.00 (tasks=1, runs=5) | 0.000 | [0.000, 0.000] | 1 |

## 3. Per-task paired comparison (A minus B)

| task | scope | A successes/n = rate [Wilson 95%] | B successes/n = rate [Wilson 95%] | A - B success rate | Fisher exact p (two-sided, descriptive) |
| --- | --- | --- | --- | --- | --- |
| CDB-S01 | headline | 5/5 = 1.00 [0.57, 1.00] | 5/5 = 1.00 [0.57, 1.00] | 0.000 | 1.0000 |
| CDB-S02 | headline | 0/5 = 0.00 [0.00, 0.43] | 0/5 = 0.00 [0.00, 0.43] | 0.000 | 1.0000 |
| CDB-S03 | headline | 3/5 = 0.60 [0.23, 0.88] | 5/5 = 1.00 [0.57, 1.00] | -0.400 | 0.4444 |
| CDB-S04 | headline | 5/5 = 1.00 [0.57, 1.00] | 5/5 = 1.00 [0.57, 1.00] | 0.000 | 1.0000 |
| MB-09 | other | 5/5 = 1.00 [0.57, 1.00] | 5/5 = 1.00 [0.57, 1.00] | 0.000 | 1.0000 |
| MB-10 | coverage | 4/5 = 0.80 [0.38, 0.96] | 0/5 = 0.00 [0.00, 0.43] | 0.800 | 0.0476 |
| MB-11 | coverage | 4/5 = 0.80 [0.38, 0.96] | 0/5 = 0.00 [0.00, 0.43] | 0.800 | 0.0476 |

Note: Per-task Fisher exact p-values are descriptive only. No multiplicity correction is applied and n per cell is tiny, so these p-values must not be used as decision thresholds.

## 4. pass@1 and pass^3

pass^k is the probability that all k runs succeed, estimated per task as C(s,k)/C(n,k) and averaged over tasks with n >= k (k=3). pass@1 is the mean per-task success rate, which equals the task-macro success. micro = pooled successes / pooled runs (tasks with more runs weigh more).

| scope | arm | tasks | runs | pass@1 (= task-macro) | micro success | pass^3 | tasks omitted from pass^3 (n < k) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| headline | cc-cua-driver | 4 | 20 | 0.650 | 0.650 | 0.525 (tasks used=4) | none |
| headline | cc-codex-cu | 4 | 20 | 0.750 | 0.750 | 0.750 (tasks used=4) | none |
| coverage | cc-cua-driver | 2 | 10 | 0.800 | 0.800 | 0.400 (tasks used=2) | none |
| coverage | cc-codex-cu | 2 | 10 | 0.000 | 0.000 | 0.000 (tasks used=2) | none |
| other | cc-cua-driver | 1 | 5 | 1.000 | 1.000 | 1.000 (tasks used=1) | none |
| other | cc-codex-cu | 1 | 5 | 1.000 | 1.000 | 1.000 (tasks used=1) | none |

## 5. Per arm x task (non-excluded runs; failures never dropped)

### 5a. Outcomes

| task | group/scope | arm | excl | n runs | successes | success rate [Wilson 95%] | mean score (n scored) | failed, status completed | timeout | agent_error | infra_error (not excluded) | confirmation requested |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | CDB/headline | cc-cua-driver | 0 | 5 | 5 | 1.00 [0.57, 1.00] | 1.000 (n=5) | 0 | 0 | 0 | 0 | 0 |
| CDB-S01 | CDB/headline | cc-codex-cu | 0 | 5 | 5 | 1.00 [0.57, 1.00] | 1.000 (n=5) | 0 | 0 | 0 | 0 | 0 |
| CDB-S02 | CDB/headline | cc-cua-driver | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.650 (n=5) | 5 | 0 | 0 | 0 | 0 |
| CDB-S02 | CDB/headline | cc-codex-cu | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.710 (n=5) | 5 | 0 | 0 | 0 | 0 |
| CDB-S03 | CDB/headline | cc-cua-driver | 0 | 5 | 3 | 0.60 [0.23, 0.88] | 0.940 (n=5) | 2 | 0 | 0 | 0 | 0 |
| CDB-S03 | CDB/headline | cc-codex-cu | 0 | 5 | 5 | 1.00 [0.57, 1.00] | 1.000 (n=5) | 0 | 0 | 0 | 0 | 0 |
| CDB-S04 | CDB/headline | cc-cua-driver | 0 | 5 | 5 | 1.00 [0.57, 1.00] | 1.000 (n=5) | 0 | 0 | 0 | 0 | 0 |
| CDB-S04 | CDB/headline | cc-codex-cu | 0 | 5 | 5 | 1.00 [0.57, 1.00] | 1.000 (n=5) | 0 | 0 | 0 | 0 | 0 |
| MB-09 | P/other | cc-cua-driver | 0 | 5 | 5 | 1.00 [0.57, 1.00] | 1.000 (n=5) | 0 | 0 | 0 | 0 | 0 |
| MB-09 | P/other | cc-codex-cu | 0 | 5 | 5 | 1.00 [0.57, 1.00] | 1.000 (n=5) | 0 | 0 | 0 | 0 | 0 |
| MB-10 | P/coverage | cc-cua-driver | 0 | 5 | 4 | 0.80 [0.38, 0.96] | 0.820 (n=5) | 1 | 0 | 0 | 0 | 0 |
| MB-10 | P/coverage | cc-codex-cu | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.800 (n=5) | 5 | 0 | 0 | 0 | 0 |
| MB-11 | P/coverage | cc-cua-driver | 0 | 5 | 4 | 0.80 [0.38, 0.96] | 0.810 (n=5) | 1 | 0 | 0 | 0 | 0 |
| MB-11 | P/coverage | cc-codex-cu | 0 | 5 | 0 | 0.00 [0.00, 0.43] | 0.050 (n=5) | 5 | 0 | 0 | 0 | 0 |

Mean score is over runs with a non-null score only; `mean_null_as_zero` is in the JSON.

### 5b. wall_s (seconds)

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | cc-cua-driver | 5 | 89.1 | 90.0 | 87.5 | 98.4 | 75.9 | 98.9 |
| CDB-S01 | cc-codex-cu | 5 | 52.8 | 55.2 | 52.3 | 55.2 | 52.3 | 63.7 |
| CDB-S02 | cc-cua-driver | 5 | 34.6 | 34.9 | 32.1 | 34.6 | 31.6 | 41.5 |
| CDB-S02 | cc-codex-cu | 5 | 32.0 | 32.8 | 31.6 | 34.4 | 30.2 | 35.7 |
| CDB-S03 | cc-cua-driver | 5 | 42.8 | 45.1 | 41.5 | 48.1 | 40.0 | 53.0 |
| CDB-S03 | cc-codex-cu | 5 | 43.2 | 44.7 | 41.8 | 43.7 | 41.2 | 53.5 |
| CDB-S04 | cc-cua-driver | 5 | 109.5 | 98.2 | 102.5 | 116.3 | 22.9 | 140.1 |
| CDB-S04 | cc-codex-cu | 5 | 52.0 | 47.6 | 28.7 | 59.8 | 27.5 | 70.2 |
| MB-09 | cc-cua-driver | 5 | 33.6 | 33.2 | 32.9 | 35.2 | 26.0 | 38.3 |
| MB-09 | cc-codex-cu | 5 | 76.4 | 91.6 | 52.8 | 138.2 | 35.9 | 154.7 |
| MB-10 | cc-cua-driver | 5 | 97.2 | 101.1 | 82.8 | 106.3 | 66.2 | 153.1 |
| MB-10 | cc-codex-cu | 5 | 31.8 | 36.3 | 29.2 | 40.5 | 27.0 | 53.2 |
| MB-11 | cc-cua-driver | 5 | 60.3 | 56.8 | 55.1 | 60.4 | 37.0 | 71.1 |
| MB-11 | cc-codex-cu | 5 | 55.5 | 56.6 | 42.1 | 66.3 | 36.9 | 82.0 |

### 5c. steps (tool_calls.total)

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | cc-cua-driver | 5 | 28.0 | 27.4 | 26.0 | 28.0 | 24.0 | 31.0 |
| CDB-S01 | cc-codex-cu | 5 | 16.0 | 16.0 | 15.0 | 17.0 | 15.0 | 17.0 |
| CDB-S02 | cc-cua-driver | 5 | 5.0 | 5.4 | 5.0 | 5.0 | 5.0 | 7.0 |
| CDB-S02 | cc-codex-cu | 5 | 5.0 | 5.4 | 5.0 | 6.0 | 5.0 | 6.0 |
| CDB-S03 | cc-cua-driver | 5 | 6.0 | 6.6 | 5.0 | 8.0 | 5.0 | 9.0 |
| CDB-S03 | cc-codex-cu | 5 | 6.0 | 6.8 | 6.0 | 8.0 | 6.0 | 8.0 |
| CDB-S04 | cc-cua-driver | 5 | 46.0 | 40.8 | 46.0 | 48.0 | 3.0 | 61.0 |
| CDB-S04 | cc-codex-cu | 5 | 17.0 | 13.8 | 5.0 | 19.0 | 5.0 | 23.0 |
| MB-09 | cc-cua-driver | 5 | 9.0 | 9.2 | 9.0 | 9.0 | 7.0 | 12.0 |
| MB-09 | cc-codex-cu | 5 | 22.0 | 24.6 | 14.0 | 37.0 | 10.0 | 40.0 |
| MB-10 | cc-cua-driver | 5 | 45.0 | 44.4 | 44.0 | 52.0 | 18.0 | 63.0 |
| MB-10 | cc-codex-cu | 5 | 9.0 | 9.4 | 9.0 | 10.0 | 6.0 | 13.0 |
| MB-11 | cc-cua-driver | 5 | 20.0 | 23.6 | 19.0 | 31.0 | 15.0 | 33.0 |
| MB-11 | cc-codex-cu | 5 | 11.0 | 10.6 | 10.0 | 11.0 | 7.0 | 14.0 |

### 5d. tokens: input

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | cc-cua-driver | 5 | 46 | 45 | 44 | 46 | 42 | 48 |
| CDB-S01 | cc-codex-cu | 5 | 30 | 30 | 28 | 32 | 26 | 32 |
| CDB-S02 | cc-cua-driver | 5 | 10 | 11 | 10 | 10 | 10 | 14 |
| CDB-S02 | cc-codex-cu | 5 | 10 | 11 | 10 | 12 | 10 | 12 |
| CDB-S03 | cc-cua-driver | 5 | 14 | 15 | 12 | 18 | 10 | 20 |
| CDB-S03 | cc-codex-cu | 5 | 14 | 15 | 14 | 16 | 14 | 16 |
| CDB-S04 | cc-cua-driver | 5 | 48 | 40 | 42 | 50 | 8 | 52 |
| CDB-S04 | cc-codex-cu | 5 | 34 | 27 | 10 | 38 | 10 | 42 |
| MB-09 | cc-cua-driver | 5 | 16 | 17 | 16 | 18 | 14 | 20 |
| MB-09 | cc-codex-cu | 5 | 46 | 51 | 30 | 76 | 22 | 82 |
| MB-10 | cc-cua-driver | 5 | 54 | 54 | 50 | 68 | 28 | 70 |
| MB-10 | cc-codex-cu | 5 | 20 | 20 | 18 | 22 | 14 | 28 |
| MB-11 | cc-cua-driver | 5 | 36 | 38 | 36 | 38 | 30 | 48 |
| MB-11 | cc-codex-cu | 5 | 24 | 23 | 20 | 24 | 16 | 30 |

### 5e. tokens: cached input

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | cc-cua-driver | 5 | 1029000 | 1038941 | 968654 | 1068598 | 942301 | 1186152 |
| CDB-S01 | cc-codex-cu | 5 | 243875 | 244607 | 234078 | 263915 | 205029 | 276140 |
| CDB-S02 | cc-cua-driver | 5 | 62910 | 70802 | 62855 | 67588 | 62847 | 97808 |
| CDB-S02 | cc-codex-cu | 5 | 47337 | 51483 | 45973 | 60009 | 42727 | 61367 |
| CDB-S03 | cc-cua-driver | 5 | 106472 | 118564 | 91067 | 158559 | 72405 | 164318 |
| CDB-S03 | cc-codex-cu | 5 | 109329 | 110619 | 102884 | 116414 | 98459 | 126010 |
| CDB-S04 | cc-cua-driver | 5 | 1418363 | 1101087 | 1111212 | 1452938 | 38647 | 1484277 |
| CDB-S04 | cc-codex-cu | 5 | 363393 | 259084 | 54609 | 391837 | 46378 | 439204 |
| MB-09 | cc-cua-driver | 5 | 149242 | 147166 | 100878 | 201502 | 70368 | 213838 |
| MB-09 | cc-codex-cu | 5 | 424016 | 553749 | 250401 | 927726 | 155748 | 1010855 |
| MB-10 | cc-cua-driver | 5 | 1081206 | 1175938 | 1071767 | 1390024 | 409737 | 1926956 |
| MB-10 | cc-codex-cu | 5 | 122793 | 131486 | 106981 | 149651 | 74845 | 203162 |
| MB-11 | cc-cua-driver | 5 | 615136 | 663118 | 571782 | 696385 | 425585 | 1006700 |
| MB-11 | cc-codex-cu | 5 | 162948 | 153756 | 122235 | 168584 | 89051 | 225964 |

### 5f. tokens: output

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | cc-cua-driver | 5 | 4398 | 4396 | 4313 | 4443 | 3886 | 4939 |
| CDB-S01 | cc-codex-cu | 5 | 2572 | 2585 | 2509 | 2701 | 2401 | 2744 |
| CDB-S02 | cc-cua-driver | 5 | 2071 | 2219 | 2040 | 2086 | 2025 | 2872 |
| CDB-S02 | cc-codex-cu | 5 | 1877 | 2013 | 1767 | 2203 | 1760 | 2460 |
| CDB-S03 | cc-cua-driver | 5 | 3487 | 3539 | 3390 | 3844 | 2905 | 4069 |
| CDB-S03 | cc-codex-cu | 5 | 3324 | 3633 | 3310 | 3830 | 2804 | 4896 |
| CDB-S04 | cc-cua-driver | 5 | 6436 | 5754 | 6371 | 6614 | 648 | 8702 |
| CDB-S04 | cc-codex-cu | 5 | 1780 | 1796 | 850 | 2686 | 811 | 2853 |
| MB-09 | cc-cua-driver | 5 | 1166 | 1250 | 1147 | 1368 | 873 | 1695 |
| MB-09 | cc-codex-cu | 5 | 3473 | 4482 | 1767 | 7763 | 1551 | 7857 |
| MB-10 | cc-cua-driver | 5 | 6935 | 6761 | 6614 | 7372 | 2158 | 10724 |
| MB-10 | cc-codex-cu | 5 | 972 | 1177 | 953 | 1461 | 651 | 1846 |
| MB-11 | cc-cua-driver | 5 | 2282 | 2661 | 2239 | 3491 | 1562 | 3730 |
| MB-11 | cc-codex-cu | 5 | 1878 | 1706 | 1321 | 1995 | 1002 | 2334 |

### 5g. estimated cost (USD)

| task | arm | n | median | mean | Q1 | Q3 | min | max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | cc-cua-driver | 5 | 0.5570 | 0.5626 | 0.5453 | 0.5885 | 0.5078 | 0.6145 |
| CDB-S01 | cc-codex-cu | 5 | 0.1659 | 0.1749 | 0.1495 | 0.2018 | 0.1381 | 0.2194 |
| CDB-S02 | cc-cua-driver | 5 | 0.0742 | 0.0786 | 0.0735 | 0.0744 | 0.0582 | 0.1124 |
| CDB-S02 | cc-codex-cu | 5 | 0.0587 | 0.0579 | 0.0537 | 0.0595 | 0.0517 | 0.0659 |
| CDB-S03 | cc-cua-driver | 5 | 0.1332 | 0.1323 | 0.1235 | 0.1395 | 0.1174 | 0.1478 |
| CDB-S03 | cc-codex-cu | 5 | 0.1317 | 0.1349 | 0.1247 | 0.1325 | 0.1217 | 0.1638 |
| CDB-S04 | cc-cua-driver | 5 | 0.6524 | 0.5456 | 0.5905 | 0.7253 | 0.0288 | 0.7310 |
| CDB-S04 | cc-codex-cu | 5 | 0.1759 | 0.1404 | 0.0700 | 0.1990 | 0.0373 | 0.2197 |
| MB-09 | cc-cua-driver | 5 | 0.1696 | 0.1415 | 0.1040 | 0.1754 | 0.0736 | 0.1848 |
| MB-09 | cc-codex-cu | 5 | 0.2247 | 0.2751 | 0.1663 | 0.4234 | 0.1255 | 0.4354 |
| MB-10 | cc-cua-driver | 5 | 0.5836 | 0.5675 | 0.5261 | 0.5951 | 0.2796 | 0.8533 |
| MB-10 | cc-codex-cu | 5 | 0.1003 | 0.1026 | 0.0977 | 0.1085 | 0.0687 | 0.1379 |
| MB-11 | cc-cua-driver | 5 | 0.3293 | 0.3554 | 0.3103 | 0.4184 | 0.2542 | 0.4650 |
| MB-11 | cc-codex-cu | 5 | 0.1249 | 0.1200 | 0.1061 | 0.1254 | 0.0896 | 0.1542 |

Quartiles use linear interpolation (IQR = Q3 - Q1). Cost and token counts only include runs where the field is present (see n).

### 5h. Tool calls by class (mean per run; failed = summed over runs)

| task | arm | n runs | observe | click | type | key | scroll | drag | set_value | other | builtin | total (mean) | failed calls (sum) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | cc-cua-driver | 5 | 9.0 | 5.4 | 0.4 | 0.0 | 1.4 | 0.0 | 0.6 | 0.0 | 10.6 | 27.4 | 16 |
| CDB-S01 | cc-codex-cu | 5 | 2.8 | 3.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 1.0 | 9.2 | 16.0 | 11 |
| CDB-S02 | cc-cua-driver | 5 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 5.4 | 5.4 | 2 |
| CDB-S02 | cc-codex-cu | 5 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 5.4 | 5.4 | 1 |
| CDB-S03 | cc-cua-driver | 5 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 6.6 | 6.6 | 4 |
| CDB-S03 | cc-codex-cu | 5 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 6.8 | 6.8 | 4 |
| CDB-S04 | cc-cua-driver | 5 | 11.0 | 12.2 | 2.8 | 6.6 | 0.6 | 0.0 | 3.2 | 0.2 | 4.2 | 40.8 | 14 |
| CDB-S04 | cc-codex-cu | 5 | 1.6 | 6.4 | 0.0 | 0.0 | 0.0 | 0.0 | 0.2 | 1.2 | 4.4 | 13.8 | 5 |
| MB-09 | cc-cua-driver | 5 | 4.6 | 1.6 | 0.0 | 0.0 | 0.0 | 2.0 | 0.0 | 0.0 | 1.0 | 9.2 | 1 |
| MB-09 | cc-codex-cu | 5 | 1.0 | 1.2 | 0.0 | 0.0 | 0.0 | 14.4 | 0.0 | 7.0 | 1.0 | 24.6 | 0 |
| MB-10 | cc-cua-driver | 5 | 20.6 | 4.0 | 0.0 | 0.2 | 0.2 | 1.8 | 0.0 | 15.6 | 2.0 | 44.4 | 4 |
| MB-10 | cc-codex-cu | 5 | 1.0 | 3.6 | 0.0 | 0.0 | 1.2 | 1.0 | 0.0 | 1.6 | 1.0 | 9.4 | 0 |
| MB-11 | cc-cua-driver | 5 | 10.8 | 1.8 | 0.8 | 0.0 | 0.0 | 0.0 | 0.0 | 8.6 | 1.6 | 23.6 | 7 |
| MB-11 | cc-codex-cu | 5 | 1.0 | 5.4 | 0.0 | 0.0 | 0.2 | 0.4 | 0.0 | 2.6 | 1.0 | 10.6 | 0 |

## 6. Raw per-run values (variance is visible; runs in run_index order)

| task | arm | passed (1/0) | wall_s | steps | non-completed status | excluded runs |
| --- | --- | --- | --- | --- | --- | --- |
| CDB-S01 | cc-cua-driver | 1 1 1 1 1 | 98.4 87.5 75.9 89.1 98.9 | 28 31 24 26 28 | - | - |
| CDB-S01 | cc-codex-cu | 1 1 1 1 1 | 52.3 52.8 63.7 55.2 52.3 | 15 17 17 15 16 | - | - |
| CDB-S02 | cc-cua-driver | 0 0 0 0 0 | 41.5 34.6 34.6 32.1 31.6 | 7 5 5 5 5 | - | - |
| CDB-S02 | cc-codex-cu | 0 0 0 0 0 | 31.6 30.2 32.0 35.7 34.4 | 5 5 5 6 6 | - | - |
| CDB-S03 | cc-cua-driver | 0 1 1 1 0 | 40.0 48.1 53.0 41.5 42.8 | 6 8 9 5 5 | - | - |
| CDB-S03 | cc-codex-cu | 1 1 1 1 1 | 53.5 43.2 41.2 41.8 43.7 | 8 6 6 6 8 | - | - |
| CDB-S04 | cc-cua-driver | 1 1 1 1 1 | 116.3 22.9 109.5 102.5 140.1 | 46 3 46 48 61 | - | - |
| CDB-S04 | cc-codex-cu | 1 1 1 1 1 | 70.2 28.7 59.8 27.5 52.0 | 23 5 19 5 17 | - | - |
| MB-09 | cc-cua-driver | 1 1 1 1 1 | 33.6 32.9 38.3 35.2 26.0 | 9 9 12 9 7 | - | - |
| MB-09 | cc-codex-cu | 1 1 1 1 1 | 76.4 138.2 154.7 35.9 52.8 | 22 40 37 10 14 | - | - |
| MB-10 | cc-cua-driver | 1 0 1 1 1 | 66.2 153.1 82.8 106.3 97.2 | 18 63 44 45 52 | - | - |
| MB-10 | cc-codex-cu | 0 0 0 0 0 | 29.2 53.2 31.8 27.0 40.5 | 9 13 9 6 10 | - | - |
| MB-11 | cc-cua-driver | 1 1 1 0 1 | 55.1 60.3 37.0 60.4 71.1 | 20 19 15 31 33 | - | - |
| MB-11 | cc-codex-cu | 0 0 0 0 0 | 82.0 36.9 55.5 42.1 66.3 | 14 7 11 10 11 | - | - |

## 7. Efficiency

Medians with [Q1, Q3] and n. Conditional = successful runs only; unconditional = all non-excluded runs. Pooled across tasks, so the task mix of each arm affects these numbers; compare per-task values in section 5 before drawing conclusions. Total tokens = input + output (assumes cached input is a subset of input and reasoning a subset of output).

### Headline tasks

| arm | basis | trials | wall_s (s) | steps | tokens in | tokens cached | tokens out | tokens total | cost (USD) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | success only | 13 | 89.1 [53.0, 102.5] (n=13) | 28.0 [9.0, 46.0] (n=13) | 44 [20, 48] (n=13) | 1029000 [164318, 1186152] (n=13) | 4398 [3886, 6371] (n=13) | 4444 [3928, 6413] (n=13) | 0.5570 [0.1478, 0.6145] (n=13) |
| cc-cua-driver | all non-excluded | 20 | 50.5 [38.7, 98.5] (n=20) | 8.5 [5.0, 28.8] (n=20) | 19 [10, 46] (n=20) | 161438 [71201, 1079252] (n=20) | 3865 [2676, 4567] (n=20) | 3895 [2688, 4614] (n=20) | 0.1437 [0.1029, 0.5890] (n=20) |
| cc-codex-cu | success only | 15 | 52.3 [42.5, 54.3] (n=15) | 15.0 [6.0, 17.0] (n=15) | 26 [14, 32] (n=15) | 205029 [106106, 270028] (n=15) | 2701 [2455, 3082] (n=15) | 2731 [2484, 3110] (n=15) | 0.1495 [0.1282, 0.1875] (n=15) |
| cc-codex-cu | all non-excluded | 20 | 43.5 [33.8, 53.0] (n=20) | 7.0 [5.8, 16.2] (n=20) | 15 [12, 30] (n=20) | 112872 [58659, 248885] (n=20) | 2540 [1853, 2816] (n=20) | 2570 [1869, 2837] (n=20) | 0.1321 [0.0643, 0.1684] (n=20) |

### All tasks (non-excluded trials)

| arm | basis | trials | wall_s (s) | steps | tokens in | tokens cached | tokens out | tokens total | cost (USD) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | success only | 26 | 68.7 [39.1, 98.1] (n=26) | 22.0 [9.0, 41.2] (n=26) | 39 [18, 48] (n=26) | 778718 [173614, 1078846] (n=26) | 3865 [1811, 6013] (n=26) | 3895 [1833, 6056] (n=26) | 0.3971 [0.1711, 0.5873] (n=26) |
| cc-cua-driver | all non-excluded | 35 | 55.1 [36.1, 93.2] (n=35) | 18.0 [7.0, 32.0] (n=35) | 30 [14, 47] (n=35) | 425585 [99343, 1070182] (n=35) | 3487 [2056, 4691] (n=35) | 3497 [2066, 4737] (n=35) | 0.2796 [0.1205, 0.5703] (n=35) |
| cc-codex-cu | success only | 20 | 52.5 [42.9, 60.8] (n=20) | 15.0 [7.5, 17.5] (n=20) | 29 [16, 35] (n=20) | 238976 [114643, 370504] (n=20) | 2722 [2246, 3361] (n=20) | 2754 [2274, 3383] (n=20) | 0.1648 [0.1301, 0.2062] (n=20) |
| cc-codex-cu | all non-excluded | 35 | 43.7 [35.0, 55.4] (n=35) | 10.0 [6.0, 15.5] (n=35) | 22 [14, 30] (n=35) | 149651 [93755, 247138] (n=35) | 2203 [1656, 2774] (n=35) | 2215 [1672, 2797] (n=35) | 0.1255 [0.0936, 0.1661] (n=35) |

## 8. Time per tool call by class

Per trial, `action_latency_ms` gives a median and p90 per action class. Here: the median of the per-trial medians (and the median of per-trial p90s) across trials, with the number of trials that timed that class. All non-excluded trials.

| class | A median of trial medians (cc-cua-driver) | A median of trial p90 | B median of trial medians (cc-codex-cu) | B median of trial p90 |
| --- | --- | --- | --- | --- |
| observe | 187.6 ms (trials=24, actions=280) | 1022.5 ms | 882.7 ms (trials=23, actions=37) | 882.7 ms |
| click | 1169.6 ms (trials=24, actions=125) | 1914.2 ms | 764.9 ms (trials=23, actions=98) | 1145.7 ms |
| type | 2907.8 ms (trials=8, actions=20) | 5807.4 ms | n/a (0 trials) | n/a |
| key | 1989.5 ms (trials=5, actions=34) | 4329.8 ms | n/a (0 trials) | n/a |
| scroll | 290.2 ms (trials=9, actions=11) | 290.2 ms | 1024.5 ms (trials=5, actions=7) | 1026.2 ms |
| drag | 1664.4 ms (trials=9, actions=19) | 2496.1 ms | 1333.9 ms (trials=11, actions=79) | 1355.6 ms |
| set_value | 2075.8 ms (trials=6, actions=19) | 3160.8 ms | 1587.6 ms (trials=1, actions=1) | 1587.6 ms |
| other | 175.9 ms (trials=11, actions=122) | 187.8 ms | 89.3 ms (trials=22, actions=67) | 451.4 ms |

## 9. Disturbance

Only non-excluded trials with `disturbance.available` true and `human_input_suspected` false are used. Cells show mean / max per trial. hid_events total = move + down + key + scroll. Any disturbance = front_changes, key_loss, keystrokes_leaked or clicks_leaked > 0, or any hid event, or pointer_deviation_episodes > 0.

| arm | trials used | dropped: human_input_suspected | unavailable | front_changes | key_loss | keystrokes_leaked | clicks_leaked | scrolls_leaked (not in 'any') | hid_events total | pointer_max_deviation_px | any disturbance [Wilson 95%] |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cc-cua-driver | 22 | 13 | 0 | 0.23 / 2.00 | 0.14 / 1.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 56.7 / 498.0 | 4/22 = 0.18 [0.07, 0.39] |
| cc-codex-cu | 35 | 0 | 0 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.00 / 0.00 | 0.0 / 0.0 | 0/35 = 0.00 [0.00, 0.10] |

## 10. Data-quality flags

| flag | value |
| --- | --- |
| trials with passed=true but status != completed (counted as failures) | none |
| excluded trials with no excluded_reason | none |
| infra_error trials NOT excluded (counted as failures) | cc-cua-driver: 0, cc-codex-cu: 0 |
| human_input_suspected (non-excluded; dropped from disturbance only) | cc-cua-driver: 13, cc-codex-cu: 0 |
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
| cc-cua-driver | 35 | 26 | 11.92 | 0.341 (median 0.28, n=35) | 1096 | 132895 | 21578077 | 1567760 | 15.7 (median 15, n=35) | 1.11 (median 1, n=35) | 7.87e+03 (median 8.66e+03, n=35) | 29 |
| cc-codex-cu | 35 | 20 | 5.03 | 0.144 (median 0.125, n=35) | 882 | 86962 | 7523927 | 663240 | 12.6 (median 11, n=35) | 0.857 (median 1, n=35) | 6.45e+03 (median 7.25e+03, n=35) | 28 |

Trial status by arm: cc-codex-cu completed=35, cc-cua-driver completed=35
- 7-day quota used per trial, cc-cua-driver: mean 0.0011 over 35 trials (the login is shared, so other sessions add noise)
- 7-day quota used per trial, cc-codex-cu: mean 0.0009 over 35 trials (the login is shared, so other sessions add noise)
- pauses: 2 totalling 0.1 min (see pauses.jsonl)
