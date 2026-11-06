# Provenance: UM-01

**The user's mouse keeps working while the agent works in the background.** An original Cua task, written for this benchmark (first committed
2026-11-06). It runs in BenchLab's `desk` mode while BenchSentinel's mouse user moves and clicks the real pointer from a seeded script. It
is checked by [`evaluate.py`](evaluate.py) from the app's own state and event
log.

* **Author:** Cua AI, Inc. The task, its brief, its seeded data, the BenchLab
  mode it uses and its checker are original. Names, values and labels are
  generated from the trial seed or invented.
* **Sources:** none. No task text, data, code or asset comes from another
  benchmark, a third party or a customer corpus. BenchLab uses system fonts
  only and draws its icons in code.
* **Related work:** The mouse user extends the typing user of the background condition (Amendment 15) and the undisturbed criteria of Amendment 22. No public benchmark we know of simulates a person using the pointer during an agent's task; nothing was taken from one.
* **Review:** design note in [`../../SHARED-DESKTOP.md`](../../SHARED-DESKTOP.md).
* **Licence:** FSL-1.1-MIT, under the benchmark's [LICENSE](../../../../LICENSE).
