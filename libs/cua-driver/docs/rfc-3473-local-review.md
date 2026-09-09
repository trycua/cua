# RFC 3473: local invariant review

Status: historical pre-implementation investigation, not a scoped implementation
proposal. See the [worklog](rfc-3473-worklog.md) for subsequent selection and results.
Source tested: `08f102f177e8fa674047074845868c2caf4098c3`.
The executable diff is unchanged from `d923193a906707a425f4ec158d966c8d34709d48`.
No production changes, additional commits, pushes, or GitHub updates were made
for this review. The existing draft PR is parked for eventual passing work.

## Reproduction

From `libs/cua-driver/rust`, using Rust 1.97.1 on the available macOS host:

```bash
cargo test --locked -p cua-driver-core --test snapshot_invariants -- --nocapture --test-threads=1
cargo test --locked -p cua-driver-core --lib element_token::tests -- --test-threads=1
cargo test --locked -p platform-macos --lib ax::cache::tests -- --test-threads=1
cargo test --locked -p cua-driver-sdk --lib shutdown_drains_an_already_admitted_call -- --nocapture --test-threads=1
cargo test --locked -p cua-driver-core --lib physical_desktop_actions_are_admitted_one_at_a_time -- --test-threads=1
```

| Check | Result |
| --- | --- |
| New shared invariants | 3 passed, 4 failed |
| Existing token tests | 20 passed |
| Native CF cache-lifetime tests | 3 passed |
| Existing SDK admitted-call shutdown drain | 1 passed |
| Existing desktop admission serialization | 1 passed |

Logs: `artifacts/cua-driver/rfc-3473/local-review/` (local ignored artifacts).
These are host unit/component tests, not Lume GUI E2E. No latency comparison was
performed. The previously compiled AppKit scenario was not executed in this run.

## 1. Empty membership: a concrete representation error

`element_token.rs` stores `max_element_index = element_count.saturating_sub(1)`.
For zero elements this becomes zero. Resolution rejects only indices greater
than that maximum, so index zero resolves successfully.

This fails the desired membership contract directly, without any scheduler or
native application. Production macOS state reads explicitly register zero-count
snapshots, so this is not merely an impossible registry input. The native cache
normally still rejects an absent element; successful registry resolution is not
evidence that a GUI action was delivered.

Implication: an inclusive maximum cannot represent an empty set. For dense
membership, a count plus an exclusive bound is sufficient and simpler. Linux's
application-wide indices require separate characterization before treating every
backend as dense. No wrapper, lease, or new runtime is needed to explain this
failure. A correction would still require review of public refusal differences.

## 2. Identity/payload mismatch: real split paths, incomplete reproduction

The test resolves a token, forces replacement, then looks up the returned
window/index in a different store. It gets `replacement-target`.

The production macOS click path has the same split responsibilities:

- `tools/click.rs:353`: resolve token to window/index;
- `tools/click.rs:430`: acquire a retained element from the current window cache.

There is also a more significant publication interval:

- `tools/get_window_state.rs:290`: update the native cache;
- `tools/get_window_state.rs:309`: optionally await screenshot work;
- `tools/get_window_state.rs:491`: register the new token snapshot.

A same-runtime reader can therefore replace native data before the old identity
is retired. `get_window_state` is not among the tools admitted by
`is_physical_desktop_action` in `cua-driver-core/src/tool.rs:2542`. The desktop
coordinator passing its test proves action/action serialization, not read/action
serialization. SDK dispatch uses shared lifecycle read guards, not an exclusive
per-runtime action/read lock.

This makes the source-level concern substantive, rather than disproved by the
existing coordinator. However, the new test still uses stand-in strings and
standalone components, not the actual ToolRegistry/native capture path. It does
not certify end-to-end wrong-target delivery or cover every transport's scheduling.

