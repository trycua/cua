
## Task 2 Execution Report - 2026-08-09

- Commit: `a71f26ef15c2cd0f956de0c854c25cd7d6ec3ced` (`test(cua-sandbox): add live Fleet ephemeral smoke`)
- Scope: added only `libs/python/cua-sandbox/tests/live/test_fleet_ephemeral.py` and the offline OAuth credential-gating test in `libs/python/cua-sandbox/tests/test_live_fleet_e2e_support.py`. No workflow or rollout files changed.

### TDD Evidence

- RED: `cd libs/python/cua-sandbox && .venv/bin/python -m pytest -q tests/test_live_fleet_e2e_support.py::test_live_test_requires_both_oauth_values` failed with `ModuleNotFoundError: No module named 'tests.live.test_fleet_ephemeral'` after adding the credential-gating test and before creating the live module.
- GREEN: the same focused test passed after creating the live module: `1 passed in 0.02s`.

### Credential-Free Validation

- `cd libs/python/cua-sandbox && env -u CUA_CLIENT_ID -u CUA_CLIENT_SECRET .venv/bin/python -m pytest -q tests/live/test_fleet_ephemeral.py`: `1 skipped in 0.01s`.
- `cd libs/python/cua-sandbox && env -u CUA_CLIENT_ID -u CUA_CLIENT_SECRET .venv/bin/python -m pytest -q tests/test_live_fleet_e2e_support.py tests/live/test_fleet_ephemeral.py`: `12 passed, 1 skipped in 0.03s`.
- No live Fleet calls were made; both OAuth values were explicitly removed for collection and validation.

### Self-Review

- Preserved the approved pinned Duo image digest, `server_port=8000`, CPU/memory, startup/request timeouts, and disabled telemetry.
- Preserved the template-port, 1024x768 screen, PNG signature/size, and Linux shell assertions.
- Preserved sanitized summary output, normal cleanup wait, resource inventory, emergency cleanup, primary-error handling, leak failure behavior, and HTTP client closure.
- `git diff --check` and `git diff --cached --check` completed without whitespace errors before commit.

### Concerns

- The brief's historical aggregate expectation is `6 passed, 1 skipped`; the current approved Task 1 support suite contains additional tests, so the actual credential-free aggregate result is `12 passed, 1 skipped`.
- The live scenario remains intentionally unexecuted because validation was required to be credential-free. The report itself is intentionally uncommitted, as requested after recording the Task 2 commit SHA.

## Task 2 Review Remediation - 2026-08-10

- Commit: `645d7d15a937b13c226a686d4bf0a156e28a8433` (`test(cua-sandbox): harden live Fleet cleanup`)
- Scope: remediated only the Task 2 live Fleet pytest and its credential-free support tests. Task 3, workflows, and live rollout behavior were not started or changed.

### TDD Evidence

- RED: `test_selected_namespace_rejects_unsafe_override` failed because `selected_namespace` did not exist; GREEN passed after adding the generated-name/prefix guard.
- RED: `test_existing_safe_namespace_never_provisions_or_deletes` failed because the runner lacked `namespace_exists`; GREEN passed after checking absence before `Sandbox.ephemeral` and gating cleanup on a creation-attempt flag.
- RED: `test_cleanup_failure_does_not_mask_primary_failure` failed for both polling and inventory with `CleanupFailure` replacing `PrimaryFailure`; GREEN passed after recording the sanitized cleanup error type and suppressing it when a primary error exists.
- RED: `test_summary_write_failure_still_closes_and_preserves_primary_error` failed with `SummaryWriteFailure` replacing the primary error before close; GREEN passed after closing before summary output and preserving the primary error.
- RED: `test_close_failure_does_not_mask_primary_error` failed with `CloseFailure` replacing the primary error; GREEN passed after recording the sanitized close error and propagating it only when no primary error exists.

### Final Credential-Free Validation

- `cd libs/python/cua-sandbox && env -u CUA_CLIENT_ID -u CUA_CLIENT_SECRET .venv/bin/python -m pytest -q tests/test_live_fleet_e2e_support.py tests/live/test_fleet_ephemeral.py`
- Result: `18 passed, 1 skipped in 0.05s`.
- No live Fleet calls were made; OAuth credentials were explicitly removed.

### Self-Review

- Overrides must retain the dedicated `cua-live-` prefix; all selected namespaces are checked absent through the public support `namespace_exists()` before provisioning.
- Emergency cleanup runs only after a verified-absent namespace reaches the creation-attempt path; pre-existing namespaces neither provision nor delete.
- Primary errors retain precedence over cleanup, summary-write, and close failures; sanitized error type metadata is captured in the summary where writing can succeed.
- The pinned Duo image, port `8000`, resource/timeout/telemetry settings, functional assertions, summary sanitization, and leak behavior remain unchanged.

### Concerns

- Live Fleet execution remains intentionally unperformed because validation is credential-free.
- A failed summary write cannot itself be persisted to that failed output file; the runner still closes the HTTP client and preserves any primary error.

## Task 2 Ownership-Boundary Remediation - 2026-08-10

