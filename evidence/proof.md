
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
