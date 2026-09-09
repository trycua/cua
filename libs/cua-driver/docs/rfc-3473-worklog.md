# RFC 3473 worklog

Decision record: [RFC 3473](https://github.com/trycua/cua/issues/3473).
Execution: [draft PR #3616](https://github.com/trycua/cua/pull/3616).
Design: [desktop vertical-slice technical specification](rfc-3473-slice-a-spec.md).

## Current selection

The maintainer subsequently selected implementation of the simplified desktop
slice, recorded in the [scope reply](https://github.com/trycua/cua/issues/3473#issuecomment-5573495529).
The implementation evolves the runtime-owned cache, removes independent token
validity, and reuses native guards rather than introducing whole-record leases.
Acceptance requires reduced production cyclomatic complexity, net non-test code
deletion, regression evidence, and measured latency (prefer improvement).
Browser and typed-action migration, and general cancelled-blocking-worker draining,
remain outside this selection. The complete parent RFC is not thereby accepted.

Branch: `test/rfc-3473-snapshot-invariants`, canonical repository head.
Test candidate: `d923193a906707a425f4ec158d966c8d34709d48`.
Base: `ed289df50257bd6a65f9ee7964bb842777a1a10a`.
Later implementation/test evidence was collected from a working-tree overlay
identified by source hashes in its artifact directories. The draft PR records
the immutable implementation revision when that overlay is published. The native baseline was the installed version
0.23.2 binary identified below, not a source-certified build of this test commit.

## Deferred drain diagnostic

At the maintainer's request, removed the newly added
`sdk_shutdown_waits_for_native_capture_after_caller_cancellation` test and its
otherwise-unused completion flag from this slice. This was our investigation
probe, not a test inherited from main. The finding remains documented: shutdown
can return while a cancelled caller's native worker is still running, including
on the tested pre-refactor source. This change does not fix that behavior.

The diagnostic remains recoverable from
[the earlier implementation commit](https://github.com/trycua/cua/blob/b1a74850e7ebbc08769b78d81b8de26333c67a80/libs/cua-driver/rust/crates/cua-driver-sdk/src/snapshot_lifecycle_tests.rs#L275-L317)
for a future, explicitly selected drain workstream. In-scope admitted-call
shutdown, cancelled-publication and native ownership tests remain unchanged.
After removal, the macOS SDK library passes **56 tests, 0 failed, 0 ignored**;
formatting and diff checks pass. No production code changed. Earlier failure
counts below are historical evidence, not the current suite status.

## Current-main integration

Integrated main `84340731fc38d6571881db27e3cf9f884d8f0e88` in an isolated
worktree, preserving the existing branch history and unrelated local changes.
Resolutions retain upstream recording coordinate projection/recentering, Windows
transport reporting, SDK lifecycle maintenance, macOS capture-only behavior and
Linux Hyprland window-scope refusal. All desktop token resolution still acquires
from the runtime-owned cache; the independent registry is not restored.

The starting base also contained unmerged browser commits `884c04123` and
`ed289df50`. Their 19 browser/documentation paths were restored to current main
so that this PR does not ship those unrelated capabilities. Their commits remain
in history; no replacement browser implementation was added.

After this scope cleanup, macOS validation passes: core **604**, platform **368**
(**2 existing ignored**), SDK **57**, and shared/dispatch invariants **7 + 3**.
The AppKit integration test binary compiles; GUI execution remains unverified.

Refreshed the normalized production comparison against the integrated main,
using Python 3.12.11 and the pinned parsers. Analyzer tests pass and all measured
sources parse without errors. Changed production files have **1,376 additions /
1,527 deletions**, net **-151 lines**. Structural complexity is **4,510 -> 4,446
(-64)**; decision surplus **3,759 -> 3,706 (-53)**. These supersede the old-base
-67/-49 comparison. Evidence is under `artifacts/rfc3473-integration/` in the
integration worktree; reproduction tools remain checked in.

Windows/Linux validation must cover the integrated source. The earlier cache
latency results are historical until repeated against the updated baseline.
Native AppKit regression, desktop latency and exact-SHA canonical certification
remain required before readiness.

## Weak discovery cleanup after integration

The unchanged Memcheck job on `f933999dd` reported one 180-byte possibly-lost
allocation from the weak runtime-cache directory. It reported no definitely- or
indirectly-lost blocks. Cache destruction now removes its matching discovery
entry and releases the directory allocation when empty. Pointer identity keeps
an older binding's destruction from unregistering a newer binding in the same
scope. Payload destruction remains outside the directory lock.

Extended the existing weak-discovery test to check entry removal and empty-table
capacity. Cache tests pass **8/8**; macOS core/platform/SDK suites remain
**604 / 368 / 57 passed**, with the same two platform ignores; shared/dispatch
invariants pass **7 + 3**. No worker-drain behavior, permissions, Memcheck flags
or suppressions changed. CI must verify the shutdown result on Linux before this
finding is considered resolved. The final production comparison needs to include
this additional cleanup.

## Actionable items

| ID | Action | Status | Exit evidence |
| --- | --- | --- | --- |
| T0 | Make selection visible and open one linked draft PR | Complete | #3473 scope reply; #3616 |
| T1 | Add shared invariant tests against existing components | Complete for initial seven cases | 3 pass, 4 fail; no masked failures |
| T2 | Add admitted macOS native lifetime coverage | Complete | New CF test and two existing cache tests pass |
| T3 | Extend canonical AppKit stale-ref row with both target forms and fresh recovery | Written; execution blocked | Test binary compiles; external counter oracle implemented |
| T4 | Run one exact-SHA canonical Lume matrix | Blocked | No verified prepared seed in configured host inventory |
| T5 | Review red tests at the actual dispatch/lifecycle boundaries | Controlled and native baseline reproductions reviewed | Both modeled dispatch and the installed built-in capture/click path exhibit identity/payload mismatch; see T6a |
| T6 | Add missing SDK shutdown/cancellation, native runtime isolation, Windows geometry and sparse Linux membership cases | Shared and native platform coverage added; Windows SDK probes policy-blocked | Baseline and current evidence below; cancelled-worker draining remains red and excluded |
| T6a | Reproduce through built-in AppKit capture/click with independent fixture state | Native failure reproduced on approved installed baseline | Old token activates replacement once; fresh token increments it again; video finalized. Binary hash recorded, source_sha null |
| T7 | Freeze performance baselines before any production edits | Pre-edit macOS cache measurements recorded; full task evidence pending | Final repeatability, formatting-normalized complexity/deletion counts and end-to-end latency remain gates |
| S1 | Specify unified desktop ownership and end-to-end flows | Local review draft complete | Scope, entry ownership, publication/resolution/retirement, diagrams, deletion audit, platform migration and acceptance gates |
| A | Implement the selected desktop simplification | Local implementation and validation in progress | All platform consumers migrated locally; Windows native tests pass; compatibility audit and certification pending |
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

### 2026-09-07 — Local review after maintainer workflow correction

The draft PR is parked for eventual passing implementation; this investigation
stays local before scoping that work. No new commits, pushes or GitHub updates.

Reran seven new invariants (3 pass, 4 fail), existing token tests (20 pass), and
native cache tests (3 pass). Also ran the existing SDK shutdown drain and desktop
coordinator checks (1 pass each). Logs are under
`artifacts/cua-driver/rfc-3473/local-review/`.

[Local failure analysis](rfc-3473-local-review.md) traces each failure to code and
separates direct errors from incomplete component probes. Main corrections to
the plan: reuse native retain guards and SDK lifecycle admission; do not assume
whole-record leases or another shutdown state machine are needed. The current
retirement test does not exercise actual SDK shutdown and must be replaced or
supplemented at that boundary before guiding implementation. The macOS state
producer updates its cache before awaiting optional capture and registering the
new token identity; this is a stronger integration target than the stand-in race.

### 2026-09-07 — Higher-fidelity local probes

Added nine uncommitted tests through actual ToolRegistry dispatch and the real
embedded SDK/ABI. No production behavior, API visibility, dependency, commit,
push, or GitHub change. Final runs: dispatcher 2 pass/1 fail; SDK 3 pass/3 fail.
The original shared probes remain 3 pass/4 fail; native cache controls remain
3 pass. New files are rustfmt-clean; diff whitespace checks pass.

The real dispatcher permits the modeled cache-update/capture/token-registration
interleaving. SDK shutdown drains ordinary publication and retires its token;
caller cancellation prevents late publication but does not keep shutdown waiting
for the already-running blocking probe. Real CF retain counts in SDK-owned host
fixtures confirm native payload retention beyond token eviction and shutdown,
with balanced release at handle destruction. Two-runtime native ownership and
reference isolation pass.

Tests substitute controlled capture/input probes and a host-owned real ToolState;
they do not invoke the private built-in capture tool or send native input. This
is stronger integration evidence, not Lume E2E or a native action cancellation
reproduction. Preserve that distinction when scoping eventual changes.

Evidence and local-source hashes:
`artifacts/cua-driver/rfc-3473/high-fidelity/`. Detailed analysis and commands are
in [the higher-fidelity review](rfc-3473-local-review.md#higher-fidelity-follow-up--2026-09-07).
Keep publication, retention policy and cancelled-worker cleanup separate; no
whole-record lease or new authoritative wrapper is selected by these results.

### 2026-09-07 — Built-in AppKit scenario, native permissions blocker

Added a fixture-only mode that replaces one NSButton and journals original versus
replacement activations. The native case uses actual get_window_state/click tools
and FIFO backpressure on screenshot_out_file to pause real publication without a
product hook. It asserts the same AX index, a genuinely pending PNG writer, no
replacement activation from the old token, and positive fresh-token recovery.
Wired the case into the existing macOS harness runner and documented its unproven
status in the test matrix.

The fixture and Rust integration test compile. Both FIFO release/cleanup controls
pass. Native diagnostics remain blocked: the first unrecorded diagnostic reached
get_window_state and received permissions_pending; after correcting an explicit
recording-environment requirement, the configured diagnostic fails at behavioral
recording start with the same driver permission gate (exit code 75). This is not
a pass or failure of native snapshot consistency. No TCC changes or daemon/VM
replacement were performed, and test-owned fixture processes were cleaned up.

All attempts, setup failures, typed result and source hashes are preserved under
`artifacts/cua-driver/rfc-3473/native-publication/`. See the built-in reproduction
section of [the local review](rfc-3473-local-review.md) for details and commands.
No production driver behavior, commits, pushes or GitHub records changed.

### 2026-09-07 — Base app approved; native failure reproduced

After maintainer approval of the base app, read-only permission status reports
both Accessibility and Screen Recording grants. Reran the native case through
the existing daemon, leaving standard mode and its process unchanged. No broader
authorization was needed.

The old token performed AXPress on the replacement button while the real capture
writer was still pending. External fixture state: original_clicks=0 and
replacement_clicks=1; fresh-token recovery then produced replacement_clicks=2.
The exact-index and pending-writer preconditions passed and the MP4 finalized.
This is an observed native targeting failure, not the prior modeled interleaving.

The installed version is 0.23.2 with source_sha=null. Evidence therefore identifies
the installed binary by SHA-256, not the local checkout:
`67ccfc99e69ebb5881fdfc3787d85abcd8cc2beb7423f255549557623cab6907`.
Artifacts: `artifacts/cua-driver/rfc-3473/native-publication/base-approved/`.
The native test remains red locally. No source changes were committed or pushed.

### 2026-09-07 — Vertical-slice technical specification

Replaced the historical tests-first outline with a full local design-review spec
for unified desktop snapshot ownership. It maps the before/after architecture,
state request, action resolution and replacement/cleanup sequences. It defines
atomic commit/retention points, backend responsibilities, error behavior, resource
retirement, exact deletions, migration, release impact and performance/native gates.

The proposal has one snapshot authority, not an owner above surviving maps. Existing
native retain guards, authorization and scheduling remain. Whole-record leases,
browser migration, typed action migration and general cancelled-worker draining
are excluded. Physical collection placement, legacy constructor lifetime and
shutdown cleanup integration are explicit review decisions, not hidden assumptions.
No production implementation, commit, push or GitHub change accompanied this spec.

### 2026-09-07 — Native Linux compilation and unit validation

Validated the transferred uncommitted implementation on `linux-2`, Ubuntu
24.04.4 x86_64, with Rust 1.97.1. This guest is detached at investigation base
`ed289df50257bd6a65f9ee7964bb842777a1a10a`; the inherited implementation is a
dirty working-tree overlay, not an exact committed candidate. Source hashes and
logs are under `artifacts/cua-driver/rfc-3473/linux-validation/`.

Prepared a private dependency sysroot because pkg-config/development packages and
passwordless sudo were unavailable. No system packages, daemon settings, or GUI
permissions were changed. Used debug-free, non-incremental builds; cleaned only
this validation's generated Cargo target before the release-feature build to fit
the 9.2 GiB disk.

| Check | Result |
| --- | --- |
| Core library | 592 passed |
| Shared snapshot invariants | 7 passed |
| Dispatch snapshot invariants | 3 passed |
| Linux library, default features | 309 passed, 5 ignored |
| Linux library, release-shipped `portal-input` | 313 passed, 5 ignored |
| SDK library | 52 passed, 1 failed |

The four Linux cache cases cover sparse membership, empty snapshots,
duplicate/unindexed nodes, and replacement with fresh-reference recovery. The
last two were added during this validation. No Linux production correction was
needed to compile or pass these suites.

The sole SDK failure is the previously established
`sdk_shutdown_waits_for_native_capture_after_caller_cancellation`; no assertion
was weakened or masked. The five ignored Linux tests require live X11 servers,
Secret Service mutation, or writable uinput. `DISPLAY` and `WAYLAND_DISPLAY` are
unset. These are native Linux build/unit results, not live desktop certification.
The complete matrix, source-identified native wrong-target reproduction, optional
`portal-capture`/Nix lanes, and end-to-end latency remain unverified.

## Windows continuation and corrected production comparison

Recovered the Windows continuation from its saved workspace and integrated its
four Windows files. The continuation patch and source hashes are preserved under
`artifacts/cua-driver/rfc-3473/windows-validation/`. No other workstream's checkout
was modified. This evidence was collected from an uncommitted overlay on the
investigation base, not a certified candidate SHA.

Windows state preparation owns the native payload inside its blocking worker
before screenshot work. Actions, focus, scroll, value verification and recording
resolve the same snapshot entry and carry the acquired native guard and matching
metadata. There are no remaining executable references to `TokenRegistry`,
`element_token::global`, identity-only resolution or late cache accessors in the
measured crates. Removed obsolete helper arguments and redundant native retains;
null pointers are refused, MSAA targets are not cast through UIA value interfaces,
and the SetValue worker owns its no-activate guard. The existing native guards
remain the lifetime mechanism; no whole-record lease was added.

Added Windows tests for null-pointer refusal and native target lifetime after
caller cancellation. Existing forced-interleave, replacement/geometry, eviction,
retirement and unpublished-payload tests pass. The MSAA metadata case also checks
that UIA focus operations refuse the MSAA guard.

| Check | Result |
| --- | --- |
| Windows library, final focused candidate | 209 passed, 3 intentionally ignored crash demonstrations |
| Shared snapshot invariants | 7 passed |
| Dispatch snapshot invariants | 3 passed |
| Core library, candidate | 574 passed, 13 failed |
| Core library, isolated pre-refactor baseline | 574 passed, same 13 failures |
| SDK validation | Blocked by inherited policy and cascading test-lock poisoning |
| Metrics analyzer tests | 2 passed |

The SDK probe returns `permission_denied`: this guest inherits
`CUA_DRIVER_POLICY_FILE=C:/ProgramData/Cua/hermes-cua-policy.yaml`, which denies
`health_report` before native work starts. Its timeout is not evidence of the
known cancelled-worker drain failure. The pre-refactor SDK suite also fails under
this policy. Broader SDK failure counts are not a reliable comparison because
one panic poisons the shared test mutex. The policy and daemon permissions were
not changed. Windows SDK lifecycle evidence still needs an approved test setup.

Baseline builds use a separate native Cargo target directory. A first comparison
using a shared target mixed cached crate APIs and is superseded by the isolated
run. An initial MSVC object timestamp failure was avoided by using a native
per-user target directory, two jobs and no debug/incremental output.

### Corrected non-test measurements before the window-width audit

Scope: changed production Rust files in core, SDK and the three platform crates.
Both sources use rustfmt 1.97.1, with test-only items/modules, comments and blank
lines excluded. Tree-sitter 0.25.2 / tree-sitter-rust 0.24.2 parse all measured
files without errors. The earlier Lizard complexity and line figures are
superseded because that parser missed substantial Rust function bodies.

| Metric | Before | Current | Change |
| --- | ---: | ---: | ---: |
| Non-test source lines in changed files | 23,215 | 23,125 | -90 |
| Structural cyclomatic complexity | 3,430 | 3,370 | -60 |
| Decision surplus | 2,878 | 2,824 | -54 |

Normalized production diff: **1,331 added, 1,421 deleted**. Complexity counts
function entry plus if/let-else/loops/try, short-circuit operators, match arms and
guards. Closure decisions belong to their enclosing function; nested functions
are separate; macros are not expanded. This is a disclosed source-level metric,
not a claim about the compiler-expanded control-flow graph.

The review-only `non-test-production.diff`, per-file JSON reports, analyzer and
source manifest make the comparison inspectable. The projection is not a patch
to apply to original source. Native window-width and legacy multi-binding
compatibility review, repeatable end-to-end latency and exact-SHA desktop
certification remain gates. Internal cache microbenchmarks are not E2E evidence.

## Window-width and ownership audit follow-up

The audit found that the unified cache had inherited the old token registry's
32-bit window projection as its storage key. Windows/Linux native interfaces use
64-bit identities; two windows differing only in their high bits must not share
replacement, validation or retirement state.

The shared cache, parsed reference and resolved target now carry `u64` window IDs.
Windows and Linux producers, actions and recording no longer narrow them. The
macOS adapter performs checked conversion to its native `u32` boundary and refuses
out-of-range values instead of silently truncating. Token formatting remains
unchanged: snapshot IDs are still 32-bit identities independent of window IDs.

New coverage proves that windows with equal low bits remain distinct, both target
argument forms reject a mismatched full-width ID, and retirement removes only the
specified window. Windows fake-COM and recording tests now use a window ID above
32 bits. Added Linux compositor-ID coverage and a macOS conversion-boundary test;
those two platform-native suites still need rerunning after this correction.

Ownership tests also prove that bindings sharing a scope keep independently owned
payloads and that weak recording discovery does not extend payload lifetime.
Legacy discovery still selects the last registered binding within a scope, as the
previous hook maps did; it does not own those bindings or create token validity.

Current Windows evidence: eight shared cache tests pass, all ten integration
invariants pass, and the Windows library remains 209 passed / 3 intentionally
ignored. Core library: 577 passed / the same 13 baseline failures. SDK production
and test code type-check. No host policy or daemon permission change was made.

Updated normalized production comparison: **1,375 added / 1,442 deleted**, net
**67 lines removed**. Changed-file structural complexity is **3,430 -> 3,381**
(-49); decision surplus is **2,878 -> 2,834** (-44). All measured files parse
without errors. These supersede the pre-audit comparison above.

The prior Linux/macOS execution evidence predates this width correction. Rerun
the affected native build/unit coverage, then reproduce the original AppKit case
against a source-identified implementation and perform stable-candidate desktop
and latency certification. This is not another policy-infrastructure workstream.

## Linux revalidation after the window-width correction

Revalidated on the replacement linux-1 guest with Rust 1.97.1. The guest needed
private apt indexes and an extracted per-user native dependency sysroot; no
system packages or policy settings were changed. Logs, dependency hashes and the
uncommitted source manifest are in
`artifacts/cua-driver/rfc-3473/linux-width-validation/`.

| Check | Result |
| --- | --- |
| Core library | 595 passed |
| Shared snapshot invariants | 7 passed |
| Dispatch snapshot invariants | 3 passed |
| Linux library | 310 passed, 5 ignored |
| Linux library with portal-input | 314 passed, 5 ignored |
| SDK library | 52 passed, 1 known excluded drain failure |

The compositor-ID collision/retirement test passes in both Linux configurations.
No Linux production correction was needed. In-scope SDK shutdown retirement and
cancelled-publication tests pass; the remaining failure is the previously
established cancelled-blocking-worker drain diagnostic, not a policy refusal on
this guest. Its assertion remains unchanged.

There is no active X11 or Wayland display. This completes the affected Linux
build/unit revalidation, not desktop E2E or latency certification. macOS native
coverage and the original source-identified AppKit regression are next.

## macOS revalidation and repeated cache latency

After the full-width identity correction, mac-studio passes 362 platform library
tests (2 ignored), 594 core library tests and all 10 shared/dispatch invariants.
The SDK library has 56 passes and the same one excluded cancelled-worker drain
failure. The in-scope SDK lifecycle and native CF ownership cases pass. No test
assertion was weakened or hidden.

Repeated the release-mode cache comparison against the pre-refactor source in a
separate baseline target directory. Three paired executions alternate order;
each workload keeps 40 samples of one million operations after five warmups.
All three per-run medians improved for every workload.

| Internal workload | Baseline median ns | Candidate median ns | Reduction |
| --- | ---: | ---: | ---: |
| Exact target acquisition | 58.62 | 36.68 | 37.4% |
| Same-window publication, 64 members | 62.37 | 29.99 | 51.9% |
| Publication/acquisition/eviction, 64 members | 200.80 | 148.36 | 26.1% |

These measurements use integer payloads, not native AX/COM retention, screenshots,
IPC or complete desktop actions. They do not establish end-to-end latency. The
baseline adapter and reproduction commands are checked in under
`tests/metrics/`; logs and binary hashes are under
`artifacts/cua-driver/rfc-3473/macos-width-validation/`.

Rechecked Lume inventory: it contains stopped workers, but no identifiable
prepared seed with the required provenance. The host reports no valid signing
identity. Other workstreams' workers were not booted or reused, and TCC was not
modified. The original native AppKit candidate regression and canonical desktop
certification remain blocked on an eligible prepared test environment.

The existing tests-first commits are retained when publishing the implementation
on #3616. Publication is for review and an immutable candidate identity, not a
readiness or merge claim. The intentionally failing excluded SDK drain diagnostic
remains visible in the source and suite results.

## Coordination

Revalidate [#2075](https://github.com/trycua/cua/pull/2075) before touching Windows
cache behavior; its freshness change is separate and contributor credit must be
preserved. Snapshot-related #3005, #3377 and #3573 were identified during initial
planning; refresh their actual state/diffs before any production change.

## Final selected-slice evidence — 2026-09-09

This section supersedes the earlier environment blocker and draft-next-action
notes above; those notes describe the evidence available at their timestamps.
The selected scope remains the maintainer reply on
[#3473](https://github.com/trycua/cua/issues/3473#issuecomment-5573495529),
not acceptance or implementation of the full RFC.

### Integration and bounded corrections

- Published the tests-first implementation as `b1a74850e`.
- At the user's explicit direction, `126414a08` removed only our newly authored,
  out-of-scope cancelled-native-worker shutdown-drain diagnostic and its unused
  signal. The finding remains deferred, not fixed or counted as a pass; its
  original source is recoverable at `b1a74850e`. In-scope assertions remain.
- Integrated main while preserving upstream behavior and contributor history;
  excluded inherited browser changes outside this slice.
- `17c2fecd3` unregisters a dying weak-directory binding by pointer identity,
  preserving a newer same-scope binding and freeing empty directory capacity.
  The unchanged Memcheck gate passes without suppressions. Native payload
  destruction remains outside the cache lock.
- The installed trusted Lume workflow can execute with its configured,
  provenance-labelled local baseline. The earlier assertion that no such
  execution was available was incorrect; successful execution does not turn
  that worker-derived baseline into a certified immutable private seed.
- The first complete macOS run passed 158 behavioral rows but failed strict
  evidence validation for a semantic SwiftUI popover action without a drawable
  point. Cherry-picked only `4c8b9ff20867de6e0398ef6de7a3a2a3413cdbd2`
  from #2907 with `-x`, as `c1f8010a8`, preserving credit and linking the landing
  location on the source PR. No fabricated point, fixture movement, or relaxed
  pixel evidence was introduced. Main subsequently incorporated that PR.
- At `c1f8010a8`, macOS desktop and standalone-browser suites passed. A Windows
  background pixel row failed identically on main `84340731`; both failures
  remain recorded on #3621. They were not retried into passes.
- Integrated main `6c0348b059595e63d1df96e6df2047ca7dbbbf1c`, including
  the upstream Windows fix #3671 and SDK maintenance, producing final product
  candidate `2a8b170609abc0cd9eb5bda0feae5f95bf0bb75c`. The Windows merge
  resolution retains the admitted native target inside the blocking worker
  while preserving upstream typed UIA unavailability/refusal handling.

### Exact-product desktop evidence

All mandatory desktop lanes were rerun on `2a8b17060` after that integration:

| Platform | Evidence | Result |
| --- | --- | --- |
| Windows | [34350875905](https://github.com/trycua/cua/actions/runs/34350875905) | 136/136 rows; all lanes and installer passed |
| Linux X11 | [34350878655](https://github.com/trycua/cua/actions/runs/34350878655) | 129/129 rows; all lanes and installer passed |
| macOS | Lume `20260909T122704Z-8a66cb28` | 159/159 desktop rows, including original AppKit pending-publication regression and strict evidence/video validation |

Local merged-tree tests passed: 604 core, 71 SDK, 368 macOS platform tests
(2 existing ignored), and all 10 shared/dispatch invariants. Ordinary PR CI,
including release metadata, Nix, Memcheck, and platform checks passed on the
product candidate. No mandatory desktop row was filtered or waived.

The requested optional standalone-browser extension passed **17/18** rows,
not 18/18. Its final Chrome collision case failed during exact-window posture
setup, before the ambiguity assertion: AX focused window `3035` while the
frontmost ordinary window remained `72`. The driver refused the unverified
posture. [#3681](https://github.com/trycua/cua/issues/3681) retains this failure,
its unknown cause, and the distinction from wrong-target input.

A single baseline-only diagnostic, `paired-20260909T142514Z`, ran that focused
case against product source `6c0348b05` using the existing `2a8b17060` harness;
the browser test, testkit, and standalone runner sources are identical between
those commits. It passed the expected ambiguity refusal and strict video
validation. This does **not** reproduce the original failure, prove a baseline
failure, certify unrun baseline lanes, or repair the failed candidate full run.
No automatic candidate retry or assertion relaxation was performed.

### Deletion, complexity, and latency

The pinned parser reports zero parse errors. Comparing identical changed-file
production scope against main `6c0348b05`:

- 1,395 additions / 1,532 deletions: **137 fewer production lines**;
- normalized production NLOC: 28,987 → 28,850;
- disclosed source complexity: 4,514 → 4,453 (**−61**);
- decision surplus: 3,762 → 3,711 (**−51**).

These are production projections, excluding tests/comments, not a claim that
the entire PR diff is smaller. Historical integer-payload cache measurements
show substantial internal reductions but do not establish native task speedups.

The updated native comparison uses main `6c0348b05` and product `2a8b17060`.
Run `paired-20260909T131409Z` completed all 3,200 rows: 16 AB/BA paired blocks,
20 measured samples plus five warmups per version/workload/block. The estimator,
20,000-resample paired bootstrap, seed 3473, and 1.05 upper-bound gate were fixed
before collection. No samples were replayed or discarded after observing results.

| Native workload | Paired ratio | 95% ratio interval |
| --- | ---: | --- |
| Snapshot + screenshot | 1.00393 | 0.97532–1.02746 |
| Semantic press + observed counter | 0.99555 | 0.98906–1.00183 |
| Background pixel address + observed counter | 1.00230 | 0.98663–1.01686 |
| Set value + observed text | 0.99957 | 0.99818–1.00082 |

All four upper bounds exclude a slowdown greater than 5% in this measured scope.
Native latency is essentially flat, not meaningfully faster. The complete compact
receipt, byte-identical measured client, analyzer, integrity checks, and commands
are checked in under [`tests/metrics`](../tests/metrics/README.md).

The workload is macOS AppKit over persistent daemon-backed MCP, with a visible
target, screenshots and native state confirmation, and no video recording. Its
pixel-addressed background action uses the AX hit-test bridge, not raw physical
delivery. It does not measure browser tasks, Windows/Linux native task latency,
native-read counts, or native memory high-water marks. The broader performance
matrix in the full RFC/spec is **not certified** by these results.

The guest completed despite an interrupted host monitoring connection; the
supervised controller collected the original samples, restored the standard
daemon, and verified the owned worker stopped. The baseline-only browser probe
also restored standard mode and stopped the worker. No credential changes, TCC
repair, baseline deletion, or repeated sampling were needed.

### Review handoff and remaining limits

The selected desktop ownership correction has exact-product mandatory desktop
and focused invariant evidence, demonstrated production deletion/complexity
reduction, and bounded native latency evidence. Handoff is for review of that
slice, not a claim that the whole RFC or optional browser matrix is certified.
The PR remains `Refs #3473`, not an issue-closing claim.

Keep #3681, local-seed provenance, unmeasured native performance/resource scopes,
Wayland compositor limits, Linux live AT-SPI lookup without new native leases,
Windows geometry freshness #2075, and the explicitly deferred general SDK drain
finding visible. Do not convert them into green evidence or silently expand
this implementation to resolve them.

The final evidence-only follow-up adds documentation, measurements, and the
already-executed supplementary benchmark client/analyzer; it does not change
Rust product code, native fixtures, canonical runners, or their environment.
The client digest and reconstructed raw-data digest are tested, and published
statistics exactly match the original raw-row analysis. Account for that final
diff against `2a8b17060` rather than repeat unrelated desktop rows. Recheck final
PR title/release metadata and ordinary CI before marking review-ready. Do not
merge automatically; after an eventual merge, run the short main/release-path
smoke required by the selected scope.