- Commit: `e7a3221c03d86586382c52c6a7f2c519d98f79e0` (`fix(cua-sandbox): gate Fleet cleanup by ownership`)
- Scope: added the public Fleet namespace-ownership signal, gated Task 2 emergency namespace cleanup on that signal, hardened override validation and failure precedence, and added only focused transport/Sandbox/live support coverage. Task 3 and workflow/rollout changes were not started.

### TDD Evidence

- RED: public ownership tests failed because neither `FleetCloudTransport` nor `Sandbox` exposed `owns_namespace`; GREEN passed after adding read-only properties with false for non-Fleet transports.
- RED: DNS override cases accepted uppercase, trailing-hyphen, overlong, and underscore names; GREEN passed after full DNS-1123/63-character validation.
- RED: a preflight-absent attached-race simulation invoked emergency cleanup, and pre-yield provisioning failure invoked emergency cleanup; GREEN passed after recording ownership only from the yielded public `Sandbox.owns_namespace` signal.
- RED: cleanup/leak errors were replaced by close and/or summary failures; GREEN passed for cleanup+close, cleanup+summary, and cleanup+close+summary after phase-ordered error collection.

### Final Credential-Free Validation

- `cd libs/python/cua-sandbox && .venv/bin/python -m pytest -q tests/test_fleet_cloud_transport.py`: `33 passed in 0.06s`.
- `cd libs/python/cua-sandbox && env -u CUA_CLIENT_ID -u CUA_CLIENT_SECRET .venv/bin/python -m pytest -q tests/test_live_fleet_e2e_support.py tests/live/test_fleet_ephemeral.py`: `27 passed, 1 skipped in 0.07s`.
- No live Fleet calls were made; OAuth credentials were explicitly removed for the Task 1/2 suite.

### Self-Review

- Fleet reports ownership only after a successful namespace creation; a 409 create race that attaches to an existing namespace reports false.
- The live test captures ownership only inside a successfully-entered `Sandbox.ephemeral` context. It never whole-namespace-deletes after a pre-yield provisioning failure or when a raced/attached namespace is not owned.
- Primary body/provisioning failure wins over cleanup/leak, close, and summary-write failures; cleanup/leak then wins over close, which wins over summary-write. Secondary error summaries record sanitized type names where possible.
- Preserved the approved image digest, port 8000, resource/timeouts/telemetry settings, assertions, artifact output, summary sanitization entry point, and credential gate.

### Concerns

- Live Fleet execution remains intentionally unperformed because validation is credential-free.
- `tests/test_vm_cleanup.py` was attempted as broader Sandbox coverage and reported 8 existing fixture/setup failures: its affected tests patch `FleetCloudTransport` while passing a legacy `api_key`, which routes to `CloudTransport`. This remediation does not alter that routing or those fixtures.
- A summary-write error cannot be persisted to the output file whose write failed; the runner still closes the client and preserves any earlier error.

## Task 2 User-Approved Option 1 Remediation - 2026-08-10

### Architectural Decision

- User approved Option 1 after three reviews established that Fleet's name-only namespace deletion cannot be made race-safe without an immutable conditional-delete identity.
- The live Fleet smoke test now relies exclusively on `Sandbox.ephemeral` for owned resource cleanup. After the context exits, it polls for namespace absence; a remaining namespace triggers sanitized inventory collection, records the leak in the summary, and fails without issuing an explicit whole-namespace deletion.
- Removed the Task 2-only public `Sandbox.owns_namespace` and `FleetCloudTransport.owns_namespace` APIs. Fleet's private ownership state remains internal to `FleetCloudTransport` automatic cleanup.

### TDD Evidence

- RED: `test_live_cleanup_exposes_no_explicit_namespace_deletion_api` failed because `cleanup_namespace` and both public `owns_namespace` properties remained available.
- RED: leak/recreation coverage failed because the runner still read `sandbox.owns_namespace`; owned-leak close/summary precedence tests failed for the same obsolete dependency.
- GREEN: removed the explicit deletion helper/import, ownership reads, and public APIs. Leak paths set `namespace_leak`, collect inventory, and fail; pre-yield provisioning failures never enter cleanup; primary failures still precede cleanup/leak, close, and summary errors.

### Final Credential-Free Validation

- `cd libs/python/cua-sandbox && .venv/bin/python -m pytest -q tests/test_fleet_cloud_transport.py`: `30 passed in 0.08s`.
- `cd libs/python/cua-sandbox && env -u CUA_CLIENT_ID -u CUA_CLIENT_SECRET .venv/bin/python -m pytest -q tests/test_live_fleet_e2e_support.py tests/live/test_fleet_ephemeral.py`: `26 passed, 1 skipped in 0.07s`.
- OAuth credentials were explicitly removed for the Task 1/2 suite; no live Fleet calls were made.

### Self-Review

- The live test does not import, call, or expose `cleanup_namespace`; it never explicitly deletes a namespace after `Sandbox.ephemeral` exits.
- DNS validation, Fleet template port assertions, credential gating, sanitized summary output, cleanup/error precedence, client close, and summary-write coverage remain intact.
- The remediation stays within Task 2. Task 3 and CI monitoring were not started.

### Concerns

- Live Fleet execution remains intentionally unperformed because validation is credential-free.
- A detected namespace leak is intentionally diagnostic-only: it collects sanitized inventory, records the leak, and relies on owned automatic cleanup plus Alertmanager rather than risking a racing name-only namespace deletion.
