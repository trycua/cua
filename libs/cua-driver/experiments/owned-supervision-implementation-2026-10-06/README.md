# Owned supervision implementation tracker

First implementation increment for [RFC issue #4771](https://github.com/trycua/cua/issues/4771) and [RFC PR #4772](https://github.com/trycua/cua/pull/4772). This branch is a draft, submitted at the contributor's request while the RFC remains under review. It does not enable the measured asynchronous text path or change existing action behavior.

The feature `experimental-owned-supervision` adds a bounded runtime owner in core and a macOS `Snapshot::supervise_owned` transfer. Reserve capacity before input; transfer the original native snapshot and its protection lease without an intervening await after dispatch. Receipt waiters own receivers, while the observer task owns its finish guard and protection future. Waiter timeout/cancellation cannot cancel that task. Orderly runtime shutdown must close admission and drain the owner before destroying the executor. Lost executor ownership records `interrupted`, not finished.

The owner stores opaque random receipt IDs, trusted runtime scope keys and observation status/counts. It does not store supplied values, element tokens, app names, window titles, screenshots or application transaction payloads. Foreign scopes cannot read, fence or release a receipt. Pending receipts cannot be released; terminal receipts consume bounded capacity until explicitly released. Automatic retention/expiry and transport projections await the RFC decision.

The macOS adapter reports whether polling actually ran, foreground-change evidence and the count of new windows. It does not assert value commitment. An unpolled or panicked observer records failure. No research environment switch, global task vector or known-sentinel restoration hook is included.

## Implementation status

- [x] Bounded admission before mutation, opaque receipt identity, trusted scope isolation and explicit states.
- [x] Observer ownership independent of fences, timeouts and cancelled waiters.
- [x] Closed admission plus bounded, repeatable runtime drain.
- [x] Explicit unpolled/panic/interrupted outcomes; no later success conceals earlier failure.
- [x] Transfer of the unchanged macOS snapshot/protection lease to the owner.
- [ ] Wire ownership into admitted text dispatch and release input coordinators only after actual dispatch/focus handling.
- [ ] Add SDK/CLI/MCP receipt read/fence surfaces and synchronous compatibility fixtures.
- [ ] Wire runtime/session shutdown, retention and receipt eviction policy.
- [ ] Add a qualified real application transaction-evidence adapter and dependent-plan executor.
- [ ] Qualify ordinary foreground restoration and user/other-agent interference without the private research helper.
- [ ] Run end-to-end native tests of the integrated implementation and required desktop qualification before release.

## Local checks

From `libs/cua-driver/rust`, using the pinned Rust 1.97.1 toolchain:

```sh
cargo test --locked -p cua-driver-core -p platform-macos --features platform-macos/experimental-owned-supervision --lib
cargo fmt --check -p cua-driver-core -p platform-macos
```

Eight owner tests cover waiter cancellation with a real lease-drop signal, capacity, foreign scopes, pending-release refusal, ID non-reuse, drain/fence timeouts, abandoned reservations, panic/unpolled outcomes, mixed success/failure and executor shutdown. The private native experiment's performance results belong to the research packet; they must not be attributed to this first owner increment.
