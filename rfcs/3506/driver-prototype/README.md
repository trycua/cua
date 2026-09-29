# RFC 3506 Driver-admitted single-key sidecar

This is a **focus-bound experiment, not a target-bound input capability**. Production sources are unchanged. `kwin_helper::available()` remains false. No default Cargo feature, runtime environment switch, installed daemon, CLI, or MCP surface enables this experiment.

## Artifacts and verification

- `driver.patch`: exact unified patch against `93f7afc89a09e7f9d39da22990e26036a74adfbd`, applied only to a scratch archive.
- `prototype.rs`: reviewable source of the module added by that patch.
- `sdk_harness.rs`: standalone example calling the normal typed `CuaDriver::press_key` once; no raw socket call or direct platform invocation.
- `prepare.py`: copy/archive, separately apply, compile, run non-desktop checks.
- `test_contract.py`: structural isolation check (observed red before patch creation, then green).

```sh
python3 rfcs/3506/driver-prototype/test_contract.py
python3 rfcs/3506/driver-prototype/prepare.py
```

Build flag is exactly `RUSTFLAGS='--cfg cua3506_prototype --check-cfg=cfg(cua3506_prototype)'`, with the existing `portal-input` Cargo feature. No new dependencies. The tiny compile-only addition to `PressKeyInput` carries `delivery_mode: foreground` through canonical SDK serialization and admission; without this addition the released typed key input cannot express that mode. Default builds exclude the field and experimental branch.

Binary:
`/home/netbos/.hermes/cache/scratch/cua3506-driver/source/libs/cua-driver/rust/target/debug/examples/rfc3506`

Build/test log:
`/home/netbos/.hermes/cache/scratch/cua3506-driver/build.log`

Verified: two non-desktop Rust checks; inactive-target negative control fails when the active guard is removed and passes after restoration; SDK harness self-check; patch applicability to untouched checkout; production source diff empty. Compilation retains existing dead-code/unused-assignment and `nom` future-compatibility warnings. Read-only `--identity` also successfully queried the live trusted KWin owner `:1.8`, including braced helper generation/window UUIDs. No input, activation, portal startup, or portal-readiness call was executed by the implementer.

## Exact parent protocol

`rfc3506 --self-check` never constructs a Driver runtime or opens a portal.
`rfc3506 --identity` performs only trusted D-Bus identity discovery and prints:

```json
{
  "owner": ":1.8",
  "snapshot": { "generation": "{UUID}", "windows": ["v1 objects plus internal_id"] },
  "now_ns": 123
}
```

For an **explicitly approved** parent experiment, use `rfc3506 --execute` with exactly one JSON object on stdin, then close stdin. Parent selects the synthetic target from its trusted identity snapshot, supplies supported KWin scripting activation before the call, and retains `op_seq` across any process failure. Do not resend/reconnect/rearm an uncertain operation.

```json
{
  "owner": ":1.8",
  "pid": 123,
  "token": 14,
  "generation": "{00000000-0000-0000-0000-000000000001}",
  "internal_id": "{00000000-0000-0000-0000-000000000002}",
  "deadline_ns": 999999999999999,
  "op_seq": 1,
  "key": "a",
  "precheck_gate": null,
  "postcheck_gate": null,
  "drop_ack": false
}
```

The values above are shape examples, **not live identities or an expiry recommendation**. `pid` is the target app PID, not KWin's PID. `deadline_ns` is the parent's absolute CLOCK_MONOTONIC deadline, not Unix time. Choose the experiment's explicit lease; there is no invented safe-latency threshold. UUID strings may be canonical or QString-braced and are compared exactly, never normalized.

The canonical call is:

```json
{
  "key": "a",
  "target": { "kind": "window", "pid": 123, "window_id": 14 },
  "delivery_mode": "foreground",
  "session": "rfc3506-one-shot"
}
```

The trusted expectation is armed out-of-band in the same process and is **not** caller-supplied authority in tool arguments. Only that exact single key/window foreground form is experimental. Modifiers, element/pixel-focus forms and mismatching foreground expectations reject. Background delivery and every other tool remain on their original paths. There is no global-input fallback, activation request, wtype call, AX action, or extra readiness round trip in this branch.

Output JSON contains `sdk_submission_ns`, `caller_ack_ns`, `operation` (`op_seq`, `submission_ns`, `may_start`, `emission_ns`, `worker_ack_ns`), canonical SDK `result`, and `no_retry:true`. All timestamps are CLOCK_MONOTONIC ns. `emission_ns` is immediately before the first reis key request; `worker_ack_ns` is after successful client flush, **not compositor receipt or app delivery**. Zero means that stage was not observed. The SDK may replace successful platform structured content with its normal ActionResult, so the separately retained operation report is authoritative for instrumentation. Successful output is `effect:unverifiable`, not confirmed application effect. Loss/error after `may_start` is unknown and non-retryable.

