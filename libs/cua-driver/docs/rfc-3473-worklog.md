# RFC 3473 worklog

Decision record: [RFC 3473](https://github.com/trycua/cua/issues/3473).
Execution: [draft PR #3616](https://github.com/trycua/cua/pull/3616).
Design: [tests-first invariant specification](rfc-3473-slice-a-spec.md).

## Current selection

The maintainer selected tests against existing code, including failing desired
invariants, followed by canonical Lume E2E and evidence review. No production
refactor is selected. The initial owner/record/lease proposal was replaced with
a test-first plan; it must not be treated as accepted implementation guidance.
The RFC itself still awaits a maintainer decision.

Branch: `test/rfc-3473-snapshot-invariants`, canonical repository head.
Test candidate: `d923193a906707a425f4ec158d966c8d34709d48`.
Base: `ed289df50257bd6a65f9ee7964bb842777a1a10a`.
Subsequent documentation changes do not change the tested executable sources.

## Actionable items

| ID | Action | Status | Exit evidence |
| --- | --- | --- | --- |
| T0 | Make selection visible and open one linked draft PR | Complete | #3473 scope reply; #3616 |
| T1 | Add shared invariant tests against existing components | Complete for initial seven cases | 3 pass, 4 fail; no masked failures |
| T2 | Add admitted macOS native lifetime coverage | Complete | New CF test and two existing cache tests pass |
| T3 | Extend canonical AppKit stale-ref row with both target forms and fresh recovery | Written; execution blocked | Test binary compiles; external counter oracle implemented |
| T4 | Run one exact-SHA canonical Lume matrix | Blocked | No verified prepared seed in configured host inventory |
| T5 | Review red tests at the actual dispatch/lifecycle boundaries | Pending | Determine reachability and smallest deletion-oriented changes; do not patch synthetic composition alone |
| T6 | Add missing SDK shutdown/cancellation, native runtime isolation, Windows geometry and sparse Linux membership cases | Pending | Deterministic tests against actual owning paths |
| T7 | Freeze performance baselines before any production edits | Pending | Exact SHA, environment and benchmark manifest |
| A | Select a minimal desktop simplification from evidence | Unselected | Maintainer review; explicit deletion inventory; RFC decision |
| B | Migrate typed action families across supported backends | Unselected | Family fixtures, direct records, legacy branches deleted |
| C | Consolidate browser snapshot ownership if accepted | Unselected | Binding/lifecycle/continuation parity, nested ownership deleted |
| D | Certify affected production candidate and release path | Unselected | Canonical platform matrix, compatibility/performance gates, release metadata and post-merge smoke |

No child issues or revived investigation milestones. Keep #3473 as the decision
record and #3616 as the execution record. This intentionally red PR is not ready
to merge.

## Evidence log

### 2026-09-05 — Initial planning

Read the RFC and source at the pinned baseline; drafted an owner/lease-oriented
spec and worklog. The RFC's 67 macOS checks were reported historical evidence,
not tests rerun by this session. The maintainer subsequently rejected treating
an additive abstraction as the starting point.

### 2026-09-07 — Tests-first baseline

Refreshed #3473 (still open, assigned to `injaneity`, no new decision) and searched
for active PRs mentioning 3473 before starting. Posted selection and opened #3616.
No production behavior changes; `platform-macos/src/ax/cache.rs` changes are
inside its existing test module only. Existing unrelated `uv.lock` and Kotlin
`.gradle/` changes remain untouched and uncommitted.

Installed the repository-pinned Rust 1.97.1 toolchain on the available macOS host;
no repository dependency or lockfile change was required.

| Check | Result | Interpretation |
| --- | --- | --- |
| `cua-driver-core --test snapshot_invariants` | 3 passed, 4 failed | Desired component-level invariants, deliberately red |
| `cua-driver-core --lib element_token::tests` | 20 passed | Existing token behavior preserved |
| `platform-macos --lib ax::cache::tests` | 3 passed | Includes admitted handle surviving cache destruction until worker completion |
| `cua-driver --test harness_appkit_test --no-run` | Passed | Compilation only; no GUI delivery claim |
| Canonical Lume matrix | Not run: environment blocked | No eligible golden seed; not a product failure or skip-to-green |

The four failures are:

- Empty snapshots admit index zero through the registry.
- Token resolution followed by separately scheduled cache lookup can observe the
  replacement payload in the composed-component test.
- Registry retirement alone does not release the separate unadmitted payload.
- Registry eviction alone does not release the separate unadmitted payload.

The last three expose missing composition guarantees, not proof that actual
public dispatch or full SDK shutdown permits the same behavior. Review real
scheduling and lifecycle paths before selecting a production fix. The test suite
is intentionally incomplete; T6 lists the missing boundaries.

Local logs (ignored artifacts, not checked-in or remotely published):

```text
artifacts/cua-driver/rfc-3473/core-invariants-candidate.log
artifacts/cua-driver/rfc-3473/token-baseline.log
artifacts/cua-driver/rfc-3473/native-cache.log
artifacts/cua-driver/rfc-3473/appkit-build-retry.log
```

The first AppKit compile attempt lost its tool-host connection without a final
status. Only that incomplete compile was retried; it passed. Native builds emitted
existing duplicate Swift bridge-symbol warnings. No failure was retried to green.

### Lume blocker

Lume 0.5.3 is available at `~/.local/bin/lume`, outside the initial PATH. Configured
storage contains only the stopped `cua-driver-macos-e2e-pr3373-20260904T173717Z`
worker. No prepared immutable seed/backups or matching provenance record was
found. That other workstream's VM was not booted, modified, or reused.

To unblock T4, provide a verified prepared seed/storage with its maintainer record,
or authorize preparing a new seed and its human signing/TCC consent stages. Run
from Terminal in a disposable guest's logged-in desktop using the canonical
wrapper; never bypass preflight or run host tests as a substitute.

## Coordination

Revalidate [#2075](https://github.com/trycua/cua/pull/2075) before touching Windows
cache behavior; its freshness change is separate and contributor credit must be
preserved. Snapshot-related #3005, #3377 and #3573 were identified during initial
planning; refresh their actual state/diffs before any production change.

## Maintenance

Update status, exact tested SHA, evidence, gaps and next action after each
substantive step, and keep the PR description aligned. Record decisions in #3473,
not only locally. No performance improvement, native E2E pass, cross-platform
certification or architecture acceptance is claimed.

Next action: unblock the Lume seed, run the stable test candidate once, and review
its evidence alongside the red component tests before selecting simplifications.