Implication: focus the next deterministic reproduction on publication and lookup
at the actual integration boundary, especially the capture interval. Moving two
calls adjacent would shorten the gap, not prove atomicity. A solution must bind
identity checking to native lookup; it need not retain a whole tree or invent a
new native-resource guard. Serializing every state capture with input is also not
a free fix: it changes scheduling and may add screenshot latency to actions.

## 3. Eviction cleanup: a real retention mismatch, not measured leakage

The registry bounds valid snapshots at eight per runtime/process. Its eviction
only removes metadata. `ElementCacheCore` is a separate unbounded map whose
entries disappear on replacement, explicit remove, or map destruction. The
macOS ElementCache wrapper exposes no removal operation, and its production
updates come from state reads; registry eviction does not notify it.

Consequently, the test sees zero payload drops after publishing nine distinct
windows, instead of the requested one. Closing the test cache releases all nine:
this is retained ownership, not proof that references are permanently leaked.
Repeated reads of the same window do replace and release its old payload; the
mismatch is across distinct window keys and lifetime boundaries.

Implication: distinguish a bound on actionable identities from a bound on retained
native payloads. The current test uses the real generic map but a mock payload;
add native/runtime-level retention evidence before choosing cleanup policy. Avoid
adding a second independently maintained eviction queue. Prefer one existing
retention decision controlling both data and validity if the invariant is accepted.

## 4. Runtime retirement cleanup: the test overstates what it exercises

The red test calls `TokenRegistry::clear_runtime_scope` while deliberately keeping
an unrelated cache variable alive. It does not instantiate or shut down an SDK
runtime. Requiring that method to drop an arbitrary standalone map is not a valid
unit contract for that method in isolation.

Actual runtime shutdown (`cua-driver-sdk/src/runtime.rs:215`) already:

1. closes admission;
2. takes the lifecycle write lock to drain admitted invocations;
3. revokes authority/session state;
4. clears token metadata;
5. finalizes recording.

The existing `shutdown_drains_an_already_admitted_call` test passes locally.
`ToolState` creates a fresh native cache for each constructed platform registry;
there is not one process-global native cache shared by every SDK runtime. That
existing ownership matters and should not be replaced without a failing test.

Shutdown keeps the registry field alive while the runtime object is retained;
there is no explicit native-cache retirement call in that shutdown path. Eventual
runtime/registry/state destruction can release the cache through existing RAII.
Exactly how long closed handles keep native references needs an actual SDK/native
lifetime test, not this standalone composition assertion.

Implication: keep the desired lifecycle invariant, but replace or supplement this
probe at the real runtime boundary before using it to justify production work.
Do not add another shutdown admission state machine: one already exists. The drain
test does not prove behavior after async cancellation detaches blocking native work;
that remains a separate coverage gap.

## What the passing tests remove from the plan

- Same-window invalidation already works at the token layer.
- Resolving a token does not refresh publication-order eviction.
- Clearing one runtime's token metadata preserves another runtime's metadata.
- Existing native retain guards keep admitted resources alive across replacement
  and cache destruction; release balances after the native-work thread finishes.
- SDK shutdown already drains normally admitted calls.
- Desktop actions already share admission serialization.

These are existing mechanisms to retain, not capabilities to build again.
In particular, no failing test currently requires whole-record leases. Their
potential extra retained memory would be a new cost without demonstrated benefit.

## Effect on the original plan

The original spec bundled three different questions into a new abstraction:
reference membership, identity-to-payload consistency, and resource retirement.
The evidence does not justify that bundle or a latency-improvement claim.

Revised local sequence, not a selected PR scope:

1. Keep the direct membership regression as a precise test.
2. Reproduce the actual publication/lookup boundary deterministically without
   adding a public test hook or replacing scheduling.
3. Replace the shutdown stand-in with real runtime lifecycle evidence and quantify
   native retention across eviction/closed handles.
4. Preserve the existing native guard and shutdown machinery.
5. Only then choose which existing structure should absorb identity or cleanup
   responsibility, and identify the state/maps/coordination the change deletes.

