
## Fix round 1

- Launcher now runs the command in its own session, records its process group in the ledger, kills a leaked group after the check, and keeps the supervising wrappers alive on cancellation until the command's teardown finishes.
- README example: work dir outside the checkout, same `--arms` for preflight and run, `libglib2.0-bin` listed.
- Added regressions: unregistered background child fails the check; SIGTERM waits for delayed cleanup of a separately sessioned child.
- The scoped repository checks pass (329 passed, 30 skipped); launcher tests pass with stubbed desktop components.

## Fix round 2

- Launcher: a failed ledger or `command.pid` publication now kills and reaps the new command group and exits 1, so no child is left unowned.
- Launcher: `CDB_SESSION_TIMEOUT` no longer wraps the session in `timeout`. A timer in the launcher now sends the same forwarded TERM as cancellation, so the command finishes its own cleanup while the wrappers stay up. The status is 124 and `exit` records `command=124`.
- Added `test_timeout_waits_for_delayed_cleanup` (separately sessioned child, delayed TERM cleanup).
- The scoped repository checks passed: 329 passed, 31 skipped, em-dash check clean.
- Not run: the real-desktop launcher tests. Xvfb dependencies (openbox, picom, gdbus) are absent here, so they skip. The script passes `bash -n`. No launcher-level regression was added for the ledger-write failure, since it cannot be forced without failing the earlier component writes.

## Fix round 3

- Doctor: SIGTERM is deferred while the private driver is being started and handed over, and raised only once cleanup owns the daemon. Regression `test_doctor_sigterm_at_startup_handoff_reaps_daemon` fails before the change (daemon left alive) and passes after.
- Launcher: `CDB_SESSION_TIMEOUT` and `CDB_SESSION_GRACE` must be finite; `nan` and `inf` exit 2 with no children. Covered in `test_evidence_status_reuse_and_usage` (red before, green after).
- Ledger: entries carry the process start time; the checker ignores an entry whose pid now belongs to a process with a different start time, so reused evidence directories and recycled pids no longer give false leaks. Regression `test_recycled_pid_is_not_the_recorded_child` (red before, green after).
- Proof correction: against the earlier launcher (6daeee479), `test_timeout_when_leader_exits_before_term_ignoring_child` passes; it is additional coverage and a mutation check (leader-only stop), not a regression that was red there. The automated base probe was skipped (merge in range), so it supplies no red evidence.
- Removed the vacuous empty-directory assertion in `test_noop_without_variable`.
- Not run: the full four-arm Linux run. It needs the desktop packages (openbox, picom, gdbus), an agent CLI and a built driver, none of which this host has; it remains to be done in a provisioned environment before merge.

## Fix round 4

- Doctor cancellation is scoped to one invocation. A small scope object replaces the module-global `_cancel`; `main()` restores the previous scope and SIGTERM handler on every exit path. A deferred cancel is now also honoured when the private daemon start fails with a handled error. Regression `test_doctor_cancel_then_retry_starts_cleanly` (cancelled doctor start, then a healthy `start_driver()` and a second doctor run in the same process) fails against the retained head with a stale `SystemExit(143)` and passes now. `test_cancel_helpers_are_inert_outside_doctor` covers the no-op outside a doctor run.
- The real-desktop launcher suite ran for real: openbox, picom, x11-utils, libglib2.0-bin and the other desktop packages were installed. `LinuxLauncherTest` has no skips. Run bare, 7 of 8 pass and the one that needs a display skips; run inside the launcher, `test_linux_session.py` and `test_linux_tools.py` give 55 passed, 0 skipped, and the launcher `exit` reads `command=0 evidence=0`.
- The first real preflight found two defects the stubbed tests could not, both fixed with regressions that fail against the retained head and pass now:
  - With one explicit Linux driver, `cc-cua-driver` kept `run_script` enabled, so preflight rejected its tool list. The `--cua-bin` override now disables `run_script` for every arm except the script arm (`LinuxSingleBuildTest`).
  - The Linux capture check looked at an empty desktop and rightly rejected the uniform frame. It now shows a BenchLab window for the duration of the check and stops it afterwards. `LinuxPreflightCaptureTest` runs `preflight()` on a real isolated desktop (inside the launcher), takes a real screenshot, validates it with the runner's own capture check (valid, non-uniform), and asserts the window is running during the capture and gone afterwards. With the capture-subject integration removed from `preflight()` it fails (the frame is taken with no window); at head it passes. `LinuxCaptureSubjectOffLinuxTest` covers that macOS is unchanged. The offline-capture flow test stubs the helper.
- One earlier full-run attempt was discarded and rerun from scratch: the CDB tasks failed in the harness because `npx` was not on the session `PATH`, which the evidence check reported as `recording not started`. That was an environment error in the run setup, not a repository defect.
- Full run: one driver built from source at the base commit, pins written from the in-session doctor, the four Cua Driver setups, both conditions (foreground and background), default phase counts, each inside the launcher with `--check-run` on its run directory. Both run directories reached `DONE` and both launcher `exit` files read `evidence=0`. No trial was excluded.
- Skipped, not failed: `CDB-S03` (`CDB-G03` in Bench v2) needs GnuCash, which this host does not have. It was excluded from the run and is recorded as skipped. All other Bench v2 tasks ran.
- The scoped repository checks pass (355 passed, 24 skipped; em-dash check clean).
- Not run here: macOS-only bench paths (Swift BenchLab) and Windows; PR CI covers them. Run results stay outside the repository.

