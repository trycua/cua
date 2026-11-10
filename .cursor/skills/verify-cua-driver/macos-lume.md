# macOS lane: Lume guest

Use this lane for any change to macOS or AppKit code: `platform-macos`, the AppKit/SwiftUI/TextEdit harnesses, `scripts/ci/macos/**`, or `libs/cua-driver/tests/runners/macos-lume/**`. It runs the canonical guest entry point, `libs/cua-driver/tests/runners/macos-lume/run-all.sh`, inside a disposable clone of a stopped, already-granted Lume seed. The seed contract and the by-hand flow are in [the runner README](../../../libs/cua-driver/tests/runners/macos-lume/README.md); this page covers only how the verify skill drives it.

Do not run the AppKit E2E tests directly on the maintainer host's own desktop. That desktop isn't disposable, its TCC grants belong to whoever installed it, and on the relay Mac mini the fixture hangs (see [Host notes](#host-notes)).

## Run

The Mac with Lume has an installed wrapper, `~/.local/bin/lume-e2e`, configured by `~/.local/share/cua-lume-e2e/config.json`. It clones the configured seed and boots the worker. It then copies a `git archive` of the exact SHA, starts `run-all.sh` from the guest's GUI Terminal (not over SSH), pulls back the evidence and stops the worker. It never deletes a VM. The skill helper wraps it:

```bash
H=.cursor/skills/verify-cua-driver/scripts/verify-macos.sh
$H doctor                                              # host + seed readiness, no VM started
$H --source /abs/clean/checkout focus <cell>           # one registered cell, one attempt
$H --source /abs/clean/checkout full                   # full matrix (about an hour)
$H --source /abs/clean/checkout full --standalone-browser
```

`--source` defaults to this checkout. The source must be a clean git root, and its `HEAD` is the SHA under test. Use a detached worktree at the PR head, not a working branch with edits. The helper runs `lume-e2e --doctor` first. The wrapper prints `evidence=<run dir>` before it clones and `result.json` at the end. Exit 0 means the runner passed and the wrapper verified the evidence and the standard-daemon restore. Exit 1 means a test or infrastructure failure. Exit 2 means it was blocked before execution.

Main has no `scripts/ci/macos/appkit-cells.tsv` yet, so on main an AppKit change runs `full`. The cells you can focus on main are `macos-textedit-type-text-ax-background`, `macos-{electron,tauri,wkwebview}-scroll-ax-{foreground,background}` and `macos-{electron,tauri,wkwebview}-left-click-ax-background-typed-sdk`. Pick the one whose route the diff changes. On a branch that carries the manifest, its AppKit names can be focused too. A focused pass is `mode: focused`, `focused_passing_cells: 1`. It is evidence for that cell only, never full-matrix certification.

A Mac without the wrapper has to follow the runner README by hand: clone a worker, run `sync-vm-worktree.sh push`, run `run-all.sh` in the guest Terminal, run `pull-artifacts`, then stop the worker.

## Evidence

Read `<run dir>/result.json` first: `sha`, `mode`, `status`, `runner_exit`, `clone_source`, `clone_source_note`, `evidence_collected`, `worker_stopped`. The guest log is `evidence/runner.log`. The canonical reports are under `evidence/source/artifacts/cua-driver/macos/`: `summary.md`, `results.jsonl`, `failures.json`, `retry-record.json` for focused runs, and per-cell `recordings/` with MP4s. Check that `sha` is still the PR head before you claim a result. A moved head is untested.

## Seed

The wrapper clones `seed` from `config.json` and records `seed_note` in every result. On the relay Mac mini the seed is `cua-driver-browser-baseline-20261104-tk86-ax`: Tk 9.1, Chrome, and the Terminal Accessibility grant below. Its parent, `cua-driver-browser-baseline-20261104-tk86`, was deleted on 2026-11-10, and the seed still clones, and its clones boot, without it. The seed must be stopped; never boot or edit it for a run. To change it, clone it to a new name, make the one change in the clone, stop the clone, and point `config.json` at it with an updated `seed_note`.

The runner stops at exit 2 when the seed lacks one of these. The seed steps are in the [runner README](../../../libs/cua-driver/tests/runners/macos-lume/README.md#create-the-private-seed):

- **Tk 8.6 or later** (`require_modern_tk`). Apple's `/usr/bin/python3` has Tk 8.5.9. `brew install python-tk` puts Homebrew's `python3` first; the current seed has Tk 9.1.
- **Terminal → System Events Automation** ("Terminal cannot control System Events").
- **Terminal Accessibility** ("grant Terminal Accessibility in the seed"). The switch in System Settings looks on before it is saved. It persists only after the admin-password dialog accepts the guest password; then the runner's `AXRole` probe returns `AXApplication`. With no human at the display, the guest's own `cua-driver-local` can do the clicks and typing over SSH.
- **CuaDriverLocal** Accessibility, Screen Recording, direct capture and a stable signing identity.

## Host notes

On the relay Mac mini:

- **AppKit fixture hang.** On the host desktop, `~/Library/Application Support/com.apple.sharedfilelist` returns `Interrupted system call` (EINTR). The AppKit fixture's main thread then blocks in the Apple menu's recent-items lookup, and AX calls return `-25204`. `harness_appkit_smoke` fails on main this way too. Run AppKit cells in the Lume guest only. Host-side `cargo test` for `platform-macos` unit tests is fine.
- **Root-owned `~/.npm`.** Host-side `npm ci` and `npm install` fail with `EACCES`. Pass a private cache, for example `npm ci --cache "$(mktemp -d)"`, and delete it afterwards. The guest is unaffected.
- `lume ls` lists every past `cua-e2e-*` worker. The wrapper only stops them, so don't treat a stopped worker as the seed, and don't prune VMs from a verify run.
- **Blocked before a VM starts.** Doctor refuses under `minimum_free_gib` (80) of free disk, and a second run refuses while another holds `run.lock`. Report the lane as blocked with that reason, not as having no guest.
- **Lume 0.5.3 `lume stop` kills itself, not the VM,** so the run ends `worker_stopped: false` and `incomplete` even when every cell passed. Read `runner_exit` and `summary.md` for the test result. Stop that worker with `kill -TERM` on its exact `lume run <worker>` pid, never `lume serve`. Lume on main finds the owner from a recorded file instead, so updating the installed lume fixes it.

## Report

The `verify:` line names the lane, the mode and the cell, for example `macOS Lume focused macos-appkit-native-geometry-mismatch-px-background`. Name what was not run: the full matrix, standalone browsers, SIP-on permission flow, Linux or Windows.
