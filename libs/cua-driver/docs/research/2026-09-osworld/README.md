# OSWorld dogfooding, September 2026

Research notes behind the Linux delivery changes in this branch. They were
written during the work, so they read as a log rather than a spec.

1. [Eval: stock driver vs the classic pyautogui loop](01-eval-stock-driver-vs-pyautogui.md).
   Same model, tasks, image and step budget: 80 % vs 13 %. Each cause was
   reproduced in isolation on a fresh VM.
2. [Optimization log](02-optimization-log.md). Foreground rewrite, AT-SPI
   budgets and partial-tree labels, the daemon deadlock, coordinate frames,
   and the MPX + uinput background route, with every run's numbers.
3. [Papercuts](03-papercuts.md). 44 numbered findings across cua-sandbox,
   Fleet, and cua-driver, each with the exact input, timing, and screenshot
   evidence where it existed.

Harness and diagnostics referenced by these notes are in the session's
scratchpad; the guide for running OSWorld on Fleet or locally is
`docs/content/docs/how-to-guides/sandbox/run-osworld-on-cloud-fleet.mdx`.
Paths such as `osworld-on-cua/...` and `diag-*/` in the notes refer to the
author's working folder, not to this repository.
