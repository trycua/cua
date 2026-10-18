# Native jev-use intermediate verification proof

Sanitized text proof for the native recipe change that ends whole-task waits
after an independently witnessed counter increment or Large radio selection.
This is not a new Jev or provider benchmark. No issue-closing claim is made.
Refs [#4336](https://github.com/trycua/cua/issues/4336) as complementary
timing work.

## 2026-10-18 maintenance verification

Refreshed onto upstream `b286bea824eef1a3a1c154ab140223a7b97e0048` with an
additive merge. The snapshot compatibility reads from #4913 remain intact;
the intermediate-effect policy stays in the example runners above Driver.

Offline verification with locked dependencies:

- Python 3.12: 255 tests, one skipped; no failures.
- TypeScript: 123 tests, no failures; typecheck passed.
- Both chooser CLIs: mock default and native/v2 fixtures passed.
- `verify_native_waits.py --baseline-sha b286bea824eef1a3a1c154ab140223a7b97e0048`:
  nine cells and 18 verified terminals; virtual waits 25.5s baseline / 9.3s candidate.
- The current-main and original-baseline poller AST hashes both equal
  `bf6c63164a59d3c3b5cbf72fe859e9a23381ec0007fae51f2a70068074e933d9`.

The suites exercise delayed/missing effects, exact increment validation,
terminal success/refutation priority, oracle errors, conservative unsupported
and exhausted-budget waits, and the runner stopping before a dependent
mutation after timeout. No new GUI run or provider request was made. The live
measurements below are historical September evidence and do not certify the
refreshed head or the current released Driver. Exact-head CI and maintainer
review are separate gates recorded on PR #4368.

## Historical September identities

| Item | Value |
| --- | --- |
| Immutable poller baseline | `62cdfbcd9356cf2e1b47ab09fee94eddd1d881cc` |
| Fresh package base | `0f29c142d7fe3e05ea0ce276cee11b3a9725ba01` |
| Candidate source | original live-tested six-file candidate; production bytes unchanged on the fresh package base |
| Released Cua Driver | 0.30.4 |
| Offline runtime | Python 3.12.9, macOS, deterministic mock chooser |

`poll_oracle` at the fresh base is byte-identical to the immutable baseline.
Production candidate files were not rewritten for this proof. No new Jev calls
were made; the mock path constructed no paid provider client.

## Offline policy corpus

`verify_native_waits.py` compiles baseline `poll_oracle` from Git and runs the
production candidate poller. Nine delayed-effect cells (18 terminals) all
verify. Virtual policy latency is 25.5s baseline versus 9.3s candidate. Those
numbers are fake-clock sleeps, not live or model time.

## Live Python / macOS A/B

Four interleaved baseline/candidate pairs per task, task order rotated, on
fresh fixture snapshots. 24 trials completed; 56 dispatched mutations have
successful post-dispatch receipts; every trial has a final window capture.
Focus monitors recorded 4,605 samples with zero fixture-foreground samples and
zero activation notifications. Sampling does not prove the absence of every
transient focus event.

| Task | Baseline median task | Candidate median task |
| --- | ---: | ---: |
| counter | 10.770s | 6.444s |
| choose-size | 7.564s | 5.485s |
| save-note | 8.148s | 8.137s |

Four trials per cell do not establish a general performance guarantee.

Counter pair 3 is retained: candidate task time was 13.083s versus baseline
10.889s despite removing 4.050s of verification waiting. Measured driver-call
latency increased on that pair. The OS, daemon, or delivery cause was not
instrumented and is unproved.

## Fixture and monitor limits

The live harness used a patched accessory AppKit fixture launched with
`open -g -n -W`, a 0.5s startup allowance, an owned focus monitor, and
in-process scoped driver calls. That is not a claim that the stock repository
fixture is canonical focus proof.

## Gaps

Python/macOS live only. TypeScript has unit and integration coverage, not a
new native live pass. Other operating systems, generic real-app
postconditions, operator concurrency, and delayed-effect live fixtures remain
unproved. The fixture oracle does not demonstrate application durability or
every downstream side effect.