## Admission trace (real code path, not a bypass)

1. SDK generated `press_key(PressKeyInput)` → `invoke_typed`: validate input, serialize it, then normal `invoke`.
2. Same-process `NativeAbiDriver::invoke` → standard ABI asynchronous operation machinery → `DriverRuntime::invoke` / `invoke_with_context_and_evidence`; runtime lifecycle read lease and immutable compatibility authorization context remain in force.
3. `ToolRegistry::invoke_authorized` strips reserved caller fields, checks runtime suspension/revocation, resolves the tool, rejects non-object args, normalizes delivery and typed target, enforces delivery/target compatibility, then executes normal policy/permission/manifest admission.
4. Runtime session namespacing/ended-session checks, capture-scope enforcement, text-input overlap admission, active protected-resource adapters, authorized-dispatch commit and lifecycle dispatch admission all precede the platform invocation. Desktop-action coordinator and normal recording/history/cursor bookkeeping are unchanged. Host construction disables overlay and desktop environment preparation; it does not bypass admission or grant residual permissions. A parent policy/manifest refusal still wins before this experiment.
5. `PressKeyTool::invoke` consults the compile-only armed operation at its entry, **after** registry admission. It consumes that operation once, marks submission, and queues `Cmd::GuardedPressKey` on the existing bounded libei worker channel.
6. Existing worker negotiates/queues until its keyboard device is ready (or existing timeout). In `EisState::run_command`, after selecting the resumed keyboard/interface, the worker obtains a **new** trusted `GetIdentitySnapshot` and validates expected unique owner, target PID/token, helper generation, internal UUID, active true, minimized false and v1 snapshot invariants. The existing helper trust check proves matching `org.cua.KWinTarget` / `org.kde.KWin` unique owners, same UID, KWin process identity and GetVersion=1; the identity method is invoked on that unique owner, with no snapshot retry/cache. Owner is checked again afterward.
7. Worker rechecks local closure/deadline immediately before emission under an operation-local lock, marks `may_start`, timestamps, emits one press/release pair and frames on the same keyboard, checks flush, and returns its existing channel acknowledgement. Caller timeout/error closes the operation under that lock so a still-pending command cannot later start. There is no alternate route or retry.
8. Normal core execution-record projection/output validation and SDK normalization return the result. Harness retains the operation state, timestamps caller completion, closes the local operation and prints one JSON report.

## Real fault seams and limitations

- `--execute-closed` closes local authority before the canonical SDK call; worker input is never started by this branch.
- `precheck_gate` is an absolute **scratch** path prefix. Worker writes `<prefix>.reached` after keyboard readiness and waits for `<prefix>.release`. Parent switches to synthetic B using supported scripting, then creates the release file. The subsequent fresh snapshot must reject A as inactive before emission.
- `postcheck_gate` uses the same protocol **after** final identity/focus observation and before emission. Parent takeover here deliberately demonstrates the remaining global-input TOCTOU race. There is intentionally no extra recheck after this fault seam; normal runs have no seam wait. Closure/expiry are still checked afterward. Never interpret this seam's success as target safety.
- Gate files must not preexist; parent owns creation/cleanup in scratch. Waiting is bounded by the supplied operation deadline, not a separate guessed timeout.
- `drop_ack:true` really suppresses the existing worker-to-caller reply after execution. The receive channel disconnects, returning unknown after may-start. This is **worker acknowledgement loss**, not simulated loss of final stdout; parent can separately drop/kill its receiver to test outer caller acknowledgement loss.
- One armed operation per process is retained forever; no rearm/reconnect API. Parent must retain sequence/unknown state across process restarts. This is not durable daemon-wide deduplication.
- Local deadline/close belong to this one-shot operation, in addition to normal Driver admission. This is not a new delegated-session/remote-transport lease implementation, and does not continuously revalidate general Driver session revocation during input startup.
- Focus can change after snapshot and before KWin processes globally delivered libei input; closed windows/helper owner changes can likewise race the final observation. No userspace guard makes this atomic. `available()` therefore stays false; production enablement is explicitly not implemented.
- Input execution, pre/postcheck takeovers and lost-ack receipt still require parent-controlled live synthetic runs. Offline checks prove rejection logic/build/isolation, not desktop delivery.
