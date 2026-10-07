# Experimental owned macOS supervision

A native text write normally waits for the window-change detector before returning. With `experimental-owned-supervision`, `dispatch_set_value` returns after actual input and foreground restoration. A runtime owner keeps the original detector and focus-protection lease alive in the background. Existing `set_value` behavior is unchanged.

The operation requires a named session, exact window ID and current element token. It supports native `AXTextField` controls; web surfaces and other roles refuse before input. Capacity is reserved before mutation. An uncertain input retains its receipt and must not be replayed blindly.

`get_action_supervision` reads the session-scoped receipt, including fresh original-foreground-window and physical-input checks. `activation_after_dispatch` records later activation notifications. Missing or negative guard evidence stops dependent input. Dispatch-time activation remains in the full observation even when restoration succeeds before return.

`fence_action_supervision` waits for the original observer; its timeout does not cancel that observer. `release_action_supervision` releases only terminal receipts. The owner holds at most 128 receipts; foreign sessions cannot read, fence or release them. IDs are opaque and not reused. Typed refusals include `pending`, `timeout` and `unavailable`.

**A receipt is not application commitment.** Every response leaves `application_commit` unverified. Advancing an independent text plan requires a qualified application's terminal transaction evidence, fresh field/record proof and intact foreground/input guards. An AX value echo alone is insufficient. The included AppKit fixture supplies those transaction acknowledgements; this PR does not add an adapter for arbitrary applications.

The SDK and CLI forward the compile-time feature. SDK shutdown closes admission and attempts a bounded drain; executor loss records interruption, not successful observation. Foreground restoration remains reactive and does not provide a separate desktop or input lane. General application, dispatch-time cancellation and concurrent-client qualification remain open.

## Build and checks

```sh
cargo +1.97.1 build --locked -p cua-driver --features experimental-owned-supervision
cargo +1.97.1 test --locked -p cua-driver-core -p platform-macos -p cua-driver-sdk --features cua-driver-sdk/experimental-owned-supervision --lib
```

Local checks pass: 875 core tests, 102 SDK tests and 473 macOS tests; six existing macOS tests remain ignored. Owner tests cover cancelled waiters, timeout retention, bounded capacity, scoped live guards, terminal release, runtime drain, panic/unpolled outcomes and executor loss. Native binding tests refuse missing window/token arguments before native work. Full desktop qualification remains outstanding.

The [qualification packet](qualification/README.md) uses the SDK example host, exact native bindings, independent transaction acknowledgements and fresh AX/foreground proof. It has no fixture-specific restoration hook. [RFC issue #4771](https://github.com/trycua/cua/issues/4771) and [RFC PR #4772](https://github.com/trycua/cua/pull/4772) track the lifecycle proposal; [research #110](https://github.com/open-horizon-labs/computer-use/pull/110) records the earlier prototype. This implementation stays experimental while the RFC and broader qualification remain open.