The eventual PR should contain passing tests at the owning boundaries plus a
minimal, evidence-backed change. Do not make the current artificial compositions
pass by introducing another layer just to satisfy them. Lume E2E remains useful
for public stale refusal, no-mutation and fresh recovery, but a sequential GUI row
cannot by itself prove atomic publication or memory cleanup.

## Higher-fidelity follow-up — 2026-09-07

Added nine more local tests, without production changes or GitHub updates:

- `cua-driver-core/tests/snapshot_dispatch_invariants.rs`: actual ToolRegistry
  dispatch, authorization context, session/runtime scope, public argument resolver
  and scheduling; controlled tool bodies stand in for capture and native lookup.
- `cua-driver-sdk/src/snapshot_lifecycle_tests.rs`: actual embedded SDK, ABI
  completion/cancellation and shutdown, using the existing host-tool registration
  seam. Native cases own a real macOS ToolState/ElementCache and measure CF retains.
  The only tracked source hook is a `cfg(test)` module declaration in SDK `lib.rs`.

The built-in GetWindowStateTool is private to the platform crate. Rather than
change production visibility, the native fixture registers a host tool owning the
same concrete ToolState type. It is not a live invocation of the built-in capture
tool. CFString supplies a safe native CF lifetime oracle, not an actionable AX
object; no native input is sent.

### Results

| New test | Result | What is proved |
| --- | --- | --- |
| Pending capture cannot pair old identity with replacement payload | Fail | Real registry dispatch reaches revision 2 with revision 1's token during the controlled update-before-registration interval |
| Completed replacement refuses stale token and recovers fresh lookup | Pass | Real dispatch refuses before lookup, then reaches only revision 2 for its fresh token |
| Cross-generation refusal precedes payload lookup | Pass | Real dispatch scopes the reference and leaves foreign payload untouched |
| SDK shutdown drains snapshot publication and retires its result | Pass | Normally admitted publishing completes before shutdown clears its token; new calls are refused after admission closes |
| Cancelled capture cannot publish after shutdown | Pass | Cancellation through the public Rust SDK/ABI drops the async publisher; releasing its blocking worker does not publish a token |
| Shutdown waits for blocking capture after caller cancellation | Fail | Shutdown completion is observed while the worker is still parked and native_done is false |
| Closed SDK handle releases unadmitted native snapshot at shutdown | Fail | CF retain count remains base + 1 until handle destruction; token is already stale |
| Token eviction releases corresponding native snapshot | Fail | Nine window payload retains remain where the accepted bound would permit eight; oldest token is stale |
| Destroying one SDK runtime preserves another's native snapshot | Pass | First native retain releases, second remains, and second's own token resolves; second destruction balances its retain |

Totals: **5 passed, 4 failed** for the new nine. Repeated both new suites with
the default Rust test-thread setting: the same four tests fail and the same five
pass. Reran the original shared probes (3 passed, 4 failed) and native cache
controls (3 passed). No failures were masked.

### Interpretation changes

The publication failure is no longer only two components called manually: the
actual dispatcher permits the problematic modeled interleaving. Native capture
and actual click remain substituted, so this is still not proof of GUI delivery.
The capture pause mirrors `get_window_state`'s existing native-cache update,
screenshot await, and token registration order. The action probe intentionally
returns an error before native input and journals only the lookup revision.

The retirement findings now cross real SDK ownership and native CF release, not
only a standalone mock map. Both tests verify release after handle destruction
before asserting the stronger desired shutdown/eviction contract. This confirms
retention beyond token validity in the registered native fixture, not permanent
leakage. The normal two-runtime isolation test passes, arguing against replacing
existing per-runtime cache ownership solely for isolation.

Cancellation is a separate boundary. `abi.rs::spawn_completion` aborts the async
work on operation cancellation; dropping that invocation releases its lifecycle
read guard, while Tokio cannot abort already-running spawn_blocking work. Thus
normal shutdown draining and cancelled-worker draining have different outcomes.
The passing no-late-publication test matters: this result does not establish
snapshot resurrection. Nor does the host capture probe demonstrate a broken
built-in input action's cleanup. Test affected native producers before proposing
a general runtime change or adding this concern to the first snapshot PR.

