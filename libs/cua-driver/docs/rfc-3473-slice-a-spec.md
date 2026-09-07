# RFC 3473: tests-first snapshot invariants

Status: experimental tests; production architecture unselected.
Decision record: [RFC 3473](https://github.com/trycua/cua/issues/3473).
Execution: [draft PR #3616](https://github.com/trycua/cua/pull/3616).
Progress: [worklog](rfc-3473-worklog.md).

This replaces the initial owner/record/lease implementation proposal following
maintainer feedback: first establish invariants against existing code, then use
the evidence to select deletion-oriented simplifications. No new authoritative
wrapper, lease type, or duplicate store is required by these tests.

The problem and desired behavior derive from `injaneity`'s RFC. The RFC remains
pending review; selecting tests does not accept its production architecture.

## Rules

- Tests assert behavior and lifetime, not preferred Rust type names.
- Keep failures visible: no expected-failure masking or new ignore annotations
  except the existing native GUI harness convention.
- Separate component counterexamples from public-dispatch reproductions.
- Preserve existing behavior with fixtures; review differences explicitly.
- Require every production proposal to identify state or coordination it deletes.
- Measure before claiming latency improvements. No performance claim is made here.

## Invariant matrix

Paths are relative to `libs/cua-driver/rust/crates/`.

| Invariant | Current test/evidence | Remaining evidence |
| --- | --- | --- |
| Empty snapshots have no addressable members | `cua-driver-core/tests/snapshot_invariants.rs`: empty membership test | Public state/action outcome characterization |
| Newer reads invalidate every older member | Shared replacement test; existing token unit tests | AppKit E2E execution for token and snapshot/index forms |
| Identity cannot resolve into a replacement payload | Shared barrier/channel test against TokenRegistry and ElementCacheCore | Establish public dispatch reachability and admission ordering before calling this an end-to-end race |
| Runtime retirement releases unadmitted payload | Shared composed-component drop-counter test | Real SDK lifecycle and platform-state wiring |
| Eviction releases unadmitted payload | Shared composed-component drop-counter test | Native platform eviction/resource measurements |
| Resolution does not reorder bounded retention | Shared publication-order eviction test | None for metadata ordering; payload bounds remain separate |
| Clearing one runtime preserves another's metadata | Shared same-pid/window runtime test | Native payload isolation through actual runtime dispatch |
| Admitted native resources survive destruction | `platform-macos/src/ax/cache.rs`: guard held by a native-work thread after cache drop | Actual async cancellation and runtime shutdown integration; Windows UIA/MSAA equivalents |
| Stale requests do not mutate; fresh recovery delivers once | Existing AppKit stale-ref row now checks both forms, unchanged counter, and fresh-token counter increment | Logged-in canonical Lume execution |
| Pointer, geometry and roles belong to the same snapshot | Not added yet | Windows-specific deterministic test with actual cache accessors |
| Closed runtime cannot republish in-flight capture | Not added yet | SDK admission/capture barrier test, not merely calling clear on a standalone registry |
| Membership handles sparse native indices | Not added yet | Actual Linux walk/lookup fixtures before asserting dense membership policy |

The composed-component tests deliberately use the existing split publication and
lookup APIs. Their red results show that those APIs alone do not enforce the
proposed invariant. They do not prove that an existing scheduler permits that
interleaving, nor that `clear_runtime_scope` alone represents full SDK shutdown.
Do not patch the isolated APIs solely to turn these tests green. Follow the real
call path first and retain or revise the test at the appropriate boundary.

## Test commands

From `libs/cua-driver/rust`:

```bash
cargo test --locked -p cua-driver-core --test snapshot_invariants -- --nocapture --test-threads=1
cargo test --locked -p cua-driver-core --lib element_token::tests -- --test-threads=1
cargo test --locked -p platform-macos --lib ax::cache::tests -- --nocapture --test-threads=1
cargo test --locked -p cua-driver --test harness_appkit_test --no-run
```

The AppKit row remains owned by `harness_appkit_test.rs` and is already selected
by the canonical macOS runner; no parallel scenario runner or runtime layer was
added. Compilation is not GUI execution.

Once the tests are stable and a verified seed is available, use one clean,
committed candidate and run inside Terminal in the disposable guest's logged-in
GUI session:

```bash
libs/cua-driver/tests/runners/macos-lume/run-all.sh
```

Follow [the Lume image and signing contract](../tests/runners/macos-lume/README.md).
Do not reuse another workstream's worker, run the gate over SSH, or substitute
host CF tests for Lume E2E. Retrieve all artifacts even when the run fails.
The ordinary unit suite and GUI suite are separate; a GUI pass cannot erase red
shared invariants.

## Review before production changes

Classify each result as an existing guarantee, an unmet desired contract, an
unproven composition, or an environment failure. For every proposed edit ask:

1. What state, conversion, or coordination step disappears?
2. Which observable test requires the remaining code?
3. Can an existing structure own the responsibility directly?
4. What allocation, lock, retain, native read, or memory cost is introduced?

Start with removable work whose dependencies are proven, such as Linux's unused
mirror. Do not presume a new owner is the answer. Coordinate Windows changes with
[PR #2075](https://github.com/trycua/cua/pull/2075), preserving contributor credit.

Freeze publication, resolution, replacement, cleanup, allocation and native-read
baselines before production edits. Apply the RFC's per-platform task-success,
latency confidence and memory gates before landing a refactor. Those benchmarks
are not part of the evidence produced by this tests-only draft.
