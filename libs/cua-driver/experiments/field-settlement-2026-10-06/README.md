# Settle native text while retaining delayed-focus protection

Follow-up to #4776, tracking #4792. `dispatch_set_value` gains an opt-in `settle: true` argument under the existing default-off `experimental-owned-supervision` feature. It waits for the exact on-screen native text field to react and remain unchanged for 150 ms. The original focus-protection observer remains owned independently. Existing synchronous `set_value` and ordinary owned dispatch keep their waiting behavior.

The response distinguishes the field's `settlement`, full `supervision_status`, and `application_commit`. Field settlement never marks application commitment verified. A dependent application transaction still needs bound terminal acknowledgement and fresh field/record checks. A full protection fence remains an explicit separate operation.

The common tracker samples at 20 ms intervals, gives absent reaction a 600 ms budget, resets quiet time on every value change, and gives continued changes a two-second budget. Native AX calls can extend those budgets. A verified no-op can settle without claiming a reaction. Missing target/value evidence, a window leaving the screen, foreground/input interference, an unexpected window, or failed/lost observation refuses settlement. Every attempted write retains its receipt; an error is not permission to replay input.

## Measured results

Latest upstream main `a7524cfd1` plus the experimental owned-supervision, field-settlement and broad AX-observation patches. All native arms use the same binary; the synchronous baseline is the patched build's ordinary `set_value`, not stock upstream. The reference source is `74ffae110`. Installed and running versions were checked before trials. Exact source, binary hash and check timestamps are in [offscreen-results.json](offscreen-results.json).

Two-field task: exact fresh bindings, per-field application transaction acknowledgement and final AX/record proof. One warm-up and four scored trials per arm, in alternating order. Median times:

| Path | Verified task | Task plus full protection fence |
| --- | ---: | ---: |
| Synchronous Cua baseline | 2,512 ms | 2,512 ms |
| Owned dispatch | 464 ms | 1,498 ms |
| Owned dispatch with field settlement | 855 ms | 1,684 ms |
| Reference driver | 493 ms | 493 ms; no Cua protection fence |

Owned dispatch was 5.4× faster than the synchronous baseline and 6% faster than the reference in this small sample. Field settlement was 2.9× faster than baseline but added 391 ms versus owned dispatch; it was 1.7× slower than reference. Settlement adds observable reaction/quiet-value evidence. It does not eliminate the full protection fence or outperform owned dispatch when a bound application acknowledgement already supplies stronger progress evidence. These scripted fixture results do not establish general agent performance. Raw ranges and medians are in [summary.json](summary.json).

All twenty trials preserved foreground and detected no competing physical input. Five final field controls passed: no-op without a false reaction, early rejection, absent reaction, continuous changes, and closed-window refusal. The closed-window test exposed that WindowServer visibility can outlive application-window membership; settling now also requires the bound window in the application's fresh AXWindows list. [Field controls](offscreen-controls.json).

Six final transaction guards passed: wrong transaction, changed record, first/second-write rejection, late rejection, and missing acknowledgement. Pending receipts survive a zero-timeout fence, cannot be released while pending, cannot be accessed from a foreign session, and become unavailable after terminal release. [Guard and receipt evidence](offscreen-results.json). Disconnect drained the owned observer before clean process exit (879 ms), with independently committed values and preserved foreground. [Disconnect evidence](offscreen-disconnect-results.json).

The fixtures use a non-activating application policy and are positioned before showing. Fresh WindowServer PID/window bounds must prove placement entirely on the owned virtual display before input. Neither harness activates a sentinel or restores the user's app. All final runs removed their owned display and verified original topology. Earlier setup, stale-window and physical-input failures remain preserved as pilots; they are excluded from current timings. Receipt-release controls require separate clients so the qualification host never fences already released IDs.

Local gates on latest upstream passed: 880 core, 102 SDK and 473 macOS tests; six existing macOS tests ignored. The earlier zero-quiet mutation failed two keeper tests. Final membership-change gates are recorded in the manifest.

## Remaining qualification

The session-wide physical-input guard still interrupts off-screen work when the user types elsewhere. Deliberate activation and concurrent-input focus-restoration tests require a separate desktop because a virtual display shares macOS focus. No guest was provisioned. Those final-candidate tests, the canonical desktop matrix and the RFC decision remain outstanding; this PR remains an experimental draft.

## Reproduce in an isolated macOS guest

Build the SDK `owned_supervision_host` example with the experimental feature and Rust 1.97.1. Supply `CUA_PROJECTION_SOURCE`, `CUA_OBSERVATION_FIXTURE_SOURCE`, `CUA_OWNED_HOST`, `REFERENCE_EVAL_SOURCE`, `REFERENCE_MODULE`, `REFERENCE_UPSTREAM_REPO` and `REFERENCE_UPSTREAM_REF` externally, as described in the parent [qualification packet](../owned-supervision-implementation-2026-10-06/qualification/README.md). Both candidates must be current and their running versions verified; upstream movement invalidates preflight until the candidate is rebuilt. The baseline, owned and settled native arms use the same freshly built binary and AX observation patch.

Run `measure.py`, `field_controls.py`, `qualify.py`, `receipts.py`, and `disconnect.py` sequentially in the guest. Each owns synthetic fixtures and restores the guest's original foreground. Do not run these on the user's desktop as an isolation fallback.

## Off-screen text trials

Supply `OFFSCREEN_MODEL_SOURCE` pointing to the existing display helper checkout, along with the build/provider variables above. Run `offscreen.py` and `offscreen_controls.py` sequentially. They refuse foreground or placement changes and exclude physical-input interference. Set `OFFSCREEN_QUALIFICATION_ONLY=1` for receipt/transaction checks, or `OFFSCREEN_DISCONNECT_ONLY=1` for transport-disconnect draining. Do not substitute the foreground harness if they refuse. Off-screen evidence does not establish safe concurrent use while the global physical-input guard remains unchanged.
