# Results of 6 Oct 2026

Runs of `PREREGISTRATION.md` (Amendments 1 and 2). Two runs on one macOS VM: `main` (CDB-S01 to S04 with coding tools and the probes MB-09 to MB-11, 70 counted trials) and `gui` (the GUI-only variant CDB-G02 to G04, 30 counted trials plus one rate-limited attempt that is not counted).

| File | What |
|---|---|
| `main/results.public.jsonl`, `gui/results.public.jsonl` | One row per attempt. CDB rows are scrubbed (`tools/scrub_results.py`): no check names, no agent text, no evaluator diagnostics, only counts of checks passed and failed. The CDB task pack is proprietary and stays private. |
| `main/excluded-harness-bug.public.jsonl` | The three attempts of CDB-S02 run 1 that ended as harness exceptions (adapter bug, fixed in `5530ba708`); not counted, the pair was rerun |
| `*/launcher.log`, `*/pauses.jsonl` | Preflight output, trial lines and pauses |
| `gui/account-switch.txt` | The credential switch during the GUI-only run |
| `analysis_*.md/json` | `analyze_bench.py` output for the CDB suite, the probes and the GUI-only variant (headline group in each) |
| `table_*.md` | Per-task, per-arm tables (`tools/report_tables.py`) |

Quota readings (seven-day utilization of the seat the runner used): 0.19 at the first trial (08:57 UTC), 0.26 at the end of `main`, 0.29 before the five-hour window of the first seat was rejected at 11:20 UTC, 0.85 to 0.86 on the second seat from 11:54 UTC. The stop line was 0.95.

The full report, with failure analysis and recordings, is private. Raw streams, recordings and the proprietary task material are not in this repository.
