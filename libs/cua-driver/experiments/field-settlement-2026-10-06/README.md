# Settle native text while retaining delayed-focus protection

Follow-up to #4776, tracking #4792. `dispatch_set_value` gains an opt-in `settle: true` argument under the existing default-off `experimental-owned-supervision` feature. It waits for the exact on-screen native text field to react and remain unchanged for 150 ms. The original focus-protection observer remains owned independently. Existing synchronous `set_value` and ordinary owned dispatch keep their waiting behavior.

The response distinguishes the field's `settlement`, full `supervision_status`, and `application_commit`. Field settlement never marks application commitment verified. A dependent application transaction still needs bound terminal acknowledgement and fresh field/record checks. A full protection fence remains an explicit separate operation.

The common tracker samples at 20 ms intervals, gives absent reaction a 600 ms budget, resets quiet time on every value change, and gives continued changes a two-second budget. Native AX calls can extend those budgets. A verified no-op can settle without claiming a reaction. Missing target/value evidence, a window leaving the screen, foreground/input interference, an unexpected window, or failed/lost observation refuses settlement. Every attempted write retains its receipt; an error is not permission to replay input.

## Evidence and remaining work

Final-source local gates pass: 879 core, 102 SDK and 473 macOS tests, with six existing macOS tests ignored; feature-enabled CLI and feature-disabled SDK/macOS checks; formatting and whitespace checks. A zero-quiet-period mutation fails two keeper tests. These offline results do not replace native qualification.

The first native control pilot passed no-op reporting, early rejection, early activation, no reaction and continuous changes. Closing a window exposed stale AX evidence: the field remained readable and was incorrectly reported settled. The implementation now requires fresh on-screen window evidence before input and throughout settling. **That fix still needs native requalification.** Raw prior-candidate outcomes are in [field-controls-pilot.json](field-controls-pilot.json); they are not qualification of the final candidate.

Two attempted comparisons were excluded: one detected competing physical input; the other failed foreground setup before text input. Both restored the original foreground application. No finished matched performance comparison is claimed.

Host desktop trials stopped after the user requested isolation. Fresh discovery found no registered Cua Spaces, local Lume VMs or cached macOS images. A virtual display shares the macOS session's foreground and keyboard focus; it cannot isolate the foreground-protection tests. An isolated macOS guest is required to finish native qualification. No VM was provisioned.

The remaining native gate comprises the same-build alternating baseline/owned/settled/reference comparison; re-running the six field controls after the visibility fix; wrong transaction/record, missing acknowledgement, first/second-write and late rejection controls; delayed activation after the early return; receipt lifetime and client disconnect. Those scripts are included, but their final-candidate results remain pending.

## Reproduce in an isolated macOS guest

Build the SDK `owned_supervision_host` example with the experimental feature and Rust 1.97.1. Supply `CUA_PROJECTION_SOURCE`, `CUA_OBSERVATION_FIXTURE_SOURCE`, `CUA_OWNED_HOST`, `REFERENCE_EVAL_SOURCE`, `REFERENCE_MODULE`, `REFERENCE_UPSTREAM_REPO` and `REFERENCE_UPSTREAM_REF` externally, as described in the parent [qualification packet](../owned-supervision-implementation-2026-10-06/qualification/README.md). Both candidates must be current and their running versions verified; upstream movement invalidates preflight until the candidate is rebuilt. The baseline, owned and settled native arms use the same freshly built binary and AX observation patch.

Run `measure.py`, `field_controls.py`, `qualify.py`, `receipts.py`, and `disconnect.py` sequentially in the guest. Each owns synthetic fixtures and restores the guest's original foreground. Do not run these on the user's desktop as an isolation fallback.
