# One action and its next observation: SDK experiment

This opt-in example measures whether a host can return one element action and a fresh observation in one MCP call. It adds no production tool, generated contract, default behavior, or new grant path. [RFC #2794](https://github.com/trycua/cua/issues/2794) remains unresolved; this is narrower evidence for that discussion, not an implementation of its proposed pixel-change verification or the separate action-batch API.

The example creates a private same-process SDK runtime. The experimental `experiment_action_observe` request permits one `click`, `set_value`, or `type_text` with explicit PID, window ID, and an observed element token. It derives the following AX read from the same PID/window, requests no screenshot, and dispatches both children through ordinary `CuaDriver.call_tool` calls. Each child retains canonical argument validation and authorization. It never retries the action.

The response preserves `action_result` and `observation_result` independently. An action acknowledgment is not proof of its semantic effect. An observation failure must not prompt repeating an already dispatched action. The example has no settling or transactional guarantee; the caller owns record matching and independent expected-effect checks. This transport example does not introduce a production lifecycle identity or authorization model.

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

The four focused tests cover missing exact binding, same-owner observation derivation, invalid child-result envelopes, and an observation failure after exactly one action dispatch. They are contract checks, not substitutes for native effects.

For the optional live benchmark, use a Python environment with PyObjC and an Arc checkout providing its existing synthetic `benchmarks/fixture_form.py` (the trial used `74ffae1108b1cb4b1f6b161084af12646f544ba5`). The Arc fixture dependency supplies an application oracle; Arc does not deliver any input in this experiment. The SDK example host requires its ordinary macOS Accessibility grant. Do not run this live smoke merely to execute offline tests.

```sh
ARC_EVAL_SOURCE=/path/to/arc-cua ACTION_OBSERVE_BINARY="$PWD/target/debug/examples/action_observe_experiment" python crates/cua-driver-sdk/examples/action_observe/benchmark.py
```

Native evidence currently covers macOS only. The example uses shared SDK dispatch, but Windows/X11/Wayland behavior has not been live-qualified. Public composition, cross-platform acceptance, cancellation, result schemas, and rollout require separate maintainer decisions and the canonical platform matrices.
