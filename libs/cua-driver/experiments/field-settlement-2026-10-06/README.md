# Settle native text while retaining delayed-focus protection

Follow-up to #4776, tracking #4792. `dispatch_set_value` gains an opt-in `settle: true` argument under the existing default-off `experimental-owned-supervision` feature. It waits for the exact on-screen native text field to react and remain unchanged for 150 ms. The original focus-protection observer remains owned independently. Existing synchronous `set_value` and ordinary owned dispatch keep their waiting behavior.

The response distinguishes the field's `settlement`, full `supervision_status`, and `application_commit`. Field settlement never marks application commitment verified. A dependent application transaction still needs bound terminal acknowledgement and fresh field/record checks. A full protection fence remains an explicit separate operation.

The common tracker samples at 20 ms intervals, gives absent reaction a 600 ms budget, resets quiet time on every value change, and gives continued changes a two-second budget. Native AX calls can extend those budgets. A verified no-op can settle without claiming a reaction. Missing target/value evidence, a window leaving the screen, foreground/input interference, an unexpected window, or failed/lost observation refuses settlement. Every attempted write retains its receipt; an error is not permission to replay input.

## Evidence and remaining work

Final-source local gates pass: 879 core, 102 SDK and 473 macOS tests, with six existing macOS tests ignored; feature-enabled CLI and feature-disabled SDK/macOS checks; formatting and whitespace checks. A zero-quiet-period mutation fails two keeper tests. These offline results do not replace native qualification.

The first native control pilot passed no-op reporting, early rejection, early activation, no reaction and continuous changes. Closing a window exposed stale AX evidence: the field remained readable and was incorrectly reported settled. The implementation now requires fresh on-screen window evidence before input and throughout settling. **That fix still needs native requalification.** Raw prior-candidate outcomes are in [field-controls-pilot.json](field-controls-pilot.json); they are not qualification of the final candidate.

Two attempted comparisons were excluded: one detected competing physical input; the other failed foreground setup before text input. Both restored the original foreground application. No finished matched performance comparison is claimed.

The explicitly selected off-screen display now hosts the synthetic fixtures. They use a non-activating application policy, are positioned before being shown, and must pass a fresh WindowServer check proving the entire window lies on the owned virtual display before input. The controller never activates a foreground sentinel or restores the user's app. It removes only its owned display and independently verifies the original topology.

The off-screen pilot completed baseline and owned writes with bound application acknowledgements and fresh field proof, without activating the target or changing the user's foreground. It was excluded from performance qualification because physical input occurred during the owned protection fence. The subsequent no-op control returned `interrupted`, with `physical_input_unchanged: false`, while retaining its receipt. This exposes a concurrency limitation: the guard observes physical input across the session, including input unrelated to the off-screen target. These are interruption evidence, not qualified speed results. See [offscreen-results.json](offscreen-results.json) and [offscreen-controls.json](offscreen-controls.json). Earlier setup failures are preserved separately.

Ordinary text trials can run off-screen. Deliberate focus-steal controls still require a separate desktop because the virtual display shares macOS foreground focus. No guest was provisioned.

The remaining native gate comprises the same-build alternating baseline/owned/settled/reference comparison; re-running the six field controls after the visibility fix; wrong transaction/record, missing acknowledgement, first/second-write and late rejection controls; delayed activation after the early return; receipt lifetime and client disconnect. Those scripts are included, but their final-candidate results remain pending.

## Reproduce in an isolated macOS guest

Build the SDK `owned_supervision_host` example with the experimental feature and Rust 1.97.1. Supply `CUA_PROJECTION_SOURCE`, `CUA_OBSERVATION_FIXTURE_SOURCE`, `CUA_OWNED_HOST`, `REFERENCE_EVAL_SOURCE`, `REFERENCE_MODULE`, `REFERENCE_UPSTREAM_REPO` and `REFERENCE_UPSTREAM_REF` externally, as described in the parent [qualification packet](../owned-supervision-implementation-2026-10-06/qualification/README.md). Both candidates must be current and their running versions verified; upstream movement invalidates preflight until the candidate is rebuilt. The baseline, owned and settled native arms use the same freshly built binary and AX observation patch.

Run `measure.py`, `field_controls.py`, `qualify.py`, `receipts.py`, and `disconnect.py` sequentially in the guest. Each owns synthetic fixtures and restores the guest's original foreground. Do not run these on the user's desktop as an isolation fallback.

## Off-screen text trials

Supply `OFFSCREEN_MODEL_SOURCE` pointing to the existing display helper checkout, along with the build/provider variables above. Run `offscreen.py` and `offscreen_controls.py` sequentially. They refuse foreground or placement changes and exclude physical-input interference. Do not substitute the foreground harness if they refuse. Off-screen evidence does not establish safe concurrent use while the global physical-input guard remains unchanged.