The SDK cancellation test waits for the async invocation's drop notification,
then closes admission while the worker is held by a channel. A 250 ms timeout
only bounds the observation window before releasing the worker; observed early
completion with native_done false is the failure oracle, not elapsed latency.
Worker release and final shutdown are awaited before asserting. Registry tests
use notifications for capture ordering and a five-second deadlock watchdog; they
do not use sleeps to manufacture the publication interleaving.

### Reproduce and identify this local diff

From `libs/cua-driver/rust`:

```bash
cargo test --locked -p cua-driver-core --test snapshot_dispatch_invariants -- --nocapture --test-threads=1
cargo test --locked -p cua-driver-sdk --lib snapshot_lifecycle_tests -- --nocapture --test-threads=1
```

These tests are uncommitted additions on base
`08f102f177e8fa674047074845868c2caf4098c3`, not a new certified candidate SHA.
Source hashes and the cfg(test) hook patch are under
`artifacts/cua-driver/rfc-3473/high-fidelity/`; final result logs are
`dispatch-final.log` and `sdk-final.log`. Rustfmt on the new test files and
`git diff --check` pass. Existing Swift bridge linker warnings remain.
No GUI E2E, Windows/Linux native execution, or latency benchmark was run.

### What remains before scoping production work

Keep publication consistency, retention policy and cancelled-worker cleanup as
separate questions. Existing element guards, normal SDK draining, cancellation's
no-late-publication behavior and runtime isolation should be preserved. None of
these results requires whole-record leases. The next integration gap is actual
built-in capture/action behavior and its failure/cancellation ownership, not
another generic store or scheduling wrapper.

## Built-in native-path reproduction — 2026-09-07

Added an opt-in AppKit fixture mode and a native scenario under the existing
`harness_appkit_test` owner. The macOS runner now selects
`snapshot_publication::harness_appkit_pending_snapshot_cannot_retarget_token`.
No production driver function, visibility, or capture callback changed.

### How the scenario removes the mocked boundary

1. The real built-in `get_window_state` captures an original NSButton and returns
   its token. The fixture window contains deterministic high-entropy image content
   so its screenshot is large enough for file-pipe backpressure.
2. A fixture-only command replaces that button with a different NSButton. An
   external JSON journal acknowledges the change; no driver state read is used
   to coordinate the replacement.
3. A second proxy to the same installed daemon calls the real `get_window_state`
   with a FIFO as `screenshot_out_file`. The test reads only the PNG header and
   leaves the remaining bytes undrained. This holds the actual file write after
   cache update and before the state tool registers its new snapshot identity.
4. While the writer is demonstrably still open/pending, the first proxy calls the
   built-in foreground `click` with the original token. The fixture journals
   original and replacement activations separately and acknowledges a checkpoint.
5. The test verifies the writer remained pending, drains it, joins the capture,
   checks that the replacement occupied the original element index, and exercises
   fresh-token recovery as a positive control.
6. The safety assertion requires zero replacement activations from the old token.
   A successful fresh click must increment the replacement journal exactly once.

This uses public tools and ordinary file-write backpressure, not a test hook in
product code. The FIFO guard drains/releases its worker on failure too. Two local
helper tests verify both explicit release and drop cleanup using a controlled
large writer. The native case also rejects small images, premature writer closure,
missing fixture acknowledgements, and a changed target index rather than silently
passing an interleaving it did not establish.

### What ran

