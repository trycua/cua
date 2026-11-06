# Provenance: PA-01

**Parallel agents on many copies of one app.** An original Cua task, written for this benchmark (first committed
2026-11-06). It runs in BenchLab's `desk` mode, one process per copy, on macOS ([`../../swift`](../../swift)), Linux ([`../../linux`](../../linux)) and Windows ([`../../windows`](../../windows)). It
is checked by [`evaluate.py`](evaluate.py) from the app's own state and event
log.

* **Author:** Cua AI, Inc. The task, its brief, its seeded data, the BenchLab
  mode it uses and its checker are original. Names, values and labels are
  generated from the trial seed or invented.
* **Sources:** none. No task text, data, code or asset comes from another
  benchmark, a third party or a customer corpus. BenchLab uses system fonts
  only and draws its icons in code.
* **Related work:** Running several agents at once on one computer is a deployment pattern, not a benchmark task we know of; the closest public work measures one agent per machine (OSWorld, WindowsAgentArena). Nothing was taken from any benchmark.
* **Review:** design note in [`../../SHARED-DESKTOP.md`](../../SHARED-DESKTOP.md).
* **Licence:** FSL-1.1-MIT, under the benchmark's [LICENSE](../../../../LICENSE).
