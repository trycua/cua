# One action and its next observation: SDK experiment

This opt-in example measures whether a host can return one element action and a fresh observation in one MCP call. It adds no production tool, generated contract, default behavior, or new grant path. [RFC #2794](https://github.com/trycua/cua/issues/2794) remains unresolved; this is narrower evidence for that discussion, not an implementation of its proposed pixel-change verification or the separate action-batch API.

The host defaults to a private same-process SDK runtime. Trusted host configuration `ACTION_OBSERVE_BACKEND=daemon` instead uses supported `CuaDriver::connect(None)` against the existing daemon, without changing its grants/settings or owning its lifecycle. Initialization returns bounded `driver.metadata()` receipt separately from client compilation version. The experimental `experiment_action_observe` request permits one `click`, `set_value`, or `type_text` with explicit PID, window ID, and an observed element token. It derives the following AX read from the same PID/window, requests no screenshot, and dispatches both children through ordinary `CuaDriver.call_tool` calls. Each child retains canonical argument validation and authorization. It never retries the action.

The response preserves `action_result` and `observation_result` independently. An action acknowledgment is not proof of its semantic effect. An observation failure must not prompt repeating an already dispatched action. The example has no settling or transactional guarantee; the caller owns record matching and independent expected-effect checks. This transport example does not introduce a production lifecycle identity or authorization model.

## Reusable opt-in SDK operation

[`operation.rs`](operation.rs) is a callable composition recipe taking an existing `CuaDriver` handle. The example host calls the same helper; consumers can import the module alongside their own SDK host without starting this example's MCP transport:

```rust
#[path = "action_observe/operation.rs"]
mod operation;

let cancellation = operation::Cancellation::default();
let result = operation::action_observe(
    &driver,
    serde_json::json!({"tool":"set_value","arguments":exact_observed_arguments}),
    operation::Options::default(),
    cancellation.clone(),
).await;
```

The caller supplies exact observed arguments and owns the existing SDK driver's lifecycle. Options bound action and read waits to positive durations no longer than 60 seconds (defaults: 30 and 5 seconds). Calling `cancellation.cancel()` before dispatch sends no input. Cancellation or timeout after dispatch leaves effects unknown and never causes replay; dropping the future does not undo an OS action. Cancellation after a receipt preserves that receipt and can skip the read. An interrupted or wrong-owner read makes `observation_available` false. Availability requires the exact requested PID/window, a nonempty snapshot ID, and an elements array; it does not establish expected-effect or whole-task success. Refused and pending child results remain intact. `child_dispatches` records the children actually invoked, independently of visible request count.

This module remains an opt-in example rather than a generated public SDK method or production MCP tool. Public promotion and default behavior remain maintainer decisions under the RFC.

## Actual synthetic trial

On macOS 27.0.1, the source baseline was Cua Driver 0.34.0 at `0b90b6f4a`. Ten alternating trials used owned shown AppKit forms, public MCP calls, exact current tokens, and independent app state. All five standalone and five composite form runs filled two fields and enabled Subscribe without submitting. Each condition executed the same six canonical registry calls.

| Three-action form loop, excluding the initial read | Standalone calls | Composite calls |
|---|---:|---:|
| Independently correct outcomes | 5/5 | 5/5 |
| LLM-visible tool calls | 6 | 3 |
| Median summed MCP time | 3,694.77 ms | 3,703.12 ms |
| Median scripted loop time | 3,695.46 ms | 3,703.33 ms |

The measured improvement is fewer visible calls, not faster native execution. The composite was about 0.23% slower in summed MCP latency, which is within this small trial's noise. Including the common initial read changes seven visible calls to four. These scripted timings omit model reasoning and do not establish agent-loop latency, token savings, or autonomous accuracy. A separate [one-pair live agent pilot](agent_pilot.json) completed both conditions with current tokens and independent proof: six calls took 29.25 seconds versus three calls taking 14.57 seconds, while driver time was 3.35 versus 3.42 seconds. Intervening analysis and scheduling were uncontrolled; this demonstrates the call-count mechanism, not a reliable latency multiplier. The full child outcomes also increase response-envelope complexity; no response-size saving is claimed.

[Sanitized results](results.json) contain timings, counts, version, executable digest, and synthetic field values only. Global app menus, screenshots, and private host inventories are not retained. No virtual display, VM, GPU, real user document, authenticated browser, or installed daemon modification was used.

## Reproduce

From `libs/cua-driver/rust`:

```sh
cargo test --locked -p cua-driver-sdk --example action_observe_experiment
cargo build --locked -p cua-driver-sdk --example action_observe_experiment
```

Eleven focused Rust tests cover missing exact binding, same-owner observation derivation and validation, invalid child-result envelopes, preserved typed refusals/pending receipts, cancellation before/during input, and observation timeout/failure after exactly one action dispatch. Six Python gateway tests exercise wrong-owner/window, stale/duplicate tokens, changed literals, unsupported argument expansion, and input replay; the full gateway test checks no invalid request reaches the child driver. They are contract checks, not substitutes for native effects.

For the optional live benchmark, use a Python environment with PyObjC and an Arc checkout providing its existing synthetic `benchmarks/fixture_form.py` (the trial used `74ffae1108b1cb4b1f6b161084af12646f544ba5`). The Arc fixture dependency supplies an application oracle; Arc does not deliver any input in this experiment. The SDK example host requires its ordinary macOS Accessibility grant. Do not run this live smoke merely to execute offline tests.

```sh
ARC_EVAL_SOURCE=/path/to/arc-cua ACTION_OBSERVE_BINARY="$PWD/target/debug/examples/action_observe_experiment" python crates/cua-driver-sdk/examples/action_observe/benchmark.py
```

Native evidence currently covers macOS only. The example uses shared SDK dispatch, but Windows/X11/Wayland behavior has not been live-qualified. Public composition, cross-platform acceptance, result schemas, and rollout require separate maintainer decisions and the canonical platform matrices.

## Repeated real tool-agent qualification

`run_agent_trials.py` runs fresh owned fixtures in alternating order through the callable SDK helper and the same connected signed daemon for both conditions. The agent sees only `agent_gateway.py` tools; that gateway enforces owned PID/window, current tokens, the three permitted literal actions, and at most one attempt per field. Public `list_windows` discovery excludes hidden shadow windows, and setup requires the three actual controls. The runner fails preflight on a locked session, no active display, outdated release/source, or a mismatched running daemon metadata receipt. It retains actual Codex JSONL usage, overall CLI turn events, completed MCP item counts, visible gateway calls, and canonical child dispatch counts separately. The CLI's overall turn count does not establish provider request count.

```sh
ACTION_OBSERVE_BINARY="$PWD/target/debug/examples/action_observe_experiment" ACTION_OBSERVE_PYTHON=/path/to/pyobjc-python ARC_EVAL_SOURCE=/path/to/arc-cua python crates/cua-driver-sdk/examples/action_observe/run_agent_trials.py /path/to/evidence --pairs 5 --model gpt-6-sol
```

[Nine current preparation attempts](preparation_failures.json) are excluded from scoring: missing MCP environment, read-only tool approval denial, SDK discovery schema mismatch, and empty AX snapshots all caused safe stops without input. Independent session metadata then showed the Mac locked with no active displays; current repeated success, latency, and token results remain unavailable until an active unlocked session permits qualification. No session unlock or display manipulation is attempted by the harness. The historical scripted trial and single pilot above remain the only measured successful-loop evidence currently retained.