| Check | Result |
| --- | --- |
| Repo AppKit fixture build (`macos.sh --only appkit`) | Passed |
| Rust AppKit integration binary including the new native case | Compiled |
| FIFO backpressure and drop-release helper tests | 2 passed |
| Native diagnostic without recording setup | Reached the first built-in state request; refused with `permissions_pending` |
| Intermediate evidence-path attempt | Test setup error: recording directory absent; replaced unwrap with an explicit requirement |
| Final diagnostic with standard recording/results environment | Blocked by `permissions_pending` at behavioral recording start; exit code 75 from the driver |
| Rustfmt/new support file, shell syntax and diff whitespace | Passed |

The final native run did **not** reach the publication race assertion. It is an
environment failure, not evidence that the native invariant passes or fails.
The installed daemon's source SHA was not certified for this local diagnostic.
The FIFO behavior is tested; native image size, actual AX index replacement and
old-token delivery still require an authorized run.

Read-only permission status reports `daemon_running: false` / `unknown`, while a
pre-existing installed daemon accepts proxy connections and refuses work with
`permissions_pending`. Both observations are retained. The existing daemon,
macOS grants and other workstream's Lume VM were left untouched. The test-owned
AppKit process was cleaned up on failure.

### Files and evidence

- `tests/fixtures/apps/macos/appkit/SnapshotPublication.swift`: opt-in native
  fixture, noise image, replacement control and independent JSON journal.
- `rust/crates/cua-driver/tests/support/appkit_snapshot_publication.rs`: scenario,
  FIFO lifetime helper and its two non-GUI controls.
- `harness_appkit_test.rs`, fixture `main.swift`, and the macOS runner: test-only
  wiring; ordinary fixture mode remains unchanged.
- `artifacts/cua-driver/rfc-3473/native-publication/`: build logs, helper results,
  all attempted-run logs, configured typed result, permission status, recording
  setup artifacts and source hashes. Nothing is a certified trajectory/video.

For a local diagnostic on a permission-ready daemon, from `libs/cua-driver/rust`
(set the standard socket variable if that daemon uses a non-default socket):

```bash
export CUA_E2E_RECORDINGS_ROOT="$(git rev-parse --show-toplevel)/artifacts/cua-driver/rfc-3473/native-publication/recordings"
export CUA_E2E_RESULTS_FILE="$(git rev-parse --show-toplevel)/artifacts/cua-driver/rfc-3473/native-publication/results.jsonl"
cargo test --locked -p cua-driver --test harness_appkit_test snapshot_publication::harness_appkit_pending_snapshot_cannot_retarget_token -- --ignored --exact --nocapture --test-threads=1
```

This targeted diagnostic does not replace the canonical exact-source Lume matrix.
The next prerequisite is a permission-ready, source-identified native environment.
Do not select a production fix from an environment-blocked test or bypass TCC to
force a result. All additions remain local and uncommitted.

## Native reproduction after base-app approval — 2026-09-07

The maintainer approved the installed base CuaDriver app. Read-only status now
reports Accessibility and Screen Recording grants attributed to that daemon.
Reran the native case against the existing default socket without restarting the
daemon, changing its standard permission mode, or altering TCC configuration.

**The native invariant fails on the installed binary.** The test reached and
passed the capture barrier, writer-still-pending checks, same-index check and
fresh-token positive control. It then failed on the actual wrong-target oracle:

| Observation | Recorded value |
| --- | --- |
| Old-token action | `Performed AXPress on [1] AXButton "Replacement".` |
| Fixture after old-token action | `original_clicks=0`, `replacement_clicks=1`, checkpoint acknowledged |
| Fixture after fresh-token recovery | `original_clicks=0`, `replacement_clicks=2` |
| Behavioral recording | Finalized |
| Driver permission mode before/after | Standard; same daemon PID |

The independent fixture journal proves native activation of the replacement
control; this is no longer merely a modeled lookup mismatch. Public action effect
remains `unverifiable`; the test does not relabel that result as confirmed. Its
external fixture oracle supplies the delivery evidence.

### Provenance and limitations

Installed version: `0.23.2`. `get_config.source_sha` is `null`; this is an
installed-binary baseline reproduction, **not** exact-source certification of
our checkout or a complete Lume matrix result. The tested installed binary is:

