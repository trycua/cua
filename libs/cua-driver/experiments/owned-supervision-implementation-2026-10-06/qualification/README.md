# Native qualification

This packet exercises the shipped experimental tools through the SDK example host. The host uses ordinary foreground restoration and retains the original native observer; it has no fixture-specific restoration hook. The fixture supplies immutable transaction acknowledgements that arbitrary applications do not necessarily expose.

## Matched comparison

The task writes two native fields. Before each input, the runner obtains fresh observations, binds the current window and element token, checks the same record, and arms a transaction. Application commitment requires the bound acknowledgement, fresh field/record proof and intact foreground/input guards. Cua's full observer remains owned after that milestone and is fenced separately.

Five alternating pairs include one warmup pair and four scored pairs. All ten trials pass value, foreground and competing-input checks. Cleanup restores the original foreground application.

| Scored median | Time |
| --- | ---: |
| Cua independently verified two-field commitment | 373 ms |
| Reference driver independently verified two-field commitment | 507 ms |
| Cua full supervision fence and final verification | 1,421 ms |

Cua uses upstream `12365b36d41417cae34eb0f90b49e8eaf023ac51`, this implementation, and the separate broad AX observation patch from #4762. The measured stacked source is `78a96c9ded11e09e2b29d753f383edec93c8e5e2`; the embedded backend is 0.34.0. The reference source is `74ffae1108b1cb4b1f6b161084af12646f544ba5`, with running server 0.1.1. The result establishes the combined path on this fixture; it does not isolate this PR's incremental contribution or predict arbitrary application or agent performance. Exact versions, preflight timestamps, binary identity and individual trials are in [combined-commit.json](combined-commit.json).

## Adversarial checks

[qualify.py](qualify.py) checks wrong transaction identity, changed record, first-write rejection, second-write rejection, delayed rejection, missing acknowledgement and delayed activation. Each refuses the unsafe continuation; delayed activation stops before the dependent second write. The observer is drained after refusal. Foreground restoration is reactive: the activation control can briefly activate the target and must not be interpreted as a separate input lane.

[receipts.py](receipts.py) checks actual pending receipts, zero-timeout refusal, pending-release refusal, foreign-session refusal, full fencing, terminal release and unavailability after release. A timed-out waiter leaves the observer alive.

[disconnect.py](disconnect.py) closes the host's input transport after native dispatch. Shutdown retains the observer and focus protection; a second host independently observes the resulting field and foreground state. A closed transport cannot report a receipt's final outcome to its former client.

These checks supplement 875 core, 102 SDK and 473 macOS tests, with six existing macOS tests ignored. Feature-enabled CLI and feature-disabled SDK/macOS checks also pass on Rust 1.97.1. The canonical cross-platform desktop matrix, dispatch-time cancellation, concurrent clients and real-application acknowledgement adapters remain unqualified.

## Reproduce

Build the SDK `owned_supervision_host` example with `experimental-owned-supervision`, pinned Rust 1.97.1, and release stripping disabled. Supply candidate paths and reference-provider configuration at runtime:

- `CUA_PROJECTION_SOURCE`: clean worktree containing the measured stack.
- `CUA_OBSERVATION_FIXTURE_SOURCE`: the observation-projection experiment directory from #4762.
- `CUA_OWNED_HOST`: absolute path to the freshly built SDK example.
- `REFERENCE_EVAL_SOURCE`: installed reference evaluation source containing `fixture_form`.
- `REFERENCE_MODULE`, `REFERENCE_UPSTREAM_REPO`, `REFERENCE_UPSTREAM_REF`: reference provider module and upstream identity.

Use a Python environment with PyObjC, AppKit and Quartz. Run `measure.py`, `qualify.py`, `receipts.py`, then `disconnect.py` sequentially. Do not measure during compilation or other desktop input. Preflight checks current upstream identities and running backend versions; upstream movement requires updating the pinned base and rebuilding before another comparison. Each script operates only owned synthetic fixtures and restores the original foreground application.
