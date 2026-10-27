
## Fix round 1

- Launcher now runs the command in its own session, records its process group in the ledger, kills a leaked group after the check, and keeps the supervising wrappers alive on cancellation until the command's teardown finishes.
- README example: work dir outside the checkout, same `--arms` for preflight and run, `libglib2.0-bin` listed.
- Added regressions: unregistered background child fails the check; SIGTERM waits for delayed cleanup of a separately sessioned child.
- The scoped repository checks pass (329 passed, 30 skipped); launcher tests pass with stubbed desktop components.