```text
SHA-256 67ccfc99e69ebb5881fdfc3787d85abcd8cc2beb7423f255549557623cab6907
/Applications/CuaDriver.app/Contents/MacOS/cua-driver
```

Evidence is under
`artifacts/cua-driver/rfc-3473/native-publication/base-approved/`: daemon config,
permission status, binary hash, test log, typed result and the recording folder's
`snapshot-old-action.json`, `snapshot-after-old.json`,
`snapshot-after-fresh.json`, PNG, trajectory and MP4. The typed row correctly
remains failed; its missing final FixtureState success oracle is secondary to
the deliberate failing assertion, not evidence that the independent JSON journal
was absent.

This resolves the local permission blocker for native diagnosis. Standard-mode
authorization was sufficient for this case; the earlier concern that unrestricted
mode might be necessary was not borne out. Do not restart or weaken the daemon's
mode just to reproduce it.

### Effect on scoping

Publication consistency now has concrete native failure evidence on the installed
baseline. Prioritize fixing the identity/payload publication and resolution
boundary while retaining existing native guards. A whole-record lease abstraction
or broader scheduling rewrite still does not follow from the evidence. Retention
policy and cancellation drain findings remain separate. Before making a candidate
claim, build/identify the exact production source and rerun this test there.

## Final product review — 2026-09-09

The implementation and source-identified evidence now supersede the
pre-implementation status above. Reviewed product candidate:
`2a8b170609abc0cd9eb5bda0feae5f95bf0bb75c`, against main
`6c0348b059595e63d1df96e6df2047ca7dbbbf1c`.

The final pass checked the authority boundary, lifecycle, platform adapters,
recording hooks, and the Windows integration conflict:

- Identity, exact membership, and native payload publish together under the
  runtime-owned cache lock. Empty and sparse membership no longer become a
  permissive range; full-width window identities remain intact.
- Admission retains the selected native target under the same authority, then
  actions and payload destruction run outside the storage lock. Windows keeps
  the admitted guard inside its blocking closure and retains upstream typed
  provider refusal behavior. No late latest-cache lookup substitutes a target.
- Native capture workers prepare private payloads; publication follows the
  awaited result in the producer future. A cancelled producer cannot publish
  merely because its detached native preparation eventually returns. This is
  not a general promise that SDK shutdown drains every cancelled native worker.
- Weak runtime discovery does not own payloads or decide successful validity.
  Pointer-identity cleanup cannot unregister a newer same-scope binding;
  directory locking and cache clearing do not move native destruction under
  the cache storage lock.
- The final path list does not migrate browser snapshots, typed action APIs,
  authorization, general scheduling, or Windows geometry freshness. Linux still
  performs its existing live AT-SPI lookup after validated sparse admission;
  no new native lease contract is claimed there.

No additional blocking snapshot-ownership finding was identified in this local
review. This is not an independent maintainer approval. Exact-source mandatory
desktop runs pass all 424 rows across Windows, Linux X11, and macOS, including
the original native pending-publication/fresh-token control. The pinned
production projection removes 137 lines and 61 complexity points.

The updated bounded AppKit/MCP native comparison passes the four predeclared
95% upper bounds for excluding a slowdown greater than 5%; its largest upper
bound is 1.02746. It does not establish meaningful native speedup or certify
raw physical, browser, other-platform latency, native-read counts, or native
memory high-water behavior.

The optional browser extension remains 17/18, with an unexplained exact-window
setup refusal before its last ambiguity assertion. A focused baseline-only
probe passed and did not reproduce that failure. Keep #3681 visible; neither
this review nor that probe converts the failed candidate run into a pass.
The [worklog](rfc-3473-worklog.md#final-selected-slice-evidence--2026-09-09)
contains exact run links, provenance, cleanup, deferred scope, and the final
supplementary-evidence diff accounting. Review handoff concerns only the
maintainer-selected slice, not completion of RFC #3473.
